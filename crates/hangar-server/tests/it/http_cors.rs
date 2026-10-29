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
use axum::{body::{Body,to_bytes},http::{HeaderMap,HeaderName,HeaderValue,Method},response::Response};
use serde_json::Value;

#[tokio::test]
async fn cors_matches_original_preflight_errors_cookies_and_body_limit_order(){
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/cors.json")).unwrap();
    for (index,row) in fixture["cases"].as_array().unwrap().iter().enumerate(){
        let mut headers=HeaderMap::new();
        for (key,value) in row["headers"].as_object().unwrap(){headers.insert(HeaderName::from_bytes(key.as_bytes()).unwrap(),HeaderValue::from_str(value.as_str().unwrap()).unwrap());}
        let method:Method=row["method"].as_str().unwrap().parse().unwrap();
        let response=if let Some(response)=hangar_server::http_cors::preflight(&method,&headers){response}else{
            let mut response=Response::builder().status(row["base"]["status"].as_u64().unwrap() as u16).body(Body::from(row["base"]["body"].as_str().unwrap().to_owned())).unwrap();
            for (key,value) in row["base"]["headers"].as_object().unwrap(){response.headers_mut().insert(HeaderName::from_bytes(key.as_bytes()).unwrap(),HeaderValue::from_str(value.as_str().unwrap()).unwrap());}
            hangar_server::http_cors::apply(&headers,&mut response);response
        };
        assert_eq!(response.status().as_u16(),row["expected"]["status"].as_u64().unwrap() as u16,"caso {index}");
        let actual:serde_json::Map<String,Value>=response.headers().iter().map(|(key,value)|(key.to_string(),Value::String(value.to_str().unwrap().into()))).collect();
        assert_eq!(Value::Object(actual),row["expected"]["headers"],"caso {index}");
        assert_eq!(to_bytes(response.into_body(),1024*1024).await.unwrap().as_ref(),row["expected"]["body"].as_str().unwrap().as_bytes(),"caso {index}");
    }
}
