use std::time::Duration;

use axum::body::Body;
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::HttpConnector;
use hyper_util::rt::{TokioExecutor, TokioTimer};

/// Todo cliente HTTP do servidor nasce aqui. Uma dependência liga `gzip`, `http2` e (no Windows)
/// `native-tls` no reqwest, e o Cargo estende isso a todo cliente: sem estes desligamentos ele
/// passaria a descomprimir sozinho, negociar HTTP/2 e usar o TLS do sistema.
pub fn http_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder().no_gzip().http1_only().tls_backend_rustls()
}

/// Todo socket TCP do servidor, aceito ou aberto. Sem isso o Nagle segura o último pedaço de uma
/// resposta em pedaços até o ACK atrasado do outro lado; o asyncio do Python já liga sozinho.
pub fn nodelay(tcp: &mut tokio::net::TcpStream) {
    if let Err(e) = tcp.set_nodelay(true) {
        tracing::warn!("TCP_NODELAY não ligou: {e}");
    }
}

pub type HttpClient = Client<HttpConnector, Body>;

/// Tamanho máximo do quadro do runtime e do texto de um lote.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

/// Abaixo dos 5 s em que o uvicorn fecha a conexão ociosa: reaproveitar uma que ele está
/// fechando derruba o pedido no meio e vira 502 na tela.
pub const POOL_IDLE: Duration = Duration::from_secs(3);

pub fn client() -> HttpClient {
    let mut connector = HttpConnector::new();
    // Upload em pedaços ao Python esbarra no mesmo Nagle das respostas (ver `nodelay`).
    connector.set_nodelay(true);
    Client::builder(TokioExecutor::new())
        .pool_idle_timeout(POOL_IDLE)
        .pool_timer(TokioTimer::new())
        .build(connector)
}

/// Código de quem recusa abrir prévia de porta que não é página (Postgres, Redis).
pub const PORT_NOT_HTTP: &str = "port_not_http";

/// `127.0.0.1` primeiro; sem nada ali, o `[::1]` (servidor que só escuta em IPv6).
pub async fn connect_loopback(port: u16) -> std::io::Result<tokio::net::TcpStream> {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
    connect_first(&[IpAddr::V4(Ipv4Addr::LOCALHOST), IpAddr::V6(Ipv6Addr::LOCALHOST)], port).await
}

/// Cada endereço na ordem, até um aceitar. O erro que volta segue `loopback_error`.
pub async fn connect_first(ips: &[std::net::IpAddr], port: u16) -> std::io::Result<tokio::net::TcpStream> {
    let mut failed: Option<std::io::Error> = None;
    for ip in ips {
        match tokio::net::TcpStream::connect((*ip, port)).await {
            Ok(tcp) => return Ok(tcp),
            Err(e) => failed = Some(match failed { Some(before) => loopback_error(before, e), None => e }),
        }
    }
    Err(failed.unwrap_or_else(|| std::io::ErrorKind::InvalidInput.into()))
}

/// Sem IPv6 na máquina o `[::1]` falha por outro motivo; aí a recusa do IPv4 é a que diz "nada escuta ali".
fn loopback_error(v4: std::io::Error, v6: std::io::Error) -> std::io::Error {
    use std::io::ErrorKind::ConnectionRefused;
    if v4.kind() == ConnectionRefused && v6.kind() != ConnectionRefused { v4 } else { v6 }
}

/// A porta do loopback respondeu e a resposta não é HTTP. Sem isso, a prévia de um Postgres mostra o
/// 502 cru do proxy. Silêncio dentro do prazo não decide: o `next dev` compila a página no primeiro pedido.
pub async fn answers_not_http(port: u16) -> bool {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let probe = async {
        let mut tcp = connect_loopback(port).await.ok()?;
        tcp.write_all(b"HEAD / HTTP/1.0\r\nHost: localhost\r\n\r\n").await.ok()?;
        let mut head = [0u8; 5];
        // Fechar sem responder também é resposta: servidor HTTP sempre manda a linha de status.
        Some(tcp.read_exact(&mut head).await.is_err() || &head != b"HTTP/")
    };
    tokio::time::timeout(Duration::from_secs(3), probe).await.ok().flatten().unwrap_or(false)
}

pub struct AbortOnDrop(pub tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) { self.0.abort(); }
}

#[cfg(test)]
mod tests {
    use std::io::{Error, ErrorKind};

    #[test]
    fn loopback_error_keeps_the_ipv4_refusal_when_ipv6_is_missing() {
        let pick = |v4, v6| super::loopback_error(Error::from(v4), Error::from(v6)).kind();
        for missing in [ErrorKind::AddrNotAvailable, ErrorKind::NetworkUnreachable, ErrorKind::Unsupported] {
            assert_eq!(pick(ErrorKind::ConnectionRefused, missing), ErrorKind::ConnectionRefused);
        }
        assert_eq!(pick(ErrorKind::ConnectionRefused, ErrorKind::ConnectionRefused), ErrorKind::ConnectionRefused);
        assert_eq!(pick(ErrorKind::TimedOut, ErrorKind::AddrNotAvailable), ErrorKind::AddrNotAvailable);
    }

    #[tokio::test]
    async fn connect_first_tries_in_order_and_an_empty_list_is_an_error() {
        use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // Nada no [::1] dessa porta (ou máquina sem IPv6): a falha dele não impede o 127.0.0.1 depois.
        let tcp = super::connect_first(&[IpAddr::V6(Ipv6Addr::LOCALHOST), IpAddr::V4(Ipv4Addr::LOCALHOST)], port).await.unwrap();
        assert_eq!(tcp.peer_addr().unwrap().ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(super::connect_first(&[], port).await.unwrap_err().kind(), ErrorKind::InvalidInput);
    }
}
