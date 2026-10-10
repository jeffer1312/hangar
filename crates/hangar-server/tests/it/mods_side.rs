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
async fn python_band_is_dropped_for_owned_session() {
    let (python, server, mods, _dir) = setup().await;
    mods.publish_ui("s", 1, band("rust"));
    let mut events = sse(open_events(server, "s", "", &[]).await);
    assert_eq!(next_named(&mut events, "plugin_ui").await.data, band("rust").to_string());
    wait_until(|| python.side_conns() == 1).await;
    python.push_side("plugin_ui", &json!({"above": null, "panes": []}).to_string());
    python.push_side("plugin_toast", &json!({"id": "py-1", "text": "velho", "plugin": "m", "timeoutMs": 4000}).to_string());
    python.push_side("stats", "{}");
    loop {
        let event = next_non_ping(&mut events).await;
        assert!(event.event != "plugin_ui" && event.event != "plugin_toast", "o Python não sobrescreve a interface do Rust");
        if event.event == "stats" { break; }
    }
}

#[tokio::test]
async fn rebind_keeps_the_rust_band() {
    let (python, server, mods, dir) = setup().await;
    mods.publish_ui("s", 1, band("rust"));
    let mut events = sse(open_events(server, "s", "", &[]).await);
    assert_eq!(next_named(&mut events, "plugin_ui").await.data, band("rust").to_string());
    wait_until(|| python.side_conns() == 1).await;
    let other = dir.path().join("depois-do-clear.jsonl");
    append_lines(&other, 0..1);
    let info = info_json("claude-headless", &other);
    python.set_info(info.clone());
    python.push_side("info", &info.to_string());
    next_named(&mut events, "reset").await;
    assert_eq!(next_named(&mut events, "plugin_ui").await.data, band("rust").to_string(), "a faixa volta com o reset");
    python.push_side("stats", "{}");
    loop {
        let event = next_non_ping(&mut events).await;
        assert!(event.event != "plugin_ui", "a faixa volta uma vez só");
        if event.event == "stats" { break; }
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

#[tokio::test]
async fn python_band_and_toast_pass_when_rust_does_not_own_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("conversation.jsonl");
    append_lines(&path, 0..1);
    let (python, upstream) = spawn_fake().await;
    python.set_info(info_json("claude-headless", &path));
    let server = spawn_state(AppState::new(config(upstream, "127.0.0.1"))).await;
    let mut events = sse(open_events(server, "s", "", &[]).await);
    wait_until(|| python.side_conns() == 1).await;
    python.push_side("plugin_ui", &json!({"above": null, "panes": []}).to_string());
    assert_eq!(next_named(&mut events, "plugin_ui").await.data, json!({"above": null, "panes": []}).to_string());
    python.push_side("plugin_toast", &json!({"id": "py-1", "text": "do python", "plugin": "m", "timeoutMs": 4000}).to_string());
    let toast: Value = serde_json::from_str(&next_named(&mut events, "plugin_toast").await.data).unwrap();
    assert_eq!(toast["text"], "do python");
}
