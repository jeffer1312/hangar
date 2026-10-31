//! Pares de máquina, com disco e portas próprios.
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use serde_json::{Value, json};
use std::sync::Arc;

#[tokio::test]
async fn peers_route_is_owned_without_python() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".hangar/config")).unwrap();
    std::fs::write(home.path().join(".hangar/config/peers.json"), "{}").unwrap();
    std::fs::write(home.path().join(".hangar/config/.env"), "").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "contract-internal".into(),
        auth_token: "contract-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().display().to_string()),
            ("USERPROFILE".into(), home.path().display().to_string()),
        ]
        .into(),
    ));
    let router = hangar_server::routes::router(Arc::new(state));
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/api/peers"))
        .bearer_auth("contract-owner")
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let body: Value =
        serde_json::from_slice(&response.bytes().await.unwrap()).unwrap_or(Value::Null);
    task.abort();
    assert_eq!(
        (status, body),
        (200, json!([])),
        "PEE-001/002: a lista vazia é do Rust, mesmo sem Python"
    );
}

#[test]
fn peer_writers_preserve_each_other_and_refusals_preserve_bytes() {
    use hangar_server::machine_peers::store::Store;
    let home = tempfile::tempdir().unwrap();
    let path = home.path().join("peers.json");
    std::thread::scope(|scope| {
        for n in 0..20 {
            let path = path.clone();
            scope.spawn(move ||{Store::new(path).put(&json!({"id":format!("m{n}"),"base_url":"https://peer.invalid","token":"contract"})).unwrap();});
        }
    });
    let store = Store::new(path.clone());
    assert_eq!(store.list().as_array().unwrap().len(), 20);
    let before = std::fs::read(&path).unwrap();
    assert!(store.put(&json!({"id":"BAD"})).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    std::fs::write(&path, b"{broken").unwrap();
    assert!(store.app_list().is_err());
    assert!(
        store
            .put(&json!({"id":"valid","base_url":"https://p","token":"t"}))
            .is_err()
    );
    assert_eq!(std::fs::read(&path).unwrap(), b"{broken");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

#[test]
fn origin_respects_explicit_ports_case_and_disabled_peers() {
    use hangar_server::machine_peers::origin_allowed;
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".hangar/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("peers.json"),
        r#"{"a":{"base_url":"https://UPPER:443","enabled":false}}"#,
    )
    .unwrap();
    assert!(origin_allowed(
        home.path(),
        None,
        "https://UPPER:443",
        None,
        "",
        &[]
    ));
    assert!(!origin_allowed(
        home.path(),
        None,
        "https://upper",
        None,
        "",
        &[]
    ));
    assert!(origin_allowed(
        home.path(),
        None,
        "http://x:80",
        Some(" x:80 "),
        "",
        &[]
    ));
    assert!(!origin_allowed(
        home.path(),
        None,
        "http://x",
        Some("x:80"),
        "",
        &[]
    ));
    assert!(origin_allowed(
        home.path(),
        None,
        "https://extra:9",
        None,
        "",
        &["extra:9".into()]
    ));
}

proptest::proptest! {
    #[test]
    fn peer_id_validation_matches_public_grammar(value in ".{0,100}") {
        let expected=!value.is_empty()&&value.len()<=32&&value.bytes().next().is_some_and(|c|c.is_ascii_lowercase()||c.is_ascii_digit())&&value.bytes().all(|c|c.is_ascii_lowercase()||c.is_ascii_digit()||c==b'-'||c==b'_');
        proptest::prop_assert_eq!(hangar_server::machine_peers::store::valid_id(&json!(value)).is_ok(),expected);
    }
    #[test]
    fn arbitrary_origin_without_authority_never_authorizes(value in "[a-zA-Z0-9._ -]{0,80}") {
        let home=tempfile::tempdir().unwrap();
        proptest::prop_assert!(!hangar_server::machine_peers::origin_allowed(home.path(),None,&value,Some(&value),"",&[]));
    }
}

#[test]
fn doctor_qr_cli_operates_without_server_or_python() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".hangar/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("peers.json"), "{}").unwrap();
    std::fs::write(
        config.join(".env"),
        "CP_AUTH_TOKEN=contract-owner\nCP_PUBLIC_URL=https://fixture.invalid\n",
    )
    .unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"))
        .args(["doctor", "--qr"])
        .env_clear()
        .env("HOME", home.path())
        .env("USERPROFILE", home.path())
        .env("PATH", "")
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8(out.stdout).unwrap(),
        "  Abra no celular: https://fixture.invalid/?token=contract-owner\n\n"
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn pairing_svg_decodes_the_exact_url() {
    for url in [
        "https://host.invalid/?token=contract-owner",
        "http://192.0.2.1:5173/?token=á&b=sim",
    ] {
        let svg = hangar_server::machine_peers::network::qr_svg(url).unwrap();
        if let Ok(path) = std::env::var("HANGAR_QR_PROOF") { std::fs::write(path, &svg).unwrap(); }
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        let size = regex::Regex::new(r#"viewBox="0 0 (\d+) (\d+)""#)
            .unwrap()
            .captures(&svg)
            .unwrap();
        let width = size[1].parse::<usize>().unwrap();
        assert_eq!(size[1], size[2]);
        let mut pixels = std::collections::HashSet::new();
        for coordinate in regex::Regex::new(r"M(\d+),(\d+)h1v1h-1z")
            .unwrap()
            .captures_iter(&svg)
        {
            pixels.insert((
                coordinate[1].parse::<usize>().unwrap() - 1,
                coordinate[2].parse::<usize>().unwrap() - 1,
            ));
        }
        let grid = rqrr::Grid::new(rqrr::SimpleGrid::from_func(width - 2, |x, y| {
            pixels.contains(&(x, y))
        }));
        assert_eq!(grid.decode().unwrap().1, url);
    }
}

#[tokio::test]
async fn real_peer_checks_classify_http_and_drop_auth_on_redirect() {
    use axum::{
        Router,
        http::{HeaderMap, StatusCode},
        routing::get,
    };
    use hangar_server::machine_peers::{Service, network};
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".hangar/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join(".env"), "").unwrap();
    std::fs::write(
        config.join("peers.json"),
        r#"{"remote":{"base_url":"https://registered.invalid","token":"peer-only-secret"}}"#,
    )
    .unwrap();
    let service = Service::new(
        home.path(),
        None,
        "contract-owner".into(),
        8765,
        "127.0.0.1".into(),
        "fixture".into(),
    )
    .unwrap();
    let state = Arc::new(std::sync::Mutex::new((
        200_u16,
        json!({"identificador":"remote"}),
        Vec::<Option<String>>::new(),
    )));
    let handler_state = state.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = Router::new().route(
        "/api/peers/identificador",
        get(move |headers: HeaderMap| {
            let state = handler_state.clone();
            async move {
                let mut state = state.lock().unwrap();
                state.2.push(
                    headers
                        .get("authorization")
                        .and_then(|h| h.to_str().ok())
                        .map(str::to_owned),
                );
                (StatusCode::from_u16(state.0).unwrap(), state.1.to_string())
            }
        }),
    );
    let router = router.route("/same/api/peers/identificador", get(|| async { (StatusCode::FOUND, [("location", "/api/peers/identificador")]) }));
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    for (status, body, expected) in [
        (200, json!({"identificador":"remote"}), "ok"),
        (401, json!({"detail":"recusado"}), "recusou"),
        (500, json!({"identificador":"remote"}), "ok"),
        (200, json!([]), "estranho"),
        (404, json!({"identificador":"other"}), "estranho"),
    ] {
        {
            let mut s = state.lock().unwrap();
            s.0 = status;
            s.1 = body;
        }
        let response = network::check(&service, &format!("http://{address}"), "remote").await;
        assert_eq!(response["estado"], expected);
        assert!(response["tempo_ms"].is_number());
    }
    assert!(
        state
            .lock()
            .unwrap()
            .2
            .iter()
            .all(|a| a.as_deref() == Some("Bearer peer-only-secret"))
    );
    {
        let mut s = state.lock().unwrap();
        s.0 = 200;
        s.1 = json!({"identificador":"remote"});
        s.2.clear();
    }
    let redirect_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let redirect_address = redirect_listener.local_addr().unwrap();
    let destination = format!("http://{address}/api/peers/identificador");
    let redirect = Router::new().route(
        "/api/peers/identificador",
        get(move || {
            let destination = destination.clone();
            async move { (StatusCode::FOUND, [("location", destination)]) }
        }),
    );
    let redirect_task =
        tokio::spawn(async move { axum::serve(redirect_listener, redirect).await.unwrap() });
    assert_eq!(
        network::check(&service, &format!("http://{redirect_address}"), "remote").await["estado"],
        "ok"
    );
    assert_eq!(
        state.lock().unwrap().2,
        vec![None],
        "D04: token do peer não atravessa origem num redirect"
    );
    state.lock().unwrap().2.clear();
    assert_eq!(network::check(&service, &format!("http://{address}/same"), "remote").await["estado"], "ok");
    assert_eq!(state.lock().unwrap().2, vec![Some("Bearer peer-only-secret".into())], "redirect na mesma origem conserva a credencial");
    task.abort();
    redirect_task.abort();
    let dead = crate::refused_address();
    let response = network::check(&service, &format!("http://{dead}"), "remote").await;
    assert_eq!(response["estado"], "falhou");
    assert_eq!(response["motivo"], "recusou");
}

#[test]
fn peers_cli_reads_disabled_explicitly_and_filters_enumeration_offline() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join(".hangar/config");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("peers.json"),r#"{"remote":{"base_url":"https://fixture.invalid/","token":"synthetic","enabled":false},"enabled":{"base_url":"https://enabled.invalid/","token":"second"},"app":{"app":{"id":"only"}}}"#).unwrap();
    let run = |args: &[&str]| {
        std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"))
            .args(args)
            .env_clear()
            .env("HOME", home.path())
            .env("USERPROFILE", home.path())
            .env("PATH", "")
            .output()
            .unwrap()
    };
    let cfg = run(&["peers", "config", "remote"]);
    assert!(cfg.status.success());
    assert_eq!(cfg.stdout, b"https://fixture.invalid\tsynthetic\n");
    let list = run(&["peers", "tsv"]);
    assert!(list.status.success());
    assert_eq!(list.stdout, b"enabled\thttps://enabled.invalid\tsecond\n");
    assert_eq!(
        String::from_utf8(list.stderr).unwrap(),
        "(remote desativado no peers.json — enderece direto se precisar)\n"
    );
    let missing = run(&["peers", "config", "missing"]);
    assert_eq!(missing.status.code(), Some(1));
    assert!(missing.stdout.is_empty());
}

#[tokio::test]
async fn owner_routes_keep_failed_attempts_and_block_correct_token() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".hangar/config")).unwrap();
    std::fs::write(home.path().join(".hangar/config/.env"), "").unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config { listen: address, upstream: crate::refused_address(), internal_secret: "fixture".into(), auth_token: "owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
    state.accounts = AccountService::new(AccountEnvironment::from_map([("HOME".into(),home.path().display().to_string()),("USERPROFILE".into(),home.path().display().to_string())].into()));
    let router=hangar_server::routes::router(Arc::new(state));
    let task=tokio::spawn(async move { axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
    let client=reqwest::Client::new();
    for _ in 0..8 { assert_eq!(client.get(format!("http://{address}/api/peers")).header("X-Forwarded-For","192.0.2.15").send().await.unwrap().status(),401); }
    let response=client.get(format!("http://{address}/api/peers")).header("X-Forwarded-For","192.0.2.15").bearer_auth("owner").send().await.unwrap();
    task.abort();
    assert_eq!(response.status(),429,"a origem bloqueada não testa nem o token correto");
    assert_eq!(response.headers()["Retry-After"],"30");
}

async fn child_identity(home: &std::path::Path, inherited: Option<&str>) -> Value {
    use std::process::{Command,Stdio};
    let listener=std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address=listener.local_addr().unwrap();drop(listener);
    let mut command=Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    command.env_clear().env("HOME",home).env("USERPROFILE",home).env("PATH",home)
        .env("HANGAR_SERVER_ID","before").env("HANGAR_SERVER_LISTEN",address.to_string())
        .env("HANGAR_SERVER_UPSTREAM",crate::refused_address().to_string()).env("HANGAR_INTERNAL_SECRET","proof").env("CP_AUTH_TOKEN","proof")
        .current_dir(home).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    if cfg!(windows) {
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) { command.env(key, value); }
        }
    }
    if let Some(value)=inherited { command.env("CP_SERVER_ID",value); }
    struct Child(std::process::Child);
    impl Drop for Child { fn drop(&mut self) { let _=self.0.kill(); let _=self.0.wait(); } }
    let _child=Child(command.spawn().unwrap());
    let client=reqwest::Client::new();
    let deadline=std::time::Instant::now()+std::time::Duration::from_secs(15);
    while std::time::Instant::now()<deadline {
        if let Ok(response)=client.get(format!("http://{address}/api/peers/identificador")).bearer_auth("proof").send().await {
            let status=response.status();
            let body:Value=response.json().await.unwrap();
            if status.is_success() {return body;}
            assert_eq!(status,503,"resposta inesperada: {body}");
            assert_eq!(body,json!({"detail":"server_starting"}));
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("filho de prova não abriu a porta temporária");
}

#[tokio::test]
async fn child_restart_reads_the_persisted_identity_without_python_snapshot() {
    let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");
    std::fs::create_dir_all(&config).unwrap();std::fs::write(config.join("peers.json"),"{}").unwrap();std::fs::write(config.join(".env"),"CP_SERVER_ID=after\n").unwrap();
    assert_eq!(child_identity(home.path(),None).await["identificador"],"after");
    let mut service=hangar_server::machine_peers::Service::new(home.path(),None,"proof".into(),8765,"127.0.0.1".into(),"before".into()).unwrap();
    service.environment=[("CP_SERVER_ID".into(),"before".into())].into();
    service.save_identity(&json!({"identificador":"published"})).unwrap();
    assert_eq!(child_identity(home.path(),Some("before")).await["identificador"],"published");
    assert_eq!(child_identity(home.path(),Some("manual")).await["identificador"],"manual");
}

#[test]
fn peer_settings_snapshot_are_frozen_before_later_dotenv_edits() {
    let home=tempfile::tempdir().unwrap();let config=home.path().join(".hangar/config");std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("peers.json"),"{}").unwrap();
    std::fs::write(config.join(".env"),"CP_PUBLIC_URL=https://before.invalid\n").unwrap();
    let mut state=AppState::new(Config{listen:"127.0.0.1:0".parse().unwrap(),upstream:crate::refused_address(),internal_secret:"contract-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
    let state=Arc::new(state);
    assert_eq!(hangar_server::machine_peers::Service::from_state(&state).unwrap().value("CP_PUBLIC_URL"),"https://before.invalid");
    std::fs::write(config.join(".env"),"CP_PUBLIC_URL=https://after.invalid\n").unwrap();
    assert_eq!(hangar_server::machine_peers::Service::from_state(&state).unwrap().value("CP_PUBLIC_URL"),"https://before.invalid");
}
