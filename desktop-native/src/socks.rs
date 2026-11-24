//! SOCKS5 das páginas de uma sessão de outra máquina, no Linux. O contexto do Chromium dessa máquina manda só os
//! destinos loopback para cá (`BYPASS`), e cada CONNECT vira uma conexão do túnel `/api/port-tunnel` até a mesma porta
//! lá: a página fica em `http://localhost:3000`, com a origem, os cookies e o HMR dela. O resto vai direto pelo bypass.
//! Sem senha (o Chromium não autentica SOCKS): o dono da conexão é conferido pelo uid, como no encaminhamento.
//!
//! O modo **agente** é o do contexto onde rodam os verbos do agente da sessão remota: sem bypass, todo pedido chega
//! aqui. Loopback vai ao túnel; o resto é resolvido aqui e só conecta direto se nenhum endereço for da rede deste
//! computador. Resolver e conectar no mesmo lugar fecha redirect, `fetch` do `eval`, subrecurso e DNS que muda depois.
use std::{collections::HashMap, io, net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr}, sync::{Arc, Mutex}, time::{Duration, Instant}};
use tokio::{io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt}, net::{TcpListener, TcpStream}, runtime::Handle,
    task::JoinHandle, time::timeout};
use crate::{api::Api, port_forward::{self, ForwardError}};

/// Tudo direto, menos loopback: o `<-loopback>` tira o desvio implícito do loopback, e a última regra que casa vence.
pub const BYPASS: &str = "*,<-loopback>";
/// Nada direto: o `<-loopback>` também manda o loopback ao proxy.
pub const AGENT_BYPASS: &str = "<-loopback>";

/// Cliente que conecta e não fala segura uma das conexões do teto.
const NEGOTIATE: Duration = Duration::from_secs(10);
/// Endereço de fora que não responde também.
const DIAL: Duration = Duration::from_secs(30);

// Códigos de resposta do SOCKS5 (RFC 1928).
const SUCCEEDED: u8 = 0;
const GENERAL_FAILURE: u8 = 1;
const NOT_ALLOWED: u8 = 2;
const HOST_UNREACHABLE: u8 = 4;
const REFUSED: u8 = 5;
const COMMAND_NOT_SUPPORTED: u8 = 7;
const ADDRESS_NOT_SUPPORTED: u8 = 8;

type Conn = Arc<Mutex<(Api, String)>>;

/// De quem é o contexto que usa o listener.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Mode {
    /// A página que a pessoa abre: só loopback chega aqui (`BYPASS`).
    Person,
    /// Os verbos do agente remoto: tudo chega aqui (`AGENT_BYPASS`).
    Agent,
}

struct Listener { port: u16, conn: Conn, task: JoinHandle<()> }

impl Drop for Listener { fn drop(&mut self) { self.task.abort(); } }

/// Um listener por servidor remoto (`Api::identity`) e por modo, vivo enquanto o app vive.
#[derive(Default)]
pub struct Socks { listeners: HashMap<(String, Mode), Listener> }

impl Socks {
    /// A porta do SOCKS de `api`, que nasce na primeira vez em `127.0.0.1:0`. A senha e a conexão passam a valer
    /// para as próximas conexões.
    pub fn ensure(&mut self, runtime: &Handle, api: &Api, token: &str) -> Result<u16, String> { self.listen(Mode::Person, runtime, api, token) }

    /// O do contexto do agente remoto de `api`, para usar com `AGENT_BYPASS`.
    pub fn ensure_agent(&mut self, runtime: &Handle, api: &Api, token: &str) -> Result<u16, String> { self.listen(Mode::Agent, runtime, api, token) }

    fn listen(&mut self, mode: Mode, runtime: &Handle, api: &Api, token: &str) -> Result<u16, String> {
        let key = (api.identity(), mode);
        if let Some(listener) = self.listeners.get(&key).filter(|l| !l.task.is_finished()) {
            *lock(&listener.conn) = (api.clone(), token.to_owned());
            return Ok(listener.port);
        }
        let bound = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).and_then(|l| { l.set_nonblocking(true)?; Ok(l) })
            .map_err(|e| format!("SOCKS sem porta neste computador: {e}"))?;
        let port = bound.local_addr().map_err(|e| e.to_string())?.port();
        let conn: Conn = Arc::new(Mutex::new((api.clone(), token.to_owned())));
        let task = runtime.spawn(serve(bound, conn.clone(), mode));
        self.listeners.insert(key, Listener { port, conn, task });
        Ok(port)
    }
}

fn lock(conn: &Conn) -> std::sync::MutexGuard<'_, (Api, String)> { conn.lock().unwrap_or_else(|e| e.into_inner()) }

async fn serve(bound: std::net::TcpListener, conn: Conn, mode: Mode) {
    let Ok(listener) = TcpListener::from_std(bound) else { return };
    // O teto do SOCKS não tem onde aparecer na tela: o Chromium só vê a conexão fechada.
    port_forward::accept_capped(listener, |tcp| connection(tcp, conn.clone(), mode), || {}).await
}

async fn connection(mut tcp: TcpStream, conn: Conn, mode: Mode) {
    let Ok(Ok(asked)) = timeout(NEGOTIATE, negotiate(&mut tcp)).await else { return };
    let decided = match asked {
        Ok((host, port)) => route(mode, &host, port, &System).await,
        Err(code) => Route::Refuse(code),
    };
    let port = match decided {
        Route::Tunnel(port) => port,
        Route::Refuse(code) => { let _ = reply(&mut tcp, code).await; return; }
        Route::Direct(addrs) => return direct(tcp, &addrs).await,
    };
    let (api, token) = lock(&conn).clone();
    // Nunca conexão direta neste computador: sem túnel, a página mostraria o localhost daqui.
    match port_forward::tunnel(&api, &token, port).await {
        Ok(socket) => if reply(&mut tcp, SUCCEEDED).await.is_ok() { port_forward::relay(socket, tcp).await; },
        Err(refused) => { let _ = reply(&mut tcp, code_of(refused)).await; }
    }
}

/// Só aos endereços já conferidos: o nome não é resolvido de novo.
async fn direct(mut tcp: TcpStream, addrs: &[SocketAddr]) {
    match timeout(DIAL, TcpStream::connect(addrs)).await {
        Ok(Ok(mut remote)) => if reply(&mut tcp, SUCCEEDED).await.is_ok() { let _ = tokio::io::copy_bidirectional(&mut tcp, &mut remote).await; },
        Ok(Err(e)) if e.kind() == io::ErrorKind::ConnectionRefused => { let _ = reply(&mut tcp, REFUSED).await; }
        _ => { let _ = reply(&mut tcp, HOST_UNREACHABLE).await; }
    }
}

/// O que fazer com um CONNECT.
#[derive(Debug, PartialEq, Eq)]
enum Route { Tunnel(u16), Direct(Vec<SocketAddr>), Refuse(u8) }

/// O que a decisão do modo agente pergunta à rede; os testes trocam por um dublê.
trait Net {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>>;
    /// As redes das interfaces deste computador (endereço e máscara).
    fn interfaces(&self) -> io::Result<Arc<[Prefix]>>;
    /// Quanto o DNS pode demorar: nome que não responde segura uma das conexões do teto.
    fn resolve_wait(&self) -> Duration { DIAL }
}

/// A rede de uma interface: tudo que casa com `addr` nos bits de `mask` está no enlace deste computador.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Prefix { addr: IpAddr, mask: IpAddr }

impl Prefix {
    fn contains(&self, ip: IpAddr) -> bool {
        match (ip, self.addr, self.mask) {
            (IpAddr::V4(ip), IpAddr::V4(addr), IpAddr::V4(mask)) => ip.to_bits() & mask.to_bits() == addr.to_bits() & mask.to_bits(),
            (IpAddr::V6(ip), IpAddr::V6(addr), IpAddr::V6(mask)) => ip.to_bits() & mask.to_bits() == addr.to_bits() & mask.to_bits(),
            _ => false,
        }
    }
}

struct System;

/// A lista muda pouco e é lida a cada CONNECT.
const INTERFACES_FRESH: Duration = Duration::from_secs(5);

impl Net for System {
    async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> { Ok(tokio::net::lookup_host((host, port)).await?.collect()) }

    fn interfaces(&self) -> io::Result<Arc<[Prefix]>> {
        static CACHE: Mutex<Option<(Instant, Arc<[Prefix]>)>> = Mutex::new(None);
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, list)) = cache.as_ref() && at.elapsed() < INTERFACES_FRESH { return Ok(list.clone()); }
        let list: Arc<[Prefix]> = read_interfaces()?.into();
        *cache = Some((Instant::now(), list.clone()));
        Ok(list)
    }
}

/// Endereço e máscara de cada interface, pelo `getifaddrs`. Sem máscara vale só o próprio endereço.
fn read_interfaces() -> io::Result<Vec<Prefix>> {
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `getifaddrs` preenche `head` com uma lista que só é solta pelo `freeifaddrs` abaixo.
    if unsafe { libc::getifaddrs(&mut head) } != 0 { return Err(io::Error::last_os_error()); }
    let mut list = Vec::new();
    let mut cursor = head;
    while !cursor.is_null() {
        // SAFETY: nó da lista viva, que só é solta depois do laço.
        let node = unsafe { &*cursor };
        // SAFETY: ponteiros do próprio nó, nulos ou com a família certa no `sa_family`.
        let (addr, mask) = unsafe { (sockaddr_ip(node.ifa_addr), sockaddr_ip(node.ifa_netmask)) };
        if let Some(addr) = addr {
            let host = match addr { IpAddr::V4(_) => IpAddr::V4(Ipv4Addr::from_bits(u32::MAX)), IpAddr::V6(_) => IpAddr::V6(Ipv6Addr::from_bits(u128::MAX)) };
            let mask = mask.filter(|mask| mask.is_ipv4() == addr.is_ipv4()).unwrap_or(host);
            list.push(Prefix { addr, mask });
        }
        cursor = node.ifa_next;
    }
    // SAFETY: a lista que o `getifaddrs` devolveu, solta uma vez.
    unsafe { libc::freeifaddrs(head) };
    Ok(list)
}

/// # Safety
/// `sa` é nulo ou aponta para um `sockaddr` cujo `sa_family` diz o tamanho real.
unsafe fn sockaddr_ip(sa: *const libc::sockaddr) -> Option<IpAddr> {
    if sa.is_null() { return None; }
    match i32::from(unsafe { (*sa).sa_family }) {
        libc::AF_INET => Some(Ipv4Addr::from(u32::from_be(unsafe { (*sa.cast::<libc::sockaddr_in>()).sin_addr.s_addr })).into()),
        libc::AF_INET6 => Some(Ipv6Addr::from(unsafe { (*sa.cast::<libc::sockaddr_in6>()).sin6_addr.s6_addr }).into()),
        _ => None,
    }
}

async fn route(mode: Mode, host: &str, port: u16, net: &impl Net) -> Route {
    if loopback(host) { return Route::Tunnel(port); }
    if mode == Mode::Person { return Route::Refuse(NOT_ALLOWED); }
    let host = host.trim_start_matches('[').trim_end_matches(']');
    let addrs = match host.parse::<IpAddr>() {
        Ok(ip) => vec![SocketAddr::new(ip, port)],
        Err(_) => {
            let name = host.trim_end_matches('.').to_ascii_lowercase();
            if name == "local" || name.ends_with(".local") { return Route::Refuse(NOT_ALLOWED); }
            match timeout(net.resolve_wait(), net.resolve(&name, port)).await {
                Ok(Ok(addrs)) if !addrs.is_empty() => addrs,
                _ => return Route::Refuse(HOST_UNREACHABLE),
            }
        }
    };
    // Sem a lista não dá para saber o que é daqui: recusa.
    let Ok(interfaces) = net.interfaces() else { return Route::Refuse(GENERAL_FAILURE) };
    // Um endereço daqui basta para recusar: o `connect` tentaria todos.
    let local = |ip: IpAddr| refused_ip(ip) || [Some(ip), embedded_v4(ip).map(IpAddr::V4)].into_iter().flatten()
        .any(|ip| interfaces.iter().any(|net| net.contains(ip)));
    if addrs.iter().any(|a| local(a.ip())) { Route::Refuse(NOT_ALLOWED) } else { Route::Direct(addrs) }
}

/// O IPv4 que um IPv6 leva dentro e que é o destino de fato: mapeado (`::ffff:a.b.c.d`) e NAT64 (`64:ff9b::/96`).
fn embedded_v4(ip: IpAddr) -> Option<Ipv4Addr> {
    let IpAddr::V6(v6) = ip else { return None };
    if let Some(v4) = v6.to_ipv4_mapped() { return Some(v4); }
    (v6.segments()[..6] == [0x64, 0xff9b, 0, 0, 0, 0]).then(|| Ipv4Addr::from_bits(v6.to_bits() as u32))
}

/// A rede deste computador e o que não é destino de fora: loopback, privada, de enlace, CGNAT (a do Tailscale),
/// `0.0.0.0/8`, `192.0.0.0/24`, `198.18.0.0/15` (o IP falso de VPNs), multicast, broadcast, ULA, site-local,
/// IPv4-compatível, 6to4, Teredo e o NAT64 local (`64:ff9b:1::/48`); mapeado e NAT64 valem pelo IPv4 de dentro.
fn refused_ip(ip: IpAddr) -> bool {
    if let Some(v4) = embedded_v4(ip) { return refused_ip(v4.into()); }
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, c, _] = v4.octets();
            v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_broadcast() || v4.is_multicast() || a == 0
                || (a == 100 && (64..128).contains(&b)) || (a == 198 && (b & 0xfe) == 18) || (a == 192 && b == 0 && c == 0)
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            // `::/96` cobre `::`, `::1` e o IPv4-compatível; 6to4 e Teredo saem por relés até qualquer IPv4.
            s[..6] == [0; 6] || v6.is_multicast() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
                || (s[0] & 0xffc0) == 0xfec0 || s[0] == 0x2002 || (s[0] == 0x2001 && s[1] == 0) || s[..3] == [0x64, 0xff9b, 1]
        }
    }
}

fn code_of(error: ForwardError) -> u8 {
    match error {
        ForwardError::NothingListening => REFUSED,
        ForwardError::PortRefused => NOT_ALLOWED,
        _ => GENERAL_FAILURE,
    }
}

/// Saudação sem autenticação e o pedido. `Ok(Ok((destino, porta)))` é um CONNECT; `Ok(Err(código))`, o que responder
/// antes de fechar.
async fn negotiate(stream: &mut (impl AsyncRead + AsyncWrite + Unpin)) -> io::Result<Result<(String, u16), u8>> {
    let [version, methods] = read_array(stream).await?;
    if version != 5 { return Err(io::ErrorKind::InvalidData.into()); }
    let mut offered = vec![0; usize::from(methods)];
    stream.read_exact(&mut offered).await?;
    if !offered.contains(&0) {
        stream.write_all(&[5, 0xff]).await?;
        return Err(io::ErrorKind::PermissionDenied.into());
    }
    stream.write_all(&[5, 0]).await?;
    let [version, command, _, kind] = read_array(stream).await?;
    if version != 5 { return Err(io::ErrorKind::InvalidData.into()); }
    // O endereço vem inteiro antes da resposta, mesmo recusado: o cliente espera ler a resposta depois dele.
    let host = match kind {
        1 => Some(IpAddr::from(read_array::<4>(stream).await?).to_string()),
        3 => {
            let [len] = read_array(stream).await?;
            let mut name = vec![0; usize::from(len)];
            stream.read_exact(&mut name).await?;
            String::from_utf8(name).ok()
        }
        4 => Some(IpAddr::from(read_array::<16>(stream).await?).to_string()),
        _ => None,
    };
    let Some(host) = host else { return Ok(Err(ADDRESS_NOT_SUPPORTED)) };
    let port = u16::from_be_bytes(read_array(stream).await?);
    if command != 1 { return Ok(Err(COMMAND_NOT_SUPPORTED)); }
    Ok(Ok((host, port)))
}

async fn read_array<const N: usize>(stream: &mut (impl AsyncRead + Unpin)) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    stream.read_exact(&mut bytes).await?;
    Ok(bytes)
}

async fn reply(stream: &mut (impl AsyncWrite + Unpin), code: u8) -> io::Result<()> {
    stream.write_all(&[5, code, 0, 1, 0, 0, 0, 0, 0, 0]).await?;
    stream.flush().await
}

/// Destino que é a própria máquina do servidor. O Chromium manda o IP como nome (`127.0.0.1`, `::1`); `0.0.0.0` e
/// `::` valem como `localhost`, o endereço que o Vite imprime.
pub fn loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => ip.is_loopback() || ip.is_unspecified(),
        Ok(IpAddr::V6(ip)) => ip.is_loopback() || ip.is_unspecified() || ip.to_ipv4_mapped().is_some_and(|v4| v4.is_loopback()),
        Err(_) => {
            let name = host.trim_end_matches('.').to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
    }
}

/// `url` com `0.0.0.0` ou `[::]` trocado por `localhost`; `None` quando não é um deles. Sem a troca o Chromium vai
/// direto a este computador: esses dois não são loopback para o bypass.
pub fn localhost_url(url: &str) -> Option<String> {
    let mut parsed = url::Url::parse(url).ok()?;
    let unspecified = match parsed.host()? {
        url::Host::Ipv4(ip) => ip.is_unspecified(),
        url::Host::Ipv6(ip) => ip.is_unspecified(),
        url::Host::Domain(_) => false,
    };
    if !unspecified { return None; }
    parsed.set_host(Some("localhost")).ok()?;
    Some(parsed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// O DNS e as interfaces deste computador, de mentira. `slow.example` nunca responde.
    #[derive(Default)]
    struct Fake { names: HashMap<&'static str, Vec<IpAddr>>, interfaces: Vec<Prefix>, no_interfaces: bool, asked: std::cell::RefCell<Vec<String>> }

    impl Net for Fake {
        async fn resolve(&self, host: &str, port: u16) -> io::Result<Vec<SocketAddr>> {
            self.asked.borrow_mut().push(host.to_owned());
            if host == "slow.example" { return std::future::pending().await; }
            self.names.get(host).map(|ips| ips.iter().map(|ip| SocketAddr::new(*ip, port)).collect()).ok_or_else(|| io::ErrorKind::NotFound.into())
        }
        fn interfaces(&self) -> io::Result<Arc<[Prefix]>> {
            if self.no_interfaces { return Err(io::ErrorKind::Other.into()); }
            Ok(self.interfaces.clone().into())
        }
        // estavel: prazo curto do DNS; o esperado em `slow.example` é justamente ele vencer
        fn resolve_wait(&self) -> Duration { Duration::from_millis(50) }
    }

    fn ip(text: &str) -> IpAddr { text.parse().unwrap() }

    fn prefix(addr: &str, mask: &str) -> Prefix { Prefix { addr: ip(addr), mask: ip(mask) } }

    /// A LAN deste computador nos testes: um /24 IPv4 público e um /64 IPv6 global, das faixas de documentação.
    fn lan() -> Vec<Prefix> { vec![prefix("203.0.113.7", "255.255.255.0"), prefix("2001:db8:1::7", "ffff:ffff:ffff:ffff::")] }

    /// O que o listener do modo `mode` faz com o pedido `sent`.
    async fn decide(mode: Mode, sent: &[u8], net: &Fake) -> Route {
        match run(sent).await.0.unwrap() {
            Ok((host, port)) => route(mode, &host, port, net).await,
            Err(code) => Route::Refuse(code),
        }
    }

    /// Manda `sent` como cliente e devolve o que `negotiate` leu e o que ele respondeu.
    async fn run(sent: &[u8]) -> (io::Result<Result<(String, u16), u8>>, Vec<u8>) {
        let (mut client, mut server) = tokio::io::duplex(1024);
        client.write_all(sent).await.unwrap();
        let decided = timeout(Duration::from_secs(5), negotiate(&mut server)).await.expect("decide no prazo");
        drop(server);
        let mut answered = Vec::new();
        client.read_to_end(&mut answered).await.unwrap();
        (decided, answered)
    }

    fn connect(kind: u8, address: &[u8], port: u16) -> Vec<u8> {
        let mut bytes = vec![5, 1, 0, 5, 1, 0, kind];
        bytes.extend_from_slice(address);
        bytes.extend_from_slice(&port.to_be_bytes());
        bytes
    }

    fn name(host: &str) -> Vec<u8> { [&[host.len() as u8][..], host.as_bytes()].concat() }

    #[tokio::test]
    async fn connect_to_loopback_gives_the_port() {
        for request in [connect(3, &name("localhost"), 3000), connect(3, &name("127.0.0.1"), 3000), connect(3, &name("::1"), 3000),
            connect(1, &[127, 0, 0, 1], 3000), connect(4, &std::net::Ipv6Addr::LOCALHOST.octets(), 3000), connect(3, &name("0.0.0.0"), 3000)] {
            let (_, answered) = run(&request).await;
            assert_eq!(answered, [5, 0], "só a escolha do método; a resposta do CONNECT espera o túnel");
            let net = Fake::default();
            for mode in [Mode::Person, Mode::Agent] {
                assert_eq!(decide(mode, &request, &net).await, Route::Tunnel(3000), "{mode:?} {request:?}");
            }
            assert!(net.asked.borrow().is_empty(), "loopback não passa pelo DNS");
        }
    }

    #[tokio::test]
    async fn other_destinations_and_commands_are_refused_with_their_code() {
        let net = Fake::default();
        assert_eq!(decide(Mode::Person, &connect(3, &name("example.com"), 80), &net).await, Route::Refuse(NOT_ALLOWED));
        assert_eq!(decide(Mode::Person, &connect(1, &[192, 0, 2, 10], 80), &net).await, Route::Refuse(NOT_ALLOWED));
        assert!(net.asked.borrow().is_empty(), "a pessoa só manda loopback para cá");
        let mut bind = connect(3, &name("localhost"), 3000);
        bind[4] = 2;
        assert_eq!(run(&bind).await.0.unwrap(), Err(COMMAND_NOT_SUPPORTED));
        assert_eq!(run(&[5, 1, 0, 5, 1, 0, 9, 0, 80]).await.0.unwrap(), Err(ADDRESS_NOT_SUPPORTED));
    }

    #[tokio::test]
    async fn the_agent_never_reaches_the_network_of_this_computer() {
        let net = Fake {
            names: HashMap::from([("intranet.example", vec![ip("10.0.0.7")]), ("rebind.example", vec![ip("198.51.100.4"), ip("192.168.0.9")]),
                ("loop.example", vec![ip("127.0.0.1")]), ("own.example", vec![ip("203.0.113.7")])]),
            interfaces: lan(),
            ..Default::default()
        };
        // O Chromium manda o IP como nome (atyp 3), sem colchetes no IPv6.
        for host in ["10.1.2.3", "172.16.5.4", "172.31.255.255", "192.168.1.1", "169.254.169.254", "100.64.0.1", "100.64.0.3",
            "0.1.2.3", "224.0.0.251", "239.255.255.250", "255.255.255.255", "fd00::1", "fc00::1", "fe80::1", "[fe80::1]", "ff02::1",
            "::ffff:192.168.0.1", "64:ff9b::c0a8:1", "203.0.113.7", "2001:db8:1::7",
            // A rede inteira das interfaces, não só o próprio endereço: a LAN com IPv6 global ou IPv4 público.
            "203.0.113.200", "2001:db8:1::abcd", "[2001:db8:1:0:ffff::1]", "::ffff:203.0.113.9", "64:ff9b::cb00:7109",
            // IPv4-compatível, site-local, IP falso de VPN, 192.0.0.0/24, 6to4, Teredo e NAT64 local.
            "::c000:20a", "::203.0.113.30", "fec0::1", "feff::1", "198.18.0.1", "198.19.255.254", "192.0.0.8",
            "2002:c000:20a::1", "2002:c0a8:1::1", "2001::1", "2001:0:4136:e378::1", "64:ff9b:1::c000:20a",
            "intranet.example", "rebind.example", "loop.example", "own.example", "printer.local", "Printer.Local."] {
            assert_eq!(decide(Mode::Agent, &connect(3, &name(host), 80), &net).await, Route::Refuse(NOT_ALLOWED), "{host}");
        }
        assert_eq!(decide(Mode::Agent, &connect(1, &[192, 168, 0, 1], 80), &net).await, Route::Refuse(NOT_ALLOWED));
        assert_eq!(decide(Mode::Agent, &connect(4, &"fe80::1".parse::<Ipv6Addr>().unwrap().octets(), 80), &net).await,
            Route::Refuse(NOT_ALLOWED));
        assert!(!net.asked.borrow().iter().any(|h| h.ends_with(".local")), ".local nem vai ao DNS");
    }

    #[tokio::test]
    async fn the_agent_connects_public_addresses_as_resolved_here() {
        let net = Fake { names: HashMap::from([("site.example", vec![ip("198.51.100.20"), ip("2001:db8::20")])]), ..Default::default() };
        let direct = |addrs: &[&str], port| Route::Direct(addrs.iter().map(|a| SocketAddr::new(ip(a), port)).collect());
        assert_eq!(decide(Mode::Agent, &connect(3, &name("192.0.2.10"), 80), &net).await, direct(&["192.0.2.10"], 80));
        assert_eq!(decide(Mode::Agent, &connect(1, &[203, 0, 113, 30], 8080), &net).await, direct(&["203.0.113.30"], 8080));
        assert_eq!(decide(Mode::Agent, &connect(3, &name("2001:db8::5"), 443), &net).await, direct(&["2001:db8::5"], 443));
        assert_eq!(decide(Mode::Agent, &connect(3, &name("Site.Example."), 443), &net).await, direct(&["198.51.100.20", "2001:db8::20"], 443));
        assert_eq!(decide(Mode::Agent, &connect(3, &name("missing.example"), 443), &net).await, Route::Refuse(HOST_UNREACHABLE));
        // Vizinhos das faixas recusadas e da LAN continuam de fora.
        let net = Fake { interfaces: lan(), ..Default::default() };
        for host in ["198.17.255.1", "198.20.0.1", "192.0.1.1", "2001:1::1", "2003::1", "64:ff9b:2::1", "203.0.114.1", "2001:db8:1:1::1"] {
            assert_eq!(decide(Mode::Agent, &connect(3, &name(host), 80), &net).await, direct(&[host], 80), "{host}");
        }
    }

    #[tokio::test]
    async fn dns_that_does_not_answer_and_unknown_interfaces_are_refused() {
        let net = Fake::default();
        let decided = timeout(Duration::from_secs(5), decide(Mode::Agent, &connect(3, &name("slow.example"), 443), &net)).await;
        assert_eq!(decided.expect("o prazo do DNS vence antes"), Route::Refuse(HOST_UNREACHABLE));
        let blind = Fake { no_interfaces: true, ..Default::default() };
        assert_eq!(decide(Mode::Agent, &connect(3, &name("198.51.100.20"), 80), &blind).await, Route::Refuse(GENERAL_FAILURE));
    }

    #[test]
    fn a_prefix_matches_only_its_own_family_and_bits() {
        let v4 = prefix("192.0.2.130", "255.255.255.192");
        assert!(v4.contains(ip("192.0.2.129")) && v4.contains(ip("192.0.2.191")));
        assert!(!v4.contains(ip("192.0.2.127")) && !v4.contains(ip("192.0.2.192")) && !v4.contains(ip("::ffff:192.0.2.130")));
        let v6 = prefix("2001:db8:aa::1", "ffff:ffff:ffff::");
        assert!(v6.contains(ip("2001:db8:aa:ffff::9")) && !v6.contains(ip("2001:db8:ab::1")) && !v6.contains(ip("192.0.2.130")));
        let host = prefix("198.51.100.7", "255.255.255.255");
        assert!(host.contains(ip("198.51.100.7")) && !host.contains(ip("198.51.100.8")));
    }

    #[tokio::test]
    async fn a_client_without_the_no_auth_method_is_turned_away() {
        let (decided, answered) = run(&[5, 1, 2]).await;
        assert!(decided.is_err());
        assert_eq!(answered, [5, 0xff]);
        assert!(run(&[4, 1, 0]).await.0.is_err(), "SOCKS4 não é deste servidor");
    }

    #[test]
    fn loopback_is_the_machine_itself_and_unspecified_counts() {
        for host in ["localhost", "LOCALHOST.", "app.localhost", "127.0.0.1", "127.8.9.1", "::1", "[::1]", "0.0.0.0", "::", "::ffff:127.0.0.1"] {
            assert!(loopback(host), "{host}");
        }
        for host in ["example.com", "localhost.example.com", "192.168.0.10", "10.0.0.1", "fe80::1", "notlocalhost"] {
            assert!(!loopback(host), "{host}");
        }
    }

    #[test]
    fn unspecified_hosts_become_localhost_and_the_rest_stays() {
        assert_eq!(localhost_url("http://0.0.0.0:5173/a?b=1").as_deref(), Some("http://localhost:5173/a?b=1"));
        assert_eq!(localhost_url("http://[::]:3000/").as_deref(), Some("http://localhost:3000/"));
        assert_eq!(localhost_url("http://localhost:3000/"), None);
        assert_eq!(localhost_url("https://example.com/"), None);
        assert_eq!(localhost_url("about:blank"), None);
    }

    #[test]
    fn tunnel_refusals_map_to_socks_codes() {
        assert_eq!(code_of(ForwardError::NothingListening), REFUSED);
        assert_eq!(code_of(ForwardError::PortRefused), NOT_ALLOWED);
        assert_eq!(code_of(ForwardError::Unreachable), GENERAL_FAILURE);
    }
}
