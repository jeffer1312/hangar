//! Par externo (`external_pair_api.py`): convite, aceite, resgate, recado e fim entre máquinas de
//! pessoas diferentes. As rotas do dono vão atrás da checagem de dono de quem monta; as do
//! convidado, atrás do porteiro da 8766.
//!
//! Uma chave pode ter mais de um par (resgate com token já existente): recado e saída nomeiam a
//! sessão (`session` no corpo, `?session=` na saída) e só sem ambiguidade a omitem, como o Python.
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{any, delete, get, post};
use axum::{Extension, Router};
use futures_util::future::BoxFuture;
use serde_json::{Map, Value, json};

use super::caller::Caller;
use super::owner_routes::{OwnerShareState, io_failed, prereq_error, reach_or_revoke};
use super::pairs::{self, ExternalPair};
use super::store::{Kind, PeekError, Share};
use super::tunnel::GUEST_PORT;
use super::ShareState;
use crate::session_write::{detail, json_response};
use crate::groups::peers::PeerError;
use crate::groups::local::{JoinRefusal, external_link_refusal, is_remote};
use crate::groups::routes::{ORQ_ARBITER_CODE, ORQ_ARBITER_MSG, ORQ_OUTSIDE_CODE, ORQ_OUTSIDE_MSG, STORM_MAX, STORM_WINDOW, outside_addresses, providers, storm};
use crate::groups::service::GroupError;
use crate::groups::store::file_stem;
use crate::routes::AppState;
use crate::session_write::input::{deliver_text, write_input};
use crate::transcript::py::py_str;
use crate::workspace_routes::html_escape;

const MSG_ALREADY: &str = "a sessão está pareada com outra máquina tua — desfaça esse par antes de parear com outra pessoa";
const REMOTE_LABEL: &str = "resposta da outra máquina: ";
const MSG_PARTIAL: &str = "pareamento desfeito aqui, mas a outra máquina pode continuar pareada: desfaça lá";
const RESP_KEYS: [&str; 5] = ["ok", "delivered", "steered", "native", "queued"];
const MAX_MESSAGE: usize = 64000;

fn repeated() -> Response {
    detail(StatusCode::CONFLICT, "erro_par_repetido", "esta sessão já está pareada com essa sessão de fora", json!({}))
}

pub type Reply = Result<(u16, Option<Value>), PeerError>;

/// A outra máquina; os testes trocam o Funnel de fora por um servidor local.
pub trait Remote: Send + Sync {
    fn call<'a>(&'a self, address: &'a str, token: Option<&'a str>, method: reqwest::Method, path: &'a str,
                body: Option<&'a Value>) -> BoxFuture<'a, Reply>;
}

pub struct Direct;

impl Remote for Direct {
    fn call<'a>(&'a self, address: &'a str, token: Option<&'a str>, method: reqwest::Method, path: &'a str,
                body: Option<&'a Value>) -> BoxFuture<'a, Reply> {
        Box::pin(pairs::call(address, token, method, path, body))
    }
}

pub struct PairState {
    pub app: Arc<AppState>,
    /// Lojas, porta do dono e conferência pela internet, as mesmas do convite comum.
    pub owner: Arc<OwnerShareState>,
    pub remote: Arc<dyn Remote>,
}

type St = State<Arc<PairState>>;

/// Recusa com o envelope do `erro()`: status e `{code, params, msg}`.
pub type Refusal = (StatusCode, Value);

fn fail(status: StatusCode, code: &str, msg: &str, params: Value) -> Refusal {
    (status, json!({"code": code, "params": params, "msg": msg}))
}

pub(super) fn respond((status, detail): Refusal) -> Response { json_response(status, json!({"detail": detail})) }

fn groups_unavailable(code: &str) -> Refusal {
    fail(StatusCode::SERVICE_UNAVAILABLE, "erro_grupo_indisponivel", "os grupos estão indisponíveis agora", json!({"detalhe": code}))
}

fn cut(text: &str, n: usize) -> String { text.chars().take(n).collect() }

/// (motivo, código, frase) do convite que não vale (`_REASONS`).
pub fn reason(e: &PeekError) -> (&'static str, &'static str, &'static str) {
    match e {
        PeekError::Used => ("used", "erro_convite_usado", "este convite já foi usado"),
        PeekError::Expired => ("expired", "erro_convite_vencido", "este convite venceu"),
        PeekError::Revoked => ("revoked", "erro_convite_revogado", "este convite foi cancelado por quem compartilhou"),
        PeekError::Unknown | PeekError::Io(_) => ("unknown", "erro_convite_inexistente", "convite não encontrado"),
    }
}

const REASONS: [PeekError; 4] = [PeekError::Used, PeekError::Expired, PeekError::Revoked, PeekError::Unknown];

pub(super) fn code_error(e: &PeekError) -> Refusal {
    let (why, code, msg) = reason(e);
    let status = if why == "unknown" { StatusCode::NOT_FOUND } else { StatusCode::GONE };
    fail(status, code, msg, json!({"reason": why}))
}

/// `_PAGE` do convite e do par: pt-BR fixo, a única tela servida pela porta do convidado.
// ponytail: fora do Paraglide; é a única tela servida pela porta do convite.
pub fn page(body: &str) -> String {
    format!(r#"<!doctype html><html lang="pt-BR"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1"><meta name="robots" content="noindex">
<title>Convite do Hangar</title>
<style>body{{font-family:system-ui,sans-serif;max-width:28rem;margin:3rem auto;padding:0 1rem;
color:#1b1b1f;background:#fafafa}}@media(prefers-color-scheme:dark){{body{{color:#eee;background:#161618}}}}
a,button{{display:block;width:100%;margin:.6rem 0;padding:.8rem;border-radius:.6rem;font-size:1rem;
text-align:center;border:1px solid #888;background:transparent;color:inherit;text-decoration:none}}
.p{{background:#3b6ef5;color:#fff;border-color:#3b6ef5}}</style></head><body>{body}</body></html>"#)
}

fn server_id(app: &AppState) -> String { app.groups.as_ref().map(|g| g.server_id().to_owned()).unwrap_or_default() }

/// `_owner()`: o id da máquina, ou o nome do sistema.
fn machine(app: &AppState) -> String { super::machine_name(&server_id(app)) }

/// O nome da máquina pode ter espaço e acento; o outro lado só aceita o alfabeto de `valid_owner`.
fn my_owner(app: &AppState) -> String { cut(&pairs::slug(&machine(app)), 40) }

fn leave_path(session: &str) -> String {
    format!("/api/pair?session={}", form_urlencoded::byte_serialize(session.as_bytes()).collect::<String>())
}

/// Corpo JSON com campos texto `(nome, obrigatório, teto em caracteres)`, como o modelo do
/// pydantic: 422; o opcional ausente vem vazio. `guest` = o formato do `share_guest_api.py`, que
/// junta todos os problemas e descreve o JSON torto; o do par para no primeiro.
pub(super) fn fields(bytes: &[u8], spec: &[(&str, bool, Option<usize>)], guest: bool) -> Result<(Vec<String>, Map<String, Value>), Response> {
    let invalid = |issues: Vec<Value>| json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": issues}));
    let map = match serde_json::from_slice::<Value>(bytes) {
        Ok(Value::Object(map)) => map,
        Ok(other) => return Err(invalid(vec![json!({"type": "model_attributes_type", "loc": ["body"],
            "msg": "Input should be a valid dictionary or object to extract fields from", "input": other})])),
        Err(_) if guest => return Err(invalid(vec![json!({"type": "json_invalid", "loc": ["body", 0], "msg": "JSON decode error",
            "input": {}, "ctx": {"error": "Expecting value"}})])),
        Err(_) => return Err(invalid(vec![json!({"type": "json_invalid", "loc": ["body", 0], "msg": "JSON decode error"})])),
    };
    let (mut out, mut issues) = (Vec::new(), Vec::new());
    for &(name, required, max) in spec {
        let issue = match map.get(name) {
            None if required => json!({"type": "missing", "loc": ["body", name], "msg": "Field required", "input": map}),
            None => { out.push(String::new()); continue }
            Some(Value::String(s)) => match max {
                Some(m) if s.chars().count() > m => json!({"type": "string_too_long", "loc": ["body", name],
                    "msg": format!("String should have at most {m} characters"), "input": s, "ctx": {"max_length": m}}),
                _ => { out.push(s.clone()); continue }
            },
            Some(other) => json!({"type": "string_type", "loc": ["body", name], "msg": "Input should be a valid string", "input": other}),
        };
        issues.push(issue);
        if !guest { break }
    }
    if issues.is_empty() { Ok((out, map)) } else { Err(invalid(issues)) }
}

/// `_refused`: o outro lado respondeu com o envelope dele; convite usado/vencido vira a frase própria.
fn refused_by_remote(status: u16, detail: &Value) -> Refusal {
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY);
    let text = match detail {
        Value::Object(d) => {
            for e in &REASONS {
                let (why, code, msg) = reason(e);
                if d.get("code").and_then(Value::as_str) == Some(code) {
                    return fail(status, code, msg, json!({"reason": why}));
                }
            }
            d.get("msg").and_then(Value::as_str).map_or_else(|| py_str(detail), str::to_owned)
        }
        other => py_str(other),
    };
    // Texto que o outro lado escreveu vai rotulado: a tela não deve tomá-lo por mensagem do app.
    let text = format!("{REMOTE_LABEL}{}", cut(&text, 300));
    fail(status, "erro_par_recusado", &text, json!({"detalhe": text}))
}

fn offline(text: &str, remote: bool) -> Refusal {
    let detail = format!("{}{}", if remote { REMOTE_LABEL } else { "" }, cut(text, 300));
    fail(StatusCode::BAD_GATEWAY, "erro_par_fora_do_ar", &detail, json!({"detalhe": detail}))
}

/// `_remote_failure` do envio.
fn remote_failure(e: &PeerError, address: &str) -> Refusal {
    match e {
        PeerError::Refused { status: 429, detail } => {
            // Só os dois números que a mensagem usa: params do outro lado não entram soltos.
            let mut params = Map::new();
            if let Some(raw) = detail.get("params").and_then(Value::as_object) {
                for k in ["max", "janela"] {
                    if let Some(v) = raw.get(k).filter(|v| v.is_i64() || v.is_u64()) { params.insert(k.into(), v.clone()); }
                }
            }
            fail(StatusCode::TOO_MANY_REQUESTS, "erro_group_message_tempestade", "recados demais em pouco tempo", Value::Object(params))
        }
        PeerError::Refused { status: 503, .. } => fail(StatusCode::SERVICE_UNAVAILABLE, "erro_sessao_indisponivel", "o par ainda está abrindo", json!({})),
        PeerError::Refused { detail, .. } => match detail.get("msg").and_then(Value::as_str) {
            Some(msg) if detail.is_object() => offline(msg, true),
            _ => offline(&py_str(detail), true),
        },
        other => offline(&other.text(address), false),
    }
}

/// Cada passo do desfazer roda mesmo que o anterior falhe; senão sobra token vivo no disco.
fn guarded<T, E: std::fmt::Display>(what: &str, done: Result<T, E>) -> bool {
    if let Err(e) = done {
        tracing::warn!(code = "external_pair_undo_failed", step = what, error = %e, "par externo: passo do desfazer falhou");
        return false;
    }
    true
}

async fn unlink_guarded(app: &AppState, local: &str, address: &str) {
    if let Some(groups) = &app.groups {
        guarded("restaurar o grupo", groups.external_unlink(local, address).await);
    }
}

/// Registro de par e convite saem juntos; `false` = algum dos dois ficou no disco.
fn drop_record(share: &ShareState, id: &str) -> bool {
    guarded("remover o registro", share.pairs.remove(id)) & guarded("revogar o convite", share.store.revoke(id))
}

/// Avisa o outro lado de que o par acabou, com o token que ele entregou.
async fn tell_leave(remote: &dyn Remote, rec: &ExternalPair) -> Result<(), PeerError> {
    remote.call(&rec.peer_address, Some(&rec.peer_token), reqwest::Method::DELETE, &leave_path(&rec.peer_session), None).await.map(drop)
}

async fn undo_local(app: &AppState, share: &ShareState, local: &str, address: &str, id: &str) {
    unlink_guarded(app, local, address).await;
    drop_record(share, id);
}

fn link_refused(refusal: &JoinRefusal) -> Refusal {
    match refusal {
        JoinRefusal::OrqOutside => fail(StatusCode::CONFLICT, ORQ_OUTSIDE_CODE, ORQ_OUTSIDE_MSG, json!({})),
        JoinRefusal::Orq => fail(StatusCode::CONFLICT, ORQ_ARBITER_CODE, ORQ_ARBITER_MSG, json!({})),
        _ => fail(StatusCode::CONFLICT, "erro_pareamento_mistura_cross", MSG_ALREADY, json!({})),
    }
}

/// Grava o par no grupo de `name`: o endereço entra em cada membro local dele.
async fn join_external(app: &AppState, name: &str, peer: &str) -> Result<(), Refusal> {
    let Some(groups) = &app.groups else { return Err(groups_unavailable("groups_unavailable")) };
    let wanted = [name.to_owned()];
    let harness = providers(app, &wanted).await.map_err(|failed| (StatusCode::SERVICE_UNAVAILABLE, failed))?
        .into_iter().filter(|(n, _)| n == name).collect();
    match groups.external_link(name, peer, harness, outside_addresses(app)).await {
        Ok(_) => {
            if let Ok(Some(side)) = groups.link(name).await { crate::groups::federation::outside_changed(app, &side, peer, true).await; }
            Ok(())
        }
        Err(GroupError::Refused(r)) => Err(link_refused(&r)),
        Err(e) => {
            tracing::warn!(code = "external_pair_group_failed", session = name, error = %e, "par externo: o grupo não foi gravado");
            // A escrita pode ter chegado ao disco: o `unlink` só age se o endereço estiver lá.
            unlink_guarded(app, name, peer).await;
            Err(groups_unavailable("groups_store_failed"))
        }
    }
}

/// A mesma recusa do `external_link`, antes de falar com o outro lado ou de gastar o código.
async fn link_refusal(app: &AppState, name: &str) -> Result<(), Refusal> {
    let Some(groups) = &app.groups else { return Err(groups_unavailable("groups_unavailable")) };
    let link = groups.link(name).await.map_err(|_| groups_unavailable("groups_store_failed"))?;
    let outside = outside_addresses(app);
    match external_link_refusal(link.as_ref(), &|p| outside.contains(p)) {
        Some(r) => Err(link_refused(&r)),
        None => Ok(()),
    }
}

/// O aviso de par à sessão daqui; `Err` = o texto da falha (`_erro_texto`).
async fn notify_paired(app: &AppState, me: &str, peer: &str, owner: &str) -> Result<(), String> {
    let args = crate::groups::text::TextArgs { me: me.into(), peer: peer.into(), owner: owner.into(), ..Default::default() };
    let text = crate::groups::text::protocol_text(crate::groups::text::Kind::External, &args);
    let sent = match text { Ok(text) => deliver_text(app, me, &text).await, Err(e) => Err(e) };
    sent.map_err(|e| e.get("msg").map_or_else(|| py_str(&e), py_str))
}

/// `level` do corpo: "full" = acesso completo (`Share`), "view" ou ausente = ver e recado (`Pair`).
fn level_in(map: &Map<String, Value>) -> Result<Kind, Response> {
    match map.get("level") {
        None | Some(Value::Null) => Ok(Kind::Pair),
        Some(Value::String(s)) if s == "view" => Ok(Kind::Pair),
        Some(Value::String(s)) if s == "full" => Ok(Kind::Share),
        Some(other) => Err(json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"type": "literal_error",
            "loc": ["body", "level"], "msg": "Input should be 'full' or 'view'", "input": other}]}))),
    }
}

fn level_name(kind: Kind) -> &'static str { if kind == Kind::Share { "full" } else { "view" } }

/// Sem `level` = Hangar antigo do outro lado: o que eu dou a ele cai para "ver e recado".
fn clamp_for_old(other: Option<&Value>, mine: Kind) -> Kind {
    if other.is_none_or(Value::is_null) { Kind::Pair } else { mine }
}

/// `name` e os membros daqui do grupo dele.
async fn group_locals(app: &AppState, name: &str) -> Result<Vec<String>, String> {
    let mut locals = vec![name.to_owned()];
    if let Some(groups) = &app.groups
        && let Some(side) = groups.link(name).await.map_err(|e| e.to_string())? {
        locals.extend(side.peers.into_iter().filter(|p| !is_remote(p) && p != name));
    }
    Ok(locals)
}

/// `(sessão, vida)` de cada uma; sem vida conhecida o acesso não teria fim e ela fica de fora.
async fn with_lives(share: &ShareState, sessions: impl IntoIterator<Item = String>) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    for session in sessions {
        match share.lives.life(&session, true).await {
            Some(life) => out.push((session, Some(life))),
            None => tracing::warn!(code = "external_pair_grant_skipped", session = %session, "par externo: sessão sem vida, acesso não dado"),
        }
    }
    out
}

/// O nível vale para cada sessão daqui do grupo da ponta do par, cada uma com a própria vida.
async fn grant_group(app: &AppState, share: &ShareState, root_id: &str, name: &str, kind: Kind) -> Result<(), String> {
    let sessions = with_lives(share, group_locals(app, name).await?).await;
    share.store.grant_many(root_id, &sessions, kind).map_err(|e| e.to_string())
}

/// Membro local do grupo sem acesso pelo par de fora ganha "ver e recado", qualquer que seja o
/// nível do par: entrar em grupo (agente, `orq`, `hangar-send`) nunca dá escrita a quem é de fora.
/// Quem já tinha fica com o que tinha. Devolve as falhas de disco.
pub async fn grant_members(share: &ShareState, members: &[String]) -> Vec<String> {
    let mut errs = Vec::new();
    // O caso comum, grupo sem ninguém de fora, não lê as lojas.
    if !members.iter().any(|m| is_remote(m)) { return errs }
    for rec in share.pairs.all().into_iter().filter(|r| members.contains(&r.address())) {
        let have: Vec<String> = share.store.children(&rec.share_id).into_iter().map(|c| c.session).collect();
        let missing = members.iter().filter(|m| !is_remote(m) && **m != rec.local_session && !have.contains(m)).cloned().collect::<Vec<_>>();
        let sessions = with_lives(share, missing).await;
        if let Err(e) = share.store.grant_many(&rec.share_id, &sessions, Kind::Pair) {
            let names: Vec<&str> = sessions.iter().map(|(s, _)| s.as_str()).collect();
            errs.push(format!("{}: {e}", names.join(", ")));
        }
    }
    errs
}

// --- dono ------------------------------------------------------------------------------------

/// O "Liberar" (ou o rebaixar) de `name` para a máquina `alias`: vale para cada par dessa máquina no
/// grupo da sessão. Só o dono chega aqui; o convidado é barrado pelo porteiro.
async fn pair_access(State(st): St, Path(name): Path<String>, bytes: Bytes) -> Response {
    let (v, body) = match fields(&bytes, &[("alias", true, Some(40))], false) { Ok(v) => v, Err(r) => return r };
    let level = match level_in(&body) { Ok(l) => l, Err(r) => return r };
    let share = &st.owner.share;
    let Some(life) = share.lives.life(&name, true).await else {
        return detail(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessão não encontrada", json!({}));
    };
    let recs = match pairs_reaching(&st.app, share, &name, &v[0]).await { Ok(r) => r, Err(r) => return respond(r) };
    if recs.is_empty() { return respond(no_pair()) }
    for rec in &recs {
        if let Err(e) = share.store.grant(&rec.share_id, &name, Some(life.clone()), level) { return io_failed(e) }
    }
    // A sessão pode ter saído do grupo enquanto o nível era gravado; a saída já tirou o acesso, e o
    // grant que veio depois o devolveria. Relê e desfaz o que deixou de valer.
    // Sem conseguir reler, fecha: o dono repete o "Liberar".
    let (still, failed) = match pairs_reaching(&st.app, share, &name, &v[0]).await {
        Ok(r) => (r.into_iter().map(|r| r.share_id).collect::<Vec<_>>(), None),
        Err(r) => (Vec::new(), Some(respond(r))),
    };
    for rec in recs.iter().filter(|r| !still.contains(&r.share_id)) {
        if let Err(e) = share.store.ungrant(&rec.share_id, &name) { return io_failed(e) }
    }
    if let Some(r) = failed { return r }
    if still.is_empty() { return respond(no_pair()) }
    json_response(StatusCode::OK, json!({"ok": true}))
}

/// Os pares da máquina `alias` que alcançam `session`: os dela e os dos membros do grupo dela.
async fn pairs_reaching(app: &AppState, share: &ShareState, session: &str, alias: &str) -> Result<Vec<ExternalPair>, Refusal> {
    let peers = match &app.groups {
        Some(groups) => groups.link(session).await.map_err(|_| groups_unavailable("groups_store_failed"))?
            .map(|l| l.peers).unwrap_or_default(),
        None => Vec::new(),
    };
    Ok(share.pairs.all().into_iter()
        .filter(|r| r.alias == alias && (r.local_session == session || peers.contains(&r.address()))).collect())
}

/// "Sair do pareamento" e "Desfazer par": acaba o par `alias::session` para o grupo todo e avisa o
/// outro lado.
async fn undo_pair(State(st): St, Path(alias): Path<String>, RawQuery(query): RawQuery) -> Response {
    let session = form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(k, _)| k == "session").map(|(_, v)| v.into_owned()).unwrap_or_default();
    let address = format!("{alias}::{session}");
    let share = &st.owner.share;
    let Some(rec) = share.pairs.by_address(&address) else { return respond(no_pair()) };
    let errs = end_pair(&st.app, share, &*st.remote, &rec.local_session, &address).await.unwrap_or_default();
    let warning = if errs.is_empty() { Value::Null } else {
        json!({"code": "erro_pareamento_saida_falhou", "params": {"avisos": errs}, "msg": "aviso de saída falhou"})
    };
    json_response(StatusCode::OK, json!({"ok": true, "warning": warning}))
}

async fn pair_invite(State(st): St, Path(name): Path<String>, bytes: Bytes) -> Response {
    // Corpo vazio (cliente antigo) = ver e recado.
    let map = match serde_json::from_slice::<Value>(&bytes) { Ok(Value::Object(m)) => m, _ => Map::new() };
    let level = match level_in(&map) { Ok(l) => l, Err(r) => return r };
    let share = &st.owner.share;
    let Some(life) = share.lives.life(&name, true).await else {
        return detail(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessão não encontrada", json!({}));
    };
    if st.owner.owner_port == GUEST_PORT {
        let msg = format!("o app roda na porta do convite ({GUEST_PORT}): troque CP_PORT no backend/.env e reinicie");
        return detail(StatusCode::CONFLICT, "erro_compartilhar_porta_do_convite", &msg, json!({"port": GUEST_PORT}));
    }
    let base = match share.open_guest_url().await { Ok(b) => b, Err(e) => return prereq_error(e) };
    let (s, code) = match share.store.create_pair_invite(&name, Some(life), level) { Ok(v) => v, Err(e) => return io_failed(e) };
    // Mesma regra do convite comum: o link só sai depois de abrir pela internet.
    if let Some(refusal) = reach_or_revoke(&st.owner, &base, format!("/par/{code}"), &s.id).await { return refusal }
    json_response(StatusCode::OK, json!({"id": s.id, "link": format!("{base}/par/{code}"), "expires_at": s.code_expires_at}))
}

/// Desfaz o lado de lá com o token que ele entregou; `false` = o outro lado pode seguir pareado.
async fn undo_remote(remote: &dyn Remote, address: &str, token: &str, session: &str) -> bool {
    match remote.call(address, Some(token), reqwest::Method::DELETE, &leave_path(session), None).await {
        Ok(_) => true,
        Err(e) => {
            tracing::warn!(code = "external_pair_remote_not_undone", error = %e.text(address), "par externo: outro lado não desfeito");
            false
        }
    }
}

async fn pair_accept(State(st): St, Path(name): Path<String>, bytes: Bytes) -> Response {
    let (vals, body) = match fields(&bytes, &[("link", true, None)], false) { Ok(v) => v, Err(r) => return r };
    let level = match level_in(&body) { Ok(l) => l, Err(r) => return r };
    let parsed = pairs::parse_pair_link(&vals[0])
        .filter(|(_, code)| (1..=64).contains(&code.len()) && code.bytes().all(|b| b.is_ascii_alphanumeric()));
    let Some((address, code)) = parsed else {
        return detail(StatusCode::BAD_REQUEST, "erro_par_link_invalido", "link de par inválido", json!({}));
    };
    let (app, share, remote) = (&*st.app, &*st.owner.share, &*st.remote);
    let Some(life) = share.lives.life(&name, true).await else {
        return detail(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessão não encontrada", json!({}));
    };
    // Antes de chamar o outro lado: recusar depois queimaria o código dele à toa.
    if let Err(r) = link_refusal(app, &name).await { return respond(r) }
    if st.owner.owner_port == GUEST_PORT {
        return detail(StatusCode::CONFLICT, "erro_compartilhar_porta_do_convite", "porta do convite", json!({"port": GUEST_PORT}));
    }
    let my_base = match share.open_guest_url().await { Ok(b) => b, Err(e) => return prereq_error(e) };
    // Vale antes do resgate: recado do outro lado pode chegar antes de este lado terminar.
    let (mine, my_token) = match share.store.create_redeemed(&name, Some(life), Kind::Pair) { Ok(v) => v, Err(e) => return io_failed(e) };
    let ask = json!({"code": code, "session": name, "address": my_base, "token": my_token, "owner": my_owner(app),
        "level": level_name(level)});
    let resp = match remote.call(&address, None, reqwest::Method::POST, "/api/pair/redeem", Some(&ask)).await {
        Ok((_, resp)) => resp,
        Err(e) => {
            guarded("revogar o convite", share.store.revoke(&mine.id));
            return respond(match &e {
                PeerError::Refused { status: s @ (404 | 410 | 409 | 400), detail } => refused_by_remote(*s, detail),
                // A chamada pode ter chegado e gravado o par lá; só a resposta se perdeu.
                PeerError::Transport(_) => fail(StatusCode::BAD_GATEWAY, "erro_par_incerto", "não deu para confirmar o pareamento", json!({})),
                other => offline(&other.text(&address), matches!(other, PeerError::Refused { .. })),
            });
        }
    };
    let resp = resp.unwrap_or(Value::Null);
    let text = |k: &str| resp.get(k).and_then(Value::as_str);
    let (owner, session, token) = (text("owner").unwrap_or(""), text("session").unwrap_or(""), text("token"));
    if !(pairs::valid_owner(owner) && pairs::valid_session(session) && token.is_some_and(pairs::valid_token)) {
        guarded("revogar o convite", share.store.revoke(&mine.id));
        // O outro lado já gravou o par com o nosso token: desfaz lá, se o token dele serve pra isso.
        let undoable = token.filter(|t| !t.is_empty() && t.bytes().all(|b| (0x20..0x7f).contains(&b)));
        if let Some(t) = undoable && !undo_remote(remote, &address, t, session).await {
            return detail(StatusCode::BAD_GATEWAY, "erro_pareamento_desfeito_parcial", MSG_PARTIAL, json!({}));
        }
        return detail(StatusCode::BAD_GATEWAY, "erro_par_resposta_invalida", "resposta do par inválida", json!({}));
    }
    let token = token.unwrap_or_default();
    // A sessão de fora só se conhece pela resposta: o outro lado já gravou e tem de ser desfeito.
    if share.pairs.paired_with(&address, session) {
        guarded("revogar o convite", share.store.revoke(&mine.id));
        if !undo_remote(remote, &address, token, session).await {
            return detail(StatusCode::BAD_GATEWAY, "erro_pareamento_desfeito_parcial", MSG_PARTIAL, json!({}));
        }
        return repeated();
    }
    let given = clamp_for_old(resp.get("level"), level);
    let alias = share.pairs.free_alias(owner, &address, &app.peers.ids(), &server_id(app));
    let peer = format!("{alias}::{session}");
    let mut linked = false;
    let done: Result<(), String> = async {
        share.pairs.add(ExternalPair { share_id: mine.id.clone(), local_session: name.clone(), alias: alias.clone(),
            peer_owner: owner.into(), peer_session: session.into(), peer_address: address.clone(), peer_token: token.into(),
            created_at: share.store.now() }).map_err(|e| e.to_string())?;
        join_external(app, &name, &peer).await.map_err(|(_, d)| d["msg"].as_str().unwrap_or_default().to_owned())?;
        linked = true;
        grant_group(app, share, &mine.id, &name, given).await?;
        notify_paired(app, &name, &peer, owner).await
    }.await;
    if let Err(e) = done {
        // Qualquer falha aqui desfaz os dois lados.
        if linked { unlink_guarded(app, &name, &peer).await; }
        guarded("revogar o convite", share.store.revoke(&mine.id));
        let undone = undo_remote(remote, &address, token, session).await;
        guarded("remover o registro", share.pairs.remove(&mine.id));
        if !undone {
            return detail(StatusCode::BAD_GATEWAY, "erro_pareamento_desfeito_parcial", MSG_PARTIAL, json!({}));
        }
        let msg = format!("pareamento desfeito: falha ao avisar as sessões ({e})");
        return detail(StatusCode::BAD_GATEWAY, "erro_pareamento_desfeito", &msg, json!({"avisos": e}));
    }
    // `level` = o que o outro lado me deu (Hangar antigo não diz: "view"); `level_given` = o que eu dei.
    let received = if resp.get("level").and_then(Value::as_str) == Some("full") { "full" } else { "view" };
    json_response(StatusCode::OK, json!({"ok": true, "alias": alias, "owner": owner, "session": session,
        "level": received, "level_given": level_name(given)}))
}

async fn list_pairs(State(st): St) -> Response {
    let full_by_root = st.owner.share.store.full_sessions();
    let all: Vec<Value> = st.owner.share.pairs.all().into_iter().map(|r| {
        let full = full_by_root.get(&r.share_id).is_some_and(|s| s.contains(&r.local_session));
        json!({"local_session": r.local_session, "alias": r.alias, "owner": r.peer_owner, "session": r.peer_session,
            "address": r.peer_address, "token": r.peer_token, "level_given": level_name(if full { Kind::Share } else { Kind::Pair })})
    }).collect();
    json_response(StatusCode::OK, Value::Array(all))
}

fn no_pair() -> Refusal { fail(StatusCode::NOT_FOUND, "erro_par_inexistente", "este par externo não existe", json!({})) }

/// Remetente fora do alfabeto de `valid_session` fica sem `from`: o outro lado recusaria o recado
/// inteiro, e sem ele o cabeçalho cai na ponta do par.
fn with_from(mut body: Value, sender: &str) -> Value {
    if pairs::valid_session(sender) { body["from"] = json!(sender); }
    body
}

/// Recado de `sender` ao par de fora `target` (`alias::sessao`); também o caminho do MCP. Vale de
/// qualquer membro local do grupo para qualquer sessão de fora do grupo do par: `to` diz qual, e o
/// outro lado confere.
pub async fn send(app: &AppState, share: &ShareState, remote: &dyn Remote, sender: &str, target: &str, text: &str)
    -> Result<Value, Refusal> {
    let (alias, to) = target.split_once("::").unwrap_or((target, ""));
    if !pairs::valid_session(to) { return Err(no_pair()) }
    let mine = pairs_reaching(app, share, sender, alias).await?;
    let rec = mine.iter().find(|r| r.address() == target).or(mine.first()).cloned().ok_or_else(no_pair)?;
    let body = with_from(json!({"text": text, "session": rec.peer_session, "to": to}), sender);
    post_message(app, share, remote, &rec, &body).await
}

/// Aviso de grupo de `sender` à máquina de fora do par `rec`, um salto só: lá ele chega aos locais
/// do grupo e não volta a ser espalhado. `session` fica para o Hangar antigo, que ignora `group`.
pub async fn notify_outside(app: &AppState, share: &ShareState, remote: &dyn Remote, rec: &ExternalPair, sender: &str, text: &str, notice: &str)
    -> Result<(), Value> {
    let body = with_from(json!({"text": text, "group": true, "session": rec.peer_session, "notice": notice}), sender);
    post_message(app, share, remote, rec, &body).await.map(drop).map_err(|(_, detail)| detail)
}

async fn post_message(app: &AppState, share: &ShareState, remote: &dyn Remote, rec: &ExternalPair, body: &Value) -> Result<Value, Refusal> {
    if share.pairs.ambiguous(&rec.alias, &app.peers.ids()) {
        return Err(fail(StatusCode::CONFLICT, "erro_par_endereco_ambiguo",
            &format!("'{}' é ao mesmo tempo máquina tua e par externo", rec.alias), json!({})));
    }
    match remote.call(&rec.peer_address, Some(&rec.peer_token), reqwest::Method::POST, "/api/pair/message", Some(body)).await {
        Err(PeerError::Refused { status: 410, .. }) => {
            teardown(app, share, rec, true).await?;
            Err(fail(StatusCode::GONE, "erro_par_encerrado", "o par foi encerrado do outro lado", json!({})))
        }
        Err(e) => Err(remote_failure(&e, &rec.peer_address)),
        // Só as chaves que o app conhece, com valor primitivo: o dict do outro lado nunca passa adiante.
        Ok((_, resp)) => Ok(Value::Object(RESP_KEYS.iter().filter_map(|k| {
            resp.as_ref()?.get(*k).filter(|v| v.is_boolean() || v.is_number()).map(|v| ((*k).to_owned(), v.clone()))
        }).collect())),
    }
}

async fn external_send(State(st): St, bytes: Bytes) -> Response {
    let (v, _) = match fields(&bytes, &[("sender", true, None), ("target", true, None), ("text", true, None)], false) { Ok(v) => v, Err(r) => return r };
    match send(&st.app, &st.owner.share, &*st.remote, &v[0], &v[1], &v[2]).await {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(refusal) => respond(refusal),
    }
}

/// Fim do par externo de `local` com `address` quando a sessão sai do grupo: avisa o outro lado e
/// apaga registro e convite. `None` = não é par externo dela. `local` pode ser o nome saneado.
pub async fn end_pair(app: &AppState, share: &ShareState, remote: &dyn Remote, local: &str, address: &str) -> Option<Vec<Value>> {
    let rec = share.pairs.all().into_iter()
        .find(|r| r.address() == address && (r.local_session == local || file_stem(&r.local_session) == local))?;
    let failed = |code: &str, msg: String| json!({"sessao": address, "erro": {"code": code, "params": {"peer": address}, "msg": msg}});
    let mut errs = Vec::new();
    if share.pairs.ambiguous(&rec.alias, &app.peers.ids()) {
        // Só o aviso ao outro lado é pulado (o alias também é máquina tua); a limpeza local vale.
        errs.push(failed("erro_par_endereco_ambiguo", format!("'{}' é ao mesmo tempo máquina tua e par externo", rec.alias)));
    } else if let Err(e) = tell_leave(remote, &rec).await {
        let answered = matches!(e, PeerError::Refused { .. });
        if !matches!(e, PeerError::Refused { status: 410, .. }) {
            let text = format!("{}{}", if answered { REMOTE_LABEL } else { "" }, cut(&e.text(&rec.peer_address), 300));
            errs.push(failed("erro_peer_nao_avisado", text));
        }
    }
    if !unlink_all(app, address).await {
        errs.push(failed("erro_par_limpeza_falhou", format!("o par externo com {address} continua no grupo de alguma sessão daqui")));
    }
    if !drop_record(share, &rec.share_id) {
        // Convite que não foi revogado deixa o outro lado mandando recado: a saída não pode dizer que limpou.
        errs.push(failed("erro_par_limpeza_falhou",
            format!("o par externo com {address} não foi apagado por inteiro deste lado; o convite dele pode continuar valendo")));
    }
    Some(errs)
}

/// Tira `address` de todo grupo daqui que ainda o tenha: a ponta pode já ter saído do dela, e os
/// outros membros ficariam com um par que não existe mais. `false` = algum grupo ficou com ele.
async fn unlink_all(app: &AppState, address: &str) -> bool {
    let Some(groups) = &app.groups else { return true };
    let Ok(all) = groups.sidecars().await else { return false };
    let mut ok = true;
    // Um `external_unlink` já limpa o grupo inteiro: um por gid basta.
    let mut groups_done = std::collections::HashSet::new();
    for (stem, sidecar) in all.into_iter().filter(|(_, s)| s.peers.iter().any(|p| p == address)) {
        if !sidecar.gid.is_empty() && !groups_done.insert(sidecar.gid.clone()) { continue }
        let done = guarded("tirar o par do grupo", groups.external_unlink(&stem, address).await);
        if done { crate::groups::federation::outside_changed(app, &sidecar, address, false).await; }
        ok &= done;
    }
    ok
}

/// Sessão morta fora do app: ninguém mais desfaz o par dela. Se era a ponta, o par acaba para o
/// grupo inteiro (registro e convite saem na hora; o aviso ao outro lado segue solto); se era só
/// membro, perde o acesso de fora.
pub async fn session_died(app: Arc<AppState>, share: Arc<ShareState>, remote: Arc<dyn Remote>, name: &str) {
    for rec in share.pairs.all() {
        if rec.local_session != name && file_stem(&rec.local_session) != name {
            guarded("revogar o acesso", share.store.ungrant(&rec.share_id, name));
            continue;
        }
        drop_record(&share, &rec.share_id);
        unlink_all(&app, &rec.address()).await;
        let remote = remote.clone();
        tokio::spawn(async move {
            if let Err(e) = tell_leave(&*remote, &rec).await {
                tracing::warn!(code = "external_pair_remote_not_told", error = %e.text(&rec.peer_address), "par externo: outro lado não avisado da sessão morta");
            }
        });
    }
}

/// A saída do grupo vem primeiro e sem engolir a falha: registro e convite ficam, e o outro lado
/// tenta de novo com o token que ainda vale.
async fn teardown(app: &AppState, share: &ShareState, rec: &ExternalPair, notify: bool) -> Result<(), Refusal> {
    let Some(groups) = &app.groups else { return Err(groups_unavailable("groups_unavailable")) };
    let side = groups.link(&rec.local_session).await.ok().flatten();
    if let Err(e) = groups.external_unlink(&rec.local_session, &rec.address()).await {
        tracing::warn!(code = "external_pair_unlink_failed", session = %rec.local_session, error = %e, "par externo: o grupo não foi desfeito");
        return Err(groups_unavailable("groups_store_failed"));
    }
    if let Some(side) = side { crate::groups::federation::outside_changed(app, &side, &rec.address(), false).await; }
    drop_record(share, &rec.share_id);
    if notify {
        let text = format!("[painel: par externo encerrado] '{}' saiu do pareamento. Volte a operar independente.", rec.address());
        if let Err(e) = deliver_text(app, &rec.local_session, &text).await {
            tracing::warn!(code = "external_pair_end_notice_failed", session = %rec.local_session, error = %py_str(&e), "par externo: aviso de encerramento falhou");
        }
    }
    Ok(())
}

pub fn owner_pair_router(state: Arc<PairState>) -> Router {
    Router::new()
        .route("/api/sessions/{name}/pair-invite", post(pair_invite))
        .route("/api/sessions/{name}/pair-accept", post(pair_accept))
        .route("/api/sessions/{name}/pair-access", post(pair_access))
        .route("/api/external-pairs", get(list_pairs))
        .route("/api/external-pairs/send", post(external_send))
        .route("/api/external-pairs/{alias}", delete(undo_pair))
        .with_state(state)
}

/// As rotas do dono para quem chega pelo Connect; quem monta põe a checagem do segredo na frente.
pub fn private_pair_router(state: Arc<PairState>) -> Router {
    const BASE: &str = "/__hangar_server/share/owner";
    Router::new()
        .route(&format!("{BASE}/pair_invite/{{name}}"), any(pair_invite))
        .route(&format!("{BASE}/pair_accept/{{name}}"), any(pair_accept))
        .route(&format!("{BASE}/pairs_send"), any(external_send))
        .route(&format!("{BASE}/pairs_list"), any(list_pairs))
        .route(&format!("{BASE}/pair_access/{{name}}"), any(pair_access))
        .route(&format!("{BASE}/pairs_undo/{{alias}}"), any(undo_pair))
        .with_state(state)
}

// --- convidado -------------------------------------------------------------------------------

/// GET nunca gasta o código: prévia de link (WhatsApp) também faz GET.
async fn pair_page(State(st): St, Path(code): Path<String>) -> Response {
    let body = match st.owner.share.store.peek(&code, Kind::Pair) {
        Err(e) => format!("<h1>Convite indisponível</h1><p>{}.</p>", html_escape(reason(&e).2)),
        Ok(_) => format!("<h1>{} quer parear uma sessão com a tua</h1>\
            <p>No app desktop do Hangar, clique com o botão direito na sessão que vai \
            trabalhar junto e escolha <b>Parear por convite…</b>. Ou cole este link \
            na conversa da sessão e peça para ela aceitar.</p>\
            <button onclick=\"navigator.clipboard.writeText(location.href);this.textContent='Link copiado'\">Copiar link</button>",
            html_escape(&machine(&st.app))),
    };
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8"), (header::CACHE_CONTROL, "no-store"),
      (header::REFERRER_POLICY, "no-referrer")], page(&body)).into_response()
}

async fn pair_redeem(State(st): St, bytes: Bytes) -> Response {
    let spec = [("code", true, Some(64)), ("session", true, Some(64)), ("address", true, Some(200)), ("token", true, Some(200)), ("owner", true, Some(40))];
    let (v, ask) = match fields(&bytes, &spec, false) { Ok(v) => v, Err(r) => return r };
    let [code, session, address, token, owner] = &v[..] else { unreachable!() };
    let (app, share) = (&*st.app, &*st.owner.share);
    let address = pairs::normalize_address(address);
    let Some(address) = address.filter(|_| pairs::valid_token(token) && pairs::valid_session(session)) else {
        return detail(StatusCode::BAD_REQUEST, "erro_par_endereco_invalido", "endereço do par inválido", json!({}));
    };
    if !pairs::valid_owner(owner) {
        return detail(StatusCode::BAD_REQUEST, "erro_par_nome_invalido", "nome da máquina do par inválido", json!({}));
    }
    let invite = match share.store.peek(code, Kind::Pair) { Ok(s) => s, Err(e) => return respond(code_error(&e)) };
    let my_base = match share.public_guest_url().await {
        Ok(base) => base,
        Err(e) => {
            tracing::warn!(code = "external_pair_tunnel_unavailable", error = %e.fix, "par externo: túnel indisponível ao resgatar convite");
            return detail(StatusCode::SERVICE_UNAVAILABLE, "erro_sessao_indisponivel", "indisponível por instantes", json!({}));
        }
    };
    let name = invite.session;
    if share.pairs.paired_with(&address, session) { return repeated(); }
    // Antes de gastar o código: grupo que não aceita par de fora recusa sem queimar o convite.
    if let Err(r) = link_refusal(app, &name).await { return respond(r) }
    let alias = share.pairs.free_alias(owner, &address, &app.peers.ids(), &server_id(app));
    let peer = format!("{alias}::{session}");
    if let Err(r) = join_external(app, &name, &peer).await { return respond(r) }
    let undone = || fail(StatusCode::INTERNAL_SERVER_ERROR, "erro_pareamento_desfeito", "pareamento desfeito", json!({"avisos": ""}));
    let (redeemed, my_token) = match share.store.redeem(code, owner, None, Kind::Pair) {
        Ok(v) => v,
        Err(e) => {
            unlink_guarded(app, &name, &peer).await;
            return respond(match e {
                PeekError::Io(io) => {
                    tracing::warn!(code = "external_pair_redeem_failed", session = %name, error = %io, "par externo: resgate falhou");
                    undone()
                }
                other => code_error(&other),
            });
        }
    };
    let added = share.pairs.add(ExternalPair { share_id: redeemed.id.clone(), local_session: name.clone(), alias, peer_owner: owner.clone(),
        peer_session: session.clone(), peer_address: address, peer_token: token.clone(), created_at: share.store.now() });
    if let Err(e) = added {
        tracing::warn!(code = "external_pair_record_failed", session = %name, error = %e, "par externo: resgate desfeito");
        undo_local(app, share, &name, &peer, &redeemed.id).await;
        return respond(undone());
    }
    let given = clamp_for_old(ask.get("level"), invite.level.unwrap_or(Kind::Pair));
    if let Err(e) = grant_group(app, share, &redeemed.id, &name, given).await {
        tracing::warn!(code = "external_pair_grant_failed", session = %name, error = %e, "par externo: nível não gravado, resgate desfeito");
        undo_local(app, share, &name, &peer, &redeemed.id).await;
        return respond(undone());
    }
    if let Err(e) = notify_paired(app, &name, &peer, owner).await {
        tracing::warn!(code = "external_pair_notice_failed", session = %name, error = %e, "par externo: aviso falhou, resgate desfeito");
        undo_local(app, share, &name, &peer, &redeemed.id).await;
        // Rota aberta: o texto da falha (caminho, fila) não volta a quem chamou.
        return respond((StatusCode::BAD_GATEWAY, undone().1));
    }
    json_response(StatusCode::OK, json!({"session": name, "owner": my_owner(app),
        "address": my_base, "token": my_token, "level": level_name(given)}))
}

/// O registro de par do token que fala. Com várias raízes, `session` escolhe (a ponta ou um membro
/// do grupo com registro filho); sem ela, só vale a única.
fn pair_of(share: &ShareState, caller: Option<&Caller>, session: Option<&str>) -> Result<Share, Refusal> {
    let outside = || fail(StatusCode::FORBIDDEN, "erro_fora_do_convite", "fora do par", json!({}));
    let Some(Caller::Guest(grant)) = caller else { return Err(outside()) };
    let guest = share.store.lookup_token(&grant.token).ok_or_else(outside)?;
    let roots = guest.pair_shares();
    let picked: Vec<&Share> = match session {
        Some(s) if roots.len() > 1 => {
            let anchors: Vec<&Share> = roots.iter().copied().filter(|r| r.session == s).collect();
            if !anchors.is_empty() { anchors } else {
                roots.into_iter().filter(|r| guest.records.iter().any(|c|
                    c.parent_id.as_deref() == Some(r.id.as_str()) && c.session == s && c.revoked_at.is_none())).collect()
            }
        }
        _ => roots,
    };
    match picked[..] {
        [one] => Ok(one.clone()),
        [] => Err(outside()),
        _ => Err(fail(StatusCode::CONFLICT, "erro_par_sessao_ambigua", "esta chave tem mais de um par: diga a sessão", json!({}))),
    }
}

/// Aviso em entrega (`Pending`: quem chega depois espera o `rx` fechar) ou já entregue.
enum Seen {
    Pending { id: u64, rx: tokio::sync::watch::Receiver<()> },
    Done(std::time::Instant),
}

/// O mesmo aviso de grupo chega por cada par que a outra máquina tem conosco; só o primeiro
/// entregue entra.
// ponytail: mapa do processo podado pela janela da tempestade, como o `storm`.
fn seen_notices() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, Seen>> {
    use std::collections::HashMap;
    use std::sync::{LazyLock, Mutex};
    static SEEN: LazyLock<Mutex<HashMap<String, Seen>>> = LazyLock::new(Mutex::default);
    SEEN.lock().unwrap_or_else(|e| e.into_inner())
}

/// Chave do aviso entregue: o destino (máquina `me` e sessão) e o `notice`, nunca quem mandou. O
/// mesmo aviso vindo por duas máquinas do outro dono, ou pelo par e pela dona do grupo, entra uma vez.
pub(crate) fn notice_key(me: &str, notice: &str, session: &str) -> String { format!("{me}\n{notice}\n{session}") }

/// Reserva do aviso para uma entrega: `None` = já entregue dentro da janela da tempestade. Em
/// entrega por outra chegada: espera ela acabar; entregou = `None`, falhou = reserva aqui. Checar e
/// reservar sob a mesma trava impede que duas chegadas simultâneas entreguem. Quem chama limita a espera.
pub(crate) async fn notice_claim(key: String) -> Option<NoticeClaim> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    loop {
        let mut rx = {
            let mut seen = seen_notices();
            seen.retain(|_, s| !matches!(s, Seen::Done(t) if t.elapsed() >= STORM_WINDOW));
            match seen.get(&key) {
                Some(Seen::Done(_)) => return None,
                Some(Seen::Pending { rx, .. }) => rx.clone(),
                None => {
                    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let (tx, rx) = tokio::sync::watch::channel(());
                    seen.insert(key.clone(), Seen::Pending { id, rx });
                    return Some(NoticeClaim { key: Some(key), id, _tx: tx });
                }
            }
        };
        // Fecha quando a reserva é firmada ou solta (o `tx` cai junto); o laço relê o estado.
        let _ = rx.changed().await;
    }
}

/// Solta a reserva ao sair sem `delivered()` (falha, prazo estourado, future abandonado): o reenvio
/// entra. Só mexe na entrada se ela ainda for desta reserva.
pub(crate) struct NoticeClaim { key: Option<String>, id: u64, _tx: tokio::sync::watch::Sender<()> }

impl NoticeClaim {
    pub(crate) fn delivered(mut self) {
        let Some(k) = self.key.take() else { return };
        let mut seen = seen_notices();
        if matches!(seen.get(&k), Some(Seen::Pending { id, .. }) if *id == self.id) {
            seen.insert(k, Seen::Done(std::time::Instant::now()));
        }
    }
}

impl Drop for NoticeClaim {
    fn drop(&mut self) {
        let Some(k) = self.key.take() else { return };
        let mut seen = seen_notices();
        if matches!(seen.get(&k), Some(Seen::Pending { id, .. }) if *id == self.id) {
            seen.remove(&k);
        }
    }
}

async fn pair_message(State(st): St, caller: Option<Extension<Caller>>, bytes: Bytes) -> Response {
    let (v, map) = match fields(&bytes, &[("text", true, Some(MAX_MESSAGE))], false) { Ok(v) => v, Err(r) => return r };
    // `from` é só o nome da sessão de lá; a máquina sai do registro.
    let from = match map.get("from") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if pairs::valid_session(s) => Some(s.clone()),
        Some(_) => return detail(StatusCode::BAD_REQUEST, "erro_par_remetente_invalido", "remetente do recado inválido", json!({})),
    };
    let to = map.get("to").filter(|v| !v.is_null());
    let session = map.get("session").and_then(Value::as_str);
    let share = &st.owner.share;
    let pair = match pair_of(share, caller.as_deref(), session) { Ok(s) => s, Err(r) => return respond(r) };
    let Some(rec) = share.pairs.by_share(&pair.id) else {
        let mut r = detail(StatusCode::SERVICE_UNAVAILABLE, "erro_sessao_indisponivel", "par ainda abrindo", json!({}));
        r.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
        return r;
    };
    if storm(&format!("ext:{}", pair.id)) {
        return detail(StatusCode::TOO_MANY_REQUESTS, "erro_group_message_tempestade",
            &format!("mais de {STORM_MAX} avisos de grupo em {}s — parece loop; espere ou responda 1:1", STORM_WINDOW.as_secs()),
            json!({"max": STORM_MAX, "janela": STORM_WINDOW.as_secs()}));
    }
    let locals = match group_locals(&st.app, &pair.session).await {
        Ok(l) => l,
        Err(e) => {
            tracing::warn!(code = "external_pair_group_read_failed", session = %pair.session, error = %e, "par externo: grupo ilegível no recado");
            return respond(groups_unavailable("groups_store_failed"));
        }
    };
    // Máquina pelo registro (carimbado pelo token), sessão pelo `from` validado; nunca do texto.
    let who = format!("{}::{}", rec.alias, from.as_deref().unwrap_or(&rec.peer_session));
    let clean = pairs::sanitize_message(&v[0]);
    let text = format!("[de fora: {who}] {clean}");
    if map.get("group").and_then(Value::as_bool) == Some(true) {
        // Um salto na fronteira: aqui recebem os locais do grupo do par; nada volta a sair a outro par de fora.
        let mut last = (StatusCode::OK, json!({"ok": true}));
        let mut delivered = false;
        let notice = map.get("notice").and_then(Value::as_str).filter(|n| !n.is_empty() && n.len() <= 64);
        let me = server_id(&st.app);
        for session in &locals {
            let claim = match notice {
                Some(n) => match notice_claim(notice_key(&me, n, session)).await {
                    None => { delivered = true; continue; }
                    c => c,
                },
                None => None,
            };
            let (status, reply) = write_input(&st.app, session, &text, true).await;
            if status.is_success() {
                delivered = true;
                if let Some(c) = claim { c.delivered(); }
            } else {
                tracing::warn!(code = "external_pair_group_notice_failed", session = %session, status = status.as_u16(), "par externo: aviso de grupo de fora não entregue");
                last = (status, reply);
            }
        }
        // Grupo entre máquinas: a dona leva às outras máquinas do dono, nunca a outro par de fora.
        crate::groups::fed_notice::relay_outside(st.app.clone(), pair.session.clone(), who, clean, notice.unwrap_or_default().to_owned());
        return if delivered { json_response(StatusCode::OK, json!({"ok": true})) } else { json_response(last.0, last.1) };
    }
    // `to` = a sessão daqui que a resposta 1:1 nomeia; sem ele, a de `session` se for do grupo.
    let target = match to {
        Some(Value::String(t)) if locals.contains(t) => t.clone(),
        Some(_) => return respond(no_pair()),
        None => session.filter(|s| locals.iter().any(|l| l == s)).unwrap_or(&pair.session).to_owned(),
    };
    let (status, reply) = write_input(&st.app, &target, &text, true).await;
    json_response(status, reply)
}

async fn pair_leave(State(st): St, caller: Option<Extension<Caller>>, RawQuery(query): RawQuery) -> Response {
    let session = form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(k, _)| k == "session").map(|(_, v)| v.into_owned());
    let share = &st.owner.share;
    let pair = match pair_of(share, caller.as_deref(), session.as_deref()) { Ok(s) => s, Err(r) => return respond(r) };
    match share.pairs.by_share(&pair.id) {
        Some(rec) => if let Err(r) = teardown(&st.app, share, &rec, true).await { return respond(r) },
        None => if let Err(e) = share.store.revoke(&pair.id) { return io_failed(e) },
    }
    json_response(StatusCode::OK, json!({"ok": true}))
}

pub fn guest_pair_router(state: Arc<PairState>) -> Router {
    Router::new()
        .route("/par/{code}", get(pair_page))
        .route("/api/pair/redeem", post(pair_redeem))
        .route("/api/pair/message", post(pair_message))
        .route("/api/pair", delete(pair_leave))
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Entrada trocada por outra reserva (poda): soltar ou firmar a antiga não mexe na nova.
    #[tokio::test]
    async fn old_claim_never_touches_a_newer_one() {
        let k = "teste\nold-claim\ns".to_owned();
        let still_new = || matches!(seen_notices().get(&k), Some(Seen::Pending { .. }));
        for firm in [false, true] {
            let old = notice_claim(k.clone()).await.unwrap();
            seen_notices().remove(&k); // a poda
            let new = notice_claim(k.clone()).await.unwrap();
            if firm { old.delivered() } else { drop(old) }
            assert!(still_new(), "firm={firm}: a reserva nova segue valendo");
            drop(new);
            assert!(seen_notices().get(&k).is_none());
        }
    }
}
