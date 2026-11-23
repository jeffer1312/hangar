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
    use std::net::{Ipv4Addr, Ipv6Addr};
    use tokio::net::TcpStream;
    match TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await {
        Ok(tcp) => Ok(tcp),
        Err(_) => TcpStream::connect((Ipv6Addr::LOCALHOST, port)).await,
    }
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
