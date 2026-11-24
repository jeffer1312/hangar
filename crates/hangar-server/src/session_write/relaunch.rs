//! `POST /modo-execucao` e `/recarregar`: a troca de modo e o reinício rodam no Rust do começo ao fim, o
//! Codex pela `runtime::codex_relaunch` e o Claude pela reabertura do spec (`runtime::claude_reopen`), com o
//! corpo e os códigos da rota Python (`api.py`). Sessão que não é de nenhum dos dois (Pi,
//! omp, Kimi, Antigravity) recebe aqui a recusa que a rota Python dava. A recuperação da transferência é do Rust.
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};

use super::input::json_content_type;
use super::{BODY_LIMIT, busy_body, invalid_body, json_response, session_name, too_large};
use crate::routes::{AppState, cors, gate, who};
use crate::share::caller::Caller;
use crate::runtime::protocol::RuntimeError;
use crate::runtime::claude_reopen::{ReopenFailure,Reopened};
use crate::session_open::{ChangeReason,SessionChange,Surface};

/// Pânico no meio da troca: não se sabe em que fase parou, então nunca é "não troquei".
const PANICKED: &str = "relaunch_panicked";

fn uncertain(error: &RuntimeError) -> bool {
    error.code == PANICKED || crate::runtime::codex_relaunch::kill_phase(error)
}

/// `ModoExecucaoBody`: só `terminal`, booleano estrito.
fn terminal_field(headers: &axum::http::HeaderMap, bytes: &[u8]) -> Option<bool> {
    if !json_content_type(headers) { return None; }
    match serde_json::from_slice::<Value>(bytes).ok()? {
        Value::Object(fields) if fields.len() == 1 => fields.get("terminal")?.as_bool(),
        _ => None,
    }
}

/// O app mostra só `params.erro`: sem o código junto, as falhas de terminal são todas a mesma frase genérica.
fn with_code(error: &RuntimeError) -> String { format!("{} ({})", error.message, error.code) }

/// O `HTTPException` do `_trocar_modo`: recusa e falha de troca são 409 `erro_troca_modo`, como no Python,
/// inclusive terminal que não fechou ou processo que não saiu (o código de origem vai em `params.code`); só
/// a troca interrompida por pânico, sem fase conhecida, é 500 com o código.
pub fn mode_refusal(error: &RuntimeError) -> (StatusCode, Value) {
    if error.code == "session_transfer_busy" { return (StatusCode::CONFLICT, busy_body()); }
    if error.code == PANICKED {
        return (StatusCode::INTERNAL_SERVER_ERROR, super::detail_body(&error.code, &error.message, json!({})));
    }
    // A recusa que já é a frase da troca (`erro_troca_modo`) dispensa o código; as outras o levam à vista.
    let shown = if error.code == "erro_troca_modo" { error.message.clone() } else { with_code(error) };
    let mut params = json!({"erro": shown});
    if crate::runtime::codex_relaunch::kill_phase(error) || error.code == crate::runtime::codex_relaunch::STARTING || error.code == "erro_sessao_adormecida" {
        params["code"] = json!(error.code);
    }
    (StatusCode::CONFLICT, super::detail_body("erro_troca_modo", &format!("não troquei de modo: {shown}"), params))
}

/// O `_reload_session` do Codex: com terminal é 409; fase incerta é 500 com o código; o reinício que
/// falha é 502, com o código de origem em `params.code`.
pub fn reload_refusal(error: &RuntimeError) -> (StatusCode, Value) {
    match error.code.as_str() {
        "session_transfer_busy" => (StatusCode::CONFLICT, busy_body()),
        "erro_recarregar_so_sem_terminal" => (StatusCode::CONFLICT, super::detail_body(&error.code, &error.message, json!({}))),
        _ if uncertain(error) => (StatusCode::INTERNAL_SERVER_ERROR, super::detail_body(&error.code, &error.message, json!({}))),
        _ => {
            let reason: String = error.message.chars().take(200).collect();
            (StatusCode::BAD_GATEWAY, super::detail_body("erro_codex_nao_reiniciou", &format!("o Codex não subiu de novo: {reason}"), json!({"code": error.code})))
        }
    }
}

pub async fn mode(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (_, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let (true, Some(name)) = (owner, session_name(req.uri().path())) else { return super::not_owner(&st, peer, req).await };
    let Some(runtime) = st.state.runtime.get().cloned() else { return super::no_runtime(req.headers()) };
    let (parts, body) = req.into_parts();
    let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else { return too_large(&parts.headers) };
    let Some(terminal) = terminal_field(&parts.headers, &bytes) else { return invalid_body(&parts.headers) };
    let panels = Some(st.term.clone());
    let (st2, owned) = (st.clone(), name.clone());
    let outcome = detached_relaunch(async move {
        if let Some(agy) = during_life_change(&st2, &owned, antigravity_switch(&st2, &runtime, &owned, terminal)).await { return Some(Relaunched::Codex(agy)); }
        if let Some(codex) = during_life_change(&st2, &owned, runtime.relaunch_codex(&owned, terminal, panels)).await { return Some(Relaunched::Codex(codex)); }
        let surface = if terminal { Surface::Terminal } else { Surface::Headless };
        let change = SessionChange { reason: ChangeReason::ModeSwitch, surface: Some(surface), account: None, permission_mode: None,
            model: None, effort: None, service_tier: None };
        during_life_change(&st2, &owned, runtime.reopen_claude_session(&owned, change, None)).await.map(|r| Relaunched::Claude(r, !terminal))
    }).await;
    let (status, body) = match outcome {
        None => unowned_mode(&st, &name).await,
        Some(Relaunched::Codex(Ok(value))) => (StatusCode::OK, value),
        Some(Relaunched::Codex(Err(error))) => mode_refusal(&error),
        Some(Relaunched::Claude(Ok(reopened), _)) => (StatusCode::OK, reopened.answer(json!({"ok": true, "terminal": terminal}))),
        Some(Relaunched::Claude(Err(failure), _)) => claude_refusal(CLAUDE_MODE, &failure),
    };
    // A identidade da sessão mudou (ou pode ter mudado no meio de uma falha).
    st.list.invalidate();
    let mut response = json_response(status, body);
    cors(&parts.headers, response.headers_mut());
    response
}

/// Dono e convidado completo recarregam pelo mesmo caminho (decisão 2); o que nenhum agente do Rust
/// atende é respondido aqui, nunca pelo Python.
pub async fn reload(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (_, who) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let Some(name) = session_name(req.uri().path()).filter(|n| who.as_ref().is_some_and(|c| c.can(n, req.method()))) else {
        return super::stranger(&st, peer, req, who.as_ref()).await;
    };
    let Some(runtime) = st.state.runtime.get().cloned() else { return super::no_runtime(req.headers()) };
    let code = if matches!(who, Some(Caller::Guest(_))) { "session_reload_requested_by_guest" } else { "session_reload_requested_by_owner" };
    // Só vai ao diário a recarga que um agente do Rust aceitou; recusa de nome sem dono não é recarga.
    let asked = super::bridge::asked_line("sessao.recarga_pedida", code, &name, req.headers(), peer);
    if let Some(home)=st.list.env().dirs.as_ref().map(|dirs|&dirs.home) {
        let pending=crate::runtime::claude_reopen::transfer::store(home).incomplete_for_session(&name);
        match pending {
            Ok(records) if !records.is_empty()=>{
                let current_key=match runtime.transfer_key(&name).await {
                    Ok(key)=>key,
                    Err(error)=>{let mut response=json_response(StatusCode::CONFLICT,json!({"detail":error.public()}));cors(req.headers(),response.headers_mut());return response},
                };
                if let Some(record)=records.into_iter().find(|r|r.name==name && current_key.as_ref().is_none_or(|key|r.origin_meta["key"]==*key)) {
                st.diag.record_line(asked);
                let (st2,owned)=(st.clone(),name.clone());
                let result=tokio::spawn(async move {during_life_change(&st2,&owned,runtime.recover_transfer(record)).await}).await
                    .unwrap_or_else(|_|Err(hangar_conversation_transfer::TransferError::new("session_transfer_restore_failed")));
                st.list.invalidate();
                let mut response=match result {Ok(body)=>json_response(StatusCode::OK,body),Err(error)=>json_response(StatusCode::from_u16(error.status).unwrap_or(StatusCode::CONFLICT),json!({"detail":error.public()}))};
                cors(req.headers(),response.headers_mut());return response;
                }
            },
            Ok(_)=>{},
            Err(error)=>{let mut response=json_response(StatusCode::CONFLICT,json!({"detail":error.public()}));cors(req.headers(),response.headers_mut());return response},
        }
    }
    let (st2, owned) = (st.clone(), name.clone());
    let outcome = detached_relaunch(async move {
        if let Some(codex) = runtime.restart_codex(&owned).await { return Some(Relaunched::Codex(codex)); }
        let from_terminal = runtime.claude_headless(&owned).await.is_none_or(|headless| !headless);
        let change = SessionChange { reason: ChangeReason::Reload, surface: None, account: None, permission_mode: None,
            model: None, effort: None, service_tier: None };
        during_life_change(&st2, &owned, runtime.reopen_claude_session(&owned, change, None)).await.map(|r| Relaunched::Claude(r, from_terminal))
    }).await;
    if outcome.is_some() { st.diag.record_line(asked); }
    let (status, body) = match outcome {
        None => unowned_reload(&st, &name).await,
        Some(Relaunched::Codex(Ok(value))) => (StatusCode::OK, value),
        Some(Relaunched::Codex(Err(error))) => reload_refusal(&error),
        Some(Relaunched::Claude(Ok(reopened), _)) => (StatusCode::OK, reopened.answer(json!({"ok": true}))),
        Some(Relaunched::Claude(Err(failure), from_terminal)) if failure.ended_without_terminal(from_terminal) =>
            reload_ended_without_terminal(&failure),
        Some(Relaunched::Claude(Err(failure), _)) => claude_refusal(CLAUDE_RELOAD, &failure),
    };
    st.list.invalidate();
    let mut response = json_response(status, body);
    cors(req.headers(), response.headers_mut());
    response
}

/// Nome que nem o Codex nem o Claude do Rust atendem: sem sessão, outro agente no pane, ou a lista sem
/// responder, que nunca vira "não existe".
async fn unowned_reload(st: &AppState, name: &str) -> (StatusCode, Value) {
    use super::unowned::Presence;
    match super::unowned::presence(st, name).await {
        Err(error) => (StatusCode::SERVICE_UNAVAILABLE, json!({"detail": crate::list::hub::unavailable_detail(&error)})),
        Ok(Presence::Missing) => (StatusCode::NOT_FOUND, super::detail_body("erro_sessao_inexistente", "sessão não encontrada", json!({}))),
        Ok(Presence::Pane) => (StatusCode::CONFLICT, super::detail_body("erro_recarregar_so_sem_terminal",
            "recarregar só vale para sessão Claude sem terminal", json!({}))),
        Ok(Presence::Sidecar(code)) => (StatusCode::SERVICE_UNAVAILABLE, super::detail_body(code,
            "o arquivo da sessão não abre no servidor; feche e abra a sessão", json!({}))),
    }
}

/// Troca de modo de nome que nem o Codex nem o Claude do Rust atendem: as recusas do `modo_execucao` do Python.
async fn unowned_mode(st: &AppState, name: &str) -> (StatusCode, Value) {
    use super::unowned::Presence;
    match super::unowned::presence(st, name).await {
        Err(error) => (StatusCode::SERVICE_UNAVAILABLE, json!({"detail": crate::list::hub::unavailable_detail(&error)})),
        Ok(Presence::Missing) => (StatusCode::NOT_FOUND, super::detail_body("erro_sessao_inexistente", "sessão não encontrada", json!({}))),
        Ok(Presence::Pane) => (StatusCode::CONFLICT, super::detail_body("erro_modo_so_claude",
            "a troca de modo só vale para sessões Claude", json!({}))),
        Ok(Presence::Sidecar(code)) => (StatusCode::SERVICE_UNAVAILABLE, super::detail_body(code,
            "o arquivo da sessão não abre no servidor; feche e abra a sessão", json!({}))),
    }
}

enum Relaunched { Codex(Result<Value, RuntimeError>), Claude(Result<Reopened, ReopenFailure>, bool) }

/// A pílula do Antigravity, sob a porta de entrada exclusiva do nome. Para o sem terminal o registro faz tudo;
/// para o terminal, o pane abre pelo caminho do restore (`agy --conversation`, grupo e fila do nome ficam), e
/// só depois de nascer o sidecar sem terminal sai; se não nascer, a sessão volta sem terminal. `None`: não é
/// sessão do Antigravity.
async fn antigravity_switch(st: &Arc<AppState>, runtime: &Arc<crate::runtime::gateway::RuntimeRegistry>, name: &str, terminal: bool)
    -> Option<Result<Value, RuntimeError>> {
    let headless_open = runtime.antigravity_headless(name).await.is_some();
    let terminal_open = runtime.other_info(name).await.is_some_and(|info| info.provider == "antigravity");
    if !headless_open && !terminal_open {
        let home = st.accounts.env.home.clone();
        let sidecar = crate::runtime::codex_sidecar::path(&crate::runtime::antigravity_sidecar::dir(&home), name);
        let exists = match tokio::task::spawn_blocking(move || sidecar.try_exists()).await {
            Ok(Ok(exists)) => exists,
            _ => return Some(Err(RuntimeError::new("antigravity_sidecar_unreadable", "o arquivo da sessão do Antigravity sem terminal não foi lido"))),
        };
        if !exists {
            // Pane sem ator ainda (abrindo, ou na tela de primeiro uso do agy): a recusa diz isso, não "só Claude".
            let listed = st.list.discovered_session(name).await.ok().flatten().is_some_and(|(row, _)| row.provider == "antigravity");
            if listed {
                return Some(Err(RuntimeError::new("erro_troca_modo",
                    "o Antigravity com terminal ainda não respondeu (abrindo ou na tela de primeiro uso); tente de novo quando ele estiver pronto")));
            }
            return None;
        }
        // Sem terminal estacionada: já está onde pediram.
        if !terminal { return Some(Ok(json!({"ok": true, "terminal": false}))); }
    }
    if terminal_open && terminal || headless_open && !terminal { return Some(Ok(json!({"ok": true, "terminal": terminal}))); }
    let _closed = match crate::runtime::lifecycle::ClosedIngress::acquire_exclusive(runtime.ingress(), name).await {
        Ok(closed) => closed,
        Err(error) => return Some(Err(error)),
    };
    if !terminal { return runtime.antigravity_to_headless(name).await; }
    let left = match runtime.antigravity_leave_headless(name).await? {
        Ok(left) => left,
        Err(error) => return Some(Err(error)),
    };
    let opened = async {
        let home = st.accounts.env.home.clone();
        let (h, key) = (home.clone(), left.key.clone());
        let spec = tokio::task::spawn_blocking(move || crate::session_open::record::read(&h, &key)).await
            .map_err(|_| RuntimeError::new("spec_job", "a leitura do registro não terminou"))?
            .map_err(|error| RuntimeError::new("erro_troca_modo", &format!("o registro da sessão não foi lido: {:?}", error.kind())))?
            .ok_or_else(|| RuntimeError::new("erro_troca_modo", "A sessão não tem registro de abertura; a sessão foi mantida."))?;
        let mut next = spec.with(&SessionChange { reason: ChangeReason::ModeSwitch, surface: Some(Surface::Terminal), account: None,
            permission_mode: Some(left.permission_mode), model: left.model.clone(), effort: None, service_tier: None });
        next.conversation = Some(left.conversation.clone());
        let (h, written) = (home.clone(), next.clone());
        tokio::task::spawn_blocking(move || crate::session_open::record::write(&h, &written)).await
            .map_err(|_| RuntimeError::new("spec_job", "a gravação do registro não terminou"))?
            .map_err(|error| RuntimeError::new("erro_troca_modo", &format!("o registro da sessão não foi gravado: {:?}", error.kind())))?;
        if let Err(error) = open_agy_terminal(st, &home, &left.key, name, &left.conversation).await {
            let (h, previous) = (home.clone(), spec.clone());
            let undone = tokio::task::spawn_blocking(move || crate::session_open::record::write(&h, &previous)).await;
            if !matches!(undone, Ok(Ok(()))) {
                tracing::warn!(session = %name, "o registro da sessão não voltou ao sem terminal depois da troca falha");
                return Err(RuntimeError::new(&error.code, &format!("{}; o registro da sessão não voltou ao sem terminal", error.message)));
            }
            return Err(error);
        }
        Ok(())
    }.await;
    match opened {
        Ok(()) => {
            runtime.finish_leave_headless(name, &left.key).await;
            Some(Ok(json!({"ok": true, "terminal": true})))
        }
        Err(error) => {
            // O terminal não nasceu: a sessão volta sem terminal, na mesma conversa.
            if let Some(Err(reopen)) = runtime.open_antigravity_headless(name).await {
                tracing::warn!(session = %name, code = %reopen.code, "o Antigravity sem terminal não voltou depois da troca falha");
            }
            Some(Err(error))
        }
    }
}

/// Abre o pane do Antigravity pelo registro gravado, na conversa dada (`agy --conversation`).
async fn open_agy_terminal(st: &Arc<AppState>, home: &std::path::Path, key: &str, name: &str, conversation: &str) -> Result<(), RuntimeError> {
    let host = crate::session_open::server::ServerHost::new(st.clone()).await;
    let entry = crate::session_open::restore::Entry { name: name.to_owned(), provider: crate::session_open::AgentProvider::Antigravity,
        conversation: Some(conversation.to_owned()), permission_mode: None, missing_since: None, unit: None };
    let projects = st.accounts.env.claude_base.join("projects");
    match crate::session_open::restore::resolved_for(home, &projects, key, &entry) {
        Ok(resolved) => crate::session_open::open::open_session(&host, resolved, &crate::session_open::OpenCaller::Owner).await
            .map(|_| ()).map_err(|error| RuntimeError::new(error.code.unwrap_or("erro_troca_modo"), &error.msg)),
        Err(code) => Err(RuntimeError::new(&code, "o terminal do Antigravity não abriu pelo registro")),
    }
}

async fn write_record(home: &std::path::Path, spec: crate::session_open::SessionSpec) -> Result<(), RuntimeError> {
    let home = home.to_owned();
    tokio::task::spawn_blocking(move || crate::session_open::record::write(&home, &spec)).await
        .map_err(|_| RuntimeError::new("spec_job", "a gravação do registro não terminou"))?
        .map_err(|error| RuntimeError::new("erro_permissao_reabrir", &format!("o registro da sessão não foi gravado: {:?}", error.kind())))
}

/// O bypass do `agy` é flag do processo e fica fora do shift+tab: entrar ou sair dele fecha o pane e o abre de novo
/// pelo registro, na mesma conversa. Falha depois de fechar volta o registro e o terminal de antes.
pub(crate) async fn antigravity_terminal_reopen(st: &Arc<AppState>, runtime: &Arc<crate::runtime::gateway::RuntimeRegistry>, name: &str,
    mode: crate::session_open::PermissionMode) -> Result<Value, RuntimeError> {
    let _closed = crate::runtime::lifecycle::ClosedIngress::acquire_exclusive(runtime.ingress(), name).await?;
    let left = runtime.antigravity_terminal_left(name, false).await
        .ok_or_else(|| RuntimeError::new("erro_permissao_reabrir", "o terminal do Antigravity não está aberto aqui"))??;
    let home = st.accounts.env.home.clone();
    let key = left.target.key.clone();
    let reopened = |mode: crate::session_open::PermissionMode| {
        let mut spec = left.spec.with(&SessionChange { reason: ChangeReason::Bypass, surface: None, account: None,
            permission_mode: Some(mode), model: None, effort: None, service_tier: None });
        spec.conversation = Some(left.conversation.clone());
        spec
    };
    write_record(&home, reopened(mode)).await?;
    if let Err(error) = runtime.stop_other_for_switch(name).await {
        if let Err(undo) = write_record(&home, left.spec.clone()).await {
            return Err(RuntimeError::new(&error.code, &format!("{}; o registro da sessão não voltou: {}", error.message, undo.message)));
        }
        return Err(error);
    }
    let Err(error) = open_agy_terminal(st, &home, &key, name, &left.conversation).await else {
        return Ok(json!({"ok": true, "mode": mode.name(), "current": mode.name(), "requested": mode.name(), "changed": true,
            "previous_non_plan": null, "reopened": true}));
    };
    let back = match write_record(&home, reopened(left.mode)).await {
        Ok(()) => open_agy_terminal(st, &home, &key, name, &left.conversation).await,
        Err(undo) => Err(undo),
    };
    let message = match back {
        Ok(()) => format!("não reabri em {}: {}; o terminal voltou em {}", mode.name(), error.message, left.mode.name()),
        Err(back) => format!("não reabri em {}: {}; o terminal de antes também não voltou: {}", mode.name(), error.message, back.message),
    };
    Err(RuntimeError::new(&error.code, &message))
}

/// Numa tarefa própria, como a do Codex: a troca admitida termina mesmo que o cliente desista.
async fn detached_relaunch(work: impl Future<Output = Option<Relaunched>> + Send + 'static) -> Option<Relaunched> {
    tokio::spawn(work).await.unwrap_or_else(|error| {
        tracing::error!(code = PANICKED, error = %error, "troca de modo ou recarga interrompida no meio");
        Some(Relaunched::Codex(Err(RuntimeError::new(PANICKED, "A troca parou no meio; confira o estado da sessão antes de tentar de novo."))))
    })
}

/// A reabertura troca a vida da sessão (pane ⇄ sidecar, nascimento novo): durante ela a varredura dos
/// convites não a revoga, e no fim (também na falha, que pode ter mudado a vida) convite e dono-convidado
/// passam à vida nova. Falha ao gravar o dono vai ao log e não troca o resultado.
/// A reabertura do Claude numa tarefa própria (a troca admitida termina mesmo que o cliente desista), dentro
/// da troca de vida; pânico no meio é falha depois de encerrar, nunca "nada aconteceu". A lista é refeita.
pub(crate) async fn reopen_detached(st: &Arc<AppState>, name: &str,
    work: impl Future<Output = Option<Result<Reopened, ReopenFailure>>> + Send + 'static) -> Option<Result<Reopened, ReopenFailure>> {
    let (st2, owned) = (st.clone(), name.to_owned());
    let outcome = tokio::spawn(async move { during_life_change(&st2, &owned, work).await }).await.unwrap_or_else(|_| Some(Err(ReopenFailure {
        error: RuntimeError::new(PANICKED, "A troca parou no meio; confira o estado da sessão antes de tentar de novo."), stopped: true, landed: None })));
    st.list.invalidate();
    outcome
}

pub(crate) async fn during_life_change<T>(st:&AppState,name:&str,work:impl Future<Output = T>) -> T {
    let Some(share)=st.share.clone() else { return work.await };
    share.lives.facts().begin_mode_change(name);
    let result=work.await;
    let (life,uncertain)=match st.state.runtime.get() {
        Some(runtime)=>runtime.pending_transfer_life(name).await,
        None=>(None,false),
    };
    if uncertain {
        share.lives.facts().set_transfer_uncertain(name,true);
        share.lives.facts().end_mode_change(name);
        return result;
    }
    share.lives.facts().set_life(name,life);
    if let Some(life)=share.lives.life(name,true).await {
        if let Err(error)=share.store.set_life(name,Some(life.clone())) { tracing::warn!(session=%name,kind=?error.kind(),"convite não acompanhou a vida nova"); }
        let (base,owned)=(st.accounts.env.claude_base.clone(),name.to_owned());
        let saved=tokio::task::spawn_blocking(move||crate::session_open::server::guest_set_life(&base,&owned,&life)).await;
        if !matches!(saved,Ok(Ok(()))) { tracing::warn!(session=%name,"dono-convidado não acompanhou a vida nova"); }
    }
    share.lives.facts().set_transfer_uncertain(name,false);
    share.lives.facts().end_mode_change(name);
    result
}

/// O terminal não reabriu e a sessão seguiu sem ele: o aviso leva o código de origem, como as outras recusas.
fn reload_ended_without_terminal(failure: &ReopenFailure) -> (StatusCode, Value) {
    let error = &failure.error;
    let shown = with_code(error);
    (StatusCode::BAD_GATEWAY, super::detail_body("erro_recarregar_sem_terminal", &shown, json!({"erro": shown, "code": error.code})))
}

/// Código do aviso e a frase dele, por ação recusada.
pub type ClaudeAction = (&'static str, &'static str);
pub const CLAUDE_MODE: ClaudeAction = ("erro_troca_modo", "não troquei de modo");
pub const CLAUDE_RELOAD: ClaudeAction = ("erro_recarregar", "não recarreguei");
pub const CLAUDE_RESUME: ClaudeAction = ("erro_retomar", "não retomei a conversa");

/// O `HTTPException` do `_trocar_modo`/`_reload_terminal` do Claude: ocupada é o motivo com a frase do app;
/// o resto é 409 com o código da ação e o código de origem em `params.code`.
pub fn claude_refusal(action: ClaudeAction, failure: &ReopenFailure) -> (StatusCode, Value) {
    let verb = action.1;
    let error = &failure.error;
    if let Some(busy) = super::busy_refusal(error) { return busy; }
    let shown = with_code(error);
    (StatusCode::CONFLICT, super::detail_body(action.0, &format!("{verb}: {shown}"),
        json!({"erro": shown, "code": error.code})))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_refusals_keep_the_python_codes() {
        let (status, body) = mode_refusal(&RuntimeError::new("erro_troca_modo", "Esta política de permissões não é suportada sem terminal; a sessão foi mantida."));
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["detail"]["code"], "erro_troca_modo");
        assert!(body["detail"]["msg"].as_str().unwrap().starts_with("não troquei de modo: Esta política"));
        assert_eq!(mode_refusal(&RuntimeError::new("session_transfer_busy", "x")).1["detail"]["code"], "session_transfer_busy");
        // Terminal que não fechou ou processo que não saiu: 409 `erro_troca_modo` do Python, com o código de origem.
        for code in ["relaunch_process_alive", "relaunch_kill_survived", "relaunch_kill_mux"] {
            let (status, body) = mode_refusal(&RuntimeError::new(code, "O terminal ainda está encerrando; não abri outro processo."));
            assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["params"]["code"].as_str()), (StatusCode::CONFLICT, Some("erro_troca_modo"), Some(code)));
        }
    }

    /// O aviso do app mostra só `params.erro`: a frase genérica de um erro de terminal leva o código junto.
    #[test]
    fn a_claude_refusal_shows_the_real_code_in_the_message() {
        let failure = ReopenFailure { error: RuntimeError::new("lifecycle_native_read", "operação terminal conservada no diário"), stopped: false, landed: None };
        let (status, body) = claude_refusal(CLAUDE_MODE, &failure);
        assert_eq!((status, body["detail"]["code"].as_str(), body["detail"]["params"]["code"].as_str()), (StatusCode::CONFLICT, Some("erro_troca_modo"), Some("lifecycle_native_read")));
        assert!(body["detail"]["params"]["erro"].as_str().unwrap().contains("lifecycle_native_read"), "{body}");
        assert!(body["detail"]["msg"].as_str().unwrap().contains("lifecycle_native_read"), "{body}");
        // O Codex também: só a recusa que já é a frase da troca dispensa o código.
        let body = mode_refusal(&RuntimeError::new("codex_terminal_starting_x", "frase genérica")).1;
        assert!(body["detail"]["params"]["erro"].as_str().unwrap().contains("codex_terminal_starting_x"), "{body}");
        assert!(body["detail"]["msg"].as_str().unwrap().contains("codex_terminal_starting_x"), "{body}");
        assert!(!mode_refusal(&RuntimeError::new("erro_troca_modo", "frase")).1["detail"]["msg"].as_str().unwrap().contains("(erro_troca_modo)"));
    }

    /// Recarregar e retomar dizem o que falhou com a frase deles, que o app traduz pelo código.
    #[test]
    fn reload_and_resume_refusals_name_their_own_action() {
        let failure = ReopenFailure { error: RuntimeError::new("relaunch_confirmation", "o Claude não voltou"), stopped: false, landed: None };
        let pt = include_str!("../../../../messages/pt.json");
        let en = include_str!("../../../../messages/en.json");
        for (action, code, verb) in [(CLAUDE_RELOAD, "erro_recarregar", "não recarreguei: "), (CLAUDE_RESUME, "erro_retomar", "não retomei a conversa: ")] {
            let body = claude_refusal(action, &failure).1;
            assert_eq!(body["detail"]["code"], code);
            assert!(body["detail"]["msg"].as_str().unwrap().starts_with(verb), "{body}");
            assert_eq!(body["detail"]["params"]["code"], "relaunch_confirmation");
            for messages in [pt, en] { assert!(messages.contains(&format!("\"{code}\": ")), "{code} sem frase nos idiomas"); }
        }
    }

    #[test]
    fn a_reload_that_ended_without_terminal_shows_the_real_code() {
        let failure = ReopenFailure { error: RuntimeError::new("relaunch_confirmation", "o Claude não voltou"), stopped: true, landed: None };
        let (status, body) = reload_ended_without_terminal(&failure);
        assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::BAD_GATEWAY, Some("erro_recarregar_sem_terminal")));
        assert!(body["detail"]["params"]["erro"].as_str().unwrap().contains("relaunch_confirmation"), "{body}");
        assert!(body["detail"]["msg"].as_str().unwrap().contains("relaunch_confirmation"), "{body}");
    }

    #[test]
    fn reload_refusals_keep_the_python_codes() {
        assert_eq!(reload_refusal(&RuntimeError::new("erro_recarregar_so_sem_terminal", "x")).0, StatusCode::CONFLICT);
        let (status, body) = reload_refusal(&RuntimeError::new("cano_nao_subiu", "binário não encontrado: codex"));
        assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::BAD_GATEWAY, Some("erro_codex_nao_reiniciou")));
        assert_eq!(body["detail"]["params"]["code"], "cano_nao_subiu", "o código de origem não se perde");
        let (status, body) = reload_refusal(&RuntimeError::new("relaunch_process_alive", "ainda vivo"));
        assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some("relaunch_process_alive")));
    }

    #[tokio::test]
    async fn a_panic_mid_switch_is_an_uncertain_phase_not_a_refusal() {
        let crash = true;
        let Some(Relaunched::Codex(Err(outcome))) = detached_relaunch(async move { if crash { panic!("no meio") } None }).await
            else { panic!("o pânico vira erro com código") };
        assert_eq!(outcome.code, PANICKED);
        for (status, body) in [mode_refusal(&outcome), reload_refusal(&outcome)] {
            assert_eq!((status, body["detail"]["code"].as_str()), (StatusCode::INTERNAL_SERVER_ERROR, Some(PANICKED)));
        }
    }

    #[test]
    fn only_the_strict_body_is_served_here() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(axum::http::header::CONTENT_TYPE, "application/json".parse().unwrap());
        assert_eq!(terminal_field(&headers, br#"{"terminal":true}"#), Some(true));
        for bad in [&br#"{"terminal":1}"#[..], br#"{"terminal":true,"x":1}"#, b"[]", b"{}"] {
            assert_eq!(terminal_field(&headers, bad), None);
        }
        // Como o FastAPI: sem `Content-Type` o corpo é JSON; outro tipo não é.
        assert_eq!(terminal_field(&axum::http::HeaderMap::new(), br#"{"terminal":true}"#), Some(true));
        headers.insert(axum::http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        assert_eq!(terminal_field(&headers, br#"{"terminal":true}"#), None);
    }
}
