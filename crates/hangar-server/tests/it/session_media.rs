use super::fake::{Fake, OWNER, client, spawn_fake};
use axum::http::StatusCode;
use hangar_server::{
    auth::TrustedHosts,
    config::Config,
    locate::{BoxFuture, InfoFailed, Locate},
    routes::{AppState, router},
    runtime::gateway::RuntimeRegistry,
    transcript::InternalInfo,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Sessions(Mutex<HashMap<String, InternalInfo>>);
impl Locate for Sessions {
    fn info<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<InternalInfo>, InfoFailed>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().get(name).cloned()) })
    }
}

struct Scene {
    dir: tempfile::TempDir,
    sessions: Arc<Sessions>,
    python: Arc<Fake>,
    runtime: Arc<RuntimeRegistry>,
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Scene {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Scene {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let (python, upstream) = spawn_fake().await;
        let cfg = Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream,
            auth_token: OWNER.into(),
            internal_secret: "isolated".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        };
        let sessions = Arc::new(Sessions::default());
        let mut state = AppState::new(cfg);
        state.side.locate = sessions.clone();
        state.hangar_dir = dir.path().join(".hangar");
        let runtime = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "s".into(), "i".into()));
        assert!(state.state.runtime.set(runtime.clone()).is_ok());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>())
                .await
                .unwrap();
        });
        Self { dir, sessions, python, runtime, address, task }
    }
    async fn post(&self, path: &str, body: &str) -> (StatusCode, Value) {
        let response = client().post(format!("http://{}{path}", self.address)).bearer_auth(OWNER)
            .header("content-type", "application/json").body(body.to_owned()).send().await.unwrap();
        let status = response.status();
        (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
    }
    fn progress_file(&self, id: &str) -> PathBuf {
        let dir = self.dir.path().join(".hangar/tool-progress");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(format!("{id}.jsonl"))
    }
    /// Sessão Claude com o transcript dado.
    fn claude(&self, name: &str, lines: &[Value]) -> PathBuf {
        let path = self.dir.path().join(format!("{name}.jsonl"));
        std::fs::write(&path, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
        self.sessions.0.lock().unwrap().insert(name.into(), info(&path, "claude"));
        path
    }
    async fn get_raw(&self, path: &str) -> reqwest::Response {
        client().get(format!("http://{}{path}", self.address)).bearer_auth(OWNER).send().await.unwrap()
    }
    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let response = self.get_raw(path).await;
        let status = response.status();
        (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
    }
}

fn info(path: &Path, provider: &str) -> InternalInfo {
    let cwd = path.parent().unwrap().join("projeto");
    serde_json::from_value(json!({"provider": provider, "jsonl": path, "cwd": cwd, "session_key": path.file_stem().unwrap().to_str().unwrap(), "lifecycle_id": "life", "history": {}})).unwrap()
}

// --- tool-progress ---

#[tokio::test]
async fn tool_progress_reads_steps_of_unknown_session_too() {
    let scene = Scene::new().await;
    std::fs::write(
        scene.progress_file("toolu_abc"),
        "{\"t\": 1.5, \"message\": \"abrindo\"}\nnão é json\n[1]\n{\"t\": null, \"message\": 3}\n{\"message\": \"sem t\"}\n",
    )
    .unwrap();
    let (status, body) = scene.get("/api/sessions/qualquer/tool-progress/toolu_abc").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body, json!([{"t": 1.5, "message": "abrindo"}, {"t": null, "message": "sem t"}]));
}

#[tokio::test]
async fn tool_progress_without_file_is_empty() {
    let scene = Scene::new().await;
    let (status, body) = scene.get("/api/sessions/s/tool-progress/toolu_nada").await;
    assert_eq!((status, body), (StatusCode::OK, json!([])));
}

#[tokio::test]
async fn tool_progress_keeps_last_200_lines_before_dropping_invalid_and_cuts_message_at_500_chars() {
    let scene = Scene::new().await;
    let long = "é".repeat(600);
    let mut text = String::new();
    for i in 0..250 {
        text.push_str(&format!("{{\"t\": {i}, \"message\": \"m{i}\"}}\n"));
    }
    text.push_str("lixo\n");
    text.push_str(&format!("{{\"t\": 999, \"message\": \"{long}\"}}\n"));
    std::fs::write(scene.progress_file("toolu_long"), text).unwrap();
    let (status, body) = scene.get("/api/sessions/s/tool-progress/toolu_long").await;
    assert_eq!(status, StatusCode::OK);
    let items = body.as_array().unwrap();
    // 200 últimas linhas = 198 válidas (52..249) + lixo + a longa.
    assert_eq!(items.len(), 199);
    assert_eq!(items[0], json!({"t": 52, "message": "m52"}));
    assert_eq!(items[198]["message"].as_str().unwrap().chars().count(), 500);
}

#[tokio::test]
async fn tool_progress_rejects_ids_out_of_shape_including_trailing_newline() {
    let scene = Scene::new().await;
    std::fs::write(scene.progress_file("toolu_x\n"), "{\"t\":1,\"message\":\"vazou\"}\n").unwrap_or(());
    for id in ["abc", "toolu_", "toolu_a.b", "toolu_x%0A", &format!("toolu_{}", "a".repeat(81))] {
        let (status, body) = scene.get(&format!("/api/sessions/s/tool-progress/{id}")).await;
        assert_eq!((status, body), (StatusCode::BAD_REQUEST, json!({"detail": "invalid tool_use_id"})), "{id}");
    }
}

#[tokio::test]
async fn tool_progress_skips_lines_that_are_not_utf8() {
    let scene = Scene::new().await;
    let mut bytes = b"{\"t\": 1, \"message\": \"ok\"}\n".to_vec();
    bytes.extend_from_slice(b"{\"t\": 2, \"message\": \"\xff\xfe\"}\n");
    std::fs::write(scene.progress_file("toolu_bin"), bytes).unwrap();
    let (status, body) = scene.get("/api/sessions/s/tool-progress/toolu_bin").await;
    assert_eq!((status, body), (StatusCode::OK, json!([{"t": 1, "message": "ok"}])));
}

// --- transcript-image ---

const PNG: &str = "iVBORw0KGgo=";

fn image_line(uuid: &str, media: &str, data: &str) -> Value {
    json!({"type":"user","uuid":uuid,"timestamp":"2026-09-01T10:00:03.000Z","message":{"role":"user","content":[
        {"type":"text","text":"[Image #1] olha isto"},
        {"type":"image","source":{"type":"base64","media_type":media,"data":data}}]}})
}

#[tokio::test]
async fn transcript_image_serves_the_pasted_block_with_immutable_cache() {
    let scene = Scene::new().await;
    scene.claude("s", &[json!({"type":"user","uuid":"u-001","message":{"role":"user","content":"oi"}}), image_line("u-003", "image/png", PNG)]);
    let response = scene.get_raw("/api/sessions/s/transcript-image/u-003/0").await;
    assert_eq!(response.status(), StatusCode::OK);
    let h = response.headers().clone();
    assert_eq!(h["content-type"], "image/png");
    assert_eq!(h["cache-control"], "max-age=31536000, immutable");
    assert_eq!(h["x-content-type-options"], "nosniff");
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"\x89PNG\r\n\x1a\n");
}

#[tokio::test]
async fn transcript_image_not_found_cases() {
    let scene = Scene::new().await;
    scene.claude("s", &[
        json!({"type":"user","uuid":"texto","message":{"role":"user","content":"só texto"}}),
        image_line("u-003", "image/png", PNG),
        json!({"type":"user","uuid":"url","message":{"role":"user","content":[{"type":"image","source":{"type":"url","url":"http://x"}}]}}),
        json!({"type":"user","uuid":"tool","message":{"role":"user","content":[{"type":"tool_result","content":[{"type":"image","source":{"type":"base64","media_type":"image/png","data":PNG}}]}]}}),
        image_line("pad", "image/png", "iVBORw0KGgo"),
    ]);
    for (uuid, idx) in [("u-003", "1"), ("u-003", "-1"), ("nenhum", "0"), ("texto", "0"), ("url", "0"), ("tool", "0"), ("pad", "0")] {
        let (status, body) = scene.get(&format!("/api/sessions/s/transcript-image/{uuid}/{idx}")).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{uuid}/{idx}");
        assert_eq!(body["detail"]["code"], "erro_imagem_nao_encontrada", "{uuid}/{idx}");
    }
    let (status, body) = scene.get("/api/sessions/outra/transcript-image/u-003/0").await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("erro_sessao_inexistente")));
}

#[tokio::test]
async fn transcript_image_base64_is_lenient_like_python() {
    let scene = Scene::new().await;
    scene.claude("s", &[image_line("u", "image/png", "iVBO\nRw0K!Ggo=")]);
    let response = scene.get_raw("/api/sessions/s/transcript-image/u/0").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"\x89PNG\r\n\x1a\n");
}

#[tokio::test]
async fn transcript_image_never_serves_active_content_types() {
    let scene = Scene::new().await;
    scene.claude("s", &[image_line("html", "text/html", PNG), image_line("svg", "image/svg+xml", PNG), image_line("jpg", "image/jpeg", PNG),
        json!({"type":"user","uuid":"sem","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","data":PNG}}]}})]);
    for (uuid, expected) in [("html", "application/octet-stream"), ("svg", "application/octet-stream"), ("jpg", "image/jpeg"), ("sem", "image/png")] {
        let response = scene.get_raw(&format!("/api/sessions/s/transcript-image/{uuid}/0")).await;
        assert_eq!(response.status(), StatusCode::OK, "{uuid}");
        assert_eq!(response.headers()["content-type"], expected, "{uuid}");
        assert_eq!(response.headers()["x-content-type-options"], "nosniff", "{uuid}");
    }
}

#[tokio::test]
async fn transcript_image_of_a_transfer_stays_with_python() {
    let scene = Scene::new().await;
    scene.claude("s", &[image_line("u", "image/png", PNG)]);
    let path = "/api/sessions/s/transcript-image/transfer:rec:claude:u/0";
    let _ = scene.get_raw(path).await;
    assert_eq!(scene.python.hits_to(path), 1);
    let _ = scene.get_raw("/api/sessions/s/transcript-image/u/0").await;
    assert_eq!(scene.python.hits_to("/api/sessions/s/transcript-image/u/0"), 0);
}

// --- anexos ---

impl Scene {
    async fn upload(&self, name: &str, body: &'static [u8], filename: &str) -> (StatusCode, Value) {
        let response = client().post(format!("http://{}/api/sessions/{name}/upload", self.address)).bearer_auth(OWNER)
            .header("x-filename", filename).body(body).send().await.unwrap();
        let status = response.status();
        (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
    }
    fn facts_asked(&self, name: &str) -> usize {
        self.python.hits_to(&format!("/internal/sessions/{name}/uploads-facts"))
    }
}

#[tokio::test]
async fn uploads_never_ask_python_for_the_session_facts() {
    let scene = Scene::new().await;
    scene.claude("s", &[]);
    let (status, saved) = scene.upload("s", b"abc", "nota%20final.TXT").await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let path = PathBuf::from(saved["path"].as_str().unwrap());
    // O cofre grava pelo caminho real: no macOS a pasta temporária é link para /private/var.
    let vault = std::fs::canonicalize(scene.dir.path().join(".hangar/uploads")).unwrap();
    assert!(std::fs::canonicalize(&path).unwrap().starts_with(&vault), "{path:?}");
    assert_eq!(path.parent().unwrap().file_name().unwrap(), "s", "pasta chaveada pelo id do transcript");
    assert_eq!(path.extension().unwrap(), "txt");
    assert_eq!((saved["frames"].clone(), saved["transcript"].clone()), (json!([]), json!("")));
    let (status, listed) = scene.get("/api/sessions/s/uploads").await;
    let filename = path.file_name().unwrap().to_str().unwrap();
    assert_eq!((status, listed["files"][0]["filename"].clone(), listed["files"][0]["size"].clone()), (StatusCode::OK, json!(filename), json!(3)));
    let response = scene.get_raw(&format!("/api/sessions/s/uploads/{filename}")).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"].to_str().unwrap().split(';').next(), Some("text/plain"));
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"abc");
    assert_eq!(scene.facts_asked("s"), 0);
}

#[tokio::test]
async fn uploads_answer_missing_session_and_cwd_with_python_codes() {
    let scene = Scene::new().await;
    let (status, body) = scene.upload("nenhuma", b"abc", "a.txt").await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("erro_sessao_inexistente")));
    let path = scene.claude("semcwd", &[]);
    let mut without = info(&path, "claude");
    without.cwd = None;
    scene.sessions.0.lock().unwrap().insert("semcwd".into(), without);
    let (status, body) = scene.upload("semcwd", b"abc", "a.txt").await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("erro_cwd_indisponivel")));
    let (status, body) = scene.get("/api/sessions/semcwd/uploads").await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("erro_cwd_indisponivel")));
    assert_eq!(scene.facts_asked("nenhuma") + scene.facts_asked("semcwd"), 0);
}

#[tokio::test]
async fn upload_waits_for_the_transfer_but_listing_does_not() {
    let scene = Scene::new().await;
    scene.claude("s", &[]);
    scene.runtime.ingress().hold("s", std::time::Duration::from_millis(10)).await.unwrap();
    let (status, body) = scene.upload("s", b"abc", "a.txt").await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("session_transfer_busy")));
    assert_eq!(scene.get("/api/sessions/s/uploads").await, (StatusCode::OK, json!({"files": []})));
}

#[test]
fn upload_retention_follows_runtime_config_then_environment_then_thirty_days() {
    use hangar_server::uploads::http::retention_days;
    assert_eq!(retention_days(&json!({"upload_retention_days": 7}), Some("9")), 7);
    assert_eq!(retention_days(&json!({"upload_retention_days": 0}), Some("9")), 0);
    assert_eq!(retention_days(&json!({}), Some(" 9 ")), 9);
    assert_eq!(retention_days(&Value::Null, Some("x")), 30);
    assert_eq!(retention_days(&Value::Null, None), 30);
}

#[tokio::test]
async fn video_transcription_never_goes_through_python() {
    let (python, upstream) = spawn_fake().await;
    let state = AppState::new(Config { listen: "127.0.0.1:0".parse().unwrap(), upstream, auth_token: OWNER.into(),
        internal_secret: "isolated".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    let text = hangar_server::uploads::media::transcribe(&state, b"audio".to_vec()).await;
    assert_eq!(text, "");
    assert_eq!(python.hits_to("/internal/sessions/s/upload-transcript"), 0);
}

// --- transcribe ---

/// Servidor com um provedor de transcrição de mentira (devolve "ola mundo" e guarda o corpo) e a
/// configuração servida pelo Python falso; qualquer outra chamada ao Python fica registrada.
struct Speech {
    scene: Scene,
    heard: Arc<Mutex<Vec<Vec<u8>>>>,
    python_paths: Arc<Mutex<Vec<String>>>,
    private: SocketAddr,
    _tasks: Vec<tokio::task::JoinHandle<()>>,
}
async fn serve_router(router: axum::Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    (address, tokio::spawn(async move { axum::serve(listener, router).await.unwrap() }))
}
impl Speech {
    async fn new(providers: Value) -> Self {
        let heard: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        let keep = heard.clone();
        let (provider, provider_task) = serve_router(axum::Router::new().route("/audio/transcriptions", axum::routing::post(move |body: axum::body::Bytes| {
            keep.lock().unwrap().push(body.to_vec());
            async { ([("content-type", "application/json")], r#"{"text":"ola mundo"}"#) }
        }))).await;
        let providers = if providers.is_null() {
            json!([{"id": "p", "kind": "openai", "model": "m", "base_url": format!("http://{provider}")}])
        } else { providers };
        let config = json!({"providers": providers, "organization": {"dictation_organization_mode": "none"}}).to_string();
        let python_paths: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = python_paths.clone();
        let (upstream, upstream_task) = serve_router(axum::Router::new()
            .route("/internal/transcription/config", axum::routing::get(move || { let c = config.clone(); async move { ([("content-type", "application/json")], c) } }))
            .fallback(move |uri: axum::http::Uri| {
                if crate::app_route(uri.path()) { seen.lock().unwrap().push(uri.path().to_owned()); }
                async { StatusCode::NOT_FOUND }
            })).await;
        let mut scene = Scene::new().await;
        scene.task.abort();
        let cfg = Config { listen: "127.0.0.1:0".parse().unwrap(), upstream, auth_token: OWNER.into(), internal_secret: "isolated".into(),
            log_path: None, trusted: TrustedHosts::parse("127.0.0.1") };
        let mut state = AppState::new(cfg);
        state.side.locate = scene.sessions.clone();
        state.hangar_dir = scene.dir.path().join(".hangar");
        assert!(state.state.runtime.set(scene.runtime.clone()).is_ok());
        let state = Arc::new(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        scene.address = listener.local_addr().unwrap();
        let public = state.clone();
        scene.task = tokio::spawn(async move {
            axum::serve(listener, router(public).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let private = listener.local_addr().unwrap();
        let private_task = tokio::spawn(async move {
            axum::serve(listener, hangar_server::routes::terminal_router(state).into_make_service_with_connect_info::<SocketAddr>()).await.unwrap();
        });
        Self { scene, heard, python_paths, private, _tasks: vec![provider_task, upstream_task, private_task] }
    }
    /// O pedido que o Python repassa de uma porta dele, já autenticado lá.
    async fn forwarded(&self, path: &str, query: &str, body: &'static [u8], guest: bool) -> (StatusCode, Value) {
        let mut req = client().post(format!("http://{}/__hangar_server/forwarded{query}", self.private))
            .header("x-hangar-internal", "isolated").header("x-hangar-path", path).header("x-filename", "a.webm").body(body);
        if guest {
            req = req.header("x-hangar-forwarded-guest", "1");
        }
        let response = req.send().await.unwrap();
        let status = response.status();
        (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
    }
    async fn transcribe(&self, query: &str, body: &'static [u8], filename: Option<&str>) -> (StatusCode, Value) {
        let mut req = client().post(format!("http://{}/api/sessions/s/transcribe{query}", self.scene.address)).bearer_auth(OWNER).body(body);
        if let Some(f) = filename {
            req = req.header("x-filename", f);
        }
        let response = req.send().await.unwrap();
        let status = response.status();
        (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
    }
    fn heard_contains(&self, needle: &[u8]) -> bool {
        self.heard.lock().unwrap().iter().any(|b| b.windows(needle.len()).any(|w| w == needle))
    }
}

#[tokio::test]
async fn transcribe_saves_the_audio_and_answers_without_cleaning() {
    let speech = Speech::new(Value::Null).await;
    speech.scene.claude("s", &[]);
    let (status, body) = speech.transcribe("", b"\x00audio-bytes", Some("gravacao.webm")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let path = body["path"].as_str().unwrap().to_owned();
    assert!(path.ends_with(".webm"));
    assert_eq!(std::fs::read(&path).unwrap(), b"\x00audio-bytes");
    assert_eq!(body, json!({"path": path, "text": "ola mundo", "provider": body["provider"].clone()}));
    assert!(body["provider"].is_string());
    assert!(speech.heard_contains(b"\x00audio-bytes"));
    assert_eq!(*speech.python_paths.lock().unwrap(), Vec::<String>::new(), "nada além da configuração vai ao Python");
}

#[tokio::test]
async fn transcribe_without_a_provider_is_503_with_code() {
    let speech = Speech::new(json!([])).await;
    speech.scene.claude("s", &[]);
    let (status, body) = speech.transcribe("", b"audio", Some("a.webm")).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert!(body["detail"]["code"].as_str().unwrap().starts_with("transcription_"), "{body}");
}

#[tokio::test]
async fn transcribe_with_cleaning_none_keeps_the_text_and_says_who_transcribed() {
    let speech = Speech::new(Value::Null).await;
    speech.scene.claude("s", &[]);
    let (status, body) = speech.transcribe("?limpar=1&organization_mode=none", b"audio", Some("ditado.wav")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["text"].clone(), body["raw"].clone(), body["organization_mode"].clone(), body["estilo_aplicado"].clone()),
        (json!("ola mundo"), json!("ola mundo"), json!("none"), json!("cru")));
    assert!(body["aviso"].is_null() && body["provider"].is_string() && body["path"].is_string(), "{body}");
}

#[tokio::test]
async fn transcribe_with_an_unknown_style_keeps_the_text_with_the_request_code() {
    let speech = Speech::new(Value::Null).await;
    speech.scene.claude("s", &[]);
    let (status, body) = speech.transcribe("?limpar=1&estilo=inexistente&organization_mode=external_api", b"audio", Some("a.wav")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!((body["text"].clone(), body["organization_code"].clone(), body["organization_mode"].clone(), body["estilo_aplicado"].clone()),
        (json!("ola mundo"), json!("dictation_request_invalid"), json!("external_api"), json!("cru")));
}

#[tokio::test]
async fn transcribe_by_file_reuses_the_upload_without_a_second_copy() {
    let speech = Speech::new(Value::Null).await;
    speech.scene.claude("s", &[]);
    let saved = client().post(format!("http://{}/api/sessions/s/upload?audio_only=1", speech.scene.address)).bearer_auth(OWNER)
        .header("x-filename", "ditado.webm").body(&b"audio-do-ditado"[..]).send().await.unwrap().text().await.unwrap();
    let saved: Value = serde_json::from_str(&saved).unwrap();
    let saved = saved["path"].as_str().unwrap().to_owned();
    let name = Path::new(&saved).file_name().unwrap().to_str().unwrap().to_owned();
    let (status, body) = speech.transcribe(&format!("?arquivo={name}"), b"", None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["path"], json!(saved));
    assert!(speech.heard_contains(b"audio-do-ditado"));
    assert_eq!(std::fs::read_dir(Path::new(&saved).parent().unwrap()).unwrap().count(), 1);
}

#[tokio::test]
async fn transcribe_by_file_refuses_names_outside_the_folder_and_missing_files() {
    let speech = Speech::new(Value::Null).await;
    speech.scene.claude("s", &[]);
    for name in ["..%2Fsegredo.webm", "a%2Fb.webm", "..", "x%5Cy.webm"] {
        let (status, _) = speech.transcribe(&format!("?arquivo={name}"), b"", None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{name}");
    }
    let (status, body) = speech.transcribe("?arquivo=1-abc.webm", b"", None).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::NOT_FOUND, json!("erro_upload_inexistente")));
    assert!(speech.heard.lock().unwrap().is_empty());
}

#[tokio::test]
async fn transcribe_checks_session_transfer_and_query_types() {
    let speech = Speech::new(Value::Null).await;
    speech.scene.claude("s", &[]);
    let (status, body) = speech.transcribe("?limpar=talvez", b"audio", Some("a.wav")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let (status, body) = speech.transcribe("?include_recent_messages=x", b"audio", Some("a.wav")).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    let response = client().post(format!("http://{}/api/sessions/nenhuma/transcribe", speech.scene.address)).bearer_auth(OWNER)
        .body("a").send().await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(body["detail"]["code"], "erro_sessao_inexistente");
    speech.scene.runtime.ingress().hold("s", std::time::Duration::from_millis(10)).await.unwrap();
    let (status, body) = speech.transcribe("?limpar=talvez", b"audio", Some("a.wav")).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::CONFLICT, json!("session_transfer_busy")));
}

#[tokio::test]
async fn forwarded_from_python_ports_is_served_here_and_keeps_the_guest_limits() {
    let speech = Speech::new(Value::Null).await;
    speech.scene.claude("s", &[]);
    let (status, saved) = speech.forwarded("/api/sessions/s/upload", "?audio_only=1", b"audio-antigo", false).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let absolute = saved["path"].as_str().unwrap().to_owned();
    let query = format!("?arquivo={}", percent_encoding::utf8_percent_encode(&absolute, percent_encoding::NON_ALPHANUMERIC));
    let (status, body) = speech.forwarded("/api/sessions/s/transcribe", &query, b"", true).await;
    assert_eq!((status, body["detail"]["code"].clone()), (StatusCode::FORBIDDEN, json!("erro_arquivo_caminho_convidado")), "{body}");
    let (status, body) = speech.forwarded("/api/sessions/s/transcribe", &query, b"", false).await;
    assert_eq!((status, body["path"].clone()), (StatusCode::OK, json!(absolute)), "{body}");
    let (status, body) = speech.forwarded("/api/sessions/nenhuma/bash-output", "", br#"{"command": "ls"}"#, true).await;
    assert_eq!((status, body), (StatusCode::OK, json!({"text": null})));
    assert_eq!(*speech.python_paths.lock().unwrap(), Vec::<String>::new());
}

#[test]
fn unreadable_retention_config_never_prunes_with_the_default() {
    use hangar_server::uploads::http::retention_from;
    assert_eq!(retention_from(Ok(br#"{"upload_retention_days": 365}"#.to_vec()), None), Ok(365));
    assert_eq!(retention_from(Err(std::io::ErrorKind::NotFound.into()), Some("9")), Ok(9));
    assert_eq!(retention_from(Err(std::io::ErrorKind::NotFound.into()), None), Ok(30));
    assert_eq!(retention_from(Ok(b"{trunc".to_vec()), None), Err("upload_retention_config_invalid"));
    assert_eq!(retention_from(Err(std::io::ErrorKind::PermissionDenied.into()), None), Err("upload_retention_config_unreadable"));
}

#[test]
fn bash_output_says_when_the_process_table_cannot_be_read() {
    use hangar_server::list::procs::{ChildrenMap, ProcessView};
    struct Broken;
    impl ProcessView for Broken {
        fn children(&self, _: std::time::Duration) -> std::io::Result<Arc<ChildrenMap>> { Err(std::io::ErrorKind::PermissionDenied.into()) }
        fn cwd(&self, _: i64) -> Option<PathBuf> { None }
        fn env_var(&self, _: i64, _: &str) -> std::io::Result<Option<std::ffi::OsString>> { Ok(None) }
        fn start_time(&self, _: i64) -> Option<f64> { None }
    }
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("sessions")).unwrap();
    std::fs::write(dir.path().join(format!("sessions/{}.json", std::process::id())),
        json!({"sessionId": "sa", "pid": std::process::id(), "status": "busy", "updatedAt": 1000}).to_string()).unwrap();
    let result = hangar_server::session_media::bash_output(&[dir.path().to_path_buf()], "sa", "ls", &Broken);
    assert_eq!(result, Err("bash_output_procs_unreadable"));
}

#[test]
fn transcript_image_read_failure_is_an_error_not_a_missing_image() {
    let dir = tempfile::tempdir().unwrap();
    // Um diretório no lugar do transcript: abrir dá certo no Unix e ler falha.
    let result = hangar_server::session_media::find_image(dir.path(), "u", 0);
    assert!(result.is_err(), "{result:?}");
}

#[test]
fn dictation_uses_the_dictation_deadline_and_attachments_the_file_one() {
    use hangar_server::uploads::transcribe::profile;
    assert_eq!(serde_json::to_value(profile(true)).unwrap(), json!("dictation"));
    assert_eq!(serde_json::to_value(profile(false)).unwrap(), json!("file"));
}

#[test]
fn video_extensions_are_the_ones_the_app_serves() {
    use hangar_server::uploads::media::is_video;
    for ext in ["mp4", "mov", "webm", "mkv", "m4v", "avi"] {
        assert!(is_video(Path::new(&format!("/tmp/a.{ext}"))), "{ext}");
        assert!(is_video(Path::new(&format!("/tmp/A.{}", ext.to_uppercase()))), "{ext}");
    }
    for ext in ["png", "jpg", "mp3", "pdf", ""] {
        assert!(!is_video(Path::new(&format!("/tmp/a.{ext}"))), "{ext}");
    }
}

#[test]
fn gallery_breaks_same_second_ties_by_name_not_directory_order() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path().join("project/session");
    std::fs::create_dir_all(&folder).unwrap();
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    for name in ["c.bin", "a.bin", "b.bin"] {
        std::fs::write(folder.join(name), b"x").unwrap();
        std::fs::File::options().write(true).open(folder.join(name)).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(when)).unwrap();
    }
    let store = hangar_server::uploads::store::UploadStore::new(dir.path()).unwrap();
    let names: Vec<String> = store.list("project", "session", 0, 1_700_000_000.0).unwrap().into_iter().map(|e| e.filename).collect();
    assert_eq!(names, ["a.bin", "b.bin", "c.bin"]);
}

#[test]
fn transcription_warnings_join_the_fallback_and_the_cleaning() {
    use hangar_server::uploads::transcribe::with_provider;
    let fallback = Some("Transcrito pelo Groq: ElevenLabs não respondeu");
    assert_eq!(with_provider(json!({"text": "t", "raw": "t", "aviso": "a limpeza devolveu texto vazio — ficou o original"}), "Groq", fallback),
        json!({"text": "t", "raw": "t", "provider": "Groq", "aviso_transcricao": fallback,
            "aviso": "Transcrito pelo Groq: ElevenLabs não respondeu · a limpeza devolveu texto vazio — ficou o original"}));
    assert_eq!(with_provider(json!({"text": "Olá.", "aviso": null, "estilo_aplicado": "prosa"}), "Groq", fallback),
        json!({"text": "Olá.", "aviso": fallback, "estilo_aplicado": "prosa", "provider": "Groq", "aviso_transcricao": fallback}));
    assert_eq!(with_provider(json!({"path": "p", "text": "ola"}), "Groq", fallback),
        json!({"path": "p", "text": "ola", "provider": "Groq", "aviso": fallback, "aviso_transcricao": fallback}));
    assert_eq!(with_provider(json!({"path": "p", "text": "ola"}), "p", None), json!({"path": "p", "text": "ola", "provider": "p"}));
}

// --- bash-output ---

#[tokio::test]
async fn bash_output_waits_for_no_body_while_the_session_is_held() {
    let scene = Scene::new().await;
    scene.runtime.ingress().hold("s", std::time::Duration::from_millis(10)).await.unwrap();
    let (status, body) = scene.post("/api/sessions/s/bash-output", "não é json").await;
    assert_eq!(status, StatusCode::CONFLICT);
    assert_eq!(body["detail"]["code"], "session_transfer_busy");
}

#[tokio::test]
async fn bash_output_body_is_strict() {
    let scene = Scene::new().await;
    let long = json!({"command": "x".repeat(200_001)}).to_string();
    for body in ["{}", r#"{"command": 3}"#, r#"{"command": "ls", "mais": 1}"#, "[]", long.as_str()] {
        let (status, reply) = scene.post("/api/sessions/s/bash-output", body).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{}", &body[..body.len().min(40)]);
        assert!(reply["detail"].is_array(), "{reply}");
    }
    let exact = json!({"command": "é".repeat(200_000)}).to_string();
    assert_eq!(scene.post("/api/sessions/s/bash-output", &exact).await.0, StatusCode::OK);
}

#[tokio::test]
async fn bash_output_of_unknown_session_is_null() {
    let scene = Scene::new().await;
    let (status, body) = scene.post("/api/sessions/nenhuma/bash-output", r#"{"command": "sleep 30"}"#).await;
    assert_eq!((status, body), (StatusCode::OK, json!({"text": null})));
    assert_eq!(scene.python.hits_to("/api/sessions/nenhuma/bash-output"), 0);
}

#[test]
fn bash_output_tail_is_16000_bytes_decoded_with_replacement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.output");
    let mut bytes = "é".as_bytes().to_vec();
    bytes.extend(std::iter::repeat_n(b'a', 15_999));
    std::fs::write(&path, &bytes).unwrap();
    let text = hangar_server::session_media::read_tail(&path, 16_000).unwrap().unwrap();
    assert!(text.starts_with('\u{fffd}'), "{:?}", &text[..4]);
    assert_eq!(text.chars().filter(|&c| c == 'a').count(), 15_999);
    assert_eq!(hangar_server::session_media::read_tail(&dir.path().join("sumiu"), 16_000), Ok(None));
    assert_eq!(hangar_server::session_media::read_tail(dir.path(), 16_000), Err("bash_output_file_unreadable"));
}

#[test]
fn task_output_paths_follow_the_claude_layout() {
    use hangar_server::session_media::is_task_output;
    assert!(is_task_output(Path::new("/tmp/claude-1000/-proj/abc/tasks/b1.output")));
    #[cfg(windows)]
    assert!(is_task_output(Path::new(r"C:\Users\x\AppData\Local\Temp\claude\proj\tasks\b1.output")));
    assert!(!is_task_output(Path::new("/tmp/claude-1000/proj/outros/b1.output")));
    assert!(!is_task_output(Path::new("/tmp/x/proj/tasks/b1.output")));
    assert!(!is_task_output(Path::new("/tmp/claude-1000/proj/tasks/b1.log")));
}

// No macOS a saída ao vivo do Bash não é lida (ver `stdout_files`).
#[cfg(any(target_os = "linux", windows))]
#[test]
fn stdout_files_of_a_child_include_its_redirected_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.txt");
    let file = std::fs::File::create(&path).unwrap();
    let mut command = if cfg!(windows) {
        let mut c = std::process::Command::new("ping");
        c.args(["-n", "30", "127.0.0.1"]);
        c
    } else {
        let mut c = std::process::Command::new("sleep");
        c.arg("30");
        c
    };
    let mut child = command.stdout(file).spawn().unwrap();
    let files = hangar_server::session_media::stdout_files(child.id() as i64).unwrap();
    let _ = child.kill();
    let _ = child.wait();
    let wanted = std::fs::canonicalize(&path).unwrap();
    assert!(files.iter().any(|f| std::fs::canonicalize(f).ok().as_ref() == Some(&wanted)), "{files:?}");
}

/// Agente de mentira: um `sh` que roda o Bash do Claude Code (`… && eval '<cmd>' && pwd`) com a
/// saída num arquivo; o grupo inteiro morre no fim.
#[cfg(target_os = "linux")]
struct Agent(std::process::Child);
#[cfg(target_os = "linux")]
impl Agent {
    fn spawn(mark: &str, out: &Path) -> Self {
        use std::os::unix::process::CommandExt;
        let script = format!("sh -c \"true && eval 'printenv MARK; sleep 30' && pwd\" > '{}'; true", out.display());
        Self(std::process::Command::new("sh").arg("-c").arg(script).env("MARK", mark).process_group(0).spawn().unwrap())
    }
    fn pid(&self) -> i64 { self.0.id() as i64 }
}
#[cfg(target_os = "linux")]
impl Drop for Agent {
    fn drop(&mut self) {
        let _ = std::process::Command::new("kill").args(["-TERM", "--", &format!("-{}", self.0.id())]).status();
        let _ = self.0.wait();
    }
}

#[cfg(target_os = "linux")]
fn native(cfg: &Path, sid: &str, pid: i64) {
    std::fs::create_dir_all(cfg.join("sessions")).unwrap();
    std::fs::write(cfg.join(format!("sessions/{pid}.json")), json!({"sessionId": sid, "pid": pid, "status": "busy", "updatedAt": 1000}).to_string()).unwrap();
}

#[cfg(target_os = "linux")]
fn written(path: &Path) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::fs::metadata(path).map(|m| m.len()).unwrap_or(0) == 0 {
        assert!(std::time::Instant::now() < until, "o agente não escreveu em {}", path.display());
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

#[cfg(target_os = "linux")]
#[test]
fn bash_output_reads_only_the_command_under_the_session_agent() {
    use hangar_server::{list::procs::SystemProcs, session_media::bash_output};
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("conta");
    let tasks = dir.path().join("claude-1000/proj/sess/tasks");
    let loose = dir.path().join("claude-1000/proj/sess/outros");
    std::fs::create_dir_all(&tasks).unwrap();
    std::fs::create_dir_all(&loose).unwrap();
    let (a, b, c) = (Agent::spawn("a", &tasks.join("a.output")), Agent::spawn("b", &tasks.join("b.output")), Agent::spawn("c", &loose.join("c.output")));
    native(&cfg, "sa", a.pid());
    native(&cfg, "sb", b.pid());
    native(&cfg, "sc", c.pid());
    for f in [tasks.join("a.output"), tasks.join("b.output"), loose.join("c.output")] {
        written(&f);
    }
    let procs = SystemProcs::default();
    let dirs = [cfg];
    let command = "  printenv MARK; sleep 30 ";
    assert_eq!(bash_output(&dirs, "sa", command, &procs), Ok(Some("a\n".to_owned())));
    assert_eq!(bash_output(&dirs, "sb", command, &procs), Ok(Some("b\n".to_owned())));
    assert_eq!(bash_output(&dirs, "sc", command, &procs), Ok(None), "saída fora de tasks/ não conta");
    assert_eq!(bash_output(&dirs, "sd", command, &procs), Ok(None), "sessão sem agente não lê a de outra");
    assert_eq!(bash_output(&dirs, "sa", "outro comando", &procs), Ok(None));
    assert_eq!(bash_output(&dirs, "sa", "   ", &procs), Ok(None));
}
