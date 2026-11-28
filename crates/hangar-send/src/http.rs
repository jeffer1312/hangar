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

/// Teto do corpo da resposta: nenhuma resposta do Hangar chega perto, e um servidor errado não
/// enche a memória de quem chama.
pub const BODY_LIMIT: usize = 16 * 1024 * 1024;

/// Por que não há resposta para ler.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// A conexão nem abriu (recusada, sem rota, prazo da conexão): o pedido não chegou.
    NoAnswer,
    /// A conexão abriu e a resposta não veio (prazo vencido ou corte no meio): o pedido pode ter
    /// chegado. Fora do envio de recado vale a mesma frase do `NoAnswer`.
    Cut,
    /// O pedido nem saiu (endereço ou token que não cabem num pedido HTTP): a frase diz qual.
    Invalid(String),
    /// A resposta passou do teto: o pedido saiu e foi atendido.
    TooLarge(String),
}

/// Resposta crua: status e corpo, ou o motivo de não haver.
pub(crate) type Fetched = Result<(u16, Vec<u8>), Failure>;

/// Prazo em segundos de uma variável, como o curl aceita; inválido fica no padrão.
pub(crate) fn seconds(raw: Option<&str>, default: f64) -> f64 {
    raw.and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0 && *value < 1e9)
        .unwrap_or(default)
}

pub(crate) async fn fetch(endpoint: &Endpoint, method: &str, path: &str, body: Option<&str>) -> Fetched {
    fetch_with(endpoint, method, path, body, &[]).await
}

/// `fetch` com cabeçalhos a mais (a identidade da sessão que chama).
pub(crate) async fn fetch_with(endpoint: &Endpoint, method: &str, path: &str, body: Option<&str>, headers: &[(&str, String)]) -> Fetched {
    let invalid = |detail: String| Err(Failure::Invalid(detail));
    let address = format!("{}{path}", endpoint.base);
    let Ok(url) = reqwest::Url::parse(&address) else {
        return invalid(format!("endereço inválido: {address}"));
    };
    let Ok(auth) = HeaderValue::from_bytes(format!("Bearer {}", endpoint.token).as_bytes()) else {
        return invalid("o token do .env tem um caractere que não cabe num cabeçalho HTTP".into());
    };
    let Ok(method) = reqwest::Method::from_bytes(method.as_bytes()) else {
        return invalid(format!("método HTTP inválido: {method}"));
    };
    // Sem seguir redirecionamento, como o curl: um 301 é a resposta, não outro destino com o token.
    let client = match reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs_f64(endpoint.connect)).build()
    {
        Ok(client) => client,
        Err(error) => return invalid(format!("não consegui preparar o cliente HTTP: {error}")),
    };
    let mut request = client.request(method, url).timeout(Duration::from_secs_f64(endpoint.total)).header(AUTHORIZATION, auth);
    for (name, value) in headers {
        let Ok(value) = HeaderValue::from_str(value) else {
            return invalid(format!("o valor de {name} não cabe num cabeçalho HTTP"));
        };
        request = request.header(*name, value);
    }
    if let Some(body) = body {
        request = request.header(CONTENT_TYPE, "application/json").body(body.to_owned());
    }
    let mut response = match request.send().await {
        Ok(response) => response,
        Err(error) if error.is_connect() => return Err(Failure::NoAnswer),
        Err(_) => return Err(Failure::Cut),
    };
    let status = response.status().as_u16();
    let mut bytes = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) if bytes.len() + chunk.len() > BODY_LIMIT => {
                return Err(Failure::TooLarge(format!("a resposta de {address} passou de {} MiB", BODY_LIMIT / (1024 * 1024))));
            }
            Ok(Some(chunk)) => bytes.extend_from_slice(&chunk),
            Ok(None) => return Ok((status, bytes)),
            Err(_) => return Err(Failure::Cut),
        }
    }
}

/// O desfecho do `api()`: corpo do 2xx, ou o código já explicado no stderr (1 recusa, 2 sem resposta).
pub(crate) fn report(endpoint: &Endpoint, fetched: Fetched, err: &mut dyn Write) -> Result<Vec<u8>, u8> {
    let (status, body) = match fetched {
        Ok(answer) => answer,
        Err(Failure::NoAnswer | Failure::Cut) => {
            let _ = match &endpoint.peer {
                Some(peer) => writeln!(err, "⚠ servidor '{peer}' fora do ar ({})", endpoint.base),
                None => writeln!(err, "erro: backend inacessível em {} (hangar rodando?)", endpoint.base),
            };
            return Err(2);
        }
        Err(Failure::Invalid(detail) | Failure::TooLarge(detail)) => {
            let _ = match &endpoint.peer {
                Some(peer) => writeln!(err, "⚠ servidor '{peer}': {detail}"),
                None => writeln!(err, "erro: {detail}"),
            };
            return Err(1);
        }
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
