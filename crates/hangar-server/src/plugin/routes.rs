//! As rotas do plugin do Claude, montadas na porta pública e na ponte local (`routes::plugin_routes`).
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Instant;

use axum::{Router, body::Bytes, extract::{ConnectInfo, DefaultBodyLimit, FromRequest, Request, State}, http::{HeaderMap, StatusCode}, response::{IntoResponse, Response}, routing::post};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::auth::{TrustedHosts, is_loopback};
use crate::interactions::InteractionBinding;
use crate::mods::bridge::{machine_key_ok, mint_keyed, session_token_ok};
use super::{AgentState, Ask, PermissionGate, PluginMode, Pull, Receipt, ServiceError, Store};

/// Curto o bastante para a troca chegar ao app logo; o plugin rearma o `/pull` em 50 ms, então não gira em laço.
const MIRROR_MISSING_POLL: std::time::Duration = std::time::Duration::from_secs(1);

pub type SourceFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, ServiceError>> + Send + 'a>>;

#[derive(Clone, Debug)]
pub struct Identity {
    pub name: String,
    pub binding: InteractionBinding,
    pub plugin_key: String,
}

/// Retrato confirmado pelo runtime/lista, nunca preenchido com o corpo do plugin.
#[derive(Clone, Debug)]
pub struct Discovery {
    pub identity: Identity,
    pub pane: Option<String>,
    pub socket: Option<String>,
    pub mux_session: Option<String>,
    pub psmux_pid: Option<String>,
}

/// As fontes reais entram com o corte; erro de ligação não vira resposta vazia.
pub trait Sources: Send + Sync {
    fn enabled(&self) -> SourceFuture<'_, bool>;
    fn resolve<'a>(&'a self, name: &'a str, key: Option<&'a str>) -> SourceFuture<'a, Identity>;
    fn discovery(&self) -> SourceFuture<'_, Vec<Discovery>>;
    fn permission_gate<'a>(&'a self, identity: &'a Identity) -> SourceFuture<'a, PermissionGate>;
    fn band_present<'a>(&'a self, identity: &'a Identity) -> SourceFuture<'a, bool>;
    fn context<'a>(&'a self, identity: &'a Identity, context: MeasuredContext) -> SourceFuture<'a, ()>;
    fn rate<'a>(&'a self, identity: &'a Identity, tokens: u64, seconds: f64) -> SourceFuture<'a, ()>;
    /// O turno terminou com erro de API: se foi o limite de uso da conta, a troca de conta fica sabendo.
    fn turn_error<'a>(&'a self, identity: &'a Identity) -> SourceFuture<'a, ()>;
}

pub struct RouteState {
    pub store: Arc<Store>,
    pub secret: String,
    pub trusted: TrustedHosts,
    pub sources: Arc<dyn Sources>,
}

/// Os modelos Python não têm teto de corpo nessas nove rotas. Não herdar os 16 KiB dos mods.
pub fn router<S: Clone + Send + Sync + 'static>(state: Arc<RouteState>) -> Router<S> {
    Router::new()
        .route("/api/plugin/whoami", post(whoami))
        .route("/api/plugin/pull", post(pull))
        .route("/api/plugin/ask", post(ask))
        .route("/api/plugin/ask-fim", post(finish))
        .route("/api/plugin/plan", post(plan))
        .route("/api/plugin/filled", post(filled))
        .route("/api/plugin/submitted", post(submitted))
        .route("/api/plugin/state", post(state_event))
        .route("/api/plugin/suggest", post(suggest))
        .route("/api/plugin/rate", post(rate))
        .route("/api/plugin/agent", post(agent))
        .route("/api/plugin/agent-sent", post(agent_sent))
        .layer(DefaultBodyLimit::disable())
        .with_state(state)
}

use crate::json_envelope::Envelope;
#[derive(Clone, Debug, Deserialize)]
pub struct Whoami {
    pub chave: String,
    pub pane: Option<String>,
    pub nome: Option<String>,
    pub tmux: Option<String>,
    pub session_id: Option<String>,
}
#[derive(Deserialize)]
struct Finished {
    id: String, vencedor: String,
    publication_id: Option<String>, generation: Option<u64>, session_id: Option<String>,
}
#[derive(Deserialize)]
struct Acknowledged {
    ok: bool, publication_id: Option<String>, generation: Option<u64>, session_id: Option<String>,
    #[serde(default)]
    dropped: bool,
}
#[derive(Clone, Debug, Deserialize)]
pub struct MeasuredContext { pub used: u64, pub window: u64 }
#[derive(Deserialize)]
struct StateBody {
    estado: String, motivo: Option<String>, session_id: Option<String>, context: Option<MeasuredContext>,
    // `cwd`, `model`, `tool` e `origin` já eram auxiliares ignorados pela memória Python.
}
#[derive(Deserialize)]
struct Plan { id: String, plan: String, path: Option<String> }
#[derive(Deserialize)]
struct Suggest { texto: String, mostrada: bool }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentEvent { agent_id: String, estado: AgentState, at: Option<i64> }
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentSent { id: String, is_delivered: bool, reason: Option<String> }
#[derive(Deserialize)]
struct Rate { tokens: u64, seconds: f64, session_id: String }

/// O workspace não liga a feature `json` do axum; corpo que não desserializa é `plugin_body_invalid`.
struct Json<T>(T);
impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for Json<T> {
    type Rejection = Response;
    async fn from_request(req: Request, state: &S) -> Result<Self, Response> {
        let bytes = Bytes::from_request(req, state).await.map_err(IntoResponse::into_response)?;
        serde_json::from_slice(&bytes).map(Json).map_err(|_| fail(ServiceError("plugin_body_invalid")))
    }
}
impl IntoResponse for Json<Value> {
    fn into_response(self) -> Response { crate::session_write::json_response(StatusCode::OK, self.0) }
}

fn ok() -> Response { Json(json!({"ok":true})).into_response() }
fn fail(error: ServiceError) -> Response {
    let status = match error.0 {
        "plugin_token_invalid" | "plugin_machine_key_invalid" | "plugin_loopback_required" => StatusCode::FORBIDDEN,
        "plugin_body_invalid" => StatusCode::UNPROCESSABLE_ENTITY,
        "plugin_conversation_mismatch" | "plugin_identity_missing" | "plugin_identity_ambiguous" | "plugin_instance_conflict"
        | "plugin_publication_conflict" | "plugin_disabled" | "erro_interacao_desatualizada" | "erro_interacao_sem_identidade" => StatusCode::CONFLICT,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (status, Json(json!({"detail":{"code":error.0,"params":{},"msg":"A ponte do plugin recusou o pedido."}}))).into_response()
}
fn local(state: &RouteState, peer: SocketAddr, headers: &HeaderMap) -> bool {
    let (client, _) = state.trusted.resolve(peer.ip(), headers);
    is_loopback(&client)
}
async fn admit<T>(state: &RouteState, envelope: &Envelope<T>) -> Result<Identity, ServiceError> {
    if !session_token_ok(&state.secret, &envelope.sessao, &envelope.token) { return Err(ServiceError("plugin_token_invalid")); }
    // O liga/desliga (`claude_function_hooks`) vale para o processo que nasce (`whoami`); a sessão já aberta
    // com o plugin segue atendida, como no Python, que só conferia o token aqui.
    let mut key = envelope.token.split_once('.').map(|(key, _)| key);
    let identity = match state.sources.resolve(&envelope.sessao, key).await {
        // Chave que nenhuma sessão conhece (emitida pelo Python ou por um servidor anterior) cai no nome, que o token
        // assina; chave de outra sessão continua vencendo o nome.
        Err(ServiceError("plugin_identity_missing")) if key.is_some() => { key = None; state.sources.resolve(&envelope.sessao, None).await? }
        resolved => resolved?,
    };
    if identity.binding.key.is_empty() || identity.binding.conversation.is_empty() || identity.plugin_key.is_empty()
        || key.is_none() && identity.name != envelope.sessao
        || key.is_some_and(|key| key != identity.plugin_key && key != identity.binding.key) {
        return Err(ServiceError("plugin_identity_missing"));
    }
    Ok(identity)
}

/// Pane ambíguo não escolhe o primeiro. No psmux a conversa e o PID vencem o `%1` repetido.
pub fn discover(body: &Whoami, facts: &[Discovery]) -> Result<Option<(Identity, &'static str)>, ServiceError> {
    let Some(conversation) = body.session_id.as_deref().filter(|s| !s.is_empty()) else { return Ok(None); };
    let tmux: Vec<_> = body.tmux.as_deref().unwrap_or("").split(',').collect();
    let socket = tmux.first().copied().unwrap_or("");
    let psmux = socket.find("psmux-").and_then(|at| {
        let tail = &socket[at + 6..];
        let pid: String = tail.chars().take_while(char::is_ascii_digit).collect();
        let boundary = tail[pid.len()..].chars().next().is_none_or(|c| !c.is_alphanumeric() && c != '_');
        (!pid.is_empty() && boundary).then_some(pid)
    });
    let (candidates, origin): (Vec<_>, _) = if let Some(pid) = psmux {
        let by_pid: Vec<_> = facts.iter().filter(|f| f.psmux_pid.as_deref() == Some(pid.as_str())).collect();
        if by_pid.is_empty() {
            (facts.iter().filter(|f| f.psmux_pid.is_some() && f.identity.name == body.nome.as_deref().unwrap_or("")).collect(), "psmux-nome")
        } else { (by_pid, "psmux-pid") }
    } else {
        // Um socket fornecido precisa de prova positiva, não da ausência de informação.
        let matches_socket = |f: &&Discovery| f.psmux_pid.is_none() && (socket.is_empty() || f.socket.as_deref() == Some(socket));
        if let Some(pane) = body.pane.as_deref().filter(|p| !p.is_empty()) {
            let by_pane: Vec<_> = facts.iter().filter(matches_socket).filter(|f| f.pane.as_deref() == Some(pane)).collect();
            if by_pane.is_empty() {
                let mux_id = tmux.get(2).map(|id| id.trim().trim_start_matches('$')).filter(|id| !id.is_empty());
                (facts.iter().filter(matches_socket).filter(|f| mux_id.is_some() && f.mux_session.as_deref().map(|s| s.trim_start_matches('$')) == mux_id).collect(), "tmux")
            } else { (by_pane, "pane") }
        } else {
            (facts.iter().filter(matches_socket).filter(|f| f.identity.name == body.nome.as_deref().unwrap_or("")).collect(), "nome")
        }
    };
    if candidates.len() > 1 { return Err(ServiceError("plugin_identity_ambiguous")); }
    Ok(candidates.first().filter(|f| f.identity.binding.conversation == conversation
        && !f.identity.binding.key.is_empty() && !f.identity.name.is_empty() && !f.identity.plugin_key.is_empty())
        .map(|f| (f.identity.clone(), origin)))
}

async fn whoami(State(st): State<Arc<RouteState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<Whoami>) -> Response {
    if !local(&st, peer, &headers) { return fail(ServiceError("plugin_loopback_required")); }
    if !machine_key_ok(&st.secret, &body.chave) { return fail(ServiceError("plugin_machine_key_invalid")); }
    match st.sources.enabled().await {
        Ok(false) => return Json(json!({"sessao":null})).into_response(),
        Err(error) => return fail(error),
        Ok(true) => {},
    }
    let facts = match st.sources.discovery().await { Ok(facts) => facts, Err(error) => return fail(error) };
    match discover(&body, &facts) {
        Ok(Some((identity, origin))) if !identity.plugin_key.is_empty() => Json(json!({"sessao":identity.name,
            "token":mint_keyed(&st.secret, &identity.name, &identity.plugin_key),"origem":origin})).into_response(),
        Ok(_) => Json(json!({"sessao":null})).into_response(),
        Err(error) => fail(error),
    }
}
/// O `/pull` é o que prova a conversa sem o registro nativo: recusa dele vai ao log, uma vez por sessão e código,
/// para o relatório dizer se o plugin tentou e foi recusado ou se parou de tentar.
fn pull_refused(session: &str, error: ServiceError) -> Response {
    if crate::warn_limit::allow(Some(session), error.0) {
        tracing::warn!(session, code = error.0, "plugin: /pull recusado");
    }
    fail(error)
}
async fn pull(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<Pull>>) -> Response {
    let session = body.sessao.clone();
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return pull_refused(&session, error) };
    let present = match st.sources.band_present(&identity).await { Ok(present) => present, Err(error) => return pull_refused(&session, error) };
    let instance = body.body.instance.clone();
    let poll = match st.store.begin_pull(&identity.binding, body.body, Instant::now()) { Ok(poll) => poll, Err(error) => return pull_refused(&session, error) };
    // Sem o espelho (servidor reiniciado), o plugin só reenvia o `/ui` ao ler `faixa:false`: não espera o long-poll.
    let poll = if present { poll } else { poll.cap(MIRROR_MISSING_POLL) };
    // A conversa conferida acima tem esta vida desde o nascimento da instância.
    crate::state::ask::life_started(&identity.binding.conversation, &instance);
    let mut result = poll.wait().await;
    result["faixa"] = json!(present);
    Json(result).into_response()
}
async fn ask(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<Ask>>) -> Response {
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    let gate = if body.body.id.starts_with("perm:") {
        match st.sources.permission_gate(&identity).await { Ok(gate) => gate, Err(error) => return fail(error) }
    } else { PermissionGate { app_present: false, terminal_attached: true, mode_without_dialog: false } };
    match st.store.begin_ask(&identity.binding, body.body, gate, Instant::now()) {
        Ok(poll) => Json(poll.wait().await).into_response(),
        Err(error) => fail(error),
    }
}
fn receipt(id: Option<String>, generation: Option<u64>, session_id: Option<String>) -> Result<Option<Receipt>, ServiceError> {
    match (id, generation, session_id) {
        (None, None, _) => Ok(None),
        (Some(publication_id), Some(generation), Some(session_id)) => Ok(Some(Receipt { publication_id, generation, session_id })),
        _ => Err(ServiceError("plugin_body_invalid")),
    }
}
async fn finish(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<Finished>>) -> Response {
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    let body = body.body;
    let receipt = match receipt(body.publication_id, body.generation, body.session_id) { Ok(r) => r, Err(error) => return fail(error) };
    match st.store.finish(&identity.binding, &body.id, &body.vencedor, receipt) {
        Ok(()) => {
            // Só o fim aceito, vença o app ou o terminal: o sidecar da chamada deixa de segurar menu e fila.
            if let Some(call) = body.id.strip_prefix("ask:") { crate::state::ask::call_ended(call); }
            ok()
        }
        Err(error) => fail(error),
    }
}
async fn acknowledge(st: &RouteState, body: Envelope<Acknowledged>, mode: PluginMode) -> Response {
    let identity = match admit(st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    let body = body.body;
    let receipt = match receipt(body.publication_id, body.generation, body.session_id) { Ok(r) => r, Err(error) => return fail(error) };
    let result = if body.dropped {
        if body.ok || mode != PluginMode::User { return fail(ServiceError("plugin_body_invalid")); }
        st.store.submission_dropped(&identity.binding, receipt.as_ref())
    } else {
        st.store.acknowledge(&identity.binding, receipt.as_ref(), mode, body.ok)
    };
    match result { Ok(()) => ok(), Err(error) => fail(error) }
}
async fn filled(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<Acknowledged>>) -> Response {
    acknowledge(&st, body, PluginMode::Fill).await
}
async fn submitted(State(st): State<Arc<RouteState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, headers: HeaderMap, Json(body): Json<Envelope<Acknowledged>>) -> Response {
    if !local(&st, peer, &headers) { return fail(ServiceError("plugin_loopback_required")); }
    acknowledge(&st, body, PluginMode::User).await
}
async fn state_event(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<StateBody>>) -> Response {
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    let body = body.body;
    if body.context.as_ref().is_some_and(|c| c.used == 0 || c.window == 0) { return fail(ServiceError("plugin_body_invalid")); }
    // Todo fim de turno confere o limite: o aviso de limite semanal não chega com motivo `error`.
    let turn_end = body.estado == "idle";
    if let Err(error) = st.store.record_state(&identity.binding, body.estado, body.motivo, Instant::now()) { return fail(error); }
    // O estado já valeu; a leitura do limite que falha não muda a resposta ao plugin.
    if turn_end && let Err(error) = st.sources.turn_error(&identity).await {
        tracing::warn!(session = %identity.name, code = error.0, "fim de turno: o limite de uso não foi conferido");
    }
    if let Some(context) = body.context {
        if body.session_id.as_deref() != Some(identity.binding.conversation.as_str()) { return fail(ServiceError("plugin_conversation_mismatch")); }
        if let Err(error) = st.sources.context(&identity, context).await { return fail(error); }
    }
    ok()
}
/// Aviso do plano: guarda e responde na hora, porque o diálogo do terminal não espera o app.
async fn plan(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<Plan>>) -> Response {
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    let Plan { id, plan, path } = body.body;
    let Some(tool_use_id) = id.strip_prefix("plan:").filter(|id| !id.is_empty()) else { return fail(ServiceError("plugin_body_invalid")) };
    match st.store.notice_plan(&identity.binding, tool_use_id.to_owned(), plan, path) { Ok(()) => ok(), Err(error) => fail(error) }
}
async fn suggest(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<Suggest>>) -> Response {
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    match st.store.suggest(&identity.binding, body.body.texto, body.body.mostrada) { Ok(()) => ok(), Err(error) => fail(error) }
}
/// Sinal de ciclo de vida de um subagente (`agent.spawn`, fim do turno dele, retomada).
async fn agent(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<AgentEvent>>) -> Response {
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    let AgentEvent { agent_id, estado, at } = body.body;
    match st.store.record_agent(&identity.binding, &agent_id, estado, at) { Ok(()) => ok(), Err(error) => fail(error) }
}
/// Resposta do `$.session.send` a um texto que o `/pull` entregou.
async fn agent_sent(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<AgentSent>>) -> Response {
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    let AgentSent { id, is_delivered, reason } = body.body;
    match st.store.agent_sent(&identity.binding, &id, is_delivered, reason) { Ok(()) => ok(), Err(error) => fail(error) }
}
async fn rate(State(st): State<Arc<RouteState>>, Json(body): Json<Envelope<Rate>>) -> Response {
    if body.body.tokens == 0 || body.body.tokens > 1_000_000 || !body.body.seconds.is_finite() || body.body.seconds <= 0.0 || body.body.seconds > 3600.0 {
        return fail(ServiceError("plugin_body_invalid"));
    }
    let identity = match admit(&st, &body).await { Ok(identity) => identity, Err(error) => return fail(error) };
    if body.body.session_id != identity.binding.conversation { return fail(ServiceError("plugin_conversation_mismatch")); }
    match st.sources.rate(&identity, body.body.tokens, body.body.seconds).await { Ok(()) => ok(), Err(error) => fail(error) }
}
