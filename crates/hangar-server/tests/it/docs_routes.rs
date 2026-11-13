//! `GET /api/news` e `GET /api/manual`: os documentos da versão em uso, só para o dono.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::Value;
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
        Self { home, base: format!("http://{address}"), task }
    }

    /// `current` apontando para uma versão com só as pastas de `docs/` pedidas.
    fn install(&self, folders: &[&str]) -> std::path::PathBuf {
        let dir = self.home.path().join(".hangar/versions/0.1.0.1");
        for folder in folders { std::fs::create_dir_all(dir.join("docs").join(folder)).unwrap(); }
        std::fs::create_dir_all(&dir).unwrap();
        hangar_server::update::swap(&hangar_server::update::Layout::new(self.home.path()), &dir).unwrap();
        dir
    }

    async fn get(&self, path: &str, owner: bool) -> reqwest::Response {
        let request = reqwest::Client::new().get(format!("{}{path}", self.base));
        if owner { request.bearer_auth("fixture-owner") } else { request }.send().await.unwrap()
    }
}

async fn body(response: reqwest::Response) -> Value { serde_json::from_slice(&response.bytes().await.unwrap()).unwrap() }

#[tokio::test]
async fn docs_routes_read_the_current_version() {
    let fixture = Fixture::start().await;
    let dir = fixture.install(&["novidades", "manual"]);
    std::fs::write(dir.join("docs/manual/primeira.md"), "---\ntitulo: Primeira\ncategoria: comecar\nonde: todos\n---\nintro\n").unwrap();
    std::fs::write(dir.join("docs/novidades/2026-11-01-plugins.md"),
        "---\ntitulo: Plugins\ntipo: novo\nonde: todos\nmanual: primeira\n---\ntexto\n").unwrap();
    let response = fixture.get("/api/news", true).await;
    assert_eq!(response.status(), 200);
    let news = body(response).await;
    assert_eq!(news["news"][0]["id"], "2026-11-01-plugins");
    assert_eq!(news["news"][0]["manual"], "primeira");
    let response = fixture.get("/api/manual", true).await;
    assert_eq!(response.status(), 200);
    let manual = body(response).await;
    assert_eq!(manual["categories"][0]["id"], "comecar");
    assert_eq!(manual["categories"][0]["articles"][0]["news"][0]["id"], "2026-11-01-plugins");
}

#[tokio::test]
async fn docs_routes_missing_folder_is_503() {
    let fixture = Fixture::start().await;
    fixture.install(&[]);
    for (path, code) in [("/api/news", "news_unavailable"), ("/api/manual", "manual_unavailable")] {
        let response = fixture.get(path, true).await;
        assert_eq!(response.status(), 503, "{path}");
        assert_eq!(body(response).await["detail"]["code"], code);
    }
}

#[tokio::test]
async fn docs_routes_refuse_guests() {
    let fixture = Fixture::start().await;
    fixture.install(&["novidades", "manual"]);
    for path in ["/api/news", "/api/manual"] {
        let response = fixture.get(path, false).await;
        assert_eq!(response.status(), 403, "{path}");
        assert_eq!(body(response).await["detail"]["code"], "docs_owner_only");
    }
}
