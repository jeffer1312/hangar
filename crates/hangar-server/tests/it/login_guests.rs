//! Contratos de convidados.
use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::Value;
use std::sync::Arc;

#[test]
fn memory_rules_out_tokens_while_the_file_stays_the_same() {
    use hangar_server::login_guests::store::{Store, hash};
    let home = crate::physical_tempdir();
    let path = home.path().join("guests.json");
    let store = Store::new(path.clone());
    let save = |name: &str, token: &[u8]| std::fs::write(&path, serde_json::json!({"guests": [{"id": "ana", "name": name, "root": "/tmp",
        "token_hash": hash(token)}], "sessions": {}}).to_string()).unwrap();

    assert!(store.rules_out(b"palpite"), "sem arquivo não há convidado");
    save("Ana", b"guest");
    assert!(!store.rules_out(b"palpite"), "arquivo ainda não lido vai ao `lookup`");
    assert!(store.lookup(b"palpite").is_none());
    assert!(store.rules_out(b"palpite"), "lido, o palpite para na memória");
    assert!(!store.rules_out(b"guest"));

    save("Ana Maria", b"outro-token-de-convidado");
    assert!(!store.rules_out(b"palpite"), "arquivo mudou: a memória não decide");
    assert!(store.lookup(b"outro-token-de-convidado").is_some());
    assert!(store.rules_out(b"guest"), "token trocado sai junto");
}

#[tokio::test]
async fn login_guests_public_gate_denies_owner_routes_before_upstream() {
    use hangar_server::login_guests::{Service, Settings};
    let home = crate::physical_tempdir();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config { listen:address,
        internal_secret:"fixture".into(), auth_token:"owner".into(), log_path:None,
        trusted:TrustedHosts::parse("127.0.0.1") });
    let service = Service::new(Settings { guests:home.path().join("guests.json"), vault:home.path().join("vault.json"),
        runtime_config:home.path().join("runtime-config.json"), sync_enabled:false, bootstrap:String::new(),
        secret:b"fixture".to_vec(), secure:false, rate_max:10.into(), rate_window:300.into() });
    service.store.create("Ana", home.path().to_str().unwrap(), false, true, "ana".into(), "guest".into()).unwrap();
    state.share = None;
    state.login_guests = Arc::new(service);
    let task=tokio::spawn(async move { axum::serve(listener,hangar_server::routes::router(Arc::new(state))
        .into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
    let client=reqwest::Client::new();
    for path in ["/api/config", "/api/voice", "/api/peers", "/api/sessions/owner/share", "/api/push/vapid", "/api/push/settings", "/api/push/subscribe", "/api/push/mute", "/api/push/quiet-hours", "/api/push/expo/subscribe"] {
        let response=client.get(format!("http://{address}{path}")).bearer_auth("guest").send().await.unwrap();
        assert_eq!(response.status().as_u16(),403,"GUE-030/032: {path}");
        let body:Value=response.json().await.unwrap();
        assert_eq!(body["detail"]["code"],"erro_fora_do_convidado");
    }
    task.abort();
}

/// Guarda (achado 3 da revisão da R2): convidado com login em escrita de sessão fora do alcance dele
/// recebe o 403 e não perde o login; o Python (fora do ar aqui) não é procurado. Já valia antes da R2,
/// pelo porteiro do login (`login_guests/gate.rs` `check` → `deny`), que a remoção não pode perder.
#[tokio::test]
async fn login_guest_write_outside_its_reach_is_the_rust_403() {
    use hangar_server::login_guests::{Service, Settings};
    let home = crate::physical_tempdir();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config { listen:address,
        internal_secret:"fixture".into(), auth_token:"owner".into(), log_path:None,
        trusted:TrustedHosts::parse("127.0.0.1") });
    let service = Service::new(Settings { guests:home.path().join("guests.json"), vault:home.path().join("vault.json"),
        runtime_config:home.path().join("runtime-config.json"), sync_enabled:false, bootstrap:String::new(),
        secret:b"fixture".to_vec(), secure:false, rate_max:10.into(), rate_window:300.into() });
    service.store.create("Ana", home.path().to_str().unwrap(), false, true, "ana".into(), "guest".into()).unwrap();
    state.share = None;
    state.login_guests = Arc::new(service);
    let task=tokio::spawn(async move { axum::serve(listener,hangar_server::routes::router(Arc::new(state))
        .into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
    let client=reqwest::Client::new();
    for route in ["keys", "term-input", "select", "select/submit", "answer", "interrupt", "steer", "model", "codex/mode", "question/skip", "codex/plan/implement"] {
        let response=client.post(format!("http://{address}/api/sessions/do-dono/{route}")).bearer_auth("guest")
            .header("content-type","application/json").body("{}").send().await.unwrap();
        assert_eq!(response.status().as_u16(),403,"{route}");
        let body:Value=response.json().await.unwrap();
        assert_eq!(body["detail"]["code"],"erro_fora_do_convidado","{route}");
    }
    for route in ["models", "limits", "commands", "codex-permissions"] {
        let response=client.get(format!("http://{address}/api/sessions/do-dono/{route}")).bearer_auth("guest").send().await.unwrap();
        assert_eq!(response.status().as_u16(),403,"{route}");
    }
    task.abort();
}

/// W-09 do convidado com login: o porteiro dele não deixa o `whoami` passar (fora do alcance), antes da rota.
#[tokio::test]
async fn login_guest_whoami_is_the_rust_403() {
    use hangar_server::login_guests::{Service, Settings};
    let home = crate::physical_tempdir();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config { listen:address,
        internal_secret:"fixture".into(), auth_token:"owner".into(), log_path:None,
        trusted:TrustedHosts::parse("127.0.0.1") });
    let service = Service::new(Settings { guests:home.path().join("guests.json"), vault:home.path().join("vault.json"),
        runtime_config:home.path().join("runtime-config.json"), sync_enabled:false, bootstrap:String::new(),
        secret:b"fixture".to_vec(), secure:false, rate_max:10.into(), rate_window:300.into() });
    service.store.create("Ana", home.path().to_str().unwrap(), false, true, "ana".into(), "guest".into()).unwrap();
    state.share = None;
    state.login_guests = Arc::new(service);
    let task=tokio::spawn(async move { axum::serve(listener,hangar_server::routes::router(Arc::new(state))
        .into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
    let response=reqwest::Client::new().get(format!("http://{address}/api/whoami")).bearer_auth("guest")
        .header("x-hangar-session","ana").send().await.unwrap();
    assert_eq!(response.status().as_u16(),403);
    let body:Value=response.json().await.unwrap();
    assert_eq!(body,serde_json::json!({"detail":{"code":"erro_fora_do_convidado","params":{},"msg":"fora do acesso do convidado"}}));
    task.abort();
}

mod arbitrary_inputs {
    use super::*;
    use proptest::prelude::*;
    fn document() -> impl Strategy<Value=Value> {
        prop_oneof![Just(Value::Null),any::<bool>().prop_map(Value::Bool),any::<i64>().prop_map(|v|serde_json::json!(v)),".{0,80}".prop_map(Value::String)]
            .prop_recursive(3,32,8,|inner|prop_oneof![proptest::collection::vec(inner.clone(),0..8).prop_map(Value::Array),
                proptest::collection::vec((prop_oneof![Just("guests".to_owned()),Just("sessions".to_owned()),Just("root".to_owned()),Just("name".to_owned()),Just("user".to_owned()),Just("base_rev".to_owned()),".{0,16}"],inner),0..8).prop_map(|fields|Value::Object(fields.into_iter().collect()))])
    }
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]
        #[test]
        fn login_guests_external_payloads_and_sidecars_do_not_panic(value in document(), raw in proptest::collection::vec(any::<u8>(),0..256)) {
            use hangar_server::login_guests::{Service,Settings};
            let home=crate::physical_tempdir();
            let service=Arc::new(Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:String::new(),secret:vec![1],secure:false,rate_max:10.into(),rate_window:300.into()}));
            let rt=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
            rt.block_on(async {
                for (path,bytes) in [("/api/guests",value.to_string().into_bytes()),("/api/sync/setup",value.to_string().into_bytes()),("/api/sync/login",raw)] {
                    let request=axum::http::Request::builder().method("POST").uri(path).header("content-type","application/json").body(axum::body::Body::from(bytes)).unwrap();
                    let response=service.respond(true,"127.0.0.1".into(),false,request).await;
                    let body=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
                    let body:Value=serde_json::from_slice(&body).unwrap();
                    assert_ne!(body["detail"]["code"],"guests_worker_failed");
                }
            });
            std::fs::write(home.path().join("guests.json"),value.to_string()).unwrap();
            let _=service.store.lookup(b"untrusted");
            let _=service.store.visible(None,"untrusted",None);
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn login_guests_terminal_closes_after_login_revocation() {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux},login_guests::{Service,Settings},term::{Terms,TermConfig}};
    use serde_json::json;
    use std::time::Duration;
    use futures_util::{SinkExt,StreamExt};
    use tokio_tungstenite::tungstenite::{Message};
    let root=crate::physical_tempdir();let program=crate::list_support::sessions(root.path(),1);
    let script=std::fs::read_to_string(&program).unwrap().replace("[ \"$1\" = list-panes ]", "[ \"$1\" = attach ] && exec cat\n[ \"$1\" = list-panes ]");
    std::fs::write(&program,script).unwrap();
    let home=root.path().join("home");let claude=home.join(".claude");
    std::fs::create_dir_all(home.join(".hangar/current")).unwrap();std::fs::create_dir_all(home.join(".hangar/config")).unwrap();
    std::fs::write(home.join(".hangar/config/.env"),"CP_PUBLIC_URL=https://public.example:8443\nCP_TERM_ORIGINS=https://extra.example\n").unwrap();
    std::fs::write(home.join(".hangar/config/peers.json"),r#"{"peers":{"peer":{"base_url":"https://peer.example:8443","enabled":false}}}"#).unwrap();
    let mut state=AppState::new(crate::fake::config(""));
    state.accounts=AccountService::new(AccountEnvironment::from_map([("HOME".into(),home.to_string_lossy().into_owned()),("USERPROFILE".into(),home.to_string_lossy().into_owned())].into()));
    state.term=Arc::new(Terms::new(TermConfig{program:program.clone().into_os_string(),..TermConfig::default()}));
    state.list=Arc::new(ListBridge::new(ListEnv{mux:Mux::with_program(&program,Duration::from_secs(5)),capture_program:program.into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs:parse_dirs(&json!({"home":home,"claude":claude,"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string())},FactsClient::new()));
    let service=Service::new(Settings{guests:claude.join("guests.json"),vault:home.join("vault.json"),runtime_config:claude.join("runtime-config.json"),sync_enabled:false,bootstrap:String::new(),secret:vec![1],secure:false,rate_max:10.into(),rate_window:300.into()});
    service.store.create("Ana",home.to_str().unwrap(),true,true,"fixture".into(),"guest-token".into()).unwrap();state.share = None;
    state.login_guests=Arc::new(service);
    let service=state.login_guests.clone();let terms=state.term.clone();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
    let (mut socket,response)=tokio_tungstenite::connect_async(format!("ws://{address}/api/sessions/s0/term?token=guest-token")).await.unwrap();
    assert_eq!(response.status(),101);
    socket.send(Message::Binary(b"fixture\n".to_vec().into())).await.unwrap();
    let alive=tokio::time::timeout(Duration::from_secs(3),socket.next()).await.ok().flatten();
    assert!(matches!(alive,Some(Ok(Message::Binary(_)))|Some(Ok(Message::Text(_)))),"GUE-034: PTY falso está vivo antes de revogar");
    service.store.delete("fixture").unwrap();
    let code=tokio::time::timeout(Duration::from_secs(7),async {
        while let Some(Ok(message))=socket.next().await {if let Message::Close(frame)=message{return frame.map(|f|u16::from(f.code));}}
        None
    }).await.ok().flatten();
    let _=socket.close(None).await;
    let _=tokio::time::timeout(Duration::from_secs(5),async{while terms.is_active("s0"){tokio::time::sleep(Duration::from_millis(20)).await;}}).await;
    task.abort();
    assert_eq!(code,Some(4410),"GUE-034: revogação encerra o terminal com 4410");
}

async fn sync_request(service: &Arc<hangar_server::login_guests::Service>, method: &str, path: &str, raw: &str, cookie: Option<&str>) -> (u16, Value, Option<String>) {
    let mut request = axum::http::Request::builder().method(method).uri(path).header("content-type", "application/json");
    if let Some(cookie) = cookie { request = request.header("cookie", cookie); }
    let response = service.respond(true, "127.0.0.1".into(), false, request.body(axum::body::Body::from(raw.to_owned())).unwrap()).await;
    let status = response.status().as_u16();
    let cookie = response.headers().get("set-cookie").map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let value = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8(bytes.to_vec()).unwrap()));
    (status, value, cookie)
}

#[tokio::test]
async fn cookie_context_child_helper() {
    let Ok(mode)=std::env::var("HANGAR_GUEST_COOKIE_PROBE") else{return;};
    let root=std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    let state=AppState::new(crate::fake::config(""));
    let mut service=hangar_server::login_guests::Service::from_state(&state).await;
    service.now=||2000000000;service.entropy=|n|vec![b'x';n];let service=Arc::new(service);
    let result=if mode=="issue" {
        let _=std::fs::remove_file(root.join("vault.json"));
        let registered=sync_request(&service,"POST","/api/sync/register",r#"{"user":"ana","salt":"fixture","auth_hash":"fixture","bootstrap":"bootstrap"}"#,None).await;
        assert_eq!(registered.0,200);
        let response=sync_request(&service,"POST","/api/sync/login",r#"{"user":"ana","auth_hash":"fixture"}"#,None).await;
        if let Some(cookie)=&response.2 {std::fs::write(root.join("cookie.txt"),cookie.split(';').next().unwrap()).unwrap();}
        response
    }else{
        let cookie=std::fs::read_to_string(root.join("cookie.txt")).unwrap();
        sync_request(&service,"GET","/api/sync/vault","",Some(&cookie)).await
    };
    std::fs::write(root.join("result.json"),serde_json::json!({"status":result.0,"body":result.1}).to_string()).unwrap();
}

#[tokio::test]
async fn login_guests_cookie_survives_child_relaunch_and_follows_parent_context() {
    use serde_json::json;
    let root=crate::physical_tempdir();std::fs::create_dir(root.path().join("claude")).unwrap();
    fn context(root:&std::path::Path,file:&str,epoch:&str,key:&str)->Value {
        let mut config=json!({"abi":hangar_auth::AUTH_ABI,"store_version":hangar_auth::AUTH_STORE,"mode":"create","state_path":root.join(file),"parent_epoch":epoch.repeat(16),"expected_token":"owner-fake","receipt_key":key.repeat(32)});
        hangar_auth::SharedLimiter::open(serde_json::from_value(config.clone()).unwrap()).unwrap();
        config["mode"]=json!("attach");config
    }
    async fn child(root:&std::path::Path,config:Option<&Value>,mode:&str,secret:Option<&str>)->Value {
        let mut command=tokio::process::Command::new(std::env::current_exe().unwrap());
        command.args(["--exact","login_guests::cookie_context_child_helper","--nocapture"])
            .env_clear().env("HOME",root).env("USERPROFILE",root).env("CLAUDE_CONFIG_DIR",root.join("claude"))
            .env("CP_AUTH_TOKEN","owner-fake").env("CP_SYNC","true").env("CP_SYNC_BOOTSTRAP","bootstrap")
            .env("CP_SYNC_DATA",root.join("vault.json")).env("CP_LAN_BIND_IP","127.0.0.1")
            .env("HANGAR_GUEST_COOKIE_PROBE",mode).kill_on_drop(true);
        // O runtime de rede do Windows precisa localizar as bibliotecas do sistema.
        #[cfg(windows)]
        if let Some(system_root)=std::env::var_os("SystemRoot") { command.env("SystemRoot",system_root); }
        if let Some(config)=config{command.env("HANGAR_AUTH_CONTEXT",config.to_string());}
        if let Some(secret)=secret{command.env("CP_SYNC_SESSION_SECRET",secret);}
        let output=tokio::time::timeout(std::time::Duration::from_secs(20),command.output()).await.expect("prazo do processo isolado").unwrap();
        assert!(output.status.success(),"processo de prova {}: stdout={} stderr={}",output.status,String::from_utf8_lossy(&output.stdout),String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&std::fs::read(root.join("result.json")).unwrap()).unwrap()
    }
    let first=context(root.path(),"first.json","01","ab");
    assert_eq!(child(root.path(),Some(&first),"issue",None).await["status"],200);
    assert_eq!(child(root.path(),Some(&first),"read",None).await["status"],200,"GUE-068: relançamento do filho conserva o login");
    let next=context(root.path(),"next.json","02","ab");
    assert_eq!(child(root.path(),Some(&next),"read",None).await["status"],401,"nova época revoga");
    let changed=context(root.path(),"changed.json","01","cd");
    assert_eq!(child(root.path(),Some(&changed),"read",None).await["status"],401,"nova chave revoga");
    std::fs::write(root.path().join("first.json"),b"invalid").unwrap();
    assert_eq!(child(root.path(),Some(&first),"read",None).await["status"],503,"contexto quebrado recusa sem segredo de reserva");
    assert_eq!(child(root.path(),Some(&next),"issue",Some("explicit-fixture")).await["status"],200);
    assert_eq!(child(root.path(),Some(&changed),"read",Some("explicit-fixture")).await["status"],200,"segredo explícito vence o contexto");
    assert_eq!(child(root.path(),None,"issue",None).await["status"],200);
    assert_eq!(child(root.path(),None,"read",None).await["status"],401,"sem contexto o segredo é por processo");
}

mod guest_accounts {
    //! Contas de convidado que a máquina guarda (`~/.hangar/guest-accounts.json`): criadas pelo token do
    //! dono, sem a Sincronização ligada, e abertas pelo login de sincronização que os clientes já têm.
    use super::*;
    use base64::{Engine, engine::general_purpose::STANDARD};
    use hangar_server::login_guests::{Service, Settings};
    use serde_json::json;

    const CONNECT: &str = "192.0.2.1";

    fn service(home: &std::path::Path, sync_enabled: bool, rate_max: u32) -> Arc<Service> {
        Arc::new(Service::new(Settings { guests: home.join("guests.json"), vault: home.join("sync-vault.json"),
            runtime_config: home.join("runtime-config.json"), sync_enabled, bootstrap: "bootstrap".into(),
            secret: b"fixture".to_vec(), secure: false, rate_max: rate_max.into(), rate_window: 300.into() }))
    }

    async fn call(service: &Arc<Service>, owner: bool, ip: &str, method: &str, path: &str, body: Option<Value>, cookie: Option<&str>) -> (u16, Value, Option<String>) {
        let mut request = axum::http::Request::builder().method(method).uri(path).header("content-type", "application/json");
        if let Some(cookie) = cookie { request = request.header("cookie", cookie); }
        let raw = body.map(|b| b.to_string()).unwrap_or_default();
        let response = service.respond(owner, ip.into(), true, request.body(axum::body::Body::from(raw)).unwrap()).await;
        let status = response.status().as_u16();
        let cookie = response.headers().get("set-cookie").map(|v| String::from_utf8_lossy(v.as_bytes()).split(';').next().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null), cookie)
    }

    fn account(user: &str, password: Option<&str>) -> Value {
        let mut body = json!({"user": user, "sees_owner": false, "owner_sees": true,
            "servers": [{"server_id": "srv-casa", "label": "Casa", "base_url": "https://casa.jeff.hangar.dev.br",
                "guest_id": "g-1", "token": "token-da-convidada", "root": "/home/jeff/projetos"}]});
        if let Some(password) = password { body["password"] = json!(password); }
        body
    }

    /// O que o Expo faz em `syncLogin`: prelogin, chaves, login e leitura do cofre.
    async fn sign_in(service: &Arc<Service>, ip: &str, user: &str, password: &str) -> (u16, Option<String>, [u8; 32]) {
        let (status, prelogin, _) = call(service, false, ip, "GET", &format!("/api/sync/prelogin?user={user}"), None, None).await;
        assert_eq!(status, 200, "{prelogin}");
        assert_eq!(prelogin["iterations"], 600000);
        let salt = STANDARD.decode(prelogin["salt"].as_str().unwrap()).unwrap();
        let keys = hangar_guest_accounts::derive_keys(password, &salt, 600000);
        let (status, _, cookie) = call(service, false, ip, "POST", "/api/sync/login", Some(json!({"user": user, "auth_hash": keys.auth_hash})), None).await;
        (status, cookie, keys.enc_key)
    }

    #[tokio::test]
    async fn owner_token_creates_a_guest_that_signs_in_without_sync() {
        let home = crate::physical_tempdir();
        let service = service(home.path(), false, 10);
        let (status, created, _) = call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("Lhais87.Oliveira", Some("senha-dela"))), None).await;
        assert_eq!(status, 200, "{created}");
        assert_eq!(created["user"], "lhais87.oliveira", "nome guardado em minúsculas");
        assert!(!home.path().join("sync-vault.json").exists(), "o cofre antigo não é tocado");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(home.path().join("guest-accounts.json")).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        let (status, listed, _) = call(&service, true, "127.0.0.1", "GET", "/api/guest-accounts", None, None).await;
        assert_eq!(status, 200);
        assert_eq!(listed, json!([{"user": "lhais87.oliveira", "sees_owner": false, "owner_sees": true, "name_taken": false,
            "servers": [{"server_id": "srv-casa", "label": "Casa", "base_url": "https://casa.jeff.hangar.dev.br", "guest_id": "g-1", "root": "/home/jeff/projetos"}]}]));
        assert_eq!(call(&service, false, "127.0.0.1", "GET", "/api/guest-accounts", None, None).await.0, 401, "só o dono administra");

        let (status, cookie, key) = sign_in(&service, CONNECT, "Lhais87.Oliveira", "senha-dela").await;
        assert_eq!(status, 200);
        let cookie = cookie.expect("login grava o cookie");
        let headers = axum::http::HeaderMap::from_iter([(axum::http::header::COOKIE, cookie.parse().unwrap())]);
        assert!(service.sync_session_valid(&headers), "o porteiro do Connect deixa o cofre passar sem a Sincronização");
        let (status, vault, _) = call(&service, false, CONNECT, "GET", "/api/sync/vault", None, Some(&cookie)).await;
        assert_eq!(status, 200, "{vault}");
        let blob: hangar_guest_accounts::Blob = serde_json::from_value(vault["enc_blob"].clone()).unwrap();
        let plain = hangar_guest_accounts::decrypt(&key, &blob).expect("a chave do cliente abre o cofre");
        let list: Value = serde_json::from_slice(&plain).unwrap();
        assert_eq!(list, json!([{"id": "srv-casa", "label": "Casa", "baseUrl": "https://casa.jeff.hangar.dev.br", "token": "token-da-convidada"}]));
        assert_eq!(call(&service, false, CONNECT, "POST", "/api/sync/logout", None, None).await.0, 200);

        let (status, refused, _) = call(&service, false, CONNECT, "PUT", "/api/sync/vault", Some(json!({"enc_blob": blob, "base_rev": 1})), Some(&cookie)).await;
        assert_eq!(status, 403);
        assert_eq!(refused["detail"]["code"], "erro_cofre_do_servidor");
    }

    #[tokio::test]
    async fn edit_keeps_token_and_base_url_and_new_server_needs_a_token() {
        let home = crate::physical_tempdir();
        let service = service(home.path(), false, 10);
        assert_eq!(call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("ana.silva", Some("x"))), None).await.0, 200);
        let mut edit = account("ana.silva", None);
        edit["servers"][0].as_object_mut().unwrap().remove("token");
        edit["servers"][0].as_object_mut().unwrap().remove("base_url");
        edit["servers"][0]["root"] = json!("/home/jeff/outra");
        let (status, saved, _) = call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(edit.clone()), None).await;
        assert_eq!(status, 200, "{saved}");
        assert_eq!(saved["servers"][0]["base_url"], "https://casa.jeff.hangar.dev.br");
        let (_, cookie, key) = sign_in(&service, "10.0.0.5", "ana.silva", "x").await;
        let (_, vault, _) = call(&service, false, "10.0.0.5", "GET", "/api/sync/vault", None, cookie.as_deref()).await;
        let blob: hangar_guest_accounts::Blob = serde_json::from_value(vault["enc_blob"].clone()).unwrap();
        let list: Value = serde_json::from_slice(&hangar_guest_accounts::decrypt(&key, &blob).unwrap()).unwrap();
        assert_eq!(list[0]["token"], "token-da-convidada", "a senha e o token ficam");

        edit["servers"][0]["server_id"] = json!("srv-novo");
        let (status, refused, _) = call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(edit), None).await;
        assert_eq!(status, 400);
        assert_eq!(refused["detail"]["code"], "erro_convidado_invalido");
    }

    #[tokio::test]
    async fn names_need_dot_or_at_and_creation_needs_a_password() {
        let home = crate::physical_tempdir();
        let service = service(home.path(), false, 10);
        for bad in ["anasilva", "ana silva.x", "ana/x.y"] {
            let (status, body, _) = call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account(bad, Some("x"))), None).await;
            assert_eq!(status, 400, "{bad}: {body}");
            assert_eq!(body["detail"]["code"], "erro_convidado_invalido");
        }
        let (status, body, _) = call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("ana.silva", None)), None).await;
        assert_eq!((status, body["detail"]["code"].as_str()), (400, Some("erro_senha_obrigatoria")));
        assert!(!home.path().join("guest-accounts.json").exists(), "pedido recusado não grava nada");
    }

    #[tokio::test]
    async fn removed_guest_no_longer_signs_in() {
        let home = crate::physical_tempdir();
        let service = service(home.path(), false, 10);
        assert_eq!(call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("ana.silva", Some("x"))), None).await.0, 200);
        assert_eq!(call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("bia@casa", Some("y"))), None).await.0, 200);
        let (_, cookie, _) = sign_in(&service, "10.0.0.5", "ana.silva", "x").await;
        assert_eq!(call(&service, false, "127.0.0.1", "POST", "/api/guest-accounts/ana.silva/delete", None, None).await.0, 401);
        assert_eq!(call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts/ana.silva/delete", None, None).await.0, 200);
        assert_eq!(call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts/ana.silva/delete", None, None).await.0, 404);
        assert_eq!(call(&service, false, "10.0.0.5", "GET", "/api/sync/vault", None, cookie.as_deref()).await.0, 401, "o cookie dela cai junto");
        let (status, ..) = sign_in(&service, "10.0.0.6", "ana.silva", "x").await;
        assert_eq!(status, 401);
        let (_, listed, _) = call(&service, true, "127.0.0.1", "GET", "/api/guest-accounts", None, None).await;
        assert_eq!(listed.as_array().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn unknown_name_gets_a_stable_fake_salt() {
        let home = crate::physical_tempdir();
        let first_life = service(home.path(), false, 10);
        assert_eq!(call(&first_life, false, CONNECT, "GET", "/api/sync/prelogin?user=ninguem.aqui", None, None).await.0, 404,
            "sem conta de convidado e sem Sincronização a rota continua fechada");
        assert_eq!(call(&first_life, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("ana.silva", Some("x"))), None).await.0, 200);
        let salt = |body: Value| body["salt"].as_str().unwrap().to_owned();
        let first = salt(call(&first_life, false, CONNECT, "GET", "/api/sync/prelogin?user=ninguem.aqui", None, None).await.1);
        let again = salt(call(&first_life, false, CONNECT, "GET", "/api/sync/prelogin?user=Ninguem.Aqui", None, None).await.1);
        let other = salt(call(&first_life, false, CONNECT, "GET", "/api/sync/prelogin?user=outro.nome", None, None).await.1);
        let real = salt(call(&first_life, false, CONNECT, "GET", "/api/sync/prelogin?user=ana.silva", None, None).await.1);
        assert_eq!(first, again);
        assert_ne!(first, other);
        assert_ne!(first, real);
        assert_eq!(STANDARD.decode(&first).unwrap().len(), 16, "mesma forma do sal de verdade");
        let restarted = service(home.path(), false, 10);
        assert_eq!(salt(call(&restarted, false, CONNECT, "GET", "/api/sync/prelogin?user=ninguem.aqui", None, None).await.1), first,
            "o segredo do arquivo mantém o sal depois de reiniciar");
    }

    #[tokio::test]
    async fn connect_attempts_count_per_user_and_direct_ones_per_address() {
        let home = crate::physical_tempdir();
        let service = service(home.path(), false, 2);
        assert_eq!(call(&service, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("ana.silva", Some("x"))), None).await.0, 200);
        let wrong = |user: &'static str, ip: &'static str| {
            let service = service.clone();
            async move { call(&service, false, ip, "POST", "/api/sync/login", Some(json!({"user": user, "auth_hash": "errado"})), None).await.0 }
        };
        assert_eq!(wrong("ana.silva", CONNECT).await, 401);
        assert_eq!(wrong("Ana.Silva", CONNECT).await, 401);
        assert_eq!(wrong("ana.silva", CONNECT).await, 429, "o nome em qualquer caixa conta junto");
        assert_eq!(wrong("bia@casa", CONNECT).await, 401, "pela entrada do Connect outro nome não fica preso");
        assert_eq!(wrong("ana.silva", "10.0.0.5").await, 401);
        assert_eq!(wrong("ana.silva", "10.0.0.5").await, 401);
        assert_eq!(wrong("bia@casa", "10.0.0.5").await, 429, "fora do Connect o endereço conta, como antes");
    }

    #[tokio::test]
    async fn old_sync_routes_stay_as_they_were() {
        let home = crate::physical_tempdir();
        let disabled = service(home.path(), false, 10);
        assert_eq!(call(&disabled, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("ana.silva", Some("x"))), None).await.0, 200);
        for (method, path) in [("GET", "/api/sync/guests"), ("GET", "/api/sync/status"), ("POST", "/api/sync/register")] {
            assert_eq!(call(&disabled, true, "127.0.0.1", method, path, Some(json!({})), None).await.0, 404, "{method} {path}: desligada segue fechada");
        }

        let enabled = service(home.path(), true, 10);
        let (status, _, _) = call(&enabled, false, "127.0.0.1", "POST", "/api/sync/register",
            Some(json!({"user": "jeff.f", "salt": "fixture", "auth_hash": "fixture", "bootstrap": "bootstrap"})), None).await;
        assert_eq!(status, 200, "o cadastro do dono continua no cofre antigo");
        let (status, _, cookie) = call(&enabled, false, "127.0.0.1", "POST", "/api/sync/login", Some(json!({"user": "jeff.f", "auth_hash": "fixture"})), None).await;
        assert_eq!(status, 200);
        let (status, guests, _) = call(&enabled, false, "127.0.0.1", "GET", "/api/sync/guests", None, cookie.as_deref()).await;
        assert_eq!((status, guests), (200, json!([])), "a lista antiga não mostra as contas novas");
        let (status, refused, _) = call(&enabled, true, "127.0.0.1", "POST", "/api/guest-accounts", Some(account("Jeff.F", Some("y"))), None).await;
        assert_eq!((status, refused["detail"]["code"].as_str()), (409, Some("erro_usuario_em_uso")), "o nome do dono do cofre antigo fica com ele");
    }
}
