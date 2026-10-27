use super::*;
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::Method,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime},
};

fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/extra.json")).unwrap()
}
fn config() -> crate::config::Config {
    crate::config::Config::from_lookup(|key| match key {
        "HANGAR_SERVER_LISTEN" => Some("192.0.2.50:18765".into()),
        "HANGAR_SERVER_UPSTREAM" => Some("127.0.0.1:1".into()),
        "HANGAR_INTERNAL_SECRET" => Some("fixture-internal".into()),
        "CP_AUTH_TOKEN" => Some("fixture-owner".into()),
        _ => None,
    })
    .unwrap()
}
fn state() -> Arc<crate::routes::AppState> {
    Arc::new(crate::routes::AppState::new(config()))
}

#[tokio::test]
async fn http_content_type_and_validation_precede_session_discovery() {
    let st = state();
    for case in fixture()["bodies"].as_array().unwrap() {
        let mut builder = Request::builder().method(Method::POST).uri(format!(
            "/api/sessions/absent/{}/model",
            case["provider"].as_str().unwrap()
        ));
        if let Some(mime) = case["content_type"].as_str() {
            builder = builder.header("content-type", mime);
        }
        let request = builder
            .body(Body::from(case["body"].as_str().unwrap().to_owned()))
            .unwrap();
        let response = serve(&st, request).await;
        if case["status"] == 200 {
            continue;
        }
        let status = response.status().as_u16();
        let got: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap())
                .unwrap();
        assert_eq!(
            json!({"status":status,"body":got}),
            json!({"status":case["status"],"body":case["expected"]}),
            "{case}"
        );
    }
}

type Ws =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
async fn server(peer: &str) -> (String, Arc<inbox::Store>, tokio::task::JoinHandle<()>) {
    let state = state();
    let store = inbox::Store::new();
    let serving = store.clone();
    let peer = peer.parse::<std::net::SocketAddr>().unwrap();
    let app = axum::Router::new().route(
        "/api/pi/inbox",
        axum::routing::get(move |request: Request| {
            let state = state.clone();
            let store = serving.clone();
            async move {
                inbox::websocket_with(&state, peer, request, store, Duration::from_millis(500))
                    .await
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("ws://{address}/api/pi/inbox"), store, task)
}
async fn connect(url: &str) -> Ws {
    tokio_tungstenite::connect_async(format!("{url}?token=fixture-owner"))
        .await
        .unwrap()
        .0
}
async fn send(ws: &mut Ws, value: Value) {
    ws.send(tokio_tungstenite::tungstenite::Message::Text(
        value.to_string().into(),
    ))
    .await
    .unwrap();
}
async fn closed(ws: &mut Ws) -> (u16, usize) {
    let mut pings = 0;
    loop {
        match tokio::time::timeout(Duration::from_secs(3), ws.next())
            .await
            .unwrap()
        {
            Some(Ok(tokio_tungstenite::tungstenite::Message::Close(frame))) => {
                return (frame.unwrap().code.into(), pings);
            }
            Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                let v: Value = serde_json::from_str(&text).unwrap();
                pings += usize::from(v["ping"] == true);
            }
            other => panic!("fechamento ausente: {other:?}"),
        }
    }
}
#[tokio::test]
async fn websocket_closure_codes_and_heartbeat_match_python() {
    let (url, store, task) = server("127.0.0.1:1").await;
    for case in fixture()["sockets"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let mut ws = connect(&url).await;
        match name {
            "missing" => send(&mut ws, json!({"none":1})).await,
            "first-large" => ws
                .send(tokio_tungstenite::tungstenite::Message::Text(
                    "x".repeat(256 * 1024 + 10).into(),
                ))
                .await
                .unwrap(),
            "later-large" => {
                send(&mut ws, json!({"pane":"fixture-large"})).await;
                ws.send(tokio_tungstenite::tungstenite::Message::Text(
                    "x".repeat(256 * 1024 + 10).into(),
                ))
                .await
                .unwrap();
            }
            "heartbeat" => send(&mut ws, json!({"pane":"fixture-heartbeat"})).await,
            _ => panic!("caso desconhecido"),
        }
        let (code, pings) = closed(&mut ws).await;
        assert_eq!(
            json!({"close_code":code,"pings":pings}),
            json!({"close_code":case["close_code"],"pings":case["pings"]}),
            "{name}"
        );
        assert!(!store.has_line("fixture-large"));
        assert!(!store.has_line("fixture-heartbeat"));
    }
    task.abort();
}
#[tokio::test]
async fn websocket_auth_bind_reconnection_unknown_ack_and_unicode_editor_are_real() {
    let (url, store, task) = server("192.0.2.50:1").await;
    assert!(tokio_tungstenite::connect_async(&url).await.is_err());
    let mut old = connect(&url).await;
    send(&mut old, json!({"chave":"session","pane":"%1"})).await;
    tokio::time::sleep(Duration::from_millis(10)).await;
    let pending = {
        let store = store.clone();
        tokio::spawn(async move { store.question("session", "editor").await })
    };
    let _ = old.next().await.unwrap().unwrap();
    let mut new = connect(&url).await;
    send(&mut new, json!({"chave":"session","pane":"%1"})).await;
    assert_eq!(pending.await.unwrap(), None);
    old.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(store.has_line("session"));
    assert!(!store.has_line("%1"));
    send(&mut new, json!({"id":"unknown","ok":true})).await;
    let reading = {
        let store = store.clone();
        tokio::spawn(async move { store.question("session", "editor").await })
    };
    let text = new.next().await.unwrap().unwrap().into_text().unwrap();
    let question: Value = serde_json::from_str(&text).unwrap();
    let editor = "á".repeat(128 * 1024);
    send(&mut new, json!({"id":question["id"],"resposta":editor})).await;
    assert_eq!(reading.await.unwrap(), Some(editor));
    new.close(None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(10)).await;
    assert!(!store.has_line("session"));
    task.abort();
    let (url, _store, task) = server("192.0.2.99:1").await;
    assert!(
        tokio_tungstenite::connect_async(format!("{url}?token=fixture-owner"))
            .await
            .is_err()
    );
    task.abort();
}

#[test]
fn provider_current_cache_uses_mtime_and_forgets_missing_and_new_life() {
    let root = tempfile::tempdir().unwrap();
    let target = pure::sidecar_path("/a/old.jsonl", root.path());
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, r#"{"models":[],"current":{"provider":"p"}}"#).unwrap();
    let stamp = SystemTime::now() - Duration::from_secs(20);
    std::fs::File::options()
        .write(true)
        .open(&target)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    assert_eq!(
        pi_provider("/a/old.jsonl", root.path()).as_deref(),
        Some("p")
    );
    std::fs::write(&target, r#"{"models":[],"current":{"provider":"q"}}"#).unwrap();
    std::fs::File::options()
        .write(true)
        .open(&target)
        .unwrap()
        .set_modified(stamp)
        .unwrap();
    assert_eq!(
        pi_provider("/a/old.jsonl", root.path()).as_deref(),
        Some("p")
    );
    assert_eq!(pi_provider("/a/new.jsonl", root.path()), None);
    std::fs::remove_file(&target).unwrap();
    assert_eq!(pi_provider("/a/old.jsonl", root.path()), None);
}
#[test]
fn kimi_last_good_marker_survives_torn_json_but_not_deleted_marker() {
    let root = tempfile::tempdir().unwrap();
    let marker = root.path().join(".hangar-state/life.json");
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, r#"{"state":"working","ts":1}"#).unwrap();
    let dirs = vec![root.path().to_owned()];
    assert!(routes::kimi_working(&dirs, Some("/sessions/life.jsonl")));
    std::fs::write(&marker, "{").unwrap();
    assert!(routes::kimi_working(&dirs, Some("/sessions/life.jsonl")));
    std::fs::remove_file(marker).unwrap();
    assert!(!routes::kimi_working(&dirs, Some("/sessions/life.jsonl")));
}

#[tokio::test(start_paused = true)]
async fn inbox_serializes_writes_preserves_ids_and_cleans_failure_timeout_and_cancel() {
    let store = inbox::Store::new();
    assert_eq!(store.question("missing", "editor").await, None);
    assert_eq!(store.deliver("missing", "x", None).await, "sem-linha");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let line = store.register("session", tx);
    let a = {
        let s = store.clone();
        tokio::spawn(async move { s.deliver("session", "a", None).await })
    };
    let first = rx.recv().await.unwrap();
    let b = {
        let s = store.clone();
        tokio::spawn(async move { s.deliver("session", "b", Some("stable")).await })
    };
    tokio::task::yield_now().await;
    assert!(rx.try_recv().is_err());
    assert_eq!(first["id"].as_str().unwrap().len(), 32);
    store.respond("session", &json!({"id":first["id"],"ok":false}));
    assert_eq!(a.await.unwrap(), "deferred");
    let second = rx.recv().await.unwrap();
    assert_eq!(second["id"], "stable");
    assert_ne!(first["id"], second["id"]);
    store.respond("session", &json!({"id":"stable","ok":true}));
    assert_eq!(b.await.unwrap(), "sent");
    let q = {
        let s = store.clone();
        tokio::spawn(async move { s.question("session", "editor").await })
    };
    let question = rx.recv().await.unwrap();
    store.respond("session", &json!({"id":question["id"],"resposta":null}));
    assert_eq!(q.await.unwrap(), None);
    let q = {
        let s = store.clone();
        tokio::spawn(async move { s.question("session", "editor").await })
    };
    let _ = rx.recv().await.unwrap();
    tokio::time::advance(inbox::QUESTION_TIMEOUT).await;
    assert_eq!(q.await.unwrap(), None);
    let q = {
        let s = store.clone();
        tokio::spawn(async move { s.question("session", "editor").await })
    };
    let _ = rx.recv().await.unwrap();
    q.abort();
    let _ = q.await;
    store.remove("session", &line);
    drop(rx);
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    store.register("broken", tx);
    drop(rx);
    assert_eq!(store.deliver("broken", "x", None).await, "deferred");
    assert!(!store.has_line("broken"));
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    store.register("broken-question", tx);
    drop(rx);
    assert_eq!(store.question("broken-question", "editor").await, None);
    assert!(!store.has_line("broken-question"));
}
#[test]
fn inbox_endpoint_wildcard_partial_io_and_empty_accounts_keep_the_available_accounts() {
    let root = tempfile::tempdir().unwrap();
    let bad = root.path().join("not-directory");
    std::fs::write(&bad, "fixture").unwrap();
    let good = root.path().join("good");
    let result = inbox::write_endpoint(
        &[bad, good.clone()],
        "0.0.0.0:8765".parse().unwrap(),
        "fixture",
        1.,
    );
    assert_eq!(result, vec![good.join(".hangar-conn.json")]);
    let value: Value = serde_json::from_slice(&std::fs::read(&result[0]).unwrap()).unwrap();
    assert_eq!(value["url"], "ws://127.0.0.1:8765/api/pi/inbox");
    assert!(inbox::write_endpoint(&[], "0.0.0.0:8765".parse().unwrap(), "fixture", 1.).is_empty());
}

#[cfg(unix)]
struct Processes {
    root: PathBuf,
    provider: String,
}
#[cfg(unix)]
impl crate::list::procs::ProcessView for Processes {
    fn children(&self, _: Duration) -> std::io::Result<Arc<crate::list::procs::ChildrenMap>> {
        Ok(Arc::default())
    }
    fn argv(&self, _: i64) -> Vec<String> {
        if self.provider == "claude" {
            vec![
                "claude".into(),
                "--session-id".into(),
                "00000000-0000-4000-8000-000000000001".into(),
            ]
        } else {
            vec![self.provider.clone()]
        }
    }
    fn cwd(&self, _: i64) -> Option<PathBuf> {
        Some(self.root.clone())
    }
    fn env_var(&self, _: i64, key: &str) -> std::io::Result<Option<std::ffi::OsString>> {
        Ok(match key {
            "CLAUDE_CONFIG_DIR" => Some(self.root.join(".claude").into_os_string()),
            "CP_PROVIDER" => Some(self.provider.clone().into()),
            _ => None,
        })
    }
    fn start_time(&self, _: i64) -> Option<f64> {
        Some(1.)
    }
    fn fds(&self, _: i64) -> Vec<PathBuf> {
        vec![]
    }
}
#[cfg(unix)]
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}
#[cfg(unix)]
fn executable(path: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    write(path, text);
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
}
#[cfg(unix)]
fn route_state(
    root: &Path,
    provider: &str,
    catalog: Option<&Value>,
) -> Arc<crate::routes::AppState> {
    use crate::list::{
        bridge::{ListBridge, ListEnv},
        discover_other::Dirs,
        facts::FactsClient,
        mux::Mux,
    };
    let shim = root.join("mux");
    write(
        &root.join("pane"),
        "────────────────────\n\n────────────────────\n",
    );
    write(
        &root.join("picker"),
        "Select a model\nThinking (←→ to switch)\nLow [ High ]\n",
    );
    write(
        &root.join("result"),
        "Switched to One with thinking low for this session only.\n",
    );
    let folder = root.display();
    executable(
        &shim,
        &format!(
            r#"#!/bin/sh
case "$1" in
list-panes) printf 'fixture\t1\t123\t{folder}\t%%1\t0\t{provider}\t10\t0\t0\t0\n';;
capture-pane) cat '{folder}/pane';;
send-keys)
 printf '%s\n' "$@" >> '{folder}/keys'
 if [ -f '{folder}/after.json' ]; then cat '{folder}/after.json' > '{folder}/.claude/.hangar-pi/models/life.json'; fi
 case "$*" in
 *'/model'*) cat '{folder}/picker' > '{folder}/pane';;
 *'M-s'*) cat '{folder}/result' > '{folder}/pane';;
 esac
 ;;
esac
"#
        ),
    );
    let dirs = Dirs {
        home: root.to_owned(),
        claude: root.join(".claude"),
        codex_home: root.join(".codex"),
        pi_sessions: root.join(".pi/agent/sessions"),
        omp_config: root.join(".omp"),
        omp_agent: root.join(".omp/agent"),
        kimi_home: root.join(".kimi-code"),
    };
    write(
        &dirs.claude.join(".hangar-pi/1.json"),
        &json!({"file":root.join("life.jsonl"),"ts":10}).to_string(),
    );
    write(&root.join("life.jsonl"), "{}");
    write(
        &dirs
            .claude
            .join("projects")
            .join(hangar_workspace::worktrees::sanitize_cwd(
                &root.to_string_lossy(),
            ))
            .join("00000000-0000-4000-8000-000000000001.jsonl"),
        "{}",
    );
    write(
        &dirs.claude.join(".hangar-kimi/1.json"),
        &json!({"session_id":"life","cwd":root,"ts":10}).to_string(),
    );
    write(
        &dirs
            .kimi_home
            .join("sessions")
            .join(crate::list::discover_other::kimi_workdir_key(
                &root.to_string_lossy(),
            ))
            .join("life/agents/main/wire.jsonl"),
        "{}",
    );

    if let Some(catalog) = catalog {
        if provider == "pi" || provider == "omp" {
            write(
                &dirs.claude.join(".hangar-pi/models/life.json"),
                &catalog.to_string(),
            );
        } else if provider == "kimi" {
            let mut text = format!("default_model = {}\n", catalog["default"]);
            for m in catalog["models"].as_array().unwrap() {
                text += &format!(
                    "[models.{}]\nmodel = {}\ndisplay_name = {}\nmax_context_size = {}\nsupport_efforts = {}\ndefault_effort = {}\n",
                    m["alias"],
                    m["id"],
                    m["name"],
                    m["context_length"],
                    m["efforts"],
                    m["default_effort"]
                );
            }
            write(&dirs.kimi_home.join("config.toml"), &text);
        }
    }
    let cfg = config();
    let facts = FactsClient::new(cfg.upstream, cfg.internal_secret.clone());
    let mut state = crate::routes::AppState::new(cfg);
    state.origins_home = root.to_owned();
    state.list = Arc::new(ListBridge::new(
        ListEnv {
            mux: Mux::with_program(shim.as_os_str(), Duration::from_secs(2)),
            capture_program: shim.into_os_string(),
            procs: Arc::new(Processes {
                root: root.to_owned(),
                provider: provider.into(),
            }),
            dirs: Some(dirs),
        },
        facts,
    ));
    Arc::new(state)
}
#[cfg(unix)]
#[tokio::test]
async fn public_get_routes_match_all_python_provider_and_missing_source_cases() {
    for case in fixture()["routes"].as_array().unwrap() {
        let root = tempfile::tempdir().unwrap();
        let provider = case["provider"].as_str().unwrap();
        let state = route_state(
            root.path(),
            provider,
            (case["source"] == "present").then_some(&case["catalog"]),
        );
        let request = Request::builder()
            .uri(format!(
                "/api/sessions/fixture/{}/models",
                case["endpoint"].as_str().unwrap()
            ))
            .body(Body::empty())
            .unwrap();
        let response = serve(&state, request).await;
        let status = response.status().as_u16();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap())
                .unwrap();
        assert_eq!(
            json!({"status":status,"body":body}),
            json!({"status":case["status"],"body":case["expected"]}),
            "{case}"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn catalog_cli_argv_utf8_exit_errors_and_non_executable_path_match_python() {
    let root = tempfile::tempdir().unwrap();
    let bad = root.path().join("first/pi");
    write(&bad, "not executable");
    let good = root.path().join("second/pi");
    let log = root.path().join("argv");
    executable(
        &good,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf 'provider model context max-out thinking images\\np m 1M 8K yes yes\\n'\n",
            log.display()
        ),
    );
    let env = vec![(
        "PATH".into(),
        std::env::join_paths([bad.parent().unwrap(), good.parent().unwrap()])
            .unwrap()
            .to_string_lossy()
            .into_owned(),
    )];
    assert_eq!(catalog::program_from("pi", &env), Some(good.clone()));
    let models = catalog::read_from("pi", env.clone()).await.unwrap();
    assert_eq!(models[0]["id"], "m");
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "--list-models\n");
    let omp = root.path().join("second/omp");
    executable(
        &omp,
        &format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' '{{\"models\":[{{\"provider\":\"p\",\"id\":\"m\",\"contextWindow\":128000,\"maxTokens\":8192,\"thinking\":true,\"input\":[\"image\"]}}]}}'\n",
            log.display()
        ),
    );
    assert_eq!(
        catalog::read_from("omp", env.clone()).await.unwrap()[0]["context"],
        "128K"
    );
    assert_eq!(std::fs::read_to_string(&log).unwrap(), "models\n--json\n");
    executable(&good, "#!/bin/sh\nprintf 'falha explícita' >&2\nexit 3\n");
    let failure = catalog::read_from("pi", env.clone()).await.unwrap_err();
    assert_eq!(failure.status, 502);
    assert_eq!(failure.detail["code"], "erro_pi_list_models");
    assert_eq!(failure.detail["params"]["erro"], "falha explícita");
    executable(&good,"#!/bin/sh\nprintf 'provider model context max-out thinking images cost\np m 1M 8K yes yes 1\n'\n");
    assert!(catalog::read_from("pi",env.clone()).await.unwrap_err().detail["params"]["erro"].as_str().unwrap().contains("nao devolveu modelo nenhum"));
    executable(&good,"#!/bin/sh\nprintf 'provider model context max-out thinking images\np m\\377 1M 8K yes yes\n'\n");
    assert!(catalog::read_from("pi",env.clone()).await.unwrap_err().detail["params"]["erro"].as_str().unwrap().contains("ilegivel"));
    executable(&good,"#!/bin/sh\nprintf 'provider model context max-out thinking images\np m 128\\377K 8K yes yes\n'\n");
    assert_eq!(catalog::read_from("pi",env.clone()).await.unwrap()[0]["context"],"128�K");
    for provider in ["pi", "omp"] {
        let failure = catalog::read_from(
            provider,
            vec![(
                "PATH".into(),
                root.path().join("missing").to_string_lossy().into_owned(),
            )],
        )
        .await
        .unwrap_err();
        assert_eq!(failure.detail["code"], format!("erro_{provider}_ausente"));
    }
}
#[test]
fn validation_contract_keeps_catalog_identity_and_model_effort_support() {
    for case in fixture()["validation"].as_array().unwrap() {
        let input = &case["input"];
        let got = match case["op"].as_str().unwrap() {
            "check_pi" => pure::check_pi(
                &input[0],
                input[1].as_str().unwrap(),
                input[2].as_str().unwrap(),
            )
            .map(|_| Value::Null),
            "check_kimi" => pure::check_kimi(&input[0], input[1].as_str().unwrap()),
            "check_effort" => {
                pure::check_effort(&input[0], input[1].as_str().unwrap()).map(Value::String)
            }
            _ => unreachable!(),
        };
        let got = match got {
            Ok(v) => json!({"ok":v}),
            Err(e) => json!({"error":{"status":e.status,"detail":e.detail}}),
        };
        assert_eq!(got, case["expected"], "{case}");
    }
}
#[cfg(unix)]
#[tokio::test]
async fn public_post_validation_matches_python_and_never_types_unknown_choices() {
    for case in fixture()["posts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["status"] != 200)
    {
        let root = tempfile::tempdir().unwrap();
        let provider = case["provider"].as_str().unwrap();
        let state = route_state(root.path(), provider, Some(&case["catalog"]));
        let request = Request::builder()
            .method(Method::POST)
            .header("content-type", "application/json")
            .uri(format!("/api/sessions/fixture/{provider}/model"))
            .body(Body::from(case["body"].to_string()))
            .unwrap();
        let response = serve(&state, request).await;
        let status = response.status().as_u16();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap())
                .unwrap();
        assert_eq!(
            json!({"status":status,"body":body}),
            json!({"status":case["status"],"body":case["expected"]}),
            "{case}"
        );
        assert!(
            !root.path().join("keys").exists(),
            "a validação não pode digitar escolhas inválidas"
        );
    }
}
#[tokio::test(start_paused = true)]
async fn cold_catalog_error_is_not_cached_and_pi_readback_polls_until_actual_change() {
    struct Fails(std::sync::atomic::AtomicUsize);
    impl catalog::Source for Fails {
        fn read<'a>(&'a self, _: &'a str) -> catalog::CatalogFuture<'a> {
            Box::pin(async move {
                self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Err(ModelError::text(502, "fixture-failure"))
            })
        }
    }
    let source = Arc::new(Fails(0.into()));
    let cache = catalog::Catalogs::new(source.clone(), Duration::from_secs(600));
    assert_eq!(cache.read("pi").await.unwrap_err().status, 502);
    assert_eq!(cache.read("pi").await.unwrap_err().status, 502);
    assert_eq!(source.0.load(std::sync::atomic::Ordering::SeqCst), 2);
    struct Changing(std::sync::Mutex<std::collections::VecDeque<Value>>);
    impl driver::Io for Changing {
        fn capture(&self) -> driver::IoFuture<'_, Result<String, ModelError>> {
            Box::pin(async { Ok(String::new()) })
        }
        fn key<'a>(&'a self, _: &'a str, _: bool) -> driver::IoFuture<'a, Result<(), ModelError>> {
            Box::pin(async { Ok(()) })
        }
        fn editor(&self) -> driver::IoFuture<'_, Option<String>> {
            Box::pin(async { None })
        }
        fn catalog(&self) -> driver::IoFuture<'_, Option<Value>> {
            Box::pin(async { self.0.lock().unwrap().pop_front() })
        }
    }
    let before = json!({"models":[],"current":{"provider":"p","id":"one"},"thinking":"low","ts":1});
    let after = json!({"models":[],"current":{"provider":"p","id":"two"},"thinking":"high","ts":2});
    let io = Changing(std::sync::Mutex::new(
        [before.clone(), before.clone(), after.clone()].into(),
    ));
    let result = driver::pi_read_back(
        &io,
        &before,
        Some("p"),
        Some("two"),
        None,
        driver::Timing::default(),
    )
    .await
    .unwrap();
    assert_eq!(result["current"], after["current"]);
    assert!(io.0.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn public_post_success_routes_keep_python_response_and_keyboard_effects() {
    for case in fixture()["posts"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["status"] == 200)
    {
        let root = tempfile::tempdir().unwrap();
        let provider = case["provider"].as_str().unwrap();
        let state = route_state(root.path(), provider, Some(&case["catalog"]));
        write(&root.path().join("after.json"), &case["after"].to_string());
        let request = Request::builder()
            .method(Method::POST)
            .header("content-type", "application/json")
            .uri(format!("/api/sessions/fixture/{provider}/model"))
            .body(Body::from(case["body"].to_string()))
            .unwrap();
        let response = serve(&state, request).await;
        let status = response.status().as_u16();
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap())
                .unwrap();
        assert_eq!(
            json!({"status":status,"body":body}),
            json!({"status":case["status"],"body":case["expected"]}),
            "{case}"
        );
        let keys = std::fs::read_to_string(root.path().join("keys")).unwrap();
        assert_eq!(
            keys.matches('\r').count(),
            if provider == "pi" { 2 } else { 1 },
            "{keys}"
        );
        if provider == "pi" {
            assert!(keys.find("/cp-model p one").unwrap() < keys.find("/cp-think high").unwrap());
        } else {
            assert!(keys.contains("p/one"));
            assert!(keys.contains("M-s"));
        }
    }
}
#[tokio::test]
async fn websocket_path_does_not_claim_http_methods_or_authenticate_plain_http() {
    let state = state();
    for method in [Method::GET, Method::POST, Method::OPTIONS] {
        assert_eq!(allowed_method("/api/pi/inbox", None), None);
        let request = Request::builder()
            .method(method)
            .uri("/api/pi/inbox")
            .body(Body::empty())
            .unwrap();
        let response = serve(&state, request).await;
        assert_eq!(response.status(), 404);
        let body: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 1 << 20).await.unwrap())
                .unwrap();
        assert_eq!(body, json!({"detail":"Not Found"}));
    }
    fn assert_send<T: Send>(_: T) {}
    let request = Request::builder()
        .uri("/api/model-options?provider=kimi")
        .body(Body::empty())
        .unwrap();
    assert_send(serve(&state, request));
}

#[cfg(unix)]
#[test]
fn inbox_atomic_writer_creates_private_temporary_before_any_bytes() {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join(".hangar-conn.json");
    let observed = std::sync::atomic::AtomicBool::new(false);
    crate::runtime::queue::atomic_write_inspected(&target, b"fixture-token", |file| {
        assert_eq!(file.metadata()?.permissions().mode() & 0o777, 0o600);
        assert_eq!(file.metadata()?.len(), 0);
        assert!(!target.exists());
        observed.store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    })
    .unwrap();
    assert!(observed.load(std::sync::atomic::Ordering::SeqCst));
    assert_eq!(std::fs::read(target).unwrap(), b"fixture-token");
}

#[test]
fn legacy_pytest_inputs_contract_replays_every_captured_python_assertion_input() {
    let fixture: Value = serde_json::from_str(include_str!("fixtures/legacy-calls.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let args = &case["input"];
        let text = |index: usize| args[index].as_str().unwrap_or("");
        let result: Result<Value, ModelError> = match case["op"].as_str().unwrap() {
            "parse" => Ok(json!(pure::parse_pi(text(0)))),
            "parse_omp" => Ok(json!(pure::parse_omp(text(0)))),
            "pi_model_command" => pure::model_command(text(0), text(1)).map(Value::String),
            "pi_think_command" => pure::think_command(text(0)).map(Value::String),
            "pi_check_known" => pure::check_pi(&args[0], text(1), text(2)).map(|_| Value::Null),
            "pi_confirms" => Ok(json!(pure::confirms(
                &args[0],
                args[1].as_str(),
                args[2].as_str(),
                args[3].as_str()
            ))),
            "pi_read_catalog" => {
                Ok(pure::parse_pi_catalog(text(0).as_bytes()).unwrap_or(Value::Null))
            }
            "kimi_read_catalog" => Ok(pure::parse_kimi(text(0)).unwrap_or(Value::Null)),
            "kimi_clean_alias" => pure::clean(text(0), "alias").map(Value::String),
            "kimi_check_known" => pure::check_kimi(&args[0], text(1)),
            "kimi_check_effort" => pure::check_effort(&args[0], text(1)).map(Value::String),
            "kimi_parse_switched" => Ok(pure::parse_switched(text(0)).unwrap_or(Value::Null)),
            "kimi_parse_thinking_set" => {
                Ok(pure::parse_thinking_set(text(0)).unwrap_or(Value::Null))
            }
            "kimi_parse_thinking_row" => {
                Ok(pure::parse_thinking_row(text(0)).unwrap_or(Value::Null))
            }
            "kimi_confirms" => Ok(json!(pure::new_line_matches(
                (!args[0].is_null()).then_some(&args[0]),
                (!args[1].is_null()).then_some(&args[1]),
                "name",
                text(2)
            ))),
            "kimi_confirms_effort" => Ok(json!(pure::new_line_matches(
                (!args[0].is_null()).then_some(&args[0]),
                (!args[1].is_null()).then_some(&args[1]),
                "level",
                text(2)
            ))),
            op => panic!("operação não coberta: {op}"),
        };
        let got = match result {
            Ok(value) => json!({"ok":value}),
            Err(error) => json!({"error":{"status":error.status,"detail":error.detail}}),
        };
        assert_eq!(
            got, case["expected"],
            "{}: {}",
            case["pytest"], case["input"]
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn public_model_routes_refuse_overlay_and_kimi_working_without_typing(){
 let overlay=include_str!(concat!(env!("CARGO_MANIFEST_DIR"),"/../../backend/tests/fixtures/pane_awaiting_input.txt"));
 for provider in ["pi","kimi"] {let root=tempfile::tempdir().unwrap();let catalog=fixture()["posts"].as_array().unwrap().iter().find(|c|c["provider"]==provider).unwrap()["catalog"].clone();let state=route_state(root.path(),provider,Some(&catalog));write(&root.path().join("pane"),overlay);
 let request=Request::builder().method(Method::POST).header("content-type","application/json").uri(format!("/api/sessions/fixture/{provider}/model")).body(Body::from(json!({"effort":"high"}).to_string())).unwrap();let response=serve(&state,request).await;assert_eq!(response.status(),409);let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),1<<20).await.unwrap()).unwrap();assert_eq!(body,json!({"detail":"pane com overlay aberto ou sessao morta — nada foi digitado"}));assert!(!root.path().join("keys").exists());
 }
 let root=tempfile::tempdir().unwrap();let catalog=fixture()["posts"].as_array().unwrap().iter().find(|c|c["provider"]=="kimi").unwrap()["catalog"].clone();let state=route_state(root.path(),"kimi",Some(&catalog));write(&root.path().join(".claude/.hangar-state/life.json"),r#"{"state":"working","ts":1}"#);
 let request=Request::builder().method(Method::POST).header("content-type","application/json").uri("/api/sessions/fixture/kimi/model").body(Body::from(json!({"model":"p/one"}).to_string())).unwrap();let response=serve(&state,request).await;assert_eq!(response.status(),409);let body:Value=serde_json::from_slice(&to_bytes(response.into_body(),1<<20).await.unwrap()).unwrap();assert_eq!(body["detail"]["code"],"erro_sessao_trabalhando");assert!(!root.path().join("keys").exists());
}
