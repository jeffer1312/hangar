//! Liga o `hangar-auto-continue` ao servidor: a agenda das paradas passageiras recebe o fim de turno do
//! runtime, lê os fatos da sessão pela lista interna e pela fila, manda o "continue" pelo `/input` e escreve
//! a nota pela fila. O registro é o mesmo da troca de conta no limite.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use hangar_api::session::SessionRow;
use hangar_auto_continue::scheduler::BoxFuture;
use hangar_auto_continue::{Command, Facts, Host, Ledger, StopHit, ledger, text};
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::list::hub::InternalSubscription;
use crate::routes::AppState;
use crate::runtime::gateway::RuntimeRegistry;

const EVENT: &str = "session.auto_continue";
/// De quanto em quanto se confere se alguém ainda precisa da lista produzindo sem cliente aberto.
const HOLD_CHECK: Duration = Duration::from_secs(60);
/// Espera pela primeira produção da lista quando a assinatura acabou de nascer.
const FIRST_LIST: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(250);

/// Hora e agente da última parada de cada sessão: a fila diz se a pessoa escreveu depois dela.
type Stops = Arc<Mutex<HashMap<String, (f64, String)>>>;

/// O que o resto do servidor usa: mandar à agenda e o registro dividido com o `limit_switch`.
pub struct AutoContinue {
    commands: UnboundedSender<Command>,
    stops: Stops,
    pub ledger: Arc<Mutex<Ledger>>,
}

impl AutoContinue {
    pub fn stop(&self, hit: StopHit) {
        lock(&self.stops).insert(hit.name.clone(), (hit.at, hit.provider.clone()));
        let _ = self.commands.send(Command::Stop(hit));
    }

    pub fn cancel(&self, name: &str, why: &'static str) {
        let _ = self.commands.send(Command::Cancel { name: name.to_owned(), why });
    }

    pub fn forget(&self, name: &str) {
        lock(&self.stops).remove(name);
        let _ = self.commands.send(Command::Forget(name.to_owned()));
    }

    pub fn rename(&self, old: &str, new: &str) {
        let mut stops = lock(&self.stops);
        if let Some(stop) = stops.remove(old) { stops.insert(new.to_owned(), stop); }
        drop(stops);
        let _ = self.commands.send(Command::Rename { old: old.to_owned(), new: new.to_owned() });
    }
}

/// A sessão fechou de vez.
pub(crate) fn forget(st: &AppState, name: &str) {
    if let Some(auto) = st.auto_continue.get() { auto.forget(name); }
}

pub(crate) fn renamed(st: &AppState, old: &str, new: &str) {
    if let Some(auto) = st.auto_continue.get() { auto.rename(old, new); }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { mutex.lock().unwrap_or_else(PoisonError::into_inner) }

/// A agenda, o repasse das paradas do runtime e quem segura a lista. Sem runtime não há o que continuar.
pub fn spawn(st: Arc<AppState>) -> Vec<JoinHandle<()>> {
    let Some(runtime) = st.state.runtime.get().cloned() else { return Vec::new() };
    let ledger = Arc::new(Mutex::new(Ledger::load(ledger::path(&st.accounts.env.home))));
    // A espera gravada antes do reinício volta com a hora da parada: sem ela a fila não diria quem escreveu depois.
    let stops: Stops = Arc::new(Mutex::new(lock(&ledger).parked().into_iter()
        .map(|waiting| (waiting.hit.name, (waiting.hit.at, waiting.hit.provider))).collect()));
    let host = Arc::new(Native { st: st.clone(), runtime: runtime.clone(), stops: stops.clone(), sub: Mutex::new(None) });
    let (commands, scheduler) = hangar_auto_continue::spawn(host.clone(), ledger.clone());
    let auto = Arc::new(AutoContinue { commands, stops, ledger: ledger.clone() });
    if st.auto_continue.set(auto.clone()).is_err() { return Vec::new() }
    let (sink, mut hits) = tokio::sync::mpsc::unbounded_channel();
    runtime.set_stop_sink(sink);
    let forward = tokio::spawn(async move {
        while let Some(hit) = hits.recv().await { auto.stop(hit); }
    });
    let holder = tokio::spawn(hold(host, ledger));
    vec![scheduler, forward, holder]
}

/// O `ListHub` só produz enquanto há assinatura. Quem precisa dela sem cliente aberto: a sessão parada à
/// espera da rede (os fatos) e a sessão Claude com terminal (a parada dela é lida pela produção da lista).
async fn hold(host: Arc<Native>, ledger: Arc<Mutex<Ledger>>) {
    loop {
        let waiting = !lock(&ledger).parked().is_empty();
        let need = if waiting { Some(true) } else {
            // Descoberta que falha não decide: fica como estava.
            host.st.list.discover(None).await.ok().map(|rows| rows.iter().any(|row| row.provider == "claude" && !row.headless))
        };
        match need {
            Some(true) => { host.subscription(); }
            Some(false) => *lock(&host.sub) = None,
            None => {}
        }
        tokio::time::sleep(HOLD_CHECK).await;
    }
}

struct Native {
    st: Arc<AppState>,
    runtime: Arc<RuntimeRegistry>,
    stops: Stops,
    sub: Mutex<Option<Arc<InternalSubscription>>>,
}

impl Native {
    fn subscription(&self) -> Arc<InternalSubscription> {
        lock(&self.sub).get_or_insert_with(|| Arc::new(self.st.hub.subscribe_internal(&self.st.list, &self.st.devices, &self.st.diag))).clone()
    }

    /// As linhas da lista do dono; a assinatura recém-nascida espera a primeira produção, e a lista em erro
    /// cai na produção avulsa.
    async fn rows(&self) -> Option<Arc<Vec<SessionRow>>> {
        let sub = self.subscription();
        let start = tokio::time::Instant::now();
        loop {
            if let Some(rows) = sub.rows() { return Some(rows) }
            if start.elapsed() >= FIRST_LIST { break }
            tokio::time::sleep(POLL).await;
        }
        self.st.list.snapshot().await.ok().map(|produced| produced.rows)
    }

    /// O endereço do motor da sessão, quando ela roda num.
    fn engine_base(&self, engine: &str) -> Option<String> {
        let (engines, _) = crate::engines::store::state(&self.st.accounts.env);
        engines.get(engine).and_then(|e| e["base_url"].as_str()).map(str::to_owned)
    }
}

/// Orquestração: executor e revisor não levam `orq_arbiter`, o grupo só aparece na linha `orq` da execução.
fn in_orq(row: &SessionRow, rows: &[SessionRow]) -> bool {
    row.orq_arbiter.is_some() || row.pair_gid.as_deref()
        .is_some_and(|gid| rows.iter().any(|r| r.provider == "orq" && r.pair_gid.as_deref() == Some(gid)))
}

/// Mensagem de alguém na fila; a nota da conversa e o próprio "continue" não contam.
fn from_someone(entry: &Value) -> bool {
    entry["papel"] != "assistant" && !text::is_auto_continue(entry["text"].as_str().unwrap_or_default())
}

fn facts_of(row: &SessionRow, rows: &[SessionRow], queue: &[Value], stopped_at: Option<f64>, handoff: bool) -> Facts {
    Facts {
        life: row.lifecycle_id.clone().unwrap_or_default(),
        // O interruptor por sessão mora no spec e entra com a rota dele.
        off: false,
        state: row.state.clone(),
        queue_pending: queue.iter().any(|entry| entry["delivered"] == false && from_someone(entry)),
        question_open: row.pending_questions > 0 || row.question.is_some() || row.ask_form.is_some(),
        orq: in_orq(row, rows),
        user_after_stop: stopped_at.is_some_and(|at| queue.iter().any(|entry| from_someone(entry) && entry["ts"].as_f64().is_some_and(|ts| ts > at))),
        handoff: handoff || row.handoff.is_some(),
    }
}

/// P4: motor em loopback (o CLIProxy sempre responde) sonda o provedor; motor remoto, o endereço dele.
fn target_for(provider: &str, engine_base: Option<&str>) -> Option<String> {
    let public = if provider.starts_with("codex") { "chatgpt.com:443" } else if provider.starts_with("claude") { "api.anthropic.com:443" } else { return None };
    let Some(base) = engine_base.map(str::trim).filter(|base| !base.is_empty()) else { return Some(public.to_owned()) };
    match reqwest::Url::parse(base).ok().and_then(|url| url.host_str().map(str::to_owned)) {
        Some(host) if !loopback(&host) => Some(base.to_owned()),
        _ => Some(public.to_owned()),
    }
}

fn loopback(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost") || host.trim_matches(['[', ']']).parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

impl Host for Native {
    fn facts<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Option<Facts>> {
        Box::pin(async move {
            let rows = self.rows().await?;
            let row = rows.iter().find(|row| row.name == name)?;
            let id = format!("autocont-load:{}", crate::mods::state::random_hex(8));
            // Fila que não se lê (sessão adormecida, sem ator) não prova mensagem nenhuma esperando.
            let queue = match self.runtime.queue_action(name, id, crate::runtime::queue::Action::Load).await {
                Ok(Value::Array(queue)) => queue,
                _ => Vec::new(),
            };
            let stopped_at = lock(&self.stops).get(name).map(|(at, _)| *at);
            Some(facts_of(row, &rows, &queue, stopped_at, self.runtime.handoff_held(name)))
        })
    }

    fn probe_target(&self, name: &str) -> Option<String> {
        let row = self.st.list.last_rows().and_then(|rows| rows.iter().find(|row| row.name == name).cloned());
        let provider = row.as_ref().map(|row| row.provider.clone()).or_else(|| lock(&self.stops).get(name).map(|(_, provider)| provider.clone()))?;
        let base = row.and_then(|row| row.engine).and_then(|engine| self.engine_base(&engine));
        target_for(&provider, base.as_deref())
    }

    fn send_continue<'a>(&'a self, name: &'a str, text: &'a str) -> BoxFuture<'a, Result<bool, String>> {
        Box::pin(async move {
            let (status, reply) = crate::session_write::input::write_input(&self.st, name, text, false).await;
            if status.is_success() { return Ok(reply["delivered"] == true); }
            Err(reply["detail"]["code"].as_str().unwrap_or("auto_continue_send_refused").to_owned())
        })
    }

    fn note<'a>(&'a self, name: &'a str, id: &'a str, call: &'a str, text: String, meta: Value) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move { crate::notice_actions::note(&self.st, name, id, call, &text, meta).await })
    }

    fn diary(&self, name: &str, code: &str, reason: &'static str) { self.st.diag.report(EVENT, name, code, reason); }

    fn now(&self) -> f64 { crate::state::monitor::wall_now() }

    fn hhmm(&self, at: f64) -> String { crate::runtime::local_policy::local_hhmm(at.floor() as i64).unwrap_or_default() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(fields: Value) -> SessionRow { serde_json::from_value(fields).unwrap() }

    #[test]
    fn the_probe_goes_to_the_provider_or_to_a_remote_engine() {
        assert_eq!(target_for("claude", None).as_deref(), Some("api.anthropic.com:443"));
        assert_eq!(target_for("codex", None).as_deref(), Some("chatgpt.com:443"));
        for local in ["http://127.0.0.1:8317", "http://localhost:8317/v1", "http://[::1]:8317"] {
            assert_eq!(target_for("claude", Some(local)).as_deref(), Some("api.anthropic.com:443"), "{local}");
        }
        assert_eq!(target_for("claude", Some("https://api.moonshot.ai/anthropic")).as_deref(), Some("https://api.moonshot.ai/anthropic"));
        assert_eq!(target_for("pi", None), None);
    }

    #[test]
    fn the_facts_read_the_row_and_the_queue() {
        let me = row(json!({"name": "s", "lifecycle_id": "k:abc", "state": "idle", "pair_gid": "g1"}));
        let rows = vec![me.clone(), row(json!({"name": "g1-orq", "provider": "orq", "pair_gid": "g1"}))];
        let queue = vec![
            json!({"id": "autocont:l1", "papel": "assistant", "text": "A conexão caiu às 10:00; continuo quando a rede voltar.", "ts": 200.0, "delivered": true}),
            json!({"id": "q1", "text": text::TRANSIENT, "ts": 210.0, "delivered": false}),
        ];
        let facts = facts_of(&me, &rows, &queue, Some(100.0), false);
        assert_eq!(facts.life, "k:abc");
        assert!(facts.orq, "membro de grupo orq");
        assert!(!facts.queue_pending && !facts.user_after_stop, "a nota e o próprio continue não são da pessoa");
        let mut wrote = queue.clone();
        wrote.push(json!({"id": "q2", "text": "segue", "ts": 220.0, "delivered": false}));
        let facts = facts_of(&me, &rows[..1], &wrote, Some(100.0), false);
        assert!(facts.queue_pending && facts.user_after_stop && !facts.orq);
        assert!(!facts_of(&me, &rows[..1], &wrote, Some(300.0), false).user_after_stop, "mensagem de antes da parada");
        let asking = row(json!({"name": "s", "pending_questions": 1}));
        assert!(facts_of(&asking, &[], &[], None, false).question_open);
        assert!(facts_of(&asking, &[], &[], None, true).handoff);
    }
}
