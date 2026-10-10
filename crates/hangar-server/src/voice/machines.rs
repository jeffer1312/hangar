//! Onde a voz age: este servidor pela própria API HTTP (as mesmas rotas que os apps usam) e os peers.
use super::rules::{Delivery, PairResult};
use crate::groups::peers::PeerClient;
use hangar_api::{chat::ChatEvent, session::SessionRow};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};
use std::{collections::HashSet, net::{Ipv4Addr, SocketAddr}, sync::Arc, time::Duration};

/// Chave da máquina da voz.
pub const HERE: &str = "";

#[derive(Clone)]
pub struct SelfApi { base: String, token: String, http: reqwest::Client }

impl SelfApi {
    pub fn new(addr: SocketAddr, token: &str) -> Self {
        let addr = if addr.ip().is_unspecified() { SocketAddr::new(Ipv4Addr::LOCALHOST.into(), addr.port()) } else { addr };
        // Sem proxy nem redirect: o token do dono nunca sai deste servidor.
        let http = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(120)).build().expect("cliente http local");
        Self { base: format!("http://{addr}"), token: token.to_owned(), http }
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        let mut req = self.http.request(method, format!("{}{path}", self.base)).bearer_auth(&self.token);
        if let Some(body) = body {
            req = req.header(reqwest::header::CONTENT_TYPE, "application/json").body(body.to_string());
        }
        let resp = req.send().await.map_err(|_| "O servidor não respondeu.".to_owned())?;
        let status = resp.status();
        let value: Value = resp.bytes().await.ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or(Value::Null);
        if status.is_success() { return Ok(value); }
        Err(value["detail"]["message"].as_str()
            .or_else(|| value["detail"].as_str())
            .or_else(|| value["message"].as_str())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("O servidor recusou ({status}).")))
    }
}

pub struct Machines { pub own: SelfApi, pub peers: Arc<PeerClient>, pub own_label: String }

fn enc(name: &str) -> String { utf8_percent_encode(name, NON_ALPHANUMERIC).to_string() }

impl Machines {
    pub async fn call(&self, machine: &str, method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value, String> {
        if machine == HERE { return self.own.call(method, path, body).await; }
        self.peers.call(machine, method, path, body.as_ref()).await.map(|v| v.unwrap_or(Value::Null)).map_err(|e| e.text(machine))
    }

    pub fn label(&self, machine: &str) -> String {
        if machine == HERE { self.own_label.clone() } else { machine.to_owned() }
    }

    /// `(máquina, linha)` de todas as máquinas que responderam, e o rótulo das que não.
    pub async fn sessions(&self) -> (Vec<(String, SessionRow)>, Vec<String>) {
        let (rows, unreachable, _) = self.sessions_except(&HashSet::new()).await;
        (rows, unreachable)
    }

    /// Como `sessions`, sem ler as máquinas de `skip` (contam como sem resposta); o terceiro item são os peers que falharam agora.
    pub async fn sessions_except(&self, skip: &HashSet<String>) -> (Vec<(String, SessionRow)>, Vec<String>, Vec<String>) {
        let machines: Vec<String> = std::iter::once(HERE.to_owned()).chain(self.peers.enabled_ids()).collect();
        let (read, skipped): (Vec<String>, Vec<String>) = machines.into_iter().partition(|m| !skip.contains(m));
        let reads = futures_util::future::join_all(read.iter().map(|m| self.call(m, reqwest::Method::GET, "/api/sessions", None))).await;
        let (mut rows, mut failed) = (Vec::new(), Vec::new());
        let mut unreachable: Vec<String> = skipped.iter().map(|m| self.label(m)).collect();
        for (machine, read) in read.into_iter().zip(reads) {
            match read.ok().and_then(|v| serde_json::from_value::<Vec<SessionRow>>(v).ok()) {
                Some(list) => rows.extend(list.into_iter().map(|r| (machine.clone(), r))),
                None => {
                    unreachable.push(self.label(&machine));
                    if machine != HERE { failed.push(machine); }
                }
            }
        }
        (rows, unreachable, failed)
    }

    pub async fn history(&self, machine: &str, name: &str, limit: usize) -> Result<Vec<ChatEvent>, String> {
        let path = format!("/api/sessions/{}/history?limit={limit}", enc(name));
        let value = self.call(machine, reqwest::Method::GET, &path, None).await?;
        serde_json::from_value(value).map_err(|_| "Resposta inválida do histórico.".to_owned())
    }

    pub async fn send(&self, machine: &str, name: &str, text: &str) -> Result<Delivery, String> {
        let path = format!("/api/sessions/{}/input", enc(name));
        let value = self.call(machine, reqwest::Method::POST, &path, Some(json!({"text": text, "steer": false}))).await?;
        let delivery: Delivery = serde_json::from_value(value).map_err(|_| "Resposta inválida do envio.".to_owned())?;
        if delivery.ok { Ok(delivery) } else { Err("A sessão recusou o pedido.".to_owned()) }
    }

    pub async fn pair(&self, machine: &str, target: &str, origin: &str) -> Result<PairResult, String> {
        let path = format!("/api/sessions/{}/pair", enc(target));
        let body = json!({"peers": [origin], "task": "", "replace_task": false});
        self.call(machine, reqwest::Method::POST, &path, Some(body)).await.map(|v| PairResult::from_value(&v))
    }

    pub async fn unpair(&self, machine: &str, name: &str) -> Result<PairResult, String> {
        let path = format!("/api/sessions/{}/pair", enc(name));
        self.call(machine, reqwest::Method::DELETE, &path, None).await.map(|v| PairResult::from_value(&v))
    }

    pub async fn close(&self, machine: &str, name: &str) -> Result<(), String> {
        let path = format!("/api/sessions/{}", enc(name));
        self.call(machine, reqwest::Method::DELETE, &path, None).await.map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::groups::peers::PeerBook;
    use axum::{Router, http::header::CONTENT_TYPE, response::IntoResponse, routing::{get, post}};

    // O axum daqui é sem a feature `json`.
    fn js(v: Value) -> axum::response::Response { ([(CONTENT_TYPE, "application/json")], v.to_string()).into_response() }

    async fn fake_api() -> SocketAddr {
        let app = Router::new()
            .route("/api/sessions", get(|| async { js(json!([{"name": "hangar", "provider": "claude", "state": "idle"}])) }))
            .route("/api/sessions/{n}/input", post(|b: String| async move {
                assert_eq!(serde_json::from_str::<Value>(&b).unwrap()["steer"], false);
                js(json!({"ok": true, "delivered": true}))
            }))
            .route("/api/sessions/{n}/history", get(|| async { js(json!([{"id": "1", "kind": "assistant_msg", "text": "feito"}])) }));
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
        addr
    }

    fn machines(addr: SocketAddr) -> Machines {
        Machines { own: SelfApi::new(addr, "t"), peers: Arc::new(PeerClient::new(PeerBook::new(None))), own_label: "casa".into() }
    }

    #[tokio::test]
    async fn here_lists_sends_and_reads_history() {
        let m = machines(fake_api().await);
        let (rows, unreachable) = m.sessions().await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, HERE);
        assert!(unreachable.is_empty());
        assert_eq!(m.send(HERE, "hangar", "oi").await.unwrap(), Delivery { ok: true, delivered: true });
        assert_eq!(m.history(HERE, "hangar", 20).await.unwrap()[0].text.as_deref(), Some("feito"));
        assert_eq!(m.label(HERE), "casa");
    }

    #[tokio::test]
    async fn unknown_peer_is_an_error_text_not_a_panic() {
        let m = machines("127.0.0.1:9".parse().unwrap());
        assert!(m.send("vps", "x", "oi").await.is_err());
    }
}
