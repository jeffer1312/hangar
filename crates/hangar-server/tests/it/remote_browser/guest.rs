#![cfg(unix)]
use super::support::{self, Scene};
use super::transport::{Desktop, KEY, seed_session, until};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as ClientMessage;

/// O nome leva `connect_devices` para sair no mesmo filtro dos outros testes da Task: `Desktop` só existe neste módulo.
#[tokio::test]
async fn connect_devices_revocation_closes_the_remote_browser() {
    if support::isolated(
        "remote_browser::guest::connect_devices_revocation_closes_the_remote_browser",
        |root| assert!(!root.join("git.calls").exists()),
    ).await { return; }
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let (id, token) = scene.state.connect_devices.enroll("celular").unwrap();
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/api/sessions/nav-contract/nav-remoto?token={token}",
        scene.owner.address
    )).await.expect("aparelho aprovado chega ao navegador");
    let mut events = vec![];
    until(&mut socket, &mut events, |v| v["url"] == "http://contract.test/ready").await;
    scene.state.connect_devices.remove(&id).unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let message = socket.next().await.expect("WS encerrou sem Close").unwrap();
            if let ClientMessage::Close(frame) = message { break frame; }
        }
    }).await.expect("o vigia fecha em até 5 s").unwrap();
    assert_eq!(u16::from(frame.code), 4410);
    let _ = socket.close(None).await;
    while socket.next().await.is_some() {}
    desktop.server.close().await;
    scene.close().await;
}

#[tokio::test]
async fn real_guest_gate_allows_share_and_blocks_pair_other_session_and_revocation() {
    use hangar_server::share::store::Kind;
    if support::isolated("remote_browser::guest::real_guest_gate_allows_share_and_blocks_pair_other_session_and_revocation", |root| {
        assert!(!root.join("git.calls").exists());
    }).await { return; }
    let scene = Scene::new().await;
    let client = reqwest::Client::new();
    for name in ["s0","headless"] {
        std::fs::write(scene.nav_dir().join("native.json"),json!({"chave":format!("server::{name}"),"url":"https://example.invalid/página?q=ação#parte"}).to_string()).unwrap();
        let share = scene.guest_token(name,Kind::Share).await;
        let answer = client.get(format!("http://{}/api/sessions/{name}/navegador?token={share}",scene.guest.address)).send().await.unwrap();
        assert_eq!(answer.status().as_u16(),200);
        let corpus = support::contract("url-commands");
        let want = &corpus["cases"].as_array().unwrap().iter()
            .find(|c|c["name"]=="get_signed_share_reaches_handler").unwrap()["expected"]["responses"][0]["body"];
        assert_eq!(answer.json::<Value>().await.unwrap(),*want);
        let other = if name=="s0" {"headless"} else {"s0"};
        let answer = client.get(format!("http://{}/api/sessions/{other}/navegador?token={share}",scene.guest.address)).send().await.unwrap();
        assert_eq!(answer.status().as_u16(),403);
        let pair = scene.guest_token(name,Kind::Pair).await;
        let answer = client.get(format!("http://{}/api/sessions/{name}/navegador?token={pair}",scene.guest.address)).send().await.unwrap();
        assert_eq!(answer.status().as_u16(),403);
        scene.state.share.as_ref().unwrap().store.revoke_session(name, Some(Kind::Share)).unwrap();
        let answer = client.get(format!("http://{}/api/sessions/{name}/navegador?token={share}",scene.guest.address)).send().await.unwrap();
        assert_eq!(answer.status().as_u16(),410);
    }
    scene.upstream.assert_no_browser_or_discovery_calls();
    scene.close().await;
}

#[tokio::test]
async fn share_guest_stays_in_rust_and_revocation_closes_4410() {
    if support::isolated(
        "remote_browser::guest::share_guest_stays_in_rust_and_revocation_closes_4410",
        |root| assert!(!root.join("git.calls").exists()),
    )
    .await
    {
        return;
    }
    use hangar_server::share::store::Kind;
    let scene = Scene::new().await;
    seed_session(&scene);
    let desktop = Desktop::new(&json!({})).await;
    desktop.publish(&scene, KEY);
    let pair = scene.guest_token("nav-contract", Kind::Pair).await;
    let denied = tokio_tungstenite::connect_async(format!(
        "ws://{}/api/sessions/nav-contract/nav-remoto?token={pair}",
        scene.guest.address
    ))
    .await;
    assert!(
        matches!(denied,Err(tokio_tungstenite::tungstenite::Error::Http(ref r)) if r.status()==403)
    );
    let token = scene.guest_token("nav-contract", Kind::Share).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{}/api/sessions/nav-contract/nav-remoto?token={token}",
        scene.guest.address
    ))
    .await
    .expect("convite completo deve chegar ao Rust");
    let mut events = vec![];
    until(&mut socket, &mut events, |v| {
        v["url"] == "http://contract.test/ready"
    })
    .await;
    scene
        .state
        .share
        .as_ref()
        .unwrap()
        .store
        .revoke_session("nav-contract", None)
        .unwrap();
    let frame = tokio::time::timeout(Duration::from_secs(7), async {
        loop {
            let message = socket.next().await.expect("WS encerrou sem Close").unwrap();
            if let ClientMessage::Close(frame) = message {
                break frame;
            }
        }
    })
    .await
    .unwrap()
    .unwrap();
    assert_eq!(u16::from(frame.code), 4410);
    let _ = socket.close(None).await;
    while socket.next().await.is_some() {}
    scene.upstream.assert_no_browser_or_discovery_calls();
    desktop.server.close().await;
    scene.close().await;
}
