//! R5 (decisão 5): quem não é o dono numa rota só do dono recebe a recusa do Rust, contando a tentativa,
//! também nas rotas que o Python nunca teve (lá era 404/405 sem autenticar). Nada chega ao Python.
use serde_json::{Value, json};

use crate::fake::*;

const REFUSED: &str = r#"{"detail":{"code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}"#;

async fn status_and_body(response: reqwest::Response) -> (u16, Value) {
    let status = response.status().as_u16();
    (status, serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null))
}

/// R5-07: `/api/presence` só existe no Rust; sem o dono, 401 em vez do 404 do FastAPI.
#[tokio::test]
async fn r5_07_rust_only_route_refuses_the_stranger_with_401() {
    let (python, upstream) = spawn_fake().await;
    let srv = spawn_server(config(upstream, "127.0.0.1")).await;
    let refused: Value = serde_json::from_str(REFUSED).unwrap();
    let get = client().get(format!("http://{srv}/api/presence")).bearer_auth("errado").send().await.unwrap();
    assert_eq!(status_and_body(get).await, (401, refused.clone()));
    let post = client().post(format!("http://{srv}/api/presence")).header("content-type", "application/json")
        .body(json!({"mode": "auto"}).to_string()).send().await.unwrap();
    assert_eq!(status_and_body(post).await, (401, refused));
    assert_eq!(python.hits_to("/api/presence"), 0);
}

/// R5-08: `POST /api/sessions/{n}/conta` (o Python só tinha o `GET`): sem o dono, 401 em vez do 405.
#[tokio::test]
async fn r5_08_method_the_python_never_had_is_401_for_the_stranger() {
    let (python, upstream) = spawn_fake().await;
    let srv = spawn_server(config(upstream, "127.0.0.1")).await;
    let post = client().post(format!("http://{srv}/api/sessions/s0/conta")).header("content-type", "application/json")
        .body("{}").send().await.unwrap();
    assert_eq!(status_and_body(post).await, (401, serde_json::from_str::<Value>(REFUSED).unwrap()));
    let get = client().get(format!("http://{srv}/api/sessions/s0/conta")).send().await.unwrap();
    assert_eq!(get.status().as_u16(), 401, "o `GET` que o Python tinha: o mesmo 401");
    assert_eq!(python.hits_to("/api/sessions/s0/conta"), 0);
}
