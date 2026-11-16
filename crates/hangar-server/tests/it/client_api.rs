//! `GET /api/client-api`: a versão da API que o app nativo confere; só o dono, atendida no Rust.
use crate::fake::*;

use serde_json::{Value, json};

#[tokio::test]
async fn owner_gets_the_client_api_and_others_get_401_without_python() {
    let (python, _) = spawn_fake().await;
    let public = spawn_state_for(&python, hangar_server::routes::AppState::new(config("127.0.0.1"))).await;
    let url = format!("http://{public}/api/client-api");

    let r = client().get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(body, json!({"api": hangar_api::CLIENT_API}));

    for token in [None, Some("convidado")] {
        let mut req = client().get(&url);
        if let Some(token) = token { req = req.bearer_auth(token); }
        assert_eq!(req.send().await.unwrap().status(), 401, "token {token:?}");
    }
}

