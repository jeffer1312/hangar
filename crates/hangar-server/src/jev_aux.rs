//! Modelo auxiliar do Jev: lista os modelos do provedor e detecta o CLIProxyAPI local.
//! O `runtime-config.json` só é lido aqui; quem grava é o `/api/config` do Python.

use crate::routes::{AppState, cors, gate, pass};
use crate::session_write::{detail_body, json_response};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const CLIPROXY_URL: &str = "http://127.0.0.1:8317/v1";
const OTHER_ADDRESS: &str = "erro_jev_aux_chave_outro_endereco";

fn log(text: impl AsRef<str>) { tracing::warn!(target: "jev_aux", "{}", text.as_ref()); }

#[derive(Deserialize)]
struct ModelsBody {
    base_url: String,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    use_saved_key: bool,
    #[serde(default)]
    use_cliproxy_key: bool,
}

/// `access.api-keys` no formato novo, senão `api-keys` na raiz (no novo, a raiz é dos provedores).
/// Mesma ordem do `computer_control._cliproxy_keys`.
fn cliproxy_keys(home: &Path) -> Vec<String> {
    let path = home.join(".cli-proxy-api").join("config.yaml");
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => { log(format!("cliproxy config unreadable kind={:?}", e.kind())); return Vec::new() }
    };
    let config = match serde_saphyr::from_str::<Value>(&text) {
        Ok(config) => config,
        Err(_) => { log("cliproxy config is not valid yaml"); return Vec::new() }
    };
    // Chave sem aspas só de dígitos vira número no YAML: continua sendo chave.
    let as_key = |v: &Value| v.as_str().map(str::to_string).or_else(|| v.is_number().then(|| v.to_string()));
    [&config["access"]["api-keys"], &config["api-keys"]].into_iter()
        .map(|list| list.as_array().into_iter().flatten().filter_map(as_key).map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty()).collect::<Vec<_>>())
        .find(|keys| !keys.is_empty()).unwrap_or_default()
}

/// Endereço e chave salvos, pela regra do Python: o `jev_aux_*` assim que algum existe, senão o
/// `jev_texto_*` de antes da migração.
fn saved(claude_dir: &Path) -> (String, String) {
    let runtime = match std::fs::read(claude_dir.join("runtime-config.json")) {
        Ok(bytes) => serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_| { log("runtime-config is not valid json"); Value::Null }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => { log(format!("runtime-config unreadable kind={:?}", e.kind())); Value::Null }
    };
    let text = |field: &str| runtime[field].as_str().map(str::trim).unwrap_or_default().to_string();
    let aux = ["jev_aux_base_url", "jev_aux_api_key", "jev_aux_modelo", "jev_aux_esforco"].iter().any(|f| runtime.get(*f).is_some());
    if aux { (text("jev_aux_base_url"), text("jev_aux_api_key")) } else { (text("jev_texto_base_url"), text("jev_texto_api_key")) }
}

/// Base do endpoint sem barra final e sem `/chat/completions`, com a query preservada.
fn normalized(url: &str) -> Option<reqwest::Url> {
    let mut url = reqwest::Url::parse(url.trim()).ok().filter(|u| matches!(u.scheme(), "http" | "https") && u.host_str().is_some())?;
    let path = url.path().trim_end_matches('/');
    let path = path.strip_suffix("/chat/completions").unwrap_or(path).to_string();
    url.set_path(&path);
    Some(url)
}

fn same_address(a: &reqwest::Url, b: &str) -> bool { normalized(b).is_some_and(|b| b.as_str().trim_end_matches('/') == a.as_str().trim_end_matches('/')) }

fn is_cliproxy(url: &reqwest::Url) -> bool {
    matches!(url.host_str(), Some("127.0.0.1" | "localhost")) && url.port_or_known_default() == Some(8317)
}

fn failure(status: StatusCode, code: &str, msg: &str, params: Value) -> Response {
    json_response(status, detail_body(code, msg, params))
}

async fn cliproxy_reply(home: std::path::PathBuf) -> Response {
    let has_keys = match tokio::task::spawn_blocking(move || !cliproxy_keys(&home).is_empty()).await {
        Ok(has) => has,
        Err(e) => { log(format!("cliproxy key read failed: {e}")); false }
    };
    let running = tokio::time::timeout(Duration::from_millis(500), tokio::net::TcpStream::connect("127.0.0.1:8317")).await
        .is_ok_and(|r| r.is_ok());
    json_response(StatusCode::OK, json!({"preset_url": CLIPROXY_URL, "running": running, "has_keys": has_keys}))
}

async fn models_reply(st: &AppState, req: Request) -> Response {
    if req.method() != Method::POST { return StatusCode::METHOD_NOT_ALLOWED.into_response(); }
    let home = st.accounts.env.home.clone();
    let claude_dir = st.voice.claude_dir().to_path_buf();
    match axum::body::to_bytes(req.into_body(), 64 * 1024).await.ok()
        .and_then(|bytes| serde_json::from_slice::<ModelsBody>(&bytes).ok()) {
        None => failure(StatusCode::BAD_REQUEST, "erro_corpo_invalido", "corpo inválido", json!({})),
        Some(body) => list(body, home, claude_dir).await,
    }
}

pub async fn cliproxy(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner { return pass(&st, req, &fwd).await; }
    let mut resp = cliproxy_reply(st.accounts.env.home.clone()).await;
    cors(req.headers(), resp.headers_mut());
    resp
}

pub async fn models(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner { return pass(&st, req, &fwd).await; }
    let headers = req.headers().clone();
    let mut resp = models_reply(&st, req).await;
    cors(&headers, resp.headers_mut());
    resp
}

/// Ponte do Python (Connect): o pedido do dono que entrou pela porta dele, já autenticado lá.
pub async fn private(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) { return StatusCode::NOT_FOUND.into_response(); }
    let path = req.headers().get("x-hangar-path").and_then(|v| v.to_str().ok()).unwrap_or_default().to_string();
    match (req.method().clone(), path.as_str()) {
        (Method::GET, "/api/jev/aux/cliproxy") => cliproxy_reply(st.accounts.env.home.clone()).await,
        (_, "/api/jev/aux/models") => models_reply(&st, req).await,
        _ => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn list(body: ModelsBody, home: std::path::PathBuf, claude_dir: std::path::PathBuf) -> Response {
    let Some(base) = normalized(&body.base_url) else {
        return failure(StatusCode::BAD_REQUEST, "erro_jev_aux_url", "o endereço precisa começar com http:// ou https://", json!({}));
    };
    let typed = body.api_key.as_deref().map(str::trim).unwrap_or_default().to_string();
    let wants_saved = body.use_saved_key || (typed.is_empty() && !body.use_cliproxy_key);
    let (keys, (saved_url, saved_key)) = match tokio::task::spawn_blocking(move || (cliproxy_keys(&home), saved(&claude_dir))).await {
        Ok(read) => read,
        Err(e) => { log(format!("key read failed: {e}")); (Vec::new(), (String::new(), String::new())) }
    };
    // Chave guardada só vai ao endereço a que pertence: o rascunho da tela não a leva a outro servidor.
    let key = if body.use_cliproxy_key {
        if !is_cliproxy(&base) {
            return failure(StatusCode::BAD_REQUEST, OTHER_ADDRESS, "digite a chave para este endereço", json!({}));
        }
        match keys.into_iter().next() {
            Some(k) => k,
            None => return failure(StatusCode::BAD_REQUEST, "erro_jev_aux_cliproxy_key",
                "o ~/.cli-proxy-api/config.yaml não tem nenhuma api-key", json!({})),
        }
    } else if wants_saved {
        if saved_key.is_empty() {
            String::new()
        } else if same_address(&base, &saved_url) {
            saved_key
        } else if body.use_saved_key {
            return failure(StatusCode::BAD_REQUEST, OTHER_ADDRESS, "digite a chave para este endereço", json!({}));
        } else {
            // Sem pedir a salva, outro endereço vai sem chave: há endpoint local que não a pede.
            String::new()
        }
    } else {
        typed
    };
    match fetch(&base, &key).await {
        Ok(models) => json_response(StatusCode::OK, json!({"models": models})),
        Err((status, error)) => failure(StatusCode::BAD_GATEWAY, "erro_jev_aux_models", &error,
            json!({"status": status, "error": error})),
    }
}

/// Anthropic pede `x-api-key`; os compatíveis com a OpenAI (CLIProxyAPI, OpenRouter), Bearer.
async fn fetch(base: &reqwest::Url, key: &str) -> Result<Vec<String>, (Option<u16>, String)> {
    // Sem redirect: um 30x levaria a chave a outro host.
    let client = crate::http_client().timeout(Duration::from_secs(8)).redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| (None, e.to_string()))?;
    let anthropic = base.host_str().is_some_and(|h| h == "anthropic.com" || h.ends_with(".anthropic.com"));
    let mut url = base.clone();
    url.set_path(&format!("{}/models", base.path().trim_end_matches('/')));
    // A Anthropic pagina a lista (20 por padrão): sem o teto, modelos somem calados.
    if anthropic { url.query_pairs_mut().append_pair("limit", "1000"); }
    let mut request = client.get(url);
    if anthropic {
        request = request.header("x-api-key", key).header("anthropic-version", "2023-06-01");
    } else if !key.is_empty() {
        request = request.bearer_auth(key);
    }
    let response = request.send().await.map_err(|e| (None, without_key(&e.to_string(), key)))?;
    let status = response.status();
    if status.is_redirection() {
        return Err((Some(status.as_u16()), "o endereço redirecionou para outro lugar; use o endereço final".to_string()));
    }
    let text = response.text().await.map_err(|e| (Some(status.as_u16()), without_key(&e.to_string(), key)))?;
    let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::Null);
    if !status.is_success() {
        return Err((Some(status.as_u16()), without_key(&provider_phrase(&parsed, &text, status.as_u16()), key)));
    }
    let Some(data) = parsed["data"].as_array() else {
        return Err((Some(status.as_u16()), "o endereço respondeu, mas não é uma lista de modelos (confira a base, ex. .../v1)".to_string()));
    };
    let mut ids: Vec<String> = data.iter().filter_map(|m| m["id"].as_str().map(str::to_string)).collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn provider_phrase(parsed: &Value, text: &str, status: u16) -> String {
    let phrase = parsed["error"]["message"].as_str().or_else(|| parsed["error"].as_str()).or_else(|| parsed["message"].as_str())
        .map(str::to_string).unwrap_or_else(|| text.trim().chars().take(300).collect());
    if phrase.is_empty() { format!("o provedor respondeu {status}") } else { phrase }
}

fn without_key(text: &str, key: &str) -> String {
    if key.is_empty() { text.to_string() } else { text.replace(key, "***") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cliproxy_keys_come_from_the_yaml_list() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".cli-proxy-api")).unwrap();
        std::fs::write(home.path().join(".cli-proxy-api/config.yaml"),
            "port: 8317\napi-keys:\n  - \"hangar-abc\"\n  - other\nremote-management:\n  secret-key: x\n").unwrap();
        assert_eq!(cliproxy_keys(home.path()), ["hangar-abc", "other"]);
        std::fs::write(home.path().join(".cli-proxy-api/config.yaml"),
            "access:\n  api-keys:\n    - \"new-key\"\n    - 12345678\napi-keys:\n  gemini:\n    - name: g\n").unwrap();
        assert_eq!(cliproxy_keys(home.path()), ["new-key", "12345678"]);
        assert!(cliproxy_keys(Path::new("/nonexistent")).is_empty());
    }

    #[test]
    fn saved_key_only_goes_to_its_own_address() {
        let base = normalized("https://api.anthropic.com/v1/chat/completions?x=1").unwrap();
        assert_eq!(base.as_str(), "https://api.anthropic.com/v1?x=1");
        assert!(same_address(&base, "https://api.anthropic.com/v1/?x=1"));
        assert!(!same_address(&base, "https://evil.example/v1?x=1"));
        assert!(is_cliproxy(&normalized("http://localhost:8317/v1").unwrap()));
        assert!(!is_cliproxy(&normalized("http://127.0.0.1:9000/v1").unwrap()));
        assert!(normalized("ftp://x/v1").is_none());
    }

    #[tokio::test]
    async fn saved_key_for_another_address_is_refused_before_any_request() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("runtime-config.json"),
            r#"{"jev_aux_base_url":"https://api.anthropic.com/v1","jev_aux_api_key":"secret"}"#).unwrap();
        let body = ModelsBody { base_url: "https://evil.example/v1".into(), api_key: None, use_saved_key: true, use_cliproxy_key: false };
        let resp = list(body, dir.path().to_path_buf(), dir.path().to_path_buf()).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains(OTHER_ADDRESS));
        let body = ModelsBody { base_url: "ftp://implicit".into(), api_key: None, use_saved_key: false, use_cliproxy_key: false };
        assert_eq!(list(body, dir.path().to_path_buf(), dir.path().to_path_buf()).await.status(), StatusCode::BAD_REQUEST);
        let body = ModelsBody { base_url: "https://evil.example/v1".into(), api_key: None, use_saved_key: false, use_cliproxy_key: true };
        assert_eq!(list(body, dir.path().to_path_buf(), dir.path().to_path_buf()).await.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn provider_phrase_prefers_the_error_message() {
        let parsed = json!({"error": {"type": "authentication_error", "message": "invalid x-api-key"}});
        assert_eq!(provider_phrase(&parsed, "", 401), "invalid x-api-key");
        assert_eq!(provider_phrase(&Value::Null, "", 502), "o provedor respondeu 502");
        assert_eq!(without_key("bad key sk-1 here", "sk-1"), "bad key *** here");
    }

    #[tokio::test]
    async fn fetch_lists_sorted_ids_and_returns_the_provider_phrase() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            for (status, body) in [
                ("200 OK", r#"{"data":[{"id":"b"},{"id":"a"},{"x":1}]}"#),
                ("401 Unauthorized", r#"{"error":{"message":"bad key k1"}}"#),
                ("200 OK", "<html>login</html>"),
            ] {
                let reply = format!("HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len());
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;
                tokio::io::AsyncWriteExt::write_all(&mut socket, reply.as_bytes()).await.unwrap();
            }
        });
        let base = normalized(&format!("http://{addr}/v1")).unwrap();
        assert_eq!(fetch(&base, "k1").await.unwrap(), ["a", "b"]);
        assert_eq!(fetch(&base, "k1").await.unwrap_err(), (Some(401), "bad key ***".to_string()));
        assert!(fetch(&base, "k1").await.unwrap_err().1.contains("não é uma lista de modelos"));
    }
}
