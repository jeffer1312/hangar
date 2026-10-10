// crates/hangar-server/src/routes.rs
//! Rotas do hangar-server: saúde, custos, lista de sessões, histórico e chat ao vivo do Claude e do
//! Codex para o dono; todo o resto é repasse ao Python. Falha do Rust nessas rotas é 503 com código, nunca repasse.
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::PathRejection;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::serve::ListenerExt;
use bytes::Bytes;
use futures_util::StreamExt;
use http_body_util::BodyExt;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use crate::auth::{self, Auth};
use crate::config::Config;
use crate::proxy::{self, Forward, HttpClient};
use crate::side::{Binding, Hub, Hubs, INFO_TTL, Lease, Out, Queued, SideCtx, remember_info};
use crate::tail::{self, Watchers};
use crate::transcript::{InternalInfo, SKIPPED_LINES, history_etag, merged_history};

const INFO_TIMEOUT: Duration = Duration::from_secs(10);
const PING_EVERY: Duration = Duration::from_secs(10);
const COMMENT_EVERY: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

pub struct AppState {
    pub accounts: crate::accounts::AccountService,
    pub cfg: Config,
    pub auth: Auth,
    pub http: HttpClient,
    pub side: SideCtx,
    pub terminal: crate::terminal_control::TerminalPool,
    pub terminal_address: Option<SocketAddr>,
    pub workspace_slots: Arc<tokio::sync::Semaphore>,
    pub workspace_read_slots: Arc<tokio::sync::Semaphore>,
    pub workspace_meta_slots: Arc<tokio::sync::Semaphore>,
    pub diag: crate::diag::DiagClient,
    pub costs: Arc<crate::costs::collect::Collector>,
    pub fx: Arc<crate::costs::fx::Fx>,
    pub reports: Arc<crate::costs::ReportCache>,
    pub origins_home: std::path::PathBuf,
    pub origins: std::sync::Mutex<indexmap::IndexMap<std::path::PathBuf, crate::costs::origins::Origins>>,
    pub list: Arc<crate::list::bridge::ListBridge>,
    /// Produtor único da lista do dono; liga com a primeira lista aberta.
    pub hub: Arc<crate::list::hub::ListHub>,
    /// Painéis de terminal real de todas as portas.
    pub term: Arc<crate::term::Terms>,
    /// O que os `Monitor`s de estado compartilham (Claude com terminal).
    pub state: Arc<crate::state::live::StateEnv>,
    /// Interface dos mods das sessões sem terminal do Rust: o ator publica, as rotas consultam.
    pub mods: crate::mods::state::Mods,
    /// Páginas HTML publicadas pelas sessões (`~/.hangar/paginas`); o teste troca por uma pasta temporária.
    pub pages: Arc<crate::pages::store::Store>,
    /// Onde está o Chromium do servidor, perguntado a cada medição; o teste força "ausente".
    pub chromium: fn() -> Option<std::path::PathBuf>,
    /// Quanto uma escrita espera a porta de entrada da sessão reabrir (os testes encurtam).
    pub write_gate_wait: std::time::Duration,
    /// Grupos de sessões em `.hangar-pair`; `None` sem as pastas da lista (as rotas seguem ao Python).
    pub groups: Option<Arc<crate::groups::service::GroupService>>,
    /// Outras máquinas do dono (`peers.json`), para o par 1:1 entre máquinas.
    pub peers: Arc<crate::groups::peers::PeerClient>,
}

impl AppState {
    pub fn new(cfg: Config) -> AppState {
        Self::with_terminal_pool(cfg, crate::terminal_control::TerminalPool::new())
    }

    pub fn with_terminal_pool(cfg: Config, terminal: crate::terminal_control::TerminalPool) -> AppState {
        let costs = Arc::new(crate::costs::collect::Collector::new(
            crate::costs::index::default_dir(), crate::costs::pricing::default_dir(),
            crate::costs::areas::default_map_file(),
            Arc::new(crate::costs::collect::HttpScopes::new(cfg.upstream, cfg.internal_secret.clone())),
        ));
        Self::with_parts(cfg, terminal, costs, Arc::new(crate::costs::fx::Fx::new()))
    }

    pub fn with_parts(cfg: Config, terminal: crate::terminal_control::TerminalPool,
                      costs: Arc<crate::costs::collect::Collector>, fx: Arc<crate::costs::fx::Fx>) -> AppState {
        let http = proxy::client();
        let mods = crate::mods::state::Mods::default();
        let mut side = SideCtx {
            upstream: cfg.upstream,
            secret: cfg.internal_secret.clone(),
            http: http.clone(),
            watchers: Watchers::default(),
            hubs: Hubs::default(),
            infos: Default::default(),
            monitors: None,
            mods: mods.clone(),
        };
        mods.bind_hubs(side.hubs.downgrade());
        let diag = crate::diag::DiagClient::new(cfg.upstream, cfg.internal_secret.clone());
        let facts = crate::list::facts::FactsClient::new(cfg.upstream, cfg.internal_secret.clone());
        let list = Arc::new(crate::list::bridge::ListBridge::new(crate::list::bridge::ListEnv::from_env(), facts));
        let state = Arc::new(crate::state::live::StateEnv::new(terminal.clone(), list.clone(),
            crate::state::facts::StateFactsClient::new(cfg.upstream, cfg.internal_secret.clone()), diag.clone()));
        side.monitors = Some(crate::state::live::spawner(state.clone()));
        let groups = crate::groups::from_env(list.env().dirs.as_ref(),
            Arc::new(crate::groups::orq::PythonOrq::new(cfg.upstream, cfg.internal_secret.clone(), http.clone())), list.clone());
        AppState { accounts: crate::accounts::AccountService::new(crate::accounts::environment::AccountEnvironment::capture()), groups, peers: Arc::new(crate::groups::peers_from_env()), auth: Auth::new(&cfg.auth_token), http, side, cfg, terminal, terminal_address: None, diag,
            workspace_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            workspace_read_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            workspace_meta_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            costs, fx, reports: Arc::new(crate::costs::ReportCache::default()),
            origins_home: std::path::PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()),
            origins: std::sync::Mutex::new(indexmap::IndexMap::new()),
            list, state,
            hub: Arc::default(),
            term: Arc::default(),
            mods,
            pages: Arc::new(crate::pages::store::Store::new(crate::pages::store::Store::default_root())),
            chromium: crate::pages::chrome::find,
            write_gate_wait: std::time::Duration::from_secs(30) }
    }

    pub(crate) fn skill_origins(&self, repo: &std::path::Path) -> crate::costs::origins::Origins {
        let mut origins = self.origins.lock().unwrap();
        let cache = origins.shift_remove(repo).unwrap_or_else(|| crate::costs::origins::Origins::new(self.origins_home.clone(), repo.to_owned()));
        origins.insert(repo.to_owned(), cache.clone());
        while origins.len() > 8 { origins.shift_remove_index(0); }
        cache
    }

    /// `info` da sessão com cache curto: várias telas abrindo juntas viram uma consulta só. Só o
    /// `/events` usa, porque o primeiro `info` da conexão interna corrige um valor velho com `reset`.
    pub(crate) async fn info(&self, name: &str) -> Result<Option<InternalInfo>, InfoFailed> {
        if let Some((at, v)) = self.side.infos.lock().unwrap().get(name) {
            if at.elapsed() < INFO_TTL {
                return Ok(v.clone());
            }
        }
        let v = fetch_info(&self.http, self.cfg.upstream, &self.cfg.internal_secret, name).await?;
        // Só a sessão encontrada fica no cache: cada reconexão volta a perguntar pela inexistente.
        if v.is_some() { remember_info(&self.side.infos, name, v.clone()); }
        Ok(v)
    }
}

/// A rota interna não respondeu o `info` (fora do ar, erro, corpo inválido). 404 não é isto.
pub(crate) struct InfoFailed;

/// `Ok(None)` = 404: sessão inexistente, que o Python responde. O segredo recusado também é 404, e
/// o Python registra essa recusa no diário (`internal.recusado`).
pub(crate) async fn fetch_info(http: &HttpClient, upstream: SocketAddr, secret: &str, name: &str) -> Result<Option<InternalInfo>, InfoFailed> {
    let url = format!("http://{upstream}/internal/sessions/{}/info", utf8_percent_encode(name, NON_ALPHANUMERIC));
    let req = axum::http::Request::get(url).header("x-hangar-internal", secret).body(Body::empty()).map_err(|_| InfoFailed)?;
    let resp = match tokio::time::timeout(INFO_TIMEOUT, http.request(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(session = %name, "info interna falhou: {e}");
            return Err(InfoFailed);
        }
        Err(_) => {
            tracing::warn!(session = %name, "info interna sem resposta");
            return Err(InfoFailed);
        }
    };
    if resp.status() == StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        tracing::warn!(session = %name, status = %resp.status(), "info interna recusada");
        return Err(InfoFailed);
    }
    let body = match tokio::time::timeout(INFO_TIMEOUT, resp.into_body().collect()).await {
        Ok(Ok(b)) => b.to_bytes(),
        _ => {
            tracing::warn!(session = %name, "corpo da info interna não chegou");
            return Err(InfoFailed);
        }
    };
    match serde_json::from_slice(&body) {
        Ok(v) => Ok(Some(v)),
        Err(e) => {
            // Só a posição: a mensagem do serde pode citar o valor.
            tracing::warn!(session = %name, line = e.line(), column = e.column(), "info interna inválida");
            Err(InfoFailed)
        }
    }
}

/// Falha do Rust numa rota que é dele: 503 com o código, que o app mostra, e uma linha no diário.
/// `detail` é o envelope que o `lerErro` do app já traduz; `reason` é frase fixa, nunca conversa.
pub(crate) fn route_failed(st: &AppState, req: &HeaderMap, event: &'static str, name: &str, code: &'static str, reason: &'static str) -> Response {
    st.diag.report(event, name, code, reason);
    let body = serde_json::json!({"ok": false, "error_code": code, "message": reason,
        "detail": {"code": code, "params": {"motivo": reason}, "msg": format!("{reason} — {code}")}}).to_string();
    let mut resp = (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "application/json")], body).into_response();
    cors(req, resp.headers_mut());
    resp
}

pub(crate) const INFO_REASON: &str = "o backend não devolveu os dados da sessão";

pub async fn serve(listener: TcpListener, cfg: Config) -> std::io::Result<()> {
    serve_with_terminal_pool(listener, cfg, crate::terminal_control::TerminalPool::new()).await
}

pub async fn serve_with_terminal_pool(listener: TcpListener, cfg: Config, pool: crate::terminal_control::TerminalPool) -> std::io::Result<()> {
    serve_with_state(listener, AppState::with_terminal_pool(cfg, pool)).await
}

pub async fn serve_with_state(listener: TcpListener, mut state: AppState) -> std::io::Result<()> {
    let plugin = crate::plugin_listener::bind(&listener).await?;
    // Bind LAN específico não recebe tráfego de loopback: a observação tem uma porta própria.
    let private = TcpListener::bind("127.0.0.1:0").await?;
    state.terminal_address = Some(private.local_addr()?);
    let state = Arc::new(state);
    // Abortada na saída: a tarefa segura o estado do servidor, que sobreviveria a ele.
    let _group_sweep = crate::groups::sweep::spawn(state.clone()).map(crate::AbortOnDrop);
    let plugin_state = state.clone();
    tokio::select! {
        result = axum::serve(listener.tap_io(crate::nodelay), router(state.clone()).into_make_service_with_connect_info::<SocketAddr>()) => result,
        result = axum::serve(private.tap_io(crate::nodelay), terminal_router(state).into_make_service_with_connect_info::<SocketAddr>()) => result,
        result = async {
            match plugin {
                Some(plugin) => axum::serve(plugin.tap_io(crate::nodelay), plugin_router(plugin_state).into_make_service_with_connect_info::<SocketAddr>()).await,
                None => std::future::pending::<std::io::Result<()>>().await,
            }
        } => result,
    }
}

pub fn terminal_router(state: Arc<AppState>) -> Router {
    let router = Router::new()
        .route("/__hangar_server/terminal", axum::routing::post(crate::terminal_routes::terminal))
        .route("/__hangar_server/workspace", axum::routing::post(crate::workspace_routes::private))
        .route("/__hangar_server/claude/customizations", axum::routing::post(crate::claude_customizations::private))
        .route("/__hangar_server/list", axum::routing::post(crate::list::bridge::private))
        .route("/__hangar_server/accounts", axum::routing::post(crate::accounts::http::private))
        .route("/__hangar_server/uploads/{name}", axum::routing::any(crate::uploads::http::private))
        .route("/__hangar_server/quotas", axum::routing::post(crate::accounts::quotas::private))
        .route("/__hangar_server/accounts/claude", axum::routing::post(crate::accounts::http::private_claude))
        .route("/__hangar_server/accounts/public", axum::routing::any(crate::accounts::http::private_public))
        .route("/__hangar_server/pages", axum::routing::post(crate::pages::routes::publish_bridge))
        .route("/__hangar_server/groups", axum::routing::post(crate::groups::bridge::private))
        .route("/__hangar_server/mods/{name}/{op}", axum::routing::post(crate::mods::routes::bridge))
        .layer(axum::middleware::from_fn(crate::migration_status::count_bridge));
    // Painel e canal do estado ficam fora da contagem: conexões longas, não chamadas da ponte.
    router.route("/__hangar_server/term", get(crate::term::private_ws))
        .route("/__hangar_server/state/{name}/events", get(crate::side::private_events))
        .with_state(state)
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/sessions/{name}/term", get(crate::term::session_ws).fallback(pass_any))
        .route("/api/hangar-terminals/{ident}/term", get(crate::term::hangar_ws).fallback(pass_any))
        .route("/__hangar_server/health", get(health))
        .route("/__hangar_server/terminal", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/workspace", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/claude/customizations", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/list", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/accounts", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/uploads/{name}", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/quotas", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/accounts/claude", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/accounts/public", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/pages", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/groups", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/mods/{name}/{op}", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/term", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/__hangar_server/state/{name}/events", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        // Outro método nessas rotas (preflight OPTIONS, HEAD) segue ao Python.
        .route("/api/sessions", get(crate::list::hub::list).fallback(pass_any))
        .route("/api/sessions/events", get(crate::list::hub::events).fallback(pass_any))
        .route("/api/sessions/{name}/history", get(history).fallback(pass_any))
        .route("/api/sessions/{name}/events", get(events).fallback(pass_any))
        // Interface dos mods: o Rust atende a sessão sem terminal dele; o resto segue ao Python.
        .route("/api/sessions/{name}/plugin/press", axum::routing::post(crate::mods::routes::press).fallback(pass_any))
        .route("/api/sessions/{name}/plugin/close", axum::routing::post(crate::mods::routes::close).fallback(pass_any))
        .route("/api/sessions/{name}/plugin/show", axum::routing::post(crate::mods::routes::show).fallback(pass_any))
        .route("/api/sessions/{name}/plugin/input", axum::routing::post(crate::mods::routes::input).fallback(pass_any))
        .merge(plugin_routes())
        // Páginas HTML da conversa: o dono lê no Rust; o convidado segue ao Python.
        .route("/api/sessions/{name}/pages/{id}", get(crate::pages::routes::page).fallback(pass_any))
        .route("/api/sessions/{name}/pages/{id}/shot", get(crate::pages::routes::shot).fallback(pass_any))
        // Escritas Claude: o Rust decide por pedido e repassa o que não é dele (corpo intacto).
        .route("/api/sessions/{name}/input", axum::routing::post(crate::session_write::input::input).fallback(pass_any))
        .route("/api/sessions/{name}/steer", axum::routing::post(crate::session_write::input::steer).fallback(pass_any))
        .route("/api/sessions/{name}/interrupt", axum::routing::post(crate::session_write::control::interrupt).fallback(pass_any))
        .route("/api/sessions/{name}/select", axum::routing::post(crate::session_write::control::select).fallback(pass_any))
        .route("/api/sessions/{name}/select/submit", axum::routing::post(crate::session_write::control::select_submit).fallback(pass_any))
        .route("/api/sessions/{name}/answer", axum::routing::post(crate::session_write::answer::answer).fallback(pass_any))
        .route("/api/sessions/{name}/keys", axum::routing::post(crate::session_write::control::keys).fallback(pass_any))
        .route("/api/sessions/{name}/term-input", axum::routing::post(crate::session_write::control::term_input).fallback(pass_any))
        .route("/api/sessions/{name}/queue/{entry_id}", axum::routing::delete(crate::session_write::control::queue_remove).fallback(pass_any))
        // Rotas só do Codex: o Rust atende a sessão sem terminal dele; o resto segue ao Python.
        .route("/api/sessions/{name}/models", get(crate::session_write::codex::models).fallback(pass_any))
        .route("/api/sessions/{name}/model", axum::routing::post(crate::session_write::codex::model).fallback(pass_any))
        .route("/api/sessions/{name}/service-tier", axum::routing::post(crate::session_write::codex::service_tier).fallback(pass_any))
        .route("/api/sessions/{name}/codex/mode", axum::routing::post(crate::session_write::codex::mode).fallback(pass_any))
        .route("/api/sessions/{name}/limits", get(crate::session_write::codex::limits).fallback(pass_any))
        .route("/api/sessions/{name}/question/skip", axum::routing::post(crate::session_write::codex::skip_question).fallback(pass_any))
        .route("/api/sessions/{name}/commands", get(crate::session_write::codex::commands).fallback(pass_any))
        .route("/api/sessions/{name}/codex-permissions", get(crate::session_write::codex::permissions).fallback(pass_any))
        // Mesmo caminho: o axum junta o POST à rota de cima (um repasse só, o dela).
        .route("/api/sessions/{name}/codex-permissions", axum::routing::post(crate::session_write::codex::set_permission))
        .route("/api/sessions/{name}/cost", get(crate::costs_routes::session_cost).fallback(pass_any))
        .route("/api/costs", get(crate::costs_routes::costs).fallback(pass_any))
        .route("/api/cotacao", get(crate::costs_routes::cotacao).fallback(pass_any))
        .route("/api/uso", get(crate::costs_routes::usage).fallback(pass_any))
        .route("/api/migration/status", get(crate::migration_status::status).fallback(pass_any))
        // Grupos (`/pair`, `/group-message`, `/pair/contract`, `/pair-remote`, `/unpair-remote`).
        .merge(crate::groups::routes::router())
        .fallback(pass_any)
        .layer(axum::middleware::from_fn(crate::migration_status::count_public))
        .with_state(state)
}

fn plugin_routes() -> Router<Arc<AppState>> {
    let router = Router::new()
        .route("/api/plugin/press-start", axum::routing::post(crate::mods::bridge::press_start).fallback(pass_any))
        .route("/api/plugin/opened", axum::routing::post(crate::mods::bridge::opened).fallback(pass_any))
        .route("/api/plugin/ui", axum::routing::post(crate::mods::bridge::ui).fallback(pass_any))
        .route("/api/plugin/toast", axum::routing::post(crate::mods::bridge::toast).fallback(pass_any))
        .route("/api/plugin/pressed", axum::routing::post(crate::mods::bridge::pressed).fallback(pass_any))
        .route("/api/plugin/copied", axum::routing::post(crate::mods::bridge::copied).fallback(pass_any))
        .route("/api/plugin/focus-target", axum::routing::post(crate::mods::bridge::focus_target).fallback(pass_any))
        .route("/api/plugin/focused", axum::routing::post(crate::mods::bridge::focused).fallback(pass_any))
        .route("/api/plugin/scroll", axum::routing::post(crate::mods::bridge::scroll).fallback(pass_any));
    ["whoami", "pull", "suggest", "ask", "ask-fim", "filled", "submitted", "state", "rate"]
        .into_iter().fold(router, |router, endpoint| {
            router.route(&format!("/api/plugin/{endpoint}"), axum::routing::any(pass_any))
        })
}

fn plugin_router(state: Arc<AppState>) -> Router {
    // A ponte local não expõe o restante da API nem dá acesso às rotas privadas.
    plugin_routes().fallback(|| async { StatusCode::NOT_FOUND })
        .layer(axum::middleware::from_fn(crate::migration_status::count_public))
        .with_state(state)
}

async fn health(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let owns: Vec<_> = crate::session_write::table::owned_modes().into_iter()
        .map(|(provider, headless)| serde_json::json!({"provider": provider.name(), "headless": headless})).collect();
    let body = serde_json::json!({"ok": true, "version": env!("CARGO_PKG_VERSION"),
        "protocol": crate::INTERNAL_PROTOCOL,
        "owns": owns,
        // O painel de terminal real é do Rust em todas as plataformas.
        "terminal_panel": true,
        // Sem as pastas da lista não há serviço de grupos: as rotas seguem ao Python, que fica dono.
        "groups": st.groups.is_some(),
        "terminal_address": st.terminal_address.map(|a| a.to_string())}).to_string();
    let mut resp = ([(header::CONTENT_TYPE, "application/json")], body).into_response();
    cors(&headers, resp.headers_mut());
    resp
}

/// Quem pede e se é o dono, resolvidos uma vez por pedido.
pub(crate) fn gate(st: &AppState, peer: SocketAddr, req: &Request) -> (Forward, bool) {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let token = auth::presented_token(req.headers(), req.uri().query(), req.method(), https);
    let owner = st.auth.is_owner(&client_ip, token.as_deref());
    (Forward { client_ip, https }, owner)
}

pub(crate) async fn pass(st: &AppState, req: Request, fwd: &Forward) -> Response {
    let upgrade = req.headers().contains_key(header::UPGRADE);
    let mut resp = proxy::forward(&st.http, st.cfg.upstream, req, fwd).await;
    resp.extensions_mut().insert(crate::migration_status::Forwarded);
    match resp.status() {
        StatusCode::UNAUTHORIZED => st.auth.record_fail(&fwd.client_ip),
        // WebSocket recusado antes do aceite chega como 403 e o Python já contou a falha.
        // Um 403 de origem também conta: só desliga o atalho, o lado seguro.
        StatusCode::FORBIDDEN if upgrade => st.auth.record_fail(&fwd.client_ip),
        StatusCode::TOO_MANY_REQUESTS => st.auth.mark_blocked(&fwd.client_ip),
        _ => {}
    }
    resp
}

pub(crate) async fn pass_any(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    // Anexos e contas do dono são do Rust; o convidado segue ao Python.
    let uploads = crate::uploads::http::matches(req.method(), req.uri().path());
    if (uploads || crate::accounts::http::matches(req.method(), req.uri().path()))
        && gate(&st, peer, &req).1
    {
        let headers = req.headers().clone();
        let path = req.uri().path().to_owned();
        let mut response = if uploads {
            crate::uploads::http::public(st.clone(), req).await
        } else {
            crate::accounts::http::public(st.clone(), req).await
        };
        // Sem o Python nessas rotas, a falha só chega ao diário exportável por aqui.
        if uploads {
            let session = crate::uploads::http::tail(&path).map_or("", |(name, _)| name);
            st.diag.report_response("rust.uploads_failed", session, &response, "a operação de anexo falhou");
        } else {
            let scope = crate::accounts::http::journal_scope(&path);
            st.diag.report_response("rust.accounts_failed", &scope, &response, "a operação de conta falhou");
        }
        cors(&headers, response.headers_mut());
        return response;
    }
    if crate::worktree_routes::matches(req.method(), req.uri().path()) {
        let (forward, owner) = gate(&st, peer, &req);
        // Convidado segue ao Python, que o recusa como antes.
        if owner {
            let headers = req.headers().clone();
            let mut response = crate::worktree_routes::public(st, req, forward).await;
            cors(&headers, response.headers_mut());
            return response;
        }
    }
    if crate::workspace_routes::matches(req.method(), req.uri().path()) {
        let (forward, owner) = gate(&st, peer, &req);
        if owner {
            let headers = req.headers().clone();
            let mut response = crate::workspace_routes::public(st, req, forward).await;
            cors(&headers, response.headers_mut());
            return response;
        }
    }
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    pass(&st, req, &Forward { client_ip, https }).await
}

/// Id de correlação do diário: cabeçalho x-hangar-req; o EventSource só manda ?diag_req.
fn diag_req(req: &Request) -> String {
    let h: String = req
        .headers()
        .get("x-hangar-req")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .chars()
        .take(32)
        .collect();
    if !h.is_empty() {
        return h;
    }
    auth::query_param(req.uri().query(), "diag_req")
        .filter(|c| (1..=32).contains(&c.len()) && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
        .unwrap_or_default()
}

async fn history(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    req: Request,
) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    let name = match path {
        Ok(Path(n)) if owner && req.method() == Method::GET => n,
        _ => return pass(&st, req, &fwd).await,
    };
    let limit = match auth::query_param(req.uri().query(), "limit") {
        None => None,
        Some(v) => match v.parse::<i64>() {
            // Como o Python: `limit <= 0` é o histórico inteiro (api.py:2918), nunca um 400.
            Ok(n) if n > 0 => usize::try_from(n).ok(),
            Ok(_) => None,
            // Texto ou vazio fica com o FastAPI e o 422 dele.
            Err(_) => return pass(&st, req, &fwd).await,
        },
    };
    // Sem o cache: sessão recriada com o mesmo nome dentro do TTL devolveria a conversa morta, e
    // nada a corrigiria depois (o `/events` só a corrige com `reset` por estar ligado ao hub).
    let Ok(info) = fetch_info(&st.http, st.cfg.upstream, &st.cfg.internal_secret, &name).await else {
        return route_failed(&st, req.headers(), "rust.history_failed", &name, "internal_info", INFO_REASON);
    };
    if info.is_some() { remember_info(&st.side.infos, &name, info.clone()); }
    // Sessão inexistente ou provedor fora do Rust: o Python é o dono.
    let Some(hreq) = info.and_then(|i| i.history_request(limit)) else {
        return pass(&st, req, &fwd).await;
    };
    tracing::debug!(session = %name, req = %diag_req(&req), "history");
    let inm = req.headers().get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()).map(str::to_owned);
    let log_name = name.clone();
    let done = tokio::task::spawn_blocking(move || -> std::io::Result<(Option<String>, Option<Vec<u8>>)> {
        let etag = history_etag(&hreq);
        if etag.is_some() && etag == inm {
            return Ok((etag, None));
        }
        let before = SKIPPED_LINES.load(Ordering::Relaxed);
        // Já vem com o corte `evs[-limit:]` da rota (Task 9).
        let evs = merged_history(&hreq)?;
        tail::log_skipped(&log_name, before);
        Ok((etag, Some(serde_json::to_vec(&evs)?)))
    })
    .await;
    let (etag, body) = match done {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            tracing::warn!(session = %name, kind = ?e.kind(), "history no Rust falhou");
            return route_failed(&st, req.headers(), "rust.history_failed", &name, "history_io", "a leitura do histórico falhou");
        }
        Err(e) => {
            // Sem `{e}`: a mensagem do pânico pode citar texto da conversa.
            tracing::warn!(session = %name, panic = e.is_panic(), cancelled = e.is_cancelled(), "history no Rust caiu");
            return route_failed(&st, req.headers(), "rust.history_failed", &name, "history_panic", "a leitura do histórico caiu no servidor");
        }
    };
    let mut resp = match body {
        None => StatusCode::NOT_MODIFIED.into_response(),
        Some(body) => {
            let mut r = Response::new(Body::empty());
            r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
            let body = maybe_gzip(req.headers(), r.headers_mut(), body);
            *r.body_mut() = Body::from(body);
            r
        }
    };
    if let Some(v) = etag.and_then(|e| HeaderValue::from_str(&e).ok()) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    cors(req.headers(), resp.headers_mut());
    resp
}

async fn events(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    req: Request,
) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    let name = match path {
        Ok(Path(n)) if owner && req.method() == Method::GET => n,
        _ => return pass(&st, req, &fwd).await,
    };
    let Ok(info) = st.info(&name).await else {
        return route_failed(&st, req.headers(), "rust.events_failed", &name, "internal_info", INFO_REASON);
    };
    // Sessão inexistente ou provedor fora do Rust: o Python é o dono.
    let Some(binding) = info.as_ref().and_then(Binding::from_info) else {
        return pass(&st, req, &fwd).await;
    };
    // A query vence: o app recria o EventSource a cada queda, e objeto novo não manda o cabeçalho.
    let resume = auth::query_param(req.uri().query(), "last_event_id")
        .filter(|v| !v.is_empty())
        .or_else(|| req.headers().get("last-event-id").and_then(|v| v.to_str().ok()).map(str::to_owned));
    tracing::debug!(session = %name, req = %diag_req(&req), retomada = resume.is_some(), "events: abriu");
    // O app que sabe juntar a diferença da vista dos mods anuncia; app antigo segue com a vista inteira.
    let deltas = auth::query_param(req.uri().query(), "ui_delta").as_deref() == Some("1");
    let lease = st.side.hubs.acquire(&name, binding, &st.side);
    let hub = lease.hub.clone();
    let (tx, rx) = mpsc::channel::<Queued>(64);
    tokio::spawn(client_loop(lease, resume, tx));
    let mut resp = Response::new(Body::from_stream(device_stream(rx, hub, deltas).map(Ok::<Bytes, Infallible>)));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    cors(req.headers(), h);
    resp
}

/// O que a conexão de um aparelho escreve, na ordem da fila. O marcador do `plugin_ui` só vira quadro
/// aqui, quando o corpo da resposta pede o próximo: o aparelho lento recebe a vista mais nova, e a fila
/// guarda marcadores, não cópias de até ~400 KB.
fn device_stream(rx: mpsc::Receiver<Queued>, hub: Arc<Hub>, deltas: bool) -> impl futures_util::Stream<Item = Bytes> {
    futures_util::stream::unfold((rx, hub, None), move |(mut rx, hub, mut ui)| async move {
        loop {
            let item = rx.recv().await?;
            if let Some(frame) = hub.resolve(item, &mut ui, deltas) {
                return Some((frame, (rx, hub, ui)));
            }
        }
    })
}

async fn client_loop(lease: Lease, resume: Option<String>, out: mpsc::Sender<Queued>) {
    let hub = lease.hub.clone();
    // O front dá 10 s para o primeiro quadro: o ping sai antes de qualquer leitura.
    if !push(&out, tail::ping_frame()).await {
        return;
    }
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING_EVERY, PING_EVERY);
    let mut comment = tokio::time::interval_at(start + COMMENT_EVERY, COMMENT_EVERY);
    let mut resume = resume;
    loop {
        let Some(att) = hub.attach(resume.take()).await else {
            let _ = push(&out, tail::reset_frame()).await;
            return;
        };
        for f in att.frames {
            if !push(&out, f).await {
                return;
            }
        }
        if !push(&out, Queued::Has(att.has_ui.then_some(att.ui))).await {
            return;
        }
        let (generation, mut rx, mut ui) = (att.generation, att.rx, att.ui);
        let rebind = loop {
            tokio::select! {
                _ = out.closed() => return,
                _ = ping.tick() => if !push(&out, tail::ping_frame()).await { return },
                _ = comment.tick() => if !push(&out, tail::comment_frame()).await { return },
                msg = rx.recv() => match msg {
                    Ok(Out::Tail(g, f)) if g == generation => if !push(&out, f).await { return },
                    Ok(Out::Tail(..)) => {}
                    Ok(Out::Side(f)) => if !push(&out, f).await { return },
                    // A versão que já foi no retrato da entrada não sai de novo.
                    Ok(Out::Ui(version)) if version > ui => {
                        ui = version;
                        if !push(&out, Queued::Ui(version)).await { return }
                    }
                    Ok(Out::Ui(_)) => {}
                    Ok(Out::Rebind) => break true,
                    Ok(Out::Close) | Err(broadcast::error::RecvError::Closed) => break false,
                    // Atrasado demais para o canal: fecha, e o aparelho retoma pelo último id.
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        tracing::info!(session = %hub.name, "aparelho atrasado; conexão fechada para retomar");
                        return;
                    }
                },
            }
        };
        if !push(&out, tail::reset_frame()).await || !rebind {
            return;
        }
    }
}

/// Envio preso por 30 s fecha a conexão, como o send_timeout do Python.
async fn push(out: &mpsc::Sender<Queued>, item: impl Into<Queued>) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, out.send(item.into())).await, Ok(Ok(())))
}

/// CORS das respostas do próprio Rust, como o CORSMiddleware do Python: `*`, sem credenciais,
/// ETag legível pelo JS. Preflight não chega aqui: vai sem token e é repassado.
pub(crate) fn cors(req: &HeaderMap, resp: &mut HeaderMap) {
    if req.contains_key(header::ORIGIN) {
        resp.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        resp.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("ETag"));
    }
}

/// Gzip só quando o cliente pede e o corpo passa de 1 KB. SSE nunca: o gzip seguraria os quadros.
pub(crate) fn maybe_gzip(req: &HeaderMap, resp: &mut HeaderMap, body: Vec<u8>) -> Vec<u8> {
    let wants = req
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    let sse = resp
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    if !wants || sse || body.len() < 1024 {
        return body;
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(5));
    std::io::Write::write_all(&mut enc, &body).expect("escrita em memória");
    resp.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    resp.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    enc.finish().expect("escrita em memória")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    /// Estado com um Python mínimo: `/ws` recusa com 403, `/limited` responde 429.
    async fn state_with_upstream() -> Arc<AppState> {
        use axum::routing::any;
        let app = Router::new()
            .route("/ws", any(|| async { StatusCode::FORBIDDEN }))
            .route("/limited", any(|| async { StatusCode::TOO_MANY_REQUESTS }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let cfg = Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream,
            internal_secret: "s".into(),
            auth_token: "dono".into(),
            log_path: None,
            trusted: auth::TrustedHosts::parse("127.0.0.1"),
        };
        Arc::new(AppState::new(cfg))
    }

    fn request(path: &str, ip: &str, ws: bool, token: &str) -> Request {
        let mut b = Request::builder()
            .uri(path)
            .header("x-forwarded-for", ip)
            .header(header::AUTHORIZATION, format!("Bearer {token}"));
        if ws {
            b = b.header("connection", "Upgrade").header("upgrade", "websocket");
        }
        b.body(axum::body::Body::empty()).unwrap()
    }

    fn is_owner(st: &AppState, ip: &str) -> bool {
        let peer: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        gate(st, peer, &request("/x", ip, false, "dono")).1
    }

    #[tokio::test]
    async fn websocket_403_and_429_from_python_lock_the_shortcut() {
        let st = state_with_upstream().await;
        let peer: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let fwd = |ip: &str| gate(&st, peer, &request("/x", ip, false, "x")).0;

        // 8 palpites errados em WebSocket: o Python conta e fecha com 403.
        for _ in 0..8 {
            let r = pass(&st, request("/ws", "198.51.100.7", true, "errado"), &fwd("198.51.100.7")).await;
            assert_eq!(r.status(), StatusCode::FORBIDDEN);
        }
        assert!(!is_owner(&st, "198.51.100.7"), "o token certo não abre o atalho nessa origem");
        assert!(is_owner(&st, "198.51.100.8"), "outra origem segue normal");

        // Um 429 do Python satura a origem de uma vez.
        let r = pass(&st, request("/limited", "198.51.100.9", false, "errado"), &fwd("198.51.100.9")).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(!is_owner(&st, "198.51.100.9"));

        // 403 sem Upgrade não é falha de token.
        let st2 = state_with_upstream().await;
        for _ in 0..8 {
            pass(&st2, request("/ws", "198.51.100.10", false, "x"), &fwd("198.51.100.10")).await;
        }
        assert!(is_owner(&st2, "198.51.100.10"));

        // Loopback é isento.
        let r = pass(&st, request("/limited", "127.0.0.1", false, "x"), &fwd("127.0.0.1")).await;
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(is_owner(&st, "127.0.0.1"));
    }

    /// Aparelho ligado a um hub sem conexão interna (porta sem ninguém): só o que o teste entrega chega.
    async fn device(dir: &std::path::Path) -> (SideCtx, std::pin::Pin<Box<dyn futures_util::Stream<Item = Bytes> + Send>>) {
        device_with(dir, false).await
    }

    async fn device_with(dir: &std::path::Path, deltas: bool) -> (SideCtx, std::pin::Pin<Box<dyn futures_util::Stream<Item = Bytes> + Send>>) {
        let ctx = SideCtx {
            upstream: "127.0.0.1:9".parse().unwrap(),
            secret: "s".into(),
            http: proxy::client(),
            watchers: Watchers::default(),
            hubs: Hubs::default(),
            infos: Default::default(),
            monitors: None,
            mods: Default::default(),
        };
        let jsonl = dir.join("t.jsonl");
        std::fs::write(&jsonl, "").unwrap();
        let lease = ctx.hubs.acquire("s", Binding { provider: crate::transcript::Provider::Claude, jsonl, key: "k".into(), headless: false }, &ctx);
        let hub = lease.hub.clone();
        let (tx, rx) = mpsc::channel::<Queued>(64);
        tokio::spawn(client_loop(lease, None, tx));
        // O aparelho já assinou o canal: o que for entregue daqui em diante vai pela fila dele.
        tokio::time::timeout(Duration::from_secs(5), async {
            while hub.tx.receiver_count() == 0 { tokio::time::sleep(Duration::from_millis(5)).await; }
        }).await.unwrap();
        (ctx, Box::pin(device_stream(rx, hub, deltas)))
    }

    /// Os próximos quadros sem `ping`, como (evento, dado), até o evento `until` inclusive.
    async fn read_until(stream: &mut (impl futures_util::Stream<Item = Bytes> + Unpin), until: (&str, &str)) -> Vec<(String, String)> {
        let mut out = Vec::new();
        loop {
            let frame = tokio::time::timeout(Duration::from_secs(5), stream.next()).await.unwrap().unwrap();
            let text = String::from_utf8_lossy(&frame).into_owned();
            let event = text.lines().find_map(|line| line.strip_prefix("event: ")).unwrap_or("").to_owned();
            let data = text.lines().find_map(|line| line.strip_prefix("data: ")).unwrap_or("").to_owned();
            if event == "ping" || event.is_empty() { continue; }
            let done = (event.as_str(), data.as_str()) == until;
            out.push((event, data));
            if done { return out; }
        }
    }

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter().map(|(event, data)| (event.to_string(), data.to_string())).collect()
    }

    #[tokio::test]
    async fn slow_device_gets_only_the_newest_band_and_every_other_event_in_order() {
        // I2: o aparelho que não lê enquanto a faixa muda não acumula as vistas intermediárias; os outros
        // eventos chegam todos, na ordem, e a faixa sai no lugar do último marcador dela.
        let dir = tempfile::tempdir().unwrap();
        let (ctx, mut stream) = device(dir.path()).await;
        for (event, data) in [("state", "1"), ("plugin_ui", "u1"), ("state", "2"), ("plugin_ui", "u2"),
                              ("message", "{\"id\":\"m1\"}"), ("plugin_ui", "u3"), ("state", "3")] {
            ctx.hubs.deliver("s", event, data);
        }
        // A fila do aparelho já tem tudo (marcadores, não quadros) antes de ele ler.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(read_until(&mut stream, ("state", "3")).await,
            pairs(&[("state", "1"), ("state", "2"), ("message", "{\"id\":\"m1\"}"), ("plugin_ui", "u3"), ("state", "3")]));
    }

    #[tokio::test]
    async fn repeated_band_does_not_drop_the_view_a_slow_device_still_has_to_send() {
        // N1: a mesma vista de novo (o Python reenvia a faixa a cada religação; o Rust limpa duas vezes) não
        // pode invalidar o marcador que a fila do aparelho lento ainda tem.
        let dir = tempfile::tempdir().unwrap();
        let (ctx, mut stream) = device(dir.path()).await;
        ctx.hubs.deliver("s", "plugin_ui", "u1");
        ctx.hubs.deliver("s", "plugin_ui", "u1");
        ctx.hubs.deliver("s", "state", "1");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(read_until(&mut stream, ("state", "1")).await, pairs(&[("plugin_ui", "u1"), ("state", "1")]));
    }

    #[tokio::test]
    async fn device_that_keeps_up_gets_every_band_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, mut stream) = device(dir.path()).await;
        for (event, data) in [("plugin_ui", "u1"), ("state", "1"), ("plugin_ui", "u2"), ("plugin_ui", "u3")] {
            ctx.hubs.deliver("s", event, data);
            assert_eq!(read_until(&mut stream, (event, data)).await, pairs(&[(event, data)]));
        }
        // A mesma vista de novo não sai.
        ctx.hubs.deliver("s", "plugin_ui", "u3");
        ctx.hubs.deliver("s", "state", "2");
        assert_eq!(read_until(&mut stream, ("state", "2")).await, pairs(&[("state", "2")]));
    }

    fn mods_view(band: &str, second: &str) -> String {
        serde_json::json!({"above": {"type": "Text", "children": [band]}, "panes": [
            {"id": "a", "tree": {"type": "Text", "children": ["painel grande"]}},
            {"id": "b", "tree": {"type": "Text", "children": [second]}}], "source": "terminal"}).to_string()
    }

    #[tokio::test]
    async fn device_that_announced_gets_only_what_changed_in_the_view() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, mut stream) = device_with(dir.path(), true).await;
        let first = mods_view("1", "x");
        ctx.hubs.deliver("s", "plugin_ui", &first);
        assert_eq!(read_until(&mut stream, ("plugin_ui", &first)).await.len(), 1, "a primeira vista sai inteira");
        ctx.hubs.deliver("s", "plugin_ui", &mods_view("2", "x"));
        ctx.hubs.deliver("s", "state", "1");
        let got = read_until(&mut stream, ("state", "1")).await;
        assert_eq!(got[0].0, "plugin_ui_delta");
        assert_eq!(serde_json::from_str::<serde_json::Value>(&got[0].1).unwrap(), serde_json::json!({
            "above": {"type": "Text", "children": ["2"]}, "panes": [{"id": "a", "same": true}, {"id": "b", "same": true}], "source": "terminal"}));
        // Só o segundo painel muda: a faixa igual não vai.
        ctx.hubs.deliver("s", "plugin_ui", &mods_view("2", "y"));
        ctx.hubs.deliver("s", "state", "2");
        let got = read_until(&mut stream, ("state", "2")).await;
        assert_eq!(serde_json::from_str::<serde_json::Value>(&got[0].1).unwrap(), serde_json::json!({
            "panes": [{"id": "a", "same": true}, {"id": "b", "tree": {"type": "Text", "children": ["y"]}}], "source": "terminal"}));
    }

    #[tokio::test]
    async fn device_that_skipped_a_view_or_did_not_announce_gets_it_whole() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, mut slow) = device_with(dir.path(), true).await;
        let (first, last) = (mods_view("1", "x"), mods_view("3", "x"));
        ctx.hubs.deliver("s", "plugin_ui", &first);
        read_until(&mut slow, ("plugin_ui", &first)).await;
        // Duas vistas enquanto ele não lê: a diferença da última é sobre a do meio, que ele não tem.
        ctx.hubs.deliver("s", "plugin_ui", &mods_view("2", "x"));
        ctx.hubs.deliver("s", "plugin_ui", &last);
        ctx.hubs.deliver("s", "state", "1");
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(read_until(&mut slow, ("state", "1")).await, pairs(&[("plugin_ui", &last), ("state", "1")]));

        let dir = tempfile::tempdir().unwrap();
        let (ctx, mut old) = device_with(dir.path(), false).await;
        for view in [&first, &last] {
            ctx.hubs.deliver("s", "plugin_ui", view);
            assert_eq!(read_until(&mut old, ("plugin_ui", view)).await, pairs(&[("plugin_ui", view)]));
        }
    }

    #[tokio::test]
    async fn device_that_arrives_gets_the_whole_view_then_differences() {
        let dir = tempfile::tempdir().unwrap();
        let (ctx, mut early) = device_with(dir.path(), true).await;
        let first = mods_view("1", "x");
        ctx.hubs.deliver("s", "plugin_ui", &first);
        read_until(&mut early, ("plugin_ui", &first)).await;
        // Quem entra agora recebe a vista inteira no retrato, e a próxima já como diferença sobre ela.
        let (tx, rx) = mpsc::channel::<Queued>(64);
        let lease = ctx.hubs.acquire("s", Binding { provider: crate::transcript::Provider::Claude, jsonl: dir.path().join("t.jsonl"),
            key: "k".into(), headless: false }, &ctx);
        let hub = lease.hub.clone();
        let receivers = hub.tx.receiver_count();
        tokio::spawn(client_loop(lease, None, tx));
        let mut late = Box::pin(device_stream(rx, hub.clone(), true));
        assert_eq!(read_until(&mut late, ("plugin_ui", &first)).await.last().unwrap().1, first);
        tokio::time::timeout(Duration::from_secs(5), async {
            while hub.tx.receiver_count() == receivers { tokio::time::sleep(Duration::from_millis(5)).await; }
        }).await.unwrap();
        ctx.hubs.deliver("s", "plugin_ui", &mods_view("2", "x"));
        ctx.hubs.deliver("s", "state", "1");
        assert_eq!(read_until(&mut late, ("state", "1")).await[0].0, "plugin_ui_delta");
    }

    #[test]
    fn cors_only_with_origin() {
        let mut resp = HeaderMap::new();
        cors(&HeaderMap::new(), &mut resp);
        assert!(resp.is_empty());
        let mut req = HeaderMap::new();
        req.insert(header::ORIGIN, HeaderValue::from_static("http://outra"));
        cors(&req, &mut resp);
        assert_eq!(resp[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(resp[header::ACCESS_CONTROL_EXPOSE_HEADERS], "ETag");
    }

    #[test]
    fn gzip_only_when_asked_and_large() {
        let big = vec![b'a'; 2048];
        let mut resp = HeaderMap::new();
        assert_eq!(maybe_gzip(&HeaderMap::new(), &mut resp, big.clone()), big);
        let mut req = HeaderMap::new();
        req.insert(header::ACCEPT_ENCODING, HeaderValue::from_static("gzip, deflate"));
        assert_eq!(maybe_gzip(&req, &mut resp, b"curto".to_vec()), b"curto");
        assert!(resp.get(header::CONTENT_ENCODING).is_none());
        let packed = maybe_gzip(&req, &mut resp, big.clone());
        assert_eq!(resp[header::CONTENT_ENCODING], "gzip");
        assert_eq!(resp[header::VARY], "Accept-Encoding");
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(&packed[..]).read_to_end(&mut out).unwrap();
        assert_eq!(out, big);

        let mut sse = HeaderMap::new();
        sse.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
        assert_eq!(maybe_gzip(&req, &mut sse, big.clone()), big);
        assert!(sse.get(header::CONTENT_ENCODING).is_none());
    }

    #[test]
    fn diag_req_prefers_header_then_valid_query() {
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=abc_1-Z")
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "abc_1-Z");
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=tem%20espaco")
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "");
        let long = "x".repeat(40);
        let r = axum::http::Request::builder()
            .uri("/api/sessions/s/events?diag_req=q")
            .header("x-hangar-req", long.as_str())
            .body(Body::empty())
            .unwrap();
        assert_eq!(diag_req(&r), "x".repeat(32));
    }
}
