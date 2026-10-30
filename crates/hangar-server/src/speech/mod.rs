//! Leitura em voz alta, narração e os consumidores do cliente externo de texto.
pub mod config;
pub mod http;
mod body;
mod history;
pub mod tts;
pub mod text_client;
pub mod narration;
pub mod thought;
pub mod translations;
pub mod suggestion;

#[derive(Default)]
pub struct Service {
    pub tts: tts::Service,
    pub thought: thought::Service,
    pub translations: translations::Service,
}

pub async fn configuration(state: &crate::routes::AppState) -> Result<config::SpeechConfig, SpeechError> {
    let home = state.accounts.env.home.clone();
    let claude = state.accounts.env.claude_base.clone();
    let backend = crate::install_root::root(&home).map(|root| root.join("backend"));
    tokio::task::spawn_blocking(move || config::SpeechConfig::load(&home, &claude, backend.as_deref()))
        .await.map_err(|_| SpeechError::new(503, "Não foi possível carregar a configuração de voz."))?
}

/// Acrescenta a capacidade do motor Rust à resposta geral de configuração que já existia.
/// A leitura das credenciais ocorre no arquivo, sem uma consulta interna ao Python.
pub(crate) async fn configuration_reply(state: &crate::routes::AppState, response: axum::response::Response)
    -> axum::response::Response {
    use axum::{body::{Body, to_bytes}, http::{StatusCode, header}};
    if !response.status().is_success() { return response; }
    let (mut parts, body) = response.into_parts();
    let bytes = match to_bytes(body, 4 * 1024 * 1024).await {
        Ok(bytes) => bytes,
        Err(_) => return crate::session_write::json_response(StatusCode::BAD_GATEWAY,
            serde_json::json!({"detail":{"code":"speech_config_response_unreadable","msg":"Não foi possível ler a resposta de configuração."}})),
    };
    let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return axum::response::Response::from_parts(parts, Body::from(bytes));
    };
    let enabled = match configuration(state).await {
        Ok(config) => config.thought_available(),
        Err(_) => {
            state.diag.report("speech.config_failed", "", "speech_config_unavailable", "A configuração da tradução está indisponível.");
            false
        },
    };
    if let Some(readonly) = value.get_mut("somente_leitura").and_then(serde_json::Value::as_object_mut) {
        readonly.insert("traducao_pensamento".into(), serde_json::json!(enabled));
    }
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return axum::response::Response::from_parts(parts, Body::from(bytes));
    };
    parts.headers.remove(header::CONTENT_LENGTH);
    parts.headers.remove(header::ETAG);
    axum::response::Response::from_parts(parts, Body::from(bytes))
}

#[derive(Debug, Clone)]
pub struct SpeechError {
    pub status: u16,
    pub detail: String,
}

impl SpeechError {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self { status, detail: detail.into() }
    }
}

impl std::fmt::Display for SpeechError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.status, self.detail)
    }
}

impl std::error::Error for SpeechError {}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::{Body, to_bytes}, http::Response};
    use serde_json::{Value, json};
    use std::{collections::BTreeMap, sync::Arc};

    #[tokio::test]
    async fn thought_capability_reads_the_file_without_python_and_refreshes_after_writing() {
        let directory = tempfile::tempdir().unwrap();
        let claude = directory.path().join(".claude");
        std::fs::create_dir_all(&claude).unwrap();
        let path = claude.join("runtime-config.json");
        let mut state = crate::routes::AppState::new(crate::config::Config {
            listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:9".parse().unwrap(),
            internal_secret: "fixture-private".into(), auth_token: "fixture-owner".into(),
            log_path: None, trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = crate::accounts::AccountService::new(
            crate::accounts::environment::AccountEnvironment::from_map(BTreeMap::from([
                ("HOME".into(), directory.path().to_string_lossy().into_owned()),
                ("USERPROFILE".into(), directory.path().to_string_lossy().into_owned()),
            ])));
        let state = Arc::new(state);
        for enabled in [true, false, true] {
            std::fs::write(&path, json!({"traduzir_pensamento":enabled,
                "llm_base_url":"https://text.test/v1", "llm_api_key":"fixture-provider-key"}).to_string()).unwrap();
            let reply = Response::new(Body::from(json!({"campos":{}, "somente_leitura":{"terminal_panel":true}}).to_string()));
            let reply = configuration_reply(&state, reply).await;
            assert!(reply.status().is_success());
            let bytes = to_bytes(reply.into_body(), 1024 * 1024).await.unwrap();
            let body: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(body["somente_leitura"]["traducao_pensamento"], enabled);
            assert_eq!(body["somente_leitura"]["terminal_panel"], true);
            assert!(!String::from_utf8_lossy(&bytes).contains("fixture-provider-key"));
        }
    }
}
