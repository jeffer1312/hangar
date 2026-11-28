//! A rede voltou? Uma sondagem abre o TCP e manda um `HEAD` ao alvo, com prazo curto: qualquer resposta
//! HTTP, até um erro, prova que o caminho até o provedor abriu. Sem passo de DNS à parte: o nome que não
//! resolve já é rede fora.

use std::time::Duration;

use futures_util::future::BoxFuture;

pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub trait Probe: Send + Sync {
    /// `target` é `host:porta` (vira `https://host:porta/`) ou um endereço com esquema, como o do motor.
    fn reachable<'a>(&'a self, target: &'a str) -> BoxFuture<'a, bool>;
}

pub struct HttpProbe {
    client: Option<reqwest::Client>,
}

impl HttpProbe {
    pub fn new() -> Self {
        let built = hangar_core::net::http_client()
            .connect_timeout(PROBE_TIMEOUT)
            .timeout(PROBE_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build();
        match built {
            Ok(client) => Self { client: Some(client) },
            Err(error) => {
                tracing::warn!(%error, "sonda de rede sem cliente HTTP; a continuação não espera a rede");
                Self { client: None }
            }
        }
    }
}

impl Default for HttpProbe {
    fn default() -> Self {
        Self::new()
    }
}

pub fn url(target: &str) -> String {
    if target.contains("://") { target.to_owned() } else { format!("https://{target}/") }
}

impl Probe for HttpProbe {
    fn reachable<'a>(&'a self, target: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            // Sem cliente a sonda não diz nada: esperar por ela deixaria a sessão parada até as 2 h, e os
            // tetos do registro seguram o envio cedo demais.
            let Some(client) = &self.client else { return true };
            matches!(tokio::time::timeout(PROBE_TIMEOUT, client.head(url(target)).send()).await, Ok(Ok(_)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn a_bare_target_becomes_https() {
        assert_eq!(url("api.anthropic.com:443"), "https://api.anthropic.com:443/");
        assert_eq!(url("http://10.0.0.5:8317"), "http://10.0.0.5:8317");
    }

    #[tokio::test]
    async fn any_http_answer_is_reachable_and_a_closed_port_is_not() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            socket.write_all(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n").await.unwrap();
        });
        let probe = HttpProbe::new();
        assert!(probe.reachable(&format!("http://127.0.0.1:{port}/")).await);
        server.await.unwrap();
        // A porta do ouvinte já fechado: ninguém atende.
        assert!(!probe.reachable(&format!("http://127.0.0.1:{port}/")).await);
    }
}
