//! `POST /api/keyring/unlock` e o evento `keyring` da lista do dono, com o chaveiro falso: a senha
//! chega inteira ao chaveiro, só do dono, e nunca vai ao Python nem volta numa resposta.
#![cfg(unix)]
use crate::fake::{OWNER, client, next_named};
use crate::list_support::{open, server};
use hangar_server::keyring::{Backend, KeyringState, UnlockError};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

const PASSWORD: &str = "senha-do-chaveiro-7Hq";

#[derive(Default)]
struct FakeKeyring {
    locked: Mutex<bool>,
    received: Mutex<Vec<Vec<u8>>>,
    /// O daemon aceita a senha mas segue trancado até reiniciar (prompt que travou o daemon).
    stuck: Mutex<bool>,
    restarts: Mutex<u32>,
}

impl Backend for FakeKeyring {
    fn state(&self) -> Option<KeyringState> {
        Some(if *self.locked.lock().unwrap() { KeyringState::Locked } else { KeyringState::Unlocked })
    }
    fn unlock(&self, password: &[u8]) -> Result<(), UnlockError> {
        self.received.lock().unwrap().push(password.to_vec());
        if password != PASSWORD.as_bytes() { return Err(UnlockError::Denied); }
        if !*self.stuck.lock().unwrap() { *self.locked.lock().unwrap() = false; }
        Ok(())
    }
    fn restart(&self) -> bool {
        *self.restarts.lock().unwrap() += 1;
        *self.stuck.lock().unwrap() = false;
        true
    }
}

fn locked() -> Arc<FakeKeyring> {
    let k = Arc::new(FakeKeyring::default());
    *k.locked.lock().unwrap() = true;
    k
}

async fn post(addr: std::net::SocketAddr, token: Option<&str>, headers: &[(&str, &str)], body: Value) -> (u16, Value) {
    let mut req = client().post(format!("http://{addr}/api/keyring/unlock")).json(&body);
    if let Some(t) = token { req = req.bearer_auth(t); }
    for (k, v) in headers { req = req.header(*k, *v); }
    let r = req.send().await.unwrap();
    let status = r.status().as_u16();
    let text = r.text().await.unwrap();
    assert!(!text.contains(PASSWORD), "a senha voltou numa resposta: {text}");
    (status, serde_json::from_str(&text).unwrap_or(Value::Null))
}

#[tokio::test(flavor = "multi_thread")]
async fn owner_unlocks_and_the_list_publishes_the_change() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let keyring = locked();
    srv.list.keyring.set_backend(keyring.clone());
    srv.list.keyring.refresh();
    let mut es = open(srv.addr).await;
    let ev = next_named(&mut es, "keyring").await;
    assert_eq!(serde_json::from_str::<Value>(&ev.data).unwrap(), json!({"state": "locked"}));

    let (status, body) = post(srv.addr, Some(OWNER), &[], json!({"password": "errada"})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (403, json!("keyring_wrong_password")));

    let (status, body) = post(srv.addr, Some(OWNER), &[], json!({"password": PASSWORD})).await;
    assert_eq!((status, body), (200, json!({"state": "unlocked"})));
    assert_eq!(keyring.received.lock().unwrap().last().unwrap(), PASSWORD.as_bytes());
    let ev = next_named(&mut es, "keyring").await;
    assert_eq!(serde_json::from_str::<Value>(&ev.data).unwrap(), json!({"state": "unlocked"}));
    assert_eq!(*keyring.restarts.lock().unwrap(), 0, "abriu sem reiniciar o daemon");
    assert_eq!(srv.python.hits_to("/api/keyring/unlock"), 0);
}

/// Senha aceita e chaveiro ainda trancado: reinicia o daemon e manda a senha de novo, uma vez.
#[tokio::test(flavor = "multi_thread")]
async fn accepted_but_still_locked_restarts_the_daemon_once() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let keyring = locked();
    *keyring.stuck.lock().unwrap() = true;
    srv.list.keyring.set_backend(keyring.clone());
    let (status, body) = post(srv.addr, Some(OWNER), &[], json!({"password": PASSWORD})).await;
    assert_eq!((status, body), (200, json!({"state": "unlocked"})));
    assert_eq!(*keyring.restarts.lock().unwrap(), 1);
    assert_eq!(keyring.received.lock().unwrap().len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_owner_and_never_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let keyring = locked();
    srv.list.keyring.set_backend(keyring.clone());
    let body = json!({"password": PASSWORD});
    assert_eq!(post(srv.addr, None, &[], body.clone()).await.0, 401);
    assert_eq!(post(srv.addr, Some("outro"), &[], body.clone()).await.0, 401);
    assert_eq!(post(srv.addr, Some(OWNER), &[("x-hangar-guest", "assinado")], body.clone()).await.0, 403);
    let (status, b) = post(srv.addr, Some(OWNER), &[], json!({"senha": PASSWORD})).await;
    assert_eq!((status, b["detail"]["code"].clone()), (422, json!("keyring_bad_request")));
    assert_eq!(post(srv.addr, Some(OWNER), &[], json!({"password": ""})).await.0, 422);
    assert!(keyring.received.lock().unwrap().is_empty());
    assert_eq!(srv.python.hits_to("/api/keyring/unlock"), 0);
}
