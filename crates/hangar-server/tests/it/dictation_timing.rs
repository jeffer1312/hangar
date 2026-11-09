//! `dictation.timing`: cada pedido de ditado deixa no diário o tempo de cada etapa, sem texto nem chave.
use axum::{Router, routing::{get, post}};
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::{AppState, router},
};
use serde_json::{Value, json};
use std::{net::SocketAddr, path::{Path, PathBuf}, sync::{Arc, Mutex}};

const SPOKEN: &str = "Fala original do ditado.";

/// Contas num HOME temporário: o diário do teste não cai no da máquina.
pub fn isolated_accounts(home: &Path) -> AccountService {
    AccountService::new(environment(home))
}

fn environment(home: &Path) -> AccountEnvironment {
    let home = home.to_string_lossy().into_owned();
    AccountEnvironment::from_map([("HOME".into(), home.clone()), ("USERPROFILE".into(), home)].into())
}

/// As linhas `dictation.timing` do diário do servidor, esperando até chegarem `count`.
pub async fn timing_lines(home: &Path, count: usize) -> (Vec<Value>, String) {
    let dir: PathBuf = hangar_server::diag::diary_dir_in(&environment(home)).unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let text: String = std::fs::read_dir(&dir).map(|d| d.flatten().map(|e| std::fs::read_to_string(e.path()).unwrap_or_default()).collect())
            .unwrap_or_default();
        let lines: Vec<Value> = text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok())
            .filter(|l| l["evento"] == "dictation.timing").collect();
        if lines.len() >= count || tokio::time::Instant::now() >= deadline {
            return (lines, text);
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

struct Server {
    home: tempfile::TempDir,
    config: Arc<Mutex<Value>>,
    base: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl Server {
    async fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream.local_addr().unwrap();
        let config = Arc::new(Mutex::new(json!({
            "providers": [{"id":"p","name":"Fixture STT","kind":"openai","api_key":"fixture-secret-key",
                "base_url":format!("http://{upstream_addr}/v1"),"model":"fixture"}],
            "organization": {"dictation_organization_mode":"none"}})));
        let served = config.clone();
        let python = tokio::spawn(async move {
            axum::serve(upstream, Router::new()
                .route("/internal/transcription/config", get(move || {
                    let value = served.lock().unwrap().clone();
                    async move { ([("content-type", "application/json")], value.to_string()) }
                }))
                .route("/v1/audio/transcriptions", post(|| async { ([("content-type", "application/json")], json!({"text": SPOKEN}).to_string()) })))
                .await.unwrap();
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: addr,
            upstream: upstream_addr,
            internal_secret: "private".into(),
            auth_token: "owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.hangar_dir = home.path().join(".hangar");
        state.accounts = isolated_accounts(home.path());
        let app = router(Arc::new(state));
        let server = tokio::spawn(async move {
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        Self { home, config, base: format!("http://{addr}"), tasks: vec![python, server] }
    }
    async fn post(&self, path: &str, body: reqwest::Body) -> u16 {
        reqwest::Client::new().post(format!("{}{path}", self.base)).bearer_auth("owner").body(body)
            .send().await.unwrap().status().as_u16()
    }
}

/// Sem texto, chave ou endereço do provedor na linha: o diário vai em relatório de problema.
fn assert_clean(text: &str) {
    for secret in [SPOKEN, "fixture-secret-key", "/v1"] {
        assert!(!text.contains(secret), "{secret:?} vazou no diário: {text}");
    }
}

#[tokio::test]
async fn dictation_timing_records_each_step_of_a_public_dictation() {
    let server = Server::start().await;
    assert_eq!(server.post("/api/dictation/transcribe?limpar=1&organization_mode=none", "audio".into()).await, 200);
    let (lines, text) = timing_lines(server.home.path(), 1).await;
    assert_eq!(lines.len(), 1, "{text}");
    let line = &lines[0];
    assert_eq!((line["rota"].as_str(), line["status"].as_u64(), line["provider"].as_str(), line["bytes"].as_u64(), line["nivel"].as_str()),
        (Some("ditado"), Some(200), Some("Fixture STT"), Some(5), Some("ok")), "{line}");
    for field in ["config_ms", "read_ms", "stt_ms", "organize_ms", "total_ms"] {
        assert!(line[field].is_u64(), "{field} ausente: {line}");
    }
    assert!(line["total_ms"].as_u64() >= line["stt_ms"].as_u64(), "{line}");
    assert_clean(&text);
}

#[tokio::test]
async fn dictation_timing_records_reorganization_and_failure() {
    let server = Server::start().await;
    let body = json!({"texto": SPOKEN, "estilo": "limpar", "organization_mode": "none"}).to_string();
    assert_eq!(server.post("/api/ditado/relimpar", body.into()).await, 200);
    server.config.lock().unwrap()["providers"] = json!([]);
    assert_eq!(server.post("/api/dictation/transcribe?limpar=0", "audio".into()).await, 503);
    let (lines, text) = timing_lines(server.home.path(), 2).await;
    assert_eq!(lines.len(), 2, "{text}");
    let revise = lines.iter().find(|l| l["rota"] == "relimpar").unwrap_or_else(|| panic!("sem relimpar: {text}"));
    assert_eq!((revise["status"].as_u64(), revise["nivel"].as_str()), (Some(200), Some("ok")), "{revise}");
    assert!(revise["organize_ms"].is_u64() && revise["total_ms"].is_u64(), "{revise}");
    let failed = lines.iter().find(|l| l["rota"] == "ditado").unwrap_or_else(|| panic!("sem ditado: {text}"));
    assert_eq!((failed["status"].as_u64(), failed["nivel"].as_str()), (Some(503), Some("erro")), "{failed}");
    assert!(failed["codigo"].as_str().is_some_and(|c| c.starts_with("transcription_")), "{failed}");
    assert_clean(&text);
}

/// A reserva que transcreveu, o pedido de organização que volta cru e a query recusada não saem como `ok` sem código.
#[tokio::test]
async fn dictation_timing_marks_fallback_raw_text_and_refused_query() {
    let server = Server::start().await;
    {
        let mut config = server.config.lock().unwrap();
        let good = config["providers"][0].clone();
        let mut bad = good.clone();
        bad["id"] = json!("q");
        bad["base_url"] = json!(good["base_url"].as_str().unwrap().replace("/v1", "/fora/v1"));
        config["providers"] = json!([bad, good]);
    }
    assert_eq!(server.post("/api/dictation/transcribe?limpar=0", "audio".into()).await, 200);
    let (lines, text) = timing_lines(server.home.path(), 1).await;
    assert_eq!((lines[0]["nivel"].as_str(), lines[0]["codigo"].as_str()), (Some("aviso"), Some("transcription_fallback")), "{text}");
    assert_eq!(server.post("/api/dictation/transcribe?organization_mode=nenhum", "audio".into()).await, 200);
    let (lines, text) = timing_lines(server.home.path(), 2).await;
    assert_eq!((lines[1]["nivel"].as_str(), lines[1]["codigo"].as_str()), (Some("aviso"), Some("dictation_request_invalid")), "{text}");
    assert_eq!(server.post("/api/dictation/transcribe?limpar=talvez", "audio".into()).await, 422);
    let (lines, text) = timing_lines(server.home.path(), 3).await;
    assert_eq!((lines[2]["nivel"].as_str(), lines[2]["codigo"].as_str()), (Some("erro"), Some("http_422")), "{text}");
}
