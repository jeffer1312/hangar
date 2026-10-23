//! O que acorda o loop e o `then`: as transições de turno do vigia de avisos. Pergunta aberta pausa
//! o loop; fim de turno confirmado (a sessão segue parada e a fila não acabou de digitar nada) dá a
//! volta do loop ou, sem loop ativo, dispara o vínculo `then`. Na subida, os loops ativos voltam:
//! sessão sumida falha, sessão parada ganha a volta. Kimi segue no Python (o fim de turno dele
//! pode ser o marcador velho).
use super::store::{is_active, status, Store, LOCK};
use crate::push::TurnEvent;
use crate::routes::AppState;
use futures_util::future::BoxFuture;
use hangar_api::session::SessionRow;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// O drain do fim de turno pega a entrada em até um quadro; a volta espera isso assentar.
pub const SETTLE: Duration = Duration::from_millis(1500);
const BOOT_TRIES: usize = 15;
const BOOT_RETRY: Duration = Duration::from_secs(2);

pub trait Host: Send + Sync + 'static {
    fn store(&self) -> Store;
    fn row_by_sid<'a>(&'a self, sid: &'a str) -> BoxFuture<'a, Option<SessionRow>>;
    /// A lista inteira; `None` enquanto ela não sabe quem está vivo.
    fn rows(&self) -> BoxFuture<'_, Option<Vec<SessionRow>>>;
    /// A fila entregou algo neste fim de turno (o turno que vem é do texto, não do trabalho).
    fn just_sent<'a>(&'a self, name: &'a str, since: f64) -> BoxFuture<'a, bool>;
    fn tick(&self, row: SessionRow) -> BoxFuture<'static, ()>;
    fn automations(&self) -> bool;
    fn deliver<'a>(&'a self, target: &'a str, text: &'a str) -> BoxFuture<'a, Result<(), String>>;
    fn notify(&self, name: &str, status: &str, reason: &str);
    fn now(&self) -> f64;
    fn diag(&self, event: &'static str, key: &str, code: &str, reason: &'static str);
}

#[derive(Debug, PartialEq)]
pub enum Chain { Off, Absent, Sent, Dropped, Failed(String) }

/// Um salto só: entregue, o vínculo sai; falha de entrega o mantém para o próximo fim de turno.
pub async fn fire_chain(host: &dyn Host, name: &str) -> Chain {
    if !host.automations() { return Chain::Off; }
    let store = host.store();
    let Some(link) = store.chain(name) else { return Chain::Absent };
    let field = |k: &str| link.get(k).and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_owned);
    let (Some(target), Some(text)) = (field("target"), field("text")) else {
        // Torto nunca fica repetindo lixo.
        if let Err(e) = store.clear_chain(name) { tracing::warn!(session = name, kind = ?e.kind(), "then: vínculo torto não saiu"); }
        return Chain::Dropped;
    };
    match host.deliver(&target, &text).await {
        Ok(()) => {
            if let Err(e) = store.clear_chain(name) {
                host.diag("loop.chain_failed", name, "chain_clear_failed", "then entregue, mas o vínculo não saiu e vai repetir");
                tracing::warn!(session = name, kind = ?e.kind(), "then: vínculo entregue não saiu");
            }
            Chain::Sent
        }
        Err(code) => {
            host.diag("loop.chain_failed", name, &code, "then não entregue; o vínculo fica para o próximo fim de turno");
            Chain::Failed(code)
        }
    }
}

pub struct Watcher<H: Host> {
    pub host: Arc<H>,
    pub settle: Duration,
    pending: Mutex<HashMap<String, tokio::task::AbortHandle>>,
    inflight: Arc<Mutex<HashSet<String>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

impl<H: Host> Watcher<H> {
    pub fn new(host: Arc<H>, settle: Duration) -> Arc<Self> {
        Arc::new(Self { host, settle, pending: Mutex::default(), inflight: Arc::default() })
    }

    /// Qualquer transição desarma o fim de turno ainda assentando da mesma sessão.
    pub fn on_event(self: &Arc<Self>, ev: TurnEvent) {
        if let Some(h) = lock(&self.pending).remove(&ev.sid) { h.abort(); }
        let state = ev.state.clone();
        match state.as_str() {
            "awaiting_input" => { let me = self.clone(); tokio::spawn(async move { me.pause(&ev.sid).await }); }
            "idle" => {
                let (me, sid) = (self.clone(), ev.sid.clone());
                let h = tokio::spawn(async move {
                    tokio::time::sleep(me.settle).await;
                    // Daqui em diante a volta não é cancelada no meio.
                    tokio::spawn(async move { me.idle(ev).await });
                });
                lock(&self.pending).insert(sid, h.abort_handle());
            }
            _ => {}
        }
    }

    async fn pause(&self, sid: &str) {
        let Some(row) = self.host.row_by_sid(sid).await else { return };
        let store = self.host.store();
        let _one = LOCK.lock().await;
        if !store.get(&row.name).is_some_and(|d| status(&d) == "running") { return; }
        let mut fields = serde_json::Map::new();
        fields.insert("status".into(), json!("paused_awaiting"));
        if let Err(e) = store.update(&row.name, fields) {
            self.host.diag("loop.pausa_falhou", &row.name, &format!("{:?}", e.kind()), "loop não pausou na pergunta; segue rodando");
        }
    }

    async fn idle(&self, ev: TurnEvent) {
        let Some(row) = self.host.row_by_sid(&ev.sid).await else { return };
        if row.provider == "kimi" { return; }
        let active = self.host.store().get(&row.name).is_some_and(|d| is_active(&d));
        if !active {
            fire_chain(&*self.host, &row.name).await;
            return;
        }
        if row.provider == "claude" && !self.host.just_sent(&row.name, ev.ts).await { self.schedule(row); }
    }

    /// Uma volta em voo por sessão.
    pub fn schedule(&self, row: SessionRow) {
        if !lock(&self.inflight).insert(row.name.clone()) { return; }
        let (inflight, name, work) = (self.inflight.clone(), row.name.clone(), self.host.tick(row));
        tokio::spawn(async move {
            work.await;
            lock(&inflight).remove(&name);
        });
    }

    /// Os loops ativos depois do reinício: sessão sumida falha e avisa; parada ganha a volta.
    pub async fn boot(&self, tries: usize, retry: Duration) {
        let mut rows = None;
        for attempt in 0..tries {
            if attempt > 0 { tokio::time::sleep(retry).await; }
            rows = self.host.rows().await;
            if rows.is_some() { break; }
        }
        let Some(rows) = rows else {
            self.host.diag("loop.boot_resume_falhou", "loops", "list_unavailable", "lista indisponível na subida; loops ativos sem retomar");
            return;
        };
        let live: HashMap<String, SessionRow> = rows.into_iter().map(|r| (super::store::file_stem(&r.name), r)).collect();
        let store = self.host.store();
        for stem in store.stems() {
            let row = {
                let _one = LOCK.lock().await;
                if !store.get(&stem).is_some_and(|d| is_active(&d)) { continue; }
                let Some(row) = live.get(&stem) else {
                    let env = HostEnv { host: &*self.host, name: &stem };
                    if let Err(e) = super::tick::end(&store, &stem, "failed", "sessão morta no boot", &env) {
                        self.host.diag("loop.boot_resume_falhou", &stem, &format!("{:?}", e.kind()), "loop de sessão morta não foi encerrado");
                    }
                    continue;
                };
                row.clone()
            };
            if row.state == "idle" { self.schedule(row); }
        }
    }
}

/// O fim do loop visto pela subida: só o relógio e o aviso.
struct HostEnv<'a> { host: &'a dyn Host, name: &'a str }

impl super::tick::Env for HostEnv<'_> {
    fn now(&self) -> f64 { self.host.now() }
    fn automations(&self) -> bool { self.host.automations() }
    fn branch(&self) -> Option<String> { None }
    fn entry_delivered(&self, _id: Option<String>) -> BoxFuture<'_, Option<bool>> { Box::pin(async { None }) }
    fn run_check(&self, _cmd: String) -> BoxFuture<'_, (i64, String)> { Box::pin(async { (super::check::UNRUNNABLE, String::new()) }) }
    fn last_assistant(&self) -> BoxFuture<'_, Option<String>> { Box::pin(async { None }) }
    fn enqueue(&self, _text: String) -> BoxFuture<'_, Result<(), String>> { Box::pin(async { Err("boot".into()) }) }
    fn notify(&self, status: &str, reason: &str) { self.host.notify(self.name, status, reason); }
}

/// O servidor de verdade.
pub struct Live { pub st: Arc<AppState> }

fn key_of(row: &SessionRow) -> Option<String> { row.jsonl.as_deref().map(crate::locate::session_key) }

impl Host for Live {
    fn store(&self) -> Store { super::store(&self.st) }
    fn row_by_sid<'a>(&'a self, sid: &'a str) -> BoxFuture<'a, Option<SessionRow>> {
        Box::pin(async move {
            for fresh in [false, true] {
                let produced = if fresh { self.st.list.refresh().await } else { self.st.list.snapshot().await }.ok()?;
                if let Some(row) = produced.rows.iter().find(|r| key_of(r).as_deref() == Some(sid)) { return Some(row.clone()); }
            }
            None
        })
    }
    fn rows(&self) -> BoxFuture<'_, Option<Vec<SessionRow>>> {
        Box::pin(async move {
            let produced = self.st.list.snapshot().await.ok()?;
            // Sem os fatos, sessão viva pode faltar: não dá para dizer quem morreu.
            (!produced.facts.unknown).then(|| produced.rows.to_vec())
        })
    }
    fn just_sent<'a>(&'a self, name: &'a str, since: f64) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let Some(runtime) = self.st.state.runtime.get() else { return false };
            let id = format!("loop-load:{}", crate::mods::state::random_hex(8));
            let Ok(Value::Array(rows)) = runtime.queue_action(name, id, crate::runtime::queue::Action::Load).await else { return false };
            // Margem para o relógio do hook e o do servidor.
            rows.iter().any(|r| ["claimed_ts", "dispatched_ts"].iter().any(|k| r[*k].as_f64().is_some_and(|t| t >= since - 1.0)))
        })
    }
    fn tick(&self, row: SessionRow) -> BoxFuture<'static, ()> {
        let st = self.st.clone();
        Box::pin(async move {
            let name = row.name.clone();
            let env = super::Live { st: st.clone(), row };
            if let Err(e) = super::tick::run_tick(&super::store(&st), &name, &env).await {
                st.diag.report("loop.tick_failed", &name, &format!("{:?}", e.kind()), "volta do loop não gravou; o loop ficou como estava");
            }
        })
    }
    fn automations(&self) -> bool { super::automations(&self.st) }
    fn deliver<'a>(&'a self, target: &'a str, text: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            crate::session_write::input::deliver_text(&self.st, target, text).await
                .map_err(|d| d["code"].as_str().unwrap_or("erro_envio_falhou_desconhecida").to_owned())
        })
    }
    fn notify(&self, name: &str, status: &str, reason: &str) { super::notify(&self.st, name, status, reason); }
    fn now(&self) -> f64 { crate::nav_requests::now() }
    fn diag(&self, event: &'static str, key: &str, code: &str, reason: &'static str) {
        tracing::warn!(code, session = key, "{reason}");
        self.st.diag.report(event, key, code, reason);
    }
}

/// Liga o vigia do loop e do `then`; vive enquanto os `JoinHandle` não forem abortados.
pub fn start(st: Arc<AppState>) -> Vec<tokio::task::JoinHandle<()>> {
    if st.state.runtime.get().is_none() { return Vec::new(); }
    let watcher = Watcher::new(Arc::new(Live { st }), SETTLE);
    let mut rx = crate::push::subscribe();
    let w = watcher.clone();
    let events = tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(ev) => w.on_event(ev),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::warn!(code = "loop_turns_lagged", perdidas = n, "loop: transições de turno perdidas");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            }
        }
    });
    let boot = tokio::spawn(async move { watcher.boot(BOOT_TRIES, BOOT_RETRY).await });
    vec![events, boot]
}

#[cfg(test)]
mod tests;
