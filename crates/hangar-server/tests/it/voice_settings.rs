//! Configuração da voz: disponível com Codex e conta Codex, escolhas gravadas pelo servidor.
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
async fn availability_and_settings_round_trip() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    // A chave antiga do beta, desligada, não esconde mais a voz.
    std::fs::write(home.path().join(".claude/runtime-config.json"), r#"{"codex_voice_beta": false, "jev_api_key": "sk-or-x"}"#).unwrap();
    let addr = crate::voice_support::server(home.path()).await;
    let (status, body) = get(addr, fake::OWNER).await;
    assert_eq!(status, 200);
    // Com conta Codex, só falta o Codex instalado; ele vem da máquina que roda o teste.
    assert_eq!(body["enabled"], body["codex"]);
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
async fn no_codex_account_means_unavailable_and_guest_token_refused() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    // A chave antiga ligada não basta: sem conta Codex a voz não aparece.
    std::fs::write(home.path().join(".claude/runtime-config.json"), r#"{"codex_voice_beta": true}"#).unwrap();
    let (addr, python) = crate::voice_support::server_with_accounts_and_python(home.path(), Vec::new()).await;
    assert_eq!(get(addr, fake::OWNER).await.1["enabled"], false);
    // Quem não é dono é recusado na borda Rust, sem receber a configuração.
    assert_eq!(get(addr, "outro-token").await, (401, json!({"error_code": "voice_owner_only"})));
    assert_eq!(python.hits_to("/api/voice/settings"), 0);
}

#[tokio::test]
async fn memory_is_listed_and_deleted_by_the_owner_only() {
    let home = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(home.path().join(".hangar/voz")).unwrap();
    std::fs::write(home.path().join(".hangar/voz/memoria.json"),
        r#"{"next":1,"entries":[{"id":"m1","kind":"pessoa","text":"Dev","learned":"2026-10-01","confirmed":"2026-10-01"}]}"#).unwrap();
    std::fs::create_dir_all(home.path().join(".claude")).unwrap();
    let (addr, python) = crate::voice_support::server_with_accounts_and_python(home.path(), vec!["default".into()]).await;
    let (status, body) = read(fake::client().get(format!("http://{addr}/api/voice/memory")).bearer_auth(fake::OWNER).send().await.unwrap()).await;
    assert_eq!(status, 200);
    assert_eq!(body["entries"][0]["text"], "Dev");
    // A memória também recusa quem não é dono na borda Rust.
    let (status, body) = read(fake::client().get(format!("http://{addr}/api/voice/memory")).bearer_auth("convidado").send().await.unwrap()).await;
    assert_eq!((status, body), (401, json!({"error_code": "voice_owner_only"})));
    assert_eq!(python.hits_to("/api/voice/memory"), 0);
    let (status, _) = read(fake::client().delete(format!("http://{addr}/api/voice/memory/m1")).bearer_auth(fake::OWNER).send().await.unwrap()).await;
    assert_eq!(status, 200);
    let (status, body) = read(fake::client().delete(format!("http://{addr}/api/voice/memory/m1")).bearer_auth(fake::OWNER).send().await.unwrap()).await;
    assert_eq!((status, body["error_code"].as_str()), (404, Some("memory_unknown")));
    std::fs::write(home.path().join(".hangar/voz/memoria.json"), "{ cortado").unwrap();
    let (status, body) = read(fake::client().get(format!("http://{addr}/api/voice/memory")).bearer_auth(fake::OWNER).send().await.unwrap()).await;
    assert_eq!((status, body["error_code"].as_str()), (500, Some("memory_unreadable")));
}
