//! Páginas da conversa: publicação pela porta privada, leitura do dono no Rust, convidado no Python.
use crate::fake;

use std::net::SocketAddr;
use std::sync::Arc;

use fake::*;
use hangar_server::pages::store::Store;
use serde_json::{Value, json};

struct Scene { python: Arc<Fake>, public: SocketAddr, private: String, _dir: tempfile::TempDir }

/// Servidor com a sessão `s1` (chave `k`), páginas numa pasta temporária e sem Chromium.
async fn scene() -> Scene {
    let (python, upstream) = spawn_fake().await;
    python.set_info(json!({"provider": "claude", "jsonl": "/t/k.jsonl", "session_key": "k", "history": {}}));
    let dir = tempfile::tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(config(upstream, "127.0.0.1"));
    state.pages = Arc::new(Store::new(dir.path().into()));
    state.chromium = || None;
    let public = spawn_state(state).await;
    let health: Value = serde_json::from_str(&client().get(format!("http://{public}/__hangar_server/health")).send().await.unwrap().text().await.unwrap()).unwrap();
    let private = health["terminal_address"].as_str().unwrap().to_owned();
    Scene { python, public, private, _dir: dir }
}

async fn publish(s: &Scene, body: Value) -> Value {
    let r = client().post(format!("http://{}/__hangar_server/pages", s.private)).header("x-hangar-internal", SECRET)
        .header("content-type", "application/json").body(body.to_string()).send().await.unwrap();
    serde_json::from_str(&r.text().await.unwrap()).unwrap()
}

async fn owner_get(s: &Scene, path: &str) -> reqwest::Response {
    client().get(format!("http://{}{path}", s.public)).bearer_auth(OWNER).send().await.unwrap()
}

#[tokio::test]
async fn publish_then_get_raw_and_isolated_shell() {
    let s = scene().await;
    let res = publish(&s, json!({"session": "s1", "html": "<p>oi</p>", "title": " Oi "})).await;
    assert_eq!(res["ok"], true, "{res}");
    let page = &res["result"]["hangar_page"];
    assert_eq!(page["title"], "Oi");
    assert_eq!(page["heights"], json!({}), "sem Chromium não há altura medida");
    assert!(res["result"]["message"].as_str().unwrap().contains("Não mencione"));
    assert_eq!(res["result"]["browser"], "ausente", "o agente sabe que a página não foi medida");
    assert_eq!(res["result"]["browser_reason"], "sem Chromium no servidor");
    let id = page["id"].as_str().unwrap().to_owned();

    let raw = owner_get(&s, &format!("/api/sessions/s1/pages/{id}?raw=1")).await;
    assert_eq!(raw.status(), 200);
    assert_eq!(raw.headers()["content-security-policy"], "sandbox");
    assert!(raw.headers()["content-type"].to_str().unwrap().starts_with("text/plain"));
    assert!(raw.text().await.unwrap().contains("hangar-theme"));

    let shell = owner_get(&s, &format!("/api/sessions/s1/pages/{id}")).await;
    assert_eq!(shell.status(), 200);
    assert_eq!(shell.headers()["referrer-policy"], "no-referrer");
    let text = shell.text().await.unwrap();
    assert!(text.contains("URL.createObjectURL") && text.contains("<title>Oi</title>"));
    assert!(!text.contains(OWNER), "o token nunca entra na casca");
    assert_eq!(s.python.hits_to(&format!("/api/sessions/s1/pages/{id}")), 0, "o dono é atendido no Rust");
}

#[tokio::test]
async fn draft_returns_relative_url_and_missing_images() {
    let s = scene().await;
    let res = publish(&s, json!({"session": "s1", "html": "<img src=\"/nao/existe.png\">", "title": "R", "draft": true})).await;
    let draft = &res["result"]["draft"];
    let id = draft["id"].as_str().unwrap();
    assert_eq!(draft["url"], format!("/api/sessions/s1/pages/{id}"));
    assert_eq!(draft["missing_images"], json!(["/nao/existe.png"]));
    assert_eq!(draft["browser"], "ausente");
    assert_eq!(draft["shot"], Value::Null);
    assert_eq!(owner_get(&s, &draft["url"].as_str().unwrap().to_owned()).await.status(), 200);
}

#[tokio::test]
async fn published_page_with_missing_image_is_refused() {
    let s = scene().await;
    let res = publish(&s, json!({"session": "s1", "html": "<img src=\"/nao/existe.png\">", "title": "R"})).await;
    assert_eq!(res["error"]["code"], "erro_pagina_imagem_ausente");
}

#[tokio::test]
async fn invalid_input_and_unknown_session_are_refused() {
    let s = scene().await;
    let code = |v: Value| v["error"]["code"].as_str().unwrap().to_owned();
    assert_eq!(code(publish(&s, json!({"session": "s1", "html": "", "title": "x"})).await), "erro_pagina_invalida");
    assert_eq!(code(publish(&s, json!({"session": "s1", "html": "<p></p>", "title": "x".repeat(201)})).await), "erro_pagina_invalida");
    assert_eq!(code(publish(&s, json!({"session": "s1", "html": "<p></p>", "title": "x", "height": 79})).await), "erro_pagina_invalida");
    let negative = publish(&s, json!({"session": "s1", "html": "<p></p>", "title": "x", "height": -1})).await;
    assert_eq!(negative["error"]["detail"], "height fica entre 80 e 2000", "{negative}");
    assert_eq!(code(publish(&s, json!({"session": "s1", "html": "<p></p>", "title": "x", "base": "http://x"})).await), "erro_pagina_invalida");
    s.python.set_info(json!({"provider": "claude", "session_key": "k", "history": {}}));
    assert_eq!(code(publish(&s, json!({"session": "s1", "html": "<p></p>", "title": "x"})).await), "erro_pagina_sem_transcript");
    s.python.set_info(Value::Null);
    assert_eq!(code(publish(&s, json!({"session": "zz", "html": "<p></p>", "title": "x"})).await), "erro_sessao_desconhecida");
}

#[tokio::test]
async fn unknown_page_is_404_and_python_down_is_503() {
    let s = scene().await;
    let r = owner_get(&s, "/api/sessions/s1/pages/0123abcd").await;
    assert_eq!(r.status(), 404);
    assert!(r.text().await.unwrap().contains("erro_pagina_expirou"));
    s.python.fail_info(axum::http::StatusCode::SERVICE_UNAVAILABLE);
    let r = owner_get(&s, "/api/sessions/s1/pages/0123abcd").await;
    assert_eq!(r.status(), 503);
    assert!(r.text().await.unwrap().contains("erro_pagina_sem_info"));
}

#[tokio::test]
async fn corrupt_page_is_an_error_not_expired() {
    let s = scene().await;
    let res = publish(&s, json!({"session": "s1", "html": "<p>oi</p>", "title": "Oi"})).await;
    let id = res["result"]["hangar_page"]["id"].as_str().unwrap().to_owned();
    std::fs::write(s._dir.path().join(format!("k/{id}.json")), "{corrompido").unwrap();
    for path in [format!("/api/sessions/s1/pages/{id}"), format!("/api/sessions/s1/pages/{id}/shot")] {
        let r = owner_get(&s, &path).await;
        assert_eq!(r.status(), 503, "{path}");
        assert!(r.text().await.unwrap().contains("erro_pagina_falhou"), "{path}");
    }
}

#[tokio::test]
async fn shot_without_chromium_is_404_with_code() {
    let s = scene().await;
    let res = publish(&s, json!({"session": "s1", "html": "<p>oi</p>", "title": "Oi"})).await;
    let id = res["result"]["hangar_page"]["id"].as_str().unwrap().to_owned();
    let r = owner_get(&s, &format!("/api/sessions/s1/pages/{id}/shot?theme=dark&width=728")).await;
    assert_eq!(r.status(), 404);
    assert!(r.text().await.unwrap().contains("erro_pagina_sem_imagem"));
}

#[tokio::test]
async fn guest_is_passed_to_python() {
    let s = scene().await;
    let r = client().get(format!("http://{}/api/sessions/s1/pages/abc", s.public)).send().await.unwrap();
    assert_eq!(r.text().await.unwrap(), "from-python");
    assert_eq!(s.python.hits_to("/api/sessions/s1/pages/abc"), 1);
}

#[tokio::test]
async fn bridge_is_private_and_needs_the_secret() {
    let s = scene().await;
    let body = json!({"session": "s1", "html": "<p>oi</p>", "title": "Oi"});
    let public = client().post(format!("http://{}/__hangar_server/pages", s.public)).header("x-hangar-internal", SECRET)
        .body(body.to_string()).send().await.unwrap();
    assert_eq!(public.status(), 404);
    let no_secret = client().post(format!("http://{}/__hangar_server/pages", s.private)).body(body.to_string()).send().await.unwrap();
    assert_eq!(no_secret.status(), 404);
}

#[tokio::test]
async fn url_page_is_published_without_html() {
    let s = scene().await;
    let res = publish(&s, json!({"session": "s1", "url": "http://localhost:3000/cidades", "title": "Cidades"})).await;
    let page = &res["result"]["hangar_page"];
    assert_eq!(page["url"], "http://localhost:3000/cidades", "{res}");
    assert_eq!(page["height"], 640, "moldura padrão do site");
    let id = page["id"].as_str().unwrap().to_owned();
    let raw = owner_get(&s, &format!("/api/sessions/s1/pages/{id}?raw=1")).await;
    assert_eq!(raw.status(), 404);
    assert!(raw.text().await.unwrap().contains("erro_pagina_sem_html"));
    let shot = owner_get(&s, &format!("/api/sessions/s1/pages/{id}/shot")).await;
    assert!(shot.text().await.unwrap().contains("erro_pagina_sem_html"));
    let tall = publish(&s, json!({"session": "s1", "url": "https://example.com", "title": "T", "height": 900})).await;
    assert_eq!(tall["result"]["hangar_page"]["height"], 900);
}

#[tokio::test]
async fn url_draft_only_validates_and_bad_urls_are_refused() {
    let s = scene().await;
    let draft = publish(&s, json!({"session": "zz", "url": "http://LOCALHOST:3000/cidades", "title": "C", "draft": true})).await;
    assert_eq!(draft["result"]["draft"], json!({"url": "http://localhost:3000/cidades"}), "{draft}");
    let code = |v: Value| v["error"]["code"].as_str().unwrap().to_owned();
    assert_eq!(code(publish(&s, json!({"session": "s1", "url": "http://127.0.0.1:8765/", "title": "x"})).await), "erro_pagina_endereco_recusado");
    assert_eq!(code(publish(&s, json!({"session": "s1", "url": "file:///etc/passwd", "title": "x"})).await), "erro_pagina_invalida");
    assert_eq!(code(publish(&s, json!({"session": "s1", "url": "https://example.com", "html": "<p></p>", "title": "x"})).await), "erro_pagina_invalida");
    assert_eq!(code(publish(&s, json!({"session": "s1", "title": "x"})).await), "erro_pagina_invalida");
}
