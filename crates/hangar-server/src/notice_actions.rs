//! Ações dos avisos da CLI do Claude: a prévia e a abertura da sessão de conserto de um hook, e o vigia que traz
//! o resultado dela para a conversa de origem como nota. A nota vai pela fila, nunca pelo `/input`: não acorda o
//! turno. A sessão de conserto nasce solta (sem grupo); o registro de consertos diz de quem ela é.
use std::{collections::{HashMap, HashSet}, io, net::SocketAddr, path::{Path, PathBuf}, sync::{Arc, LazyLock}, time::Duration};

use axum::{body::to_bytes, extract::{ConnectInfo, Path as RoutePath, Request, State}, http::StatusCode, response::Response};
use hangar_api::{chat::ChatKind, session::SessionRow};
use hangar_notice_actions::fixes::{self, Fix, Fixes};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Mutex, Notify};

use crate::{routes::AppState, runtime::queue::Action, session_open::{SessionOpenRequest, server::{Decision, open_owned, reply}},
    session_write::{detail_body, json_response}};

const TICK: Duration = Duration::from_secs(2);
/// Parada há tanto tempo seguido: entre dois passos do turno a linha passa por `idle` por um instante.
const SETTLE_S: f64 = 20.0;
/// Conserto que não respondeu nisso vira falha: a linha não fica "consertando" para sempre.
const MAX_AGE_MS: u64 = 2 * 3600 * 1000;
const MAX_DETAIL: usize = 4000;
const MAX_SUBJECT: usize = 200;
const MAX_READ: u64 = 8 * 1024 * 1024;
const EVENT: &str = "rust.notice_fix_failed";
const PANIC_PAUSE: Duration = Duration::from_secs(2);
const PANIC_PAUSE_MAX: Duration = Duration::from_secs(60);

static WAKE: LazyLock<Notify> = LazyLock::new(Notify::new);
/// Ler, mudar e gravar o registro é uma coisa só: dois cliques não abrem duas sessões.
static REGISTRY: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

/// O aviso vem do app do dono: quem pode abrir sessão com qualquer pedido. A saída do hook vai cercada no pedido.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FixBody {
    notice_id: String,
    subject: String,
    detail: String,
    /// Hook que bloqueia: a sessão de conserto nasce sem hooks, ou o mesmo hook a prenderia.
    #[serde(default)]
    blocking: bool,
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    json_response(status, detail_body(code, message, json!({})))
}

fn not_owner() -> Response { failure(StatusCode::FORBIDDEN, "notice_fix_owner_only", "só o dono conserta hooks") }

fn valid(body: &FixBody) -> bool {
    let subject = !body.subject.is_empty() && body.subject.len() <= MAX_SUBJECT
        && body.subject.bytes().all(|b| b.is_ascii_alphanumeric() || b":_-.|*".contains(&b));
    subject && fixes::valid_notice_id(&body.notice_id) && body.detail.chars().count() <= MAX_DETAIL
}

async fn owner_body(st: &AppState, peer: SocketAddr, request: Request) -> Result<FixBody, Response> {
    crate::update_arrived::owner_gate(st, peer, &request, not_owner).await?;
    to_bytes(request.into_body(), 16 * 1024).await.ok()
        .and_then(|raw| serde_json::from_slice::<FixBody>(&raw).ok()).filter(valid)
        .ok_or_else(|| failure(StatusCode::UNPROCESSABLE_ENTITY, "erro_aviso_invalido", "aviso inválido"))
}

fn list_unavailable() -> Response { failure(StatusCode::SERVICE_UNAVAILABLE, "erro_lista_indisponivel", "a lista de sessões não respondeu") }

fn claude_only(row: Option<SessionRow>) -> Result<SessionRow, Response> {
    row.filter(|row| row.provider == "claude")
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "erro_sessao_nao_encontrada", "sessão Claude não encontrada"))
}

/// Os arquivos que definem o hook ou, sem nenhum, todos os que existem.
async fn hook_files(row: &SessionRow, subject: &str) -> Vec<PathBuf> {
    let main = home().join(".claude");
    let config = crate::list::context::config_dir_of(row.conta.as_deref()).unwrap_or_else(|| main.clone());
    let cwd = PathBuf::from(row.cwd.clone().unwrap_or_default());
    let subject = subject.to_owned();
    tokio::task::spawn_blocking(move || {
        let read: Vec<(PathBuf, String)> = hangar_notice_actions::candidate_files(&config, &main, &cwd).into_iter()
            .filter_map(|path| std::fs::read_to_string(&path).ok().map(|text| (path, text))).collect();
        let read = hangar_notice_actions::without_mirrored_copy(read, &config, &main);
        let files = hangar_notice_actions::hook_sources(&subject, &read);
        if files.is_empty() { read.into_iter().map(|(path, _)| path).collect() } else { files }
    }).await.unwrap_or_default()
}

fn home() -> PathBuf { std::env::home_dir().unwrap_or_default() }

/// `POST /api/sessions/{name}/notice/fix/preview`: o que a sessão de conserto vai receber.
pub async fn preview(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, RoutePath(name): RoutePath<String>,
    request: Request) -> Response {
    let body = match owner_body(&st, peer, request).await { Ok(body) => body, Err(response) => return response };
    let row = match st.list.find_row(&name).await.map_err(|_| list_unavailable()).and_then(claude_only) {
        Ok(row) => row, Err(response) => return response,
    };
    let files = hook_files(&row, &body.subject).await;
    json_response(StatusCode::OK, json!({"subject": body.subject, "detail": body.detail, "no_hooks": body.blocking,
        "files": files.iter().map(|f| f.display().to_string()).collect::<Vec<_>>()}))
}

/// `POST /api/sessions/{name}/notice/fix`: abre a sessão de conserto e grava a nota "consertando" na origem.
pub async fn start(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, RoutePath(name): RoutePath<String>,
    request: Request) -> Response {
    let body = match owner_body(&st, peer, request).await { Ok(body) => body, Err(response) => return response };
    let rows = match st.list.snapshot().await { Ok(produced) => produced.rows, Err(_) => return list_unavailable() };
    let row = match claude_only(rows.iter().find(|row| row.name == name).cloned()) { Ok(row) => row, Err(response) => return response };
    let files = hook_files(&row, &body.subject).await;
    let prompt = hangar_notice_actions::fix_prompt(&body.subject, &body.detail, &files);
    let attempt = now_ms();
    // Gravado antes de abrir: a volta nunca acha a sessão sem saber de quem ela é. O nome fica reservado no registro.
    let taken: HashSet<String> = rows.iter().map(|r| r.name.clone()).collect();
    let (parent, notice_id, subject) = (name.clone(), body.notice_id.clone(), body.subject.clone());
    let reserved = registry(&st, move |all| {
        if fixes::running(all, &parent, &notice_id) { return Err(("erro_conserto_em_andamento", "este aviso já tem um conserto em andamento")); }
        let child = fixes::child_name(&subject, |candidate| taken.contains(candidate) || all.contains_key(candidate))
            .ok_or(("erro_nome_em_uso", "sem nome livre para a sessão de conserto"))?;
        all.insert(child.clone(), Fix { parent, notice_id, attempt, subject });
        Ok(child)
    }).await;
    let child = match reserved {
        Ok(Ok(child)) => child,
        Ok(Err((code, message))) => return failure(StatusCode::CONFLICT, code, message),
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE, "erro_consertos_gravacao", "o registro de consertos não pôde ser gravado"),
    };
    // O vigia acorda já: se a abertura terminar sem quem pediu, a volta não depende do fim deste pedido.
    WAKE.notify_one();
    let open = SessionOpenRequest {
        name: Some(child.clone()), cwd: row.cwd.clone().unwrap_or_default(),
        config_dir: crate::list::context::config_dir_of(row.conta.as_deref()).map(|dir| dir.display().to_string()),
        provider: Some("claude".into()), headless: Some(true), model: Some("sonnet".into()), initial_prompt: Some(prompt),
        creator: Some(name.clone()), disable_hooks: body.blocking, ..SessionOpenRequest::default()
    };
    let fix = Fix { parent: name.clone(), notice_id: body.notice_id, attempt, subject: body.subject };
    // Tarefa própria: o celular que vai para o fundo cancela o pedido, e a sessão aberta ficaria sem a nota
    // de andamento ou com o registro de uma abertura recusada.
    let task = tokio::spawn(async move {
        match open_owned(&st, open).await {
            Decision::Opened(_) => {
                let text = format!("Consertando em {child}");
                if let Err(code) = note(&st, &name, &format!("fix:{}:{attempt}", fix.notice_id), &text, fix_meta(&fix, &child, "running")).await {
                    // A sessão já abriu: a volta ainda grava o resultado; só a linha de andamento falta.
                    report(&st, &name, &code, "nota de andamento do conserto não gravada");
                }
                json_response(StatusCode::OK, json!({"child": child, "attempt": attempt}))
            }
            decision => {
                let removed = child.clone();
                if registry(&st, move |all| { all.remove(&removed); }).await.is_err() {
                    report(&st, &name, "notice_fixes_write_failed", "abertura recusada ficou no registro de consertos");
                }
                match decision {
                    Decision::Refused(error) => reply(&error),
                    _ => failure(StatusCode::MISDIRECTED_REQUEST, "erro_criacao_sessao", "a sessão de conserto não abriu"),
                }
            }
        }
    });
    task.await.unwrap_or_else(|_| failure(StatusCode::INTERNAL_SERVER_ERROR, "erro_criacao_sessao", "a abertura do conserto não terminou"))
}

/// Lê o registro, aplica `change` e grava, sob a trava e fora das threads do servidor. Ilegível é erro.
async fn registry<T: Send + 'static>(st: &AppState, change: impl FnOnce(&mut Fixes) -> T + Send + 'static) -> io::Result<T> {
    let _registry = REGISTRY.lock().await;
    let dir = st.hangar_dir.clone();
    tokio::task::spawn_blocking(move || {
        let mut all = fixes::load(&dir)?;
        let before = all.clone();
        let out = change(&mut all);
        if all != before { fixes::save(&dir, &all)?; }
        Ok(out)
    }).await.map_err(io::Error::other)?
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn fix_meta(fix: &Fix, child: &str, state: &str) -> Value {
    json!({"fix_of": fix.notice_id, "attempt": fix.attempt, "state": state, "child": child, "subject": fix.subject})
}

/// Nota na conversa de origem pela fila dela: a viva pelo ator, a com terminal ainda sem ator adotada antes,
/// a adormecida direto no arquivo (sem acordar).
async fn note(st: &AppState, parent: &str, entry_id: &str, text: &str, meta: Value) -> Result<(), String> {
    let runtime = st.state.runtime.get().ok_or("runtime_watcher")?;
    let action = || Action::AppendLocal { text: text.to_owned(), entry_id: Some(entry_id.to_owned()), confirms: None, hook_error: None,
        notice: None, meta: Some(meta.clone()) };
    let call = || format!("notice-fix:{entry_id}");
    match runtime.queue_action(parent, call(), action()).await {
        Ok(_) => return Ok(()),
        Err(error) if error.code != "runtime_binding" => return Err(error.code),
        Err(_) => {}
    }
    if let Some(mark) = runtime.asleep(parent).await {
        return runtime.asleep_queue_action(parent, &mark.key, &call(), action()).await.map(|_| ()).map_err(|e| e.code);
    }
    runtime.adopt_claude_terminal(parent).await.map_err(|e| e.code)?;
    runtime.queue_action(parent, call(), action()).await.map(|_| ()).map_err(|e| e.code)
}

fn report(st: &AppState, session: &str, code: &str, reason: &'static str) {
    if !crate::warn_limit::allow(Some(session), code) { return; }
    tracing::warn!(code, session, reason, "notices: conserto de hook");
    st.diag.report(EVENT, session, code, reason);
}

/// O vigia; uma pane recomeça o laço (com espera crescente), nunca o para de vez e calado.
pub fn spawn(st: Arc<AppState>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut pause = PANIC_PAUSE;
        loop {
            let mut inner = crate::AbortOnDrop(tokio::spawn(watch(st.clone())));
            match (&mut inner.0).await {
                Err(e) if e.is_panic() => {
                    report(&st, "", "notice_fixes_panicked", "vigia dos consertos caiu; recomeça");
                    tokio::time::sleep(pause).await;
                    pause = (pause * 2).min(PANIC_PAUSE_MAX);
                }
                _ => return,
            }
        }
    })
}

async fn watch(st: Arc<AppState>) {
    let mut settled: HashMap<String, (f64, f64)> = HashMap::new();
    loop {
        match registry(&st, |all| all.is_empty()).await {
            Ok(true) => { settled.clear(); WAKE.notified().await; continue }
            Ok(false) => {}
            Err(_) => { report(&st, "", "notice_fixes_unreadable", "notice-fixes.json ilegível; consertos parados");
                let _ = tokio::time::timeout(Duration::from_secs(60), WAKE.notified()).await; continue }
        }
        let sub = st.hub.subscribe_internal(&st.list, &st.devices, &st.diag);
        loop {
            tokio::time::sleep(TICK).await;
            let Some(rows) = sub.rows() else { settled.clear(); continue };
            if !round(&st, &rows, &mut settled).await { break; }
        }
    }
}

/// O que fazer com um conserto nesta rodada.
#[derive(Debug, PartialEq)]
enum Step { Wait, Answered, Failed(&'static str) }

/// `since`: desde quando a linha está parada com a mesma resposta (segundos do relógio).
fn step(fix: &Fix, row: Option<&SessionRow>, since: Option<f64>, now_s: f64, now_ms: u64) -> Step {
    if now_ms.saturating_sub(fix.attempt) > MAX_AGE_MS { return Step::Failed("a sessão de conserto não respondeu em 2 h"); }
    let Some(row) = row else { return Step::Wait };
    let answered = row.state == "idle" && row.last_reply_at.is_some_and(|at| at * 1000.0 >= fix.attempt as f64);
    if answered && since.is_some_and(|since| now_s - since >= SETTLE_S) { Step::Answered } else { Step::Wait }
}

/// Uma rodada: lê o registro, grava as notas sem a trava (a origem pode demorar) e tira os entregues. `false`
/// quando não sobrou conserto.
async fn round(st: &AppState, rows: &[SessionRow], settled: &mut HashMap<String, (f64, f64)>) -> bool {
    let Ok(all) = registry(st, |all| all.clone()).await else { return true };
    let (now_s, now) = (crate::state::monitor::wall_now(), now_ms());
    let mut finished = Vec::new();
    for (child, fix) in all {
        let row = rows.iter().find(|r| r.name == child);
        let reply_at = row.and_then(|r| r.last_reply_at).unwrap_or_default();
        let since = match settled.get(&child) {
            Some((at, from)) if *at == reply_at && row.is_some_and(|r| r.state == "idle") => Some(*from),
            _ => { settled.insert(child.clone(), (reply_at, now_s)); None }
        };
        let (state, text) = match step(&fix, row, since, now_s, now) {
            Step::Wait => continue,
            Step::Failed(why) => ("failed", format!("{child}: {why}")),
            Step::Answered => match row.and_then(|r| r.jsonl.clone()).map(PathBuf::from) {
                Some(jsonl) => match tokio::task::spawn_blocking(move || final_reply(&jsonl)).await.ok().flatten() {
                    Some(reply) => ("done", format!("{child}: {}", hangar_notice_actions::result_line(&reply))),
                    None => ("failed", format!("{child}: o turno terminou sem resposta em texto")),
                },
                None => ("failed", format!("{child}: conversa da sessão de conserto não encontrada")),
            },
        };
        match note(st, &fix.parent, &format!("fixdone:{}:{}", fix.notice_id, fix.attempt), &text, fix_meta(&fix, &child, state)).await {
            Ok(()) => finished.push(child),
            // A origem fechou ou morreu (a lista guarda a linha `dead`): o resultado não tem onde ficar.
            Err(_) if origin_gone(rows, &fix, now) => {
                report(st, &child, "notice_fix_parent_gone", "conversa de origem do conserto fechada; resultado descartado");
                finished.push(child);
            }
            Err(code) => report(st, &child, &code, "resultado do conserto não gravado; tenta de novo"),
        }
    }
    for child in &finished { settled.remove(child); }
    match registry(st, move |all| { for child in &finished { all.remove(child); } all.is_empty() }).await {
        Ok(empty) => !empty,
        Err(_) => { report(st, "", "notice_fixes_write_failed", "registro de consertos não gravado"); true }
    }
}

/// A nota não tem onde entrar: origem fora da lista ou morta, ou o dobro do prazo sem conseguir gravar.
fn origin_gone(rows: &[SessionRow], fix: &Fix, now_ms: u64) -> bool {
    let origin = rows.iter().find(|r| r.name == fix.parent);
    origin.is_none_or(|r| r.state == "dead") || now_ms.saturating_sub(fix.attempt) > 2 * MAX_AGE_MS
}

/// A última resposta em texto da sessão de conserto (não aviso).
fn final_reply(jsonl: &Path) -> Option<String> {
    let request = crate::transcript::HistoryRequest { provider: crate::transcript::Provider::Claude, jsonl: jsonl.to_owned(), queue: None,
        limit: Some(60), tail_window: crate::transcript::TAIL_WINDOW };
    let events = crate::transcript::history::merged_history_capped(&request, MAX_READ).ok()?;
    events.into_iter().rev().find(|event| event.kind == ChatKind::AssistantMsg && event.notice_meta.is_none())
        .and_then(|event| event.text).filter(|text| !text.trim().is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix() -> Fix { Fix { parent: "origem".into(), notice_id: "hookfail:a".into(), attempt: 1_000_000, subject: "Stop".into() } }
    fn row(state: &str, reply_at: Option<f64>) -> SessionRow {
        serde_json::from_value(json!({"name": "conserto-stop", "state": state, "last_reply_at": reply_at})).unwrap()
    }

    #[test]
    fn step_waits_for_a_settled_answer_after_the_attempt() {
        let (f, now_ms) = (fix(), 1_000_000 + 60_000);
        assert_eq!(step(&f, None, None, 100.0, now_ms), Step::Wait, "sessão ainda fora da lista");
        assert_eq!(step(&f, Some(&row("working", Some(1100.0))), Some(0.0), 100.0, now_ms), Step::Wait);
        assert_eq!(step(&f, Some(&row("idle", Some(999.0))), Some(0.0), 100.0, now_ms), Step::Wait, "resposta de antes do conserto");
        assert_eq!(step(&f, Some(&row("idle", Some(1100.0))), Some(90.0), 100.0, now_ms), Step::Wait, "parada há menos de 20 s");
        assert_eq!(step(&f, Some(&row("idle", Some(1100.0))), Some(70.0), 100.0, now_ms), Step::Answered);
        assert!(matches!(step(&f, None, None, 100.0, 1_000_000 + MAX_AGE_MS + 1), Step::Failed(_)), "não fica consertando para sempre");
    }

    #[test]
    fn a_dead_or_gone_origin_releases_the_fix() {
        let origin = |state: &str| -> SessionRow { serde_json::from_value(json!({"name": "origem", "state": state})).unwrap() };
        let f = fix();
        assert!(!origin_gone(&[origin("idle")], &f, f.attempt + 1000));
        assert!(origin_gone(&[origin("dead")], &f, f.attempt + 1000));
        assert!(origin_gone(&[], &f, f.attempt + 1000));
        assert!(origin_gone(&[origin("idle")], &f, f.attempt + 2 * MAX_AGE_MS + 1), "não tenta para sempre");
    }

    #[test]
    fn fix_body_is_validated() {
        let parse = |value: Value| serde_json::from_value::<FixBody>(value).ok().filter(valid);
        assert!(parse(json!({"notice_id": "hookfail:a", "subject": "PostToolUse:Edit|Write", "detail": "x"})).is_some());
        assert!(parse(json!({"notice_id": "a b", "subject": "Stop", "detail": "x"})).is_none());
        assert!(parse(json!({"notice_id": "a", "subject": "Stop; rm", "detail": "x"})).is_none());
        assert!(parse(json!({"notice_id": "a", "subject": "Stop", "detail": "x".repeat(MAX_DETAIL + 1)})).is_none());
        assert!(parse(json!({"notice_id": "a", "subject": "Stop", "detail": "x", "command": "y"})).is_none(), "campo a mais é recusado");
    }
}
