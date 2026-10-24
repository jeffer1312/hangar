use crate::fake;
use crate::mods_support;

use std::sync::Arc;

use fake::*;
use hangar_server::mods::state::*;
use hangar_server::routes::AppState;
use mods_support::NoLink;
use serde_json::{Value, json};

fn band(text: &str) -> Value {
    json!({"above": {"type": "Text", "children": [text]}, "panes": [], "shown_id": null, "columns": 110, "source": "surface"})
}

async fn setup() -> (std::sync::Arc<Fake>, std::net::SocketAddr, Mods, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation.jsonl");
    append_lines(&path, 0..1);
    let (python, server, mods) = serve_mods("s", Arc::new(NoLink)).await;
    python.set_info(info_json("claude-headless", &path));
    (python, server, mods, dir)
}

#[tokio::test]
async fn rust_band_reaches_open_and_late_devices() {
    let (_python, server, mods, _dir) = setup().await;
    mods.publish_ui("s", 1, band("antes"));
    let mut early = sse(open_events(server, "s", "", &[]).await);
    assert_eq!(next_named(&mut early, "plugin_ui").await.data, band("antes").to_string(), "hub novo nasce com a faixa");
    mods.publish_ui("s", 1, band("depois"));
    assert_eq!(next_named(&mut early, "plugin_ui").await.data, band("depois").to_string());
    let mut late = sse(open_events(server, "s", "", &[]).await);
    assert_eq!(next_named(&mut late, "plugin_ui").await.data, band("depois").to_string());
}

#[tokio::test]
async fn rust_toast_reaches_devices_with_time_left() {
    let (_python, server, mods, _dir) = setup().await;
    let mut events = sse(open_events(server, "s", "", &[]).await);
    next_any(&mut events).await;
    mods.toast("s", 1, "vitrine", "V40 aviso curto", 4000);
    let toast: Value = serde_json::from_str(&next_named(&mut events, "plugin_toast").await.data).unwrap();
    assert_eq!((toast["text"].as_str(), toast["plugin"].as_str()), (Some("V40 aviso curto"), Some("vitrine")));
    assert!(toast["timeoutMs"].as_u64().unwrap() <= 4000);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let mut late = sse(open_events(server, "s", "", &[]).await);
    let replayed: Value = serde_json::from_str(&next_named(&mut late, "plugin_toast").await.data).unwrap();
    assert!(replayed["timeoutMs"].as_u64().unwrap() < 4000, "o aparelho tardio recebe só o tempo que resta");
}

#[tokio::test]
async fn rebind_keeps_the_rust_band() {
    let (python, server, mods, dir) = setup().await;
    mods.publish_ui("s", 1, band("rust"));
    let mut events = sse(open_events(server, "s", "", &[]).await);
    assert_eq!(next_named(&mut events, "plugin_ui").await.data, band("rust").to_string());
    let other = dir.path().join("depois-do-clear.jsonl");
    append_lines(&other, 0..1);
    let info = info_json("claude-headless", &other);
    python.set_info(info.clone());
    readings(&python, 2).await;
    next_named(&mut events, "reset").await;
    assert_eq!(next_named(&mut events, "plugin_ui").await.data, band("rust").to_string(), "a faixa volta com o reset");
    append_lines(&other, 1..2);
    loop {
        let event = next_non_ping(&mut events).await;
        assert!(event.event != "plugin_ui", "a faixa volta uma vez só");
        if event.event == "message" && id_of(&event) == "u1" { break; }
    }
}

#[tokio::test]
async fn forget_clears_the_band() {
    let (_python, server, mods, _dir) = setup().await;
    mods.publish_ui("s", 1, band("rust"));
    let mut events = sse(open_events(server, "s", "", &[]).await);
    next_named(&mut events, "plugin_ui").await;
    mods.forget("s", 1);
    let cleared: Value = serde_json::from_str(&next_named(&mut events, "plugin_ui").await.data).unwrap();
    assert!(cleared["above"].is_null() && cleared["panes"] == json!([]));
}

/// Sessão sem entrada no Rust (o `claude` aberto à mão): a faixa e os avisos vêm da ponte do plugin no Rust.
#[tokio::test]
async fn the_band_of_a_session_rust_does_not_own_comes_from_the_rust_bridge_not_python() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation.jsonl");
    append_lines(&path, 0..1);
    let (python, upstream) = spawn_fake().await;
    python.set_info(info_json("claude-headless", &path));
    let server = spawn_state(AppState::new(config(upstream, "127.0.0.1"))).await;
    let mut events = sse(open_events(server, "s", "", &[]).await);
    let post = |route: &str, body: Value| client().post(format!("http://{server}/api/plugin/{route}"))
        .header("content-type", "application/json").body(body.to_string()).send();
    let token = hangar_server::mods::bridge::mint(OWNER, "s");
    assert_eq!(post("ui", json!({"sessao": "s", "token": token, "above": null, "panes": []})).await.unwrap().status(), 200);
    assert_eq!(post("toast", json!({"sessao": "s", "token": token, "text": "do rust", "plugin": "m"})).await.unwrap().status(), 200);
    let ui: Value = serde_json::from_str(&next_named(&mut events, "plugin_ui").await.data).unwrap();
    assert_eq!(ui, json!({"above": null, "panes": []}));
    let toast: Value = serde_json::from_str(&next_named(&mut events, "plugin_toast").await.data).unwrap();
    assert_eq!(toast["text"], "do rust");
    assert_eq!(python.hits_to("/api/plugin/ui") + python.hits_to("/api/plugin/toast"), 0);
}
