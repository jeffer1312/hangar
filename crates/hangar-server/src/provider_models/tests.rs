use super::*;
use driver::{Io, IoFuture, Timing};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

#[test]
fn python_catalog_contract() {
    let contract: Value = serde_json::from_str(include_str!("fixtures/contract.json")).unwrap();
    for case in contract["cases"].as_array().unwrap() {
        assert_eq!(
            contract_case(case["op"].as_str().unwrap(), &case["input"]),
            case["expected"],
            "{}: {}",
            case["op"],
            case["input"]
        );
    }
}
struct FakeIo {
    frames: Mutex<VecDeque<String>>,
    keys: Mutex<Vec<Value>>,
    editor: Option<String>,
    catalogs: Mutex<VecDeque<Option<Value>>>,
}
impl FakeIo {
    fn frames(values: &Value) -> Self {
        Self {
            frames: Mutex::new(
                values
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| s.as_str().unwrap().to_owned())
                    .collect(),
            ),
            keys: Mutex::new(vec![]),
            editor: None,
            catalogs: Mutex::default(),
        }
    }
    fn next<T: Clone>(values: &Mutex<VecDeque<T>>) -> Option<T> {
        let mut values = values.lock().unwrap();
        if values.len() > 1 {
            values.pop_front()
        } else {
            values.front().cloned()
        }
    }
}
impl Io for FakeIo {
    fn capture(&self) -> IoFuture<'_, Result<String, ModelError>> {
        Box::pin(async { Ok(Self::next(&self.frames).unwrap_or_default()) })
    }
    fn key<'a>(&'a self, key: &'a str, literal: bool) -> IoFuture<'a, Result<(), ModelError>> {
        Box::pin(async move {
            self.keys
                .lock()
                .unwrap()
                .push(json!({"key":key,"literal":literal}));
            Ok(())
        })
    }
    fn editor(&self) -> IoFuture<'_, Option<String>> {
        Box::pin(async { self.editor.clone() })
    }
    fn catalog(&self) -> IoFuture<'_, Option<Value>> {
        Box::pin(async { Self::next(&self.catalogs).flatten() })
    }
}
fn result(result: Result<Value, ModelError>) -> Value {
    match result {
        Ok(v) => json!({"ok":v}),
        Err(e) => json!({"error":{"status":e.status,"detail":e.detail}}),
    }
}
#[tokio::test(start_paused = true)]
async fn python_kimi_driver_contract() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/driver.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let io = FakeIo::frames(&case["frames"]);
        let got = driver::kimi_switch(
            &io,
            case["alias"].as_str(),
            case["display"].as_str(),
            case["effort"].as_str(),
            Timing::default(),
        )
        .await;
        assert_eq!(result(got), case["expected"], "{}", case["name"]);
        assert_eq!(
            json!(*io.keys.lock().unwrap()),
            case["keys"],
            "{}",
            case["name"]
        );
    }
}
#[tokio::test(start_paused = true)]
async fn python_pi_readback_route_contract() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/pi-readback.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let io = FakeIo::frames(&json!([]));
        io.catalogs
            .lock()
            .unwrap()
            .push_back(if case["after"].is_null() {
                None
            } else {
                Some(case["after"].clone())
            });
        let body = &case["body"];
        let got = driver::pi_read_back(
            &io,
            &case["before"],
            body["provider"].as_str(),
            body["model"].as_str(),
            body["effort"].as_str(),
            Timing::default(),
        )
        .await;
        assert_eq!(result(got), case["expected"], "{}", case["name"]);
    }
}
#[tokio::test(start_paused = true)]
async fn kimi_picker_timeout_and_identical_baseline_never_save_global_default() {
    for frames in [
        json!(["vazio"]),
        json!([
            "Switched to K3 with thinking high for this session only.",
            "Select a model",
            "Switched to K3 with thinking high for this session only."
        ]),
    ] {
        let io = FakeIo::frames(&frames);
        let outcome = driver::kimi_switch(&io, Some("p/m"), Some("K3"), None, Timing::default())
            .await
            .unwrap_err();
        assert_eq!(outcome.status, 409);
        let keys = io.keys.lock().unwrap();
        assert_eq!(keys.iter().filter(|v| v["key"] == "Enter").count(), 1);
        assert_eq!(keys.last().unwrap()["key"], "Escape");
    }
}
#[tokio::test(start_paused = true)]
async fn pi_commands_ask_the_editor_and_serialize_literal_commands_once() {
    let mut io = FakeIo::frames(&json!([
        "────────────────────\naviso\n────────────────────"
    ]));
    io.editor = Some("".into());
    driver::pi_commands(
        &io,
        &["/cp-model p m/x".into(), "/cp-think low".into()],
        Timing::default(),
    )
    .await
    .unwrap();
    assert_eq!(
        *io.keys.lock().unwrap(),
        vec![
            json!({"key":"/cp-model p m/x","literal":true}),
            json!({"key":"Enter","literal":false}),
            json!({"key":"/cp-think low","literal":true}),
            json!({"key":"Enter","literal":false})
        ]
    );
    io.editor = Some("rascunho parado".into());
    assert_eq!(
        driver::pi_commands(&io, &["/cp-think low".into()], Timing::default())
            .await
            .unwrap_err()
            .status,
        409
    );
    assert_eq!(io.keys.lock().unwrap().len(), 4);
}
#[test]
fn sidecar_catalog_is_scoped_to_transcript_and_keeps_null_and_absent_defaults() {
    let root = tempfile::tempdir().unwrap();
    let target = pure::sidecar_path("/one/life.jsonl", root.path());
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    for bad in ["bad", "[]", r#"{"models":"bad"}"#] {
        std::fs::write(&target, bad).unwrap();
        assert_eq!(pi_catalog("/one/life.jsonl", root.path()), None);
    }
    std::fs::write(&target, r#"{"models":[],"current":{"provider":"p"}}"#).unwrap();
    let catalog = pi_catalog("/one/life.jsonl", root.path()).unwrap();
    assert_eq!(
        pi_provider("/one/life.jsonl", root.path()).as_deref(),
        Some("p")
    );
    assert!(pi_catalog("/one/new-life.jsonl", root.path()).is_none());
    assert_eq!(
        pure::pi_response(&catalog),
        json!({"models":[],"current":{"provider":"p"},"thinking":null,"levels":[]})
    );
}
#[test]
fn catalog_validation_rejects_empty_format_and_corrupt_identifiers_without_rejecting_labels() {
    assert!(
        catalog::validate_models("pi", "--list-models", "changed", vec![])
            .unwrap_err()
            .contains("nao devolveu modelo nenhum")
    );
    assert!(
        catalog::validate_models(
            "pi",
            "--list-models",
            "",
            vec![json!({"provider":"p","id":"m�"})]
        )
        .unwrap_err()
        .contains("ilegivel")
    );
    assert!(
        catalog::validate_models(
            "pi",
            "--list-models",
            "",
            vec![json!({"provider":"p","id":"m","context":"20�K"})]
        )
        .is_ok()
    );
}
#[test]
fn request_bodies_keep_pydantic_optional_strings_and_reject_extra_keys() {
    assert!(routes::parse_body(br#"{"model":null,"effort":""}"#, "pi").is_ok());
    for (body, kind) in [
        (br#"{"other":1}"#.as_slice(), "extra_forbidden"),
        (br#"{"model":2}"#, "string_type"),
    ] {
        let error = routes::parse_body(body, "pi").unwrap_err();
        assert_eq!(error.status, 422);
        assert_eq!(error.detail[0]["type"], kind);
    }
}
#[test]
fn dispatch_claims_only_provider_routes_and_global_queries_for_these_providers() {
    use axum::http::Method;
    for provider in ["pi", "kimi"] {
        assert!(matches(
            &Method::GET,
            &format!("/api/sessions/x/{provider}/models")
        ));
        assert!(matches(
            &Method::POST,
            &format!("/api/sessions/x/{provider}/model")
        ));
    }
    for provider in ["pi", "omp", "kimi"] {
        assert!(catalog_request(
            &Method::GET,
            "/api/model-options",
            Some(&format!("provider={provider}"))
        ));
    }
    assert!(!catalog_request(
        &Method::GET,
        "/api/model-options",
        Some("provider=claude")
    ));
    assert!(!matches(&Method::POST, "/api/sessions/x/pi/models"));
}
#[tokio::test]
async fn inbox_empty_editor_is_an_answer_and_reading_does_not_resolve_delivery() {
    let store = inbox::Store::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let line = store.register("session", tx);
    let pending = {
        let store = store.clone();
        tokio::spawn(async move { store.deliver("session", "texto", Some("stable-id")).await })
    };
    let delivery = rx.recv().await.unwrap();
    assert_eq!(delivery["id"], "stable-id");
    assert_eq!(delivery["deliverAs"], "steer");
    let reading = {
        let store = store.clone();
        tokio::spawn(async move { store.question("session", "editor").await })
    };
    let question = rx.recv().await.unwrap();
    assert_eq!(question["pedir"], "editor");
    assert!(question.get("text").is_none());
    store.respond("session", &json!({"id":question["id"],"resposta":""}));
    assert_eq!(reading.await.unwrap(), Some(String::new()));
    assert!(!pending.is_finished());
    store.respond("session", &json!({"id":delivery["id"],"ok":true}));
    assert_eq!(pending.await.unwrap(), "sent");
    store.remove("session", &line);
}
#[tokio::test(start_paused = true)]
async fn inbox_replacement_identity_timeout_and_drop_do_not_confirm_or_erase_new_line() {
    let store = inbox::Store::new();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let old = store.register("session", tx);
    assert_eq!(store.line_for(Some("different"), Some("%1")), None);
    assert_eq!(
        store.line_for(Some("session"), Some("%1")).as_deref(),
        Some("session")
    );
    let pending = {
        let store = store.clone();
        tokio::spawn(async move { store.question("session", "editor").await })
    };
    let _ = rx.recv().await.unwrap();
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let new = store.register("session", tx);
    assert_eq!(pending.await.unwrap(), None);
    store.remove("session", &old);
    assert!(store.has_line("session"));
    assert_eq!(
        store
            .deliver_for("session", "t", Some("id"), Duration::from_millis(10))
            .await,
        "deferred"
    );
    store.remove("session", &new);
    assert_eq!(store.deliver("session", "t", None).await, "sem-linha");
}
#[test]
fn inbox_endpoint_is_written_for_each_account_with_private_permissions_and_public_bind() {
    let root = tempfile::tempdir().unwrap();
    let dirs = vec![root.path().join("a"), root.path().join("b")];
    let written = inbox::write_endpoint(
        &dirs,
        "192.0.2.3:8765".parse().unwrap(),
        "fixture-token",
        123.,
    );
    assert_eq!(written.len(), 2);
    for path in written {
        let data: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            data,
            json!({"url":"ws://192.0.2.3:8765/api/pi/inbox","token":"fixture-token","ts":123.})
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
#[test]
fn kimi_wire_boundary_ignores_config_and_nested_mentions_and_keeps_main_turn_while_agents_work() {
    let root = tempfile::tempdir().unwrap();
    let file = root.path().join("wire.jsonl");
    for (events, expected) in [
        (
            "{\"type\":\"turn.prompt\"}\n{\"type\":\"config.update\"}\n",
            Some(true),
        ),
        (
            "{\"type\":\"turn.ended\"}\n{\"text\":\"turn.prompt\"}\n",
            Some(false),
        ),
        (
            "{\"type\":\"turn.prompt\"}\n{\"type\":\"turn.cancel\"}\n",
            Some(false),
        ),
        ("bad\n{}\n", None),
    ] {
        std::fs::write(&file, events).unwrap();
        assert_eq!(routes::kimi_turn_open(&file), expected);
    }
}
struct CountingSource {
    calls: std::sync::atomic::AtomicUsize,
    fail: std::sync::atomic::AtomicBool,
}
impl catalog::Source for CountingSource {
    fn read<'a>(&'a self, provider: &'a str) -> catalog::CatalogFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
                Err(ModelError::text(502, "indisponível"))
            } else {
                Ok(vec![json!({"provider":provider,"id":"real"})])
            }
        })
    }
}
#[tokio::test(start_paused = true)]
async fn provider_catalog_single_flight_expiration_and_failed_refresh_keep_the_last_good_list() {
    use std::sync::atomic::Ordering;
    let source = Arc::new(CountingSource {
        calls: 0.into(),
        fail: false.into(),
    });
    let catalogs = catalog::Catalogs::new(source.clone(), Duration::from_secs(600));
    assert_eq!(catalogs.read("pi").await.unwrap()[0]["id"], "real");
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    catalogs.read("pi").await.unwrap();
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    catalogs.read("omp").await.unwrap();
    assert_eq!(source.calls.load(Ordering::SeqCst), 2);
    source.fail.store(true, Ordering::SeqCst);
    tokio::time::advance(Duration::from_secs(600)).await;
    for _ in 0..5 {
        assert_eq!(catalogs.read("pi").await.unwrap()[0]["id"], "real");
    }
    tokio::task::yield_now().await;
    assert_eq!(source.calls.load(Ordering::SeqCst), 3);
    source.fail.store(false, Ordering::SeqCst);
    assert_eq!(catalogs.read("pi").await.unwrap()[0]["id"], "real");
}
proptest::proptest! {
 #[test]
 fn external_catalog_and_pane_text_never_panic(input in ".{0,1500}"){
  let _=(pure::parse_pi(&input),pure::parse_omp(&input),pure::parse_kimi(&input),pure::parse_switched(&input),pure::parse_thinking_set(&input),pure::parse_thinking_row(&input),driver::occupied(&input));
 }
}

#[test]
fn inbox_limit_counts_unicode_characters_like_the_python_protocol() {
    let text = "á".repeat(256 * 1024);
    assert!(text.len() > 256 * 1024);
    assert!(inbox::message_fits(&text));
    assert!(!inbox::message_fits(&(text + "x")));
}

#[test]
fn python_model_body_contract_keeps_defaults_error_order_and_missing_body() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/body.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let parsed = routes::parse_body(
            case["body"].as_str().unwrap().as_bytes(),
            case["provider"].as_str().unwrap(),
        );
        let (status, body) = match parsed {
            Ok(value) => (200, json!(value)),
            Err(error) => (error.status, json!({"detail":error.detail})),
        };
        assert_eq!(
            json!({"status":status,"body":body}),
            json!({"status":case["status"],"body":case["expected"]}),
            "{}",
            case["body"]
        );
    }
}

#[tokio::test]
async fn inbox_access_warns_once_per_cause_and_host_and_success_rearms_the_warning() {
    use std::io::Write;
    #[derive(Clone)]
    struct Log(Arc<Mutex<Vec<u8>>>);
    impl Write for Log {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let output = Arc::new(Mutex::new(vec![]));
    let writer = output.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || Log(writer.clone()))
        .finish();
    let _trace = tracing::subscriber::set_default(subscriber);
    let config = crate::config::Config::from_lookup(|key| match key {
        "HANGAR_SERVER_LISTEN" => Some("192.0.2.50:18765".into()),
        "HANGAR_SERVER_UPSTREAM" => Some("127.0.0.1:1".into()),
        "HANGAR_INTERNAL_SECRET" => Some("fixture-internal".into()),
        "CP_AUTH_TOKEN" => Some("fixture-owner".into()),
        _ => None,
    })
    .unwrap();
    let state = Arc::new(crate::routes::AppState::new(config));
    let request = |query: &str| {
        axum::extract::Request::builder()
            .uri(format!("/api/pi/inbox{query}"))
            .body(axum::body::Body::empty())
            .unwrap()
    };
    let local = "192.0.2.50:1".parse().unwrap();
    for _ in 0..3 {
        assert_eq!(
            inbox::websocket(&state, local, request("")).await.status(),
            403
        );
    }
    let first = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert_eq!(first.matches("token recusado").count(), 1, "{first}");
    // A credencial válida rearma o aviso antes da atualização HTTP, como na rota original.
    inbox::websocket(&state, local, request("?token=fixture-owner")).await;
    inbox::websocket(&state, local, request("")).await;
    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert_eq!(log.matches("token recusado").count(), 2, "{log}");
    for _ in 0..3 {
        assert_eq!(
            inbox::websocket(
                &state,
                "192.0.2.99:1".parse().unwrap(),
                request("?token=fixture-owner")
            )
            .await
            .status(),
            403
        );
    }
    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert_eq!(log.matches("origem recusada").count(), 1, "{log}");
}
