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
    let preflight = client.request(reqwest::Method::OPTIONS, &url)
        .header("origin", "https://client.invalid").header("access-control-request-method", "GET")
        .send().await.unwrap();
    assert!(preflight.status().is_success());
    assert_eq!(preflight.headers()["access-control-allow-origin"], "https://client.invalid");
    server.abort();
    let _ = server.await;
}
