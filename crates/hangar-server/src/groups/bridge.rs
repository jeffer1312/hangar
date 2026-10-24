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
use super::service::{BoxFuture, GroupError, GroupService, OrqFacts, OrqPhase, PromoteError};
use crate::routes::AppState;

const MAX_BODY: usize = 1 << 20;
const MAX_REPLY: usize = 4 << 20;

#[derive(Deserialize)]
#[serde(tag = "op", content = "args")]
enum Operation {
    /// `facts`: o que o Python sabe da orquestração, lido antes de chamar (I-5A).
    #[serde(rename = "group.route")]
    Route { method: String, name: String, route: String, #[serde(default)] body: Value, facts: RouteFacts },
    /// Fechamento que não passou pelo `delete` do Rust (Connect, convidado): a fase do `orq` de cada
    /// gid vem no pedido, lida pelo Python antes de chamar.
    #[serde(rename = "group.leave")]
    Leave { name: String, phases: std::collections::HashMap<String, Value> },
    #[serde(rename = "group.orq_associate")]
    OrqAssociate { name: String, gid: String, mtime: f64, facts: TeamFacts },
}

/// Fatos do `orq` que o Python manda com a rota: os nomes que são linha de orquestrador (todas as
/// execuções ativas) e a fase de cada gid envolvido.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RouteFacts { orchestrators: Vec<String>, #[serde(default)] phases: std::collections::HashMap<String, Value> }

/// Pedido que chegou pela ponte do Python. Dentro dele o Rust não chama o Python: os fatos do `orq`
/// vieram no corpo, e o que só o Python faz (entregar a sessão que ele atende, promover o time) volta
/// na resposta para ele fazer depois, num sentido.
pub(crate) struct BridgeCtx {
    /// Sem fatos (repasse da 8768), o `orq` ainda é perguntado ao Python; só as entregas esperam.
    facts: bool,
    orchestrators: std::collections::HashSet<String>,
    phases: std::collections::HashMap<String, Value>,
    deliveries: std::sync::Mutex<Vec<Value>>,
    post: std::sync::Mutex<Vec<Value>>,
}

tokio::task_local! { static CTX: Arc<BridgeCtx>; }

/// O pedido da ponte em curso nesta tarefa, se há um.
pub(crate) fn current() -> Option<Arc<BridgeCtx>> { CTX.try_with(Arc::clone).ok() }

/// O pedido da ponte em curso que trouxe os fatos do `orq`.
pub(crate) fn facts() -> Option<Arc<BridgeCtx>> { current().filter(|c| c.facts) }

impl BridgeCtx {
    fn new(facts: bool, orchestrators: Vec<String>, phases: std::collections::HashMap<String, Value>) -> Arc<BridgeCtx> {
        Arc::new(BridgeCtx { facts, orchestrators: orchestrators.into_iter().collect(), phases, deliveries: Default::default(), post: Default::default() })
    }

    fn take_deliveries(&self) -> Vec<Value> { std::mem::take(&mut *self.deliveries.lock().unwrap_or_else(|e| e.into_inner())) }
}

/// Rota entre máquinas que o Python repassou da 8768: o recado a sessão que só ele atende volta no
/// corpo (`hangar_deliveries`), para ele entregar depois de responder.
pub(crate) async fn deferring(handler: impl std::future::Future<Output = Response>) -> Response {
    let ctx = BridgeCtx::new(false, Vec::new(), Default::default());
    let resp = CTX.scope(ctx.clone(), handler).await;
    let deliveries = ctx.take_deliveries();
    if deliveries.is_empty() { return resp; }
    let (mut parts, body) = resp.into_parts();
    let mut value = match to_bytes(body, MAX_REPLY).await.ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) {
        Some(v) if v.is_object() => v,
        _ => {
            // Sem corpo para levar as entregas: elas se perderiam caladas.
            tracing::error!(code = "groups_deferred_lost", count = deliveries.len(), "groups: recado adiado sem corpo de resposta para voltar ao Python");
            return crate::session_write::json_response(StatusCode::INTERNAL_SERVER_ERROR,
                crate::session_write::detail_body("erro_grupo_indisponivel", "os grupos estão indisponíveis agora", json!({"detalhe": "groups_deferred_lost"})));
        }
    };
    value["hangar_deliveries"] = Value::Array(deliveries);
    parts.headers.remove(header::CONTENT_LENGTH);
    Response::from_parts(parts, Body::from(value.to_string()))
}

impl BridgeCtx {
    pub(crate) fn orchestrators(&self, names: &[String]) -> Vec<String> {
        names.iter().filter(|n| self.orchestrators.contains(*n)).cloned().collect()
    }
    pub(crate) fn phase(&self, gid: &str) -> OrqPhase { super::orq::phase_of(self.phases.get(gid)) }
    /// Recado a sessão que só o Python atende: ele entrega depois que a ponte responder.
    pub(crate) fn defer_delivery(&self, name: &str, text: &str, steer: bool) {
        self.deliveries.lock().unwrap_or_else(|e| e.into_inner()).push(json!({"name": name, "text": text, "steer": steer}));
    }
    /// A promoção do time grava arquivos do `orq`, que só o Python escreve.
    pub(crate) fn defer_promote(&self, name: &str, gid: &str) {
        self.post.lock().unwrap_or_else(|e| e.into_inner()).push(json!({"op": "promote", "name": name, "gid": gid}));
    }
}

/// Fases que o Python mandou; gid sem fase = `Unknown`, que mantém o grupo vivo.
struct KnownPhases(std::collections::HashMap<String, Value>);

impl OrqFacts for KnownPhases {
    fn phase<'a>(&'a self, gid: &'a str) -> BoxFuture<'a, OrqPhase> {
        Box::pin(async move { super::orq::phase_of(self.0.get(gid)) })
    }

    /// A saída nunca promove.
    fn promote<'a>(&'a self, _name: &'a str, _gid: &'a str) -> BoxFuture<'a, Result<(), PromoteError>> {
        Box::pin(async { Err(PromoteError::Unavailable("groups_bridge_no_promote".into())) })
    }
}

pub(crate) struct Failed { pub(crate) code: String, detail: String }

fn fail(code: &str, detail: impl Into<String>) -> Failed { Failed { code: code.to_owned(), detail: detail.into() } }

/// Recusa do plano vira o código que o Python já traduz; disco é `groups_store_failed`.
pub(crate) fn group_failed(e: GroupError) -> Failed {
    match e {
        GroupError::Refused(JoinRefusal::Mix) => fail("erro_pareamento_mistura_cross", super::routes::MIX_MSG),
        GroupError::Refused(JoinRefusal::OrqOutside) => fail(super::routes::ORQ_OUTSIDE_CODE, super::routes::ORQ_OUTSIDE_MSG),
        GroupError::Refused(JoinRefusal::TaskConflict { existing }) => fail("erro_pareamento_tarefa_existente", existing),
        GroupError::Refused(JoinRefusal::Orq) => fail(super::routes::ORQ_ARBITER_CODE, super::routes::ORQ_ARBITER_MSG),
        GroupError::Orq(PromoteError::Conflict(text)) => fail("erro_orq_arquivo_mudou", text),
        GroupError::Orq(PromoteError::Unavailable(code)) => fail("erro_grupo_indisponivel", code),
        GroupError::Store(e) => fail("groups_store_failed", e.to_string()),
    }
}

/// `{status, body}` da rota de grupo, chamada como se viesse do dono.
pub(crate) async fn route(st: Arc<AppState>, method: &str, name: &str, route: &str, body: Value) -> Result<Value, Failed> {
    type Handler = fn(State<Arc<AppState>>, ConnectInfo<SocketAddr>, Request) -> futures_util::future::BoxFuture<'static, Response>;
    let (path, handler): (&str, Handler) = match (method, route) {
        ("POST", "pair") => ("pair", |s, c, r| Box::pin(super::routes::pair(s, c, r))),
        ("DELETE", "pair") => ("pair", |s, c, r| Box::pin(super::routes::unpair(s, c, r))),
        ("POST", "group-message") => ("group-message", |s, c, r| Box::pin(super::routes::group_message(s, c, r))),
        ("GET", "contract") => ("pair/contract", |s, c, r| Box::pin(super::routes::contract(s, c, r))),
        ("PUT", "contract") => ("pair/contract", |s, c, r| Box::pin(super::fed_notice::session_contract_put(s, c, r))),
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
        Operation::Route { method, name, route: path, body, facts } => {
            let ctx = BridgeCtx::new(true, facts.orchestrators, facts.phases);
            let mut out = CTX.scope(ctx.clone(), route(st, &method, &name, &path, body)).await?;
            out["deliveries"] = Value::Array(ctx.take_deliveries());
            out["post"] = Value::Array(std::mem::take(&mut *ctx.post.lock().unwrap_or_else(|e| e.into_inner())));
            Ok(out)
        }
        Operation::Leave { name, phases } => {
            // A saída de grupo entre máquinas avisa quem ficou sozinho: Pi/omp/Kimi só o Python atende.
            let ctx = BridgeCtx::new(true, Vec::new(), phases.clone());
            let (ex, warnings) = CTX.scope(ctx.clone(), async {
                let (ex, tell) = super::exit::leave(&st, &groups, &name, Some(&KnownPhases(phases))).await.map_err(group_failed)?;
                Ok::<_, Failed>((ex, notify_exit(&st, &groups, &name, &tell).await))
            }).await?;
            Ok(json!({"ex_peers": ex, "warnings": warnings, "deliveries": ctx.take_deliveries()}))
        }
        Operation::OrqAssociate { name, gid, mtime, facts } => {
            // Sob o lock: um join ou uma saída concorrente não vê o time meio associado.
            let done = groups.locked(async {
                let link = groups.link(&name).await?;
                let sidecars = groups.sidecars().await?;
                let root = groups.pair_root().to_path_buf();
                let team = TeamFiles { root, name, sidecars: sidecars.into_iter().map(|(_, s)| s).collect(), link_gid: link.map(|l| l.gid), facts };
                tokio::task::spawn_blocking(move || team.associate(&gid, mtime)).await
                    .map_err(|e| GroupError::Store(super::store::StoreError::Io(std::io::Error::other(e))))
            }).await.map_err(group_failed)?;
            let (status, body) = match done {
                Ok(body) => (200, body),
                Err(TeamError::Invalid(msg)) => (409, json!({"detail": {"code": "erro_orq_celula_invalida", "params": {}, "msg": msg}})),
                Err(TeamError::Changed(msg)) => (409, json!({"detail": {"code": "erro_orq_arquivo_mudou", "params": {}, "msg": msg}})),
            };
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

// ── associação do time do `orq` ao grupo (`orq_context.associate`), com os fatos que o Python mandou ──

/// Fatos do `orq` que o Python lê antes de pedir (I-5A): o Rust não pergunta de volta.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TeamFacts {
    /// `active_gid` e `draft_gid`: o Python só os lê quando o sidecar dele não tinha grupo.
    active_gid: Option<String>,
    draft_gid: Option<String>,
    /// Por gid citado (o destino e o apontado pelo rascunho): fase da execução e contrato arquivado.
    groups: std::collections::HashMap<String, GroupFacts>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GroupFacts { phase: Option<String>, archived: bool }

/// `ValueError` do Python (409 `erro_orq_celula_invalida`) e `Conflito`/`OSError` (409 `erro_orq_arquivo_mudou`).
enum TeamError { Invalid(String), Changed(String) }

const CHANGED_MSG: &str = "o time mudou desde a leitura — recarregue";
const GROUP_MARK: (&str, &str) = ("<!-- hangar-orq-group: ", " -->");
const PREFIX_MARK: (&str, &str) = ("<!-- hangar-orq-prefix: ", " -->");

struct Team { gid: String, grouped: bool, prefix: String, path: std::path::PathBuf }

struct TeamFiles { root: std::path::PathBuf, name: String, sidecars: Vec<super::model::Sidecar>, link_gid: Option<String>, facts: TeamFacts }

fn valid_gid(gid: &str) -> bool { !gid.is_empty() && gid.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') }

/// `_GROUP`/`_PREFIX` (`re.M`): a primeira linha inteira com a marca.
fn mark(text: &str, (open, close): (&str, &str)) -> Option<String> {
    text.split('\n').find_map(|line| line.strip_prefix(open)?.strip_suffix(close).filter(|v| valid_gid(v)).map(str::to_owned))
}

fn io_changed(e: std::io::Error) -> TeamError { TeamError::Changed(e.to_string()) }

/// `orq_md.ler_arquivo`: (texto, mtime); ausente = ("", 0.0).
/// Texto e mtime do mesmo arquivo aberto, mesmo com um rename no meio da leitura.
fn read(path: &std::path::Path) -> std::io::Result<(String, f64)> {
    use std::io::Read;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok((String::new(), 0.0)),
        Err(e) => return Err(e),
    };
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok((text, seconds(file.metadata()?.modified()?)))
}

fn seconds(at: std::time::SystemTime) -> f64 { at.duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

fn mtime(path: &std::path::Path) -> std::io::Result<f64> {
    match std::fs::metadata(path) {
        Ok(m) => Ok(seconds(m.modified()?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(0.0),
        Err(e) => Err(e),
    }
}

fn stale(current: f64, read: f64) -> bool { (current - read).abs() > 1e-6 }

impl TeamFiles {
    fn regras(&self, gid: &str) -> std::path::PathBuf { self.root.join(format!("regras-{gid}.md")) }

    /// Fase que o Python mandou; gid sem fato vale `unknown` (nunca "acabou").
    fn phase(&self, gid: &str) -> Option<&str> { self.facts.groups.get(gid).map_or(Some("unknown"), |g| g.phase.as_deref()) }

    fn real_group(&self, gid: &str, orchestration: bool) -> bool {
        self.sidecars.iter().any(|s| s.gid == gid && (!orchestration || s.orq)) || self.phase(gid) == Some("live")
    }

    fn group_ended(&self, gid: &str) -> bool {
        let phase = self.phase(gid);
        if matches!(phase, Some("live" | "unknown")) || self.real_group(gid, false) { return false; }
        phase == Some("ended") || self.facts.groups.get(gid).is_some_and(|g| g.archived)
    }

    fn resolve(&self) -> Result<Team, TeamError> {
        let fallback = || { let p = crate::list::discover_other::sanitize_session_name(&self.name); if p.is_empty() { "hangar".to_owned() } else { p } };
        if let Some(gid) = self.link_gid.clone().filter(|g| !g.is_empty()).or_else(|| self.facts.active_gid.clone()) {
            let path = self.regras(&gid);
            let (text, _) = read(&path).map_err(io_changed)?;
            return Ok(Team { prefix: mark(&text, PREFIX_MARK).unwrap_or_else(fallback), gid, grouped: true, path });
        }
        // O Python viu grupo no sidecar e não leu o rascunho; sem grupo agora, o time mudou no meio.
        let draft = self.facts.draft_gid.clone().ok_or_else(|| TeamError::Changed(CHANGED_MSG.into()))?;
        let path = self.regras(&draft);
        let (text, _) = read(&path).map_err(io_changed)?;
        if let Some(target) = mark(&text, GROUP_MARK) {
            let target_path = self.regras(&target);
            let (target_text, _) = read(&target_path).map_err(io_changed)?;
            // Um apontamento antigo não ressuscita contratos já arquivados.
            if !target_text.is_empty() && self.real_group(&target, false) {
                return Ok(Team { prefix: mark(&target_text, PREFIX_MARK).unwrap_or_else(fallback), gid: target, grouped: true, path: target_path });
            }
            if self.group_ended(&target) { return Ok(Team { gid: draft, grouped: false, prefix: fallback(), path }); }
            return Err(TeamError::Invalid("não foi possível confirmar o encerramento do grupo deste trabalho".into()));
        }
        Ok(Team { prefix: mark(&text, PREFIX_MARK).unwrap_or_else(fallback), gid: draft, grouped: false, path })
    }

    fn associate(&self, gid: &str, mtime_read: f64) -> Result<Value, TeamError> {
        if !valid_gid(gid) || !self.real_group(gid, true) {
            return Err(TeamError::Invalid("grupo de orquestração de destino não existe".into()));
        }
        let team = self.resolve()?;
        if team.grouped {
            if team.gid != gid { return Err(TeamError::Invalid("o time já pertence a outro grupo".into())); }
            if stale(read(&team.path).map_err(io_changed)?.1, mtime_read) { return Err(TeamError::Changed(CHANGED_MSG.into())); }
            return self.reply(&team);
        }
        let (source_text, current) = read(&team.path).map_err(io_changed)?;
        if source_text.is_empty() || mark(&source_text, GROUP_MARK).is_some() {
            return Err(TeamError::Invalid("configure o time deste trabalho antes de associá-lo".into()));
        }
        self.promote(gid, &team.path, &source_text, current, mtime_read)?;
        self.reply(&self.resolve()?)
    }

    /// `orq_context.promote`: move o registro ao grupo e deixa só o apontamento no rascunho.
    fn promote(&self, gid: &str, source: &std::path::Path, text: &str, current: f64, mtime_read: f64) -> Result<(), TeamError> {
        if gid.starts_with("draft-") || gid == "padrao" { return Err(TeamError::Invalid("grupo de destino inválido".into())); }
        if stale(current, mtime_read) { return Err(TeamError::Changed(CHANGED_MSG.into())); }
        let dest = self.regras(gid);
        let (existing, _) = read(&dest).map_err(io_changed)?;
        if !existing.is_empty() && existing != text {
            return Err(TeamError::Invalid("o grupo já possui outro contrato; associe o time antes de escrevê-lo".into()));
        }
        let moved = existing.is_empty();
        if moved { std::fs::rename(source, &dest).map_err(io_changed)?; }
        let pointer = format!("{}{gid}{}\n", GROUP_MARK.0, GROUP_MARK.1);
        // `orq_md.gravar` com o mtime lido: uma edição de fora no meio é conflito, nunca sobrescrita.
        let written = match mtime(source) {
            Ok(now) if stale(now, if moved { 0.0 } else { current }) => Err(source.display().to_string()),
            Ok(_) => super::store::replace_atomic(source, pointer.as_bytes()).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        };
        let Err(why) = written else { return Ok(()) };
        let restoration = if !moved { "não necessária".to_owned() } else {
            // Criação exclusiva: nunca substitui uma edição externa na origem.
            match std::fs::hard_link(&dest, source) {
                Ok(()) => "origem restaurada sem substituir outro arquivo".to_owned(),
                Err(e) => format!("origem não restaurada: {e}"),
            }
        };
        Err(TeamError::Invalid(format!("a promoção não foi concluída ({why}); {restoration}. Confira {} e {} antes de associar novamente",
            source.display(), dest.display())))
    }

    fn reply(&self, team: &Team) -> Result<Value, TeamError> {
        Ok(json!({"ok": true, "gid": team.gid, "grouped": team.grouped, "session_prefix": team.prefix,
            "arquivo": team.path.to_string_lossy(), "mtime": mtime(&team.path).map_err(io_changed)?}))
    }
}
