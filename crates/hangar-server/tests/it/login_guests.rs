//! Contratos de convidados medidos no Python, com upstream indisponível.
use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::Value;
use std::sync::Arc;

#[tokio::test]
async fn login_guests_owner_identity_is_served_without_python() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/http.json"
    ))
    .unwrap();
    let row = fixture["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "dono")
        .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "guest-fixture".into(),
        auth_token: "owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            hangar_server::routes::router(Arc::new(state))
                .into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let response = reqwest::Client::new()
        .get(format!("http://{address}/api/me"))
        .bearer_auth("owner")
        .send()
        .await
        .unwrap();
    let status = response.status().as_u16();
    let raw = response.text().await.unwrap();
    task.abort();
    assert_eq!(
        status,
        row["status"].as_u64().unwrap() as u16,
        "GUE-010: a identidade não depende do Python"
    );
    let body: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(body, row["response"]);
}

#[tokio::test]
async fn login_guests_python_http_contract_and_disk_effects() {
    use hangar_server::login_guests::{Service, Settings};
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/http.json"
    ))
    .unwrap();
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir(home.path().join("projects")).unwrap();
    let settings = Settings {
        guests: home.path().join("guests.json"),
        vault: home.path().join("vault.json"),
        runtime_config: home.path().join("runtime-config.json"),
        sync_enabled: true,
        bootstrap: "bootstrap".into(),
        secret: b"fixture-secret".to_vec(),
        secure: false,
        rate_max: 10.into(),
        rate_window: 300.into(),
    };
    let mut service = Service::new(settings);
    service.now = || 2000000000;
    service.entropy = |n| vec![b'x'; n];
    service.id = || "ab".repeat(8);
    service.token = || "guest-token".into();
    let service = Arc::new(service);
    let app = axum::Router::new().fallback({
        let service = service.clone();
        move |request: axum::extract::Request| {
            let service = service.clone();
            async move {
                let owner = request
                    .headers()
                    .get("authorization")
                    .is_some_and(|h| h == "Bearer owner");
                service
                    .respond(owner, "127.0.0.1".into(), false, request)
                    .await
            }
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let client = reqwest::Client::new();
    for row in fixture["rows"].as_array().unwrap() {
        let expected: Value = serde_json::from_str(
            &row.to_string()
                .replace("⟦ROOT⟧", &home.path().to_string_lossy()),
        )
        .unwrap();
        let method =
            reqwest::Method::from_bytes(expected["method"].as_str().unwrap().as_bytes()).unwrap();
        let mut request = client.request(
            method,
            format!("http://{address}{}", expected["path"].as_str().unwrap()),
        );
        if let Some(token) = expected["token"].as_str() {
            request = request.bearer_auth(token);
        }
        if let Some(cookie) = expected["cookie"].as_str() {
            request = request.header("cookie", format!("cp_sync={cookie}"));
        }
        if !expected["body"].is_null() {
            request = request.json(&expected["body"]);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let cookie = response
            .headers()
            .get("set-cookie")
            .and_then(|h| h.to_str().ok())
            .map(str::to_owned);
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            status,
            expected["status"].as_u64().unwrap() as u16,
            "{}: {body}",
            row["name"]
        );
        assert_eq!(body, expected["response"], "{}", row["name"]);
        assert_eq!(
            cookie,
            expected["set_cookie"].as_str().map(str::to_owned),
            "cookie {}",
            row["name"]
        );
        for (file, key) in [("guests.json", "guests"), ("vault.json", "vault")] {
            let actual = std::fs::read(home.path().join(file))
                .ok()
                .map(|b| serde_json::from_slice::<Value>(&b).unwrap())
                .unwrap_or(Value::Null);
            assert_eq!(actual, expected[key], "disco {file}: {}", row["name"]);
        }
    }
    task.abort();
}

#[test]
fn login_guests_gate_matches_python_matrix() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/http.json")).unwrap();
    for row in fixture["gate_rows"].as_array().unwrap() {
        let actual = hangar_server::login_guests::gate::allowed(row["method"].as_str().unwrap(), row["path"].as_str().unwrap(), |name| name == "ana");
        assert_eq!(actual, row["allowed"].as_bool().unwrap(), "GUE-031/032: {} {}", row["method"], row["path"]);
    }
}

#[tokio::test]
async fn login_guests_public_gate_denies_owner_routes_before_upstream() {
    use hangar_server::login_guests::{Service, Settings};
    let home = tempfile::tempdir().unwrap();
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
    for path in ["/api/config", "/api/voice", "/api/peers", "/api/sessions/owner/share"] {
        let response=client.get(format!("http://{address}{path}")).bearer_auth("guest").send().await.unwrap();
        assert_eq!(response.status().as_u16(),403,"GUE-030/032: {path}");
        let body:Value=response.json().await.unwrap();
        assert_eq!(body["detail"]["code"],"erro_fora_do_convidado");
    }
    task.abort();
}

#[tokio::test]
async fn login_guests_body_limit_precedes_authentication() {
    use hangar_server::login_guests::{Service,Settings};
    let home=tempfile::tempdir().unwrap();
    let service=Arc::new(Service::new(Settings {guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()}));
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/limits.json")).unwrap();
    let expected=&fixture["rows"][0];
    let request=axum::http::Request::post("/api/guests").header("content-length",104857601u64).body(axum::body::Body::empty()).unwrap();
    let response=service.respond(false,"127.0.0.1".into(),false,request).await;
    assert_eq!(response.status().as_u16(),expected["status"].as_u64().unwrap() as u16,"GUE-080: limite anterior à autenticação");
    assert_eq!(response.headers()["content-type"].to_str().unwrap(),expected["content_type"].as_str().unwrap());
    let body=axum::body::to_bytes(response.into_body(),1024).await.unwrap();
    assert_eq!(std::str::from_utf8(&body).unwrap(),expected["response"].as_str().unwrap());
    service.store.create("Ana",home.path().to_str().unwrap(),false,true,"ana".into(),"guest-token".into()).unwrap();
    let mut state=AppState::new(crate::fake::config(crate::refused_address(),""));state.share=None;state.login_guests=service;
    let mut request=axum::http::Request::post("/api/guests").header("content-length",104857601u64)
        .header("authorization","Bearer guest-token").body(axum::body::Body::empty()).unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo("127.0.0.1:1234".parse::<std::net::SocketAddr>().unwrap()));
    use tower_service::Service as TowerService;
    let response=hangar_server::routes::router(Arc::new(state)).call(request).await.unwrap();
    assert_eq!(response.status().as_u16(),expected["status"].as_u64().unwrap() as u16,"GUE-080: teto real de 100 MiB precede o porteiro público");
}

#[cfg(unix)]
#[tokio::test]
async fn login_guests_list_visibility_uses_python_matrix_without_forwarding() {
    use hangar_server::list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux};
    use hangar_server::login_guests::{Service,Settings};
    use serde_json::json;
    use std::time::Duration;
    let root=tempfile::tempdir().unwrap();
    let program=crate::list_support::sessions(root.path(),3);
    let home=root.path().join("home");let claude=home.join(".claude");
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/store.json")).unwrap();
    let disk:Value=serde_json::from_str(&fixture["matrix_disk"].to_string().replace("⟦ROOT⟧",root.path().to_str().unwrap())).unwrap();
    std::fs::write(claude.join("guests.json"),disk.to_string()).unwrap();
    let (python,upstream)=crate::fake::spawn_fake().await;
    let mut state=AppState::new(crate::fake::config(upstream,""));
    state.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::with_program(&program,Duration::from_secs(5)),capture_program:program.into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs:parse_dirs(&json!({"home":home,"claude":claude,"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string())},FactsClient::new(upstream,crate::fake::SECRET.into())));
    state.share = None;
    state.login_guests=Arc::new(Service::new(Settings{guests:claude.join("guests.json"),vault:root.path().join("vault.json"),runtime_config:claude.join("runtime-config.json"),sync_enabled:false,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()}));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
    for (index,row) in fixture["list_matrix"].as_array().unwrap().iter().enumerate() {
        let token=match index {0=>crate::fake::OWNER,1=>"token-0",_=>"token-1"};
        let response=reqwest::Client::new().get(format!("http://{address}/api/sessions")).bearer_auth(token).send().await.unwrap();
        assert_eq!(response.status(),200,"GUE-025/027: observador {index}");
        let text=response.text().await.unwrap();
        let body:Value=serde_json::from_str(&text).unwrap_or(Value::String(text));
        assert!(body.is_array(),"GUE-025/027: lista nativa: {body}");
        let names:Vec<_>=body.as_array().unwrap().iter().map(|v|v["name"].clone()).collect();
        assert_eq!(json!(names),row["names"],"GUE-025/027: observador {index}");
    }
    assert_eq!(python.hits_to("/api/sessions"),0,"GUE-027: nenhuma lista de login vai ao Python");
    task.abort();
}

#[tokio::test]
async fn login_guests_invalid_forms_match_all_python_errors() {
    use hangar_server::login_guests::{Service,Settings};
    let home=tempfile::tempdir().unwrap();
    let http:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/http.json")).unwrap();
    let registered=http["rows"].as_array().unwrap().iter().find(|r|r["name"]=="registrar").unwrap();
    std::fs::write(home.path().join("vault.json"),registered["vault"].to_string()).unwrap();
    let cookie=http["rows"].as_array().unwrap().iter().find(|r|r["name"]=="cofre_vazio").unwrap()["cookie"].as_str().unwrap();
    let mut service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:"bootstrap".into(),secret:b"fixture-secret".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
    service.now=||2000000000;let service=Arc::new(service);
    let cases:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/validation.json")).unwrap();
    for row in cases["rows"].as_array().unwrap().iter().filter(|r|r["status"]==422) {
        let request=axum::http::Request::builder().method(row["method"].as_str().unwrap()).uri(row["path"].as_str().unwrap())
            .header("content-type","application/json").header("cookie",format!("cp_sync={cookie}")).body(axum::body::Body::from(row["body"].to_string())).unwrap();
        let response=service.respond(true,"127.0.0.1".into(),false,request).await;
        assert_eq!(response.status().as_u16(),422,"GUE-081 {} {}",row["model"],row["index"]);
        let body=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
        let actual:Value=serde_json::from_slice(&body).unwrap();
        assert_eq!(actual,row["response"],"GUE-081 {} {}",row["model"],row["index"]);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn login_guests_filesystem_uses_only_guest_root() {
    use hangar_server::login_guests::{Service,Settings};
    let home=tempfile::tempdir().unwrap();let root=home.path().join("allowed");
    std::fs::create_dir_all(root.join("project")).unwrap();std::fs::create_dir(home.path().join("outside")).unwrap();
    std::fs::write(root.join("file"),"fixture").unwrap();std::os::unix::fs::symlink(home.path().join("outside"),root.join("escape")).unwrap();
    std::fs::File::open(root.join("project")).unwrap().set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH+std::time::Duration::from_secs(2000000000))).unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"fixture".into(),auth_token:"owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    let service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:false,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
    service.store.create("Ana",root.to_str().unwrap(),false,true,"fixture".into(),"guest".into()).unwrap();state.share = None;
    state.login_guests=Arc::new(service);
    let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
    let fixture:Value=serde_json::from_str(&include_str!("../../../../backend/tests/fixtures/contract/guests/fs.json").replace("⟦ROOT⟧",home.path().to_str().unwrap())).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let route=if row["name"]=="roots"{"roots"}else{"scan"};
        let mut query=form_urlencoded::Serializer::new(String::new());
        if let Some(root)=row["root"].as_str(){query.append_pair("root",root);}
        if let Some(path)=row["path"].as_str(){query.append_pair("path",path);}
        let request=reqwest::Client::new().get(format!("http://{address}/api/fs/{route}?{}",query.finish())).bearer_auth("guest");
        let response=request.send().await.unwrap();assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"GUE-036 {}",row["name"]);
        let body:Value=response.json().await.unwrap();assert_eq!(body,row["response"],"GUE-036 {}",row["name"]);
    }
    task.abort();
}

#[tokio::test]
async fn login_guests_vault_revisions_keep_python_integer_range() {
    use hangar_server::login_guests::{Service,Settings};
    let home=tempfile::tempdir().unwrap();
    let cases:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/revision.json")).unwrap();
    let mut service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:"bootstrap".into(),secret:b"fixture-secret".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
    service.now=||2000000000;let service=Arc::new(service);
    for (index,row) in cases["rows"].as_array().unwrap().iter().enumerate() {
        std::fs::write(home.path().join("vault.json"),row["initial"].to_string()).unwrap();
        let request=axum::http::Request::builder().method("PUT").uri("/api/sync/vault")
            .header("content-type","application/json").header("cookie",format!("cp_sync={}",cases["cookie"].as_str().unwrap())).body(axum::body::Body::from(row["body"].to_string())).unwrap();
        let response=service.respond(false,"127.0.0.1".into(),false,request).await;
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"GUE-044/081 revisão {index}");
        let body=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(),row["response"],"GUE-044/081 resposta {index}");
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(home.path().join("vault.json")).unwrap()).unwrap(),row["vault"],"GUE-044 disco {index}");
    }
}

#[tokio::test]
async fn login_guests_storage_normalizes_legacy_documents_before_writing() {
    use hangar_server::login_guests::{Service,Settings};
    let home=tempfile::tempdir().unwrap();
    let cases:Value=serde_json::from_str(&include_str!("../../../../backend/tests/fixtures/contract/guests/storage.json").replace("⟦ROOT⟧",home.path().to_str().unwrap())).unwrap();
    for (index,row) in cases["rows"].as_array().unwrap().iter().enumerate() {
        std::fs::write(home.path().join("guests.json"),row["initial"].to_string()).unwrap();
        let mut service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:false,bootstrap:String::new(),secret:vec![1],secure:false,rate_max:10.into(),rate_window:300.into()});
        service.id=||"ab".repeat(8);service.token=||"new-token".into();let service=Arc::new(service);
        let request=axum::http::Request::builder().method("POST").uri("/api/guests").header("content-type","application/json").body(axum::body::Body::from(row["body"].to_string())).unwrap();
        let response=service.respond(true,"127.0.0.1".into(),false,request).await;
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"GUE-012/013 arquivo {index}");
        let body=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(),row["response"],"GUE-012/013 resposta {index}");
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(home.path().join("guests.json")).unwrap()).unwrap(),row["disk"],"GUE-012/013 disco {index}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn login_guests_sse_filters_and_ends_when_token_is_revoked() {
    use hangar_server::list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux};
    use hangar_server::login_guests::{Service,Settings};
    use serde_json::json;
    use std::time::Duration;
    let root=tempfile::tempdir().unwrap();
    let program=crate::list_support::sessions(root.path(),3);
    let home=root.path().join("home");let claude=home.join(".claude");
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/store.json")).unwrap();
    let disk:Value=serde_json::from_str(&fixture["matrix_disk"].to_string().replace("⟦ROOT⟧",root.path().to_str().unwrap())).unwrap();
    std::fs::write(claude.join("guests.json"),disk.to_string()).unwrap();
    let (python,upstream)=crate::fake::spawn_fake().await;
    let mut state=AppState::new(crate::fake::config(upstream,""));
    state.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::with_program(&program,Duration::from_secs(5)),capture_program:program.into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs:parse_dirs(&json!({"home":home,"claude":claude,"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string())},FactsClient::new(upstream,crate::fake::SECRET.into())));
    state.share = None;
    state.login_guests=Arc::new(Service::new(Settings{guests:claude.join("guests.json"),vault:root.path().join("vault.json"),runtime_config:claude.join("runtime-config.json"),sync_enabled:false,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()}));
    let service=state.login_guests.clone();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
    let response=reqwest::Client::new().get(format!("http://{address}/api/sessions/events")).bearer_auth("token-0").send().await.unwrap();
    assert_eq!(response.status(),200,"GUE-028: SSE nativo");
    assert!(response.headers().get("content-type").is_some_and(|v|v.to_str().unwrap().starts_with("text/event-stream")),"GUE-028: tipo do stream");
    let mut events=crate::fake::sse(response);
    use futures_util::StreamExt;
    let first=tokio::time::timeout(Duration::from_secs(5),async {
        while let Some(Ok(event))=events.next().await {if event.event=="sessions" {return Some(event.data);}}
        None
    }).await.ok().flatten();
    assert!(first.is_some(),"GUE-028: publica sessions no stream nativo");
    let rows:Value=serde_json::from_str(&first.unwrap()).unwrap();
    let names:Vec<_>=rows.as_array().unwrap().iter().map(|v|v["name"].clone()).collect();
    assert_eq!(json!(names),fixture["list_matrix"][1]["names"],"GUE-028: mesma matriz do Python");
    service.store.delete(&"0".repeat(16)).unwrap();
    tokio::time::timeout(Duration::from_secs(7),async {while let Some(event)=events.next().await {
        let event=event.unwrap();assert_ne!(event.event,"sessions","GUE-033: não publica outra lista após revogar");
    }}).await.expect("GUE-033/034: SSE termina após revogar");
    assert_eq!(python.hits_to("/api/sessions/events"),0,"GUE-028: não encaminha o stream ao Python");
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
            let home=tempfile::tempdir().unwrap();
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

#[tokio::test]
async fn login_guests_setup_preserves_vault_and_changes_enabled_state() {
    use hangar_server::login_guests::{Service,Settings};
    let cases:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/setup.json")).unwrap();
    for row in cases["rows"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();
        for (key,file) in [("vault","vault.json"),("config","config.json")] {
            if let Some(raw)=row["initial"][key].as_str(){std::fs::write(home.path().join(file),raw).unwrap();}
        }
        let mut service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:false,bootstrap:String::new(),secret:b"fixture-secret".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
        service.entropy=|n|vec![b'x';n];service.now=||2000000000;let service=Arc::new(service);
        let request=axum::http::Request::builder().method(row["method"].as_str().unwrap()).uri(row["path"].as_str().unwrap()).header("content-type","application/json").body(axum::body::Body::from(row["body"].to_string())).unwrap();
        let response=service.respond(row["owner"].as_bool().unwrap(),"127.0.0.1".into(),false,request).await;
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"GUE-053/054/057/058 {}",row["name"]);
        assert_eq!(response.headers().get("set-cookie").map(|v|v.to_str().unwrap()),row["set_cookie"].as_str(),"GUE-057 cookie");
        let body=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
        let actual:Value=serde_json::from_slice(&body).unwrap();
        if row["status"]==500 {
            // D03: conserva status/efeitos; falha antes opaca recebe código sem dado sensível.
            assert_eq!(actual,serde_json::json!({"detail":{"code":"sync_vault_unreadable","params":{},"msg":"cofre ilegível"}}));
        }else{assert_eq!(actual,row["response"],"GUE-054 resposta {}",row["name"]);}
        for (key,file) in [("vault","vault.json"),("config","config.json")] {
            let actual=std::fs::read_to_string(home.path().join(file)).ok();
            let expected=row["after"][key].as_str();
            match (actual.as_deref(),expected) {
                (Some(a),Some(b))=>match (serde_json::from_str::<Value>(a),serde_json::from_str::<Value>(b)) {
                    (Ok(a),Ok(b))=>assert_eq!(a,b,"GUE-054/058 disco {} {key}",row["name"]),
                    _=>assert_eq!(a,b,"GUE-054 inválido preservado"),
                },
                _=>assert_eq!(actual.as_deref(),expected,"GUE-054 existência {} {key}",row["name"]),
            }
        }
    }
}

#[test]
fn login_guests_store_preserves_session_life_and_failed_adoption() {
    use hangar_server::login_guests::store::Store;
    let home=tempfile::tempdir().unwrap();
    let fixture:Value=serde_json::from_str(&include_str!("../../../../backend/tests/fixtures/contract/guests/store.json").replace("⟦ROOT⟧",home.path().to_str().unwrap())).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let path=home.path().join("guests.json");
        std::fs::write(&path,row["before"].to_string()).unwrap();
        let store=Store::new(path.clone());
        let owner=store.owner("session",row["life"].as_str(),true).map(|g|g.id);
        assert_eq!(serde_json::json!(owner),row["owner"],"GUE-018/019/022 {}",row["name"]);
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),row["disk"],"GUE-022 disco {}",row["name"]);
    }
    let row=&fixture["failed_adoption"];
    let path=home.path().join("guests.json");std::fs::write(&path,row["before"].to_string()).unwrap();
    let lock=home.path().join("guests.lock");let _=std::fs::remove_file(&lock);std::fs::create_dir(&lock).unwrap();
    let store=Store::new(path.clone());
    assert_eq!(serde_json::json!(store.owner("session",Some("k:first"),true).map(|g|g.id)),row["first"],"GUE-022: leitura sobrevive falha de gravação");
    assert_eq!(serde_json::json!(store.owner("session",Some("k:second"),true).map(|g|g.id)),row["second"],"GUE-022: vida adotada fica em memória");
    assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(path).unwrap()).unwrap(),row["disk"],"GUE-022: disco intacto após falha");
}

#[tokio::test]
async fn login_guests_rejects_structural_identity_types_without_overwriting() {
    use hangar_server::login_guests::{Service,Settings};
    let home=tempfile::tempdir().unwrap();
    let cases:Value=serde_json::from_str(&include_str!("../../../../backend/tests/fixtures/contract/guests/identity-types.json").replace("⟦ROOT⟧",home.path().to_str().unwrap())).unwrap();
    for row in cases["rows"].as_array().unwrap() {
        let path=home.path().join("guests.json");std::fs::write(&path,row["initial"].to_string()).unwrap();
        let service=Arc::new(Service::new(Settings{guests:path.clone(),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:false,bootstrap:String::new(),secret:vec![1],secure:false,rate_max:10.into(),rate_window:300.into()}));
        for (owner,route,method,body,status) in [(false,"/api/me","GET",Value::Null,401),(true,"/api/guests","POST",serde_json::json!({"root":home.path(),"name":"Nova"}),500)] {
            let request=axum::http::Request::builder().method(method).uri(route).header("authorization","Bearer guest-token").header("content-type","application/json").body(axum::body::Body::from(body.to_string())).unwrap();
            let response=service.respond(owner,"127.0.0.1".into(),false,request).await;
            // D03/GUE-067: só os quatro campos estruturais deixam de aceitar tipos arbitrários.
            assert_eq!(response.status().as_u16(),status,"GUE-067 {} {} {route}",row["field"],row["value"]);
        }
        assert_eq!(serde_json::from_slice::<Value>(&std::fs::read(&path).unwrap()).unwrap(),row["initial"],"GUE-013/067: arquivo preservado");
    }
}

#[tokio::test]
async fn login_guests_public_credentials_match_python_precedence() {
    use hangar_server::login_guests::{Service,Settings};
    let home=tempfile::tempdir().unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"fixture".into(),auth_token:"owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    let service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:false,bootstrap:String::new(),secret:vec![1],secure:false,rate_max:10.into(),rate_window:300.into()});
    service.store.create("Ana",home.path().to_str().unwrap(),false,true,"fixture".into(),"guest-token".into()).unwrap();state.share = None;
    state.login_guests=Arc::new(service);
    let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
    let cases:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/credentials.json")).unwrap();
    for (index,row) in cases["rows"].as_array().unwrap().iter().enumerate() {
        let mut request=reqwest::Client::new().request(row["method"].as_str().unwrap().parse().unwrap(),format!("http://{address}{}",row["path"].as_str().unwrap())).header("x-forwarded-proto",row["scheme"].as_str().unwrap());
        for (key,value) in row["headers"].as_object().unwrap(){request=request.header(key,value.as_str().unwrap());}
        let response=request.body("{}").send().await.unwrap();assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"GUE-029/064 credencial {index}");
        assert_eq!(response.json::<Value>().await.unwrap(),row["response"],"GUE-029/064 resposta {index}");
    }
    task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn login_guests_terminal_origin_matches_python_without_owner_restriction() {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux},login_guests::{Service,Settings},term::{Terms,TermConfig}};
    use serde_json::json;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest,Error};
    let root=tempfile::tempdir().unwrap();let program=crate::list_support::sessions(root.path(),1);
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
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
    let cases:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/terminal.json")).unwrap();
    for (index,row) in cases["rows"].as_array().unwrap().iter().enumerate() {
        let token=if row["guest"]==true {"guest-token"}else{crate::fake::OWNER};
        let mut request=format!("ws://{address}/api/sessions/s0/term?token={token}").into_client_request().unwrap();
        request.headers_mut().insert("host","fixture".parse().unwrap());
        if let Some(origin)=row["origin"].as_str(){request.headers_mut().insert("origin",origin.parse().unwrap());}
        match tokio_tungstenite::connect_async(request).await {
            Ok((mut socket,response))=>{assert_eq!(response.status(),101);assert_eq!(row["accepted"],true,"GUE-038 origem {index}");let _=socket.close(None).await;},
            Err(Error::Http(response))=>{assert_eq!(row["accepted"],false,"GUE-038 origem {index}: {}",response.status());assert_eq!(response.status(),403);},
            Err(error)=>panic!("GUE-038 transporte {index}: {error}"),
        }
    }
    task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn login_guests_terminal_closes_after_login_revocation() {
    use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux},login_guests::{Service,Settings},term::{Terms,TermConfig}};
    use serde_json::json;
    use std::time::Duration;
    use futures_util::{SinkExt,StreamExt};
    use tokio_tungstenite::tungstenite::{Message};
    let root=tempfile::tempdir().unwrap();let program=crate::list_support::sessions(root.path(),1);
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

#[cfg(unix)]
#[tokio::test]
async fn login_guests_owner_list_preserves_legacy_truthiness() {
    use hangar_server::list::{bridge::{ListBridge,ListEnv,parse_dirs},facts::FactsClient,mux::Mux};
    use hangar_server::login_guests::{Service,Settings};
    use serde_json::json;
    use std::time::Duration;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/visibility.json")).unwrap();
    for (index,row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
    let root=tempfile::tempdir().unwrap();
    let program=crate::list_support::sessions(root.path(),2);
    let home=root.path().join("home");let claude=home.join(".claude");
    let (python,upstream)=crate::fake::spawn_fake().await;
    let mut state=AppState::new(crate::fake::config(upstream,""));
    state.list=Arc::new(ListBridge::new(ListEnv {mux:Mux::with_program(&program,Duration::from_secs(5)),capture_program:program.into_os_string(),procs:Arc::new(hangar_server::list::procs::SystemProcs::default()),dirs:parse_dirs(&json!({"home":home,"claude":claude,"codex_home":home.join(".codex"),"pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string())},FactsClient::new(upstream,crate::fake::SECRET.into())));
    state.share = None;
    state.login_guests=Arc::new(Service::new(Settings{guests:claude.join("guests.json"),vault:root.path().join("vault.json"),runtime_config:claude.join("runtime-config.json"),sync_enabled:false,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()}));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
        let disk=row["before"].to_string().replace("⟦ROOT⟧",root.path().to_str().unwrap());
        std::fs::write(claude.join("guests.json"),disk).unwrap();
        let token=crate::fake::OWNER;
        let response=reqwest::Client::new().get(format!("http://{address}/api/sessions")).bearer_auth(token).send().await.unwrap();
        assert_eq!(response.status(),200,"GUE-025/027: observador {index}");
        let text=response.text().await.unwrap();
        let body:Value=serde_json::from_str(&text).unwrap_or(Value::String(text));
        assert!(body.is_array(),"GUE-025/027: lista nativa: {body}");
        let names:Vec<_>=body.as_array().unwrap().iter().map(|v|v["name"].clone()).collect();
        assert_eq!(json!(names),row["names"],"GUE-025/027: observador {index}");
    assert_eq!(python.hits_to("/api/sessions"),0,"GUE-027: nenhuma lista de login vai ao Python");
    task.abort();
    }
}

#[tokio::test]
async fn login_guests_routes_without_body_ignore_malformed_json() {
    use hangar_server::login_guests::{Service,Settings};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/ignored-body.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let root=tempfile::tempdir().unwrap();
        let service=Arc::new(Service::new(Settings{guests:root.path().join("guests.json"),vault:root.path().join("vault.json"),runtime_config:root.path().join("runtime-config.json"),sync_enabled:true,bootstrap:String::new(),secret:b"fixture-secret".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()}));
        let request=axum::http::Request::builder().method(row["method"].as_str().unwrap()).uri(row["path"].as_str().unwrap())
            .header("content-type","application/json").body(axum::body::Body::from(row["raw"].as_str().unwrap().to_owned())).unwrap();
        let response=service.respond(row["token"]=="owner","127.0.0.1".into(),false,request).await;
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"GUE-081: {row}");
        let bytes=axum::body::to_bytes(response.into_body(),usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(),row["response"],"GUE-081: {row}");
    }
}

#[tokio::test]
async fn login_guests_base64_rules_preserve_each_python_model() {
    use hangar_server::login_guests::{Service,Settings};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/base64.json")).unwrap();
    let blobs:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/blobs.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap().iter().chain(blobs["rows"].as_array().unwrap()) {
        let root=tempfile::tempdir().unwrap();
        let mut service=Service::new(Settings{guests:root.path().join("guests.json"),vault:root.path().join("vault.json"),runtime_config:root.path().join("runtime-config.json"),sync_enabled:true,bootstrap:"bootstrap".into(),secret:b"fixture-secret".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
        service.entropy=|n|vec![b'x';n];service.now=||2000000000;
        let service=Arc::new(service);
        let request=axum::http::Request::post(row["path"].as_str().unwrap()).header("content-type","application/json").body(axum::body::Body::from(row["body"].to_string())).unwrap();
        let response=service.respond(true,"127.0.0.1".into(),false,request).await;
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"GUE-081: {row}");
        let bytes=axum::body::to_bytes(response.into_body(),usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap(),row["response"],"GUE-081: {row}");
        let disk=std::fs::read(root.path().join("vault.json")).ok().map(|raw|serde_json::from_slice::<Value>(&raw).unwrap()).unwrap_or(Value::Null);
        assert_eq!(disk,row["vault"],"GUE-081: {row}");
    }
}

#[tokio::test]
async fn login_guests_public_body_limits_preserve_consumption_and_priority() {
    use hangar_server::login_guests::{Service,Settings};
    use std::sync::atomic::{AtomicBool,Ordering};
    use tower_service::Service as TowerService;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/limit-routes.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let root=tempfile::tempdir().unwrap();
        let mut service=Service::new(Settings{guests:root.path().join("guests.json"),vault:root.path().join("vault.json"),runtime_config:root.path().join("runtime-config.json"),sync_enabled:false,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
        service.body_limit=row["limit"].as_u64().unwrap() as usize;
        service.store.create("Ana",root.path().to_str().unwrap(),false,true,"ana".into(),"guest-token".into()).unwrap();
        let mut state=AppState::new(crate::fake::config(crate::refused_address(),""));state.share=None;state.login_guests=Arc::new(service);
        let token=match row["who"].as_str().unwrap(){"owner"=>crate::fake::OWNER,"guest"=>"guest-token",_=>"unknown"};
        let mut request=axum::http::Request::builder().method(row["method"].as_str().unwrap()).uri(row["path"].as_str().unwrap()).header("authorization",format!("Bearer {token}")).header("content-type","application/json");
        if row["kind"]=="content_length" {request=request.header("content-length",row["input_bytes"].as_u64().unwrap());}
        let consumed=Arc::new(AtomicBool::new(false));let observed=consumed.clone();
        let stream=futures_util::stream::once(async move{observed.store(true,Ordering::SeqCst);Ok::<_,std::io::Error>(bytes::Bytes::from_static(b"{\"x\":123}"))});
        let mut request=request.body(axum::body::Body::from_stream(stream)).unwrap();
        request.extensions_mut().insert(axum::extract::ConnectInfo("127.0.0.1:1234".parse::<std::net::SocketAddr>().unwrap()));
        let response=hangar_server::routes::router(Arc::new(state)).call(request).await.unwrap();
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"GUE-080: {row}");
        let bytes=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
        let body=serde_json::from_slice::<Value>(&bytes).unwrap_or_else(|_|Value::String(String::from_utf8(bytes.to_vec()).unwrap()));
        assert_eq!(body,row["response"],"GUE-080: {row}");
        assert_eq!(consumed.load(Ordering::SeqCst),row["consumed"].as_bool().unwrap(),"GUE-080: {row}");
    }
}

#[tokio::test]
async fn login_guests_sync_cookies_preserve_expiration_and_origin() {
    use hangar_server::login_guests::{Service, Settings};
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/cookies.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        let mut service = Service::new(Settings {
            guests: home.path().join("guests.json"), vault: home.path().join("vault.json"),
            runtime_config: home.path().join("config.json"), sync_enabled: true,
            bootstrap: String::new(), secret: b"fixture-secret".to_vec(), secure: false,
            rate_max: 10.into(), rate_window: 300.into(),
        });
        service.now = if row["now"] == 0 { || 0 } else { || 2000000000 };
        let mut request = axum::http::Request::builder()
            .method(row["method"].as_str().unwrap()).uri(row["path"].as_str().unwrap())
            .header("content-type", "application/json")
            .header("cookie", format!("cp_sync={}",row["cookie"].as_str().unwrap()));
        if let Some(site) = row["site"].as_str() { request = request.header("sec-fetch-site",site); }
        let response = Arc::new(service).respond(false,"127.0.0.1".into(),false,
            request.body(axum::body::Body::from(row["body"].to_string())).unwrap()).await;
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"cookie {}",row["name"]);
        assert_eq!(response.headers().get("set-cookie").map(|v|v.to_str().unwrap()),row["set_cookie"].as_str(),"renovação {}",row["name"]);
        let body=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(),row["response"],"resposta {}",row["name"]);
        assert!(!home.path().join("vault.json").exists(),"cofre não registrado permanece ausente");
    }
}

#[test]
fn login_guests_shortcut_placeholder_matches_python() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/placeholder.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        assert_eq!(hangar_server::shortcuts::has_placeholder(row["text"].as_str().unwrap()).as_deref(), row["result"].as_str(), "marcador {:?}",row["text"]);
    }
}

#[test]
fn login_guests_sync_settings_preserve_dotenv_coercion() {
    use hangar_server::login_guests::Settings;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/settings.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();
        let backend=home.path().join("backend");std::fs::create_dir(&backend).unwrap();
        std::fs::write(backend.join(".env"),format!("CP_{}={}\n",row["field"].as_str().unwrap().to_uppercase(),row["input"])).unwrap();
        let result=Settings::from_config(home.path(),&home.path().join("claude"),Some(&backend));
        assert_eq!(result.is_ok(),row["accepted"].as_bool().unwrap(),"configuração {row}");
        if let Ok(settings)=result {
            let value=match row["field"].as_str().unwrap() {
                "sync"=>serde_json::json!(settings.sync_enabled),
                "lan_bind_ip"=>{ assert_eq!(settings.secure,row["secure"].as_bool().unwrap()); row["value"].clone() },
                "sync_rate_max"=>serde_json::from_str::<Value>(&settings.rate_max.to_string()).unwrap(),
                "sync_rate_window"=>serde_json::from_str::<Value>(&settings.rate_window.to_string()).unwrap(),
                _=>unreachable!(),
            };
            assert_eq!(value,row["value"],"coerção {row}");
        }
    }
}

#[tokio::test]
async fn login_guests_sync_rate_preserves_fractional_clock_and_signed_limits() {
    use hangar_server::login_guests::{Service,Settings};
    use std::sync::atomic::{AtomicU64,Ordering};
    static CLOCK:AtomicU64=AtomicU64::new(0);
    fn epoch()->f64 { f64::from_bits(CLOCK.load(Ordering::SeqCst)) }
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/rate.json")).unwrap();
    let mut mismatches=Vec::new();
    for row in fixture["rows"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();
        let mut service=Service::new(Settings { guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),
            runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,
            rate_max:row["maximum"].to_string().parse().unwrap(),rate_window:row["window"].to_string().parse().unwrap() });
        service.now=||epoch() as u64;service.rate_now=epoch;let service=Arc::new(service);
        for step in row["steps"].as_array().unwrap() {
            CLOCK.store(step["now"].as_f64().unwrap().to_bits(),Ordering::SeqCst);
            let request=axum::http::Request::builder().method("POST").uri("/api/sync/login")
                .header("content-type","application/json").body(axum::body::Body::from(r#"{"user":"unknown","auth_hash":"wrong"}"#)).unwrap();
            let response=service.respond(false,step["ip"].as_str().unwrap().into(),false,request).await;
            let expected=if step["limited"]==true {429}else{401};
            if response.status().as_u16()!=expected {mismatches.push(format!("{} @{}: {} != {}",row["name"],step["now"],response.status(),expected));}
        }
        assert!(!home.path().join("vault.json").exists());
    }
    assert!(mismatches.is_empty(),"GUE-052: {mismatches:?}");
}

#[tokio::test]
async fn login_guests_sync_invalid_config_does_not_disable_local_guests() {
    use hangar_server::login_guests::Service;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/settings.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap().iter().filter(|r|r["accepted"]==false) {
        let home=tempfile::tempdir().unwrap();
        let backend=home.path().join("backend");std::fs::create_dir(&backend).unwrap();
        std::fs::write(backend.join(".env"),format!("CP_{}={}\n",row["field"].as_str().unwrap().to_uppercase(),row["input"])).unwrap();
        let service=Arc::new(Service::from_config(home.path(),&home.path().join("claude"),Some(&backend)));
        for (path,status) in [("/api/sync/status",503),("/api/sync/setup",503),("/api/me",200)] {
            let request=axum::http::Request::builder().uri(path).body(axum::body::Body::empty()).unwrap();
            let response=service.respond(true,"127.0.0.1".into(),false,request).await;
            assert_eq!(response.status().as_u16(),status,"D03 {path}");
            if status==503 {
                let body=axum::body::to_bytes(response.into_body(),4096).await.unwrap();
                assert_eq!(serde_json::from_slice::<Value>(&body).unwrap(),serde_json::json!({"detail":{"code":"sync_config_invalid","params":{},"msg":"configuração de sincronização inválida"}}));
            }
        }
        assert!(!home.path().join("claude/guests.json").exists());
        assert!(!home.path().join(".hangar/sync-vault.json").exists());
    }
}

#[test]
fn login_guests_surrogate_detection_matches_python_values() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/request-body.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap().iter().filter(|r|r["parser"]["accepted"]==true) {
        let hex=row["raw_hex"].as_str().unwrap();
        let bytes:Vec<u8>=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect();
        let value=hangar_server::orchestration::body::parse(&bytes).unwrap();
        assert_eq!(hangar_server::orchestration::codec::contains_surrogate(&value),row["parser"]["contains_surrogate"].as_bool().unwrap(),"bytes {hex}");
    }
}

#[tokio::test]
async fn login_guests_body_encoding_preserves_fastapi_response() {
    use hangar_server::login_guests::{Service,Settings};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/request-body.json")).unwrap();
    let mut differences=Vec::new();
    for (index,row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let home=tempfile::tempdir().unwrap();
        let service=Arc::new(Service::new(Settings { guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),
            runtime_config:home.path().join("config.json"),sync_enabled:false,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,
            rate_max:10.into(),rate_window:300.into() }));
        let hex=row["raw_hex"].as_str().unwrap();
        let bytes:Vec<u8>=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect();
        let mut request=axum::http::Request::builder().method("POST").uri("/api/sync/setup");
        if let Some(content_type)=row["content_type"].as_str() { request=request.header("content-type",content_type); }
        let response=service.respond(true,"127.0.0.1".into(),false,request.body(axum::body::Body::from(bytes)).unwrap()).await;
        let status=response.status().as_u16();let bytes=axum::body::to_bytes(response.into_body(),1024*1024).await.unwrap();
        let actual:Value=serde_json::from_slice(&bytes).unwrap_or_else(|_|Value::String(String::from_utf8(bytes.to_vec()).unwrap()));
        if status!=row["status"].as_u64().unwrap() as u16 || actual!=row["response"] {
            differences.push(format!("caso {index}: status {status}; corpo diferente={}",actual!=row["response"]));
        }
        assert!(!home.path().join("vault.json").exists());
    }
    assert!(differences.is_empty(),"GUE-040/054: {differences:?}");
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
async fn login_guests_vault_unicode_preserves_write_and_serialization_effects() {
    use hangar_server::{login_guests::{Service, Settings}, orchestration::codec};
    let fixture = codec::loads(include_str!("../../../../backend/tests/fixtures/contract/guests/vault-unicode.json")).unwrap();
    let mut differences = Vec::new();
    for (index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let home = tempfile::tempdir().unwrap();
        let mut service = Service::new(Settings { guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),
            runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:"fixture-bootstrap".into(),secret:b"fixture-secret".to_vec(),secure:false,
            rate_max:10.into(),rate_window:300.into() });
        service.now = || 2000000000;
        let service = Arc::new(service);
        assert_eq!(sync_request(&service,"POST","/api/sync/register",r#"{"bootstrap":"fixture-bootstrap","user":"ana","salt":"fixture","auth_hash":"fixture"}"#,None).await.0,200);
        let login = sync_request(&service,"POST","/api/sync/login",r#"{"user":"ana","auth_hash":"fixture"}"#,None).await;
        assert_eq!(login.0,200);
        let cookie = login.2.unwrap();
        for (name, method, raw) in [("put","PUT",row["raw_body"].as_str().unwrap()),("get","GET",""),("stale","PUT",r#"{"enc_blob":null,"base_rev":0}"#)] {
            let (status, response, _) = sync_request(&service,method,"/api/sync/vault",raw,Some(&cookie)).await;
            if status != row[name]["status"].as_u64().unwrap() as u16 || response != row[name]["response"] { differences.push(format!("{index}/{name}")); }
        }
        let raw_disk = std::fs::read_to_string(home.path().join("vault.json")).unwrap();
        assert!(!codec::contains_surrogate(&Value::String(raw_disk.clone())), "marcador privado não pode persistir");
        let disk = codec::loads(&raw_disk).unwrap();
        if disk["enc_blob"] != row["disk_blob"] || disk["rev"] != row["disk_rev"] { differences.push(format!("{index}/disk")); }
    }
    assert!(differences.is_empty(),"cofre e serialização: {differences:?}");
}

#[tokio::test]
async fn login_guests_admin_counter_preserves_parser_priority_and_shared_bucket() {
    use hangar_server::login_guests::{Service,Settings};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/admin-rate.json")).unwrap();
    let mut differences=Vec::new();
    for row in fixture["rows"].as_array().unwrap() {
        let home=tempfile::tempdir().unwrap();
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
        let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"fixture".into(),auth_token:"owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
        state.auth=hangar_server::auth::Auth::new("owner");state.share=None;
        let service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:false,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
        service.store.create("Ana",home.path().to_str().unwrap(),false,true,"fixture".into(),"guest-token".into()).unwrap();
        state.login_guests=Arc::new(service);
        let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
        let client=reqwest::Client::new();
        for (index,step) in row["steps"].as_array().unwrap().iter().enumerate() {
            let mut request=client.request(reqwest::Method::from_bytes(step["method"].as_str().unwrap().as_bytes()).unwrap(),format!("http://{address}{}",step["path"].as_str().unwrap()))
                .bearer_auth(step["token"].as_str().unwrap()).header("x-forwarded-for","198.51.100.7");
            if let Some(raw)=step["raw"].as_str(){ request=request.header("content-type","application/json").body(raw.to_owned()); }
            let response=request.send().await.unwrap();let status=response.status().as_u16();let retry=response.headers().get("retry-after").map(|v|v.to_str().unwrap().to_owned());
            let body:Value=response.json().await.unwrap();
            if status!=step["status"].as_u64().unwrap() as u16 || body!=step["response"] || retry.as_deref()!=step["retry_after"].as_str(){ differences.push(format!("{}/{index}: {status}",row["name"])); }
        }
        task.abort();
    }
    assert!(differences.is_empty(),"contador HTTP: {differences:?}");
}

#[tokio::test]
async fn login_guests_stored_base64_preserves_permissive_decode() {
    use hangar_server::login_guests::{Service,Settings};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/stored-base64.json")).unwrap();
    let mut differences=Vec::new();
    for (index,row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let home=tempfile::tempdir().unwrap();
        let vault=home.path().join("vault.json");let original=row["vault"].to_string();std::fs::write(&vault,&original).unwrap();
        let mut service=Service::new(Settings{guests:home.path().join("guests.json"),vault:vault.clone(),runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:String::new(),secret:b"fixture-secret".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
        service.now=||2000000000;let service=Arc::new(service);
        for (name,method,path,raw) in [("prelogin","GET","/api/sync/prelogin?user=unknown",""),("login","POST","/api/sync/login",r#"{"user":"ana","auth_hash":"fixture-auth"}"#)] {
            let (status,body,cookie)=sync_request(&service,method,path,raw,None).await;
            let expected_status=row[name]["status"].as_u64().unwrap() as u16;
            // D03 conserva status/arquivo e substitui somente o texto opaco do erro interno.
            let body_matches=if expected_status==500 { body["detail"]["code"]=="sync_vault_unreadable" } else { body==row[name]["response"] };
            if status!=expected_status || !body_matches || name=="login" && cookie.as_deref()!=row["set_cookie"].as_str() { differences.push(format!("{index}/{name}: {status}")); }
        }
        assert_eq!(std::fs::read_to_string(vault).unwrap(),original,"arquivo preservado {index}");
    }
    assert!(differences.is_empty(),"base64 persistido: {differences:?}");
}

#[tokio::test]
async fn login_guests_public_methods_preserve_routing_and_head() {
    use hangar_server::login_guests::{Service,Settings};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/methods.json")).unwrap();
    let home=tempfile::tempdir().unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"fixture".into(),auth_token:"owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.auth=hangar_server::auth::Auth::new("owner");state.share=None;
    let config=home.path().join("config.json");
    let service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:config.clone(),sync_enabled:true,bootstrap:String::new(),secret:b"fixture".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
    service.store.create("Ana",home.path().to_str().unwrap(),false,true,"fixture".into(),"guest-token".into()).unwrap();state.login_guests=Arc::new(service);
    let task=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap()});
    let client=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();let mut differences=Vec::new();
    for (index,row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        match std::fs::remove_file(&config){Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>panic!("{e}")}
        let response=client.request(reqwest::Method::from_bytes(row["method"].as_str().unwrap().as_bytes()).unwrap(),format!("http://{address}{}",row["path"].as_str().unwrap()))
            .bearer_auth(row["token"].as_str().unwrap()).header("content-type","application/json").body("{}").send().await.unwrap();
        let status=response.status().as_u16();let headers=response.headers().clone();let raw=response.text().await.unwrap();
        let body=serde_json::from_str::<Value>(&raw).unwrap_or(Value::String(raw));
        let fields=[("allow","allow"),("content_type","content-type"),("content_length","content-length"),("location","location")];
        let mut mismatched=status!=row["status"].as_u64().unwrap() as u16 || body!=row["response"];
        for (key,header) in fields {
            let value=headers.get(header).map(|v|v.to_str().unwrap().replace(&format!("http://{address}"),"⟦BASE⟧"));
            mismatched |= value.as_deref()!=row[key].as_str();
        }
        if mismatched { differences.push(format!("{index}: {} {} {} ->{status}",row["method"],row["path"],row["token"])); }
    }
    task.abort();
    assert!(differences.is_empty(),"{} divergências; primeiras: {:?}",differences.len(),&differences[..differences.len().min(30)]);
}

#[tokio::test]
async fn login_guests_vault_shapes_preserve_each_operation() {
    use hangar_server::login_guests::{Service,Settings};
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/vault-shapes.json")).unwrap();
    let mut differences=Vec::new();
    for (index,row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let home=tempfile::tempdir().unwrap();let vault=home.path().join("vault.json");
        let mut service=Service::new(Settings{guests:home.path().join("guests.json"),vault:vault.clone(),runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:"bootstrap".into(),secret:b"fixture-secret".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
        service.now=||2000000000;service.entropy=|n|vec![b'x';n];let service=Arc::new(service);
        for (case_index,case) in row["cases"].as_array().unwrap().iter().enumerate() {
            std::fs::write(&vault,row["initial"].to_string()).unwrap();
            let cookie=format!("cp_sync={}",fixture["cookie"].as_str().unwrap());
            let (status,body,_)=sync_request(&service,case["method"].as_str().unwrap(),case["path"].as_str().unwrap(),&case["body"].to_string(),Some(&cookie)).await;
            let body_matches=if case["status"]==500 {body["detail"]["code"]=="sync_vault_unreadable"}else{body==case["response"]};
            let disk:Value=serde_json::from_slice(&std::fs::read(&vault).unwrap()).unwrap();
            if status!=case["status"].as_u64().unwrap() as u16 || !body_matches || disk!=case["disk"] {differences.push(format!("{index}/{case_index}: {status}"));}
        }
    }
    assert!(differences.is_empty(),"cofre legado: {differences:?}");
}

#[tokio::test]
async fn login_guests_cookie_escaping_matches_original_login_and_renewal() {
    use hangar_server::login_guests::{Service,Settings};
    use serde_json::json;
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/login-cookies.json")).unwrap();
    let mut differences=Vec::new();
    for (index,row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let home=tempfile::tempdir().unwrap();
        let mut service=Service::new(Settings{guests:home.path().join("guests.json"),vault:home.path().join("vault.json"),runtime_config:home.path().join("config.json"),sync_enabled:true,bootstrap:"bootstrap".into(),secret:b"fixture-secret".to_vec(),secure:false,rate_max:10.into(),rate_window:300.into()});
        service.now=||2000000000;service.entropy=|n|vec![b'x';n];let service=Arc::new(service);
        let register=json!({"user":row["user"],"auth_hash":"fixture","salt":"fixture","bootstrap":"bootstrap"}).to_string();
        assert_eq!(sync_request(&service,"POST","/api/sync/register",&register,None).await.0,200);
        let login=json!({"user":row["user"],"auth_hash":"fixture"}).to_string();
        let (status,body,cookie)=sync_request(&service,"POST","/api/sync/login",&login,None).await;
        if status!=row["login"]["status"].as_u64().unwrap() as u16 || body!=row["login"]["response"] || cookie.as_deref()!=row["login"]["set_cookie"].as_str() { differences.push(format!("{index}/login: {status}")); }
        let cookie=cookie.map(|v|v.split(';').next().unwrap().to_owned());
        let (status,body,cookie)=sync_request(&service,"GET","/api/sync/vault","",cookie.as_deref()).await;
        if status!=row["vault"]["status"].as_u64().unwrap() as u16 || body!=row["vault"]["response"] || cookie.as_deref()!=row["vault"]["set_cookie"].as_str() { differences.push(format!("{index}/vault: {status}")); }
    }
    assert!(differences.is_empty(),"cookies: {differences:?}");
}
