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

pub struct AbortOnDrop(pub tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) { self.0.abort(); }
}
