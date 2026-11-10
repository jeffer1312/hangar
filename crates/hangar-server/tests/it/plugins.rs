//! Rotas `/api/plugins`: só o dono, uma mudança por vez, e a CLI termina mesmo com o cliente fora.
use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::{path::{Path, PathBuf}, sync::Arc, time::Duration};

/// `claude` falso: `list` devolve um plugin; as mudanças marcam o início, esperam `$HOME/release` quando há
/// `$HOME/hold` (teto de 20 s para não pendurar a suíte) e marcam o fim.
const FAKE_CLAUDE: &str = r#"#!/bin/sh
case "$2" in
  list) printf '%s\n' '[{"id":"caveman@caveman","version":"4.1.0","scope":"user","enabled":true,"lastUpdated":"2026-11-07T04:43:45.891Z","notes":[]},{"id":"caveman@caveman","version":"4.1.0","scope":"project","enabled":true}]' ;;
  *) touch "$HOME/started-$2"
     if [ -f "$HOME/hold" ]; then
       n=0
       while [ ! -f "$HOME/release" ] && [ "$n" -lt 400 ]; do sleep 0.05; n=$((n+1)); done
     fi
     touch "$HOME/finished-$2"
     printf '%s\n' "{\"command\":\"$2\",\"outcome\":\"ok\",\"message\":\"ok $2\"}" ;;
esac
"#;

struct Fixture {
    home: tempfile::TempDir,
    base: String,
    task: tokio::task::JoinHandle<()>,
}

impl Fixture {
    async fn start() -> Self {
        let home = tempfile::tempdir().unwrap();
        let bin = home.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        crate::write_executable(&bin.join("claude"), FAKE_CLAUDE);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let mut state = AppState::new(Config {
            listen: address, upstream: crate::refused_address(), internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
        });
        let home_text = home.path().to_string_lossy().into_owned();
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home_text.clone()),
            ("USERPROFILE".into(), home_text),
            ("PATH".into(), format!("{}:/usr/bin:/bin", bin.display())),
        ].into()));
        let router = hangar_server::routes::router(Arc::new(state));
        let task = tokio::spawn(async move {
            axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
        });
        Self { home, base: format!("http://{address}/api/plugins"), task }
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        reqwest::Client::new().request(method, format!("{}{path}", self.base)).bearer_auth("fixture-owner")
    }

    fn home(&self) -> &Path { self.home.path() }

    fn settings(&self) -> PathBuf { self.home().join(".claude/settings.json") }

    fn write_settings(&self, value: &Value) {
        std::fs::create_dir_all(self.settings().parent().unwrap()).unwrap();
        std::fs::write(self.settings(), value.to_string()).unwrap();
    }

    fn read_settings(&self) -> Value { serde_json::from_slice(&std::fs::read(self.settings()).unwrap()).unwrap() }
}

impl Drop for Fixture { fn drop(&mut self) { self.task.abort(); } }

/// Espera o marcador do `claude` falso; só falha no teto.
async fn wait_for(path: &Path) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(tokio::time::Instant::now() < deadline, "{} não apareceu", path.display());
        // estavel: intervalo do laço de espera pelo marcador, não espera fixa pelo resultado.
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn body(response: reqwest::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    let bytes = response.bytes().await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_else(|_| json!(String::from_utf8_lossy(&bytes))))
}

#[tokio::test]
async fn plugins_get_has_the_contract_shape_and_check_stamps_checked_at() {
    let fixture = Fixture::start().await;
    let (status, state) = body(fixture.request(reqwest::Method::GET, "").send().await.unwrap()).await;
    assert_eq!(status, 200, "{state}");
    assert_eq!(state["checked_at"], Value::Null);
    let plugins = state["plugins"].as_array().unwrap();
    assert_eq!(plugins.len(), 1, "escopo project fica de fora: {state}");
    assert_eq!(plugins[0], json!({
        "id": "caveman@caveman", "name": "caveman", "marketplace": "caveman", "version": "4.1.0",
        "last_updated": "2026-11-07T04:43:45.891Z", "enabled": true, "available_version": null,
        "update_known": false, "description": null, "notes": [],
    }));
    assert!(state["marketplaces"].as_array().unwrap().iter().any(|m| m["builtin"] == true));

    let (status, checked) = body(fixture.request(reqwest::Method::POST, "/check").send().await.unwrap()).await;
    assert_eq!(status, 200, "{checked}");
    let stamp = checked["checked_at"].as_str().expect("checked_at depois do check");
    assert!(chrono::DateTime::parse_from_rfc3339(stamp).is_ok() && stamp.ends_with('Z'), "{stamp}");
    assert!(fixture.home().join("finished-marketplace").exists());
    let (_, again) = body(fixture.request(reqwest::Method::GET, "").send().await.unwrap()).await;
    assert_eq!(again["checked_at"], checked["checked_at"]);
}

#[tokio::test]
async fn plugins_refuse_who_is_not_the_owner() {
    let fixture = Fixture::start().await;
    let client = reqwest::Client::new();
    for (method, path) in [(reqwest::Method::GET, ""), (reqwest::Method::POST, "/caveman@caveman/install"), (reqwest::Method::DELETE, "/caveman@caveman")] {
        let (status, value) = body(client.request(method, format!("{}{path}", fixture.base)).send().await.unwrap()).await;
        assert_eq!(status, 403, "{path}: {value}");
        assert_eq!(value["detail"]["code"], "plugins_owner_only");
    }
    assert!(!fixture.home().join("finished-install").exists());
}

#[tokio::test]
async fn plugins_run_one_change_at_a_time() {
    let fixture = Fixture::start().await;
    std::fs::write(fixture.home().join("hold"), "").unwrap();
    let first = tokio::spawn(fixture.request(reqwest::Method::POST, "/caveman@caveman/enable").send());
    wait_for(&fixture.home().join("started-enable")).await;
    let (second, second_body) = body(fixture.request(reqwest::Method::POST, "/caveman@caveman/disable").send().await.unwrap()).await;
    assert_eq!(second, 409, "{second_body}");
    assert_eq!(second_body["detail"]["code"], "plugins_busy");
    std::fs::write(fixture.home().join("release"), "").unwrap();
    let (first, first_body) = body(first.await.unwrap().unwrap()).await;
    assert_eq!(first, 200, "{first_body}");
    assert_eq!(first_body["result"]["outcome"], "ok");
    assert_eq!(first_body["plugins"][0]["id"], "caveman@caveman");
    assert!(!fixture.home().join("finished-disable").exists());
}

#[tokio::test]
async fn plugins_reject_option_like_input_before_the_cli() {
    let fixture = Fixture::start().await;
    for (method, path, code) in [
        (reqwest::Method::POST, "/-x@y/install", "plugins_invalid_id"),
        (reqwest::Method::GET, "/..@../details", "plugins_invalid_id"),
        (reqwest::Method::DELETE, "/marketplaces/--x", "plugins_invalid_name"),
        (reqwest::Method::POST, "/marketplaces/--x/update", "plugins_invalid_name"),
    ] {
        let (status, value) = body(fixture.request(method, path).send().await.unwrap()).await;
        assert_eq!(status, 422, "{path}: {value}");
        assert_eq!(value["detail"]["code"], code, "{path}");
    }
    let (status, value) = body(fixture.request(reqwest::Method::POST, "/marketplaces").json(&json!({"source": "--help"})).send().await.unwrap()).await;
    assert_eq!((status, value["detail"]["code"].clone()), (422, json!("plugins_invalid_source")), "{value}");
    let ran = std::fs::read_dir(fixture.home()).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("finished-"));
    assert!(!ran, "a CLI não pode rodar com entrada recusada");
}

#[tokio::test]
async fn plugins_change_survives_the_client_giving_up() {
    let fixture = Fixture::start().await;
    std::fs::write(fixture.home().join("hold"), "").unwrap();
    let client = tokio::spawn(fixture.request(reqwest::Method::POST, "/caveman@caveman/install").send());
    wait_for(&fixture.home().join("started-install")).await;
    // O cliente desiste com a CLI no meio: a conexão fecha.
    client.abort();
    assert!(client.await.unwrap_err().is_cancelled());
    std::fs::write(fixture.home().join("release"), "").unwrap();
    wait_for(&fixture.home().join("finished-install")).await;
}

#[tokio::test]
async fn plugins_uninstall_leaves_false_in_the_main_settings() {
    let fixture = Fixture::start().await;
    fixture.write_settings(&json!({"model": "opus", "enabledPlugins": {"caveman@caveman": true, "watch@claude-video": true}}));
    let (status, value) = body(fixture.request(reqwest::Method::DELETE, "/caveman@caveman").send().await.unwrap()).await;
    assert_eq!(status, 200, "{value}");
    assert_eq!(value["result"]["message"], "ok uninstall");
    assert_eq!(fixture.read_settings(), json!({"model": "opus", "enabledPlugins": {"caveman@caveman": false, "watch@claude-video": true}}));
}

#[tokio::test]
async fn plugins_uninstall_says_when_the_false_did_not_reach_the_settings() {
    let fixture = Fixture::start().await;
    let before = json!({"enabledPlugins": {"caveman@caveman": true}});
    fixture.write_settings(&before);
    // Outro escritor segura a trava do arquivo durante toda a remoção (ela só vence em 10 s).
    let mut lock = std::fs::canonicalize(fixture.settings()).unwrap().into_os_string();
    lock.push(".lock");
    std::fs::create_dir(&lock).unwrap();
    let (status, value) = body(fixture.request(reqwest::Method::DELETE, "/caveman@caveman").send().await.unwrap()).await;
    std::fs::remove_dir(&lock).unwrap();
    assert_eq!((status, value["detail"]["code"].clone()), (500, json!("plugins_settings_not_written")), "{value}");
    assert!(fixture.home().join("finished-uninstall").exists(), "a CLI já tinha removido");
    assert_eq!(fixture.read_settings(), before);
}

#[tokio::test]
async fn plugins_uninstall_refuses_unreadable_settings_before_the_cli() {
    let fixture = Fixture::start().await;
    // Uma pasta no lugar do arquivo: a leitura falha sem ser "ausente".
    std::fs::create_dir_all(fixture.settings()).unwrap();
    let (status, value) = body(fixture.request(reqwest::Method::DELETE, "/caveman@caveman").send().await.unwrap()).await;
    assert_eq!((status, value["detail"]["code"].clone()), (500, json!("plugins_settings_unreadable")), "{value}");
    assert!(!fixture.home().join("started-uninstall").exists(), "a CLI não pode rodar");
}

#[tokio::test]
async fn plugins_auto_update_changes_only_its_key() {
    let fixture = Fixture::start().await;
    let declared = json!({"model": "opus", "extraKnownMarketplaces": {
        "caveman": {"source": {"source": "github", "repo": "JuliusBrussee/caveman"}, "autoUpdate": true},
    }});
    fixture.write_settings(&declared);
    let (status, value) = body(fixture.request(reqwest::Method::PUT, "/marketplaces/caveman/auto-update").json(&json!({"enabled": false})).send().await.unwrap()).await;
    assert_eq!(status, 200, "{value}");
    let mut expected = declared.clone();
    expected["extraKnownMarketplaces"]["caveman"]["autoUpdate"] = json!(false);
    assert_eq!(fixture.read_settings(), expected);

    let (status, value) = body(fixture.request(reqwest::Method::PUT, "/marketplaces/other/auto-update").json(&json!({"enabled": false})).send().await.unwrap()).await;
    assert_eq!((status, value["detail"]["code"].clone()), (409, json!("plugins_auto_update_unavailable")), "{value}");
    assert_eq!(fixture.read_settings(), expected);
}

#[test]
fn plugins_migration_inventory_covers_every_route() {
    for path in [
        "/api/plugins",
        "/api/plugins/check",
        "/api/plugins/available",
        "/api/plugins/marketplaces",
        "/api/plugins/caveman@caveman",
        "/api/plugins/caveman@caveman/details",
        "/api/plugins/caveman@caveman/install",
        "/api/plugins/caveman@caveman/update",
        "/api/plugins/caveman@caveman/enable",
        "/api/plugins/caveman@caveman/disable",
        "/api/plugins/marketplaces/caveman",
        "/api/plugins/marketplaces/caveman/update",
        "/api/plugins/marketplaces/caveman/auto-update",
    ] {
        assert!(hangar_server::plugins::matches(path), "{path}");
    }
    assert!(!hangar_server::plugins::matches("/api/pluginsx"));
}
