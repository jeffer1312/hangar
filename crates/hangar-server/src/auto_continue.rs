//! Liga o `hangar-auto-continue` ao servidor: a agenda das paradas passageiras recebe o fim de turno do
//! runtime, lê os fatos da sessão pela lista interna e pela fila, manda o "continue" pelo `/input` e escreve
//! a nota pela fila. O registro é o mesmo da troca de conta no limite. O interruptor da sessão mora no spec
//! (`PUT`/`DELETE /api/sessions/{name}/auto-continue`, só do dono) e chega à linha da lista.
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use hangar_api::session::SessionRow;
use hangar_auto_continue::scheduler::BoxFuture;
use hangar_auto_continue::{Command, Facts, Host, Ledger, StopHit, ledger, text};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::list::hub::InternalSubscription;
use crate::list::mux::Mux;
use crate::routes::{AppState, cors};
use crate::runtime::gateway::RuntimeRegistry;
use crate::session_open::{SessionSpec, record};
use crate::session_write::{BODY_LIMIT, detail_body, json_response, session_name};

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
    host: Arc<Native>,
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

    /// O Cancelar da nota, respondido depois de a nota trocar. `false`: a sessão não esperava a rede.
    pub async fn cancel_waiting(&self, name: &str) -> bool {
        let (answer, answered) = tokio::sync::oneshot::channel();
        if self.commands.send(Command::CancelAnswered { name: name.to_owned(), answer }).is_err() { return false; }
        answered.await.unwrap_or(false)
    }

    pub fn forget(&self, name: &str) {
        lock(&self.stops).remove(name);
        let _ = self.commands.send(Command::Forget(name.to_owned()));
    }

    /// Os fatos da sessão recém-restaurada, para continuar o turno cortado pelo reboot. `since` é a hora do
    /// boot: mensagem da pessoa depois dele conta como escrita depois da parada.
    pub async fn restored_facts(&self, name: &str, since: f64) -> Result<Option<Facts>, String> {
        self.host.restored_facts(name, since).await
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
    let auto = Arc::new(AutoContinue { commands, stops, host: host.clone(), ledger: ledger.clone() });
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
    /// cai na produção avulsa. `Err` com o código quando nenhuma das duas se leu.
    async fn rows(&self) -> Result<Arc<Vec<SessionRow>>, String> {
        let sub = self.subscription();
        let start = tokio::time::Instant::now();
        loop {
            if let Some(rows) = sub.rows() { return Ok(rows) }
            if start.elapsed() >= FIRST_LIST { break }
            tokio::time::sleep(POLL).await;
        }
        self.st.list.snapshot().await.map(|produced| produced.rows).map_err(|error| error.code.to_owned())
    }

    /// Como `facts`, mas a fila que não se lê é erro: logo depois do restore não há agenda que reconfira, e
    /// sem a fila não se sabe se a pessoa deixou mensagem. O Claude com terminal restaurado ainda sem ator é
    /// adotado antes, senão a fila dele não responde.
    async fn restored_facts(&self, name: &str, since: f64) -> Result<Option<Facts>, String> {
        let rows = self.rows().await?;
        let Some(row) = rows.iter().find(|row| row.name == name) else { return Ok(None) };
        if row.provider == "claude" && !row.headless && let Err(error) = self.runtime.adopt_claude_terminal(name).await {
            return Err(error.code);
        }
        let id = format!("autocont-load:{}", crate::mods::state::random_hex(8));
        let queue = match self.runtime.queue_action(name, id, crate::runtime::queue::Action::Load).await {
            Ok(Value::Array(queue)) => queue,
            Ok(_) => return Err("auto_continue_queue_unreadable".to_owned()),
            Err(error) => return Err(error.code),
        };
        Ok(Some(facts_of(row, &rows, &queue, Some(since), self.runtime.handoff_held(name))))
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
        off: row.auto_continue_off,
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
    fn facts<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<Facts>, String>> {
        Box::pin(async move {
            let rows = self.rows().await?;
            let Some(row) = rows.iter().find(|row| row.name == name) else { return Ok(None) };
            let id = format!("autocont-load:{}", crate::mods::state::random_hex(8));
            // Fila que não se lê (sessão adormecida, sem ator) não prova mensagem nenhuma esperando.
            let queue = match self.runtime.queue_action(name, id, crate::runtime::queue::Action::Load).await {
                Ok(Value::Array(queue)) => queue,
                _ => Vec::new(),
            };
            let stopped_at = lock(&self.stops).get(name).map(|(at, _)| *at);
            Ok(Some(facts_of(row, &rows, &queue, stopped_at, self.runtime.handoff_held(name))))
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

/// As chaves de registro com o interruptor desligado, por casa: a lista não relê os registros a cada rodada.
/// Lidas uma vez, na primeira consulta da casa; a rota mantém o índice.
static OFF: LazyLock<Mutex<HashMap<PathBuf, HashSet<String>>>> = LazyLock::new(Mutex::default);
const OFF_MARK: &[u8] = b"auto_continue_off";

fn with_off<R>(home: &Path, change: impl FnOnce(&mut HashSet<String>) -> R) -> R {
    let mut all = OFF.lock().unwrap_or_else(PoisonError::into_inner);
    change(all.entry(home.to_owned()).or_insert_with(|| scan_off(&record::dir(home))))
}

fn scan_off(dir: &Path) -> HashSet<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return HashSet::new() };
    entries.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "json")).filter_map(|entry| {
        let bytes = crate::runtime::queue::read_replaced(&entry.path()).ok()?;
        if !bytes.windows(OFF_MARK.len()).any(|w| w == OFF_MARK) { return None; }
        let spec: SessionSpec = serde_json::from_slice(&bytes).ok()?;
        spec.auto_continue_off.then_some(spec.key)
    }).collect()
}

/// O interruptor de cada linha, pelo spec. A chave vem da vida `k:`; a sessão com terminal sem ela pergunta
/// a `CP_SESSION_KEY` ao pane só quando alguma sessão está desligada.
pub async fn fill_rows(home: &Path, mux: &Mux, rows: &mut [SessionRow]) {
    for row in rows.iter_mut() { row.auto_continue_off = false; }
    let at = home.to_owned();
    let off = tokio::task::spawn_blocking(move || with_off(&at, |off| off.clone())).await.unwrap_or_default();
    if off.is_empty() { return; }
    for row in rows.iter_mut() {
        let key = match crate::list::links::runtime_key(row.lifecycle_id.as_deref()) {
            Some(key) => Some(key.to_owned()),
            None if !row.headless => crate::session_visibility::fill::pane_key(mux, row).await,
            None => None,
        };
        row.auto_continue_off = key.is_some_and(|key| off.contains(&key));
    }
}

fn refuse(status: StatusCode, code: &str, msg: &str) -> Response { json_response(status, detail_body(code, msg, json!({}))) }

const OWNER_ONLY: (&str, &str) = ("erro_so_dono", "só o dono liga, desliga ou cancela o continuar sozinho");

/// Só o dono. Convidado (convite, par ou login) leva o 403 sem contar falha de senha; token errado, a recusa
/// contada (401, 429 bloqueado). O pedido é lido antes da espera: `&Request` atravessando um `await` tira o `Send`.
fn admit(st: &AppState, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<(), Response>> + Send + use<> {
    let guest = req.extensions().get::<crate::login_guests::gate::Login>().is_some()
        || matches!(crate::share::caller::caller(req), Some(crate::share::caller::Caller::Guest(_)));
    let owner = st.edge().owner_only(peer, req, OWNER_ONLY);
    async move {
        if guest { return Err(refuse(StatusCode::FORBIDDEN, OWNER_ONLY.0, OWNER_ONLY.1)); }
        owner.await
    }
}

/// `PUT` `{"enabled": bool}`: o interruptor da sessão. `DELETE`: o Cancelar da nota de espera.
pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let mut response = match admit(&st, peer, &req).await {
        Err(response) => response,
        Ok(()) => owner_route(&st, req).await,
    };
    cors(&headers, response.headers_mut());
    response
}

async fn owner_route(st: &AppState, req: Request) -> Response {
    let Some(name) = session_name(req.uri().path()) else { return StatusCode::NOT_FOUND.into_response() };
    if req.method() == Method::DELETE {
        let cancelled = match st.auto_continue.get() { Some(auto) => auto.cancel_waiting(&name).await, None => false };
        return json_response(StatusCode::OK, json!({"cancelled": cancelled}));
    }
    let enabled = axum::body::to_bytes(req.into_body(), BODY_LIMIT).await.ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|body| body["enabled"].as_bool());
    match enabled {
        Some(enabled) => switch(st, &name, enabled).await,
        None => refuse(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "o corpo precisa de {\"enabled\": true ou false}"),
    }
}

async fn switch(st: &AppState, name: &str, enabled: bool) -> Response {
    let Some(home) = st.list.env().dirs.as_ref().map(|dirs| dirs.home.clone()) else {
        return refuse(StatusCode::SERVICE_UNAVAILABLE, "list_dirs_missing", "as pastas das sessões não foram configuradas");
    };
    let rows = match st.list.discover(Some(crate::state::monitor::wall_now())).await {
        Ok(rows) => rows,
        Err(e) => return refuse(StatusCode::SERVICE_UNAVAILABLE, e.code, e.detail),
    };
    let Some(row) = rows.iter().find(|row| row.name == name) else {
        return refuse(StatusCode::NOT_FOUND, "session_not_found", "a sessão não existe");
    };
    let no_spec = || refuse(StatusCode::CONFLICT, "auto_continue_no_spec", "esta sessão não tem registro onde guardar o interruptor");
    let Some(key) = crate::dictation::rename_suggestion::key_of(st, name, row.lifecycle_id.as_deref()).await else { return no_spec() };
    match crate::groups::link_state::blocking(move || store_off(&home, &key, !enabled)).await {
        Ok(true) => {
            // Desligar durante a espera troca a nota pelo desfecho na hora, sem esperar o tique.
            if !enabled && let Some(auto) = st.auto_continue.get() { auto.cancel(name, "off"); }
            // A ausência de chave guardada do pane não pode atrasar a linha em 60 s.
            crate::session_visibility::fill::forget(name);
            st.list.invalidate();
            json_response(StatusCode::OK, json!({"enabled": enabled}))
        }
        Ok(false) => no_spec(),
        Err(error) => {
            tracing::warn!(session = %name, %error, "interruptor do continuar sozinho não gravado");
            st.diag.report(EVENT, name, "auto_continue_store_failed", "interruptor do continuar sozinho nao gravado");
            refuse(StatusCode::INTERNAL_SERVER_ERROR, "auto_continue_store_failed", "não foi possível guardar o interruptor")
        }
    }
}

/// Relê o registro na hora da gravação. `false`: a sessão não tem registro. Gravação e índice sob a mesma
/// trava: dois toques seguidos não deixam o índice diferente do disco.
fn store_off(home: &Path, key: &str, off: bool) -> io::Result<bool> {
    with_off(home, |keys| {
        if !record::write_auto_continue(home, key, off)? { return Ok(false) }
        if off { keys.insert(key.to_owned()); } else { keys.remove(key); }
        Ok(true)
    })
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
        assert!(!facts.off && facts_of(&row(json!({"name": "s", "auto_continue_off": true})), &[], &[], None, false).off, "o interruptor vem da linha");
    }

    #[tokio::test]
    async fn the_row_says_when_the_switch_is_off() {
        let home = tempfile::tempdir().unwrap();
        let (off, on) = ("a".repeat(32), "b".repeat(32));
        let spec = |key: &str, extra: Value| -> SessionSpec {
            let mut spec = json!({"version": 1, "key": key, "name": "s", "cwd": "/w", "provider": "claude", "surface": "headless",
                "account": {"kind": "default"}, "account_source": "default", "jev": false, "read_only": false, "created_at": 1.0});
            spec.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            serde_json::from_value(spec).unwrap()
        };
        record::write(home.path(), &spec(off.as_str(), json!({"auto_continue_off": true}))).unwrap();
        record::write(home.path(), &spec(on.as_str(), json!({}))).unwrap();
        let mux = Mux::with_program("hangar-mux-ausente", Duration::from_secs(1));
        let mut rows = vec![row(json!({"name": "s", "headless": true, "lifecycle_id": format!("k:{off}")})),
            row(json!({"name": "t", "headless": true, "lifecycle_id": format!("k:{on}")}))];
        fill_rows(home.path(), &mux, &mut rows).await;
        assert!(rows[0].auto_continue_off && !rows[1].auto_continue_off);
        assert!(store_off(home.path(), &off, false).unwrap(), "religar pela rota");
        fill_rows(home.path(), &mux, &mut rows).await;
        assert!(!rows[0].auto_continue_off, "o índice segue a rota sem reler os registros");
        assert!(!record::read(home.path(), &off).unwrap().unwrap().auto_continue_off);
        assert!(!store_off(home.path(), &"c".repeat(32), true).unwrap(), "sem registro não há onde guardar");
    }

    #[tokio::test]
    async fn an_invite_guest_gets_the_owner_only_403() {
        let st = Arc::new(AppState::new(crate::config::Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(),
            log_path: None,
            trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
        }));
        let peer: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut req = Request::builder().method("PUT").uri("/api/sessions/s/auto-continue")
            .header("authorization", "Bearer guest-token").header("content-type", "application/json")
            .body(axum::body::Body::from(r#"{"enabled": false}"#)).unwrap();
        req.extensions_mut().insert(crate::share::caller::Caller::Guest(Arc::new(crate::share::caller::GuestGrant {
            token: "guest-token".into(), token_hash: String::new(), sessions: HashMap::new(), pair_roots: vec![], invite: true })));
        let refused = route(State(st), ConnectInfo(peer), req).await;
        assert_eq!(refused.status(), StatusCode::FORBIDDEN);
        let raw = axum::body::to_bytes(refused.into_body(), usize::MAX).await.unwrap();
        assert_eq!(serde_json::from_slice::<Value>(&raw).unwrap()["detail"]["code"], "erro_so_dono");
    }
}
