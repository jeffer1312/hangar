//! Bancada: host real com o conector de prova, Tickets numa pasta temporária e sessões de mentira.
#![allow(dead_code)]
use hangar_connectors::host::{Host, HostOptions};
use hangar_connectors::manifest::Manifest;
use hangar_connectors::registry::Registry;
use hangar_mission::Store;
use hangar_mission::store::Link;
use hangar_work_review::{Diag, Engine, SessionRef, Sessions, Task};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const FIXTURE: &str = env!("CARGO_BIN_EXE_hangar-work-review-fixture");

#[derive(Default)]
pub struct FakeSessions(pub Mutex<HashMap<String, SessionRef>>);

impl Sessions for FakeSessions {
    /// `ilegivel-*`: a leitura da lista ou da identidade falhou.
    fn resolve(&self, name: String) -> Task<Result<Option<SessionRef>, String>> {
        let found = if name.starts_with("ilegivel-") { Err("list_unavailable".to_owned()) } else { Ok(self.0.lock().unwrap().get(&name).cloned()) };
        Box::pin(async move { found })
    }
}

#[derive(Default)]
pub struct FakeDiag(pub Mutex<Vec<(String, String, String)>>, pub tokio::sync::Notify);

impl Diag for FakeDiag {
    fn report(&self, code: &str, session: &str, detail: &str) {
        self.0.lock().unwrap().push((code.into(), session.into(), detail.into()));
        self.1.notify_one();
    }
}

impl FakeDiag {
    pub async fn reported(&self) {
        tokio::time::timeout(Duration::from_secs(15), self.1.notified()).await.expect("nada foi ao diário");
    }
}

pub struct Bench {
    pub dir: tempfile::TempDir,
    pub host: Arc<Host>,
    pub store: Arc<Store>,
    pub sessions: Arc<FakeSessions>,
    pub diag: Arc<FakeDiag>,
    pub engine: Arc<Engine>,
}

impl Bench {
    pub fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let options = HostOptions { init_timeout: Duration::from_secs(10), call_timeout: Duration::from_secs(10), idle: Duration::from_secs(60), exit_grace: Duration::from_secs(1) };
        let host = Host::new(Registry::new(dir.path().join("connectors.json")), options);
        let store = Arc::new(Store::new(dir.path().join("mission")));
        let (sessions, diag) = (Arc::new(FakeSessions::default()), Arc::new(FakeDiag::default()));
        let engine = Engine::new(host.clone(), store.clone(), sessions.clone(), diag.clone(), dir.path().join("cache"));
        Self { dir, host, store, sessions, diag, engine }
    }

    pub fn path(&self, name: &str) -> PathBuf { self.dir.path().join(name) }

    /// Conector com a revisão lida de `review-<id>.json` e cada pedido gravado em `calls-<id>`.
    pub fn connector(&self, id: &str, review: bool, args: &[&str]) {
        let mut command = vec![FIXTURE.to_owned(), "--id".into(), id.to_owned(),
            "--calls-file".into(), self.path(&format!("calls-{id}")).to_string_lossy().into_owned()];
        if review {
            command.extend(["--review".into(), "on".into(), "--review-file".into(), self.path(&format!("review-{id}.json")).to_string_lossy().into_owned()]);
            self.set_review(id, Value::Null);
        }
        command.extend(args.iter().map(|arg| arg.to_string()));
        let mut value = json!({"manifest_version": 1, "id": id, "name": id, "version": "1", "protocol_version": 1,
            "capabilities": {"work_tracking": 1}, "command": command});
        if review { value["capabilities"]["work_review"] = json!(1); }
        self.host.add(Manifest::parse(value.to_string().as_bytes(), self.dir.path()).unwrap()).unwrap();
    }

    pub fn set_review(&self, connector: &str, review: Value) {
        std::fs::write(self.path(&format!("review-{connector}.json")), review.to_string()).unwrap();
    }

    /// Pedidos que o conector recebeu com este método, na ordem.
    pub fn calls(&self, connector: &str, method: &str) -> Vec<Value> {
        std::fs::read_to_string(self.path(&format!("calls-{connector}"))).unwrap_or_default().lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .filter(|call| call["method"] == method).map(|call| call["params"].clone()).collect()
    }

    /// Espera o conector receber um pedido deste método (o arquivo é gravado na chegada, antes do atraso).
    pub async fn wait_call(&self, connector: &str, method: &str) {
        tokio::time::timeout(Duration::from_secs(15), async {
            // estavel: o sinal é o arquivo que o processo do conector grava ao receber o pedido; sem canal entre processos, confere a condição com teto de 15 s
            while self.calls(connector, method).is_empty() { tokio::time::sleep(Duration::from_millis(20)).await; }
        }).await.expect("o pedido não chegou ao conector");
    }

    pub fn session(&self, name: &str, life: &str, branch: Option<&str>) -> SessionRef {
        let session = SessionRef { name: name.into(), lifecycle_id: Some(life.into()), key: Some(life.into()), group_id: None, branch: branch.map(str::to_owned) };
        self.sessions.0.lock().unwrap().insert(name.into(), session.clone());
        session
    }

    pub fn link(&self, connector: &str, ticket: &str, session: &SessionRef) {
        let link = Link { key: format!("PM-{ticket}"), title: "Ticket de prova".into(), url: "https://help.invalid/t".into(), session: session.name.clone() };
        self.store.link_owner(connector, ticket, link, session.key.as_deref(), &[]).unwrap();
    }

    /// Espera o resumo publicado mudar; o teto só estoura se o engine travar.
    pub async fn changed(&self) {
        tokio::time::timeout(Duration::from_secs(15), self.engine.changed().notified()).await.expect("o resumo não mudou");
    }

    pub fn summaries(&self, current: &[(&str, Option<&str>)]) -> Value { self.engine.summaries_for(current)["reviews"].clone() }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let host = self.host.clone();
        std::thread::spawn(move || tokio::runtime::Runtime::new().unwrap().block_on(host.shutdown())).join().unwrap();
    }
}

pub fn side(asset: &str) -> Value { json!({"label": "Mock", "asset": asset, "width": 160, "height": 90}) }

pub fn decision(id: &str) -> Value {
    json!({"id": id, "title": format!("Diferença {id}"), "left": side("a:1"),
        "choices": [{"id": "igualar", "label": "Igualar"}, {"id": "manter", "label": "Manter", "requires_reason": true}]})
}

pub fn review(id: &str, state: &str, decisions: &[&str], updated_at: &str) -> Value {
    let decisions: Vec<Value> = decisions.iter().map(|id| decision(id)).collect();
    json!({"id": id, "title": "Revisão", "updated_at": updated_at, "state": state, "decisions": decisions,
        "badge": {"text": "2 para você", "tone": "warning"}})
}
