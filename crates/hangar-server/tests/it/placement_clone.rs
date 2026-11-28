//! `POST /api/placement/clone`: o clone do balanceamento com um remoto local.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::{path::{Path, PathBuf}, process::Command, sync::Arc};

struct Fixture {
    base: tempfile::TempDir,
    url: String,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        let base = tempfile::tempdir().unwrap();
        let home = base.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address, internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
        ].into()));
        let router = hangar_server::routes::router(Arc::new(state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { base, url: format!("http://{address}/api/placement/clone"), task }
    }

    fn path(&self, rest: &str) -> PathBuf { self.base.path().join(rest) }

    async fn post(&self, body: Value, token: Option<&str>) -> (u16, Value) {
        let mut request = reqwest::Client::new().post(&self.url).header("content-type", "application/json").body(body.to_string());
        if let Some(token) = token { request = request.bearer_auth(token); }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        (status, serde_json::from_slice(&response.bytes().await.unwrap()).unwrap_or(Value::Null))
    }

    async fn clone(&self, origin: &Path, dir: &Path) -> (u16, Value) {
        self.post(json!({"origin": origin.to_string_lossy(), "dir": dir.to_string_lossy()}), Some("fixture-owner")).await
    }
}

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

/// `remote.git` com um commit em `main`.
fn remote(fixture: &Fixture) -> PathBuf {
    let (remote, work) = (fixture.path("remote.git"), fixture.path("work"));
    git(fixture.base.path(), &["init", "--bare", "-b", "main", &remote.to_string_lossy()]);
    git(fixture.base.path(), &["init", "-b", "main", &work.to_string_lossy()]);
    std::fs::write(work.join("README.md"), "oi").unwrap();
    git(&work, &["add", "README.md"]);
    git(&work, &["commit", "-m", "um"]);
    git(&work, &["push", &remote.to_string_lossy(), "HEAD:main"]);
    remote
}

fn code(body: &Value) -> &str { body["detail"]["code"].as_str().unwrap_or_default() }

fn entries(dir: &Path) -> Vec<String> {
    let mut names = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect()).unwrap_or_else(|_| Vec::new());
    names.sort();
    names
}

#[tokio::test]
async fn placement_clone_clones_by_a_temporary_folder_and_reuses_the_same_origin() {
    let fixture = Fixture::start().await;
    let origin = remote(&fixture);
    let dir = fixture.path("Projetos");
    let (status, body) = fixture.clone(&origin, &dir).await;
    let root = dir.join("remote");
    assert_eq!((status, &body), (200, &json!({"root": root.to_string_lossy()})));
    assert_eq!(std::fs::read_to_string(root.join("README.md")).unwrap(), "oi");
    assert_eq!(entries(&dir), ["remote"], "nada da pasta temporária fica");
    // A segunda abertura no mesmo repositório usa o clone que já existe.
    std::fs::write(root.join("deps.txt"), "instalado").unwrap();
    assert_eq!(fixture.clone(&origin, &dir).await, (200, json!({"root": root.to_string_lossy()})));
    assert!(root.join("deps.txt").exists());
}

#[tokio::test]
async fn placement_clone_refuses_a_folder_of_another_repository() {
    let fixture = Fixture::start().await;
    let origin = remote(&fixture);
    let taken = fixture.path("Projetos/remote");
    std::fs::create_dir_all(&taken).unwrap();
    git(&taken, &["init"]);
    git(&taken, &["remote", "add", "origin", "https://github.com/outro/remote.git"]);
    let (status, body) = fixture.clone(&origin, &fixture.path("Projetos")).await;
    assert_eq!((status, code(&body)), (409, "placement_clone_conflict"), "{body}");
    assert_eq!(body["detail"]["params"]["root"], json!(taken.to_string_lossy()));
}

#[tokio::test]
async fn placement_clone_failure_is_visible_and_leaves_nothing() {
    let fixture = Fixture::start().await;
    let dir = fixture.path("Projetos");
    let (status, body) = fixture.clone(&fixture.path("missing.git"), &dir).await;
    assert_eq!((status, code(&body)), (409, "placement_clone_failed"), "{body}");
    let msg = body["detail"]["msg"].as_str().unwrap();
    assert!(msg.starts_with("o clone de missing falhou: ") && !msg.contains("Cloning into"), "{msg}");
    assert!(entries(&dir).is_empty(), "{:?}", entries(&dir));
}

#[tokio::test]
async fn placement_clone_is_only_for_the_owner_and_checks_the_body() {
    let fixture = Fixture::start().await;
    let dir = fixture.path("Projetos");
    let body = json!({"origin": fixture.path("remote.git").to_string_lossy(), "dir": dir.to_string_lossy()});
    assert_eq!(fixture.post(body.clone(), None).await.0, 401);
    assert_eq!(fixture.post(body, Some("outro")).await.0, 401);
    let (status, reply) = fixture.post(json!({"origin": "x"}), Some("fixture-owner")).await;
    assert_eq!((status, code(&reply)), (422, "erro_corpo_invalido"));
    let (status, reply) = fixture.post(json!({"origin": "https://github.com/a/b", "dir": "relativa"}), Some("fixture-owner")).await;
    assert_eq!((status, code(&reply)), (422, "placement_clone_invalid"));
    assert!(!dir.exists());
}
