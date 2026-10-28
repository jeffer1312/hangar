#![cfg(unix)]
use super::support::{self, OWNER, Scene, Server};
use super::transport::{Client, Desktop, KEY, barrier, connect, seed_session, until};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio_tungstenite::tungstenite::Message;

async fn isolated(name: &str) -> bool {
    support::isolated(&format!("remote_browser::identity::{name}"), |root| {
        assert!(!root.join("git.calls").exists(), "descoberta iniciou Git");
    }).await
}

async fn life(scene: &Scene, name: &str) -> String {
    let (row, _) = scene.state.list.discovered_session(name).await.unwrap()
        .expect("fonte temporária não foi descoberta");
    assert_eq!(row.name, name);
    assert!(row.headless);
    row.lifecycle_id.expect("a descoberta precisa demonstrar a vida da sessão")
}

fn session_source(scene: &Scene, name: &str) -> PathBuf {
    scene.home.join(".hangar/claude-headless").join(format!("{name}.json"))
}

async fn ready(scene: &Scene) -> (Client, Vec<Value>) {
    let (mut client, response) = connect(scene, "").await.unwrap();
    assert_eq!(response.status(), 101);
    let mut events = Vec::new();
    until(&mut client, &mut events, |event| event["url"] == "http://contract.test/ready").await;
    let measured = support::contract("chromium");
    assert!(events.iter().any(|event| event["t"] == "q" && event["d"] == measured["frame"]["data"]),
        "a conexão precisa transportar o JPEG congelado");
    (client, events)
}

async fn impose(client: &mut Client, events: &mut Vec<Value>, message: &Value) {
    client.send(Message::Text(message.to_string().into())).await.unwrap();
    barrier(client, events).await;
    assert!(events.iter().any(|event| event["t"] == "layout" && event["resposta"] == "layout: 900x700"));
}

async fn ended(client: &mut Client) {
    tokio::time::timeout(Duration::from_secs(4), async {
        while let Some(message) = client.next().await {
            message.unwrap();
        }
    }).await.expect("o navegador remoto não encerrou");
}

fn arguments(requests: &[Value]) -> Vec<Value> {
    requests.iter().map(|request| request["body"]["args"].clone()).collect()
}

async fn replacement_desktop(name: &str, fixture_name: &str) {
    if isolated(name).await { return; }
    let reference = support::contract("transport");
    let case = reference["cases"].as_array().unwrap().iter()
        .find(|case| case["name"] == fixture_name).unwrap();
    let change = &case["inputs"]["source_change"];
    assert_eq!(change["old_desktop_pid"], 123);
    assert_eq!(change["new_desktop_pid"], 456);
    let scene = Scene::new().await;
    seed_session(&scene);
    let before = life(&scene, "nav-contract").await;
    let old = Desktop::new(&json!({})).await;
    old.publish(&scene, KEY);
    let (mut client, mut events) = ready(&scene).await;
    impose(&mut client, &mut events, &case["inputs"]["messages"][0]).await;
    let old_requests = old.requests();
    assert_eq!(arguments(&old_requests), vec![json!([]), json!(["900", "700"])]);
    assert_eq!(old.current_layout(), "900x700");

    // Outro receptor, porta e credencial; o CDP antigo continua no desktop original.
    let new = Desktop::new(&json!({})).await;
    new.identify(456, change["new_desktop_token"].as_str().unwrap(), "900x700");
    assert_ne!(old.server.address, new.server.address);
    new.publish(&scene, change["key"].as_str().unwrap());
    assert_eq!(life(&scene, "nav-contract").await, before);
    old.detach(0, "target_closed");
    ended(&mut client).await;
    old.wait_closed(1).await;
    new.assert_no_new_requests(&[]).await;
    assert_eq!(new.current_layout(), "900x700", "D02: a vida nova não recebe o original mobile da anterior");
    scene.upstream.assert_no_browser_or_discovery_calls();
    old.server.close().await;
    new.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn late_restore_does_not_change_new_desktop() {
    replacement_desktop(
        "late_restore_does_not_change_new_desktop",
        "same_key_new_desktop_receives_old_restore",
    ).await;
}

#[tokio::test]
async fn late_restore_does_not_change_new_browser_key() {
    replacement_desktop(
        "late_restore_does_not_change_new_browser_key",
        "same_name_new_browser_receives_old_restore",
    ).await;
}

#[tokio::test]
async fn removed_session_source_refuses_new_viewers() {
    if isolated("removed_session_source_refuses_new_viewers").await { return; }
    let scene = Scene::new().await;
    seed_session(&scene);
    let before = life(&scene, "nav-contract").await;
    assert!(!before.is_empty());
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let (mut client, _) = ready(&scene).await;

    // Ausência da fonte simula a saída ou morte externa, sem executar o fechamento do Hangar.
    std::fs::remove_file(session_source(&scene, "nav-contract")).unwrap();
    scene.state.list.invalidate();
    assert!(scene.state.list.discovered_session("nav-contract").await.unwrap().is_none());
    let rejected = connect(&scene, "").await;
    assert!(matches!(rejected, Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status() == 403));
    desktop.detach(0, "target_closed");
    ended(&mut client).await;
    desktop.wait_closed(1).await;
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn recreated_session_source_does_not_inherit_the_previous_original() {
    if isolated("recreated_session_source_does_not_inherit_the_previous_original").await { return; }
    let scene = Scene::new().await;
    seed_session(&scene);
    let before = life(&scene, "nav-contract").await;
    let old = Desktop::new(&json!({})).await;
    old.publish(&scene, KEY);
    let (mut client, mut events) = ready(&scene).await;
    impose(&mut client, &mut events, &json!({"t": "layout", "w": 900, "h": 700})).await;
    assert_eq!(old.current_layout(), "900x700");

    let source = session_source(&scene, "nav-contract");
    let mut replacement: Value = serde_json::from_slice(&std::fs::read(&source).unwrap()).unwrap();
    std::fs::remove_file(&source).unwrap();
    scene.state.list.invalidate();
    assert!(scene.state.list.discovered_session("nav-contract").await.unwrap().is_none());
    replacement["key"] = json!("nav-contract-recreated-key");
    replacement["session_id"] = json!("nav-contract-recreated-transcript");
    std::fs::write(source, replacement.to_string()).unwrap();
    scene.state.list.invalidate();
    let after = life(&scene, "nav-contract").await;
    assert_ne!(after, before, "mesmo nome precisa ter outra vida demonstrada pela descoberta");
    let new = Desktop::new(&json!({})).await;
    new.identify(456, "remote-browser-contract-desktop-new", "desktop");
    new.publish(&scene, KEY);
    let (mut viewer, mut new_events) = ready(&scene).await;
    impose(&mut viewer, &mut new_events, &json!({"t": "layout", "w": 900, "h": 700})).await;
    let requests = new.requests();
    assert_eq!(arguments(&requests), vec![json!([]), json!(["900", "700"])],
        "D02: o novo espectador precisa consultar o próprio original desktop");
    old.detach(0, "target_closed");
    ended(&mut client).await;
    old.wait_closed(1).await;
    new.assert_no_new_requests(&requests).await;
    assert_eq!(new.current_layout(), "900x700");
    viewer.close(None).await.unwrap();
    ended(&mut viewer).await;
    new.wait_closed(1).await;
    new.wait_requests(4).await;
    assert_eq!(arguments(&new.requests()), vec![json!([]), json!(["900", "700"]), json!([]), json!(["desktop"])]);
    assert_eq!(new.current_layout(), "desktop", "o original mobile da vida antiga não pode atravessar");
    scene.upstream.assert_no_browser_or_discovery_calls();
    old.server.close().await;
    new.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn renamed_session_source_keeps_its_life_and_refuses_the_old_route() {
    if isolated("renamed_session_source_keeps_its_life_and_refuses_the_old_route").await { return; }
    let scene = Scene::new().await;
    seed_session(&scene);
    let before = life(&scene, "nav-contract").await;
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let (mut old, mut events) = ready(&scene).await;
    impose(&mut old, &mut events, &json!({"t": "layout", "w": 900, "h": 700})).await;
    let requests = desktop.requests();

    let source = session_source(&scene, "nav-contract");
    let mut renamed: Value = serde_json::from_slice(&std::fs::read(&source).unwrap()).unwrap();
    renamed["name"] = json!("nav-renamed");
    std::fs::write(session_source(&scene, "nav-renamed"), renamed.to_string()).unwrap();
    std::fs::remove_file(source).unwrap();
    desktop.publish(&scene, "contract::nav-renamed");
    scene.state.list.invalidate();
    assert!(scene.state.list.discovered_session("nav-contract").await.unwrap().is_none());
    assert_eq!(life(&scene, "nav-renamed").await, before, "trocar o nome não cria outra vida");
    let rejected = connect(&scene, "").await;
    assert!(matches!(rejected, Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status() == 403));
    let (mut new, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/api/sessions/nav-renamed/nav-remoto?token={OWNER}", scene.owner.address,
    )).await.unwrap();
    let mut renamed_events = Vec::new();
    until(&mut new, &mut renamed_events, |event| event["url"] == "http://contract.test/ready").await;
    desktop.detach(0, "target_closed");
    ended(&mut old).await;
    desktop.wait_closed(1).await;
    desktop.assert_no_new_requests(&requests).await;
    barrier(&mut new, &mut renamed_events).await;
    new.close(None).await.unwrap();
    ended(&mut new).await;
    desktop.wait_closed(2).await;
    desktop.assert_no_new_requests(&requests).await;
    assert_eq!(desktop.current_layout(), "900x700");
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn old_viewer_cannot_restore_after_the_new_viewer_takes_possession() {
    if isolated("old_viewer_cannot_restore_after_the_new_viewer_takes_possession").await { return; }
    let reference = support::contract("transport");
    let case = reference["cases"].as_array().unwrap().iter()
        .find(|case| case["name"] == "old_viewer_leaves_after_same_size_new_viewer").unwrap();
    let scene = Scene::new().await;
    seed_session(&scene);
    let before = life(&scene, "nav-contract").await;
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let (mut old, mut old_events) = ready(&scene).await;
    let first = &case["inputs"]["layout_by_viewer"][0];
    impose(&mut old, &mut old_events, &json!({"t": "layout", "w": first["w"], "h": first["h"]})).await;
    let (mut new, mut new_events) = ready(&scene).await;
    let second = &case["inputs"]["layout_by_viewer"][1];
    impose(&mut new, &mut new_events, &json!({"t": "layout", "w": second["w"], "h": second["h"]})).await;
    assert_eq!(life(&scene, "nav-contract").await, before);
    let requests = desktop.requests();
    assert_eq!(arguments(&requests), vec![json!([]), json!(["900", "700"]), json!(["900", "700"])]);
    old.close(None).await.unwrap();
    ended(&mut old).await;
    desktop.wait_closed(1).await;
    desktop.assert_no_new_requests(&requests).await;
    assert_eq!(desktop.current_layout(), case["expected"]["layout_after_old_viewer"].as_str().unwrap());
    barrier(&mut new, &mut new_events).await;
    new.close(None).await.unwrap();
    ended(&mut new).await;
    desktop.wait_closed(2).await;
    desktop.wait_requests(case["expected"]["cmd"].as_array().unwrap().len()).await;
    assert_eq!(desktop.requests(), *case["expected"]["cmd"].as_array().unwrap());
    assert_eq!(desktop.current_layout(), "mobile");
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}

async fn rebuild_owner(scene: &mut Scene) {
    use hangar_server::{
        list::{bridge::{ListBridge, ListEnv}, facts::FactsClient},
        locate::ListLocate,
        routes::{AppState, router},
    };
    let cfg = scene.state.cfg.clone();
    let source = scene.state.list.env();
    let list = Arc::new(ListBridge::new(ListEnv {
        mux: hangar_server::list::mux::Mux::with_program(source.capture_program.clone(), Duration::from_secs(2)),
        capture_program: source.capture_program.clone(),
        procs: source.procs.clone(), dirs: source.dirs.clone(),
    }, FactsClient::new(cfg.upstream, support::SECRET.into())));
    let mut state = AppState::new(cfg);
    state.hangar_dir = scene.nav_dir().parent().unwrap().to_owned();
    state.list = list.clone();
    state.side.locate = Arc::new(ListLocate { list: list.clone() });
    state.side.queue_dir = scene.state.side.queue_dir.clone();
    state.state = Arc::new(hangar_server::state::live::StateEnv::new(state.terminal.clone(), list, state.diag.clone()));
    state.side.monitors = Some(hangar_server::state::live::spawner(state.state.clone()));
    let state = Arc::new(state);
    let owner = Server::owner(router(state.clone())).await;
    std::mem::replace(&mut scene.owner, owner).close().await;
    scene.state = state;
}

#[tokio::test]
async fn rebuilt_server_rediscovers_live_browser_without_old_layout_possession() {
    if isolated("rebuilt_server_rediscovers_live_browser_without_old_layout_possession").await { return; }
    let mut scene = Scene::new().await;
    seed_session(&scene);
    let before = life(&scene, "nav-contract").await;
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let (mut old, mut events) = ready(&scene).await;
    impose(&mut old, &mut events, &json!({"t": "layout", "w": 900, "h": 700})).await;
    old.close(None).await.unwrap();
    ended(&mut old).await;
    desktop.wait_closed(1).await;
    desktop.wait_requests(4).await;
    assert_eq!(arguments(&desktop.requests()), vec![json!([]), json!(["900", "700"]), json!([]), json!(["mobile"])]);
    assert_eq!(desktop.current_layout(), "mobile");

    // Reconstrói estado e roteador no ensaio; não reinicia o serviço real.
    rebuild_owner(&mut scene).await;
    assert_eq!(life(&scene, "nav-contract").await, before);
    let (mut new, mut new_events) = ready(&scene).await;
    barrier(&mut new, &mut new_events).await;
    let requests = desktop.requests();
    new.close(None).await.unwrap();
    ended(&mut new).await;
    desktop.wait_closed(2).await;
    desktop.assert_no_new_requests(&requests).await;
    assert_eq!(desktop.current_layout(), "mobile");
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}
