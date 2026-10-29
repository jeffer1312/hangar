//! A mesma borda HTTP dos handlers Python, com o upstream recusando todas as chamadas.
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, UNIX_EPOCH},
};

use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    list::{
        bridge::{ListBridge, ListEnv, parse_dirs},
        facts::FactsClient,
        mux::Mux,
        procs::SystemProcs,
    },
    routes::{AppState, router},
};
use serde_json::{Value, json};

struct Server {
    _root: tempfile::TempDir,
    config: std::path::PathBuf,
    base: String,
    task: tokio::task::JoinHandle<()>,
    state: Arc<AppState>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    async fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let claude = home.join(".claude");
        std::fs::create_dir_all(claude.join("projects")).unwrap();
        std::fs::create_dir_all(claude.join(".hangar-tts")).unwrap();
        for (extension, audio) in [("mp3", "ID3fixture-audio"), ("wav", "RIFFfixture-wave")] {
            let hash = ring::digest::digest(&ring::digest::SHA256, extension.as_bytes())
                .as_ref()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            let path = claude
                .join(".hangar-tts")
                .join(format!("{hash}.{extension}"));
            std::fs::write(&path, audio).unwrap();
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000)),
                )
                .unwrap();
        }
        let mux = root
            .path()
            .join(if cfg!(windows) { "tmux.cmd" } else { "tmux" });
        #[cfg(unix)]
        crate::write_executable(&mux, "#!/bin/sh\nexit 0\n");
        #[cfg(windows)]
        std::fs::write(&mux, "@echo off\r\nexit /b 0\r\n").unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let upstream = crate::refused_address();
        let mut state = AppState::new(Config {
            listen: address,
            upstream,
            internal_secret: "fixture-private".into(),
            auth_token: "fixture-owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.hangar_dir = home.join(".hangar");
        state.accounts = AccountService::new(AccountEnvironment::from_map(BTreeMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
        ])));
        state.list = Arc::new(ListBridge::new(ListEnv { mux:Mux::with_program(&mux,Duration::from_secs(2)), capture_program:mux.into_os_string(),
            procs:Arc::new(SystemProcs::default()), dirs:parse_dirs(&json!({"home":home,"claude":claude,"codex_home":home.join(".codex"),
                "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string()) },
            FactsClient::new(upstream,"fixture-private".into())));
        let state = Arc::new(state);
        let serving = state.clone();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router(serving).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            _root: root,
            config: claude.join("runtime-config.json"),
            base: format!("http://{address}"),
            task,
            state,
        }
    }

    fn configure(&self, value: &Value) {
        std::fs::write(&self.config, value.to_string()).unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn speech_public_group_suggestion_keeps_native_sessions_with_and_without_transfers() {
    use axum::{Router, routing::post};
    let server = Server::start().await;
    crate::list_support::sessions(server._root.path(), 2);
    let prompts = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let capture = prompts.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let model = tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/chat/completions",post(move |axum::Json(body):axum::Json<Value>| {
            let capture=capture.clone();async move {
                capture.lock().unwrap().push(body["messages"][1]["content"].as_str().unwrap().to_owned());
                axum::Json(json!({"choices":[{"message":{"content":"Conferir a tarefa do grupo"}}]}))
            }
        }))).await.unwrap();
    });
    server.configure(&json!({"llm_base_url":format!("http://{address}"),"llm_api_key":"fixture","dictation_organization_mode":"none"}));
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let response = client
        .post(format!("{}/api/pair/task-suggestion", server.base))
        .bearer_auth("fixture-owner")
        .json(&json!({"sessions":["s0","s1","s0","absent"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let first = prompts.lock().unwrap()[0].clone();
    assert_eq!(first.matches("## Sessão s0").count(), 1);
    assert_eq!(first.matches("## Sessão s1").count(), 1);
    assert!(first.contains("linha 0"));
    let home = server._root.path().join("home");
    let source = home.join("recovering.jsonl");
    let raw=r#"{"type":"user","uuid":"recovery","timestamp":"2026-10-29T12:00:00Z","message":{"role":"user","content":"Continuar a recuperação."}}"#.to_owned()+"\n";
    std::fs::write(&source, &raw).unwrap();
    let digest = ring::digest::digest(&ring::digest::SHA256, raw.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let transfers = home.join(".hangar/conversation-transfers");
    std::fs::create_dir_all(&transfers).unwrap();
    std::fs::write(transfers.join("a563a017-af37-42e8-a8e5-2bd837584f8e.json"),json!({
        "id":"a563a017-af37-42e8-a8e5-2bd837584f8e","name":"recovering","source_life":"k:old","phase":"source_stopped",
        "source":{"path":source,"provider":"claude","digest":digest,"selected_uuids":["recovery"]},
        "origin_meta":{"provider":"claude","cwd":home,"jsonl":source,"headless":false},"destination_meta":null,"boundary":null,"error_code":null
    }).to_string()).unwrap();
    server.state.list.invalidate();
    let response = client
        .post(format!("{}/api/pair/task-suggestion", server.base))
        .bearer_auth("fixture-owner")
        .json(&json!({"sessions":["s0","recovering"]}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200, "{}", response.text().await.unwrap());
    let second = prompts.lock().unwrap()[1].clone();
    assert!(second.contains("## Sessão s0"));
    assert!(second.contains("## Sessão recovering"));
    assert!(second.contains("Continuar a recuperação."));
    model.abort();
}

fn fixture() -> Value {
    serde_json::from_str(include_str!("../fixtures/speech/http_contract.json")).unwrap()
}

/// D-011: somente estas mensagens mudam a grafia; os contratos Python ficam intactos.
fn accented_messages(value: &Value) -> Value {
    match value {
        Value::String(text) => Value::String(match text.as_str() {
            "identificador de audio invalido" => "identificador de áudio inválido",
            "audio nao esta mais em cache" => "áudio não está mais em cache",
            "nao sobrou nada pra falar depois de limpar o texto" => "não sobrou nada pra falar depois de limpar o texto",
            "chave do provedor nao configurada: configure a organizacao do texto em Configuracoes -> Voz" =>
                "Chave do provedor não configurada: configure a organização do texto em Configurações → Voz.",
            "chave da ElevenLabs nao configurada — abra Configurações do servidor" =>
                "Chave da ElevenLabs não configurada — abra Configurações do servidor.",
            "provedor desconhecido: outra" => "Provedor desconhecido: outra",
            other => other,
        }.to_owned()),
        Value::Array(values) => Value::Array(values.iter().map(accented_messages).collect()),
        Value::Object(values) => Value::Object(values.iter().map(|(key,value)|(key.clone(),accented_messages(value))).collect()),
        value => value.clone(),
    }
}

#[tokio::test]
async fn speech_public_matches_the_frozen_python_http_contract() {
    let server = Server::start().await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let mut differences = Vec::new();
    for case in fixture()["cases"].as_array().unwrap() {
        server.configure(&case["config"]);
        let mut request = client.request(
            case["method"].as_str().unwrap().parse().unwrap(),
            format!("{}{}", server.base, case["path"].as_str().unwrap()),
        );
        if case["auth"] == true {
            request = request.bearer_auth("fixture-owner");
        }
        if let Some(body) = case.get("raw_body").and_then(Value::as_str) {
            request = request.body(body.to_owned());
        } else if !case["body"].is_null() {
            request = request.json(&case["body"]);
        }
        for (name, value) in case["headers"].as_object().unwrap() {
            request = request.header(name, value.as_str().unwrap());
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = response.bytes().await.unwrap();
        let body = serde_json::from_slice::<Value>(&bytes).ok();
        let expected_body = case.get("response").map(accented_messages);
        let body_matches = if let Some(expected) = expected_body.as_ref() {
            body.as_ref() == Some(expected)
        } else {
            String::from_utf8_lossy(&bytes) == case["response_text"].as_str().unwrap()
        };
        let header_differences: Vec<_> = case["response_headers"].as_object().unwrap().iter().filter_map(|(name, expected)| {
            // A ordem das chaves JSON é irrelevante; o tamanho total permanece conferido no áudio.
            if name == "content-length" && headers.get("content-type").is_some_and(|h| h == "application/json") { return None; }
            (headers.get(name).and_then(|v|v.to_str().ok()) != expected.as_str()).then(|| json!({"name":name,"expected":expected,"actual":headers.get(name).and_then(|v|v.to_str().ok())}))
        }).collect();
        if status != case["status"].as_u64().unwrap() as u16
            || !body_matches
            || !header_differences.is_empty()
        {
            differences.push(json!({"case":case["name"],"status":status,"expected_status":case["status"],"body_matches":body_matches,
                "body":if bytes.len()<5000 { body.unwrap_or_else(||Value::String(String::from_utf8_lossy(&bytes).into_owned())) } else { json!({"bytes":bytes.len()}) },"headers":header_differences}));
        }
    }
    assert!(
        differences.is_empty(),
        "diferenças do contrato HTTP Python: {differences:#?}"
    );
}

#[tokio::test]
async fn speech_preflight_is_rust_owned_without_the_python_upstream() {
    let server = Server::start().await;
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for path in [
        "/api/tts",
        "/api/tts/narrar",
        "/api/tts/voices",
        "/api/tts/saldo",
        "/api/tts/audio/invalid",
        "/api/pensamento/pt",
        "/api/pair/task-suggestion",
        "/api/config-sync/translate",
    ] {
        let response = client
            .request(reqwest::Method::OPTIONS, format!("{}{path}", server.base))
            .header("origin", "https://app.test")
            .header("access-control-request-method", "POST")
            .header(
                "access-control-request-headers",
                "authorization,content-type",
            )
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200, "{path}");
        assert!(
            response
                .headers()
                .contains_key("access-control-allow-origin"),
            "{path}"
        );
    }
}

#[test]
fn speech_routes_include_refusals_and_do_not_claim_unrelated_paths() {
    use hangar_server::speech::http::matches;
    for method in [
        axum::http::Method::GET,
        axum::http::Method::POST,
        axum::http::Method::HEAD,
        axum::http::Method::OPTIONS,
        axum::http::Method::DELETE,
    ] {
        assert!(matches(&method, "/api/tts"));
        assert!(matches(&method, "/api/tts/audio/hash"));
        assert!(!matches(&method, "/api/tts-unknown"));
        assert!(!matches(&method, "/api/tts/audio/hash/child"));
    }
}
