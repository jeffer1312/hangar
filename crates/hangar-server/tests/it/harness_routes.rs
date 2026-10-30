use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, routes::AppState};
use serde_json::Value;
use std::{sync::Arc, time::Duration};

#[tokio::test]
async fn harness_admission_and_install_status_are_native() {
    let home = tempfile::tempdir().unwrap();
    let mut state = AppState::new(crate::fake::config(crate::refused_address(), "127.0.0.1"));
    state.accounts = AccountService::new(AccountEnvironment::from_map([
        ("HOME".into(), home.path().display().to_string()),
        ("USERPROFILE".into(), home.path().display().to_string()),
    ].into()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, hangar_server::routes::router(Arc::new(state))
            .into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();
    });
    let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let url = format!("http://{address}/api/harness/instalar");
    let denied = client.get(&url).header("x-forwarded-for", "192.0.2.11").send().await.unwrap();
    assert_eq!(denied.status(), 401);
    let status = client.get(&url).bearer_auth(crate::fake::OWNER).send().await.unwrap();
    assert_eq!(status.status(), 200);
    assert_eq!(status.json::<Value>().await.unwrap()["fase"], "ocioso");
    let unsupported = client.post(format!("{url}/desconhecido")).bearer_auth(crate::fake::OWNER)
        .send().await.unwrap();
    assert_eq!(unsupported.status(), 400);
    assert_eq!(unsupported.json::<Value>().await.unwrap()["detail"]["code"], "erro_harness_sem_instalador");
    let wrong = client.delete(&url).send().await.unwrap();
    assert_eq!(wrong.status(), 405);
    let redirect = client.post(format!("http://{address}/api/harness/conserto")).send().await.unwrap();
    assert_eq!(redirect.status(), 307);
    let nested = client.post(format!("http://{address}/api/harness/conserto/sem/atalho"))
        .bearer_auth(crate::fake::OWNER).send().await.unwrap();
    assert_eq!(nested.status(), 400);
    assert_eq!(nested.json::<Value>().await.unwrap()["detail"]["code"], "erro_harness_conserto");
    let preflight = client.request(reqwest::Method::OPTIONS, &url)
        .header("origin", "https://client.invalid").header("access-control-request-method", "GET")
        .send().await.unwrap();
    assert!(preflight.status().is_success());
    assert_eq!(preflight.headers()["access-control-allow-origin"], "*");
    server.abort();
    let _ = server.await;
}

#[tokio::test]
async fn codex_integration_routes_share_the_service_used_by_config_sync() {
    let home=tempfile::tempdir().unwrap();std::fs::create_dir(home.path().join(".claude")).unwrap();
    std::fs::write(home.path().join(".claude/runtime-config.json"),br#"{"codex_sync":false,"codex_memory_import":true}"#).unwrap();
    let mut state=AppState::new(crate::fake::config(crate::refused_address(),"127.0.0.1"));
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().display().to_string()),("USERPROFILE",home.path().display().to_string()),("PATH",String::new())]));
    let state=Arc::new(state);let service=state.codex_setup_service().unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();let shared=state.clone();
    let server=tokio::spawn(async move{axum::serve(listener,hangar_server::routes::router(shared).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let client=reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(5)).build().unwrap();let url=format!("http://{address}/api/harness/codex/integracao");
    for suffix in ["","/sessao"] {
        let response=client.post(format!("{url}{suffix}")).header("x-forwarded-for","192.0.2.11").send().await.unwrap();assert_eq!(response.status(),401);
    }
    let _ = service.update_and_wait("config_sync",true).await;
    let response=client.get(&url).bearer_auth(crate::fake::OWNER).send().await.unwrap();assert_eq!(response.status(),200);
    let status:Value=response.json().await.unwrap();assert_eq!(status,service.switches(service.status().unwrap()));assert_eq!(status["automatica"],false);assert_eq!(status["memoria"],true);
    for suffix in ["/sessao",""] {
        let response=client.post(format!("{url}{suffix}")).bearer_auth(crate::fake::OWNER).body("corpo ignorado como no contrato").send().await.unwrap();assert_eq!(response.status(),202);
        let status:Value=response.json().await.unwrap();assert_eq!(status["automatica"],false);assert_eq!(status["memoria"],true);
    }
    assert!(Arc::ptr_eq(&service,&state.codex_setup_service().unwrap()));
    service.shutdown().await.unwrap();let response=client.post(&url).bearer_auth(crate::fake::OWNER).send().await.unwrap();assert_eq!(response.status(),500);
    server.abort();let _=server.await;
}
