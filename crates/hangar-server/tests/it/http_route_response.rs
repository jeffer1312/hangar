use axum::{body::{Body,to_bytes},http::{HeaderMap,Method,Uri},response::Response};
use hangar_server::{http_contract::Decision,http_route_response};
use serde_json::Value;
#[tokio::test]
async fn routing_responses_match_python_including_encoded_locations_and_head(){
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/update-git/http-responses.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap(){
        let decision=match row["status"].as_u64().unwrap(){404=>Decision::NotFound,405=>Decision::MethodNotAllowed(row["allow"].as_str().unwrap().into()),307=>Decision::Redirect(row["redirect_path"].as_str().unwrap().into()),_=>unreachable!()};
        let method:Method=row["method"].as_str().unwrap().parse().unwrap();let uri:Uri=row["path"].as_str().unwrap().parse().unwrap();
        let mut headers=HeaderMap::new();headers.insert("host","server.invalid".parse().unwrap());
        let response=http_route_response::response(&decision,&method,&uri,&headers,false,"fallback.invalid").unwrap();
        assert_eq!(response.status().as_u16(),row["status"].as_u64().unwrap() as u16,"{row}");
        for (name,field) in [("allow","allow"),("content-type","content_type"),("content-length","content_length")]{assert_eq!(response.headers().get(name).and_then(|v|v.to_str().ok()),row[field].as_str(),"{name}: {row}");}
        let location=response.headers().get("location").and_then(|v|v.to_str().ok()).map(|v|v.replace("http://server.invalid","⟦BASE⟧"));assert_eq!(location.as_deref(),row["location"].as_str(),"{row}");
        let expected=if let Some(body)=row["response"].as_str(){body.to_owned()}else{row["response"].to_string()};
        assert_eq!(to_bytes(response.into_body(),1024*1024).await.unwrap().as_ref(),expected.as_bytes(),"{row}");
    }
}
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
