//! Ponte privada dos grupos (`/__hangar_server/groups`): no modo `rust`/`pending` só o Rust grava
//! `.hangar-pair`. Registry e as rotas que chegam pelas portas do Python pedem
//! aqui, com o envelope das outras pontes (`{op, args}` → `{ok, result}` | `{ok: false, error}`).
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::exit::{notify_exit, segment};
use super::local::JoinRefusal;
use super::routes::Bridged;
use super::service::{GroupError, GroupService, PromoteError};
use crate::routes::AppState;

const MAX_BODY: usize = 1 << 20;
const MAX_REPLY: usize = 4 << 20;

#[derive(Deserialize)]
#[serde(tag = "op", content = "args")]
enum Operation {
    /// A ponte conserva só método, destino e corpo; o servidor resolve os fatos.
    #[serde(rename = "group.route")]
    Route {
        method: String,
        name: String,
        route: String,
        #[serde(default)]
        body: Value,
        #[serde(default)]
        facts: Value,
    },
    /// Fechamento que não passou pelo `delete` do Rust (Connect, convidado).
    #[serde(rename = "group.leave")]
    Leave {
        name: String,
        #[serde(default)]
        phases: Value,
    },
    #[serde(rename = "group.orq_associate")]
    OrqAssociate {
        name: String,
        gid: String,
        mtime: f64,
        #[serde(default)]
        facts: Value,
    },
}

/// Pedido repassado pelo Python: só a entrega a provedores externos é adiada.
pub(crate) struct BridgeCtx {
    deliveries: std::sync::Mutex<Vec<Value>>,
}

tokio::task_local! { static CTX: Arc<BridgeCtx>; }

/// O pedido da ponte em curso nesta tarefa, se há um.
pub(crate) fn current() -> Option<Arc<BridgeCtx>> {
    CTX.try_with(Arc::clone).ok()
}

impl BridgeCtx {
    fn new() -> Arc<BridgeCtx> {
        Arc::new(BridgeCtx {
            deliveries: Default::default(),
        })
    }
    fn take_deliveries(&self) -> Vec<Value> {
        std::mem::take(&mut *self.deliveries.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// Rota entre máquinas que o Python repassou da 8768: o recado a sessão que só ele atende volta no
/// corpo (`hangar_deliveries`), para ele entregar depois de responder.
pub(crate) async fn deferring(handler: impl std::future::Future<Output = Response>) -> Response {
    let ctx = BridgeCtx::new();
    let resp = CTX.scope(ctx.clone(), handler).await;
    let deliveries = ctx.take_deliveries();
    if deliveries.is_empty() {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let mut value = match to_bytes(body, MAX_REPLY)
        .await
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    {
        Some(v) if v.is_object() => v,
        _ => {
            // Sem corpo para levar as entregas: elas se perderiam caladas.
            tracing::error!(
                code = "groups_deferred_lost",
                count = deliveries.len(),
                "groups: recado adiado sem corpo de resposta para voltar ao Python"
            );
            return crate::session_write::json_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                crate::session_write::detail_body(
                    "erro_grupo_indisponivel",
                    "os grupos estão indisponíveis agora",
                    json!({"detalhe": "groups_deferred_lost"}),
                ),
            );
        }
    };
    value["hangar_deliveries"] = Value::Array(deliveries);
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(value.to_string()))
}

impl BridgeCtx {
    /// Recado a sessão que só o Python atende: ele entrega depois que a ponte responder.
    pub(crate) fn defer_delivery(&self, name: &str, text: &str, steer: bool) {
        self.deliveries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(json!({"name": name, "text": text, "steer": steer}));
    }
}

pub(crate) struct Failed {
    pub(crate) code: String,
    detail: String,
}

fn fail(code: &str, detail: impl Into<String>) -> Failed {
    Failed {
        code: code.to_owned(),
        detail: detail.into(),
    }
}

/// Recusa do plano vira o código que o Python já traduz; disco é `groups_store_failed`.
pub(crate) fn group_failed(e: GroupError) -> Failed {
    match e {
        GroupError::Refused(JoinRefusal::Mix) => {
            fail("erro_pareamento_mistura_cross", super::routes::MIX_MSG)
        }
        GroupError::Refused(JoinRefusal::OrqOutside) => fail(
            super::routes::ORQ_OUTSIDE_CODE,
            super::routes::ORQ_OUTSIDE_MSG,
        ),
        GroupError::Refused(JoinRefusal::TaskConflict { existing }) => {
            fail("erro_pareamento_tarefa_existente", existing)
        }
        GroupError::Refused(JoinRefusal::Orq) => fail(
            super::routes::ORQ_ARBITER_CODE,
            super::routes::ORQ_ARBITER_MSG,
        ),
        GroupError::Orq(PromoteError::Conflict(text)) => fail("erro_orq_arquivo_mudou", text),
        GroupError::Orq(PromoteError::Unavailable(code)) => fail("erro_grupo_indisponivel", code),
        GroupError::Store(e) => fail("groups_store_failed", e.to_string()),
    }
}

/// `{status, body}` da rota de grupo, chamada como se viesse do dono.
pub(crate) async fn route(
    st: Arc<AppState>,
    method: &str,
    name: &str,
    route: &str,
    body: Value,
) -> Result<Value, Failed> {
    type Handler = fn(
        State<Arc<AppState>>,
        ConnectInfo<SocketAddr>,
        Request,
    ) -> futures_util::future::BoxFuture<'static, Response>;
    let (path, handler): (&str, Handler) = match (method, route) {
        ("POST", "pair") => ("pair", |s, c, r| Box::pin(super::routes::pair(s, c, r))),
        ("DELETE", "pair") => ("pair", |s, c, r| Box::pin(super::routes::unpair(s, c, r))),
        ("POST", "group-message") => ("group-message", |s, c, r| {
            Box::pin(super::routes::group_message(s, c, r))
        }),
        ("GET", "contract") => ("pair/contract", |s, c, r| {
            Box::pin(super::routes::contract(s, c, r))
        }),
        ("PUT", "contract") => ("pair/contract", |s, c, r| {
            Box::pin(super::fed_notice::session_contract_put(s, c, r))
        }),
        ("POST", "pair-remote") => ("pair-remote", |s, c, r| {
            Box::pin(super::legacy::pair_remote(s, c, r))
        }),
        ("POST", "unpair-remote") => ("unpair-remote", |s, c, r| {
            Box::pin(super::legacy::unpair_remote(s, c, r))
        }),
        _ => {
            return Err(fail(
                "groups_bridge_invalid_request",
                "rota de grupo desconhecida",
            ));
        }
    };
    let method = Method::from_bytes(method.as_bytes())
        .map_err(|_| fail("groups_bridge_invalid_request", "método"))?;
    let payload = if body.is_null() {
        Body::empty()
    } else {
        Body::from(body.to_string())
    };
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/api/sessions/{}/{path}", segment(name)))
        .header(header::CONTENT_TYPE, "application/json")
        .body(payload)
        .map_err(|_| fail("groups_bridge_invalid_request", "nome"))?;
    req.extensions_mut().insert(Bridged);
    let resp = handler(
        State(st),
        ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 0))),
        req,
    )
    .await;
    let status = resp.status().as_u16();
    let bytes = to_bytes(resp.into_body(), MAX_REPLY)
        .await
        .map_err(|_| fail("groups_bridge_reply", "corpo da rota"))?;
    let body = serde_json::from_slice(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    Ok(json!({"status": status, "body": body}))
}

async fn execute(
    st: Arc<AppState>,
    groups: Arc<GroupService>,
    op: Operation,
) -> Result<Value, Failed> {
    match op {
        Operation::Route {
            method,
            name,
            route: path,
            body,
            facts: _,
        } => {
            let ctx = BridgeCtx::new();
            let mut out = CTX
                .scope(ctx.clone(), route(st, &method, &name, &path, body))
                .await?;
            out["deliveries"] = Value::Array(ctx.take_deliveries());
            out["post"] = json!([]);
            Ok(out)
        }
        Operation::Leave { name, phases: _ } => {
            // A saída de grupo entre máquinas avisa quem ficou sozinho: Pi/omp/Kimi só o Python atende.
            let ctx = BridgeCtx::new();
            let (ex, warnings) = CTX
                .scope(ctx.clone(), async {
                    let (ex, tell) = super::exit::leave(&st, &groups, &name, None)
                        .await
                        .map_err(group_failed)?;
                    Ok::<_, Failed>((ex, notify_exit(&st, &groups, &name, &tell).await))
                })
                .await?;
            Ok(json!({"ex_peers": ex, "warnings": warnings, "deliveries": ctx.take_deliveries()}))
        }
        Operation::OrqAssociate {
            name,
            gid,
            mtime,
            facts: _,
        } => {
            // Sob o lock: um join ou uma saída concorrente não vê o time meio associado.
            let done = groups
                .locked(async {
                    let root = groups.pair_root().to_path_buf();
                    let home = st.accounts.env.home.clone();
                    let mux = st.list.env().capture_program.to_string_lossy().into_owned();
                    tokio::task::spawn_blocking(move || {
                        let _lock = crate::orchestration::context::WRITE_LOCK.lock().unwrap();
                        crate::orchestration::context::associate(
                            &home, &root, &name, &gid, mtime, &mux,
                        )
                    })
                    .await
                    .map_err(|e| {
                        GroupError::Store(super::store::StoreError::Io(std::io::Error::other(e)))
                    })
                })
                .await
                .map_err(group_failed)?;
            let (status, body) = match done {
                Ok(body) => (200, body),
                Err(msg) if !msg.contains("mudou desde") => (
                    409,
                    json!({"detail": {"code": "erro_orq_celula_invalida", "params": {}, "msg": msg}}),
                ),
                Err(msg) => (
                    409,
                    json!({"detail": {"code": "erro_orq_arquivo_mudou", "params": {}, "msg": msg}}),
                ),
            };
            Ok(json!({"status": status, "body": body}))
        }
    }
}

fn reply(value: Value) -> Response {
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/json")],
        value.to_string(),
    )
        .into_response()
}

pub async fn private(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let refused = |code: &'static str| {
        if crate::warn_limit::allow(None, code) {
            tracing::warn!(code, "ponte de grupos recusou o pedido");
        }
        StatusCode::BAD_REQUEST.into_response()
    };
    let Ok(Ok(bytes)) =
        tokio::time::timeout(Duration::from_secs(6), to_bytes(req.into_body(), MAX_BODY)).await
    else {
        return refused("groups_bridge_body");
    };
    // Operação desconhecida aqui costuma ser Python e Rust de versões diferentes.
    let Ok(op) = serde_json::from_slice::<Operation>(&bytes) else {
        return refused("groups_bridge_invalid_request");
    };
    let Some(groups) = st.groups.clone() else {
        return reply(
            json!({"ok": false, "error": {"code": "groups_bridge_off", "detail": "sem pastas de grupo"}}),
        );
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
