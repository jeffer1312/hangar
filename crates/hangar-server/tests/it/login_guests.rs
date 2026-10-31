//! Contratos de convidados, com upstream indisponível.
use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::Value;
use std::sync::Arc;

#[tokio::test]
async fn login_guests_public_gate_denies_owner_routes_before_upstream() {
    use hangar_server::login_guests::{Service, Settings};
    let home = crate::physical_tempdir();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config { listen:address, upstream:crate::refused_address(),
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
    let upstream=crate::refused_address();let mut state=AppState::new(crate::fake::config(upstream,""));
    state.accounts=AccountService::new(AccountEnvironment::from_map([("HOME".into(),home.to_string_lossy().into_owned()),("USERPROFILE".into(),home.to_string_lossy().into_owned())].into()));
    state.term=Arc::new(Terms::new(TermConfig{program:program.clone().into_os_string(),..TermConfig::default()}));
    state.list=Arc::new(ListBridge::new(ListEnv{mux:Mux::with_program(&program,Duration::from_secs(5)),capture_program:program.into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs:parse_dirs(&json!({"home":home,"claude":claude,"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string())},FactsClient::new(upstream,crate::fake::SECRET.into())));
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
    let state=AppState::new(crate::fake::config(crate::refused_address(),""));
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
