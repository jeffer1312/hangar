use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use axum::{body::Body, extract::ConnectInfo, http::Request};
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    dictation::catalog::Models,
    routes::{AppState, router},
};
use serde_json::{Value, json};
use tower::ServiceExt;

struct Scene {
    root: tempfile::TempDir,
    service: AccountService,
    home: PathBuf,
    program: PathBuf,
}

impl Scene {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        let home = root.path().join(".claude");
        std::fs::create_dir(&bin).unwrap();
        std::fs::create_dir(&home).unwrap();
        let program = bin.join(if cfg!(windows) { "claude.exe" } else { "claude" });
        std::fs::copy(env!("CARGO_BIN_EXE_hangar-dictation-fixture"), &program).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_hangar-dictation-fixture"),
            bin.join(if cfg!(windows) { "codex.exe" } else { "codex" })).unwrap();
        let service = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            ("PATH".into(), bin.to_string_lossy().into_owned()),
        ].into()));
        Self { root, service, home, program }
    }

    fn catalog(&self, id: &str) {
        std::fs::write(self.home.join("fixture-catalog.json"),
            json!({"models":[{"value":id,"displayName":id}]}).to_string()).unwrap();
    }
}

#[tokio::test]
async fn claude_creation_catalog_ignores_legacy_cache_and_refreshes_explicitly() {
    let scene = Scene::new();
    scene.catalog("new-model");
    std::fs::write(scene.home.join(".hangar-models.json"), json!({
        "ts": 4102444800u64, "resp": {"models":[{"id":"stale-model"}]}
    }).to_string()).unwrap();
    let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut state = AppState::new(Config {
        listen: address, upstream: address, internal_secret: "internal".into(),
        auth_token: "owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(scene.service.env.clone());
    let app = router(Arc::new(state));
    for (suffix, expected) in [("", "new-model"), ("", "new-model"), ("&refresh=1", "refreshed-model")] {
        let request = Request::builder().uri(format!("/api/model-options?provider=claude{suffix}"))
            .header("authorization", "Bearer owner").extension(ConnectInfo(address))
            .body(Body::empty()).unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap_or_else(|_| json!(String::from_utf8_lossy(&body)));
        assert_eq!(status, 200, "{value}");
        assert_eq!(value["models"][0]["id"], expected);
        std::fs::write(scene.home.join("fixture-catalog.json"),
            json!({"models":[{"value":"refreshed-model","displayName":"Refreshed"}]}).to_string()).unwrap();
    }
    // A mesma rota também serve a conta Codex escolhida na abertura.
    let codex_home = scene.root.path().join(".codex-work");
    std::fs::create_dir(&codex_home).unwrap();
    std::fs::write(codex_home.join(hangar_server::accounts::storage::CODEX_MARKER),
        json!({"version":1,"id":"work"}).to_string()).unwrap();
    let response = app.oneshot(Request::builder()
        .uri("/api/model-options?provider=codex&codex_account=work")
        .header("authorization", "Bearer owner").extension(ConnectInfo(address))
        .body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["models"][0]["id"], "model-.codex-work");
}

#[tokio::test]
async fn claude_catalog_detects_cli_upgrade_before_cache_expires() {
    let scene = Scene::new();
    let models = Models::default();
    let slots = tokio::sync::Semaphore::new(1);
    scene.catalog("before-upgrade");
    let (first, _) = models.get(&scene.service, &slots, "claude", &scene.home, false).await.unwrap();
    assert_eq!(first[0]["id"], "before-upgrade");
    scene.catalog("after-upgrade");
    let (cached, _) = models.get(&scene.service, &slots, "claude", &scene.home, false).await.unwrap();
    assert_eq!(cached[0]["id"], "before-upgrade");
    // O instalador troca o executável mantendo o caminho público.
    let replacement = scene.root.path().join("replacement");
    std::fs::copy(&scene.program, &replacement).unwrap();
    use std::io::Write;
    std::fs::OpenOptions::new().append(true).open(&replacement).unwrap().write_all(b"fixture-upgrade").unwrap();
    std::fs::rename(&replacement, &scene.program).unwrap();
    let (updated, _) = models.get(&scene.service, &slots, "claude", &scene.home, false).await.unwrap();
    assert_eq!(updated[0]["id"], "after-upgrade");
}
