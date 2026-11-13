use std::sync::Arc;

use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use serde_json::{Value, json};

struct Fixture {
    _home: tempfile::TempDir,
    base: String,
    token: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Fixture {
    async fn start() -> Self {
        Self::surface(false).await
    }
    async fn surface(connect: bool) -> Self {
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address,
            upstream: crate::refused_address(),
            internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    home.path().to_string_lossy().into_owned(),
                ),
            ]
            .into(),
        ));
        let token = if connect {
            let store =
                hangar_server::connect::devices::Store::new(home.path().join("devices"), None);
            let token = store.enroll("proof-device").unwrap().1;
            state.connect_devices = Arc::new(store);
            token
        } else {
            "fixture-owner".into()
        };
        let router = hangar_server::routes::router(Arc::new(state));
        let task = if connect {
            tokio::spawn(async move {
                let router = router.layer(axum::middleware::map_request(
                    hangar_server::connect::gate::rewrite_peer,
                ));
                axum::serve(listener,router.into_make_service_with_connect_info::<hangar_server::connect::gate::ConnectConn>()).await.unwrap();
            })
        } else {
            tokio::spawn(async move {
                axum::serve(
                    listener,
                    router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
                )
                .await
                .unwrap();
            })
        };
        Self {
            _home: home,
            base: format!("http://{address}/api/scheduled-tasks"),
            token,
            task,
        }
    }
    async fn request(
        &self,
        method: reqwest::Method,
        suffix: &str,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut request = reqwest::Client::new()
            .request(method, format!("{}{suffix}", self.base))
            .bearer_auth(&self.token);
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body.to_string());
        }
        request.send().await.unwrap()
    }
    fn input(&self) -> Value {
        json!({"name":"Revisão","prompt":"Revise as mudanças.","enabled":true,"schedule":{"kind":"interval","minutes":10},"target":{"kind":"new","launch":{"cwd":self._home.path()},"worktree":false}})
    }
}
#[tokio::test]
async fn scheduled_routes_owner_contract_revision_and_pause() {
    let fixture = Fixture::start().await;
    let response = fixture.request(reqwest::Method::GET, "", None).await;
    assert_eq!(response.status(), 200);
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(body["tasks"], json!([]));
    assert!(
        body["timezone"]
            .as_str()
            .unwrap()
            .parse::<chrono_tz::Tz>()
            .is_ok()
    );
    let response = fixture
        .request(reqwest::Method::POST, "", Some(fixture.input()))
        .await;
    assert_eq!(response.status(), 201);
    let task: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(task["last_status"], "never");
    assert!(task.get("attempt").is_none());
    let id = task["id"].as_str().unwrap();
    let revision = task["revision"].as_u64().unwrap();
    let response = fixture
        .request(
            reqwest::Method::PATCH,
            &format!("/{id}/enabled"),
            Some(json!({"enabled":false,"revision":revision})),
        )
        .await;
    assert_eq!(response.status(), 200);
    let paused: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(paused["next_run_at"], Value::Null);
    assert_eq!(
        fixture
            .request(
                reqwest::Method::DELETE,
                &format!("/{id}?revision={revision}"),
                None
            )
            .await
            .status(),
        409
    );
    assert_eq!(
        fixture
            .request(
                reqwest::Method::DELETE,
                &format!("/{id}?revision={}", paused["revision"]),
                None
            )
            .await
            .status(),
        200
    );
}
#[tokio::test]
async fn scheduled_routes_refuse_guests_internal_templates_and_missing_lives() {
    let fixture = Fixture::start().await;
    assert!(matches!(
        reqwest::Client::new()
            .get(&fixture.base)
            .send()
            .await
            .unwrap()
            .status()
            .as_u16(),
        401 | 403
    ));
    let mut invalid = fixture.input();
    invalid["target"]["launch"]["creator"] = json!("other");
    assert_eq!(
        fixture
            .request(reqwest::Method::POST, "", Some(invalid))
            .await
            .status(),
        400
    );
    let mut missing = fixture.input();
    missing["target"] = json!({"kind":"session","name":"missing"});
    assert_eq!(
        fixture
            .request(reqwest::Method::POST, "", Some(missing))
            .await
            .status(),
        404
    );
    assert_eq!(
        fixture
            .request(reqwest::Method::POST, "", Some(json!({"name":"t"})))
            .await
            .status(),
        400
    );
}

#[tokio::test]
async fn scheduled_routes_bound_writer_refuses_another_life_before_queueing() {
    let directory = tempfile::tempdir().unwrap();
    let registry = crate::session_write_input::registry().await;
    crate::session_write_input::open_headless(&registry, directory.path(), "same-name", "written")
        .await;
    let state = AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    assert!(state.state.runtime.set(registry).is_ok());
    let (status, body) = hangar_server::session_write::input::write_bound_input(
        &state,
        "same-name",
        "previous-life",
        None,
        "Revise.",
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["detail"]["code"], "scheduled_session_changed");
    let (status, _) = hangar_server::session_write::input::write_bound_input(
        &state,
        "same-name",
        "k-same-name",
        None,
        "Revise.",
    )
    .await;
    assert_eq!(status, 200);
}

/// A descoberta diz qual conversa a sessão mostra agora (a do `/clear`, na mesma chave).
struct CurrentConversation(&'static str);
impl hangar_server::locate::Locate for CurrentConversation {
    fn info<'a>(
        &'a self,
        _name: &'a str,
    ) -> hangar_server::locate::BoxFuture<
        'a,
        Result<Option<hangar_server::transcript::InternalInfo>, hangar_server::locate::InfoFailed>,
    > {
        Box::pin(async move {
            Ok(Some(
                serde_json::from_value(json!({"provider":"claude-headless","session_key":self.0}))
                    .unwrap(),
            ))
        })
    }
}

#[tokio::test]
async fn scheduled_routes_bound_writer_refuses_the_conversation_after_clear_in_the_same_life() {
    let directory = tempfile::tempdir().unwrap();
    let registry = crate::session_write_input::registry().await;
    crate::session_write_input::open_headless(&registry, directory.path(), "same-name", "written")
        .await;
    let mut state = AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.side.locate = Arc::new(CurrentConversation("after-clear"));
    assert!(state.state.runtime.set(registry).is_ok());
    let (status, body) = hangar_server::session_write::input::write_bound_input(
        &state,
        "same-name",
        "k-same-name",
        Some("before-clear"),
        "Revise.",
    )
    .await;
    assert_eq!(status, 409);
    assert_eq!(body["detail"]["code"], "scheduled_session_changed");
    let (status, _) = hangar_server::session_write::input::write_bound_input(
        &state,
        "same-name",
        "k-same-name",
        Some("after-clear"),
        "Revise.",
    )
    .await;
    assert_eq!(status, 200);
}

#[tokio::test]
async fn scheduled_routes_create_with_the_same_request_id_returns_the_first_task() {
    let fixture = Fixture::start().await;
    let mut body = fixture.input();
    body["request_id"] = json!("desktop-req_01");
    let first = fixture
        .request(reqwest::Method::POST, "", Some(body.clone()))
        .await;
    assert_eq!(first.status(), 201);
    let first: Value = serde_json::from_slice(&first.bytes().await.unwrap()).unwrap();
    let again = fixture.request(reqwest::Method::POST, "", Some(body)).await;
    assert_eq!(again.status(), 201);
    let again: Value = serde_json::from_slice(&again.bytes().await.unwrap()).unwrap();
    assert_eq!(again["id"], first["id"]);
    let list = fixture.request(reqwest::Method::GET, "", None).await;
    let list: Value = serde_json::from_slice(&list.bytes().await.unwrap()).unwrap();
    assert_eq!(list["tasks"].as_array().unwrap().len(), 1);
    let mut invalid = fixture.input();
    invalid["request_id"] = json!("não vale");
    assert_eq!(
        fixture
            .request(reqwest::Method::POST, "", Some(invalid))
            .await
            .status(),
        400
    );
}

#[tokio::test]
async fn scheduled_routes_verified_guest_never_lists_even_with_forwarded_owner_bearer() {
    use axum::extract::{ConnectInfo, State};
    use hangar_server::share::caller::{Caller, GuestGrant};
    let state = AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    let mut request = axum::extract::Request::builder()
        .uri("/api/scheduled-tasks")
        .header("authorization", "Bearer fixture-owner")
        .body(axum::body::Body::empty())
        .unwrap();
    request
        .extensions_mut()
        .insert(Caller::Guest(Arc::new(GuestGrant {
            token: "fixture-guest".into(),
            token_hash: "fixture-hash".into(),
            sessions: Default::default(),
            pair_roots: vec![],
            invite: true,
        })));
    let response = hangar_server::scheduled_tasks::collection(
        State(Arc::new(state)),
        ConnectInfo("127.0.0.1:1234".parse().unwrap()),
        request,
    )
    .await;
    assert_eq!(response.status(), 403);
}

#[tokio::test]
async fn scheduled_routes_connect_approved_owner_device_gets_reads_and_mutations() {
    let fixture = Fixture::surface(true).await;
    assert_eq!(
        reqwest::Client::new()
            .get(&fixture.base)
            .bearer_auth("fixture-owner")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        fixture
            .request(reqwest::Method::GET, "", None)
            .await
            .status(),
        200
    );
    let created = fixture
        .request(reqwest::Method::POST, "", Some(fixture.input()))
        .await;
    assert_eq!(created.status(), 201);
    let task: Value = serde_json::from_slice(&created.bytes().await.unwrap()).unwrap();
    let id = task["id"].as_str().unwrap();
    let mut update = fixture.input();
    update["name"] = json!("Editada");
    update["revision"] = task["revision"].clone();
    let updated = fixture
        .request(reqwest::Method::PUT, &format!("/{id}"), Some(update))
        .await;
    assert_eq!(updated.status(), 200);
    let task: Value = serde_json::from_slice(&updated.bytes().await.unwrap()).unwrap();
    let paused = fixture
        .request(
            reqwest::Method::PATCH,
            &format!("/{id}/enabled"),
            Some(json!({"enabled":false,"revision":task["revision"]})),
        )
        .await;
    assert_eq!(paused.status(), 200);
    let task: Value = serde_json::from_slice(&paused.bytes().await.unwrap()).unwrap();
    assert_eq!(
        fixture
            .request(
                reqwest::Method::DELETE,
                &format!("/{id}?revision={}", task["revision"]),
                None
            )
            .await
            .status(),
        200
    );
}

#[tokio::test]
async fn scheduled_regression_invalid_owner_credentials_are_counted_and_blocked() {
    let fixture = Fixture::start().await;
    let client = reqwest::Client::new();
    let first = client
        .get(&fixture.base)
        .bearer_auth("wrong-owner")
        .header("x-forwarded-for", "198.51.100.51")
        .send()
        .await
        .unwrap();
    assert_eq!(first.status(), 401);
    let mut blocked = false;
    for _ in 0..12 {
        let status = client
            .get(&fixture.base)
            .bearer_auth("wrong-owner")
            .header("x-forwarded-for", "198.51.100.51")
            .send()
            .await
            .unwrap()
            .status();
        if status == 429 {
            blocked = true;
            break;
        }
        assert_eq!(status, 401);
    }
    assert!(blocked, "a rota deve usar a admissão que conta falhas");
}

#[tokio::test]
async fn scheduled_regression_bound_sleep_queue_cannot_bypass_a_closed_ingress() {
    use hangar_server::runtime::gateway::RuntimeRegistry;
    use std::time::Duration;
    let home = tempfile::tempdir().unwrap();
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                home.path().to_string_lossy().into_owned(),
            ),
        ]
        .into(),
    );
    let directory = home.path().join(".hangar/codex-sessions");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(directory.join("sleeping.json"),json!({"name":"sleeping","key":"old-life","headless":true,"asleep":{"wake_surface":"headless","since":1.0}}).to_string()).unwrap();
    let registry = Arc::new(
        RuntimeRegistry::new(
            crate::refused_address(),
            "fixture-secret".into(),
            "fixture-instance".into(),
        )
        .with_codex_environment(env),
    );
    registry.bind_lifecycle();
    registry
        .ingress()
        .close("sleeping", Duration::ZERO)
        .await
        .unwrap();
    registry.wake("sleeping", "old-life");
    let mut state = AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.write_gate_wait = Duration::ZERO;
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let (status, _) = hangar_server::session_write::input::write_bound_input(
        &state,
        "sleeping",
        "old-life",
        None,
        "Não escreva depois do fechamento.",
    )
    .await;
    assert_eq!(status, 409);
    assert!(
        !home
            .path()
            .join(".claude/.hangar-queue/runtime/old-life.json")
            .exists()
    );
    registry.ingress().open("sleeping");
}

#[tokio::test]
async fn scheduled_regression_due_task_waits_for_the_actual_restore_barrier() {
    use chrono::{DateTime, Utc};
    use hangar_scheduled_tasks::{
        Attempt, Clock, Engine, Error, Host, HostFuture, Outcome, Schedule, Status, Store, Target,
        TaskInput,
    };
    use std::future::Future;
    use std::sync::Mutex;
    use std::task::Poll;
    struct TestClock(Mutex<DateTime<Utc>>);
    impl Clock for TestClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }
    struct TestHost(tokio::sync::mpsc::UnboundedSender<String>);
    impl Host for TestHost {
        fn prepare_target(&self, target: Target) -> HostFuture<'_, Result<Target, Error>> {
            Box::pin(async move { Ok(target) })
        }
        fn dispatch(&self, attempt: Attempt) -> HostFuture<'_, Outcome> {
            Box::pin(async move {
                self.0.send(attempt.task.input.prompt).unwrap();
                Outcome {
                    status: Status::Sent,
                    error: None,
                    session: Some(attempt.session_name),
                }
            })
        }
    }
    let home = tempfile::tempdir().unwrap();
    let now = DateTime::parse_from_rfc3339("2026-11-12T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc);
    let clock = Arc::new(TestClock(Mutex::new(now)));
    let (started, mut received) = tokio::sync::mpsc::unbounded_channel();
    let engine = Arc::new(Engine::new(
        Arc::new(Store::open(&home.path().join("tasks.sqlite")).unwrap()),
        chrono_tz::UTC,
        Arc::new(TestHost(started)),
        clock.clone(),
    ));
    let task = engine
        .create(TaskInput {
            name: "Revisão".into(),
            prompt: "Depois do restauro.".into(),
            enabled: true,
            schedule: Schedule::Interval { minutes: 5 },
            target: Target::New {
                launch: json!({"cwd":home.path()}),
                worktree: false,
                base: None,
            },
        })
        .await
        .unwrap();
    *clock.0.lock().unwrap() = now + chrono::Duration::hours(2);
    let mut state = AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                home.path().to_string_lossy().into_owned(),
            ),
            ("CP_AUTOMACOES".into(), "1".into()),
        ]
        .into(),
    ));
    state.startup = hangar_server::startup::Readiness::pending();
    let startup = state.startup.clone();
    state.scheduled_tasks = Arc::new(hangar_server::scheduled_tasks::Service::from_engine(
        engine.clone(),
    ));
    let restored = hangar_server::startup::Readiness::pending();
    let mut producer = Box::pin(hangar_server::scheduled_tasks::run_after_restoration(
        Arc::new(state),
        restored.clone(),
    ));
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(producer.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    startup.finish();
    assert!(
        std::future::poll_fn(|cx| Poll::Ready(producer.as_mut().poll(cx)))
            .await
            .is_pending()
    );
    assert_eq!(engine.store.list().unwrap()[0].last_status, Status::Never);
    assert!(received.try_recv().is_err());
    restored.finish();
    let worker = tokio::spawn(producer);
    assert_eq!(received.recv().await.unwrap(), "Depois do restauro.");
    let reserved = engine
        .store
        .list()
        .unwrap()
        .into_iter()
        .find(|row| row.id == task.id)
        .unwrap();
    assert_eq!(
        reserved.next_run_at.as_deref(),
        Some("2026-11-12T14:05:00+00:00")
    );
    worker.abort();
    let _ = worker.await;
}
