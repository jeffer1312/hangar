//! A interface dos mods de ponta a ponta: rota do app, porta de entrada da troca de agente, ator
//! do runtime, superfície, cano falso com a vitrine gravada e a faixa no SSE dos aparelhos.
use crate::fake;
use crate::mods_support;

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

/// O estado com o `Mods` ligado ao registro do runtime, como no `lib.rs`, e o Python falso que descobre
/// a conversa da sessão.
async fn parts() -> (AppState, Arc<fake::Fake>, Arc<RuntimeRegistry>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let transcript = dir.path().join("chat.jsonl");
    append_lines(&transcript, 0..1);
    let (python, _) = spawn_fake().await;
    python.set_info(info_json("claude-headless", &transcript));
    let state = AppState::new(config("127.0.0.1"));
    // Como o `lib.rs`: o registro é o ciclo de vida e o runtime do estado ao vivo, ligado aos hubs do chat.
    let registry = Arc::new(RuntimeRegistry::new("instance-test".into())
        .with_mods(state.mods.clone()).with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))));
    registry.bind_lifecycle();
    registry.bind_hubs(state.side.hubs.downgrade());
    let _ = state.state.lifecycle.set(registry.clone());
    let _ = state.side.runtime.set(registry.clone());
    let _ = state.state.runtime.set(registry.clone());
    (state, python, registry, dir)
}

/// A sessão `session` aberta no registro sobre o cano da vitrine, já com a faixa desenhada.
async fn open_vitrine(registry: &RuntimeRegistry, mods: &Mods, dir: &std::path::Path, swallow: &'static [&'static str])
    -> (Seen, tokio::task::JoinHandle<()>) {
    let (escuta, seen, cano) = vitrine_cano(swallow).await;
    registry.open(claude_target(dir, escuta, true)).await.unwrap();
    wait_ui(mods, |ui| ui["above"].to_string().contains("superfície desktop")).await;
    (seen, cano)
}

/// Servidor sobre `parts`, com a sessão aberta.
async fn world(swallow: &'static [&'static str]) -> World {
    let (state, python, registry, dir) = parts().await;
    let mods = state.mods.clone();
    let server = spawn_state_for(&python, state).await;
    let (seen, cano) = open_vitrine(&registry, &mods, dir.path(), swallow).await;
    World { server, mods, registry, seen, cano, _dir: dir }
}

fn press_body(key: &str) -> Value {
    serde_json::json!({"site": "above-prompt", "plugin": "vitrine", "key": key})
}

async fn press(server: SocketAddr, key: &'static str) -> (u16, Value) {
    let response = client().post(format!("http://{server}/api/sessions/session/plugin/press"))
        .header("content-type", "application/json").header("authorization", format!("Bearer {OWNER}"))
        .body(press_body(key).to_string()).send().await.unwrap();
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
    // mostrar o erro). A rota é chamada direto: o orçamento dela começa no primeiro `poll`, e a vez e a
    // porta contam dali, não de quando o pedido saiu (sob carga ele chega tarde e sobraria o prazo do clique).
    let (mut state, python, registry, dir) = parts().await;
    state.side.locate = python.locate();
    let state = Arc::new(state);
    let (seen, cano) = open_vitrine(&registry, &state.mods, dir.path(), &[]).await;
    let _cano = Abort(cano);
    let turn = state.mods.link("session").unwrap().lock;
    let held = turn.lock().await;
    registry.ingress().close("session", Duration::from_secs(1)).await.unwrap();
    let start = Instant::now();
    let mut request = Box::pin(mods_route(state.clone(), "session", "press", press_body("abrir-vitrine-botoes")));
    let first = std::future::poll_fn(|cx| std::task::Poll::Ready(std::future::Future::poll(request.as_mut(), cx))).await;
    assert!(first.is_pending(), "a rota respondeu com a vez da sessão presa: {first:?}");
    let entered = Instant::now();
    let request = tokio::spawn(request);
    tokio::time::sleep_until(tokio::time::Instant::from_std(entered + Duration::from_millis(4500))).await;
    drop(held);
    tokio::time::sleep(Duration::from_millis(300)).await;
    registry.ingress().open("session");
    let (status, body) = request.await.unwrap();
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("erro_mod_clique_sem_resposta")));
    assert!(start.elapsed() < Duration::from_secs(8), "{:?}", start.elapsed());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(pressed(&seen), 0, "o press não saiu ao mod");
    // Com o orçamento inteiro, o mesmo clique chega ao mod.
    assert_eq!(mods_route(state.clone(), "session", "press", press_body("abrir-vitrine-botoes")).await.0, 200);
    assert_eq!(pressed(&seen), 1);
    registry.close("key", 1).await.unwrap();
}

/// O cano da vitrine para junto com o teste, como no `World`.
struct Abort(tokio::task::JoinHandle<()>);

impl Drop for Abort {
    fn drop(&mut self) {
        self.0.abort();
    }
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
async fn session_leaving_rust_mid_press_answers_and_the_next_press_gets_the_fixed_answer() {
    // I3 (b): o mod engole o press; a sessão sai do Rust no meio. A rota responde com código, sem
    // pendurar o app, e o pedido seguinte já não tem a interface no Rust: recusa fixa, sem o Python.
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
    let (status, body) = press(world.server, "abrir-vitrine-botoes").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (503, Some("mods_need_rust_server")));
    assert_eq!(pressed(&world.seen), 1, "o segundo pedido não chegou ao cano pelo Rust");
}
