//! A voz pelo WebSocket do dono: um aparelho falso, um app-server falso e a API do servidor atrás.
#![cfg(unix)]
use crate::{fake, voice_support};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;

type Ws = tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(addr: std::net::SocketAddr, token: &str) -> Result<Ws, String> {
    tokio_tungstenite::connect_async(format!("ws://{addr}/api/voice?token={token}")).await.map(|(ws, _)| ws).map_err(|e| e.to_string())
}

async fn send(ws: &mut Ws, v: Value) { ws.send(Message::Text(v.to_string().into())).await.unwrap(); }

async fn until(ws: &mut Ws, kind: &str) -> Value {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await.expect("prazo").expect("fim").unwrap();
        if let Message::Text(t) = msg { let v: Value = serde_json::from_str(&t).unwrap(); if v["type"] == kind { return v; } }
    }
}

async fn hello(ws: &mut Ws, client: &str, caps: Value) {
    send(ws, json!({"type": "hello", "client": client, "screen": {"server": "", "name": "hangar"}, "caps": caps})).await;
    send(ws, json!({"type": "offer", "sdp": format!("v=0 {client}")})).await;
}

#[tokio::test]
async fn owner_opens_call_and_gets_answer() {
    let t = voice_support::voice_server(true).await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "pwa", json!(["switch_session"])).await;
    assert!(until(&mut a, "answer").await["sdp"].as_str().unwrap().starts_with("v=0 answer"));
    assert!(t.app_server_saw("thread/start"));
    let tools = t.last("thread/start")["params"]["dynamicTools"].clone();
    assert!(!tools.as_array().unwrap().iter().any(|x| x["name"] == "read_screen"));
}

#[tokio::test]
async fn disabled_beta_closes_with_code() {
    let t = voice_support::voice_server(false).await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "pwa", json!([])).await;
    assert_eq!(until(&mut a, "error").await["code"], "disabled");
}

#[tokio::test]
async fn guest_and_wrong_token_never_reach_the_call() {
    let t = voice_support::voice_server(true).await;
    assert!(open(t.addr, "convidado").await.is_err());
    assert!(!t.app_server_saw("initialize"));
}

#[tokio::test]
async fn second_device_takes_over_and_first_gets_taken() {
    let t = voice_support::voice_server(true).await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "native", json!(["switch_session", "read_screen"])).await;
    until(&mut a, "answer").await;
    send(&mut a, json!({"type": "live"})).await;
    let mut b = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut b, "pwa", json!(["switch_session"])).await;
    assert_eq!(until(&mut a, "taken").await["type"], "taken");
    assert!(until(&mut b, "answer").await["sdp"].is_string());
    assert_eq!(t.count("thread/start"), 1, "mesma thread do organizador");
    assert_eq!(t.count("thread/realtime/start"), 2);
}

#[tokio::test]
async fn screen_tool_without_capability_is_refused() {
    let t = voice_support::voice_server(true).await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "native", json!(["switch_session", "read_screen"])).await;
    until(&mut a, "answer").await;
    let mut b = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut b, "pwa", json!(["switch_session"])).await;
    until(&mut b, "answer").await;
    t.spoken_turn("turn-1", "lê a tela");
    t.tool(41, "read_screen", json!({}), "turn-1");
    let reply = t.reply_to(41).await;
    assert_eq!(reply["success"], false);
    assert!(reply["contentItems"][0]["text"].as_str().unwrap().contains("Este aparelho"));
}

/// O servidor de teste usa prazo de 400 ms no lugar dos 2 min (relógio real; `start_paused` com TCP e reqwest estoura prazos à toa).
#[tokio::test]
async fn device_drop_keeps_call_for_grace_and_resumes() {
    let t = voice_support::voice_server(true).await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "pwa", json!(["switch_session"])).await;
    until(&mut a, "answer").await;
    send(&mut a, json!({"type": "live"})).await;
    drop(a);
    // Como o app-server real: sem o aparelho, a conversa falada fecha.
    t.push(json!({"method": "thread/realtime/closed", "params": {"threadId": "t1", "reason": "peer_gone"}}));
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let mut b = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut b, "pwa", json!(["switch_session"])).await;
    until(&mut b, "answer").await;
    assert_eq!(t.count("thread/start"), 1, "voltou na mesma chamada");
    drop(b);
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    // Parar derruba o `Rpc` no meio (sem `realtime/stop`): o app-server falso vê o fim da conexão.
    assert!(t.app_server_sees("<eof>").await, "sem aparelho além do prazo, encerra");
}

#[tokio::test]
async fn beta_turned_off_mid_call_ends_with_disabled() {
    let t = voice_support::voice_server(true).await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "pwa", json!(["switch_session"])).await;
    until(&mut a, "answer").await;
    t.set_beta(false);
    assert_eq!(until(&mut a, "error").await["code"], "disabled");
    assert!(t.app_server_sees("<eof>").await, "o app-server cai junto");
}

#[tokio::test]
async fn call_that_dies_tells_the_device_why() {
    let t = voice_support::voice_server_failing_spawn().await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "pwa", json!(["switch_session"])).await;
    let error = until(&mut a, "error").await;
    assert_eq!(error["code"], "app_server");
    assert!(error["detail"].is_string());
    until(&mut a, "closed").await;
}

#[tokio::test]
async fn missing_account_is_refused_without_falling_back() {
    let t = voice_support::voice_server_with_account(true, "conta-que-sumiu").await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "pwa", json!([])).await;
    assert_eq!(until(&mut a, "error").await["code"], "account_missing");
    assert!(!t.app_server_saw("initialize"));
}

#[tokio::test]
async fn session_tool_goes_through_own_api_to_python() {
    // `set_self` aponta para o próprio servidor de teste: o envio passa pela rota real do Rust e chega ao Python falso.
    let t = voice_support::voice_server_self_api(true).await;
    let mut a = open(t.addr, fake::OWNER).await.unwrap();
    hello(&mut a, "pwa", json!(["switch_session"])).await;
    until(&mut a, "answer").await;
    t.spoken_turn("turn-1", "manda pra hangar: roda os testes");
    t.tool(42, "send_to_session", json!({"request": "roda os testes"}), "turn-1");
    let _ = t.reply_to(42).await;
    assert!(t.python.input_calls() >= 1, "chegou ao /input pelo caminho real");
}
