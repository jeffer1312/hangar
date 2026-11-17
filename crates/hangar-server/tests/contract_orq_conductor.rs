mod contract_orq_conductor_cases;

use serde_json::Value;
#[test]
fn conductor_feed_answers_each_case() {
    std::thread::Builder::new()
        .stack_size(64 * 1024 * 1024)
        .spawn(feed_cases)
        .unwrap()
        .join()
        .unwrap();
}
fn feed_cases() {
    let cases = contract_orq_conductor_cases::cases();
    assert_eq!(cases.len(), 93);
    for case in &cases {
        let root = tempfile::tempdir().unwrap();
        for (path, text) in &case.files {
            std::fs::write(root.path().join(path), text).unwrap()
        }
        if let Some(bytes) = &case.journal_bytes {
            std::fs::write(root.path().join("registro.md"), bytes).unwrap()
        }
        let actual = hangar_server::orq_conductor::feed_json_with_timezone(
            root.path(),
            chrono::FixedOffset::east_opt(0).unwrap(),
        );
        if let Some(expected) = &case.expected {
            let bytes = actual.unwrap();
            let text = String::from_utf8(bytes).unwrap();
            // Texto exato: a ordem das chaves e os inteiros enormes contam.
            assert_eq!(text, *expected, "cenário {}", case.name);
        } else {
            assert!(actual.is_err(), "{}: precisa continuar uma recusa", case.name)
        }
    }
}
proptest::proptest! {
 #[test]
 fn conductor_external_bytes_never_panic(bytes in proptest::collection::vec(proptest::num::u8::ANY,0..4096)){
  let root=tempfile::tempdir().unwrap();for name in["registro.md","jev-shadow.jsonl","eventos.jsonl"]{std::fs::write(root.path().join(name),&bytes).unwrap()}
  let _=hangar_server::orq_conductor::feed(root.path());
 }
}

use axum::http::StatusCode;
use serde_json::json;
use std::{net::SocketAddr, sync::Arc};
fn watchdog() -> Value {
    json!({"alive":true,"last_cycle":null,"since":null,"restarts":null,"unit":null,"unit_state":null,"arbiter":null,"watching":[],"source":"heartbeat"})
}
struct Servers {
    public: SocketAddr,
    tasks: Vec<tokio::task::JoinHandle<()>>,
    _home: tempfile::TempDir,
    fixture: std::path::PathBuf,
    expected_watchdog: Value,
}
impl Drop for Servers {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort()
        }
    }
}
async fn bind(router: axum::Router) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (
        addr,
        tokio::spawn(async move {
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap()
        }),
    )
}
async fn servers(root: &std::path::Path) -> Servers {
    let cfg = hangar_server::config::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        internal_secret: "fixture-secret".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: hangar_server::auth::TrustedHosts::parse("127.0.0.1"),
    };
    let home = tempfile::tempdir().unwrap();
    let runs = home.path().join(".hangar/orq");
    std::fs::create_dir_all(&runs).unwrap();
    let mut expected_watchdog = watchdog();
    let ts = chrono::Local::now().to_rfc3339();
    expected_watchdog["last_cycle"] = json!(ts);
    for id in [
        "empty",
        "fixture",
        "large-watchdog",
        "broken",
        "invalid",
        "incomplete",
        "array",
        "null",
        "malformed",
        "oversized",
        "slow",
    ] {
        let dir = runs.join(id);
        std::fs::create_dir(&dir).unwrap();
        for file in std::fs::read_dir(root).unwrap().flatten() {
            if file.path().is_file() {
                std::fs::copy(file.path(), dir.join(file.file_name())).unwrap();
            }
        }
        let watching = if id == "large-watchdog" {
            json!(["x".repeat(100_000)])
        } else {
            json!([])
        };
        std::fs::write(
            dir.join("vigia.json"),
            json!({"ts":ts,"pid":std::process::id(),"interval_s":60,"watching":watching})
                .to_string(),
        )
        .unwrap();
    }
    let fixture = runs.join("fixture");
    let mut state = hangar_server::routes::AppState::new(cfg);
    state.accounts.env = hangar_server::accounts::environment::AccountEnvironment::from_map(
        std::collections::BTreeMap::from([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                home.path().to_string_lossy().into_owned(),
            ),
        ]),
    );
    let state = Arc::new(state);
    let (public, task) = bind(hangar_server::routes::router(state)).await;
    Servers {
        public,
        tasks: vec![task],
        _home: home,
        fixture,
        expected_watchdog,
    }
}
#[tokio::test]
async fn conductor_http_owner_auth_and_readonly() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("registro.md"),
        "- 2026-09-25T13:00:00Z · aviso: ação\n",
    )
    .unwrap();
    let servers = servers(root.path()).await;
    let client = reqwest::Client::new();
    let url = format!(
        "http://{}/api/orq/empty/conductor?ignored=1",
        servers.public
    );
    let answer = client
        .get(&url)
        .bearer_auth("fixture-owner")
        .send()
        .await
        .unwrap();
    assert_eq!(answer.status(), StatusCode::OK);
    let bytes = answer.bytes().await.unwrap();
    assert!(bytes.starts_with(b"{\"watchdog\":"));
    let answer: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(answer["watchdog"], servers.expected_watchdog);
    assert_eq!(answer["feed"][0]["text"], "ação");
    assert_eq!(answer.as_object().unwrap().len(), 4);
    assert_eq!(
        client.get(&url).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    for method in [reqwest::Method::HEAD, reqwest::Method::POST] {
        for authenticated in [false, true] {
            let request = client.request(method.clone(), &url);
            let request = if authenticated { request.bearer_auth("fixture-owner") } else { request };
            let response = request.send().await.unwrap();
            assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
            assert_eq!(response.headers()["allow"], "GET");
        }
    }
    for id in ["a:b", "a%5Cb", "a%00b", "missing"] {
        let answer = client
            .get(format!("http://{}/api/orq/{id}/conductor", servers.public))
            .bearer_auth("fixture-owner")
            .send()
            .await
            .unwrap();
        assert_eq!(answer.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            decoded(answer).await["detail"]["code"],
            "erro_nao_encontrado"
        )
    }
    let large = client
        .get(format!(
            "http://{}/api/orq/large-watchdog/conductor",
            servers.public
        ))
        .bearer_auth("fixture-owner")
        .send()
        .await
        .unwrap();
    assert_eq!(large.status(), StatusCode::OK);
    assert_eq!(
        decoded(large).await["watchdog"]["watching"][0]
            .as_str()
            .unwrap()
            .len(),
        100_000
    );
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}
#[tokio::test]
async fn conductor_http_factual_errors_are_visible() {
    let root = tempfile::tempdir().unwrap();
    let servers = servers(root.path()).await;
    let client = reqwest::Client::new();
    // Os oito retornos da ponte antiga não podem influenciar o leitor local.
    for id in [
        "broken",
        "invalid",
        "incomplete",
        "array",
        "null",
        "malformed",
        "oversized",
        "slow",
    ] {
        let response = client
            .get(format!("http://{}/api/orq/{id}/conductor", servers.public))
            .bearer_auth("fixture-owner")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{id}");
        let body = decoded(response).await;
        assert_eq!(body["watchdog"], servers.expected_watchdog, "{id}");
        assert!(body["feed"].as_array().unwrap().is_empty());
    }
}

async fn decoded(response: reqwest::Response) -> Value {
    serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
}

fn external_json() -> impl proptest::strategy::Strategy<Value = Value> {
    use proptest::prelude::*;
    let leaf = prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<i64>().prop_map(|v| json!(v)),
        ".*".prop_map(Value::String)
    ];
    leaf.prop_recursive(4, 64, 8, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..8).prop_map(Value::Array),
            proptest::collection::vec((".*", inner), 0..8)
                .prop_map(|rows| Value::Object(rows.into_iter().collect()))
        ]
    })
}
proptest::proptest! {
 #[test]
 fn conductor_external_json_never_panics(rows in proptest::collection::vec(external_json(),0..20)){
  let root=tempfile::tempdir().unwrap();let content=rows.iter().map(Value::to_string).collect::<Vec<_>>().join("\n");
  std::fs::write(root.path().join("eventos.jsonl"),&content).unwrap();std::fs::write(root.path().join("jev-shadow.jsonl"),&content).unwrap();
  let _=hangar_server::orq_conductor::feed(root.path());
 }
}

#[tokio::test]
async fn conductor_http_python_errors_remain_visible() {
    let root = tempfile::tempdir().unwrap();
    let servers = servers(root.path()).await;
    let client = reqwest::Client::new();
    let journal = "- 2026-09-25T13:00:00Z · notify → arbiter: x\n";
    for (events, shadow, code) in [
        (
            "{\"ts\":\"2026-09-25T13:00:00Z\",\"tipo\":[]}".to_owned(),
            String::new(),
            "orq_conductor_python_unhashable_type",
        ),
        (
            String::new(),
            format!(
                "{{\"ts\":\"2026-09-25T13:00:00Z\",\"text\":\"x\",\"p\":1{}}}",
                "0".repeat(400)
            ),
            "orq_conductor_python_number_overflow",
        ),
        (
            String::new(),
            "{\"ts\":\"2026-09-25T13:00:00Z\",\"text\":\"x\",\"mode\":NaN}".to_owned(),
            "orq_conductor_python_nonfinite_output",
        ),
        (
            r#"{"ts":"2026-09-25T13:00:00Z","tipo":"entrega","resultado":"\ud800"}"#.to_owned(),
            String::new(),
            "orq_conductor_python_nonfinite_output",
        ),
    ] {
        std::fs::write(servers.fixture.join("registro.md"), journal).unwrap();
        std::fs::write(servers.fixture.join("eventos.jsonl"), events).unwrap();
        std::fs::write(servers.fixture.join("jev-shadow.jsonl"), shadow).unwrap();
        let response = client
            .get(format!(
                "http://{}/api/orq/fixture/conductor",
                servers.public
            ))
            .bearer_auth("fixture-owner")
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(decoded(response).await["detail"]["code"], code);
    }
}
