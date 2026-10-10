//! Configuração da voz: trava lida do runtime-config, escolhas gravadas pelo servidor.
#![cfg(unix)]
use crate::fake;
use serde_json::{Value, json};

async fn read(r: reqwest::Response) -> (u16, Value) {
    let status = r.status().as_u16();
    (status, serde_json::from_slice(&r.bytes().await.unwrap()).unwrap_or(Value::Null))
}

async fn get(addr: std::net::SocketAddr, token: &str) -> (u16, Value) {
    read(fake::client().get(format!("http://{addr}/api/voice/settings")).bearer_auth(token).send().await.unwrap()).await
}

async fn put(addr: std::net::SocketAddr, body: Value) -> (u16, Value) {
    read(fake::client().put(format!("http://{addr}/api/voice/settings")).bearer_auth(fake::OWNER)
        .header("content-type", "application/json").body(body.to_string()).send().await.unwrap()).await
}

#[tokio::test]
async fn gate_and_settings_round_trip() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    std::fs::write(home.path().join(".claude/runtime-config.json"), r#"{"codex_voice_beta": true, "jev_api_key": "sk-or-x"}"#).unwrap();
    let addr = crate::voice_support::server(home.path()).await;
    let (status, body) = get(addr, fake::OWNER).await;
    assert_eq!(status, 200);
    assert_eq!(body["enabled"], true);
    assert_eq!(body["jev"], true);
    assert_eq!(body["settings"]["codex_account"], "default");
    assert_eq!(body["call"]["active"], false);
    let (status, _) = put(addr, json!({"voice": "sol", "codex_account": "default",
        "organizer": {"direct": {"model": null, "effort": "low", "tier": null}, "plan": {"model": "gpt-x", "effort": "high", "tier": null}}})).await;
    assert_eq!(status, 200);
    let (_, body) = get(addr, fake::OWNER).await;
    assert_eq!(body["settings"]["voice"], "sol");
    assert_eq!(body["settings"]["organizer"]["plan"]["model"], "gpt-x");
}

#[tokio::test]
async fn invalid_voice_and_unknown_account_are_refused() {
    let home = tempfile::tempdir().unwrap();
    let addr = crate::voice_support::server(home.path()).await;
    let base = json!({"voice": null, "codex_account": "default", "organizer": {"direct": {"effort": "low"}, "plan": {"effort": "low"}}});
    let mut bad = base.clone(); bad["voice"] = json!("alloy");
    assert_eq!(put(addr, bad).await, (400, json!({"error_code": "invalid_voice"})));
    let mut bad = base.clone(); bad["codex_account"] = json!("nao-existe");
    assert_eq!(put(addr, bad).await, (400, json!({"error_code": "unknown_account"})));
}

#[tokio::test]
async fn beta_off_by_default_and_guest_token_refused() {
    let home = tempfile::tempdir().unwrap();
    let addr = crate::voice_support::server(home.path()).await;
    assert_eq!(get(addr, fake::OWNER).await.1["enabled"], false);
    // Quem não é dono segue ao Python (o falso responde texto a qualquer rota): nunca recebe a configuração.
    assert_eq!(get(addr, "outro-token").await.1, Value::Null);
}
