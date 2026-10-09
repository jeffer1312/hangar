//! A interface dos mods de ponta a ponta: rota do app, porta de entrada da troca de agente, ator
//! do runtime, superfície, cano falso com a vitrine gravada e a faixa no SSE dos aparelhos.
mod fake;
mod mods_support;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use fake::*;
use hangar_server::mods::state::Mods;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use mods_support::*;
use serde_json::Value;

struct World {
    python: Arc<Fake>,
    server: SocketAddr,
    mods: Mods,
    registry: Arc<RuntimeRegistry>,
    seen: Seen,
    cano: tokio::task::JoinHandle<()>,
    _dir: tempfile::TempDir,
}

impl Drop for World {
    fn drop(&mut self) {
        self.cano.abort();
    }
}

/// Servidor com o `Mods` ligado ao registro do runtime, como no `lib.rs`, e a sessão `session` aberta
/// nele sobre o cano da vitrine, já com a faixa desenhada.
async fn world(swallow: &'static [&'static str]) -> World {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("chat.jsonl");
    append_lines(&transcript, 0..1);
    let (python, upstream) = spawn_fake().await;
    python.set_info(info_json("claude-headless", &transcript));
    let state = AppState::new(config(upstream, "127.0.0.1"));
    let mods = state.mods.clone();
    let registry = Arc::new(registry(&mods));
    let _ = state.state.runtime.set(registry.clone());
    let server = spawn_state(state).await;
    let (escuta, seen, cano) = vitrine_cano(swallow).await;
    registry.open(claude_target(dir.path(), escuta, true)).await.unwrap();
    wait_ui(&mods, |ui| ui["above"].to_string().contains("superfície desktop")).await;
    World { python, server, mods, registry, seen, cano, _dir: dir }
}

async fn press(server: SocketAddr, key: &'static str) -> (u16, Value) {
    let response = client().post(format!("http://{server}/api/sessions/session/plugin/press"))
        .header("content-type", "application/json").header("authorization", format!("Bearer {OWNER}"))
        .body(serde_json::json!({"site": "above-prompt", "plugin": "vitrine", "key": key}).to_string()).send().await.unwrap();
    let status = response.status().as_u16();
    let text = response.text().await.unwrap();
    (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
}

fn pressed(seen: &Seen) -> usize {
    seen.lock().unwrap().iter().filter(|(kind, _)| kind == "ui_press").count()
}

#[tokio::test]
async fn turn_and_guard_that_eat_the_budget_keep_the_press_off_the_mod() {
    // I1: a vez da sessão e a porta (fechada por um congelamento curto) gastam o orçamento da rota; o que
    // sobra não cobre o prazo do clique, e o `ui_press` não pode sair (rodaria no mod depois de o app
    // mostrar o erro).
    let world = world(&[]).await;
    let turn = world.mods.link("session").unwrap().lock;
    let held = turn.lock().await;
    world.registry.ingress().close("session", Duration::from_secs(1)).await.unwrap();
    let start = Instant::now();
    let request = tokio::spawn(press(world.server, "abrir-vitrine-botoes"));
    tokio::time::sleep(Duration::from_millis(4500)).await;
    drop(held);
    tokio::time::sleep(Duration::from_millis(300)).await;
    world.registry.ingress().open("session");
    let (status, body) = request.await.unwrap();
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_mod_clique_sem_resposta")));
    assert!(start.elapsed() < Duration::from_secs(8), "{:?}", start.elapsed());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(pressed(&world.seen), 0, "o press não saiu ao mod");
    // Com o orçamento inteiro, o mesmo clique chega ao mod.
    assert_eq!(press(world.server, "abrir-vitrine-botoes").await.0, 200);
    assert_eq!(pressed(&world.seen), 1);
    world.registry.close("key", 1).await.unwrap();
}

#[tokio::test]
async fn app_press_goes_through_the_actor_and_the_pane_reaches_the_devices() {
    // I3 (a): rota → porta → ator → superfície → cano → resposta HTTP, e o painel aberto pelo clique
    // chega aos aparelhos pelo `/events`.
    let world = world(&[]).await;
    let mut events = sse(open_events(world.server, "session", "", &[]).await);
    let band: Value = serde_json::from_str(&next_named(&mut events, "plugin_ui").await.data).unwrap();
    assert!(band["above"].to_string().contains("superfície desktop"), "o aparelho nasce com a faixa");
    assert_eq!(press(world.server, "abrir-vitrine-botoes").await, (200, serde_json::json!({"ok": true})));
    assert_eq!(pressed(&world.seen), 1);
    loop {
        let ui: Value = serde_json::from_str(&next_named(&mut events, "plugin_ui").await.data).unwrap();
        if ui["panes"][0]["id"] == "vitrine-botoes" {
            assert_eq!((ui["shown_id"].as_str(), ui["source"].as_str()), (Some("vitrine-botoes"), Some("surface")));
            break;
        }
    }
    world.registry.close("key", 1).await.unwrap();
}

#[tokio::test]
async fn session_leaving_rust_mid_press_answers_and_the_next_press_goes_to_python() {
    // I3 (b): o mod engole o press; a sessão sai do Rust no meio. A rota responde com código, sem
    // pendurar o app, e o pedido seguinte já não tem dono no Rust: vai ao Python.
    let world = world(&["ui_press"]).await;
    let mut events = sse(open_events(world.server, "session", "", &[]).await);
    next_named(&mut events, "plugin_ui").await;
    let start = Instant::now();
    let request = tokio::spawn(press(world.server, "abrir-vitrine-botoes"));
    wait_request(&world.seen, "ui_press").await;
    world.registry.close("key", 1).await.unwrap();
    let (status, body) = request.await.unwrap();
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_mod_clique_sem_resposta")));
    assert!(start.elapsed() < Duration::from_secs(8), "{:?}", start.elapsed());
    assert!(!world.mods.owns("session"));
    let cleared: Value = serde_json::from_str(&next_named(&mut events, "plugin_ui").await.data).unwrap();
    assert!(cleared["above"].is_null() && cleared["panes"] == serde_json::json!([]), "a faixa sai dos aparelhos");
    assert_eq!(press(world.server, "abrir-vitrine-botoes").await, (200, Value::String("from-python".into())));
    assert_eq!(world.python.hits_to("/api/sessions/session/plugin/press"), 1);
    assert_eq!(pressed(&world.seen), 1, "o segundo pedido não chegou ao cano pelo Rust");
}
