//! Rotas de grupo com as respostas do Python (`api.py`: `pair_session`, `unpair_session`,
//! `group_message`, `pair_contract`; o par 1:1 entre máquinas mora em `legacy.rs`). Convidado e
//! corpo que o FastAPI recusaria seguem ao Python. Texto do protocolo, avisos e chamadas a outra
//! máquina saem depois de soltar o lock do grupo.
//!
//! `/pair` e `DELETE /pair` seguram a porta de entrada da sessão até gravar o grupo (um rename não
//! grava o nome velho por cima) e a soltam antes dos avisos: o aviso à própria sessão passa pela
//! porta de novo e, com o passe na mão, travaria contra um fechamento que espera esse passe.
use std::collections::{BTreeMap, HashMap};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Body, Bytes, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::deliver::{ProtocolArgs, protocol_text};
use super::local::{JoinRefusal, Snapshot, is_remote};
use super::orq::{is_orchestrator, list_unavailable};
use super::service::{GroupError, GroupService, JoinOutcome, JoinOwned, PromoteError};
use crate::proxy::Forward;
use crate::routes::{AppState, cors, gate, pass, pass_any};
use crate::runtime::ingress::IngressPass;
use crate::session_write::input::{deliver_text, json_content_type};
use crate::session_write::{BODY_LIMIT, busy_body, detail_body, json_response, session_name, too_large};
use crate::transcript::py::{py_repr, py_str};

/// `pair_texto.PREFIXO`.
pub(super) const PREFIX: &str = "[painel: grupo de trabalho]";
pub(super) const MIX_MSG: &str = "pareamento cross-server é 1:1 (uma sessão local + um peer remoto); uma sessão já pareada cross-server não entra em grupo local nem pareia com outro remoto";
const ORQ_MSG: &str = "o orquestrador não recebe mensagens; fale com o árbitro";
/// `_group_unavailable` do Python.
const UNAVAILABLE_MSG: &str = "os grupos estão indisponíveis agora";
const STORM_MAX: usize = 5;
const STORM_WINDOW: Duration = Duration::from_secs(60);

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/sessions/{name}/pair", post(pair).delete(unpair).fallback(pass_any))
        .route("/api/sessions/{name}/group-message", post(group_message).fallback(pass_any))
        .route("/api/sessions/{name}/pair/contract", get(contract).fallback(pass_any))
        .route("/api/sessions/{name}/pair-remote", post(super::legacy::pair_remote).fallback(pass_any))
        .route("/api/sessions/{name}/unpair-remote", post(super::legacy::unpair_remote).fallback(pass_any))
}

/// Pedido do dono que o Rust atende, com o corpo já lido.
pub(super) struct Asked { pub(super) st: Arc<AppState>, pub(super) groups: Arc<GroupService>, pub(super) name: String, parts: Parts, bytes: Bytes, fwd: Forward }

/// Pedido que chegou pela ponte privada: o Python já autorizou quem pediu. O que a rota repassaria
/// ao Python volta como erro, porque lá ele pediria à ponte de novo.
#[derive(Clone, Copy)]
pub(crate) struct Bridged;

fn relay_refused() -> Response {
    json_response(StatusCode::INTERNAL_SERVER_ERROR, detail_body("erro_grupo_indisponivel",
        UNAVAILABLE_MSG, json!({"detalhe": "groups_bridge_relay"})))
}

impl Asked {
    pub(super) async fn take(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Result<Asked, Response> {
        let bridged = req.extensions().get::<Bridged>().is_some();
        let (fwd, owner) = gate(&st, peer, &req);
        let (Some(groups), true, Some(name)) = (st.groups.clone(), owner || bridged, session_name(req.uri().path())) else {
            return Err(if bridged { relay_refused() } else { pass(&st, req, &fwd).await });
        };
        let (parts, body) = req.into_parts();
        let Ok(bytes) = to_bytes(body, BODY_LIMIT).await else { return Err(too_large(&parts.headers)) };
        Ok(Asked { st, groups, name, parts, bytes, fwd })
    }

    /// Corpo estrito como o `_StrictBody`; o que não casa fica com o FastAPI e o 422 dele.
    pub(super) fn body<T: DeserializeOwned>(&self) -> Option<T> {
        json_content_type(&self.parts.headers).then(|| serde_json::from_slice(&self.bytes).ok()).flatten()
    }

    pub(super) async fn to_python(self) -> Response {
        if self.parts.extensions.get::<Bridged>().is_some() {
            if crate::warn_limit::allow(Some(&self.name), "groups_bridge_relay") {
                tracing::warn!(code = "groups_bridge_relay", session = %self.name, "groups: corpo da ponte que a rota não aceita");
            }
            return relay_refused();
        }
        pass(&self.st, Request::from_parts(self.parts, Body::from(self.bytes)), &self.fwd).await
    }

    pub(super) fn reply(&self, status: StatusCode, body: Value) -> Response {
        let mut response = json_response(status, body);
        cors(&self.parts.headers, response.headers_mut());
        response
    }

    pub(super) fn refuse(&self, status: StatusCode, code: &str, msg: &str, params: Value) -> Response {
        self.reply(status, detail_body(code, msg, params))
    }

    /// `_transfer_guard`/`_transfer_check`: troca de agente em curso recusa com o código do Python.
    /// O passe devolvido segura a porta (rename, troca de conta esperam por ele); sem runtime, nenhum.
    pub(super) async fn enter(&self) -> Result<Option<IngressPass>, Response> {
        let Some(runtime) = self.st.state.runtime.get() else { return Ok(None) };
        match runtime.ingress().enter(&self.name, self.st.write_gate_wait).await {
            Ok(pass) => Ok(Some(pass)),
            Err(_) => Err(self.reply(StatusCode::CONFLICT, busy_body())),
        }
    }

    /// `_recusa_orq` para cada nome; a pergunta que falha nunca vira "não é orquestrador".
    pub(super) async fn orchestrator(&self, names: &[String]) -> Option<Response> {
        match is_orchestrator(&self.st, names).await {
            Ok(found) if found.is_empty() => None,
            Ok(_) => Some(self.refuse(StatusCode::CONFLICT, "erro_sessao_orq", ORQ_MSG, json!({}))),
            Err(failed) => Some(self.reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed}))),
        }
    }

    /// Erro de disco no grupo: o 500 do FastAPI para exceção não tratada, com a causa no diário.
    pub(super) fn store_failed(&self, error: &GroupError) -> Response {
        tracing::error!(code = "groups_store_failed", session = %self.name, %error, "groups: o grupo não foi lido ou gravado");
        self.st.diag.report("rust.groups_store_failed", &self.name, "groups_store_failed", "o grupo não foi lido ou gravado no disco");
        self.internal_error()
    }

    /// Volta o grupo ao de antes do join. Se o disco falha, o 500 do Python (o `pair.restore` dele
    /// levantava): responder "desfeito" seria mentira.
    pub(super) async fn restore(&self, before: Snapshot) -> Result<(), Response> {
        let Err(error) = self.groups.restore(before).await else { return Ok(()) };
        tracing::error!(code = "groups_restore_failed", session = %self.name, %error, "groups: o pareamento não foi desfeito por inteiro");
        self.st.diag.report("rust.groups_restore_failed", &self.name, "groups_restore_failed", "o pareamento não voltou ao estado anterior");
        Err(self.internal_error())
    }

    fn internal_error(&self) -> Response {
        let mut response = (StatusCode::INTERNAL_SERVER_ERROR, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "Internal Server Error").into_response();
        cors(&self.parts.headers, response.headers_mut());
        response
    }
}

/// Provedor de cada sessão viva (`registry.list()`): o retrato de até 2 s e, faltando alguém de
/// `wanted`, uma descoberta nova (como o `_cached_info_sync`).
pub(super) async fn providers(st: &AppState, wanted: &[String]) -> Result<BTreeMap<String, String>, Value> {
    let failed = |e: crate::list::bridge::ListError| list_unavailable(e.code);
    let rows = st.list.snapshot().await.map_err(failed)?.rows;
    let mut found: BTreeMap<String, String> = rows.iter().map(|r| (r.name.clone(), r.provider.clone())).collect();
    if wanted.iter().any(|n| !found.contains_key(n)) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
        found.extend(st.list.discover(Some(now)).await.map_err(failed)?.iter().map(|r| (r.name.clone(), r.provider.clone())));
    }
    Ok(found)
}

/// `_erro_texto`: o `msg` do envelope, ou o texto cru.
pub(super) fn error_text(e: &Value) -> String {
    match e {
        Value::Object(map) => map.get("msg").map_or_else(|| "None".to_owned(), py_str),
        other => py_str(other),
    }
}

fn failures_text(errs: &[Value]) -> String {
    errs.iter().map(|x| format!("{}: {}", py_str(&x["sessao"]), error_text(&x["erro"]))).collect::<Vec<_>>().join("; ")
}

pub(super) fn envelope(code: &str, msg: String, params: Value) -> Value { json!({"code": code, "params": params, "msg": msg}) }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairBody {
    #[serde(default)] peer: String,
    #[serde(default)] peers: Vec<String>,
    #[serde(default)] task: String,
    #[serde(default)] replace_task: bool,
    // Sem efeito, como no Python: o corpo estrito ainda o aceita.
    #[serde(default, rename = "notify_members")] _notify_members: bool,
    #[serde(default)] orq: bool,
}

pub(super) async fn pair(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let Some(body) = asked.body::<PairBody>() else { return asked.to_python().await };
    let raw = if !body.peers.is_empty() { body.peers } else if !body.peer.is_empty() { vec![body.peer] } else { Vec::new() };
    let mut others: Vec<String> = Vec::new();
    for p in raw.into_iter().filter(|p| !p.is_empty()) {
        if !others.contains(&p) { others.push(p); }
    }
    let held = match asked.enter().await { Ok(held) => held, Err(busy) => return busy };
    let name = asked.name.clone();
    if others.is_empty() && !body.orq {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_peer_nao_informado", "informe peer ou peers", json!({}));
    }
    if others.contains(&name) {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_autopareamento", "não dá pra parear uma sessão com ela mesma", json!({}));
    }
    let everyone: Vec<String> = std::iter::once(name.clone()).chain(others.iter().cloned()).collect();
    if let Some(refused) = asked.orchestrator(&everyone).await { return refused; }
    if others.iter().any(|o| is_remote(o)) {
        return super::legacy::pair_cross(asked, held, others, body.task, body.replace_task).await;
    }
    let harness = match providers(&asked.st, &everyone).await {
        Ok(h) => h,
        Err(failed) => return asked.reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})),
    };
    let missing = everyone.iter().filter(|n| !harness.contains_key(*n)).cloned().collect::<Vec<_>>().join(", ");
    if !missing.is_empty() {
        return asked.refuse(StatusCode::NOT_FOUND, "erro_sessao_nao_encontrada_detalhe", &format!("sessão não encontrada: {missing}"), json!({"detalhe": missing}));
    }
    let joined = asked.groups.join(JoinOwned { name, others, task: body.task, replace_task: body.replace_task, harness: harness.clone(), orq: body.orq }).await;
    let JoinOutcome { members, gid, task, orq, newcomers, before } = match joined {
        Ok(outcome) => outcome,
        Err(GroupError::Refused(JoinRefusal::Mix)) => return asked.refuse(StatusCode::BAD_REQUEST, "erro_pareamento_mistura_cross", MIX_MSG, json!({})),
        Err(GroupError::Refused(JoinRefusal::TaskConflict { existing })) => return asked.refuse(StatusCode::CONFLICT, "erro_pareamento_tarefa_existente",
            &format!("o grupo já tem tarefa: {} — repita com --substituir-tarefa pra trocar", py_repr(&json!(existing))), json!({"existente": existing})),
        Err(GroupError::Orq(PromoteError::Conflict(text))) => return asked.refuse(StatusCode::CONFLICT, "erro_orq_arquivo_mudou", &text, json!({})),
        Err(GroupError::Orq(PromoteError::Unavailable(code))) => return asked.refuse(StatusCode::SERVICE_UNAVAILABLE,
            "erro_grupo_indisponivel", UNAVAILABLE_MSG, json!({"detalhe": code})),
        Err(error) => return asked.store_failed(&error),
    };
    drop(held);
    // Só quem estava solto recebe o protocolo; grupo de orquestração não recebe nada.
    let notices = if orq { Vec::new() } else { newcomers };
    let contract = asked.groups.contract_path(&gid).to_string_lossy().into_owned();
    let inside: BTreeMap<String, String> = harness.into_iter().filter(|(n, _)| members.contains(n)).collect();
    let mut errs = Vec::new();
    for m in &notices {
        let args = ProtocolArgs { me: m.clone(), others: members.iter().filter(|x| *x != m).cloned().collect(), task: task.clone(),
            contract: Some(contract.clone()), harness: inside.clone(), ..Default::default() };
        let sent = match protocol_text(&asked.st, "group", &args).await {
            Ok(text) => deliver_text(&asked.st, m, &text).await,
            Err(failed) => Err(failed),
        };
        if let Err(e) = sent { errs.push(json!({"sessao": m, "erro": e})); }
    }
    if !notices.is_empty() && errs.len() == notices.len() {
        // Ninguém avisado: grupo fantasma. Volta ao que era antes do join.
        if let Err(failed) = asked.restore(before).await { return failed; }
        let msg = format!("pareamento desfeito: falha ao avisar as sessões ({})", failures_text(&errs));
        return asked.refuse(StatusCode::BAD_GATEWAY, "erro_pareamento_desfeito", &msg, json!({"avisos": errs}));
    }
    let warning = if errs.is_empty() { Value::Null } else {
        envelope("erro_pareamento_aviso_parcial", format!("aviso falhou em: {}", failures_text(&errs)), json!({"avisos": errs}))
    };
    asked.reply(StatusCode::OK, json!({"ok": true, "members": members, "gid": gid, "warning": warning}))
}

pub(super) async fn unpair(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let held = match asked.enter().await { Ok(held) => held, Err(busy) => return busy };
    if let Some(refused) = asked.orchestrator(std::slice::from_ref(&asked.name)).await { return refused; }
    let ex = match asked.groups.leave(&asked.name).await { Ok(ex) => ex, Err(error) => return asked.store_failed(&error) };
    drop(held);
    if ex.is_empty() { return asked.reply(StatusCode::OK, json!({"ok": true, "warning": null})); }
    let mut errs = super::exit::notify_exit(&asked.st, &asked.groups, &asked.name, &ex).await;
    let text = format!("{PREFIX} Você saiu do grupo de trabalho ({}). Volte a operar independente; use hangar-send só quando o usuário pedir.", ex.join(", "));
    if let Err(e) = deliver_text(&asked.st, &asked.name, &text).await {
        errs.push(json!({"sessao": asked.name, "erro": e}));
    }
    let warning = if errs.is_empty() { Value::Null } else {
        envelope("erro_pareamento_saida_falhou", format!("aviso de saída falhou: {}", failures_text(&errs)), json!({"avisos": errs}))
    };
    asked.reply(StatusCode::OK, json!({"ok": true, "warning": warning}))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupMessageBody {
    text: String,
    // O backend decide o caminho de cada membro; os dois só existem no corpo estrito.
    #[serde(default, rename = "remetente_nativo")] _native_sender: bool,
    #[serde(default, rename = "forcar_tmux")] _force_tmux: bool,
}

/// `_group_estourou`: 5 avisos por grupo em 60 s; o pedido recusado também conta.
// ponytail: mapa do processo que só cresce, um gid por grupo, como o `_group_envios` do Python.
fn storm(gid: &str) -> bool {
    static SENT: LazyLock<Mutex<HashMap<String, Vec<Instant>>>> = LazyLock::new(Mutex::default);
    let now = Instant::now();
    let mut sent = SENT.lock().unwrap_or_else(|e| e.into_inner());
    let times = sent.entry(gid.to_owned()).or_default();
    times.retain(|t| now.duration_since(*t) < STORM_WINDOW);
    times.push(now);
    times.len() > STORM_MAX
}

pub(super) async fn group_message(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let Some(body) = asked.body::<GroupMessageBody>() else { return asked.to_python().await };
    // `_transfer_check`: só confere; cada entrega passa pela porta do membro.
    if let Err(busy) = asked.enter().await { return busy; }
    let head = body.text.trim_start();
    if head.starts_with('/') {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_group_message_slash", "group-message não suporta slash-commands", json!({}));
    }
    if head.starts_with("[grupo:") || head.starts_with("[de:") {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_group_message_resposta",
            "aviso de grupo não pode reencaminhar um [grupo:]/[de:] — responda 1:1", json!({}));
    }
    let link = match asked.groups.link(&asked.name).await { Ok(link) => link, Err(error) => return asked.store_failed(&error) };
    let Some(link) = link.filter(|l| !l.peers.is_empty()) else {
        return asked.refuse(StatusCode::NOT_FOUND, "erro_sessao_sem_grupo", "sessão não está num grupo", json!({}));
    };
    if storm(&link.gid) {
        return asked.refuse(StatusCode::TOO_MANY_REQUESTS, "erro_group_message_tempestade",
            &format!("mais de {STORM_MAX} avisos de grupo em {}s — parece loop; espere ou responda 1:1", STORM_WINDOW.as_secs()),
            json!({"max": STORM_MAX, "janela": STORM_WINDOW.as_secs()}));
    }
    // Membro de outra máquina falha com "sessão não encontrada", como no Python: aqui ninguém o
    // procura na lista.
    let local: Vec<String> = link.peers.iter().filter(|p| !is_remote(p)).cloned().collect();
    let live = match providers(&asked.st, &local).await {
        Ok(live) => live,
        Err(failed) => return asked.reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})),
    };
    let text = format!("[grupo: {}] {}", asked.name, body.text);
    let mut failed = Vec::new();
    for p in &link.peers {
        let sent = if live.contains_key(p) { deliver_text(&asked.st, p, &text).await }
            else { Err(envelope("erro_sessao_inexistente", "sessão não encontrada".into(), json!({}))) };
        if let Err(e) = sent { failed.push(json!({"sessao": p, "erro": e})); }
    }
    let warning = if failed.is_empty() { Value::Null } else {
        envelope("erro_pareamento_grupo_falha", format!("falha em: {}", failures_text(&failed)), json!({"avisos": failed}))
    };
    asked.reply(StatusCode::OK, json!({"ok": true, "peers": link.peers, "pulados": [], "warning": warning}))
}

pub(super) async fn contract(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let link = match asked.groups.link(&asked.name).await { Ok(link) => link, Err(error) => return asked.store_failed(&error) };
    let Some(link) = link else {
        return asked.refuse(StatusCode::NOT_FOUND, "erro_sessao_nao_pareada", "sessão não está pareada", json!({}));
    };
    let path = asked.groups.contract_path(&link.gid);
    let shown = path.to_string_lossy().into_owned();
    // Ausente ou ilegível = contrato ainda vazio, como o `except OSError` do Python.
    let content = tokio::task::spawn_blocking(move || std::fs::read_to_string(path).unwrap_or_default()).await.unwrap_or_default();
    asked.reply(StatusCode::OK, json!({"peers": link.peers, "path": shown, "content": content}))
}
