//! O servidor novo grava na subida o que a atualização trouxe, e cada aparelho do dono vê uma vez.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

struct Fixture {
    home: tempfile::TempDir,
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

impl Fixture {
    async fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address, upstream: crate::refused_address(), internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
        ].into()));
        let router = hangar_server::routes::router(Arc::new(state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { home, base: format!("http://{address}/api/update/arrived"), task }
    }

    /// Pasta `versions/<version>` com as novidades `news` (nome → `tipo`) e o `current` apontando para ela.
    fn install(&self, version: &str, news: &[(&str, &str)]) {
        let dir = self.home.path().join(".hangar/versions").join(version);
        std::fs::create_dir_all(dir.join("docs/novidades")).unwrap();
        std::fs::write(dir.join("manifest.json"), json!({"version": version, "commit": "c", "channel": "latest", "protocol": 1,
            "api": 5, "os": hangar_server::update::platform(), "files": {}}).to_string()).unwrap();
        std::fs::write(dir.join("docs/novidades/README.md"), "não é novidade").unwrap();
        for (name, kind) in news {
            std::fs::write(dir.join(format!("docs/novidades/{name}.md")),
                format!("---\ntitulo: {name}\ntipo: {kind}\nonde: todos\n---\ntexto de {name}\n")).unwrap();
        }
        hangar_server::update::swap(&hangar_server::update::Layout::new(self.home.path()), &dir).unwrap();
    }

    async fn record(&self) { hangar_server::update_arrived::record_startup(self.home.path()).await; }

    async fn get(&self, device: &str, owner: bool) -> reqwest::Response {
        let request = reqwest::Client::new().get(format!("{}?device={device}", self.base));
        if owner { request.bearer_auth("fixture-owner") } else { request }.send().await.unwrap()
    }

    async fn unseen(&self, device: &str) -> Value {
        let response = self.get(device, true).await;
        assert_eq!(response.status(), 200);
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
    }

    async fn seen(&self, body: &str, owner: bool) -> reqwest::Response {
        let request = reqwest::Client::new().post(format!("{}/seen", self.base)).header("content-type", "application/json").body(body.to_owned());
        if owner { request.bearer_auth("fixture-owner") } else { request }.send().await.unwrap()
    }

    fn store(&self) -> Value { read(self.home.path()) }
}

fn read(home: &Path) -> Value { serde_json::from_slice(&std::fs::read(home.join(".hangar/update-news.json")).unwrap()).unwrap() }

fn ids(news: &Value) -> Vec<&str> { news.as_array().unwrap().iter().map(|item| item["id"].as_str().unwrap()).collect() }

async fn code(response: reqwest::Response) -> Value {
    let body: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    body["detail"]["code"].clone()
}

#[tokio::test]
async fn update_arrived_records_what_the_new_package_brought_once() {
    let fixture = Fixture::start().await;
    fixture.install("0.1.0.1", &[("velha", "novo")]);
    fixture.install("0.1.0.2", &[("velha", "novo"), ("plugins", "novo")]);
    fixture.record().await;
    let store = fixture.store();
    assert_eq!(store["version"], "0.1.0.2");
    let arrivals = store["arrivals"].as_array().unwrap();
    assert_eq!(arrivals.len(), 1);
    assert_eq!((arrivals[0]["from"].as_str(), arrivals[0]["to"].as_str()), (Some("0.1.0.1"), Some("0.1.0.2")));
    assert_eq!(arrivals[0]["news"], json!([{"id": "plugins", "title": "plugins", "kind": "new", "text": "texto de plugins",
        "platforms": "all", "try_it": null, "animation": null}]));
    fixture.record().await;
    assert_eq!(fixture.store()["arrivals"].as_array().unwrap().len(), 1, "a mesma versão não grava de novo");
}

#[tokio::test]
async fn update_arrived_first_package_without_previous_records_only_the_version() {
    let fixture = Fixture::start().await;
    fixture.install("0.1.0.1", &[("velha", "novo")]);
    fixture.record().await;
    let store = fixture.store();
    assert_eq!(store["version"], "0.1.0.1");
    assert_eq!(store["arrivals"], json!([]));
}

#[tokio::test]
async fn update_arrived_each_device_sees_a_new_arrival_once_in_kind_order() {
    let fixture = Fixture::start().await;
    fixture.install("0.1.0.1", &[]);
    fixture.record().await;
    assert_eq!(fixture.unseen("phone-1").await, json!({"news": [], "through": 0}), "aparelho novo começa sem nada");
    fixture.install("0.1.0.2", &[("b-fix", "correcao"), ("c-new", "novo"), ("a-better", "melhoria")]);
    fixture.record().await;
    let unseen = fixture.unseen("phone-1").await;
    assert_eq!(ids(&unseen["news"]), ["c-new", "a-better", "b-fix"]);
    assert_eq!(unseen["through"], 1);
    let response = fixture.seen(r#"{"device": "phone-1", "through": 1}"#, true).await;
    assert_eq!(response.status(), 200);
    assert_eq!(serde_json::from_slice::<Value>(&response.bytes().await.unwrap()).unwrap(), json!({"through": 1}));
    assert_eq!(fixture.unseen("phone-1").await, json!({"news": [], "through": 1}));
}

#[tokio::test]
async fn update_arrived_refuses_guests_before_any_write() {
    let fixture = Fixture::start().await;
    let response = fixture.get("phone-1", false).await;
    assert_eq!(response.status(), 403);
    assert_eq!(code(response).await, "update_arrived_owner_only");
    let response = fixture.seen(r#"{"device": "phone-1", "through": 1}"#, false).await;
    assert_eq!(response.status(), 403);
    assert!(!fixture.home.path().join(".hangar/update-news.json").exists());
}

#[tokio::test]
async fn update_arrived_bad_device_or_body_is_422() {
    let fixture = Fixture::start().await;
    for device in ["", "a%20b", &"x".repeat(65)] {
        let response = fixture.get(device, true).await;
        assert_eq!(response.status(), 422, "{device:?}");
        assert_eq!(code(response).await, "update_arrived_bad_device");
    }
    for body in ["not json", r#"{"device": "a b", "through": 1}"#, r#"{"device": "phone-1"}"#, r#"{"device": "phone-1", "through": -1}"#] {
        let response = fixture.seen(body, true).await;
        assert_eq!(response.status(), 422, "{body}");
        assert_eq!(code(response).await, "update_arrived_bad_device");
    }
    assert!(!fixture.home.path().join(".hangar/update-news.json").exists());
}

#[tokio::test]
async fn update_arrived_unreadable_store_is_503() {
    let fixture = Fixture::start().await;
    std::fs::create_dir_all(fixture.home.path().join(".hangar")).unwrap();
    std::fs::write(fixture.home.path().join(".hangar/update-news.json"), "broken").unwrap();
    let response = fixture.get("phone-1", true).await;
    assert_eq!(response.status(), 503);
    assert_eq!(code(response).await, "update_arrived_unavailable");
}

#[cfg(unix)]
fn git(dir: &Path, script: &str) {
    let ok = std::process::Command::new("sh").arg("-c").arg(script).current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .status().unwrap().success();
    assert!(ok, "{script}");
}

#[cfg(unix)]
#[tokio::test]
async fn update_arrived_checkout_records_files_added_since_the_known_commit() {
    let fixture = Fixture::start().await;
    let repo = fixture.home.path().join("repo");
    std::fs::create_dir_all(repo.join("docs/novidades")).unwrap();
    std::fs::write(repo.join("docs/novidades/velha.md"), "---\ntitulo: velha\ntipo: novo\nonde: todos\n---\nv\n").unwrap();
    git(&repo, "git init -q -b main && git add . && git commit -qm um");
    std::fs::create_dir_all(fixture.home.path().join(".hangar")).unwrap();
    hangar_server::update::swap(&hangar_server::update::Layout::new(fixture.home.path()), &repo).unwrap();
    fixture.record().await;
    let first = fixture.store();
    assert_eq!(first["arrivals"], json!([]), "sem versão conhecida só grava o commit");
    std::fs::write(repo.join("docs/novidades/plugins.md"), "---\ntitulo: plugins\ntipo: melhoria\nonde: desktop\n---\np\n").unwrap();
    git(&repo, "git add . && git commit -qm dois");
    fixture.record().await;
    let store = fixture.store();
    assert_ne!(store["version"], first["version"]);
    let arrivals = store["arrivals"].as_array().unwrap();
    assert_eq!(arrivals.len(), 1);
    assert_eq!(arrivals[0]["from"], first["version"]);
    assert_eq!(ids(&arrivals[0]["news"]), ["plugins"]);
}
