//! Sonda das rotas de modelo sem lifespan nem servidor HTTP: só o multiplexador e as pastas indicadas.
//! O stdin recebe {method,path,body}; HANGAR_LIST_DIRS e HOME apontam para a prova isolada.
use axum::{
    body::{Body, to_bytes},
    extract::Request,
    http::Method,
};
use hangar_server::{config::Config, provider_models, routes::AppState};
use serde_json::{Value, json};
use std::{io::Read, sync::Arc};

#[tokio::main]
async fn main() {
    let mut text = String::new();
    std::io::stdin()
        .read_to_string(&mut text)
        .expect("pedido da sonda");
    let input: Value = serde_json::from_str(&text).expect("pedido JSON");
    let method: Method = input["method"]
        .as_str()
        .unwrap_or("GET")
        .parse()
        .expect("método HTTP");
    let path = input["path"].as_str().expect("caminho HTTP");
    assert!(
        provider_models::matches(&method, path.split('?').next().unwrap())
            || provider_models::catalog_request(
                &method,
                path.split('?').next().unwrap(),
                path.split_once('?').map(|(_, q)| q)
            ),
        "a sonda só atende as rotas de modelo migradas"
    );
    let config = Config::from_lookup(|key| match key {
        "HANGAR_SERVER_LISTEN" => Some("127.0.0.1:18765".into()),
        "HANGAR_SERVER_UPSTREAM" => Some("127.0.0.1:1".into()),
        "HANGAR_INTERNAL_SECRET" => Some("fixture-internal".into()),
        "CP_AUTH_TOKEN" => Some("fixture-owner".into()),
        _ => None,
    })
    .expect("configuração da sonda");
    let state = Arc::new(AppState::new(config));
    let body = input.get("body").map(Value::to_string).unwrap_or_default();
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = provider_models::serve(&state, request).await;
    let status = response.status().as_u16();
    let bytes = to_bytes(response.into_body(), 8 * 1024 * 1024)
        .await
        .expect("resposta da sonda");
    let value: Value = serde_json::from_slice(&bytes).expect("resposta JSON");
    println!("{}", json!({"status":status,"body":value}));
}
