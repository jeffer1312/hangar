#![cfg(unix)]
use super::support::{self, OWNER, Scene, Server};
use super::transport::{Desktop, KEY, seed_session, until};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message as ClientMessage;

#[tokio::test]
async fn private_url_bridge_never_promotes_login_guest_to_owner() {
    if support::isolated(
        "remote_browser::guest::private_url_bridge_never_promotes_login_guest_to_owner",
        |root| assert!(!root.join("git.calls").exists()),
    ).await { return; }
    let scene = Scene::new().await;
    let corpus = support::contract("url-commands");
    let case = corpus["cases"].as_array().unwrap().iter()
        .find(|case| case["name"] == "get_login_guest_is_blocked").unwrap();
    let name = case["inputs"]["session"].as_str().unwrap();
    let sidecar = case["inputs"]["files"]["native.json"]["content"].to_string();
    std::fs::write(scene.nav_dir().join("native.json"), &sidecar).unwrap();
    let private = Server::owner(hangar_server::routes::terminal_router(scene.state.clone())).await;
    let response = reqwest::Client::new()
        .get(format!("http://{}/__hangar_server/forwarded?token={OWNER}", private.address))
        .bearer_auth(OWNER)
        .header("x-hangar-internal", support::SECRET)
        .header("x-hangar-path", format!("/api/sessions/{name}/navegador"))
        .header("x-hangar-forwarded-guest", "1")
        .send().await.unwrap();
    let expected = &case["expected"]["responses"][0];
    assert_eq!(json!(response.status().as_u16()), expected["status"]);
    for (key, value) in expected["headers"].as_object().unwrap() {
        assert_eq!(response.headers().get(key).and_then(|value| value.to_str().ok()), value.as_str());
    }
    assert_eq!(response.json::<Value>().await.unwrap(), expected["body"]);
    assert_eq!(std::fs::read_to_string(scene.nav_dir().join("native.json")).unwrap(), sidecar);
    assert!(!scene.root.path().join("calls.log").exists(), "recusa consultou a descoberta");
    scene.upstream.assert_no_browser_or_discovery_calls();
    private.close().await;
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
