//! `PUT /api/sessions/prewarm` e `DELETE /api/sessions/prewarm/{id}`: só o dono, no Rust; o Connect pela ponte.
use serde_json::{Value, json};

use crate::fake::*;

const REFUSED: &str = r#"{"cwd":"/nao/existe","provider":"claude","headless":true}"#;

async fn body(response: reqwest::Response) -> Value { serde_json::from_str(&response.text().await.unwrap()).unwrap() }

fn with_client(body: &str) -> String {
    let mut value: Value = serde_json::from_str(body).unwrap();
    value["client_id"] = json!("tela-1");
    value.to_string()
}

/// R5-01 (decisão 5): quem não é o dono recebe do Rust o 401 do `require_auth` (`test_needs_auth`), com
/// CORS, e o Python não recebe nada.
#[tokio::test]
async fn r5_01_not_the_owner_is_401_from_rust() {
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let refused = json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}});
    for token in [None, Some("convidado")] {
        let put = client().put(format!("http://{srv}/api/sessions/prewarm")).header("origin", "http://outro")
            .header("content-type", "application/json").body(with_client(REFUSED));
        let put = match token { Some(t) => put.bearer_auth(t), None => put };
        let response = put.send().await.unwrap();
        assert_eq!(response.status().as_u16(), 401, "token {token:?}");
        assert!(response.headers().contains_key("access-control-allow-origin"), "o PWA de outra origem lê a recusa");
        assert_eq!(response.json::<Value>().await.unwrap(), refused, "token {token:?}");
        let delete = client().delete(format!("http://{srv}/api/sessions/prewarm/abc"));
        let delete = match token { Some(t) => delete.bearer_auth(t), None => delete };
        assert_eq!(delete.send().await.unwrap().status().as_u16(), 401, "token {token:?}");
    }
    assert_eq!(python.hits_to("/api/sessions/prewarm"), 0);
    assert_eq!(python.hits_to("/api/sessions/prewarm/abc"), 0);
}

/// R5-02/R5-04 (`test_backoff_barra_apos_max_fails`): oito erros da mesma origem dão 401 e o nono 429 com
/// `Retry-After: 30`, sem avaliar a senha, nem a certa; outra origem segue livre. Tudo no Rust.
#[tokio::test]
async fn r5_02_eight_wrong_tokens_block_the_origin_in_rust() {
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let put = |token: &str, origin: &str| client().put(format!("http://{srv}/api/sessions/prewarm")).bearer_auth(token)
        .header("x-forwarded-for", origin).header("content-type", "application/json").body(with_client(REFUSED)).send();
    for attempt in 0..8 { assert_eq!(put("errado", "203.0.113.9").await.unwrap().status().as_u16(), 401, "tentativa {attempt}"); }
    let blocked = put("errado", "203.0.113.9").await.unwrap();
    assert_eq!(blocked.status().as_u16(), 429);
    assert_eq!(blocked.headers()["retry-after"], "30");
    assert_eq!(blocked.json::<Value>().await.unwrap()["detail"]["code"], "erro_muitas_tentativas");
    assert_eq!(put(OWNER, "203.0.113.9").await.unwrap().status().as_u16(), 429, "bloqueada, nem a senha certa entra");
    assert_eq!(put("errado", "203.0.113.10").await.unwrap().status().as_u16(), 401, "outra origem não é afetada");
    assert_eq!(python.hits_to("/api/sessions/prewarm"), 0);
}

/// R5-11: o cookie só autentica `GET`/`HEAD`; num `PUT` ele não vale, e a recusa é do Rust.
#[tokio::test]
async fn r5_11_cookie_on_a_write_is_401_from_rust() {
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let response = client().put(format!("http://{srv}/api/sessions/prewarm")).header("cookie", format!("cp_token={OWNER}"))
        .header("content-type", "application/json").body(with_client(REFUSED)).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 401);
    assert_eq!(python.hits_to("/api/sessions/prewarm"), 0);
}

/// A recusa é a da abertura: mesmo status e mesmo corpo do `POST /api/sessions` para o mesmo pedido.
#[tokio::test]
async fn a_refused_reserve_answers_like_the_open() {
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let send = |method: reqwest::Method, path: &'static str, body: String| client().request(method, format!("http://{srv}{path}"))
        .bearer_auth(OWNER).header("content-type", "application/json").body(body).send();
    let open = send(reqwest::Method::POST, "/api/sessions", REFUSED.into()).await.unwrap();
    let reserve = send(reqwest::Method::PUT, "/api/sessions/prewarm", with_client(REFUSED)).await.unwrap();
    assert_eq!(reserve.status(), open.status());
    assert_eq!(reserve.status(), 400);
    let (open, reserve) = (body(open).await, body(reserve).await);
    assert_eq!(reserve["detail"]["code"], "erro_cwd_inexistente");
    assert_eq!(reserve, open);

    // Corpo torto é 422 `erro_corpo_invalido`: sem client_id, com nome, com prompt, campo desconhecido.
    for raw in [REFUSED.to_owned(), with_client(r#"{"cwd":"/x","name":"s1"}"#), with_client(r#"{"cwd":"/x","initial_prompt":"oi"}"#),
                with_client(r#"{"cwd":"/x","bogus":1}"#), "nem json".to_owned()] {
        let resp = send(reqwest::Method::PUT, "/api/sessions/prewarm", raw.clone()).await.unwrap();
        assert_eq!(resp.status(), 422, "{raw}");
        assert_eq!(body(resp).await["detail"]["code"], "erro_corpo_invalido", "{raw}");
    }
    // Fora do escopo da reserva: o código próprio dela.
    let terminal = send(reqwest::Method::PUT, "/api/sessions/prewarm", with_client(r#"{"cwd":"/x","headless":false}"#)).await.unwrap();
    assert_eq!(terminal.status(), 422);
    assert_eq!(body(terminal).await["detail"]["code"], "erro_prewarm_nao_suportado");

    let gone = client().delete(format!("http://{srv}/api/sessions/prewarm/nao-existe")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(gone.status(), 204, "inexistente também é 204");
    assert_eq!(python.hits_to("/api/sessions/prewarm"), 0);
}

