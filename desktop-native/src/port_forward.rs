//! Encaminha `127.0.0.1:<porta local>` deste computador para `localhost:<porta>` da máquina de um servidor, pelo
//! túnel `/api/port-tunnel`. Cada conexão local é um WebSocket próprio, aberto pela rota em uso naquele instante.
use std::{collections::HashMap, fmt, io, net::{IpAddr, Ipv4Addr, Ipv6Addr}, sync::{Arc, Mutex}, time::Duration};
use ring::rand::{SecureRandom, SystemRandom};
use tokio::{io::{AsyncReadExt, AsyncWrite, AsyncWriteExt}, net::{TcpListener, TcpStream}, runtime::Handle,
    sync::watch, task::{JoinHandle, JoinSet}, time::timeout};
use crate::{api::{Api, route::blocked_url}, ws};

/// Abaixo do teto de 1 MiB por mensagem do servidor, e o que fica em memória por sentido.
const CHUNK: usize = 64 * 1024;
const MAX_FRAME: usize = 1024 * 1024;
/// O servidor pinga a cada 25 s: sem quadro nenhum nesse tempo, a conexão morreu no caminho.
const IDLE: Duration = Duration::from_secs(60);
/// Quantas portas acima da pedida se tenta antes de desistir.
const SEARCH: u16 = 100;
/// Conexões vivas por encaminhamento: página que martela a porta não abre centenas de túneis pelo Connect.
const MAX_CONNECTIONS: usize = 64;
/// Quanto a sonda de mesma máquina espera os bytes dela, depois do aperto de mão.
const PROBE_WAIT: Duration = Duration::from_secs(3);

/// Último problema de um encaminhamento, para a tela dizer; o texto é da tela.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardError {
    /// Rota do Connect sem credencial de aparelho: a senha do dono não vai.
    Blocked,
    /// `port_tunnel_refused`: nada escuta nessa porta na máquina do servidor.
    NothingListening,
    /// `port_tunnel_port_refused`: porta fora da faixa ou do próprio Hangar.
    PortRefused,
    /// `port_tunnel_token_required`: a credencial não chegou por `?token=` nem Bearer.
    TokenRequired,
    /// Recusa sem o cabeçalho do túnel: quem respondeu não foi ele (servidor fora, Caddy do Connect, Hangar antigo).
    Unreachable,
    /// Recusa com código que este app não conhece, ou o 401 do porteiro.
    Refused(u16),
    /// O aperto de mão não chegou ao fim (rede, TLS, tempo).
    Network,
    /// A conexão já aberta caiu sem o fechamento do servidor.
    Dropped,
    /// Uma conexão local foi fechada na hora: o encaminhamento já tinha `MAX_CONNECTIONS` vivas.
    TooMany,
}

impl ForwardError {
    /// Pelo código do cabeçalho, nunca pelo status: o Caddy do Connect também devolve 502 com a 8768 fora.
    fn of(error: ws::Error) -> Self {
        let ws::Error::Http(status, code) = error else { return Self::Network };
        match code.as_deref() {
            Some("port_tunnel_refused") => Self::NothingListening,
            // `host` que não é `127.0.0.1` nem `::1` também é recusa do destino.
            Some("port_tunnel_port_refused" | "port_tunnel_host_refused") => Self::PortRefused,
            Some("port_tunnel_token_required") => Self::TokenRequired,
            // A máquina do servidor não conseguiu discar a porta por outro motivo que não "nada escutando".
            Some("port_tunnel_unavailable") => Self::Network,
            // O 401 vem do porteiro, antes do túnel: senha recusada, não servidor fora.
            None if status != 401 => Self::Unreachable,
            _ => Self::Refused(status),
        }
    }
}

/// Por que um encaminhamento não abriu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// A sonda voltou aqui: escutar faria o túnel do servidor discar o próprio encaminhamento, em laço.
    SameMachine,
    /// Não deu para saber se é esta máquina: o motivo do túnel, ou `None` quando a sonda não voltou no prazo.
    Unavailable(Option<ForwardError>),
    /// Nenhuma porta livre neste computador; o texto é o do sistema.
    Listen(String),
}

impl fmt::Display for OpenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SameMachine => f.write_str("o servidor é este computador"),
            Self::Unavailable(Some(reason)) => write!(f, "o túnel não respondeu ({reason:?})"),
            Self::Unavailable(None) => f.write_str("não deu para conferir se o servidor é este computador"),
            Self::Listen(error) => f.write_str(error),
        }
    }
}

type Status = Option<Result<u16, OpenError>>;

/// Encaminhamento abrindo fora da thread da tela. `wait` dá a porta local, ou `None` se ele foi parado antes.
#[derive(Clone)]
pub struct Opening(watch::Receiver<Status>);

impl Opening {
    pub async fn wait(mut self) -> Option<Result<u16, OpenError>> {
        self.0.wait_for(Option::is_some).await.ok().and_then(|status| status.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForwardState { pub port: u16, pub local: u16, pub error: Option<ForwardError> }

type Shared = Arc<Mutex<Option<ForwardError>>>;
/// Rota e senha que as próximas conexões usam: a senha do servidor pode mudar com o encaminhamento de pé.
type Conn = Arc<Mutex<(Api, String)>>;

struct Forward { status: watch::Receiver<Status>, error: Shared, conn: Conn, task: Option<JoinHandle<()>> }

impl Forward {
    fn local(&self) -> Option<u16> { match &*self.status.borrow() { Some(Ok(local)) => Some(*local), _ => None } }
}

// Abortar o laço solta a escuta e o `JoinSet` dele, que aborta cada conexão viva.
impl Drop for Forward { fn drop(&mut self) { if let Some(task) = &self.task { task.abort(); } } }

/// Um por app; a chave é a identidade do servidor (`Api::identity`) e a porta remota. `closing` é a escuta recém-parada
/// de cada chave, que só a abertura da mesma chave espera.
#[derive(Default)]
pub struct PortForwards { forwards: HashMap<(String, u16), Forward>, closing: HashMap<(String, u16), JoinHandle<()>> }

/// Quanto a abertura espera a escuta de um encaminhamento recém-parado soltar a porta.
const RELEASE: Duration = Duration::from_millis(200);

impl PortForwards {
    /// Não bloqueia: a sonda e a busca de porta livre rodam em `runtime`. A mesma porta do mesmo servidor de novo
    /// devolve a abertura que já existe, com `api` e `token` valendo para as próximas conexões dela. `token` é a senha
    /// do dono do servidor de `api`, como nos outros sockets.
    pub fn open(&mut self, runtime: &Handle, api: &Api, token: &str, port: u16) -> Opening {
        let key = (api.identity(), port);
        match self.forwards.get(&key) {
            // Escuta que morreu (runtime fechando, descritor) ou abertura que falhou: tenta de novo.
            Some(forward) if forward.task.as_ref().is_some_and(JoinHandle::is_finished) => { self.forwards.remove(&key); }
            Some(forward) => {
                *lock_conn(&forward.conn) = (api.clone(), token.to_owned());
                return Opening(forward.status.clone());
            }
            None => {}
        }
        let closing = self.closing.remove(&key);
        let (tx, status) = watch::channel(None);
        let error = Shared::default();
        let conn: Conn = Arc::new(Mutex::new((api.clone(), token.to_owned())));
        let task = runtime.spawn(start(closing, conn.clone(), port, error.clone(), tx));
        self.forwards.insert(key, Forward { status: status.clone(), error, conn, task: Some(task) });
        Opening(status)
    }

    pub fn close(&mut self, server: &str, port: u16) {
        let key = (server.to_owned(), port);
        let Some(mut forward) = self.forwards.remove(&key) else { return };
        // As que já soltaram a porta não precisam ficar.
        self.closing.retain(|_, task| !task.is_finished());
        if let Some(task) = forward.task.take() {
            task.abort();
            self.closing.insert(key, task);
        }
    }

    /// Encaminhamentos já escutando de `server`, por porta remota.
    pub fn list(&self, server: &str) -> Vec<ForwardState> {
        let mut list: Vec<_> = self.forwards.iter().filter(|((id, _), _)| id == server)
            .filter_map(|((_, port), f)| Some(ForwardState { port: *port, local: f.local()?, error: *lock(&f.error) })).collect();
        list.sort_by_key(|f| f.port);
        list
    }

    /// Portas remotas de `server` ainda abrindo.
    pub fn opening(&self, server: &str) -> Vec<u16> {
        self.forwards.iter()
            .filter(|((id, _), f)| id == server && f.status.borrow().is_none() && !f.task.as_ref().is_some_and(JoinHandle::is_finished))
            .map(|((_, port), _)| *port).collect()
    }
}

fn lock(error: &Shared) -> std::sync::MutexGuard<'_, Option<ForwardError>> { error.lock().unwrap_or_else(|e| e.into_inner()) }

fn lock_conn(conn: &Conn) -> std::sync::MutexGuard<'_, (Api, String)> { conn.lock().unwrap_or_else(|e| e.into_inner()) }

/// Sonda, porta livre e escuta, nesta ordem; o resultado vai a quem espera a abertura antes da escuta começar.
async fn start(closing: Option<JoinHandle<()>>, conn: Conn, port: u16, error: Shared, tx: watch::Sender<Status>) {
    // Parar e Encaminhar de novo: o abort solta a porta só quando o runtime roda a tarefa; sem esperar,
    // a porta pulava para a seguinte sem motivo.
    if let Some(task) = closing { let _ = timeout(RELEASE, task).await; }
    let (api, token) = lock_conn(&conn).clone();
    let opened = match probe(&api, &token, PROBE_WAIT).await {
        Ok(false) => tokio::task::spawn_blocking(move || bind_from(port)).await.map_err(io::Error::other).and_then(|bound| bound)
            .and_then(|listener| { let local = listener.local_addr()?.port(); Ok((TcpListener::from_std(listener)?, local)) })
            .map_err(|error| OpenError::Listen(error.to_string())),
        Ok(true) => Err(OpenError::SameMachine),
        Err(reason) => Err(OpenError::Unavailable(reason)),
    };
    match opened {
        Ok((listener, local)) => {
            tx.send_replace(Some(Ok(local)));
            listen(listener, conn, port, error).await;
        }
        Err(failure) => { tx.send_replace(Some(Err(failure))); }
    }
}

/// O servidor é este computador? `true` só se 16 bytes aleatórios mandados pelo túnel até uma escuta daqui chegam
/// nela; `false` só se o túnel diz que nada escuta nessa porta lá, ou se outro serviço de lá responde. O resto é
/// desconhecido: sem saber, encaminhar pode virar laço.
async fn probe(api: &Api, token: &str, wait: Duration) -> Result<bool, Option<ForwardError>> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.map_err(|_| Some(ForwardError::Network))?;
    let port = listener.local_addr().map_err(|_| Some(ForwardError::Network))?.port();
    let url = ws::text_url(api, &format!("/api/port-tunnel?port={port}"), token);
    if url == blocked_url() { return Err(Some(ForwardError::Blocked)); }
    let socket = match ws::handshake(url, Some(api.identity())).await {
        Ok(socket) => socket,
        Err(error) => return match ForwardError::of(error) {
            ForwardError::NothingListening => Ok(false),
            other => Err(Some(other)),
        },
    };
    let mut nonce = [0u8; 16];
    SystemRandom::new().fill(&mut nonce).map_err(|_| Some(ForwardError::Network))?;
    let (mut ws_read, mut ws_write) = tokio::io::split(socket);
    let frame = masked(2, &nonce).map_err(|_| Some(ForwardError::Network))?;
    if ws_write.write_all(&frame).await.is_err() || ws_write.flush().await.is_err() { return Err(Some(ForwardError::Dropped)); }
    let arrived = async {
        loop {
            let Ok((mut tcp, _)) = listener.accept().await else { continue };
            let mut got = [0u8; 16];
            if tcp.read_exact(&mut got).await.is_ok() && got == nonce { return; }
        }
    };
    // Quadro ou fechamento de lá: o túnel achou outro serviço nessa porta, em outra máquina.
    let answered = ws::read_frame_as(&mut ws_read, false, MAX_FRAME);
    let decided = timeout(wait, async {
        tokio::select! {
            () = arrived => Some(true),
            frame = answered => frame.is_ok().then_some(false),
        }
    }).await;
    match decided { Ok(Some(same)) => Ok(same), _ => Err(None) }
}

/// Só `127.0.0.1`: o encaminhamento é deste computador, nunca da rede dele. O número pedido (ou o próximo) só serve
/// se ninguém atende nele aqui, nem em `::1` nem em `::`: o `localhost` do navegador iria para quem está no IPv6.
fn bind_from(port: u16) -> io::Result<std::net::TcpListener> {
    if port == 0 { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut last = io::Error::from(io::ErrorKind::AddrInUse);
    for candidate in port..=port.saturating_add(SEARCH) {
        if taken_here(candidate) {
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

fn taken_here(port: u16) -> bool {
    // No Windows o bind em 127.0.0.1 vence um servidor local em 0.0.0.0 na mesma porta e o tapa
    // calado: quem já atende ali deixa a porta ocupada.
    let here = (Ipv4Addr::LOCALHOST, port).into();
    if std::net::TcpStream::connect_timeout(&here, Duration::from_millis(100)).is_ok() { return true; }
    // Máquina sem IPv6 não tapa nada: só "em uso" conta.
    [Ipv6Addr::LOCALHOST, Ipv6Addr::UNSPECIFIED].into_iter().any(|ip| {
        std::net::TcpListener::bind((IpAddr::V6(ip), port)).is_err_and(|e| e.kind() == io::ErrorKind::AddrInUse)
    })
}

async fn listen(listener: TcpListener, conn: Conn, port: u16, error: Shared) {
    let full = error.clone();
    accept_capped(listener, |tcp| { let (api, token) = lock_conn(&conn).clone(); connection(tcp, api, token, port, error.clone()) },
        move || *lock(&full) = Some(ForwardError::TooMany)).await
}

/// Aceita para sempre, com no máximo `MAX_CONNECTIONS` vivas: a de cima do teto fecha na hora, sem aperto de mão, e
/// chama `full`. No Linux, conexão de outro usuário deste computador também fecha (`own_connection`).
pub(crate) async fn accept_capped<F, Fut>(listener: TcpListener, mut serve: F, mut full: impl FnMut())
where F: FnMut(TcpStream) -> Fut, Fut: std::future::Future<Output = ()> + Send + 'static {
    let mut connections = JoinSet::new();
    loop {
        tokio::select! {
            accepted = listener.accept() => match accepted {
                Ok((tcp, _)) => {
                    while connections.try_join_next().is_some() {}
                    if connections.len() >= MAX_CONNECTIONS { full(); continue; }
                    let _ = tcp.set_nodelay(true);
                    #[cfg(target_os = "linux")]
                    let ends = (tcp.peer_addr(), tcp.local_addr());
                    let served = serve(tcp);
                    connections.spawn(async move {
                        // A tabela do kernel é lida de arquivo: fora das threads do runtime.
                        #[cfg(target_os = "linux")]
                        if !tokio::task::spawn_blocking(move || own_connection(ends)).await.unwrap_or(false) { return; }
                        served.await
                    });
                }
                // Sem descritor livre, por exemplo: não girar em falso.
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            },
            Some(_) = connections.join_next() => {}
        }
    }
}

/// Uma conexão do túnel até `port` da máquina de `api`. A rota muda (rede local, Tailscale, Connect): é lida de novo
/// a cada chamada. `host`: o loopback que a página pediu (`127.0.0.1` ou `::1`); sem ele o servidor escolhe o que
/// chega ao processo da sessão. Servidor antigo ignora o parâmetro.
pub(crate) async fn tunnel(api: &Api, token: &str, port: u16, host: Option<IpAddr>) -> Result<reqwest::Upgraded, ForwardError> {
    let url = ws::text_url(api, &tunnel_path(port, host), token);
    if url == blocked_url() { return Err(ForwardError::Blocked); }
    ws::handshake(url, Some(api.identity())).await.map_err(ForwardError::of)
}

fn tunnel_path(port: u16, host: Option<IpAddr>) -> String {
    let host = host.map(|ip| format!("&host={}", url::form_urlencoded::byte_serialize(ip.to_string().as_bytes()).collect::<String>()));
    format!("/api/port-tunnel?port={port}{}", host.unwrap_or_default())
}

#[derive(Clone, Copy, PartialEq)]
pub(crate) enum End { Closed, Dropped, Local }

async fn connection(tcp: TcpStream, api: Api, token: String, port: u16, error: Shared) {
    // O encaminhamento é do `localhost` da lista: o servidor escolhe a família.
    let socket = match tunnel(&api, &token, port, None).await {
        Ok(socket) => socket,
        Err(refused) => { *lock(&error) = Some(refused); return; }
    };
    *lock(&error) = None;
    let end = relay(socket, tcp).await;
    if end == End::Dropped { *lock(&error) = Some(ForwardError::Dropped); }
}

/// Outro usuário deste computador não usa o túnel do dono: o lado cliente da conexão, na tabela do kernel, tem
/// de ser deste usuário. Sem achar a linha, fecha.
#[cfg(target_os = "linux")]
fn own_connection(ends: (io::Result<std::net::SocketAddr>, io::Result<std::net::SocketAddr>)) -> bool {
    let (Ok(std::net::SocketAddr::V4(client)), Ok(std::net::SocketAddr::V4(server))) = ends else { return false };
    let tables: String = ["/proc/net/tcp", "/proc/net/tcp6"].iter().filter_map(|path| std::fs::read_to_string(path).ok()).collect();
    owner_uid(&tables, client, server) == Some(unsafe { libc::geteuid() })
}

/// Dono da linha `client → server` em `/proc/net/tcp{,6}`; no `tcp6` o IPv4 aparece mapeado (`::ffff:a.b.c.d`).
#[cfg(target_os = "linux")]
fn owner_uid(tables: &str, client: std::net::SocketAddrV4, server: std::net::SocketAddrV4) -> Option<u32> {
    // O kernel escreve cada palavra de 32 bits do endereço na ordem da máquina, e a porta em hexa.
    let hex = |octets: &[u8], port: u16| {
        let words: String = octets.chunks(4).map(|w| format!("{:08X}", u32::from_ne_bytes([w[0], w[1], w[2], w[3]]))).collect();
        format!("{words}:{port:04X}")
    };
    let keys = [
        (hex(&client.ip().octets(), client.port()), hex(&server.ip().octets(), server.port())),
        (hex(&client.ip().to_ipv6_mapped().octets(), client.port()), hex(&server.ip().to_ipv6_mapped().octets(), server.port())),
    ];
    tables.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // sl, local, remoto, estado, filas, timer, retransmissões, uid
        let row = fields.get(1..8)?;
        keys.iter().any(|(local, remote)| row[0].eq_ignore_ascii_case(local) && row[1].eq_ignore_ascii_case(remote))
            .then(|| row[6].parse().ok()).flatten()
    })
}

/// Cada sentido lê um pedaço e só lê o próximo depois de escrevê-lo do outro lado: a contrapressão do TCP
/// atravessa o túnel e nada se acumula na memória.
pub(crate) async fn relay(socket: reqwest::Upgraded, tcp: TcpStream) -> End {
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

    /// Lê o pedido até o fim dos cabeçalhos e devolve ele e a porta pedida ao túnel.
    async fn request(socket: &mut TcpStream) -> (String, u16) {
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") { request.push(socket.read_u8().await.unwrap()); }
        let request = String::from_utf8(request).unwrap();
        let port = request.strip_prefix("GET /api/port-tunnel?port=").and_then(|rest| rest.split('&').next())
            .and_then(|port| port.parse().ok()).unwrap_or_else(|| panic!("{request}"));
        assert!(request.lines().next().is_some_and(|line| line.contains("&token=") && line.ends_with(" HTTP/1.1")), "{request}");
        (request, port)
    }

    /// 101 completa o aperto de mão; o resto recusa, com o código do túnel no cabeçalho quando houver.
    async fn respond(socket: &mut TcpStream, request: &str, status: u16, code: Option<&str>) {
        if status != 101 {
            let header = code.map(|code| format!("x-hangar-tunnel: {code}\r\n")).unwrap_or_default();
            socket.write_all(format!("HTTP/1.1 {status} X\r\n{header}Content-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
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

    /// Servidor de túnel falso: cada conexão roda `handler` com o pedido e a porta pedida.
    async fn raw_server<F, Fut>(handler: F) -> SocketAddr
    where F: Fn(TcpStream, String, u16) -> Fut + Send + Sync + 'static, Fut: Future<Output = ()> + Send + 'static {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handler = Arc::new(handler);
        tokio::spawn(async move { while let Ok((mut socket, _)) = listener.accept().await {
            let handler = handler.clone();
            tokio::spawn(async move { let (request, port) = request(&mut socket).await; (*handler)(socket, request, port).await });
        } });
        addr
    }

    /// Túnel de `port` em outra máquina: a sonda (outra porta) ouve "nada escutando".
    async fn fake_server<F, Fut>(port: u16, handler: F) -> SocketAddr
    where F: Fn(TcpStream, String) -> Fut + Send + Sync + 'static, Fut: Future<Output = ()> + Send + 'static {
        let handler = Arc::new(handler);
        raw_server(move |mut socket, request, asked| {
            let handler = handler.clone();
            async move {
                if asked == port { (*handler)(socket, request).await } else { respond(&mut socket, &request, 502, Some("port_tunnel_refused")).await }
            }
        }).await
    }

    /// Devolve cada mensagem binária recebida até o fechamento.
    async fn echo(mut socket: TcpStream, request: String) {
        respond(&mut socket, &request, 101, None).await;
        while let Ok(frame) = ws::read_client_frame(&mut socket).await {
            if frame.opcode != 2 { break; }
            if server_frame(&mut socket, 2, &frame.data).await.is_err() { break; }
        }
    }

    fn api(addr: SocketAddr) -> Api { Api::new(&format!("http://{addr}"), "t").unwrap() }

    /// Porta remota livre aqui, para a local sair igual a ela.
    fn free_port() -> u16 { std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port() }

    async fn open(forwards: &mut PortForwards, api: &Api, port: u16) -> u16 {
        let opening = forwards.open(&Handle::current(), api, "t", port);
        timeout(WAIT, opening.wait()).await.expect("abre no prazo").expect("não foi parado").expect("abre")
    }

    async fn read_to_end(client: &mut TcpStream) -> Vec<u8> {
        let mut data = Vec::new();
        timeout(WAIT, client.read_to_end(&mut data)).await.expect("a conexão local fecha").ok();
        data
    }

    async fn echoes(client: &mut TcpStream) {
        client.write_all(b"ping").await.unwrap();
        let mut reply = [0; 4];
        timeout(WAIT, client.read_exact(&mut reply)).await.unwrap().unwrap();
        assert_eq!(&reply, b"ping");
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

    #[test]
    fn a_server_here_on_ipv6_loopback_moves_the_forward_to_another_port() {
        // Máquina sem IPv6 não tem o que tapar.
        let Ok(busy) = std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) else { return };
        let port = busy.local_addr().unwrap().port();
        drop(std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).expect("o 127.0.0.1 dessa porta está livre"));
        let local = bind_from(port).unwrap().local_addr().unwrap();
        assert_ne!(local.port(), port);
    }

    #[test]
    fn refusal_is_read_from_the_tunnel_header_not_the_status() {
        use ForwardError::*;
        for (status, code, expected) in [
            (502, Some("port_tunnel_refused"), NothingListening),
            (422, Some("port_tunnel_port_refused"), PortRefused),
            (422, Some("port_tunnel_host_refused"), PortRefused),
            (403, Some("port_tunnel_token_required"), TokenRequired),
            (503, Some("port_tunnel_unavailable"), Network),
            (502, None, Unreachable),
            (404, None, Unreachable),
            (401, None, Refused(401)),
            (409, Some("outro"), Refused(409)),
        ] {
            assert_eq!(ForwardError::of(ws::Error::Http(status, code.map(str::to_owned))), expected, "{status} {code:?}");
        }
        assert_eq!(ForwardError::of(ws::Error::Network), Network);
    }

    #[cfg(all(target_os = "linux", target_endian = "little"))]
    #[test]
    fn the_owner_is_the_row_of_the_client_side() {
        let client = std::net::SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0xC350);
        let server = std::net::SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0x0BB8);
        let tcp = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode\n\
            0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 1\n\
            1: 0100007F:0BB8 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 2\n\
            2: 0100007F:C350 0100007F:0BB8 01 00000000:00000000 00:00000000 00000000  1001        0 3\n";
        assert_eq!(owner_uid(tcp, client, server), Some(1001));
        let tcp6 = "  0: 0000000000000000FFFF00000100007F:C350 0000000000000000FFFF00000100007F:0BB8 01 00000000:00000000 00:00000000 00000000  1002        0 4\n";
        assert_eq!(owner_uid(tcp6, client, server), Some(1002));
        assert_eq!(owner_uid(&tcp[..tcp.find("2: 0100007F:C350").unwrap()], client, server), None);
    }

    #[tokio::test]
    async fn the_tunnel_asks_the_host_of_an_explicit_ip_and_nothing_for_a_name() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = seen.clone();
        let addr = raw_server(move |mut socket, request, _| {
            record.lock().unwrap().push(request.lines().next().unwrap_or_default().to_owned());
            async move { respond(&mut socket, &request, 502, Some("port_tunnel_refused")).await }
        }).await;
        let api = api(addr);
        for host in [Some(IpAddr::V6(Ipv6Addr::LOCALHOST)), Some(IpAddr::V4(Ipv4Addr::LOCALHOST)), None] {
            assert_eq!(tunnel(&api, "t", 3000, host).await.err(), Some(ForwardError::NothingListening));
        }
        let lines = seen.lock().unwrap().clone();
        assert!(lines[0].starts_with("GET /api/port-tunnel?port=3000&host=%3A%3A1&token="), "{}", lines[0]);
        assert!(lines[1].starts_with("GET /api/port-tunnel?port=3000&host=127.0.0.1&token="), "{}", lines[1]);
        assert!(lines[2].starts_with("GET /api/port-tunnel?port=3000&token="), "{}", lines[2]);
    }

    #[tokio::test]
    async fn probe_is_same_machine_only_when_the_bytes_come_back() {
        let addr = raw_server(|mut socket, request, port| async move {
            // Túnel desta máquina: disca a porta pedida aqui e entrega o que chegar.
            let mut tcp = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
            respond(&mut socket, &request, 101, None).await;
            let frame = ws::read_client_frame(&mut socket).await.unwrap();
            tcp.write_all(&frame.data).await.unwrap();
            std::future::pending::<()>().await;
        }).await;
        let api = api(addr);
        assert_eq!(probe(&api, "t", WAIT).await, Ok(true));
        let mut forwards = PortForwards::default();
        let opening = forwards.open(&Handle::current(), &api, "t", free_port());
        assert_eq!(timeout(WAIT, opening.wait()).await.unwrap(), Some(Err(OpenError::SameMachine)));
        assert!(forwards.list(&api.identity()).is_empty(), "nunca escuta");
    }

    #[tokio::test]
    async fn probe_says_another_machine_only_with_proof() {
        let nothing = raw_server(|mut s, r, _| async move { respond(&mut s, &r, 502, Some("port_tunnel_refused")).await }).await;
        assert_eq!(probe(&api(nothing), "t", WAIT).await, Ok(false));
        let answered = raw_server(|mut s, r, _| async move {
            respond(&mut s, &r, 101, None).await;
            let _ = ws::read_client_frame(&mut s).await;
            server_frame(&mut s, 2, b"outro servico").await.unwrap();
            std::future::pending::<()>().await;
        }).await;
        assert_eq!(probe(&api(answered), "t", WAIT).await, Ok(false));
        let down = raw_server(|mut s, r, _| async move { respond(&mut s, &r, 502, None).await }).await;
        assert_eq!(probe(&api(down), "t", WAIT).await, Err(Some(ForwardError::Unreachable)));
        // Falha de lá que não é "nada escutando" não prova outra máquina.
        let unavailable = raw_server(|mut s, r, _| async move { respond(&mut s, &r, 503, Some("port_tunnel_unavailable")).await }).await;
        assert_eq!(probe(&api(unavailable), "t", WAIT).await, Err(Some(ForwardError::Network)));
        let silent = raw_server(|mut s, r, _| async move { respond(&mut s, &r, 101, None).await; std::future::pending::<()>().await }).await;
        // estavel: prazo curto da sonda; o esperado é justamente ele vencer sem resposta
        assert_eq!(probe(&api(silent), "t", Duration::from_millis(100)).await, Err(None));
    }

    #[tokio::test]
    async fn same_port_twice_returns_the_same_local_port_and_close_drops_connections() {
        let port = free_port();
        let addr = fake_server(port, echo).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = open(&mut forwards, &api, port).await;
        assert_eq!(local, port);
        assert_eq!(open(&mut forwards, &api, port).await, local);
        assert_eq!(forwards.list(&api.identity()), vec![ForwardState { port, local, error: None }]);
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        echoes(&mut client).await;
        forwards.close(&api.identity(), port);
        assert!(forwards.list(&api.identity()).is_empty());
        assert_eq!(read_to_end(&mut client).await, b"");
        timeout(WAIT, async { while TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.is_ok() {
            // estavel: intervalo da consulta até a escuta fechar; o teto é o `timeout(WAIT)` em volta
            tokio::time::sleep(Duration::from_millis(10)).await;
        } }).await.expect("a escuta local fecha");
    }

    #[tokio::test]
    async fn opening_again_with_a_new_token_reaches_the_next_connections() {
        let port = free_port();
        // Só a senha nova abre o túnel: a velha recebe o 401 do porteiro.
        let addr = fake_server(port, |mut socket, request| async move {
            if request.lines().next().is_some_and(|line| line.contains("&token=new ")) { echo(socket, request).await }
            else { respond(&mut socket, &request, 401, None).await }
        }).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let opening = forwards.open(&Handle::current(), &api, "old", port);
        let local = timeout(WAIT, opening.wait()).await.unwrap().unwrap().unwrap();
        let mut stale = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        assert_eq!(read_to_end(&mut stale).await, b"");
        let opening = forwards.open(&Handle::current(), &api, "new", port);
        assert_eq!(timeout(WAIT, opening.wait()).await.unwrap().unwrap().unwrap(), local, "a mesma escuta continua");
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        echoes(&mut client).await;
    }

    #[tokio::test]
    async fn two_simultaneous_connections_on_one_forward() {
        let port = free_port();
        let addr = fake_server(port, echo).await;
        let mut forwards = PortForwards::default();
        let local = open(&mut forwards, &api(addr), port).await;
        let mut a = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        let mut b = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        a.write_all(b"from-a").await.unwrap();
        b.write_all(b"from-b").await.unwrap();
        let (mut ra, mut rb) = ([0; 6], [0; 6]);
        timeout(WAIT, async { tokio::try_join!(a.read_exact(&mut ra), b.read_exact(&mut rb)) }).await.unwrap().unwrap();
        assert_eq!((&ra, &rb), (b"from-a", b"from-b"));
    }

    #[tokio::test]
    async fn the_connection_past_the_cap_is_closed_at_once() {
        let port = free_port();
        let addr = fake_server(port, echo).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = open(&mut forwards, &api, port).await;
        let mut held = Vec::new();
        for _ in 0..MAX_CONNECTIONS {
            let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
            // O eco prova que a conexão está viva no túnel e conta no teto.
            echoes(&mut client).await;
            held.push(client);
        }
        let mut extra = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        assert_eq!(read_to_end(&mut extra).await, b"");
        assert_eq!(error_of(&forwards, &api.identity(), port).await, ForwardError::TooMany, "a recusa do teto aparece na tela");
        echoes(&mut held[0]).await;
    }

    #[tokio::test]
    async fn only_reopening_the_same_port_waits_for_its_stopped_listener() {
        // Nada responde nesse endereço: as aberturas não chegam a escutar, e o teste só olha o registro.
        let api = Api::new("http://127.0.0.1:9", "t").unwrap();
        let server = api.identity();
        let mut forwards = PortForwards::default();
        forwards.open(&Handle::current(), &api, "t", 3000);
        forwards.close(&server, 3000);
        assert!(forwards.closing.contains_key(&(server.clone(), 3000)));
        forwards.open(&Handle::current(), &api, "t", 4000);
        assert!(forwards.closing.contains_key(&(server.clone(), 3000)), "abrir outra porta não leva a espera da 3000");
        forwards.open(&Handle::current(), &api, "t", 3000);
        assert!(!forwards.closing.contains_key(&(server, 3000)), "reabrir a 3000 espera a escuta dela");
    }

    #[tokio::test]
    async fn server_close_closes_the_local_socket_without_error() {
        let port = free_port();
        let addr = fake_server(port, |mut socket, request| async move {
            respond(&mut socket, &request, 101, None).await;
            server_frame(&mut socket, 2, b"bye").await.unwrap();
            server_frame(&mut socket, 8, &1000u16.to_be_bytes()).await.unwrap();
            // Fica de pé: quem fecha o socket local é o Close, não a queda do TCP.
            std::future::pending::<()>().await;
        }).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = open(&mut forwards, &api, port).await;
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        assert_eq!(read_to_end(&mut client).await, b"bye");
        assert_eq!(forwards.list(&api.identity())[0].error, None);
    }

    #[tokio::test]
    async fn server_dropping_closes_the_local_socket_and_says_dropped() {
        let port = free_port();
        let addr = fake_server(port, |mut socket, request| async move { respond(&mut socket, &request, 101, None).await; }).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = open(&mut forwards, &api, port).await;
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        assert_eq!(read_to_end(&mut client).await, b"");
        assert_eq!(error_of(&forwards, &api.identity(), port).await, ForwardError::Dropped);
    }

    #[tokio::test]
    async fn handshake_refusal_closes_the_local_socket_and_keeps_the_code() {
        let port = free_port();
        let addr = fake_server(port, |mut socket, request| async move { respond(&mut socket, &request, 502, Some("port_tunnel_refused")).await; }).await;
        let api = api(addr);
        let mut forwards = PortForwards::default();
        let local = open(&mut forwards, &api, port).await;
        let mut client = TcpStream::connect((Ipv4Addr::LOCALHOST, local)).await.unwrap();
        assert_eq!(read_to_end(&mut client).await, b"");
        assert_eq!(error_of(&forwards, &api.identity(), port).await, ForwardError::NothingListening);
    }

    #[tokio::test]
    async fn local_half_close_sends_an_empty_binary_and_the_reply_still_arrives() {
        let port = free_port();
        let addr = fake_server(port, |mut socket, request| async move {
            respond(&mut socket, &request, 101, None).await;
            let request = ws::read_client_frame(&mut socket).await.unwrap();
            assert_eq!((request.opcode, request.data.as_slice()), (2, &b"GET"[..]));
            let finished = ws::read_client_frame(&mut socket).await.unwrap();
            assert_eq!((finished.opcode, finished.data.len()), (2, 0), "fim de escrita é binário vazio, nunca Close");
            server_frame(&mut socket, 2, b"reply").await.unwrap();
            server_frame(&mut socket, 8, &1000u16.to_be_bytes()).await.unwrap();
            std::future::pending::<()>().await;
        }).await;
        let mut forwards = PortForwards::default();
        let local = open(&mut forwards, &api(addr), port).await;
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
        let addr = fake_server(port, move |mut socket, request| {
            let sent = counter.clone();
            async move {
                respond(&mut socket, &request, 101, None).await;
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
        let local = open(&mut forwards, &api(addr), port).await;
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
