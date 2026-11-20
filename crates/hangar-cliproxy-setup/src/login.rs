//! Login das contas pela API de gerenciamento v8 do proxy (`/v8/management/oauth/*`).
use std::time::Duration;

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(15);

/// A última senha de gerenciamento recusada. Cada recusa conta para o bloqueio do 127.0.0.1, login incluído:
/// pedido de fundo (versão, nomear contas) não repete uma senha que já falhou.
static REJECTED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub fn rejected(key: &str) -> bool {
    REJECTED.lock().unwrap_or_else(|e| e.into_inner()).as_deref() == Some(key)
}

pub fn note_rejected(key: &str) {
    *REJECTED.lock().unwrap_or_else(|e| e.into_inner()) = Some(key.to_owned());
}

/// Senha aceita pelo proxy volta a valer para os pedidos de fundo.
pub fn note_accepted(key: &str) {
    let mut rejected = REJECTED.lock().unwrap_or_else(|e| e.into_inner());
    if rejected.as_deref() == Some(key) {
        *rejected = None;
    }
}

#[derive(Clone, Debug)]
pub struct Management {
    pub base_url: String,
    pub key: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    BadInput(&'static str),
    Rejected,
    Disabled,
    /// Proxy anterior ao v8: as rotas de login que o Hangar usa (`/v8/management/oauth`) não existem nele.
    Outdated(String),
    /// A escuta da volta (1455 no ChatGPT, 54545 no Claude) já está em uso, em geral por outro login.
    PortBusy,
    Network(String),
    Proxy(u16, String),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::BadInput(_) => "erro_cliproxy_login_invalido",
            Self::Rejected => "erro_cliproxy_senha_recusada",
            Self::Disabled => "erro_cliproxy_gerenciamento_desligado",
            Self::Outdated(_) => "erro_cliproxy_versao_antiga",
            Self::PortBusy => "cliproxy_login_porta_ocupada",
            Self::Network(_) => "cliproxy_login_sem_resposta",
            Self::Proxy(..) => "cliproxy_login_falhou",
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    Wait,
    Ok,
    Failed(String),
}

pub const PROVIDERS: [&str; 2] = ["codex", "claude"];

fn valid_state(state: &str) -> bool {
    !state.is_empty() && state.len() <= 128 && state.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Só o endereço local que o provedor usa de volta, com o `state` deste login; qualquer outro não vai ao proxy.
/// Lido como URL: `http://localhost:@outro/…` começa igual, mas a máquina de verdade é a do `@`.
fn valid_redirect(url: &str, state: &str) -> bool {
    let Ok(parsed) = reqwest::Url::parse(url) else { return false };
    url.len() <= 4096
        && parsed.scheme() == "http"
        && matches!(parsed.host_str(), Some("127.0.0.1" | "localhost"))
        && parsed.username().is_empty()
        && parsed.password().is_none()
        && parsed.query_pairs().find(|(key, _)| key == "state").is_some_and(|(_, value)| value == state)
}

async fn call(m: &Management, request: reqwest::RequestBuilder) -> Result<Value, Error> {
    let response = request
        .header("Authorization", format!("Bearer {}", m.key))
        .timeout(TIMEOUT)
        .send()
        .await
        .map_err(|e| Error::Network(e.to_string()))?;
    let status = response.status().as_u16();
    let version = response.headers().get("x-cpa-version").and_then(|v| v.to_str().ok()).map(str::to_owned);
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let message = body["error"].as_str().unwrap_or("").to_owned();
    match status {
        200..=299 => {
            note_accepted(&m.key);
            Ok(body)
        }
        401 | 403 => Err(Error::Rejected),
        404 => Err(not_found(version)),
        500 if message.contains("callback server") => Err(Error::PortBusy),
        code => Err(Error::Proxy(code, message)),
    }
}

/// 404 tem duas causas: gerenciamento desligado, ou proxy anterior ao v8 sem a rota. A versão que ele anuncia separa.
fn not_found(version: Option<String>) -> Error {
    let major = version.as_deref().and_then(|v| semver::Version::parse(v.trim().trim_start_matches('v')).ok()).map(|v| v.major);
    match (major, version) {
        (Some(major), Some(version)) if major < 8 => Error::Outdated(version),
        _ => Error::Disabled,
    }
}

fn client() -> Result<reqwest::Client, Error> {
    hangar_core::net::http_client().build().map_err(|e| Error::Network(e.to_string()))
}

/// Abre o login no proxy. `is_webui` faz o proxy escutar a volta sozinho: no mesmo computador o
/// login fecha sem colar nada.
pub async fn start(m: &Management, provider: &str) -> Result<(String, String), Error> {
    if !PROVIDERS.contains(&provider) {
        return Err(Error::BadInput("provedor"));
    }
    let url = format!("{}/v8/management/oauth/auth-url?provider={provider}&is_webui=true", m.base_url);
    let body = call(m, client()?.get(url)).await?;
    let (Some(link), Some(state)) = (body["url"].as_str(), body["state"].as_str().filter(|s| valid_state(s))) else {
        return Err(Error::Proxy(200, "resposta sem url ou state".into()));
    };
    Ok((link.to_owned(), state.to_owned()))
}

pub async fn status(m: &Management, state: &str) -> Result<Status, Error> {
    if !valid_state(state) {
        return Err(Error::BadInput("state"));
    }
    // `state` já passou por `valid_state`: vai na URL sem escape.
    let url = format!("{}/v8/management/oauth/status?state={state}", m.base_url);
    let body = call(m, client()?.get(url)).await?;
    Ok(parse_status(&body))
}

pub fn parse_status(body: &Value) -> Status {
    match body["status"].as_str() {
        Some("ok") => Status::Ok,
        Some("error") => Status::Failed(body["error"].as_str().unwrap_or("").to_owned()),
        _ => Status::Wait,
    }
}

/// O celular não alcança o `127.0.0.1` da máquina: a pessoa cola o endereço em que o navegador parou.
pub async fn callback(m: &Management, state: &str, redirect_url: &str) -> Result<(), Error> {
    if !valid_state(state) {
        return Err(Error::BadInput("state"));
    }
    if !valid_redirect(redirect_url, state) {
        return Err(Error::BadInput("endereço de volta"));
    }
    let url = format!("{}/v8/management/oauth/callback", m.base_url);
    call(m, client()?.post(url).json(&json!({ "redirect_url": redirect_url }))).await.map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_reads_the_three_answers() {
        assert_eq!(parse_status(&json!({"status": "ok"})), Status::Ok);
        assert_eq!(parse_status(&json!({"status": "wait"})), Status::Wait);
        assert_eq!(parse_status(&json!({"status": "error", "error": "negado"})), Status::Failed("negado".into()));
    }

    #[test]
    fn only_a_local_return_address_of_the_same_login_is_accepted() {
        assert!(valid_redirect("http://127.0.0.1:1455/auth/callback?state=abc&code=x", "abc"));
        assert!(valid_redirect("http://localhost:54545/callback?code=x&state=abc", "abc"));
        assert!(!valid_redirect("https://evil.example/auth?state=abc", "abc"));
        assert!(!valid_redirect("http://localhost:@evil.example/auth/callback?state=abc", "abc"), "a máquina é a depois do @");
        assert!(!valid_redirect("http://127.0.0.1:x@evil.example/cb?state=abc", "abc"));
        assert!(!valid_redirect("http://127.0.0.1:1455/auth/callback?state=abcdef", "abc"), "state com prefixo igual não vale");
        assert!(!valid_state("a&b") && !valid_state("") && valid_state("Ab-9_z"));
    }

    #[test]
    fn a_missing_route_on_a_pre_v8_proxy_is_not_management_off() {
        assert_eq!(not_found(Some("7.4.2".into())), Error::Outdated("7.4.2".into()));
        assert_eq!(not_found(Some("8.0.79".into())), Error::Disabled);
        assert_eq!(not_found(None), Error::Disabled, "sem versão não dá para afirmar que é antigo");
        assert_eq!(Error::Outdated("7.4.2".into()).code(), "erro_cliproxy_versao_antiga");
    }

    #[tokio::test]
    async fn bad_input_never_reaches_the_proxy() {
        let m = Management { base_url: "http://127.0.0.1:9".into(), key: "k".into() };
        assert_eq!(start(&m, "gemini").await, Err(Error::BadInput("provedor")));
        assert_eq!(callback(&m, "abc", "http://127.0.0.1:1455/auth/callback?state=outro").await, Err(Error::BadInput("endereço de volta")));
    }
}
