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
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let refused: Value = serde_json::from_str(REFUSED).unwrap();
    let get = client().get(format!("http://{srv}/api/presence")).bearer_auth("errado").send().await.unwrap();
    assert_eq!(status_and_body(get).await, (401, refused.clone()));
    let post = client().post(format!("http://{srv}/api/presence")).header("content-type", "application/json")
        .body(json!({"mode": "auto"}).to_string()).send().await.unwrap();
    assert_eq!(status_and_body(post).await, (401, refused));
}

/// R5-08: `POST /api/sessions/{n}/conta` (o Python só tinha o `GET`): sem o dono, 401 em vez do 405.
#[tokio::test]
async fn r5_08_method_the_python_never_had_is_401_for_the_stranger() {
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let post = client().post(format!("http://{srv}/api/sessions/s0/conta")).header("content-type", "application/json")
        .body("{}").send().await.unwrap();
    assert_eq!(status_and_body(post).await, (401, serde_json::from_str::<Value>(REFUSED).unwrap()));
    let get = client().get(format!("http://{srv}/api/sessions/s0/conta")).send().await.unwrap();
    assert_eq!(get.status().as_u16(), 401, "o `GET` que o Python tinha: o mesmo 401");
}

/// Convidado com login que não vê as sessões do dono, e um que vê.
const BLIND: &str = "convidado-cego";
const SEER: &str = "convidado-que-ve";
const GUEST_REFUSED: &str = r#"{"detail":{"code":"erro_fora_do_convidado","params":{},"msg":"fora do acesso do convidado"}}"#;

/// Servidor com os dois convidados com login; a pasta segura o cadastro deles.
async fn with_guests() -> (std::net::SocketAddr, tempfile::TempDir) {
    use hangar_server::login_guests::{Service, Settings};
    let (python, _) = spawn_fake().await;
    let home = crate::physical_tempdir();
    let service = Service::new(Settings { guests: home.path().join("guests.json"), vault: home.path().join("vault.json"),
        runtime_config: home.path().join("runtime-config.json"), sync_enabled: false, bootstrap: String::new(),
        secret: b"fixture".to_vec(), secure: false, rate_max: 10.into(), rate_window: 300.into() });
    let root = home.path().to_str().unwrap();
    service.store.create("Ana", root, false, true, "ana".into(), BLIND.into()).unwrap();
    service.store.create("Bia", root, true, true, "bia".into(), SEER.into()).unwrap();
    let mut state = hangar_server::routes::AppState::new(config("127.0.0.1"));
    state.share = None;
    state.login_guests = std::sync::Arc::new(service);
    let addr = spawn_state_for(&python, state).await;
    (addr, home)
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
/// no Python.
async fn family(served: &[(&str, &str)], unserved: &[(&str, &str)]) {
    let (srv, _home) = with_guests().await;
    let (refused, guest): (Value, Value) = (serde_json::from_str(REFUSED).unwrap(), serde_json::from_str(GUEST_REFUSED).unwrap());
    for (i, (method, path)) in served.iter().chain(unserved).enumerate() {
        assert_eq!(send(srv, method, path, "errado", i as u8 + 1).await, (401, refused.clone()), "estranho: {method} {path}");
        assert_eq!(send(srv, method, path, BLIND, 100).await, (403, guest.clone()), "convidado sem a sessão: {method} {path}");
    }
    for (method, path) in unserved {
        assert_eq!(send(srv, method, path, SEER, 101).await, (403, guest.clone()), "convidado com a sessão: {method} {path}");
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
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let refused: Value = serde_json::from_str(REFUSED).unwrap();
    for (i, (method, path)) in [("POST", "/api/costs"), ("POST", "/api/search"), ("POST", "/api/archive"), ("PUT", "/api/broadcast")]
        .iter().enumerate() {
        assert_eq!(send(srv, method, path, "errado", i as u8 + 1).await, (401, refused.clone()), "{method} {path}");
    }
}

/// Troca de modo, retomada e troca de conta são só do dono: o estranho leva o 401 e o convidado com login,
/// mesmo o que vê a sessão, o 403, sem repasse ao Python. Na 8765 é guarda: a base já recusava pelo
/// `gate` + `not_owner_refusal`; o caso que mudou é a 8766, em `share_guest_serving.rs`.
#[tokio::test]
async fn r5_owner_only_relaunch_routes_refuse_everyone_else_in_rust() {
    let (srv, _home) = with_guests().await;
    let (refused, guest): (Value, Value) = (serde_json::from_str(REFUSED).unwrap(), serde_json::from_str(GUEST_REFUSED).unwrap());
    let routes =[("POST", "/api/sessions/s0/modo-execucao"), ("POST", "/api/sessions/s0/resume"),
        ("GET", "/api/sessions/s0/conta"), ("POST", "/api/sessions/s0/conta")];
    for (i, (method, path)) in routes.iter().enumerate() {
        assert_eq!(send(srv, method, path, "errado", i as u8 + 1).await, (401, refused.clone()), "estranho: {method} {path}");
        assert_eq!(send(srv, method, path, SEER, 100).await, (403, guest.clone()), "convidado com a sessão: {method} {path}");
    }
}

/// R5-09: toda rota do Rust que o Python também tinha (com `require_auth`) recusa o estranho com o 401
/// do Rust, corpo exato, e nada chega ao Python. Cada pedido de uma origem: o bloqueio não entra na conta.
#[tokio::test]
async fn r5_09_every_owner_route_the_python_had_refuses_the_stranger_with_the_exact_401() {
    let (python, _) = spawn_fake().await;
    let srv = spawn_server_for(&python, config("127.0.0.1")).await;
    let refused: Value = serde_json::from_str(REFUSED).unwrap();
    // Só as que o Rust atende sozinho: as que ainda têm handler no Python (`@app`/`@router` em
    // `backend/app`) e o `jev/*`, que o Rust repassa, ficam fora.
    let session = ["input", "steer", "interrupt", "select", "select/submit", "answer", "keys", "term-input", "recarregar",
        "modo-execucao", "model-effort", "permission-mode", "model", "service-tier", "codex/mode", "codex/plan/implement",
        "question/skip", "plugin/press", "pair", "pair-remote", "unpair-remote", "group-message", "resume", "transcribe"]
        .map(|rest| ("POST", format!("/api/sessions/s0/{rest}")));
    let reads = ["btw", "conta", "permission-modes", "models", "limits", "codex-permissions", "commands", "history",
        "pair/contract", "group", "bash-output", "tool-progress/t1", "transcript-image/u1/0"]
        .map(|rest| ("GET", format!("/api/sessions/s0/{rest}")));
    let others = [("DELETE", "/api/sessions/s0/queue/e1"), ("DELETE", "/api/sessions/s0/pair"), ("POST", "/api/broadcast"),
        ("POST", "/api/sessions"), ("POST", "/api/archive/p/id/resume"), ("GET", "/api/claude/customizations"), ("GET", "/api/search"),
        ("GET", "/api/archive"), ("POST", "/api/ask-history")]
        .map(|(method, path)| (method, path.to_owned()));
    let routes: Vec<(&str, String)> = session.into_iter().chain(reads).chain(others).collect();
    assert!(routes.len() < 250, "uma origem por rota");
    for (i, (method, path)) in routes.iter().enumerate() {
        assert_eq!(send(srv, method, path, "errado", i as u8 + 1).await, (401, refused.clone()), "{method} {path}");
    }
}

/// R5-10 (decisão da rodada 3): o convidado com login em `/api/session-defaults`, veja ele as sessões do
/// dono ou não, leva o 403 uniforme `erro_fora_do_convidado` (sem perder o login); o estranho, o 401.
#[tokio::test]
async fn r5_10_a_login_guest_on_the_session_defaults_is_the_uniform_403() {
    let (srv, _home) = with_guests().await;
    let (refused, guest): (Value, Value) = (serde_json::from_str(REFUSED).unwrap(), serde_json::from_str(GUEST_REFUSED).unwrap());
    let path ="/api/session-defaults?provider=claude";
    for method in ["GET", "PUT"] {
        assert_eq!(send(srv, method, path, SEER, 100).await, (403, guest.clone()), "convidado que vê: {method}");
        assert_eq!(send(srv, method, path, BLIND, 101).await, (403, guest.clone()), "convidado cego: {method}");
        assert_eq!(send(srv, method, path, "errado", if method == "GET" { 1 } else { 2 }).await, (401, refused.clone()), "estranho: {method}");
    }
}

/// R5-05 / T-02, T-03, T-05: o terminal da 8765 com token errado é recusado pelo Rust no aperto de mão
/// com 403 vazio, e cada recusa conta exatamente uma vez para a origem: depois de 7 recusas a origem
/// ainda não está bloqueada (o HTTP errado seguinte é 401, a oitava falha); depois dela, nem a senha
/// certa abre nada (o terminal segue recusado sem nova falha e o HTTP dá 429), e outra origem segue
/// normal. Sem Python no caminho: quem conta é o contador do Rust.
#[tokio::test]
async fn r5_05_the_terminal_with_a_wrong_token_is_403_empty_counted_once_and_the_eighth_blocks() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let srv = spawn_server(config("127.0.0.1")).await;
    let handshake = |token: &'static str, origin: &'static str| async move {
        let mut request = format!("ws://{srv}/api/sessions/s0/term?token={token}").into_client_request().unwrap();
        request.headers_mut().insert("x-forwarded-for", origin.parse().unwrap());
        match tokio_tungstenite::connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => (response.status().as_u16(), response.body().clone().unwrap_or_default()),
            other => panic!("o aperto de mão não foi recusado: {:?}", other.map(|_| ())),
        }
    };
    const BLOCKED: &str = "198.51.100.200";
    let presence = |token: &'static str| client().get(format!("http://{srv}/api/presence")).bearer_auth(token).header("x-forwarded-for", BLOCKED).send();
    for _ in 0..7 {
        assert_eq!(handshake("errado", BLOCKED).await, (403, Vec::new()), "403 vazio no aperto de mão");
    }
    assert_eq!(presence("errado").await.unwrap().status().as_u16(), 401, "7 recusas contaram 7: a oitava falha ainda é 401");
    assert_eq!(handshake("errado", BLOCKED).await.0, 403, "depois da oitava, a nona também");
    assert_eq!(handshake(OWNER, BLOCKED).await.0, 403, "nem a senha certa abre o terminal da origem bloqueada");
    assert_eq!(presence(OWNER).await.unwrap().status().as_u16(), 429, "a oitava falha bloqueou a origem");
    let other = client().get(format!("http://{srv}/api/presence")).bearer_auth(OWNER).header("x-forwarded-for", "198.51.100.201").send().await.unwrap();
    assert!(!matches!(other.status().as_u16(), 401 | 429), "outra origem segue normal: {}", other.status());
}

/// T-08 / T-04: no terminal só o `?token=` vale. Bearer e cookie do dono são recusados e contados como
/// falha, uma vez cada; a origem loopback nunca conta. Sete recusas não bloqueiam: a oitava bloqueia.
#[tokio::test]
async fn t08_the_terminal_takes_only_the_query_token_and_counts_each_refusal_once() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let srv = spawn_server(config("127.0.0.1")).await;
    let handshake = |header: Option<(&'static str, String)>, origin: &'static str| async move {
        let mut request = format!("ws://{srv}/api/sessions/s0/term").into_client_request().unwrap();
        request.headers_mut().insert("x-forwarded-for", origin.parse().unwrap());
        if let Some((name, value)) = header { request.headers_mut().insert(name, value.parse().unwrap()); }
        match tokio_tungstenite::connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => (response.status().as_u16(), response.body().clone().unwrap_or_default()),
            other => panic!("o aperto de mão não foi recusado: {:?}", other.map(|_| ())),
        }
    };
    const ORIGIN: &str = "198.51.100.210";
    for _ in 0..3 {
        assert_eq!(handshake(Some(("authorization", format!("Bearer {OWNER}"))), ORIGIN).await, (403, Vec::new()), "Bearer não abre");
        assert_eq!(handshake(Some(("cookie", format!("cp_token={OWNER}"))), ORIGIN).await, (403, Vec::new()), "cookie não abre");
    }
    assert_eq!(handshake(Some(("authorization", format!("Bearer {OWNER}"))), ORIGIN).await, (403, Vec::new()), "sétima recusa");
    for _ in 0..12 {
        assert_eq!(handshake(None, "127.0.0.1").await.0, 403, "loopback recusado sem contar");
    }
    let presence = |token: &'static str| client().get(format!("http://{srv}/api/presence")).bearer_auth(token).header("x-forwarded-for", ORIGIN).send();
    assert_eq!(presence("errado").await.unwrap().status().as_u16(), 401, "7 recusas contaram 7: a oitava falha ainda é 401");
    assert_eq!(presence(OWNER).await.unwrap().status().as_u16(), 429, "a oitava falha bloqueou a origem");
    let local = client().get(format!("http://{srv}/api/presence")).bearer_auth(OWNER).send().await.unwrap();
    assert!(!matches!(local.status().as_u16(), 401 | 429), "loopback segue normal: {}", local.status());
}

/// Aperto de mão do terminal com `?token=` cru na query, a partir de `origin`: status e corpo da recusa.
async fn term_handshake(srv: std::net::SocketAddr, query_token: &str, origin: &str) -> (u16, Vec<u8>) {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut request = format!("ws://{srv}/api/sessions/s0/term?token={query_token}").into_client_request().unwrap();
    request.headers_mut().insert("x-forwarded-for", origin.parse().unwrap());
    match tokio_tungstenite::connect_async(request).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => (response.status().as_u16(), response.body().clone().unwrap_or_default()),
        other => panic!("o aperto de mão não foi recusado: {:?}", other.map(|_| ())),
    }
}

// T-06 (o aviso no log quando a origem é bloqueada) mora em `tests/terminal_refusal_log.rs`: o
// assinante de log precisa de um processo só dele.

/// T-10: `?token=` com texto não-ASCII (UTF-8 válido ou bytes soltos) é só um token errado: 403 vazio,
/// sem derrubar nada; o servidor segue atendendo.
#[tokio::test]
async fn t10_a_non_ascii_terminal_token_is_refused_without_panic() {
    let srv = spawn_server(config("127.0.0.1")).await;
    const ORIGIN: &str = "198.51.100.240";
    for token in ["%C3%A9rr%C3%A3o-%E2%9C%93", "%FF%FE%00", "d%C3%B4no"] {
        assert_eq!(term_handshake(srv, token, ORIGIN).await, (403, Vec::new()), "{token}");
    }
    let alive = client().get(format!("http://{srv}/api/presence")).bearer_auth(OWNER).header("x-forwarded-for", "198.51.100.241").send().await.unwrap();
    assert!(!matches!(alive.status().as_u16(), 401 | 429 | 500..=599), "o servidor segue de pé: {}", alive.status());
}
