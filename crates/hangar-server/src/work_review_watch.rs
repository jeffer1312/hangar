//! Revisão do trabalho (`work_review/v1`) ligada ao servidor: a identidade da sessão (a mesma da tool
//! `mission`), o diário e a releitura no fim de cada turno pelas linhas da lista. A lista só é assinada
//! enquanto houver conector ligado com a capacidade: sem ele, o produtor para quando não há app aberto.
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

use hangar_api::session::SessionRow;
use hangar_work_review::{Ended, Engine, SessionRef, Sessions, Task, TurnRow, TurnTracker};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::diag::DiagClient;
use crate::mission_notify::SessionNotify;
use crate::routes::AppState;

const TICK: Duration = Duration::from_secs(2);
/// Conector ligado ou desligado só é relido nesse intervalo: o catálogo mora em disco.
const RECHECK: Duration = Duration::from_secs(60);
const EVENT: &str = "rust.work_review";

pub(crate) fn engine(host: Arc<hangar_connectors::Host>, mission: Arc<hangar_mission::Store>, sessions: Arc<SessionNotify>, diag: DiagClient) -> Arc<Engine> {
    Engine::new(host, mission, sessions, Arc::new(DiagSink(diag)), cache_dir())
}

/// A mesma base do índice de custos (`<cache>/hangar`): é cache, pode sumir.
fn cache_dir() -> PathBuf {
    let costs = crate::costs::index::default_dir();
    costs.parent().map_or(costs.clone(), PathBuf::from)
}

struct DiagSink(DiagClient);

impl hangar_work_review::Diag for DiagSink {
    fn report(&self, code: &str, session: &str, detail: &str) {
        let code = if detail.is_empty() { code.to_owned() } else { format!("{code}:{detail}") };
        self.0.report(EVENT, session, &code, "releitura da revisão do trabalho falhou; vale o resumo anterior");
    }
}

impl Sessions for SessionNotify {
    fn resolve(&self, name: String) -> Task<Result<Option<SessionRef>, String>> {
        let state = self.state.get().and_then(Weak::upgrade);
        let identity = hangar_mission::http::Notify::identity(self, name.clone());
        Box::pin(async move { let read: Result<_, &'static str> = async {
            let state = state.ok_or("server_stopping")?;
            let rows = state.list.snapshot().await.map_err(|_| "list_unavailable")?;
            let Some(row) = rows.rows.iter().find(|row| row.name == name) else { return Ok(None) };
            let (lifecycle_id, branch) = (row.lifecycle_id.clone(), row.branch.clone());
            let group = async {
                match &state.groups {
                    Some(groups) => groups.link(&name).await.map(|group| group.map(|group| group.gid)),
                    None => Ok(None),
                }
            };
            let (key, group_id) = tokio::join!(identity, group);
            // Sem a chave da vida o vínculo gravado não confere (a tool `mission` também recusa), mas a
            // busca pela branch não precisa dela.
            let key = key.unwrap_or_else(|_| {
                if hangar_core::warn_limit::allow(Some(&name), "work_review_session_key_unavailable") {
                    state.diag.report(EVENT, &name, "work_review_session_key_unavailable", "chave da sessão ilegível; só a branch acha o ticket");
                }
                None
            });
            let group_id = group_id.map_err(|_| "groups_unavailable")?;
            Ok(Some(SessionRef { name, lifecycle_id, key, group_id, branch }))
        }.await; read.map_err(str::to_owned) })
    }
}

/// Releitura de uma sessão sem esperar (tool `mission`, que tem corte de ~60 s).
pub(crate) fn refresh_session(st: &Arc<AppState>, name: &str) {
    let (st, name) = (st.clone(), name.to_owned());
    tokio::spawn(async move {
        if let Some(session) = resolved(&st, name).await { st.work_review.refresh(session); }
    });
}

pub(crate) fn spawn(st: Arc<AppState>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            // Abortar o laço de fora tem de abortar o `run`: soltar o JoinHandle só desanexa.
            let mut inner = crate::AbortOnDrop(tokio::spawn(run(st.clone())));
            match (&mut inner.0).await {
                Err(error) if error.is_panic() => {
                    st.diag.report(EVENT, "", "work_review_watch_panicked", "vigia do fim de turno caiu; recomeça");
                    tokio::time::sleep(RECHECK).await;
                }
                _ => return,
            }
        }
    })
}

async fn run(st: Arc<AppState>) {
    let engine = st.work_review.clone();
    let mut failing = false;
    loop {
        if !offered(&st, &mut failing).await {
            tokio::time::sleep(RECHECK).await;
            continue;
        }
        let sub = st.hub.subscribe_internal(&st.list, &st.devices, &st.diag);
        let mut tracker = TurnTracker::new();
        let mut checked = Instant::now();
        let mut live: Option<Vec<(String, Option<String>)>> = None;
        loop {
            tokio::time::sleep(TICK).await;
            let rows = sub.rows().map(|rows| turn_rows(&rows));
            if let Some(rows) = &rows { prune(&engine, rows, &mut live); }
            let ended = tracker.ended(rows.as_deref());
            if !ended.is_empty() { tokio::spawn(reread(st.clone(), engine.clone(), ended)); }
            if checked.elapsed() >= RECHECK {
                checked = Instant::now();
                // A releitura que termina depois da poda recria a vaga de quem já saiu: poda de novo no próximo tique.
                live = None;
                if !offered(&st, &mut failing).await { break; }
            }
        }
    }
}

/// Catálogo que não se lê conta como "sem conector" e é relido no próximo `RECHECK`; vai ao diário só
/// quando começa a falhar, senão repetiria a cada minuto.
async fn offered(st: &AppState, failing: &mut bool) -> bool {
    let engine = st.work_review.clone();
    let code = match tokio::task::spawn_blocking(move || engine.has_review_connector()).await {
        Ok(Ok(offered)) => {
            *failing = false;
            return offered;
        }
        Ok(Err(error)) => error.code(),
        Err(error) if error.is_panic() => "panicked",
        Err(_) => "cancelled",
    };
    if !std::mem::replace(failing, true) {
        st.diag.report(EVENT, "", &format!("work_review_catalog_unreadable:{code}"), "catálogo dos conectores ilegível; vigia do fim de turno parado até reler");
    }
    false
}

/// Sem lista do dono aberta, só daqui o engine esquece as sessões que saíram; poda quando as linhas mudam.
fn prune(engine: &Engine, rows: &[TurnRow], live: &mut Option<Vec<(String, Option<String>)>>) {
    let mut now: Vec<(String, Option<String>)> = rows.iter().map(|row| (row.name.clone(), row.lifecycle_id.clone())).collect();
    now.sort();
    if live.as_ref() == Some(&now) { return; }
    engine.prune(&now.iter().map(|(name, life)| (name.as_str(), life.as_deref())).collect::<Vec<_>>());
    *live = Some(now);
}

fn turn_rows(rows: &[SessionRow]) -> Vec<TurnRow> {
    rows.iter().map(|row| TurnRow { name: row.name.clone(), lifecycle_id: row.lifecycle_id.clone(), state: row.state.clone(),
        last_reply_at: row.last_reply_at, branch: row.branch.clone() }).collect()
}

async fn reread(st: Arc<AppState>, engine: Arc<Engine>, ended: Vec<Ended>) {
    let sessions = futures_util::future::join_all(ended.into_iter().map(|Ended { name, first_sight }| {
        let st = &st;
        async move { (resolved(st, name).await, first_sight) }
    })).await;
    for (session, first_sight) in sessions {
        let Some(session) = session else { continue };
        if first_sight { engine.refresh_if_unseen(session) } else { engine.refresh(session) }
    }
}

/// Leitura que falhou vai ao diário, uma vez por minuto por sessão e código: em segundo plano não há tela
/// para o erro, e a falha que persiste se repetiria a cada fim de turno.
async fn resolved(st: &AppState, name: String) -> Option<SessionRef> {
    match Sessions::resolve(&*st.mission_notify, name.clone()).await {
        Ok(session) => session,
        Err(code) => {
            let code = format!("work_review_session_unreadable:{code}");
            if hangar_core::warn_limit::allow(Some(&name), &code) {
                st.diag.report(EVENT, &name, &code, "sessão sem leitura; revisão não relida");
            }
            None
        }
    }
}
