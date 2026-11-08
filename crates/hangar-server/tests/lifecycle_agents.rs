use hangar_server::list::links::runtime_key;

struct Opening(std::sync::Arc<std::sync::Mutex<hangar_server::transcript::InternalInfo>>);

impl hangar_server::locate::Locate for Opening {
    fn info<'a>(&'a self, _: &'a str) -> hangar_server::locate::BoxFuture<'a, Result<Option<hangar_server::transcript::InternalInfo>, hangar_server::locate::InfoFailed>> {
        let info = self.0.lock().unwrap().clone();
        Box::pin(async move { Ok(Some(info)) })
    }
}

fn opening_state(provider: &str, root: &std::path::Path) -> (hangar_server::routes::AppState, std::sync::Arc<std::sync::Mutex<hangar_server::transcript::InternalInfo>>) {
    use std::sync::Arc;
    use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
    hangar_server::diag::disable_diary();
    hangar_server::accounts::quotas::disable_open_network_read();
    let cfg = Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:9".parse().unwrap(),
        internal_secret: "test-internal".into(), auth_token: "owner-test".into(), log_path: None, trusted: TrustedHosts::parse("") };
    let mut state = AppState::new(cfg);
    state.origins_home = root.to_owned();
    state.side.queue_dir = Some(root.join("queue"));
    state.side.runtime.set(Arc::new(hangar_server::runtime::gateway::RuntimeRegistry::new(
        "127.0.0.1:9".parse().unwrap(), "test-internal".into(), "opening-proof".into(),
    ))).ok().expect("um registro por servidor");
    let info = Arc::new(std::sync::Mutex::new(hangar_server::transcript::InternalInfo {
        provider: provider.into(), jsonl: None, session_key: String::new(),
        history: serde_json::json!({"queue":root.join("queue/session.json")}), headless: false,
        lifecycle_id: Some("k:0123456789abcdef0123456789abcdef:g:1".into()), cwd: Some(root.to_owned()),
    }));
    state.side.locate = Arc::new(Opening(info.clone()));
    (state, info)
}

fn opening_router(provider: &str, root: &std::path::Path) -> axum::Router {
    hangar_server::routes::router(std::sync::Arc::new(opening_state(provider, root).0))
}

async fn assert_opening_life_rebinds(acquire_again: bool) {
    use hangar_server::side::{Binding, Out};
    let dir = tempfile::tempdir().unwrap();
    let (state, info) = opening_state("pi", dir.path());
    let first = Binding::from_info(&info.lock().unwrap()).unwrap();
    let lease = state.side.hubs.acquire("session", first, &state.side);
    state.side.hubs.deliver("session", "preview", r#"{"session":"session","text":"texto da vida anterior","md":false,"full":false,"vivo":true}"#);
    let mut attached = lease.hub.attach(None).await.unwrap();
    assert!(attached.frames.iter().any(|frame| String::from_utf8_lossy(frame).contains("texto da vida anterior")));
    info.lock().unwrap().lifecycle_id = Some("k:abcdef0123456789abcdef0123456789:g:2".into());
    let next = Binding::from_info(&info.lock().unwrap()).unwrap();
    let second = acquire_again.then(|| state.side.hubs.acquire("session", next.clone(), &state.side));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if matches!(attached.rx.recv().await.unwrap(), Out::Rebind) { break; }
        }
    }).await.expect("a vida nova reseta a conexão anterior mesmo sem transcript");
    assert_eq!(lease.hub.binding().unwrap().life, next.life);
    let current = lease.hub.attach(None).await.unwrap();
    assert!(current.generation > attached.generation);
    assert!(!current.frames.iter().any(|frame| String::from_utf8_lossy(frame).contains("texto da vida anterior")));
    drop(second);
}

#[tokio::test]
async fn acquiring_a_recreated_opening_session_discards_the_previous_life() {
    assert_opening_life_rebinds(true).await;
}

#[tokio::test]
async fn the_opening_discovery_watcher_discards_the_previous_life() {
    assert_opening_life_rebinds(false).await;
}

#[tokio::test]
async fn invalid_history_limits_are_rejected_without_python() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    for provider in ["pi", "omp", "kimi"] {
        let dir = tempfile::tempdir().unwrap();
        let router = opening_router(provider, dir.path());
        for raw in ["", "abc", "2.5", "1e2", "٠١٢"] {
            let query = raw.bytes().map(|byte| format!("%{byte:02X}")).collect::<String>();
            let response = router.clone().oneshot(request(&format!("/api/sessions/session/history?limit={query}"))).await.unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::UNPROCESSABLE_ENTITY, "{provider}: {raw}");
            let body = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), serde_json::json!({"detail":[{
                "type":"int_parsing","loc":["query","limit"],"msg":"Input should be a valid integer, unable to parse string as an integer","input":raw
            }]}));
        }
    }
}

#[tokio::test]
async fn supported_history_integer_formats_need_no_python() {
    use tower::ServiceExt;
    for provider in ["pi", "omp", "kimi"] {
        let dir = tempfile::tempdir().unwrap();
        let router = opening_router(provider, dir.path());
        for raw in [" 2 ", "+2", "2.0", "1_000", "9223372036854775808", "-1.0"] {
            let query = raw.bytes().map(|byte| format!("%{byte:02X}")).collect::<String>();
            let response = router.clone().oneshot(request(&format!("/api/sessions/session/history?limit={query}"))).await.unwrap();
            assert_eq!(response.status(), axum::http::StatusCode::OK, "{provider}: {raw}");
        }
    }
}

fn request(route: &str) -> axum::extract::Request {
    let mut request = axum::http::Request::builder().uri(route).header("authorization", "Bearer owner-test")
        .body(axum::body::Body::empty()).unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo("127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap()));
    request
}

#[tokio::test]
async fn an_opening_agent_has_empty_history_without_a_transcript_or_python() {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    for provider in ["pi", "omp", "kimi"] {
        let dir = tempfile::tempdir().unwrap();
        let response = opening_router(provider, dir.path()).oneshot(request("/api/sessions/session/history")).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK, "{provider}");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), serde_json::json!([]), "{provider}");
    }
}

#[tokio::test]
async fn an_opening_agent_keeps_its_event_stream_without_inventing_idle() {
    use eventsource_stream::Eventsource;
    use futures_util::StreamExt;
    use tower::ServiceExt;
    for provider in ["pi", "omp", "kimi"] {
        let dir = tempfile::tempdir().unwrap();
        let response = opening_router(provider, dir.path()).oneshot(request("/api/sessions/session/events")).await.unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK, "{provider}");
        assert!(response.headers()["content-type"].to_str().unwrap().starts_with("text/event-stream"));
        let mut events = response.into_body().into_data_stream().eventsource();
        let first = tokio::time::timeout(std::time::Duration::from_secs(2), events.next()).await.unwrap().unwrap().unwrap();
        assert_eq!(first.event, "ping");
        let state = tokio::time::timeout(std::time::Duration::from_millis(250), async {
            loop {
                let event = events.next().await.expect("a conversa continua aberta").unwrap();
                if event.event == "state" { return event; }
            }
        }).await;
        assert!(state.is_err(), "{provider}: não há estado de agente antes da primeira observação comprovada");
    }
}

#[test]
fn a_native_terminal_generation_resolves_to_the_original_runtime_key() {
    let key = "0123456789abcdef0123456789abcdef";
    for generation in [1, 2] {
        let life = format!("k:{key}:g:{generation}");
        assert_eq!(runtime_key(Some(&life)), Some(key));
    }
}

#[test]
fn existing_runtime_and_untracked_terminal_identities_keep_their_meaning() {
    assert_eq!(runtime_key(Some("k:0123456789abcdef0123456789abcdef")), Some("0123456789abcdef0123456789abcdef"));
    assert_eq!(runtime_key(Some("k:terminal_native-key")), Some("terminal_native-key"));
    assert_eq!(runtime_key(Some("k:custom:g:2")), Some("custom:g:2"));
    assert_eq!(runtime_key(Some("t:1700000000")), None);
    assert_eq!(runtime_key(None), None);
}
