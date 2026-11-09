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

/// Prazo de cada sessão na entrega adiada de quem entrou pela porta do Connect, abaixo do prazo da
/// dona (`DELIVER_WAIT`, 5 s), como o das sessões do Rust.
const DEFERRED_WAIT: Duration = Duration::from_secs(3);

/// Pedido repassado pelo Python: o recado a sessão sem dono no Rust sai depois da rota (`deliver_deferred`).
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

/// Rota entre máquinas que o Python repassou da 8768: o recado a sessão sem dono no Rust sai depois da
/// rota, cada sessão em no máximo `DEFERRED_WAIT`, e quem não recebeu entra na resposta.
pub(crate) async fn deferring(st: Arc<AppState>, handler: impl std::future::Future<Output = Response>) -> Response {
    let ctx = BridgeCtx::new();
    let resp = CTX.scope(ctx.clone(), handler).await;
    let deliveries = ctx.take_deliveries();
    if deliveries.is_empty() {
        return resp;
    }
    let failed = deliver_deferred(&st, deliveries, Some(DEFERRED_WAIT)).await;
    if failed.is_empty() {
        return resp;
    }
    let (mut parts, body) = resp.into_parts();
    let Ok(bytes) = to_bytes(body, MAX_REPLY).await else {
        report_failed(&st, &mut Value::Null, failed, "grupos.entrega_adiada_falhou", &[]);
        return crate::session_write::json_response(StatusCode::INTERNAL_SERVER_ERROR, crate::session_write::detail_body(
            "erro_grupo_indisponivel", "os grupos estão indisponíveis agora", json!({"detalhe": "groups_reply_unreadable"})));
    };
    let mut value = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
    report_failed(&st, &mut value, failed, "grupos.entrega_adiada_falhou", &["failed", "pulados"]);
    if !value.is_object() {
        return Response::from_parts(parts, Body::from(bytes));
    }
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(value.to_string()))
}

/// O recado adiado, em processo: cada sessão na ordem em que chegou, as sessões ao mesmo tempo. A
/// entrega roda em tarefa própria; o prazo larga só a espera, e a sessão que não terminou nele conta
/// como falha (`groups_notice_timeout`). Devolve `{sessao, erro}` de quem não recebeu.
pub(crate) async fn deliver_deferred(st: &Arc<AppState>, deliveries: Vec<Value>, wait: Option<Duration>) -> Vec<Value> {
    let mut by_session: Vec<(String, Vec<(String, bool)>)> = Vec::new();
    for d in deliveries {
        let (Some(name), Some(text)) = (d["name"].as_str(), d["text"].as_str()) else { continue };
        let item = (text.to_owned(), d["steer"] == true);
        match by_session.iter_mut().find(|(n, _)| n == name) {
            Some((_, items)) => items.push(item),
            None => by_session.push((name.to_owned(), vec![item])),
        }
    }
    let runs = by_session.into_iter().map(|(name, items)| {
        let (st, owned) = (st.clone(), name.clone());
        let task = tokio::spawn(async move {
            let mut failed = Vec::new();
            for (text, steer) in items {
                let (status, reply) = crate::session_write::input::write_input(&st, &owned, &text, steer).await;
                if status.is_success() { continue; }
                let error = reply.get("detail").filter(|d| !d.is_null()).cloned().unwrap_or_else(|| json!({"code": "erro_envio_falhou_desconhecida", "params": {}, "msg": "falha desconhecida no envio"}));
                failed.push(json!({"sessao": owned, "erro": error}));
            }
            failed
        });
        async move {
            let joined = match wait { Some(wait) => tokio::time::timeout(wait, task).await.ok(), None => Some(task.await) };
            match joined {
                Some(Ok(failed)) => failed,
                Some(Err(_)) => vec![json!({"sessao": name, "erro": {"code": "erro_envio_falhou_desconhecida", "params": {}, "msg": "falha desconhecida no envio"}})],
                None => vec![json!({"sessao": name, "erro": {"code": "erro_envio_falhou", "params": {"erro": "groups_notice_timeout"}, "msg": "não deu pra enviar: groups_notice_timeout"}})],
            }
        }
    });
    futures_util::future::join_all(runs).await.into_iter().flatten().collect()
}

/// Quem não recebeu o recado adiado: na primeira lista de `lists` que o corpo tiver, qualificado como as
/// falhas do Rust, ou no `warning`. Sem corpo onde caber, só o diário.
fn report_failed(st: &AppState, body: &mut Value, failed: Vec<Value>, event: &'static str, lists: &[&str]) {
    if failed.is_empty() { return; }
    let names: Vec<String> = failed.iter().map(|f| f["sessao"].as_str().unwrap_or_default().to_owned()).collect();
    let fields = json!({"falhas": failed.len(), "sessoes": names.join(",")});
    st.diag.report_with(event, "", "deferred_not_delivered", "recado adiado da ponte de grupos não entregue", fields.clone());
    let me = st.groups.as_ref().map_or_else(|| st.server_id.clone(), |groups| groups.server_id().to_owned());
    if let Some(key) = lists.iter().find(|key| body.get(**key).is_some_and(Value::is_array)) {
        if let Some(list) = body[*key].as_array_mut() { list.extend(names.iter().map(|name| json!(format!("{me}::{name}")))); }
    } else if body.is_object() && body.get("warning").is_none_or(Value::is_null) {
        body["warning"] = crate::session_write::detail_body("erro_pareamento_aviso_parcial", &format!("aviso falhou em: {}", names.join("; ")),
            json!({"avisos": failed}))["detail"].clone();
    } else if let Some(warning) = body.get_mut("warning").filter(|warning| warning.is_object()) {
        if !warning["params"].is_object() { warning["params"] = json!({}); }
        if !warning["params"]["avisos"].is_array() { warning["params"]["avisos"] = json!([]); }
        if let Some(avisos) = warning["params"]["avisos"].as_array_mut() { avisos.extend(failed); }
    } else {
        st.diag.report_with("grupos.falha_sem_corpo", "", "deferred_not_delivered", "recado adiado sem corpo onde caber", fields);
    }
}

impl BridgeCtx {
    /// Recado a sessão sem dono no Rust dentro de pedido da ponte: sai depois da rota (`deliver_deferred`).
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
        GroupError::Orq(PromoteError::ConflictWithRemaining { message, .. }) => fail("erro_orq_arquivo_mudou", message),
        GroupError::Orq(PromoteError::IdentityUnavailable(text)) => fail("erro_orq_promocao_sem_identidade", text),
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
                .scope(ctx.clone(), route(st.clone(), &method, &name, &path, body))
                .await?;
            let failed = deliver_deferred(&st, ctx.take_deliveries(), None).await;
            report_failed(&st, &mut out["body"], failed, "grupos.entrega_pela_ponte", &[]);
            out["post"] = json!([]);
            Ok(out)
        }
        Operation::Leave { name, phases: _ } => {
            // A saída de grupo entre máquinas avisa quem ficou sozinho; sessão sem dono recebe depois.
            let ctx = BridgeCtx::new();
            let (ex, mut warnings) = CTX
                .scope(ctx.clone(), async {
                    let (ex, tell) = super::exit::leave(&st, &groups, &name, None)
                        .await
                        .map_err(group_failed)?;
                    Ok::<_, Failed>((ex, notify_exit(&st, &groups, &name, &tell).await))
                })
                .await?;
            warnings.extend(deliver_deferred(&st, ctx.take_deliveries(), None).await);
            Ok(json!({"ex_peers": ex, "warnings": warnings}))
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
                        let _lock = crate::orchestration::context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
                Err(crate::orchestration::context::AssociateError::Invalid(msg)) => (
                    409,
                    json!({"detail": {"code": "erro_orq_celula_invalida", "params": {}, "msg": msg}}),
                ),
                Err(crate::orchestration::context::AssociateError::Changed(msg)) => (
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
