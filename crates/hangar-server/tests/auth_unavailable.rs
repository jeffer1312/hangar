//! Binário próprio: o teste liga no ambiente do processo um contador compartilhado que não abre.
#[path = "it/fake/mod.rs"]
mod fake;
use fake::{OWNER, client, config, spawn_fake, spawn_server_for};

/// R5-03 (`auth.py:151-152`, `hangar-core/src/auth.rs:177`): contador indisponível é 503 com o código,
/// para quem não é o dono e também para a senha certa, e nada chega ao Python.
#[tokio::test]
async fn r5_03_unavailable_counter_is_503_from_rust() {
    // SAFETY: único teste deste binário; nenhuma outra thread lê o ambiente ainda.
    unsafe { std::env::set_var("HANGAR_AUTH_CONTEXT", "{}") };
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let expected = serde_json::json!({"detail": {"error_code": "auth_store_invalid", "message": "autenticação indisponível"}});
    for token in [None, Some("errado"), Some(OWNER)] {
        let put = client().put(format!("http://{srv}/api/sessions/prewarm")).header("content-type", "application/json").body("{}");
        let put = match token { Some(t) => put.bearer_auth(t), None => put };
        let response = put.send().await.unwrap();
        assert_eq!(response.status().as_u16(), 503, "token {token:?}");
        assert_eq!(response.json::<serde_json::Value>().await.unwrap(), expected, "token {token:?}");
    }
    assert_eq!(python.hits_to("/api/sessions/prewarm"), 0);
}
