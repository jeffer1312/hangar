//! Encaminha `127.0.0.1:<porta local>` deste computador para `localhost:<porta>` da máquina de um servidor, pelo
//! túnel `/api/port-tunnel`. Cada conexão local é um WebSocket próprio, aberto pela rota em uso naquele instante.
use std::{collections::HashMap, io, net::Ipv4Addr, sync::{Arc, Mutex}, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWrite, AsyncWriteExt}, net::{TcpListener, TcpStream}, runtime::Handle,
    task::{JoinHandle, JoinSet}, time::timeout};
use crate::{api::{Api, route::blocked_url}, ws};

/// Abaixo do teto de 1 MiB por mensagem do servidor, e o que fica em memória por sentido.
const CHUNK: usize = 64 * 1024;
const MAX_FRAME: usize = 1024 * 1024;
/// O servidor pinga a cada 25 s: sem quadro nenhum nesse tempo, a conexão morreu no caminho.
const IDLE: Duration = Duration::from_secs(60);
/// Quantas portas acima da pedida se tenta antes de desistir.
const SEARCH: u16 = 100;

/// Último problema de um encaminhamento, para a tela dizer; o texto é da tela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardError {
    /// Rota do Connect sem credencial de aparelho: a senha do dono não vai.
    Blocked,
    /// Recusa HTTP do aperto de mão: 502 nada escutando, 422 porta recusada, 403 sem token...
    Refused(u16),
    /// O aperto de mão não chegou ao fim (rede, TLS, tempo).
    Network,
    /// A conexão já aberta caiu sem o fechamento do servidor.
    Dropped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardState { pub port: u16, pub local: u16, pub error: Option<ForwardError> }

type Shared = Arc<Mutex<Option<ForwardError>>>;

struct Forward { local: u16, error: Shared, task: Option<JoinHandle<()>> }

// Abortar o laço solta a escuta e o `JoinSet` dele, que aborta cada conexão viva.
impl Drop for Forward { fn drop(&mut self) { if let Some(task) = &self.task { task.abort(); } } }

/// Um por app; a chave é a identidade do servidor (`Api::identity`) e a porta remota.
#[derive(Default)]
pub struct PortForwards { forwards: HashMap<(String, u16), Forward>, closing: Vec<JoinHandle<()>> }

/// Quanto `open` espera a escuta de um encaminhamento recém-parado soltar a porta.
const RELEASE: Duration = Duration::from_millis(200);

impl PortForwards {
    /// Devolve a porta local; a mesma porta do mesmo servidor de novo devolve a que já existe.
    /// `token` é a senha do dono do servidor de `api`, como nos outros sockets.
    pub fn open(&mut self, runtime: &Handle, api: &Api, token: &str, port: u16) -> io::Result<u16> {
        let key = (api.identity(), port);
        match self.forwards.get(&key) {
            // Escuta que morreu (runtime fechando, descritor) não pode seguir dizendo que encaminha.
            Some(forward) if forward.task.as_ref().is_some_and(JoinHandle::is_finished) => { self.forwards.remove(&key); }
            Some(forward) => return Ok(forward.local),
            None => {}
        }
        // Parar e Encaminhar de novo: o abort solta a porta só quando o runtime roda a tarefa; sem esperar,
        // a porta pulava para a seguinte sem motivo.
        let deadline = std::time::Instant::now() + RELEASE;
        self.closing.retain(|task| !task.is_finished());
        while !self.closing.is_empty() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
            self.closing.retain(|task| !task.is_finished());
        }
        let listener = bind_from(port)?;
        let local = listener.local_addr()?.port();
        let error = Shared::default();
        let task = runtime.spawn(listen(listener, api.clone(), token.to_owned(), port, error.clone()));
        self.forwards.insert(key, Forward { local, error, task: Some(task) });
        Ok(local)
    }

    pub fn close(&mut self, server: &str, port: u16) {
        let Some(mut forward) = self.forwards.remove(&(server.to_owned(), port)) else { return };
        if let Some(task) = forward.task.take() {
            task.abort();
            self.closing.push(task);
        }
    }

    /// Encaminhamentos de `server`, por porta remota.
    pub fn list(&self, server: &str) -> Vec<ForwardState> {
        let mut list: Vec<_> = self.forwards.iter().filter(|((id, _), _)| id == server)
            .map(|((_, port), f)| ForwardState { port: *port, local: f.local, error: *lock(&f.error) }).collect();
        list.sort_by_key(|f| f.port);
        list
    }
}

fn lock(error: &Shared) -> std::sync::MutexGuard<'_, Option<ForwardError>> { error.lock().unwrap_or_else(|e| e.into_inner()) }

/// Só `127.0.0.1`: o encaminhamento é deste computador, nunca da rede dele.
fn bind_from(port: u16) -> io::Result<std::net::TcpListener> {
    if port == 0 { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut last = io::Error::from(io::ErrorKind::AddrInUse);
    for candidate in port..=port.saturating_add(SEARCH) {
        // No Windows o bind em 127.0.0.1 vence um servidor local em 0.0.0.0 na mesma porta e o tapa
        // calado: quem já atende ali deixa a porta ocupada.
        let here = (Ipv4Addr::LOCALHOST, candidate).into();
        if std::net::TcpStream::connect_timeout(&here, Duration::from_millis(100)).is_ok() {
            last = io::ErrorKind::AddrInUse.into();
            continue;
        }
        match std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, candidate)) {
            Ok(listener) => { listener.set_nonblocking(true)?; return Ok(listener); }
            Err(error) => last = error,
        }
    }
    Err(last)
}

async fn listen(listener: std::net::TcpListener, api: Api, token: String, port: u16, error: Shared) {
    let Ok(listener) = TcpListener::from_std(listener) else {
        *lock(&error) = Some(ForwardError::Network);
        return;
    };
    let path = format!("/api/port-tunnel?port={port}");
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((tcp, _)) => {
                    // A rota muda (rede local, Tailscale, Connect): lida de novo a cada conexão.
                    let url = ws::text_url(&api, &path, &token);
                    if url == blocked_url() { *lock(&error) = Some(ForwardError::Blocked); continue; }
                    let _ = tcp.set_nodelay(true);
                    connections.spawn(connection(tcp, url, api.identity(), error.clone()));
                }
                // Sem descritor livre, por exemplo: não girar em falso.
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            },
            Some(_) = connections.join_next() => {}
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum End { Closed, Dropped, Local }

async fn connection(tcp: TcpStream, url: url::Url, identity: String, error: Shared) {
    let socket = match ws::handshake(url, Some(identity)).await {
        Ok(socket) => socket,
        Err(ws::Error::Http(status)) => { *lock(&error) = Some(ForwardError::Refused(status)); return; }
        Err(_) => { *lock(&error) = Some(ForwardError::Network); return; }
    };
    *lock(&error) = None;
    let end = relay(socket, tcp).await;
    if end == End::Dropped { *lock(&error) = Some(ForwardError::Dropped); }
}

/// Cada sentido lê um pedaço e só lê o próximo depois de escrevê-lo do outro lado: a contrapressão do TCP
/// atravessa o túnel e nada se acumula na memória.
async fn relay(socket: reqwest::Upgraded, tcp: TcpStream) -> End {
    let (mut ws_read, ws_write) = tokio::io::split(socket);
    let writer = tokio::sync::Mutex::new(ws_write);
    let (mut tcp_read, mut tcp_write) = tcp.into_split();
    let down = async {
        // Continuação (0) herda o tipo do primeiro quadro; texto não é do protocolo.
        let mut kind = 0;
        loop {
            let Ok(Ok(frame)) = timeout(IDLE, ws::read_frame_as(&mut ws_read, false, MAX_FRAME)).await else { return End::Dropped };
            match frame.opcode {
                8 => return End::Closed,
                // Escrita ocupada já é tráfego para o servidor; esperar por ela pararia esta leitura.
                9 => if let Ok(mut writer) = writer.try_lock() {
                    let Ok(frame) = masked(10, &frame.data) else { return End::Dropped };
                    if writer.write_all(&frame).await.is_err() { return End::Dropped; }
                },
                10 => {}
                opcode => {
                    if opcode != 0 { kind = opcode; }
                    if kind == 2 && tcp_write.write_all(&frame.data).await.is_err() { return End::Local; }
                }
            }
        }
    };
    let up = async {
        let mut buf = vec![0u8; CHUNK];
        loop {
            match tcp_read.read(&mut buf).await {
                // Mensagem binária vazia = o cliente terminou de escrever; a resposta continua chegando.
                Ok(0) => {
                    if send(&writer, 2, &[]).await.is_err() { return End::Dropped; }
                    return std::future::pending().await;
                }
                Ok(n) => if send(&writer, 2, &buf[..n]).await.is_err() { return End::Dropped; },
                Err(_) => return End::Local,
            }
        }
    };
    let end = tokio::select! { end = down => end, end = up => end };
    if end == End::Local { let _ = timeout(Duration::from_secs(1), send(&writer, 8, &1000u16.to_be_bytes())).await; }
    end
}

fn masked(opcode: u8, data: &[u8]) -> io::Result<Vec<u8>> {
    ws::client_frame(opcode, data).map_err(|_| io::Error::other("rng"))
}

/// Sem prazo de escrita: servidor lento segura o envio (contrapressão); servidor morto cai pelo `IDLE` da leitura.
async fn send(writer: &tokio::sync::Mutex<impl AsyncWrite + Unpin>, opcode: u8, data: &[u8]) -> io::Result<()> {
    let frame = masked(opcode, data)?;
    let mut writer = writer.lock().await;
    writer.write_all(&frame).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::Future, net::SocketAddr, sync::atomic::{AtomicUsize, Ordering}};
    use tokio::io::AsyncReadExt;

    const WAIT: Duration = Duration::from_secs(5);

    /// Lê o pedido, confere o caminho do túnel e responde `status` (101 completa o aperto de mão).
    async fn accept(socket: &mut TcpStream, port: u16, status: u16) {
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") { request.push(socket.read_u8().await.unwrap()); }
        let request = String::from_utf8(request).unwrap();
        assert!(request.starts_with(&format!("GET /api/port-tunnel?port={port}&token=t HTTP/1.1")), "{request}");
        if status != 101 {
            socket.write_all(format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
            return;
        }
        let key = request.lines().find_map(|line| line.split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("sec-websocket-key")).map(|(_, v)| v.trim())).unwrap();
        socket.write_all(format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n\r\n", ws::accept_key(key)).as_bytes()).await.unwrap();
    }

    /// Quadro do servidor, sem máscara e sem prazo (o de `ws` tem 5 s e o teste de contrapressão para mais que isso).
    async fn server_frame(socket: &mut TcpStream, opcode: u8, data: &[u8]) -> io::Result<()> {
        let mut frame = vec![0x80 | opcode];
        match data.len() {
            n @ 0..=125 => frame.push(n as u8),
            n @ 126..=65535 => { frame.push(126); frame.extend_from_slice(&(n as u16).to_be_bytes()); }
            n => { frame.push(127); frame.extend_from_slice(&(n as u64).to_be_bytes()); }
        }
        frame.extend_from_slice(data);
        socket.write_all(&frame).await
    }

    /// Servidor de túnel falso: cada conexão aceita roda `handler`.
    async fn fake_server<F, Fut>(handler: F) -> SocketAddr
    where F: Fn(TcpStream) -> Fut + Send + 'static, Fut: Future<Output = ()> + Send + 'static {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { while let Ok((socket, _)) = listener.accept().await { tokio::spawn(handler(socket)); } });
        addr
    }

    /// Devolve cada mensagem binária recebida até o fechamento.
    async fn echo(mut socket: TcpStream, port: u16) {
        accept(&mut socket, port, 101).await;
        while let Ok(frame) = ws::read_client_frame(&mut socket).await {
            if frame.opcode != 2 { break; }
            if server_frame(&mut socket, 2, &frame.data).await.is_err() { break; }
        }
    }

    fn api(addr: SocketAddr) -> Api { Api::new(&format!("http://{addr}"), "t").unwrap() }

    /// Porta remota livre aqui, para a local sair igual a ela.
    fn free_port() -> u16 { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }

    async fn read_to_end(client: &mut TcpStream) -> Vec<u8> {
        let mut data = Vec::new();
        timeout(WAIT, client.read_to_end(&mut data)).await.expect("a conexão local fecha").ok();
        data
    }

    async fn error_of(forwards: &PortForwards, server: &str, port: u16) -> ForwardError {
        timeout(WAIT, async { loop {
            if let Some(error) = forwards.list(server).iter().find(|f| f.port == port).and_then(|f| f.error) { return error; }
            // estavel: intervalo da consulta até o erro aparecer; o teto é o `timeout(WAIT)` em volta
            tokio::time::sleep(Duration::from_millis(10)).await;
        } }).await.expect("o erro aparece")
    }

    #[test]
    fn busy_local_port_moves_to_the_next_free_one() {
        let busy = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = busy.local_addr().unwrap().port();
        let local = bind_from(port).unwrap().local_addr().unwrap();
        assert_eq!(local.ip(), Ipv4Addr::LOCALHOST);
        assert!(local.port() > port && local.port() <= port.saturating_add(SEARCH), "{port} -> {local}");
        let free = free_port();
        assert_eq!(bind_from(free).unwrap().local_addr().unwrap().port(), free);
        assert_eq!(bind_from(0).unwrap_err().kind(), io::ErrorKind::InvalidInput);
    }

    #[tokio::test]
    async fn same_port_twice_returns_the_same_local_port_and_close_drops_connections() {
        let port = free_port();
        let addr = fake_server(move |socket| echo(socket, port)).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = forwards.open(&Handle::current(), &api, "t", port).unwrap();
        assert_eq!(local, port);
        assert_eq!(forwards.open(&Handle::current(), &api, "t", port).unwrap(), local);
        assert_eq!(forwards.list(&api.identity()), vec![ForwardState { port, local, error: None }]);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        client.write_all(b"ping").await.unwrap();
        let mut reply = [0; 4];
        timeout(WAIT, client.read_exact(&mut reply)).await.unwrap().unwrap();
        assert_eq!(&reply, b"ping");
        forwards.close(&api.identity(), port);
        assert!(forwards.list(&api.identity()).is_empty());
        assert_eq!(read_to_end(&mut client).await, b"");
        timeout(WAIT, async { while TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.is_ok() {
            // estavel: intervalo da consulta até a escuta fechar; o teto é o `timeout(WAIT)` em volta
            tokio::time::sleep(Duration::from_millis(10)).await;
        } }).await.expect("a escuta local fecha");
    }

    #[tokio::test]
    async fn two_simultaneous_connections_on_one_forward() {
        let port = free_port();
        let addr = fake_server(move |socket| echo(socket, port)).await;
        let mut forwards = PortForwards::default();
        let local = forwards.open(&Handle::current(), &api(addr), "t", port).unwrap();
        let mut a = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        let mut b = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        a.write_all(b"from-a").await.unwrap();
        b.write_all(b"from-b").await.unwrap();
        let (mut ra, mut rb) = ([0; 6], [0; 6]);
        timeout(WAIT, async { tokio::try_join!(a.read_exact(&mut ra), b.read_exact(&mut rb)) }).await.unwrap().unwrap();
        assert_eq!((&ra, &rb), (b"from-a", b"from-b"));
    }

    #[tokio::test]
    async fn server_close_closes_the_local_socket_without_error() {
        let port = free_port();
        let addr = fake_server(move |mut socket| async move {
            accept(&mut socket, port, 101).await;
            server_frame(&mut socket, 2, b"bye").await.unwrap();
            server_frame(&mut socket, 8, &1000u16.to_be_bytes()).await.unwrap();
            // Fica de pé: quem fecha o socket local é o Close, não a queda do TCP.
            std::future::pending::<()>().await;
        }).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = forwards.open(&Handle::current(), &api, "t", port).unwrap();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        assert_eq!(read_to_end(&mut client).await, b"bye");
        assert_eq!(forwards.list(&api.identity())[0].error, None);
    }

    #[tokio::test]
    async fn server_dropping_closes_the_local_socket_and_says_dropped() {
        let port = free_port();
        let addr = fake_server(move |mut socket| async move { accept(&mut socket, port, 101).await; }).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = forwards.open(&Handle::current(), &api, "t", port).unwrap();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        assert_eq!(read_to_end(&mut client).await, b"");
        assert_eq!(error_of(&forwards, &api.identity(), port).await, ForwardError::Dropped);
    }

    #[tokio::test]
    async fn handshake_refusal_closes_the_local_socket_and_keeps_the_status() {
        let port = free_port();
        let addr = fake_server(move |mut socket| async move { accept(&mut socket, port, 502).await; }).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = forwards.open(&Handle::current(), &api, "t", port).unwrap();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        assert_eq!(read_to_end(&mut client).await, b"");
        assert_eq!(error_of(&forwards, &api.identity(), port).await, ForwardError::Refused(502));
    }

    #[tokio::test]
    async fn local_half_close_sends_an_empty_binary_and_the_reply_still_arrives() {
        let port = free_port();
        let addr = fake_server(move |mut socket| async move {
            accept(&mut socket, port, 101).await;
            let request = ws::read_client_frame(&mut socket).await.unwrap();
            assert_eq!((request.opcode, request.data.as_slice()), (2, &b"GET"[..]));
            let finished = ws::read_client_frame(&mut socket).await.unwrap();
            assert_eq!((finished.opcode, finished.data.len()), (2, 0), "fim de escrita é binário vazio, nunca Close");
            server_frame(&mut socket, 2, b"reply").await.unwrap();
            server_frame(&mut socket, 8, &1000u16.to_be_bytes()).await.unwrap();
            std::future::pending::<()>().await;
        }).await;
        let mut forwards = PortForwards::default();
        let local = forwards.open(&Handle::current(), &api(addr), "t", port).unwrap();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        client.write_all(b"GET").await.unwrap();
        client.shutdown().await.unwrap();
        assert_eq!(read_to_end(&mut client).await, b"reply");
    }

    #[tokio::test]
    async fn large_download_streams_with_backpressure() {
        const TOTAL: usize = 50 * 1024 * 1024;
        const FRAME: usize = 60_000;
        let port = free_port();
        let sent = Arc::new(AtomicUsize::new(0));
        let counter = sent.clone();
        let addr = fake_server(move |mut socket| {
            let sent = counter.clone();
            async move {
                accept(&mut socket, port, 101).await;
                let mut offset = 0;
                while offset < TOTAL {
                    let n = FRAME.min(TOTAL - offset);
                    let chunk: Vec<u8> = (offset..offset + n).map(|ix| (ix % 251) as u8).collect();
                    if server_frame(&mut socket, 2, &chunk).await.is_err() { return; }
                    offset += n;
                    sent.store(offset, Ordering::SeqCst);
                }
                server_frame(&mut socket, 8, &1000u16.to_be_bytes()).await.unwrap();
                std::future::pending::<()>().await;
            }
        }).await;
        let mut forwards = PortForwards::default();
        let local = forwards.open(&Handle::current(), &api(addr), "t", port).unwrap();
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        // Sem ler do lado local, o servidor para de conseguir escrever: só os buffers do sistema enchem.
        // Espera o envio parar de andar (máquina lenta só demora mais); sem contrapressão ele para no TOTAL.
        let stalled = timeout(Duration::from_secs(30), async {
            let mut last = usize::MAX;
            loop {
                let now = sent.load(Ordering::SeqCst);
                if now == last { return now; }
                last = now;
                // estavel: amostra do avanço do servidor; o teste espera ele parar, com teto de 30 s
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }).await.expect("o envio do servidor para");
        assert!(stalled < TOTAL, "o servidor escreveu {stalled} bytes sem ninguém ler");
        let mut buf = vec![0u8; CHUNK];
        let mut received = 0;
        timeout(Duration::from_secs(60), async { loop {
            let n = client.read(&mut buf).await.unwrap();
            if n == 0 { break; }
            assert!(buf[..n].iter().enumerate().all(|(ix, b)| *b == ((received + ix) % 251) as u8));
            received += n;
        } }).await.expect("o download termina");
        assert_eq!(received, TOTAL);
    }
}
