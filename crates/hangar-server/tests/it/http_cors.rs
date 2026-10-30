use hangar_server::{accounts::{AccountService,environment::AccountEnvironment},config::Config,routes::AppState,auth::TrustedHosts};
use std::sync::Arc;

#[tokio::test]
async fn cors_preflight_precedes_owner_authentication(){
    let home=tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".hangar/config")).unwrap();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let mut state=AppState::new(Config{listen:address,upstream:crate::refused_address(),internal_secret:"contract-internal".into(),auth_token:"contract-owner".into(),log_path:None,trusted:TrustedHosts::parse("127.0.0.1")});
    state.accounts=AccountService::new(AccountEnvironment::from_pairs([("HOME",home.path().to_str().unwrap()),("USERPROFILE",home.path().to_str().unwrap())]));
    let router=hangar_server::routes::router(Arc::new(state));
    let task=tokio::spawn(async move{axum::serve(listener,router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap();});
    let response=reqwest::Client::new().request(reqwest::Method::OPTIONS,format!("http://{address}/api/peers"))
        .header("Origin","https://client.invalid").header("Access-Control-Request-Method","GET").send().await.unwrap();
    let status=response.status();let body=response.text().await.unwrap();task.abort();
    assert_eq!(status.as_u16(),200,"{body}");assert_eq!(body,"OK");
}
