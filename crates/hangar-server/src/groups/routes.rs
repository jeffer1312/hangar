//! Rotas de grupo com as respostas do Python (`api.py`: `pair_session`, `unpair_session`,
//! `group_message`, `pair_contract`; o grupo entre máquinas mora em `federation.rs` e o par 1:1
//! antigo em `legacy.rs`). O convidado
//! segue ao Python; corpo que o Rust não aceita é 422 `erro_corpo_invalido` daqui, sem repasse.
//! Texto do protocolo, avisos e chamadas a outra máquina saem depois de soltar o lock do grupo.
//!
//! `/pair` e `DELETE /pair` seguram a porta de entrada da sessão até gravar o grupo (um rename não
//! grava o nome velho por cima) e a soltam antes dos avisos: o aviso à própria sessão passa pela
//! porta de novo e, com o passe na mão, travaria contra um fechamento que espera esse passe.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use axum::Router;
use axum::body::{Bytes, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::request::Parts;
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::text::{ContractLine, Kind, TextArgs, protocol_text};
use super::local::{JoinRefusal, Snapshot, is_remote};
use super::orq::{is_orchestrator, list_unavailable};
use super::service::{GroupError, GroupService, JoinOutcome, JoinOwned, PromoteError};
use crate::routes::{AppState, cors, gate, pass, pass_any};
use crate::runtime::ingress::IngressPass;
use crate::session_write::input::{deliver_text, json_content_type};
use crate::session_write::{BODY_LIMIT, busy_body, detail_body, invalid_body, json_response, session_name, too_large};
use crate::transcript::py::{py_repr, py_str};

pub(super) use super::text::PREFIX;
/// Toda recusa `Mix` vem do par 1:1 antigo com outra máquina (sidecar sem `fed`), que ainda não virou grupo.
pub(super) const MIX_MSG: &str = "sessão em par 1:1 antigo com outra máquina não entra em grupo local nem recebe outro par; desfaça e refaça o par ou atualize o Hangar da outra máquina";
const ORQ_MSG: &str = "o orquestrador não recebe mensagens; fale com o árbitro";
/// `_group_unavailable` do Python.
const UNAVAILABLE_MSG: &str = "os grupos estão indisponíveis agora";
pub(crate) const STORM_MAX: usize = 5;
pub(crate) const STORM_WINDOW: Duration = Duration::from_secs(60);

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/sessions/{name}/pair", post(pair).delete(unpair).fallback(pass_any))
        .route("/api/sessions/{name}/group-message", post(group_message).fallback(pass_any))
        .route("/api/sessions/{name}/pair/contract", get(contract).put(super::fed_notice::session_contract_put).fallback(pass_any))
        .route("/api/sessions/{name}/pair-remote", post(super::legacy::pair_remote).fallback(pass_any))
        .route("/api/sessions/{name}/unpair-remote", post(super::legacy::unpair_remote).fallback(pass_any))
        .route("/api/sessions/{name}/group", get(super::federation::session_group).fallback(pass_any))
        .route("/api/groups/{gid}", get(super::federation::get_record).put(super::federation::put_record))
        .route("/api/groups/{gid}/ops", post(super::federation::ops))
        .route("/api/groups/{gid}/deliver", post(super::fed_notice::deliver))
        .route("/api/groups/{gid}/contract", get(super::fed_notice::owner_contract).put(super::fed_notice::owner_contract))
}

/// Pedido do dono que o Rust atende, com o corpo já lido.
pub(super) struct Asked { pub(super) st: Arc<AppState>, pub(super) groups: Arc<GroupService>, pub(super) name: String, parts: Parts, bytes: Bytes }

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
        Ok(Asked { st, groups, name, parts, bytes })
    }

    /// Corpo estrito como o `_StrictBody`; o que não casa (campo a mais, tipo errado, surrogate
    /// solto) é 422 daqui: repassar ao Python faria ele pedir a esta rota de novo pela ponte.
    pub(super) fn body<T: DeserializeOwned>(&self) -> Result<T, Response> {
        json_content_type(&self.parts.headers).then(|| serde_json::from_slice(&self.bytes).ok()).flatten()
            .ok_or_else(|| invalid_body(&self.parts.headers))
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
pub(crate) async fn providers(st: &AppState, wanted: &[String]) -> Result<BTreeMap<String, String>, Value> {
    let failed = |e: crate::list::bridge::ListError| list_unavailable(e.code);
    // Pela ponte do Python, a descoberta local basta: o retrato pediria os fatos a ele.
    let pick = |r: &hangar_api::session::SessionRow| (r.name.clone(), r.provider.clone());
    let mut found: BTreeMap<String, String> = if super::bridge::current().is_some() {
        st.list.discover(None).await.map_err(failed)?.iter().map(pick).collect()
    } else { st.list.snapshot().await.map_err(failed)?.rows.iter().map(pick).collect() };
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

pub(crate) fn failures_text(errs: &[Value]) -> String {
    errs.iter().map(|x| format!("{}: {}", py_str(&x["sessao"]), error_text(&x["erro"]))).collect::<Vec<_>>().join("; ")
}

pub(super) fn envelope(code: &str, msg: String, params: Value) -> Value { json!({"code": code, "params": params, "msg": msg}) }

/// Endereços dos pares externos; sem as lojas de convite, nenhum (todo remoto é máquina própria).
pub(crate) fn outside_addresses(st: &AppState) -> HashSet<String> {
    st.share.as_ref().map(|s| s.pairs.addresses()).unwrap_or_default()
}

pub(crate) const ORQ_OUTSIDE_CODE: &str = "erro_grupo_orq_par_externo";
pub(crate) const ORQ_OUTSIDE_MSG: &str = "grupo de orquestração não aceita sessão de fora: os executores entram e saem o tempo todo";
/// Árbitro sozinho no `orq`: recusa com aviso próprio, e o grupo dele fica.
pub(crate) const ORQ_ARBITER_CODE: &str = "erro_par_externo_orq";
pub(crate) const ORQ_ARBITER_MSG: &str = "a sessão é árbitra de uma orquestração e não pareia com outra máquina";

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
    /// Os `peers` são filhas que esta sessão acabou de criar (só a abertura manda).
    #[serde(default)] link: bool,
}

pub(super) async fn pair(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let body = match asked.body::<PairBody>() { Ok(body) => body, Err(refused) => return refused };
    let raw = if !body.peers.is_empty() { body.peers } else if !body.peer.is_empty() { vec![body.peer] } else { Vec::new() };
    let mut others: Vec<String> = Vec::new();
    for p in raw.into_iter().filter(|p| !p.is_empty()) {
        // Membro de fora não tem sidecar aqui: entrar com ele é entrar no grupo da ponta daqui.
        // Assim ele nunca chega à federação, que é só de máquina própria.
        let p = asked.st.share.as_ref().and_then(|s| s.pairs.by_address(&p)).map_or(p, |r| r.local_session);
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
    let mut federated = others.iter().any(|o| is_remote(o));
    let mut legacy = Vec::new();
    for n in &everyone {
        if federated { break; }
        match asked.groups.link(n).await {
            // Sessão daqui num grupo entre máquinas: entrar nele é pedir à dona dele.
            Ok(Some(s)) if s.fed.is_some() => federated = true,
            Ok(Some(s)) if s.peers.iter().any(|p| is_remote(p)) => legacy.push(n.clone()),
            Ok(_) => {}
            // Ilegível não é solta: o par a gravaria por cima.
            Err(error) => return asked.store_failed(&error),
        }
    }
    let mut held = held;
    if !federated && !legacy.is_empty() {
        // Par 1:1 antigo: converte antes, sem o passe (a conversão fala com a outra máquina).
        drop(held);
        super::convert::run(&asked.st, &asked.groups, &legacy).await;
        held = match asked.enter().await { Ok(held) => held, Err(busy) => return busy };
        for n in &legacy {
            match asked.groups.link(n).await {
                Ok(s) => if s.is_some_and(|s| s.fed.is_some()) { federated = true; break; },
                Err(error) => return asked.store_failed(&error),
            }
        }
    }
    if federated {
        if body.link {
            return asked.refuse(StatusCode::BAD_REQUEST, "erro_link_remoto", "a ligação só vale entre sessões desta máquina", json!({}));
        }
        return super::federation::pair(asked, held, others, body.task, body.replace_task, body.orq).await;
    }
    let harness = match providers(&asked.st, &everyone).await {
        Ok(h) => h,
        Err(failed) => return asked.reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})),
    };
    let missing = everyone.iter().filter(|n| !harness.contains_key(*n)).cloned().collect::<Vec<_>>().join(", ");
    if !missing.is_empty() {
        return asked.refuse(StatusCode::NOT_FOUND, "erro_sessao_nao_encontrada_detalhe", &format!("sessão não encontrada: {missing}"), json!({"detalhe": missing}));
    }
    let outside = outside_addresses(&asked.st);
    let children = if body.link { others.clone() } else { Vec::new() };
    let joined = asked.groups.join(JoinOwned { name, others, task: body.task, replace_task: body.replace_task, harness: harness.clone(),
        orq: body.orq, outside, linked: body.link }).await;
    let JoinOutcome { members, gid, task, orq, newcomers, before, promotion_warning } = match joined {
        Ok(outcome) => outcome,
        Err(GroupError::Refused(JoinRefusal::Mix)) => return asked.refuse(StatusCode::BAD_REQUEST, "erro_pareamento_mistura_cross", MIX_MSG, json!({})),
        Err(GroupError::Refused(JoinRefusal::OrqOutside)) => return asked.refuse(StatusCode::CONFLICT, ORQ_OUTSIDE_CODE, ORQ_OUTSIDE_MSG, json!({})),
        Err(GroupError::Refused(JoinRefusal::TaskConflict { existing })) => return asked.refuse(StatusCode::CONFLICT, "erro_pareamento_tarefa_existente",
            &format!("o grupo já tem tarefa: {} — repita com --substituir-tarefa pra trocar", py_repr(&json!(existing))), json!({"existente": existing})),
        // Só a ligação chega aqui: o `join` local não recusa `orq` de outro jeito.
        Err(GroupError::Refused(JoinRefusal::Orq)) => return asked.refuse(StatusCode::CONFLICT, "erro_link_orq",
            "grupo de orquestração não recebe sessão ligada", json!({})),
        Err(GroupError::Orq(PromoteError::Conflict(text))) => return asked.refuse(StatusCode::CONFLICT, "erro_orq_arquivo_mudou", &text, json!({})),
        Err(GroupError::Orq(PromoteError::ConflictWithRemaining { message, names })) => return asked.refuse(
            StatusCode::CONFLICT, "erro_orq_arquivo_mudou", &message, json!({"nao_sairam": names})),
        Err(GroupError::Orq(PromoteError::Unavailable(code))) => return asked.refuse(StatusCode::SERVICE_UNAVAILABLE,
            "erro_grupo_indisponivel", UNAVAILABLE_MSG, json!({"detalhe": code})),
        Err(error) => return asked.store_failed(&error),
    };
    drop(held);
    if !children.is_empty() { record_linked(&asked.st, children).await; }
    // Só quem estava solto recebe o protocolo; grupo de orquestração não recebe nada. A ligação também
    // não: o protocolo viraria um turno a mais da filha, pago pelo pai.
    let notices = if orq || body.link { Vec::new() } else { newcomers };
    let contract = asked.groups.contract_path(&gid);
    let inside: BTreeMap<String, String> = harness.into_iter().filter(|(n, _)| members.contains(n)).collect();
    let mut errs = Vec::new();
    for m in &notices {
        let args = TextArgs { me: m.clone(), others: members.iter().filter(|x| *x != m).cloned().collect(), task: task.clone(),
            contract: ContractLine { path: Some(contract.clone()), remote_owner: None }, harness: inside.clone(), ..Default::default() };
        let sent = match protocol_text(Kind::Group, &args) {
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
    // Falhar aqui só deixa o membro novo invisível para quem é de fora: o lado seguro, mas visível.
    let denied = match &asked.st.share { Some(share) => crate::share::pair_routes::grant_members(share, &members).await, None => Vec::new() };
    let warning = if let Some(message) = promotion_warning {
        envelope("erro_orq_promocao_sem_identidade", format!("o grupo ficou, mas o time não foi promovido: {message}"),
            json!({"sessao": asked.name, "grupo": gid, "detalhe": message}))
    } else if errs.is_empty() { access_warning(&denied) } else {
        envelope("erro_pareamento_aviso_parcial", format!("aviso falhou em: {}", failures_text(&errs)), json!({"avisos": errs, "acesso": denied}))
    };
    asked.reply(StatusCode::OK, json!({"ok": true, "members": members, "gid": gid, "warning": warning}))
}

/// Acesso de fora não gravado ao membro novo; `Null` sem falha.
pub(super) fn access_warning(denied: &[String]) -> Value {
    if denied.is_empty() { return Value::Null; }
    tracing::warn!(code = ACCESS_FAILED, failed = denied.len(), "groups: acesso de fora ao membro novo não gravado");
    envelope(ACCESS_FAILED, format!("o acesso de quem é de fora não foi gravado para: {}", denied.join("; ")), json!({"acesso": denied}))
}

pub(super) const ACCESS_FAILED: &str = "erro_grupo_acesso_externo_falhou";

/// `linked_at` das filhas em `link-returns.json`. Falhar não desfaz a ligação, que já está no grupo.
async fn record_linked(st: &AppState, children: Vec<String>) {
    let dir = st.hangar_dir.clone();
    let now = crate::state::monitor::wall_now();
    let first = children.first().cloned().unwrap_or_default();
    let written = super::link_state::blocking(move || super::link_state::record_linked(&dir, &children, now)).await;
    if let Err(error) = written {
        tracing::warn!(code = "erro_link_registro", %error, "groups: linked_at da filha não gravado");
        // Sem `linked_at` o retorno da filha não acha o começo: a falha vai ao diário, visível.
        st.diag.report("rust.groups_link_record_failed", &first, "erro_link_registro", "linked_at da filha não gravado; a ligação vale");
    }
}

pub(super) async fn unpair(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let asked = match Asked::take(st, peer, req).await { Ok(a) => a, Err(response) => return response };
    let held = match asked.enter().await { Ok(held) => held, Err(busy) => return busy };
    if let Some(refused) = asked.orchestrator(std::slice::from_ref(&asked.name)).await { return refused; }
    let (ex, tell) = match super::exit::leave(&asked.st, &asked.groups, &asked.name, None).await { Ok(left) => left, Err(error) => return asked.store_failed(&error) };
    drop(held);
    if ex.is_empty() { return asked.reply(StatusCode::OK, json!({"ok": true, "warning": null})); }
    let mut errs = super::exit::notify_exit(&asked.st, &asked.groups, &asked.name, &tell).await;
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
pub(crate) fn storm(gid: &str) -> bool {
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
    let body = match asked.body::<GroupMessageBody>() { Ok(body) => body, Err(refused) => return refused };
    // `_transfer_check`: só confere; cada entrega passa pela porta do membro.
    if let Err(busy) = asked.enter().await { return busy; }
    let head = body.text.trim_start();
    if head.starts_with('/') {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_group_message_slash", "group-message não suporta slash-commands", json!({}));
    }
    // `[de fora:` é recado que já atravessou o par: reespalhar faria bate-volta entre máquinas.
    if head.starts_with("[grupo:") || head.starts_with("[de:") || head.starts_with("[de fora:") {
        return asked.refuse(StatusCode::BAD_REQUEST, "erro_group_message_resposta",
            "aviso de grupo não pode reencaminhar um [grupo:]/[de:] — responda 1:1", json!({}));
    }
    let link = match asked.groups.link(&asked.name).await { Ok(link) => link, Err(error) => return asked.store_failed(&error) };
    let Some(link) = link.filter(|l| !l.peers.is_empty()) else {
        return asked.refuse(StatusCode::NOT_FOUND, "erro_sessao_sem_grupo", "sessão não está num grupo", json!({}));
    };
    // Grupo entre máquinas: limite e espalhamento são da dona.
    if link.fed.is_some() { return super::fed_notice::group_message(&asked, &link, body.text).await; }
    if storm(&link.gid) {
        return asked.refuse(StatusCode::TOO_MANY_REQUESTS, "erro_group_message_tempestade",
            &format!("mais de {STORM_MAX} avisos de grupo em {}s — parece loop; espere ou responda 1:1", STORM_WINDOW.as_secs()),
            json!({"max": STORM_MAX, "janela": STORM_WINDOW.as_secs()}));
    }
    // Máquina própria falha com "sessão não encontrada", como no Python: aqui ninguém a procura na
    // lista. Membro de fora recebe pela chave de saída do próprio par.
    let local: Vec<String> = link.peers.iter().filter(|p| !is_remote(p)).cloned().collect();
    let live = match providers(&asked.st, &local).await {
        Ok(live) => live,
        Err(failed) => return asked.reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": failed})),
    };
    let text = format!("[grupo: {}] {}", asked.name, body.text);
    // Um id por aviso: dois pares com a mesma máquina mandam dois POSTs e lá cada sessão o recebe uma vez.
    let notice = crate::mods::state::random_hex(8);
    let mut failed = Vec::new();
    for p in &link.peers {
        let outside = asked.st.share.as_ref().and_then(|s| s.pairs.by_address(p).map(|rec| (s, rec)));
        let sent = if let Some((share, rec)) = outside {
            crate::share::pair_routes::notify_outside(&asked.st, share, &crate::share::pair_routes::Direct, &rec, &asked.name, &body.text, &notice).await
        } else if live.contains_key(p) { deliver_text(&asked.st, p, &text).await }
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
    // Também o grupo de uma máquina: a leitura leva o `mtime` que a gravação exige.
    super::fed_notice::session_contract(&asked, &link).await
}
