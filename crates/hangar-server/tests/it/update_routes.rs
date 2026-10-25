//! `/api/atualizacao` no Rust: o dono na máquina migrada (pacote ou modo desenvolvedor) recebe a
//! resposta do Rust, no formato que os clientes leem; máquina não migrada e quem não é dono seguem
//! ao Python.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

struct Fixture {
    home: tempfile::TempDir,
    base: String,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl Drop for Fixture { fn drop(&mut self) { for task in &self.tasks { task.abort(); } } }

async fn serve(router: axum::Router) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    (address, tokio::spawn(async move { axum::serve(listener, router).await.unwrap(); }))
}

impl Fixture {
    /// O Python falso responde `python` a tudo; o índice falso publica `remote` para este sistema.
    async fn start(remote: &str) -> Self {
        let home = tempfile::tempdir().unwrap();
        let (upstream, python) = serve(axum::Router::new().fallback(|| async { "python" })).await;
        let index = json!({"platforms": {hangar_server::update::platform(): {"version": remote, "asset": "a.tar.gz",
            "sha256": "0".repeat(64), "protocol": 1, "api": 7, "commit": "novo", "dist_key": null}}}).to_string();
        let (release, index_task) = serve(axum::Router::new().fallback(move || { let index = index.clone(); async move { index } })).await;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address, upstream, internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned()),
        ].into()));
        state.update_release_base = format!("http://{release}");
        let router = hangar_server::routes::router(Arc::new(state));
        let server = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { home, base: format!("http://{address}"), tasks: vec![python, index_task, server] }
    }

    /// `~/.hangar/current` → `target`.
    fn point_current(&self, target: &Path) {
        std::fs::create_dir_all(self.home.path().join(".hangar")).unwrap();
        hangar_server::update::swap(&hangar_server::update::Layout::new(self.home.path()), target).unwrap();
    }

    fn package(&self, version: &str) {
        let dir = self.home.path().join(".hangar/versions").join(version);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), json!({"version": version, "commit": "velho", "channel": "latest", "protocol": 1,
            "api": 5, "os": hangar_server::update::platform(), "files": {}}).to_string()).unwrap();
        self.point_current(&dir);
    }

    async fn call(&self, method: reqwest::Method, path: &str, owner: bool) -> reqwest::Response {
        let request = reqwest::Client::new().request(method, format!("{}{path}", self.base));
        let request = if owner { request.bearer_auth("fixture-owner") } else { request };
        request.send().await.unwrap()
    }
}

async fn body(response: reqwest::Response) -> Value { serde_json::from_slice(&response.bytes().await.unwrap()).unwrap() }

#[tokio::test]
async fn update_routes_package_mode_is_answered_by_rust_in_the_client_shape() {
    let fixture = Fixture::start("0.1.0.8207").await;
    fixture.package("0.1.0.8200");
    let response = fixture.call(reqwest::Method::GET, "/api/atualizacao?procurar=1", true).await;
    assert_eq!(response.status(), 200);
    let value = body(response).await;
    assert_eq!(value["modo"], "pacote");
    assert_eq!((value["atualizacao_disponivel"].clone(), value["atras"].clone(), value["api_publicado"].clone()), (json!(true), json!(7), json!(7)));
    assert_eq!(value["versoes"]["repo"], "0.1.0.8200");
    assert_eq!(value["versao_legivel"]["remoto"], "0.1.0.8207");
    assert_eq!((value["pre_voo"]["pode"].clone(), value["pre_voo"]["alvo"].clone(), value["pre_voo"]["commit"].clone()),
        (json!(true), json!("main"), json!("velho")));
    assert!(["systemd", "windows", "manual"].contains(&value["pre_voo"]["topologia"].as_str().unwrap()));
    assert!(value["mudancas"].is_array() && value["passos"].is_array() && value["estado"].is_object());
}

#[tokio::test]
async fn update_routes_unmigrated_machine_and_strangers_go_to_python() {
    let fixture = Fixture::start("0.1.0.1").await;
    let unmigrated = fixture.call(reqwest::Method::GET, "/api/atualizacao", true).await;
    assert_eq!(unmigrated.text().await.unwrap(), "python");
    assert_eq!(fixture.call(reqwest::Method::POST, "/api/atualizacao/iniciar", true).await.text().await.unwrap(), "python");
    assert_eq!(fixture.call(reqwest::Method::POST, "/api/atualizacao/reiniciar", true).await.text().await.unwrap(), "python");

    fixture.package("0.1.0.1");
    // Sem o token do dono: o Python decide (e recusa), nunca o Rust.
    for (method, path) in [(reqwest::Method::GET, "/api/atualizacao"), (reqwest::Method::POST, "/api/atualizacao/iniciar"),
                           (reqwest::Method::POST, "/api/atualizacao/reiniciar")] {
        assert_eq!(fixture.call(method, path, false).await.text().await.unwrap(), "python", "{path}");
    }
}

fn sh(dir: &Path, script: &str) {
    let ok = std::process::Command::new("sh").arg("-c").arg(script).current_dir(dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .status().unwrap().success();
    assert!(ok, "{script}");
}

#[cfg(unix)]
#[tokio::test]
async fn update_routes_developer_mode_refuses_a_dirty_checkout() {
    let fixture = Fixture::start("0.1.0.1").await;
    let work = tempfile::tempdir().unwrap();
    sh(work.path(), "git init -q -b main origin && cd origin && echo a > a && git add a && git commit -qm um && cd .. && git clone -q origin clone");
    let clone = work.path().join("clone");
    std::fs::write(clone.join("a"), "editado").unwrap();
    fixture.point_current(&clone);

    let status = body(fixture.call(reqwest::Method::GET, "/api/atualizacao", true).await).await;
    assert_eq!((status["modo"].clone(), status["pre_voo"]["sujo"].clone(), status["pre_voo"]["branch"].clone()),
        (json!("desenvolvedor"), json!(1), json!("main")));

    let refused = fixture.call(reqwest::Method::POST, "/api/atualizacao/iniciar", true).await;
    assert_eq!(refused.status(), 409);
    let value = body(refused).await;
    assert_eq!(value["detail"]["code"], "erro_atualizacao_desenvolvedor");
    assert!(value["detail"]["msg"].as_str().unwrap().contains("com edição"), "{value}");
    assert_eq!(std::fs::read_to_string(clone.join("a")).unwrap(), "editado", "nada tocado");
    let state = fixture.home.path().join(".claude/.hangar-update/estado.json");
    assert!(!state.exists(), "nada lançado");
}

#[tokio::test]
async fn update_routes_incomplete_manifest_is_package_mode_with_a_clear_refusal() {
    let fixture = Fixture::start("0.1.0.9").await;
    let dir = fixture.home.path().join(".hangar/versions/0.1.0.1");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("manifest.json"), r#"{"version": "0.1.0.1", "commit": "c", "channel": "latest"}"#).unwrap();
    fixture.point_current(&dir);

    let status = body(fixture.call(reqwest::Method::GET, "/api/atualizacao", true).await).await;
    assert_eq!((status["modo"].clone(), status["pre_voo"]["pode"].clone(), status["pre_voo"]["erro"].clone()),
        (json!("pacote"), json!(false), json!("manifesto_invalido")));

    let refused = fixture.call(reqwest::Method::POST, "/api/atualizacao/iniciar", true).await;
    assert_eq!(refused.status(), 409, "não cai no Python");
    let value = body(refused).await;
    assert_eq!(value["detail"]["code"], "erro_atualizacao_manifesto");
    let state: Value = serde_json::from_slice(&std::fs::read(fixture.home.path().join(".claude/.hangar-update/estado.json")).unwrap()).unwrap();
    assert_eq!((state["fase"].clone(), state["ok"].clone()), (json!("pronto"), json!(false)));
    assert!(state["erro"].as_str().unwrap().contains("manifest.json"), "{state}");
}

impl Fixture {
    /// Um checkout do Hangar ao lado da casa: `.git` e `backend/app`.
    fn checkout(&self) -> std::path::PathBuf {
        let dir = self.home.path().join("hangar");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join("backend/app")).unwrap();
        dir
    }

    async fn dev_mode(&self, body: Value, owner: bool) -> reqwest::Response {
        let request = reqwest::Client::new().post(format!("{}/api/atualizacao/dev-mode", self.base)).json(&body);
        let request = if owner { request.bearer_auth("fixture-owner") } else { request };
        request.send().await.unwrap()
    }

    fn state(&self) -> std::path::PathBuf { self.home.path().join(".claude/.hangar-update/estado.json") }
}

#[tokio::test]
async fn update_routes_status_says_the_channel_and_the_checkout_the_screen_offers() {
    let fixture = Fixture::start("0.1.0.1").await;
    fixture.package("0.1.0.1");
    let status = body(fixture.call(reqwest::Method::GET, "/api/atualizacao", true).await).await;
    assert_eq!((status["canal"].clone(), status["checkout"].clone()), (json!("latest"), json!(null)));
    std::fs::write(fixture.home.path().join(".hangar/migration.json"), json!({"mode": "package", "date": "d", "checkout": "/x/hangar",
        "version": null, "reason": null, "notes": [], "done": []}).to_string()).unwrap();
    let status = body(fixture.call(reqwest::Method::GET, "/api/atualizacao", true).await).await;
    assert_eq!(status["checkout"], "/x/hangar");
}

/// Cada recusa chega antes de lançar o executor: código na resposta, estado e `current` intactos.
#[tokio::test]
async fn update_routes_dev_mode_refuses_with_a_code_and_launches_nothing() {
    let fixture = Fixture::start("0.1.0.1").await;
    let checkout = fixture.checkout();
    let on = json!({"on": true, "checkout": checkout.display().to_string()});

    let unmigrated = fixture.dev_mode(on.clone(), true).await;
    assert_eq!(unmigrated.status(), 409);
    assert_eq!(body(unmigrated).await["detail"]["code"], "erro_dev_mode_nao_migrado");

    fixture.package("0.1.0.1");
    for bad in [json!({}), json!({"on": true}), json!({"on": true, "checkout": "relativa"}), json!({"on": "sim"})] {
        let response = fixture.dev_mode(bad.clone(), true).await;
        assert_eq!(response.status(), 422, "{bad}");
        assert_eq!(body(response).await["detail"]["code"], "erro_corpo_invalido");
    }
    let not_checkout = fixture.dev_mode(json!({"on": true, "checkout": fixture.home.path().display().to_string()}), true).await;
    assert_eq!(not_checkout.status(), 409);
    assert_eq!(body(not_checkout).await["detail"]["code"], "erro_dev_mode_nao_e_checkout");
    let already = fixture.dev_mode(json!({"on": false}), true).await;
    assert_eq!(already.status(), 200);
    let value = body(already).await;
    assert_eq!((value["ok"].clone(), value["nada_a_fazer"].clone()), (json!(true), json!(true)), "{value}");
    assert!(!fixture.state().exists(), "nada lançado");

    // Sem o token do dono: o Python decide, nunca o Rust.
    assert_eq!(fixture.dev_mode(on, false).await.text().await.unwrap(), "python");
}

#[tokio::test]
async fn update_routes_dev_mode_off_without_a_package_refuses() {
    let fixture = Fixture::start("0.1.0.1").await;
    let checkout = fixture.checkout();
    fixture.point_current(&checkout);
    let status = body(fixture.call(reqwest::Method::GET, "/api/atualizacao", true).await).await;
    assert_eq!(status["modo"], "desenvolvedor");
    // Pela pasta canônica: no Windows o `canonicalize` daqui traz o prefixo `\\?\`, e o servidor o tira.
    let offered = std::fs::canonicalize(status["checkout"].as_str().unwrap()).unwrap();
    assert_eq!(offered, std::fs::canonicalize(&checkout).unwrap());

    let already = body(fixture.dev_mode(json!({"on": true, "checkout": checkout.display().to_string()}), true).await).await;
    assert_eq!(already["nada_a_fazer"], true);
    let refused = fixture.dev_mode(json!({"on": false}), true).await;
    assert_eq!(refused.status(), 409);
    assert_eq!(body(refused).await["detail"]["code"], "erro_dev_mode_sem_pacote");
    assert!(!fixture.state().exists(), "nada lançado");
}
