use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},
    routes::AppState};
use serde_json::Value;

#[tokio::test]
#[ignore]
async fn runtime_boot_child() {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let backend = home.join(".hangar/current/backend");
    std::fs::create_dir_all(&backend).unwrap();
    std::fs::write(backend.join(".env"),"CP_EDITOR=antigo\n").unwrap();
    let dotenv = home.join(".hangar/config/.env");
    std::fs::create_dir_all(dotenv.parent().unwrap()).unwrap();
    std::fs::write(&dotenv,"CP_EDITOR=vim\nCP_SCAN_ROOTS=\n").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut config = crate::fake::config(crate::refused_address(), "");
    config.listen = address;
    let mut state = AppState::new(config);
    state.accounts = AccountService::new(AccountEnvironment::from_map([
        ("HOME".into(),home.to_string_lossy().into_owned()),
        ("USERPROFILE".into(),home.to_string_lossy().into_owned()),
        ("PATH".into(),String::new()),
    ].into()));
    state.share = None;
    let token = state.cfg.auth_token.clone();
    let held_version = if std::env::var_os("HANGAR_BOOT_HOLD_VERSION").is_some() {
        let (entered, waiting) = std::sync::mpsc::sync_channel(1);
        let (release, released) = std::sync::mpsc::sync_channel(1);
        let root = home.join(".hangar/current");
        let worker = std::thread::spawn(move || {
            let mut first = true;
            hangar_server::boot_version::capture_with(&root, |arguments, _| {
                if first {
                    first = false;
                    entered.send(()).unwrap();
                    let _ = released.recv();
                }
                Ok(match arguments[0] {"rev-list"=>"1", "show"=>"1.0", _=>"fixture-running"}.into())
            })
        });
        waiting.recv().unwrap();
        Some((release,worker))
    } else {None};
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(hangar_server::serve_until_with_state(listener,state,async {let _=stopped.await;}));
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build().unwrap();
    if let Some((release,worker)) = held_version {
        let health = client.get(format!("http://{address}/__hangar_server/health")).send().await;
        let _ = release.send(());
        worker.join().unwrap();
        assert_eq!(health.unwrap().status(),200,"saúde não espera o git da versão");
    }
    let first = client.get(format!("http://{address}/api/config")).bearer_auth(&token).send().await.unwrap();
    let status = first.status();
    let first: Value = first.json().await.unwrap();
    if status != 200 {
        let _=stop.send(());task.await.unwrap().unwrap();
        panic!("boot deveria preparar configuração e versão: {status} {first}");
    }
    std::fs::write(&dotenv,"CP_EDITOR=alterado\n").unwrap();
    let second: Value = client.get(format!("http://{address}/api/config")).bearer_auth(&token)
        .send().await.unwrap().json().await.unwrap();
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/runtime-config.json"),"{\"editor\":\"helix\"}").unwrap();
    let third: Value = client.get(format!("http://{address}/api/config")).bearer_auth(&token)
        .send().await.unwrap().json().await.unwrap();
    let _=stop.send(());task.await.unwrap().unwrap();
    assert_eq!(first["campos"]["editor"]["valor"],"vim");
    assert_eq!(second["campos"]["editor"]["valor"],"vim","Settings fica congelado");
    assert_eq!(third["campos"]["editor"]["valor"],"helix","override continua vivo");
    assert_eq!(first["somente_leitura"]["versao"],second["somente_leitura"]["versao"]);
}

#[test]
fn runtime_boot_initializes_version_and_settings_before_public_configuration() {
    probe(false);
}

#[test]
fn runtime_boot_health_remains_available_during_version_capture() {
    probe(true);
}

fn probe(hold_version:bool) {
    let home = tempfile::tempdir().unwrap();
    let mut command=std::process::Command::new(std::env::current_exe().unwrap());
    command.args(["--exact","runtime_boot::runtime_boot_child","--ignored","--nocapture"])
        .env_clear().env("HOME",home.path()).env("USERPROFILE",home.path())
        .env("PATH","").current_dir(home.path());
    if hold_version {command.env("HANGAR_BOOT_HOLD_VERSION","1");}
    let output=command.output().unwrap();
    assert!(output.status.success(),"{}\n{}",String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
}
