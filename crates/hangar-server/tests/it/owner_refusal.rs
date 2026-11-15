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

/// Convidado com login que não vê as sessões do dono, e um que vê.
const BLIND: &str = "convidado-cego";
const SEER: &str = "convidado-que-ve";
const GUEST_REFUSED: &str = r#"{"detail":{"code":"erro_fora_do_convidado","params":{},"msg":"fora do acesso do convidado"}}"#;

/// Python falso e servidor com os dois convidados com login; a pasta segura o cadastro deles.
async fn with_guests() -> (std::sync::Arc<Fake>, std::net::SocketAddr, tempfile::TempDir) {
    use hangar_server::login_guests::{Service, Settings};
    let (python, upstream) = spawn_fake().await;
    let home = crate::physical_tempdir();
    let service = Service::new(Settings { guests: home.path().join("guests.json"), vault: home.path().join("vault.json"),
        runtime_config: home.path().join("runtime-config.json"), sync_enabled: false, bootstrap: String::new(),
        secret: b"fixture".to_vec(), secure: false, rate_max: 10.into(), rate_window: 300.into() });
    let root = home.path().to_str().unwrap();
    service.store.create("Ana", root, false, true, "ana".into(), BLIND.into()).unwrap();
    service.store.create("Bia", root, true, true, "bia".into(), SEER.into()).unwrap();
    let mut state = hangar_server::routes::AppState::new(config(upstream, "127.0.0.1"));
    state.share = None;
    state.login_guests = std::sync::Arc::new(service);
    (python, spawn_state(state).await, home)
}

/// Cada pedido de uma origem própria: as recusas contadas não somam o bloqueio de uma só.
async fn send(srv: std::net::SocketAddr, method: &str, path: &str, token: &str, origin: u8) -> (u16, Value) {
    let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
    let response = client().request(method, format!("http://{srv}{path}")).bearer_auth(token)
        .header("x-forwarded-for", format!("198.51.100.{origin}"))
        .header("content-type", "application/json").body("{}").send().await.unwrap();
    status_and_body(response).await
}

/// Uma família: o estranho leva o 401 contado e o convidado com login fora da sessão o 403, no método que
/// o Rust atende e no que só o Python tinha; o convidado que vê a sessão leva o 403 no método sem rota
/// no Python. Nada chega ao Python.
async fn family(served: &[(&str, &str)], unserved: &[(&str, &str)]) {
    let (python, srv, _home) = with_guests().await;
    let (refused, guest): (Value, Value) = (serde_json::from_str(REFUSED).unwrap(), serde_json::from_str(GUEST_REFUSED).unwrap());
    for (i, (method, path)) in served.iter().chain(unserved).enumerate() {
        assert_eq!(send(srv, method, path, "errado", i as u8 + 1).await, (401, refused.clone()), "estranho: {method} {path}");
        assert_eq!(send(srv, method, path, BLIND, 100).await, (403, guest.clone()), "convidado sem a sessão: {method} {path}");
    }
    for (method, path) in unserved {
        assert_eq!(send(srv, method, path, SEER, 101).await, (403, guest.clone()), "convidado com a sessão: {method} {path}");
    }
    for (_, path) in served.iter().chain(unserved) {
        assert_eq!(python.hits_to(path), 0, "{path}");
    }
}

/// R5 (decisão 5), `history`: o `GET` é do Rust; outro método caía no 405 do Python, que não tem mais a rota.
#[tokio::test]
async fn r5_history_refuses_strangers_and_guests_in_rust() {
    family(&[("GET", "/api/sessions/s0/history")], &[("POST", "/api/sessions/s0/history")]).await;
}

/// R5, `btw`: `GET` e `POST` do Rust; o `DELETE` caía no Python.
#[tokio::test]
async fn r5_btw_refuses_strangers_and_guests_in_rust() {
    family(&[("GET", "/api/sessions/s0/btw"), ("POST", "/api/sessions/s0/btw")], &[("DELETE", "/api/sessions/s0/btw")]).await;
}

/// R5, interface dos mods (`plugin/*`): o `POST` é do Rust; o `GET` caía no Python.
#[tokio::test]
async fn r5_mods_refuse_strangers_and_guests_in_rust() {
    family(&[("POST", "/api/sessions/s0/plugin/press"), ("POST", "/api/sessions/s0/plugin/input")],
        &[("GET", "/api/sessions/s0/plugin/press"), ("GET", "/api/sessions/s0/plugin/show")]).await;
}

/// R5, `conta`: `GET` e `POST` do Rust; o `PUT` caía no Python.
#[tokio::test]
async fn r5_account_switch_refuses_strangers_and_guests_in_rust() {
    family(&[("GET", "/api/sessions/s0/conta"), ("POST", "/api/sessions/s0/conta")], &[("PUT", "/api/sessions/s0/conta")]).await;
}

/// R5, as demais rotas de sessão do Rust: outro método caía no Python, que não tem mais nenhuma delas.
#[tokio::test]
async fn r5_other_session_routes_refuse_strangers_and_guests_in_rust() {
    family(&[], &[("GET", "/api/sessions/s0/input"), ("GET", "/api/sessions/s0/answer"), ("PUT", "/api/sessions/s0/queue/e1"),
        ("POST", "/api/sessions/s0/models"), ("PUT", "/api/sessions/s0/modo-execucao"), ("PUT", "/api/sessions/s0/resume")]).await;
}

/// R5, rotas do dono fora da sessão: o estranho leva o 401 do Rust no método que só o Python tinha.
#[tokio::test]
async fn r5_owner_routes_refuse_the_stranger_on_the_method_python_had() {
    let (python, upstream) = spawn_fake().await;
    let srv = spawn_server(config(upstream, "127.0.0.1")).await;
    let refused: Value = serde_json::from_str(REFUSED).unwrap();
    for (i, (method, path)) in [("POST", "/api/costs"), ("POST", "/api/search"), ("POST", "/api/archive"), ("PUT", "/api/broadcast")]
        .iter().enumerate() {
        assert_eq!(send(srv, method, path, "errado", i as u8 + 1).await, (401, refused.clone()), "{method} {path}");
        assert_eq!(python.hits_to(path), 0, "{path}");
    }
}

/// Troca de modo, retomada e troca de conta são só do dono: o estranho leva o 401 e o convidado com login,
/// mesmo o que vê a sessão, o 403, sem repasse ao Python. Na 8765 é guarda: a base já recusava pelo
/// `gate` + `not_owner_refusal`; o caso que mudou é a 8766, em `share_guest_serving.rs`.
#[tokio::test]
async fn r5_owner_only_relaunch_routes_refuse_everyone_else_in_rust() {
    let (python, srv, _home) = with_guests().await;
    let (refused, guest): (Value, Value) = (serde_json::from_str(REFUSED).unwrap(), serde_json::from_str(GUEST_REFUSED).unwrap());
    let routes = [("POST", "/api/sessions/s0/modo-execucao"), ("POST", "/api/sessions/s0/resume"),
        ("GET", "/api/sessions/s0/conta"), ("POST", "/api/sessions/s0/conta")];
    for (i, (method, path)) in routes.iter().enumerate() {
        assert_eq!(send(srv, method, path, "errado", i as u8 + 1).await, (401, refused.clone()), "estranho: {method} {path}");
        assert_eq!(send(srv, method, path, SEER, 100).await, (403, guest.clone()), "convidado com a sessão: {method} {path}");
        assert_eq!(python.hits_to(path), 0, "{path}");
    }
}
