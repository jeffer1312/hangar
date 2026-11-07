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

pub struct AbortOnDrop(pub tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) { self.0.abort(); }
}
