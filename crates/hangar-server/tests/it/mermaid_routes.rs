//! Rota do Mermaid: o dono é atendido no Rust (cache, 400, 503 sem Chromium); quem não é dono segue ao Python.
use crate::fake;

use std::net::SocketAddr;
use std::sync::Arc;

use fake::*;
use hangar_server::mermaid::cache::{Cache, Entry, key};
use hangar_server::mermaid::{MERMAID_VERSION, WIDTH_MAX};
use hangar_server::pages::Theme;
use serde_json::{Value, json};

struct Scene { python: Arc<Fake>, public: SocketAddr, dir: tempfile::TempDir }

/// Servidor com o cache numa pasta temporária e sem Chromium.
async fn scene() -> Scene {
    let (python, upstream) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(config(upstream, "127.0.0.1"));
    state.mermaid = Arc::new(Cache::new(dir.path().into()));
    state.chromium = || None;
    let public = spawn_state(state).await;
    Scene { python, public, dir }
}

async fn post(s: &Scene, body: &str, owner: bool) -> reqwest::Response {
    let mut req = client().post(format!("http://{}/api/mermaid", s.public)).header("origin", "http://outra").body(body.to_owned());
    if owner { req = req.bearer_auth(OWNER); }
    req.send().await.unwrap()
}

async fn json_of(r: reqwest::Response) -> Value { serde_json::from_str(&r.text().await.unwrap()).unwrap() }

/// Cabeçalho PNG mínimo: a rota só lê a medida do IHDR.
fn png(w: u32, h: u32) -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend_from_slice(&w.to_be_bytes());
    png.extend_from_slice(&h.to_be_bytes());
    png.extend_from_slice(&[8, 6, 0, 0, 0]);
    png
}

#[tokio::test]
async fn without_chromium_is_503_and_not_cached() {
    let s = scene().await;
    let r = post(&s, &json!({"source": "graph TD; A-->B", "theme": "dark", "width": 600}).to_string(), true).await;
    assert_eq!(r.status(), 503);
    assert_eq!(r.headers()["access-control-allow-origin"], "*");
    let v = json_of(r).await;
    assert_eq!(v, json!({"code": "mermaid_indisponivel", "message": "sem Chromium no servidor"}));
    assert_eq!(std::fs::read_dir(s.dir.path()).unwrap().count(), 0);
    assert_eq!(s.python.hits_to("/api/mermaid"), 0, "o dono é atendido no Rust");
}

#[tokio::test]
async fn bad_body_is_400() {
    let s = scene().await;
    for body in ["{nao e json".to_owned(), json!({"source": "", "theme": "dark", "width": 600}).to_string(),
        json!({"source": "x", "theme": "azul", "width": 600}).to_string(), json!({"source": "x".repeat(50_001), "theme": "dark", "width": 600}).to_string()] {
        let r = post(&s, &body, true).await;
        assert_eq!(r.status(), 400);
        assert_eq!(json_of(r).await, json!({"code": "erro_corpo_invalido"}));
    }
}

#[tokio::test]
async fn cached_png_and_parse_error_are_served() {
    let s = scene().await;
    let cache = Cache::new(s.dir.path().into());
    cache.put(&key(MERMAID_VERSION, Theme::Dark, WIDTH_MAX, "graph TD; A-->B"), &Entry::Png { png: png(1000, 300), width: 500, height: 150 }).unwrap();
    cache.put(&key(MERMAID_VERSION, Theme::Light, WIDTH_MAX, "flowchat"), &Entry::Invalid("No diagram type detected".into())).unwrap();

    let r = post(&s, &json!({"source": "graph TD; A-->B\n", "theme": "dark", "width": 600}).to_string(), true).await;
    assert_eq!(r.status(), 200);
    let h = r.headers();
    assert_eq!(h["content-type"], "image/png");
    assert_eq!((h["x-hangar-css-width"].to_str().unwrap(), h["x-hangar-css-height"].to_str().unwrap()), ("500", "150"));
    assert_eq!(h["cache-control"], "private, max-age=31536000, immutable");
    let exposed = h["access-control-expose-headers"].to_str().unwrap();
    assert!(exposed.contains("x-hangar-css-width") && exposed.contains("x-hangar-css-height"), "{exposed}");
    assert_eq!(r.bytes().await.unwrap().to_vec(), png(1000, 300));

    let r = post(&s, &json!({"source": "flowchat", "theme": "light", "width": 9000}).to_string(), true).await;
    assert_eq!(r.status(), 422, "largura acima do teto vale a borda (1600)");
    assert_eq!(json_of(r).await, json!({"code": "mermaid_invalido", "message": "No diagram type detected"}));
}

#[tokio::test]
async fn not_the_owner_gets_401_from_rust() {
    let s = scene().await;
    let r = post(&s, &json!({"source": "graph TD; A-->B", "theme": "dark", "width": 600}).to_string(), false).await;
    assert_eq!(r.status().as_u16(), 401);
    assert!(r.headers().contains_key("access-control-allow-origin"), "o PWA de outra origem lê a recusa");
    assert_eq!(json_of(r).await, json!({"detail": {"code": "erro_nao_autorizado", "params": {}, "msg": "unauthorized"}}));
    assert_eq!(s.python.hits_to("/api/mermaid"), 0);
}

/// O dono do Connect chega ao Python, que o autentica e repassa pela ponte privada com o caminho original.
#[tokio::test]
async fn connect_owner_comes_through_the_private_bridge() {
    let (python, upstream) = spawn_fake().await;
    let dir = tempfile::tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(config(upstream, "127.0.0.1"));
    state.mermaid = Arc::new(Cache::new(dir.path().into()));
    state.chromium = || None;
    Cache::new(dir.path().into()).put(&key(MERMAID_VERSION, Theme::Dark, WIDTH_MAX, "graph TD; A-->B"),
        &Entry::Png { png: png(1000, 300), width: 500, height: 150 }).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let private = listener.local_addr().unwrap();
    let app = hangar_server::routes::terminal_router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let url = format!("http://{private}/__hangar_server/forwarded");
    let body = json!({"source": "graph TD; A-->B", "theme": "dark", "width": 600}).to_string();
    let bridged = || client().post(&url).header("x-hangar-path", "/api/mermaid").body(body.clone());

    let r = bridged().header("x-hangar-internal", SECRET).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let h = r.headers();
    assert_eq!(h["content-type"], "image/png");
    assert_eq!((h["x-hangar-css-width"].to_str().unwrap(), h["x-hangar-css-height"].to_str().unwrap()), ("500", "150"));
    assert_eq!(h["cache-control"], "private, max-age=31536000, immutable");
    assert_eq!(r.bytes().await.unwrap().to_vec(), png(1000, 300));

    let r = client().post(&url).header("x-hangar-path", "/api/mermaid").header("x-hangar-internal", SECRET)
        .body(json!({"source": "outra", "theme": "dark", "width": 600}).to_string()).send().await.unwrap();
    assert_eq!(r.status(), 503, "sem Chromium, pela ponte também");
    assert_eq!(json_of(r).await["code"], "mermaid_indisponivel");
    assert_eq!(bridged().send().await.unwrap().status(), 404, "sem o segredo");
    let r = client().get(&url).header("x-hangar-path", "/api/mermaid").header("x-hangar-internal", SECRET).send().await.unwrap();
    assert_eq!(r.status(), 404, "só o POST");
    assert_eq!(python.hits_to("/api/mermaid"), 0, "a ponte nunca volta ao Python");
}
