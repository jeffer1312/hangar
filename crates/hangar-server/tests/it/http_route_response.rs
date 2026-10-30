use axum::{body::{Body,to_bytes},http::{HeaderMap,Method,Uri},response::Response};
use hangar_server::{http_contract::Decision,http_route_response};
#[tokio::test]
async fn head_preserves_content_length_after_a_gate_refusal(){
    let mut response=Response::builder().status(403).header("content-type","application/json").body(Body::from("{\"detail\":\"denied\"}")).unwrap();
    http_route_response::suppress_head(&Method::HEAD,&mut response);
    assert_eq!(response.headers()["content-length"],"19");assert!(to_bytes(response.into_body(),1024).await.unwrap().is_empty());
}
#[test]
fn redirect_scheme_is_supplied_by_the_trusted_resolution(){
    let mut headers=HeaderMap::new();headers.insert("host","server.invalid".parse().unwrap());headers.insert("x-forwarded-proto","https".parse().unwrap());
    let decision=Decision::Redirect("/path".into());let uri:Uri="/path/?q=1".parse().unwrap();
    let response=http_route_response::response(&decision,&Method::GET,&uri,&headers,false,"fallback.invalid").unwrap();assert_eq!(response.headers()["location"],"http://server.invalid/path?q=1");
    let response=http_route_response::response(&decision,&Method::GET,&uri,&headers,true,"fallback.invalid").unwrap();assert_eq!(response.headers()["location"],"https://server.invalid/path?q=1");
}
