//! Ponte privada dos grupos (`/__hangar_server/groups`): no modo `rust`/`pending` só o Rust grava
//! `.hangar-pair`. Registry, par externo, MCP e as rotas que chegam pelas portas do Python pedem
//! aqui, com o envelope das outras pontes (`{op, args}` → `{ok, result}` | `{ok: false, error}`).
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::exit::{leave_and_notify, segment};
use super::local::JoinRefusal;
use super::orq::PythonOrq;
use super::routes::Bridged;
use super::service::{GroupError, GroupService, PromoteError};
use crate::routes::AppState;

const MAX_BODY: usize = 1 << 20;
const MAX_REPLY: usize = 4 << 20;

#[derive(Deserialize)]
#[serde(tag = "op", content = "args")]
enum Operation {
    #[serde(rename = "group.route")]
    Route { method: String, name: String, route: String, #[serde(default)] body: Value },
    #[serde(rename = "group.leave")]
    Leave { name: String },
    #[serde(rename = "group.rename")]
    Rename { old: String, new: String },
    #[serde(rename = "group.external_link")]
    ExternalLink { local: String, address: String, #[serde(default)] harness: BTreeMap<String, String> },
    #[serde(rename = "group.external_unlink")]
    ExternalUnlink { local: String, address: String },
    #[serde(rename = "group.orq_associate")]
    OrqAssociate { name: String, gid: String, mtime: f64 },
}

struct Failed { code: String, detail: String }

fn fail(code: &str, detail: impl Into<String>) -> Failed { Failed { code: code.to_owned(), detail: detail.into() } }

/// Recusa do plano vira o código que o Python já traduz; disco é `groups_store_failed`.
fn group_failed(e: GroupError) -> Failed {
    match e {
        GroupError::Refused(JoinRefusal::Mix | JoinRefusal::AlreadyGrouped) => fail("erro_pareamento_mistura_cross", super::routes::MIX_MSG),
        GroupError::Refused(JoinRefusal::TaskConflict { existing }) => fail("erro_pareamento_tarefa_existente", existing),
        GroupError::Orq(PromoteError::Conflict(text)) => fail("erro_orq_arquivo_mudou", text),
        GroupError::Orq(PromoteError::Unavailable(code)) => fail("erro_grupo_indisponivel", code),
        GroupError::Store(e) => fail("groups_store_failed", e.to_string()),
    }
}

/// `{status, body}` da rota de grupo, chamada como se viesse do dono.
async fn route(st: Arc<AppState>, method: &str, name: &str, route: &str, body: Value) -> Result<Value, Failed> {
    type Handler = fn(State<Arc<AppState>>, ConnectInfo<SocketAddr>, Request) -> futures_util::future::BoxFuture<'static, Response>;
    let (path, handler): (&str, Handler) = match (method, route) {
        ("POST", "pair") => ("pair", |s, c, r| Box::pin(super::routes::pair(s, c, r))),
        ("DELETE", "pair") => ("pair", |s, c, r| Box::pin(super::routes::unpair(s, c, r))),
        ("POST", "group-message") => ("group-message", |s, c, r| Box::pin(super::routes::group_message(s, c, r))),
        ("GET", "contract") => ("pair/contract", |s, c, r| Box::pin(super::routes::contract(s, c, r))),
        ("POST", "pair-remote") => ("pair-remote", |s, c, r| Box::pin(super::legacy::pair_remote(s, c, r))),
        ("POST", "unpair-remote") => ("unpair-remote", |s, c, r| Box::pin(super::legacy::unpair_remote(s, c, r))),
        _ => return Err(fail("groups_bridge_invalid_request", "rota de grupo desconhecida")),
    };
    let method = Method::from_bytes(method.as_bytes()).map_err(|_| fail("groups_bridge_invalid_request", "método"))?;
    let payload = if body.is_null() { Body::empty() } else { Body::from(body.to_string()) };
    let mut req = Request::builder().method(method).uri(format!("/api/sessions/{}/{path}", segment(name)))
        .header(header::CONTENT_TYPE, "application/json").body(payload)
        .map_err(|_| fail("groups_bridge_invalid_request", "nome"))?;
    req.extensions_mut().insert(Bridged);
    let resp = handler(State(st), ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))), req).await;
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), MAX_REPLY).await.map_err(|_| fail("groups_bridge_reply", "corpo da rota"))?;
    let body = serde_json::from_slice(&bytes).unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    Ok(json!({"status": status, "body": body}))
}

async fn execute(st: Arc<AppState>, groups: Arc<GroupService>, op: Operation) -> Result<Value, Failed> {
    match op {
        Operation::Route { method, name, route: path, body } => route(st, &method, &name, &path, body).await,
        Operation::Leave { name } => {
            let (ex, warnings) = leave_and_notify(&st, &groups, &name).await.map_err(group_failed)?;
            Ok(json!({"ex_peers": ex, "warnings": warnings}))
        }
        Operation::Rename { old, new } => groups.rename(&old, &new).await.map(|()| json!({})).map_err(group_failed),
        Operation::ExternalLink { local, address, harness } =>
            groups.external_link(&local, &address, harness).await.map(|gid| json!({"gid": gid})).map_err(group_failed),
        Operation::ExternalUnlink { local, address } =>
            groups.external_unlink(&local, &address).await.map(|()| json!({})).map_err(group_failed),
        Operation::OrqAssociate { name, gid, mtime } => {
            // Sob o lock: um join ou uma saída concorrente não vê o time meio associado.
            let python = PythonOrq::from_state(&st);
            let asked = python.post("orq/associate", json!({"name": name, "gid": gid, "mtime": mtime}));
            let (status, body) = groups.locked(asked).await.map_err(|code| fail(&code, "associação do orq"))?;
            Ok(json!({"status": status, "body": body}))
        }
    }
}

fn reply(value: Value) -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

pub async fn private(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let refused = |code: &'static str| {
        if crate::warn_limit::allow(None, code) {
            tracing::warn!(code, "ponte de grupos recusou o pedido");
        }
        StatusCode::BAD_REQUEST.into_response()
    };
    let Ok(Ok(bytes)) = tokio::time::timeout(Duration::from_secs(6), to_bytes(req.into_body(), MAX_BODY)).await else {
        return refused("groups_bridge_body");
    };
    // Operação desconhecida aqui costuma ser Python e Rust de versões diferentes.
    let Ok(op) = serde_json::from_slice::<Operation>(&bytes) else { return refused("groups_bridge_invalid_request") };
    let Some(groups) = st.groups.clone() else {
        return reply(json!({"ok": false, "error": {"code": "groups_bridge_off", "detail": "sem pastas de grupo"}}));
    };
    match execute(st, groups, op).await {
        Ok(result) => reply(json!({"ok": true, "result": result})),
        Err(e) => {
            if crate::warn_limit::allow(None, "groups_bridge_failed") {
                tracing::warn!(code = %e.code, "ponte de grupos falhou");
            }
            reply(json!({"ok": false, "error": {"code": e.code, "detail": e.detail}}))
        }
    }
}
