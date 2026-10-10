use crate::fake;
use crate::mods_support;

use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

use fake::*;
use hangar_server::mods::bridge::{mint, mint_keyed};
use hangar_server::mods::state::*;
use hangar_server::routes::AppState;
use mods_support::Probe;
use serde_json::{Value, json};

async fn setup() -> (Arc<Fake>, std::net::SocketAddr, Mods, Arc<Probe>) {
    let (python, upstream) = spawn_fake().await;
    let state = AppState::new(config(upstream, "127.0.0.1"));
    let mods = state.mods.clone();
    let server = spawn_state(state).await;
    let probe = Arc::new(Probe::default());
    mods.attach_terminal("t", "proc-t", 1, probe.clone());
    (python, server, mods, probe)
}

fn signed(name: &str, extra: Value) -> Value {
    let mut body = json!({"sessao": name, "token": mint(OWNER, name)});
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    body
}

/// O `reqwest` dos testes não tem a função `json`: o corpo vai pronto, com o tipo.
async fn post(server: std::net::SocketAddr, route: &str, body: Value) -> (u16, Value) {
    let response = client().post(format!("http://{server}/api/plugin/{route}"))
        .header("content-type", "application/json").body(body.to_string()).send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn last_ui(mods: &Mods, name: &str) -> Value {
    serde_json::from_str(&mods.replay(name).into_iter().rev().find(|(event, _)| *event == "plugin_ui").unwrap().1).unwrap()
}

#[tokio::test]
async fn ui_is_mirrored_and_the_screen_decides_the_shown_pane() {
    let (python, server, mods, probe) = setup().await;
    *probe.shown.lock().unwrap() = Some("a".into());
    let panes = json!([{"id": "a", "title": "A", "placement": "dock", "columns": 58, "tree": {"type": "engine", "ref": 0}},
                       {"id": "b", "title": "B", "placement": "dock", "columns": 58, "tree": {"type": "Box"}}]);
    let (status, _) = post(server, "ui", signed("t", json!({"above": {"type": "Box"}, "columns": 87, "bodyColumns": 82, "panes": panes, "shown": "b"}))).await;
    assert_eq!(status, 200);
    let ui = last_ui(&mods, "t");
    assert_eq!((ui["shown_id"].as_str(), ui["columns"].as_u64(), ui["source"].as_str()), (Some("b"), Some(82), Some("terminal")));
    // A cópia para a prévia e o `/pull` do Python, no formato de hoje (o `BandBody` dele).
    assert_eq!(python.hits_to("/api/plugin/ui"), 1, "a cópia da faixa vai ao Python");
    // A leitura da tela vem ao fim da janela; sob carga, depois dela. Espera o efeito, não o relógio.
    fake::wait_until(|| last_ui(&mods, "t")["shown_id"] == "a").await;
    assert_eq!(probe.reads.load(SeqCst), 1, "a linha de abas da tela vence o último desenho");
}

#[tokio::test]
async fn a_renamed_session_is_found_by_the_key_its_process_was_launched_with() {
    let (python, server, mods, _probe) = setup().await;
    // Renomear sem relançar: o Rust fecha e reabre no mesmo processo, e o plugin segue mandando o nome de
    // nascimento, com a chave que recebeu no lançamento (`plugin_key` do vínculo).
    mods.forget("t", 1);
    mods.attach_terminal_keyed("novo", "proc-t", Some("lancada"), 2, Arc::new(Probe::default()));
    let mut body = json!({"sessao": "t", "token": mint_keyed(OWNER, "t", "lancada")});
    body.as_object_mut().unwrap().extend(json!({"above": {"type": "Box"}, "columns": 87, "bodyColumns": 82, "panes": []}).as_object().unwrap().clone());
    let (status, _) = post(server, "ui", body).await;
    assert_eq!(status, 200);
    assert_eq!(last_ui(&mods, "novo")["source"], "terminal");
    // O cache do Python é pelo nome de agora: a cópia leva o nome atual e o token dele.
    let copy = python.plugin_ui_bodies();
    assert_eq!(copy.len(), 1, "a cópia da faixa vai ao Python");
    assert_eq!((copy[0]["sessao"].as_str(), copy[0]["token"].as_str()), (Some("novo"), Some(mint(OWNER, "novo").as_str())));
}

#[tokio::test]
async fn presses_toasts_copies_focus_and_scroll() {
    let (_python, server, mods, _probe) = setup().await;
    let since = Instant::now();
    assert_eq!(post(server, "pressed", signed("t", json!({"requestId": "a", "element": "k", "plugin": "m"}))).await.0, 200);
    assert!(mods.wait_pressed("t", 1, "a", "m", "k", since - Duration::from_millis(1), Duration::from_millis(10)).await);
    assert!(!mods.wait_pressed("t", 1, "a", "outro", "k", since - Duration::from_millis(1), Duration::from_millis(10)).await);
    assert_eq!(post(server, "toast", signed("t", json!({"text": "aviso", "timeoutMs": 4000, "plugin": "m"}))).await.0, 200);
    assert!(mods.replay("t").iter().any(|(event, data)| *event == "plugin_toast" && data.contains("aviso")));
    let attempt = mods.begin_click("t", "a", "vitrine", "k");
    let long = "á".repeat(65536);
    assert_eq!(post(server, "copied", signed("t", json!({"attempt": attempt, "text": long}))).await.0, 200, "o teto do Python, em caracteres");
    assert_eq!(post(server, "copied", signed("t", json!({"attempt": "outra", "text": "x"}))).await.0, 409);
    assert_eq!(post(server, "focus-target", signed("t", json!({"requestId": "a", "plugin": "m", "element": "x"}))).await.1["armed"], false);
    let armed = mods.arm_focus("t", 1, "a", Some("m"), "k");
    let target = post(server, "focus-target", signed("t", json!({"requestId": "a", "plugin": "m", "element": "x"}))).await.1;
    assert_eq!(target, json!({"armed": true, "attempt": armed, "rewrite": "k"}));
    let seq = mods.focus_seq("t", 1);
    assert_eq!(post(server, "focused", signed("t", json!({"attempt": armed, "requestId": "a", "plugin": "m", "element": "k", "denied": false}))).await.0, 200);
    let seen = mods.wait_focus("t", 1, &armed, seq, Duration::from_millis(10), |_| true).await.unwrap();
    assert_eq!(seen.plugin.as_deref(), Some("m"), "o mod do foco chega ao clique");
    assert_eq!(post(server, "focused", signed("t", json!({"attempt": armed, "requestId": "a", "element": "k", "denied": false}))).await.0, 200,
        "o plugin de antes desta versão não manda o mod");
    assert_eq!(post(server, "focused", signed("t", json!({"attempt": "velha", "requestId": "a", "element": "k", "denied": false}))).await.0, 409);
    assert_eq!(post(server, "scroll", signed("t", json!({"requestId": "a", "offset": 67, "bodyRows": 38, "contentRows": 205}))).await.0, 200);
    assert_eq!(mods.last_scroll("t", 1, "a").1, Some(67));
}

#[tokio::test]
async fn a_long_toast_is_cut_not_refused() {
    // O Python corta o texto longo do aviso em vez de recusar: o teto de 16 KB das outras rotas o sumiria.
    let (_python, server, mods, _probe) = setup().await;
    let long = "x".repeat(40 * 1024);
    assert_eq!(post(server, "toast", signed("t", json!({"text": long, "plugin": "m"}))).await.0, 200);
    assert!(mods.replay("t").iter().any(|(event, _)| *event == "plugin_toast"));
}

#[tokio::test]
async fn other_sessions_bad_tokens_and_bad_bodies() {
    let (python, server, _mods, _probe) = setup().await;
    assert_eq!(post(server, "ui", signed("com-python", json!({"above": null, "panes": []}))).await.1, "from-python");
    assert_eq!(python.hits_to("/api/plugin/ui"), 1);
    let mut wrong = signed("t", json!({"requestId": "a", "element": "k"}));
    wrong["token"] = json!("x");
    assert_eq!(post(server, "pressed", wrong).await.0, 403);
    // Os limites do Pydantic vêm antes do token, como no Python: com token errado, 422.
    let bad = |extra: Value| { let mut body = signed("t", extra); body["token"] = json!("x"); body };
    for (route, body) in [
        ("ui", bad(json!({"panes": [{"id": "p".repeat(65)}]}))),
        ("ui", bad(json!({"panes": [{"id": ""}]}))),
        ("pressed", bad(json!({"requestId": "", "element": "k"}))),
        ("pressed", bad(json!({"requestId": "a", "element": "é".repeat(257)}))),
        ("copied", bad(json!({"attempt": "é".repeat(65), "text": "x"}))),
        ("copied", bad(json!({"attempt": "a", "text": "x".repeat(65537)}))),
        ("focus-target", bad(json!({"requestId": "é".repeat(65)}))),
        ("focused", bad(json!({"attempt": "", "requestId": "a", "denied": false}))),
        ("scroll", bad(json!({"requestId": "a", "offset": -1, "bodyRows": 1, "contentRows": 1}))),
        // O `owned` da fase 2 passa a responder 422 ao corpo inválido de sessão do Rust.
        ("press-start", bad(json!({"requestId": "a"}))),
        ("opened", bad(json!({"attempt": 7, "url": "https://example.com"}))),
    ] {
        assert_eq!(post(server, route, body.clone()).await.0, 422, "{route} {body}");
    }
    let big = "x".repeat(310 * 1024);
    assert_eq!(post(server, "ui", signed("t", json!({"above": {"type": "Text", "children": [big]}, "panes": []}))).await.0, 413);
    assert_eq!(python.hits_to("/api/plugin/pressed") + python.hits_to("/api/plugin/scroll")
        + python.hits_to("/api/plugin/press-start") + python.hits_to("/api/plugin/opened"), 0,
        "nada da sessão do Rust vai ao Python");
}
