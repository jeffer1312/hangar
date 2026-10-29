//! Falhas externas ao middleware não recebem os cabeçalhos dos erros tratados.
use axum::{body::{Body, to_bytes}, http::{HeaderMap, HeaderValue}, response::Response};
use hangar_server::http_cors::{self, UnhandledServerError};
use serde_json::{Value, json};

#[tokio::test]
async fn cors_unhandled_errors_match_python() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/engines/unhandled-cors.json")).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("origin", HeaderValue::from_static("https://fixture.invalid"));
    for case in fixture["cases"].as_array().unwrap() {
        let base = &case["base"];
        let mut response = Response::builder().status(base["status"].as_u64().unwrap() as u16);
        for (name, value) in base["headers"].as_object().unwrap() {
            response = response.header(name, value.as_str().unwrap());
        }
        let mut response = response.body(Body::from(base["body"].as_str().unwrap().to_owned())).unwrap();
        if case["unhandled"] == true { response.extensions_mut().insert(UnhandledServerError); }
        http_cors::apply(&headers, &mut response);
        let status = response.status().as_u16();
        let actual_headers: serde_json::Map<String, Value> = response.headers().iter().map(|(k,v)| (k.to_string(), json!(v.to_str().unwrap()))).collect();
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        assert_eq!(json!({"status":status,"headers":actual_headers,"body":String::from_utf8(body.to_vec()).unwrap()}), case["expected"], "{}", case["name"]);
    }
}
