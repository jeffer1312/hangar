//! Os pedidos ao servidor: o `api()` do script antigo, com as mesmas frases de falha.
use std::io::Write;
use std::time::Duration;

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderValue};

/// Para onde vai um pedido e com que prazos.
#[derive(Clone, Debug)]
pub(crate) struct Endpoint {
    pub base: String,
    pub token: String,
    pub connect: f64,
    pub total: f64,
    /// Máquina da varredura ou do `--close` remoto: falha vira aviso `⚠`, não `erro:`.
    pub peer: Option<String>,
}

impl Endpoint {
    pub fn with_total(&self, total: f64) -> Self {
        Self { total, ..self.clone() }
    }
}

/// Resposta crua; `None` = sem resposta (o pedido pode ter chegado).
pub(crate) type Fetched = Option<(u16, Vec<u8>)>;

/// Prazo em segundos de uma variável, como o curl aceita; inválido fica no padrão.
pub(crate) fn seconds(raw: Option<&str>, default: f64) -> f64 {
    raw.and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0 && *value < 1e9)
        .unwrap_or(default)
}

pub(crate) async fn fetch(endpoint: &Endpoint, method: &str, path: &str, body: Option<&str>) -> Fetched {
    let url = reqwest::Url::parse(&format!("{}{path}", endpoint.base)).ok()?;
    let client = reqwest::Client::builder().no_proxy()
        .connect_timeout(Duration::from_secs_f64(endpoint.connect)).build().ok()?;
    let method = reqwest::Method::from_bytes(method.as_bytes()).ok()?;
    let auth = HeaderValue::from_bytes(format!("Bearer {}", endpoint.token).as_bytes()).ok()?;
    let mut request = client.request(method, url).timeout(Duration::from_secs_f64(endpoint.total)).header(AUTHORIZATION, auth);
    if let Some(body) = body {
        request = request.header(CONTENT_TYPE, "application/json").body(body.to_owned());
    }
    let response = request.send().await.ok()?;
    let status = response.status().as_u16();
    let bytes = response.bytes().await.ok()?;
    Some((status, bytes.to_vec()))
}

/// O desfecho do `api()`: corpo do 2xx, ou o código já explicado no stderr (1 recusa, 2 sem resposta).
pub(crate) fn report(endpoint: &Endpoint, fetched: Fetched, err: &mut dyn Write) -> Result<Vec<u8>, u8> {
    let Some((status, body)) = fetched else {
        let _ = match &endpoint.peer {
            Some(peer) => writeln!(err, "⚠ servidor '{peer}' fora do ar ({})", endpoint.base),
            None => writeln!(err, "erro: backend inacessível em {} (hangar rodando?)", endpoint.base),
        };
        return Err(2);
    };
    if (200..300).contains(&status) {
        return Ok(body);
    }
    let _ = match &endpoint.peer {
        Some(peer) => write!(err, "⚠ servidor '{peer}' respondeu HTTP {status} ({}): ", endpoint.base),
        None => write!(err, "erro HTTP {status}: "),
    };
    let _ = err.write_all(&body);
    let _ = err.write_all(b"\n");
    Err(1)
}
