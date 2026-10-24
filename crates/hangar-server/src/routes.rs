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
use tokio::net::TcpListener;
use tokio::sync::{broadcast, mpsc};

use crate::auth::{self, Auth};
use crate::config::Config;
use crate::proxy::{self, Forward, HttpClient};
use crate::share::caller::{Caller, SessionPath, caller};
use crate::share::pair_routes::PairState;
use crate::side::{Binding, Hub, Hubs, Lease, Out, Queued, SideCtx};
use crate::tail::{self, Watchers};
use crate::transcript::{InternalInfo, SKIPPED_LINES, history_etag, merged_history};

const PING_EVERY: Duration = Duration::from_secs(10);
const COMMENT_EVERY: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

pub struct AppState {
    pub interactions: Arc<crate::interactions::InteractionStore>,
    pub dictation_models: Arc<crate::dictation::catalog::Models>,
    /// Vagas para processos e chamadas de organização do ditado (catálogo sem cache e execução).
    pub dictation_slots: Arc<tokio::sync::Semaphore>,
    pub transcription: Arc<crate::transcription::service::TranscriptionService>,
    pub transcription_slots: Arc<tokio::sync::Semaphore>,
    pub accounts: crate::accounts::AccountService,
    pub command_catalog: Arc<crate::commands::Service>,
    pub cfg: Config,
    pub auth: Auth,
    pub http: HttpClient,
    pub side: SideCtx,
    pub terminal: crate::terminal_control::TerminalPool,
    pub terminal_address: Option<SocketAddr>,
    /// Segundo socket da porta pública, em `[::]` dual-stack; atende como o primeiro.
    pub public_v6: Option<TcpListener>,
    pub workspace_slots: Arc<tokio::sync::Semaphore>,
    pub workspace_read_slots: Arc<tokio::sync::Semaphore>,
    pub workspace_meta_slots: Arc<tokio::sync::Semaphore>,
    pub diag: crate::diag::DiagClient,
    pub costs: Arc<crate::costs::collect::Collector>,
    pub fx: Arc<crate::costs::fx::Fx>,
    pub reports: Arc<crate::costs::ReportCache>,
    pub origins_home: std::path::PathBuf,
    /// `~/.hangar`: etapas de ferramenta e cofre de anexos; o teste troca por uma pasta temporária.
    pub hangar_dir: std::path::PathBuf,
    /// Relógio e sufixo dos nomes do cofre de anexos; o teste de contrato fixa os da referência.
    pub upload_naming: crate::uploads::store::Naming,
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
    pub plugin: Arc<crate::plugin::Store>,
    /// Páginas HTML publicadas pelas sessões (`~/.hangar/paginas`); o teste troca por uma pasta temporária.
    pub pages: Arc<crate::pages::store::Store>,
    /// Diagramas Mermaid já desenhados (`~/.hangar/mermaid`); o teste troca por uma pasta temporária.
    pub mermaid: Arc<crate::mermaid::cache::Cache>,
    /// Metadados passivos dos artefatos Claude, com leitura incremental por conversa.
    pub artifacts: Arc<crate::artifacts::Catalogs>,
    /// Onde está o Chromium do servidor, perguntado a cada medição; o teste força "ausente".
    pub chromium: fn() -> Option<std::path::PathBuf>,
    /// Quanto uma escrita espera a porta de entrada da sessão reabrir (os testes encurtam).
    pub write_gate_wait: std::time::Duration,
    /// O `claude` e o `hangar-engine` do fork do `/btw` (os testes trocam por falsos).
    pub btw_programs: crate::session_write::btw::Programs,
    /// Grupos de sessões em `.hangar-pair`; `None` sem as pastas da lista (as rotas seguem ao Python).
    pub groups: Option<Arc<crate::groups::service::GroupService>>,
    /// Outras máquinas do dono (`peers.json`), para os grupos entre máquinas.
    pub peers: Arc<crate::groups::peers::PeerClient>,
    /// Id desta máquina entre as do dono (`CP_SERVER_ID`); vazio = sem id.
    pub server_id: String,
    /// Voz ao vivo; o teste troca por um hub com pastas temporárias.
    pub voice: Arc<crate::voice::hub::VoiceHub>,
    /// Convites e par externo; `None` sem `HANGAR_SHARES_FILE` (o Python segue dono).
    pub share: Option<Arc<crate::share::ShareState>>,
    /// A porta do convidado já está escutando; a saúde anuncia.
    pub guest_bound: std::sync::atomic::AtomicBool,
    /// Instalar o SideStore no iPhone/iPad do cabo; o teste troca aparelho e Apple por falsos.
    pub device_install: Arc<crate::device_install::job::DeviceInstall>,
    /// Reservas pré-aquecidas do Claude sem terminal (tela de nova conversa aberta).
    pub prewarm: Arc<crate::prewarm::Store>,
}

impl AppState {
    /// Ligação concreta das fontes das nove rotas do plugin.
    pub fn plugin_route_state(self: &Arc<Self>) -> Arc<crate::plugin::routes::RouteState> {
        Arc::new(crate::plugin::routes::RouteState { store: self.plugin.clone(), secret: self.cfg.auth_token.clone(),
            trusted: self.cfg.trusted.clone(), sources: Arc::new(crate::plugin::sources::NativeSources::new(self)) })
    }

    pub fn new(cfg: Config) -> AppState {
        Self::with_server_id(cfg, String::new())
    }

    /// `server_id` = `HANGAR_SERVER_ID`, lido só no `main`.
    pub fn with_server_id(cfg: Config, server_id: String) -> AppState {
        Self::build(cfg, crate::terminal_control::TerminalPool::new(), server_id)
    }

    pub fn with_terminal_pool(cfg: Config, terminal: crate::terminal_control::TerminalPool) -> AppState {
        Self::build(cfg, terminal, String::new())
    }

    fn build(cfg: Config, terminal: crate::terminal_control::TerminalPool, server_id: String) -> AppState {
        let costs = Arc::new(crate::costs::collect::Collector::new(
            crate::costs::index::default_dir(), crate::costs::pricing::default_dir(),
            crate::costs::areas::default_map_file(),
            Arc::new(crate::costs::collect::HttpScopes::new(cfg.upstream, cfg.internal_secret.clone())),
        ));
        Self::with_parts(cfg, terminal, costs, Arc::new(crate::costs::fx::Fx::new()), server_id)
    }

    pub fn with_parts(cfg: Config, terminal: crate::terminal_control::TerminalPool,
                      costs: Arc<crate::costs::collect::Collector>, fx: Arc<crate::costs::fx::Fx>, server_id: String) -> AppState {
        let http = proxy::client();
        let mods = crate::mods::state::Mods::default();
        let interactions = Arc::new(crate::interactions::InteractionStore::default());
        let plugin = Arc::new(crate::plugin::Store::new(interactions.clone()));
        let facts = crate::list::facts::FactsClient::new(cfg.upstream, cfg.internal_secret.clone());
        let list = Arc::new(crate::list::bridge::ListBridge::new(crate::list::bridge::ListEnv::from_env(), facts));
        let mut side = SideCtx {
            watchers: Watchers::default(),
            hubs: Hubs::default(),
            locate: Arc::new(crate::locate::ListLocate { list: list.clone() }),
            monitors: None,
            mods: mods.clone(),
            runtime: Default::default(),
            queue_dir: list.env().dirs.as_ref().map(|dirs| dirs.claude.join(".hangar-queue")),
            problem_hold: crate::problem_gate::HOLD,
            shown_problems: list.shown_problems(),
            diag: crate::diag::DiagClient::new(cfg.upstream, cfg.internal_secret.clone()),
        };
        mods.bind_hubs(side.hubs.downgrade());
        let diag = side.diag.clone();
        let state = Arc::new(crate::state::live::StateEnv::new(terminal.clone(), list.clone(), diag.clone()));
        list.set_permissions(state.permissions.clone());
        side.monitors = Some(crate::state::live::spawner(state.clone()));
        let share = match crate::config::GuestEnv::from_env() {
            Ok(guest) => crate::share::from_env(guest.shares_file, list.env().dirs.as_ref(), list.env().capture_program.clone()),
            Err(e) => { tracing::error!("{e}"); None }
        };
        if let Some(share) = &share { list.set_shares(share.clone()); }
        let hangar_dir = std::path::PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()).join(".hangar");
        let device_install = {
            let env = crate::accounts::environment::AccountEnvironment::capture();
            Arc::new(crate::device_install::job::DeviceInstall::new(
                Arc::new(crate::device_install::UsbLink::new(&hangar_dir)), Arc::new(crate::device_install::apple::IsideloadSigner), &hangar_dir)
                .with_config(Arc::new(move || crate::runtime::codex_launch::runtime_config(&env))))
        };
        let groups = crate::groups::from_env(list.env().dirs.as_ref(), server_id.clone(),
            Arc::new(crate::groups::orq::PythonOrq::new(cfg.upstream, cfg.internal_secret.clone(), http.clone())), list.clone(), diag.clone());
        AppState { accounts: crate::accounts::AccountService::new(crate::accounts::environment::AccountEnvironment::capture()), command_catalog: Arc::default(), groups, server_id, peers: Arc::new(crate::groups::peers_from_env()), auth: Auth::new(&cfg.auth_token), http, side, cfg, terminal, terminal_address: None, public_v6: None, diag,
            workspace_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            interactions, plugin,
            transcription: Arc::default(),
            dictation_models: Arc::default(),
            dictation_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            transcription_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            workspace_read_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            workspace_meta_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            costs, fx, reports: Arc::new(crate::costs::ReportCache::default()),
            origins_home: std::path::PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()),
            prewarm: Arc::new(crate::prewarm::Store::server(&hangar_dir)),
            hangar_dir, device_install,
            upload_naming: Default::default(),
            origins: std::sync::Mutex::new(indexmap::IndexMap::new()),
            list, state,
            hub: Arc::default(),
            term: Arc::default(),
            mods,
            pages: Arc::new(crate::pages::store::Store::new(crate::pages::store::Store::default_root())),
            mermaid: Arc::new(crate::mermaid::cache::Cache::new(crate::mermaid::cache::Cache::default_root())),
            artifacts: Arc::default(),
            chromium: crate::pages::chrome::find,
            write_gate_wait: std::time::Duration::from_secs(30),
            btw_programs: Default::default(),
            voice: Arc::new(crate::voice::hub::VoiceHub::new()),
            share, guest_bound: Default::default() }
    }

    pub(crate) fn skill_origins(&self, repo: &std::path::Path) -> crate::costs::origins::Origins {
        let mut origins = self.origins.lock().unwrap();
        let cache = origins.shift_remove(repo).unwrap_or_else(|| crate::costs::origins::Origins::new(self.origins_home.clone(), repo.to_owned()));
        origins.insert(repo.to_owned(), cache.clone());
        while origins.len() > 8 { origins.shift_remove_index(0); }
        cache
    }

    /// Qual conversa é a da sessão, pela descoberta do Rust. `Ok(None)`: sessão inexistente.
    pub(crate) async fn info(&self, name: &str) -> Result<Option<InternalInfo>, InfoFailed> {
        let found = self.side.info(name).await;
        if let Err(InfoFailed(code)) = found {
            tracing::warn!(session = %name, code, "descoberta da sessão falhou");
        }
        found
    }

    /// Abrir a conversa reabre a sessão em erro, aqui no registro do runtime.
    pub(crate) fn request_reopen(&self, name: &str) {
        if let Some(runtime) = self.state.runtime.get() { runtime.request_reopen(name); }
    }
}

pub(crate) use crate::locate::InfoFailed;

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

pub async fn serve_with_state(listener: TcpListener, state: AppState) -> std::io::Result<()> {
    serve_with_guest(listener, state, None).await
}

/// `guest`: endereço da porta do convidado. Ocupada não derruba a principal: tenta até soltar.
pub async fn serve_with_guest(listener: TcpListener, mut state: AppState, guest: Option<SocketAddr>) -> std::io::Result<()> {
    let guest = guest.filter(|addr| state.share.is_some() || {
        tracing::error!(listen = %addr, "porta do convidado pedida sem HANGAR_SHARES_FILE: fica fechada");
        false
    });
    let plugin = crate::plugin_listener::bind(&listener).await?;
    // Bind LAN específico não recebe tráfego de loopback: a observação tem uma porta própria.
    let private = TcpListener::bind("127.0.0.1:0").await?;
    state.terminal_address = Some(private.local_addr()?);
    state.voice.set_self(listener.local_addr()?, &state.cfg.auth_token);
    let public_v6 = state.public_v6.take();
    let state = Arc::new(state);
    let _worktree_sweep=crate::AbortOnDrop(crate::worktree_policy::spawn(state.clone()));
    let v6_state = state.clone();
    // Abortada na saída: a tarefa segura o estado do servidor, que sobreviveria a ele.
    // As sessões com terminal que o multiplexador perdeu voltam pelo spec; antes da varredura de grupos, que
    // conta como vivos os nomes à espera dele.
    let _session_restore = crate::session_open::restore::spawn(state.clone()).map(crate::AbortOnDrop);
    let _group_sweep = crate::groups::sweep::spawn(state.clone()).map(crate::AbortOnDrop);
    let _link_return = crate::groups::link_return::spawn(state.clone()).map(crate::AbortOnDrop);
    let _federation = crate::groups::federation::spawn(state.clone()).map(crate::AbortOnDrop);
    // O aviso no celular é decidido aqui, com ou sem lista aberta.
    let _push_watch: Vec<crate::AbortOnDrop> = crate::push::start(state.clone()).into_iter().map(crate::AbortOnDrop).collect();
    // Assina as transições depois do vigia de avisos, que as publica.
    let _loop_watch: Vec<crate::AbortOnDrop> = crate::loops::watch::start(state.clone()).into_iter().map(crate::AbortOnDrop).collect();
    let _limit_switch = crate::session_write::limit_switch::spawn(state.clone()).map(crate::AbortOnDrop);
    // Sem a fotografia do Python a varredura não revoga nada (ver `life::sweep`); o túnel religa já.
    let _share_sweep = state.share.clone().map(|share| crate::AbortOnDrop(tokio::spawn(
        crate::share::owner_routes::sweep_loop(share, crate::share::owner_routes::SWEEP_EVERY))));
    let plugin_state = state.clone();
    let guest_state = state.clone();
    // Uma só para as portas: convite e par veem as mesmas lojas e a mesma conferência.
    let pair = pair_state(&state);
    let guest_pair = pair.clone();
    tokio::select! {
        result = async {
            match guest {
                Some(addr) => serve_guest(addr, guest_state, guest_pair).await,
                None => std::future::pending::<std::io::Result<()>>().await,
            }
        } => result,
        result = axum::serve(listener.tap_io(crate::nodelay), router_with(state.clone(), pair.as_ref()).into_make_service_with_connect_info::<SocketAddr>()) => result,
        result = axum::serve(private.tap_io(crate::nodelay), terminal_router_with(state, pair.as_ref()).into_make_service_with_connect_info::<SocketAddr>()) => result,
        result = async {
            match public_v6 {
                Some(v6) => axum::serve(v6.tap_io(crate::nodelay), router_with(v6_state, pair.as_ref()).layer(axum::middleware::map_request(ipv4_peer))
                    .into_make_service_with_connect_info::<SocketAddr>()).await,
                None => std::future::pending::<std::io::Result<()>>().await,
            }
        } => result,
        result = async {
            match plugin {
                Some(plugin) => axum::serve(plugin.tap_io(crate::nodelay), plugin_router(plugin_state).into_make_service_with_connect_info::<SocketAddr>()).await,
                None => std::future::pending::<std::io::Result<()>>().await,
            }
        } => result,
    }
}

/// No socket dual-stack o cliente IPv4 chega como `::ffff:a.b.c.d`; quem decide "local" e proxy confiável compara IPv4.
async fn ipv4_peer(mut req: axum::extract::Request) -> axum::extract::Request {
    if let Some(ConnectInfo(peer)) = req.extensions_mut().get_mut::<ConnectInfo<SocketAddr>>()
        && let std::net::IpAddr::V6(ip) = peer.ip()
        && let Some(v4) = ip.to_ipv4_mapped() {
        peer.set_ip(v4.into());
    }
    req
}

async fn serve_guest(addr: SocketAddr, state: Arc<AppState>, pair: Option<Arc<PairState>>) -> std::io::Result<()> {
    let listener = bind_guest(addr).await;
    state.guest_bound.store(true, Ordering::Relaxed);
    axum::serve(listener, guest_router_with(state, pair).into_make_service_with_connect_info::<crate::share::gate::GuestConn>()).await
}

/// O filho anterior pode ainda segurar a porta: tenta para sempre, um aviso por mudança de estado.
async fn bind_guest(addr: SocketAddr) -> TcpListener {
    let mut wait = Duration::from_millis(250);
    let mut failing = false;
    loop {
        match guest_socket(addr) {
            Ok(listener) => {
                tracing::info!(listen = %addr, "porta do convidado de pé");
                return listener;
            }
            Err(e) => {
                if !failing {
                    tracing::warn!(listen = %addr, "porta do convidado indisponível, tentando de novo: {e}");
                    failing = true;
                }
                tokio::time::sleep(wait).await;
                wait = (wait * 2).min(Duration::from_secs(5));
            }
        }
    }
}

fn guest_socket(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let socket = if addr.is_ipv4() { tokio::net::TcpSocket::new_v4()? } else { tokio::net::TcpSocket::new_v6()? };
    #[cfg(unix)]
    socket.set_reuseaddr(true)?;
    // ponytail: o `GuestConn` só casa com `TcpListener` puro (sem `tap_io`); a conexão aceita herda
    // o TCP_NODELAY do listener. Sistema que não herdar precisa de laço de accept próprio.
    socket.set_nodelay(true)?;
    socket.bind(addr)?;
    socket.listen(1024)
}

/// Roteador da 8766: tudo passa pelo porteiro, nada pergunta `gate`/`is_owner`.
pub fn guest_router(state: Arc<AppState>) -> Router {
    let pair = pair_state(&state);
    guest_router_with(state, pair)
}

fn guest_router_with(state: Arc<AppState>, pair: Option<Arc<PairState>>) -> Router {
    let Some(pair) = pair else {
        return Router::new().fallback(|headers: HeaderMap| async move { not_found(&headers) }).with_state(state);
    };
    let share = pair.owner.share.clone();
    // Os handlers do dono que também servem o convidado; o resto vai ao Python assinado.
    session_routes(pass_any).fallback(pass_any).with_state(state)
        .merge(crate::share::guest_routes::guest_router(share.clone(), crate::share::guest_routes::owner_name()))
        .merge(crate::share::pair_routes::guest_pair_router(pair))
        .layer(axum::middleware::from_fn_with_state(Arc::new(crate::share::gate::Gate::new(share)), crate::share::gate::guard))
}

/// Lojas, porta do dono e conferência pela internet que convite e par compartilham; `None` sem
/// `HANGAR_SHARES_FILE`.
fn pair_state(state: &Arc<AppState>) -> Option<Arc<PairState>> {
    use crate::share::{owner_routes, pair_routes};
    let share = state.share.clone()?;
    let owner = Arc::new(owner_routes::OwnerShareState { share, owner_port: state.cfg.listen.port(),
        bind_ip: state.cfg.listen.ip().to_string(), net: Arc::new(owner_routes::RealNet), reach: owner_routes::Reach::production() });
    Some(Arc::new(pair_routes::PairState { app: state.clone(), owner, remote: Arc::new(pair_routes::Direct) }))
}

/// Convites e par externo do dono na 8765; quem não é o dono segue ao Python, que recusa como antes.
fn owner_share_router(state: &Arc<AppState>, pair: Option<&Arc<PairState>>) -> Router {
    let Some(pair) = pair else { return Router::new() };
    crate::share::owner_routes::owner_router(pair.owner.clone())
        .merge(crate::share::pair_routes::owner_pair_router(pair.clone()))
        .layer(axum::middleware::from_fn_with_state(state.clone(), owner_only))
}

/// Os mesmos handlers pelo caminho privado, para o dono que chega pelo Connect.
fn private_share_router(state: &Arc<AppState>, pair: Option<&Arc<PairState>>) -> Router {
    let Some(pair) = pair else { return Router::new() };
    crate::share::owner_routes::private_router(pair.owner.clone())
        .merge(crate::share::pair_routes::private_pair_router(pair.clone()))
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::share::bridge::private_only))
}

async fn owner_only(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request,
                    next: axum::middleware::Next) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    // O preflight do CORS também é do Python.
    if !owner || req.method() == Method::OPTIONS {
        return pass(&st, req, &fwd).await;
    }
    let headers = req.headers().clone();
    let mut resp = next.run(req).await;
    cors(&headers, resp.headers_mut());
    resp
}

/// Rotas cujo handler sabe atender o dono e o convidado; `fallback` é o repasse de cada porta.
fn session_routes<H, T>(fallback: H) -> Router<Arc<AppState>>
where H: axum::handler::Handler<T, Arc<AppState>>, T: 'static {
    Router::new()
        .route("/api/sessions/{name}/term", get(crate::term::session_ws).fallback(fallback.clone()))
        // Outro método nessas rotas (preflight OPTIONS, HEAD) segue ao Python.
        .route("/api/sessions", get(crate::list::hub::list).post(crate::session_open::create).fallback(fallback.clone()))
        .route("/api/sessions/events", get(crate::list::hub::events).fallback(fallback.clone()))
        .route("/api/sessions/{name}/history", get(history).fallback(fallback.clone()))
        .route("/api/sessions/{name}/events", get(events).fallback(fallback.clone()))
        // Interface dos mods: o Rust atende a sessão sem terminal dele; o resto segue ao Python.
        .route("/api/sessions/{name}/plugin/press", axum::routing::post(crate::mods::routes::press).fallback(fallback.clone()))
        .route("/api/sessions/{name}/plugin/close", axum::routing::post(crate::mods::routes::close).fallback(fallback.clone()))
        .route("/api/sessions/{name}/plugin/show", axum::routing::post(crate::mods::routes::show).fallback(fallback.clone()))
        .route("/api/sessions/{name}/plugin/input", axum::routing::post(crate::mods::routes::input).fallback(fallback.clone()))
        // Escritas Claude: o Rust decide por pedido e repassa o que não é dele (corpo intacto).
        .route("/api/sessions/{name}/input", axum::routing::post(crate::session_write::input::input).fallback(fallback.clone()))
        .route("/api/jev/delivery", get(crate::session_write::delivery_timing::availability).fallback(fallback.clone()))
        .route("/api/sessions/{name}/steer", axum::routing::post(crate::session_write::input::steer).fallback(fallback.clone()))
        .route("/api/sessions/{name}/interrupt", axum::routing::post(crate::session_write::control::interrupt).fallback(fallback.clone()))
        .route("/api/sessions/{name}/select", axum::routing::post(crate::session_write::control::select).fallback(fallback.clone()))
        .route("/api/sessions/{name}/select/submit", axum::routing::post(crate::session_write::control::select_submit).fallback(fallback.clone()))
        .route("/api/sessions/{name}/answer", axum::routing::post(crate::session_write::answer::answer).fallback(fallback.clone()))
        .route("/api/sessions/{name}/keys", axum::routing::post(crate::session_write::control::keys).fallback(fallback.clone()))
        .route("/api/sessions/{name}/term-input", axum::routing::post(crate::session_write::control::term_input).fallback(fallback.clone()))
        // Pergunta lateral: inteira no Rust, com e sem terminal; o convidado completo também.
        .route("/api/sessions/{name}/btw", axum::routing::post(crate::session_write::btw::ask).get(crate::session_write::btw::list).fallback(fallback.clone()))
        // Painel da fila. `reorder` é caminho fixo: o DELETE e o PATCH dele valem como os de um id de mesmo nome.
        .route("/api/sessions/{name}/queue/reorder", axum::routing::post(crate::session_write::control::queue_reorder)
            .delete(crate::session_write::control::queue_remove).patch(crate::session_write::control::queue_edit).fallback(fallback.clone()))
        .route("/api/sessions/{name}/queue/{entry_id}", axum::routing::delete(crate::session_write::control::queue_remove)
            .patch(crate::session_write::control::queue_edit).fallback(fallback.clone()))
        .route("/api/sessions/{name}/queue/{entry_id}/steer", axum::routing::post(crate::session_write::control::queue_steer).fallback(fallback.clone()))
        // Rotas só do Codex: o Rust atende a sessão sem terminal dele; o resto segue ao Python.
        .route("/api/sessions/{name}/models", get(crate::session_write::codex::models).fallback(fallback.clone()))
        .route("/api/sessions/{name}/model", axum::routing::post(crate::session_write::codex::model).fallback(fallback.clone()))
        .route("/api/sessions/{name}/service-tier", axum::routing::post(crate::session_write::engine::service_tier).fallback(fallback.clone()))
        .route("/api/sessions/{name}/codex/mode", axum::routing::post(crate::session_write::codex::mode).fallback(fallback.clone()))
        .route("/api/sessions/{name}/limits", get(crate::session_write::codex::limits).fallback(fallback.clone()))
        .route("/api/sessions/{name}/question/skip", axum::routing::post(crate::session_write::codex::skip_question).fallback(fallback.clone()))
        .route("/api/orq/{exec_id}/conductor", get(crate::orq_conductor::public).fallback(fallback.clone()))
        .route("/api/sessions/{name}/commands", get(crate::commands::route).fallback(fallback.clone()))
        .route("/api/sessions/{name}/codex-permissions", get(crate::session_write::codex::permissions).fallback(fallback.clone()))
        // Mesmo caminho: o axum junta o POST à rota de cima (um repasse só, o dela).
        .route("/api/sessions/{name}/codex-permissions", axum::routing::post(crate::session_write::codex::set_permission))
        .route("/api/sessions/{name}/transcribe", axum::routing::post(crate::uploads::transcribe::route).fallback(fallback.clone()))
        .route("/api/sessions/{name}/bash-output", axum::routing::post(crate::session_media::bash_output_route).fallback(fallback.clone()))
        .route("/api/sessions/{name}/tool-progress/{id}", get(crate::session_media::tool_progress).fallback(fallback.clone()))
        .route("/api/sessions/{name}/transcript-image/{uuid}/{idx}", get(crate::session_media::transcript_image).fallback(fallback.clone()))
        .route("/api/sessions/{name}/cost", get(crate::costs_routes::session_cost).fallback(fallback))
}

pub fn terminal_router(state: Arc<AppState>) -> Router {
    let pair = pair_state(&state);
    terminal_router_with(state, pair.as_ref())
}

fn terminal_router_with(state: Arc<AppState>, pair: Option<&Arc<PairState>>) -> Router {
    let router = Router::new()
        .route("/__hangar_server/terminal", axum::routing::post(crate::terminal_routes::terminal))
        .route("/__hangar_server/workspace", axum::routing::post(crate::workspace_routes::private))
        .route("/__hangar_server/transcription/{operation}", axum::routing::post(crate::transcription::routes::private))
        .route("/__hangar_server/dictation/{operation}", axum::routing::post(crate::dictation::routes::private))
        .route("/__hangar_server/claude/customizations", axum::routing::post(crate::claude_customizations::private))
        .route("/__hangar_server/forwarded", axum::routing::any(crate::forwarded::private))
        .route("/__hangar_server/list", axum::routing::post(crate::list::bridge::private))
        .route("/__hangar_server/accounts", axum::routing::post(crate::accounts::http::private))
        .route("/__hangar_server/quotas", axum::routing::post(crate::accounts::quotas::private))
        .route("/__hangar_server/accounts/claude", axum::routing::post(crate::accounts::http::private_claude))
        .route("/__hangar_server/accounts/public", axum::routing::any(crate::accounts::http::private_public))
        .route("/__hangar_server/groups", axum::routing::post(crate::groups::bridge::private))
        .route("/__hangar_server/push/expo", axum::routing::post(crate::push_expo::private))
        .route("/__hangar_server/codex/{*rest}", axum::routing::any(crate::session_write::bridge::codex))
        .route("/__hangar_server/claude/{*rest}", axum::routing::any(crate::session_write::bridge::claude))
        .route("/__hangar_server/presence", axum::routing::get(crate::presence::private))
        .route("/__hangar_server/mods/{name}/{op}", axum::routing::post(crate::mods::routes::bridge))
        .route("/__hangar_server/voice/settings", axum::routing::any(crate::voice::routes::private_settings))
        .route("/__hangar_server/jev/aux", axum::routing::any(crate::jev_aux::private))
        .route("/__hangar_server/voice/memory", axum::routing::any(crate::voice::routes::private_memory))
        .route("/__hangar_server/voice/memory/{id}", axum::routing::any(crate::voice::routes::private_memory))
        .route("/__hangar_server/subagents", get(crate::subagents::private))
        .route("/__hangar_server/share/reachability", axum::routing::post(crate::share_reach::private))
        .route("/__hangar_server/share", axum::routing::post(crate::share::bridge::private))
        .layer(axum::middleware::from_fn(crate::migration_status::count_bridge));
    // Painel, canal do estado e voz ficam fora da contagem: conexões longas, não chamadas da ponte.
    let share = private_share_router(&state, pair);
    router.route("/__hangar_server/term", get(crate::term::private_ws))
        .route("/__hangar_server/state/{name}/events", get(crate::side::private_events))
        .route("/__hangar_server/voice", get(crate::voice::routes::private_ws))
        .with_state(state)
        .merge(share)
}

pub fn router(state: Arc<AppState>) -> Router {
    let pair = pair_state(&state);
    router_with(state, pair.as_ref())
}

fn router_with(state: Arc<AppState>, pair: Option<&Arc<PairState>>) -> Router {
    let api = public_routes(state.clone(), pair);
    // O MCP chama as tools pelas mesmas rotas que o `hangar-send` chama: o roteador público inteiro.
    api.clone().merge(crate::mcp::router(state, api))
}

fn public_routes(state: Arc<AppState>, pair: Option<&Arc<PairState>>) -> Router {
    session_routes(pass_any)
        .route("/api/hangar-terminals/{ident}/term", get(crate::term::hangar_ws).fallback(pass_any))
        .route("/__hangar_server/health", get(health))
        .route("/api/client-api", get(client_api).fallback(pass_any))
        // As rotas privadas não existem nesta porta.
        .route("/__hangar_server/{*rest}", axum::routing::any(|| async { StatusCode::NOT_FOUND }))
        .route("/api/sessions/{name}/artifacts", get(crate::artifacts::route).fallback(pass_any))
        .route("/api/sessions/{name}/subagents", get(crate::subagents::public).fallback(pass_any))
        .route("/api/sessions/{name}/subagents/{agent_id}", get(crate::subagents::public).fallback(pass_any))
        .route("/api/sessions/{name}/subagents/{agent_id}/message", axum::routing::post(crate::subagents::message).fallback(pass_any))
        .merge(plugin_routes(&state))
        // Páginas HTML da conversa: só o dono; o convidado recebe o 404 de hoje.
        .route("/api/sessions/{name}/pages/{id}", get(crate::pages::routes::page).fallback(pass_any))
        .route("/api/sessions/{name}/pages/{id}/shot", get(crate::pages::routes::shot).fallback(pass_any))
        // Diagramas Mermaid desenhados no Chromium do servidor: só o dono; o resto segue ao Python, que repassa
        // o dono do Connect pela ponte (`forwarded`) e recusa os demais.
        .route("/api/mermaid", axum::routing::post(crate::mermaid::route).fallback(pass_any))
        // Encerrar: o Codex com terminal do Rust encerra aqui e só então segue ao Python, marcado.
        .route("/api/sessions/{name}", axum::routing::delete(crate::session_write::bridge::delete).fallback(pass_any))
        .route("/api/sessions/creation-progress", get(crate::session_open::server::creation_progress).fallback(pass_any))
        // Reserva pré-aquecida do Claude sem terminal: só o dono; outro método segue ao Python.
        .route("/api/sessions/prewarm", axum::routing::put(crate::prewarm::http::put).fallback(pass_any))
        .route("/api/sessions/prewarm/{id}", axum::routing::delete(crate::prewarm::http::delete).fallback(pass_any))
        // Pedido de navegador do agente: só o dono; o Connect chega pela ponte (`forwarded`).
        .route("/api/sessions/{name}/nav", axum::routing::post(crate::nav_requests::open).delete(crate::nav_requests::confirm).fallback(pass_any))
        // Loop e `then` do dono; o convidado segue ao Python.
        .route("/api/sessions/{name}/then", axum::routing::put(crate::loops::http::route).delete(crate::loops::http::route).fallback(pass_any))
        .route("/api/sessions/{name}/loop", get(crate::loops::http::route).post(crate::loops::http::route).delete(crate::loops::http::route).fallback(pass_any))
        .route("/api/sessions/{name}/loop/refine", axum::routing::post(crate::loops::http::route).fallback(pass_any))
        .route("/api/sessions/{name}/loop/resolve", axum::routing::post(crate::loops::http::route).fallback(pass_any))
        // Bastão do dono: o Python prepara o dossiê e a sucessora Claude com terminal nasce aqui.
        .route("/api/sessions/{name}/bastao", axum::routing::post(crate::session_open::server::bastao).fallback(pass_any))
        // Retomar do Arquivo e o `/resume` ao vivo: a abertura e a reabertura do Rust, com a conversa que já existe.
        .route("/api/archive/{project}/{session_id}/resume", axum::routing::post(crate::session_open::server::archive_resume).fallback(pass_any))
        .route("/api/sessions/{name}/resume", axum::routing::post(crate::session_write::resume::route).fallback(pass_any))
        // Broadcast do dono: o alvo Codex com terminal do Rust recebe aqui; o resto segue ao Python.
        .route("/api/broadcast", axum::routing::post(crate::session_write::input::broadcast).fallback(pass_any))
        .route("/api/sessions/{name}/permission-modes", get(crate::session_write::permissions::modes).fallback(pass_any))
        .route("/api/sessions/{name}/permission-mode", axum::routing::post(crate::session_write::permissions::set_mode).fallback(pass_any))
        .route("/api/sessions/{name}/codex/plan/implement", axum::routing::post(crate::session_write::codex::plan_implement).fallback(pass_any))
        // Relançar o Codex do Rust (troca de modo e reinício) é dele do começo ao fim; o resto segue ao Python.
        .route("/api/sessions/{name}/modo-execucao", axum::routing::post(crate::session_write::relaunch::mode).fallback(pass_any))
        .route("/api/sessions/{name}/recarregar", axum::routing::post(crate::session_write::relaunch::reload).fallback(pass_any))
        // Trocar de conta (e a conta ChatGPT fixa, o modelo e o esforço dela) reabre o Claude pelo spec no Rust.
        .route("/api/sessions/{name}/conta", get(crate::session_write::account::targets_route).fallback(pass_any))
        .route("/api/sessions/{name}/conta", axum::routing::post(crate::session_write::account::switch_route))
        .route("/api/sessions/{name}/engine/model", axum::routing::post(crate::session_write::engine::model).fallback(pass_any))
        .route("/api/sessions/{name}/model-effort", axum::routing::post(crate::session_write::engine::effort).fallback(pass_any))
        .route("/api/costs", get(crate::costs_routes::costs).fallback(pass_any))
        .route("/api/cotacao", get(crate::costs_routes::cotacao).fallback(pass_any))
        .route("/api/uso", get(crate::costs_routes::usage).fallback(pass_any))
        .route("/api/migration/status", get(crate::migration_status::status).fallback(pass_any))
        .route("/api/claude/defaults", axum::routing::post(crate::claude_defaults::save).fallback(pass_any))
        .route("/api/session-defaults", axum::routing::get(crate::session_defaults::handle).put(crate::session_defaults::handle))
        .route("/api/push/expo/subscribe", axum::routing::post(crate::push_expo::subscribe))
        .route("/api/push/vapid", get(crate::push::http::route).fallback(pass_any))
        .route("/api/push/settings", get(crate::push::http::route).fallback(pass_any))
        .route("/api/push/subscribe", axum::routing::post(crate::push::http::route).fallback(pass_any))
        .route("/api/push/mute", axum::routing::post(crate::push::http::route).fallback(pass_any))
        .route("/api/push/quiet-hours", axum::routing::post(crate::push::http::route).fallback(pass_any))
        .route("/api/presence", get(crate::presence::route).fallback(pass_any))
        // Mesmo caminho: o axum junta o POST à rota de cima (um repasse só, o dela).
        .route("/api/presence", axum::routing::post(crate::presence::route))
        .route("/api/presence/heartbeat", axum::routing::post(crate::presence::route).fallback(pass_any))
        .route("/api/jev/aux/cliproxy", get(crate::jev_aux::cliproxy).fallback(pass_any))
        .route("/api/jev/aux/models", axum::routing::post(crate::jev_aux::models).fallback(pass_any))
        .route("/api/voice", get(crate::voice::routes::ws).fallback(pass_any))
        .route("/api/voice/settings", get(crate::voice::routes::settings_route).fallback(pass_any))
        // Mesmo caminho: o axum junta o PUT à rota de cima (um repasse só, o dela).
        .route("/api/voice/settings", axum::routing::put(crate::voice::routes::settings_route))
        .route("/api/voice/memory", get(crate::voice::routes::memory_route).fallback(pass_any))
        .route("/api/voice/memory/{id}", axum::routing::delete(crate::voice::routes::memory_route).fallback(pass_any))
        // Grupos (`/pair`, `/group-message`, `/pair/contract`, `/pair-remote`, `/unpair-remote`).
        .merge(crate::groups::routes::router())
        // Planos e catálogo de plugins e skills: o dono no Rust; o resto segue ao Python.
        .merge(crate::plans::routes::router())
        .route("/api/claude/customizations", get(crate::claude_customizations::catalog_route).fallback(pass_any))
        // SideStore no iPhone/iPad do cabo: só o dono, nunca repassado (a senha da Apple ID não vai ao Python).
        .route("/api/device-install/devices", get(crate::device_install::routes::devices))
        .route("/api/device-install/progress", get(crate::device_install::routes::progress))
        .route("/api/device-install/trust", axum::routing::post(crate::device_install::routes::trust))
        .route("/api/device-install/start", axum::routing::post(crate::device_install::routes::start))
        .route("/api/device-install/code", axum::routing::post(crate::device_install::routes::code))
        .route("/api/device-install/certs", axum::routing::post(crate::device_install::routes::certs))
        .fallback(pass_any)
        .layer(axum::middleware::from_fn(crate::migration_status::count_public))
        .with_state(state.clone())
        .merge(owner_share_router(&state, pair))
}

/// A ponte do plugin do Claude (`plugin::routes`) é do Rust inteira; a interface dos mods também.
fn plugin_routes(state: &Arc<AppState>) -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/plugin/press-start", axum::routing::post(crate::mods::bridge::press_start).fallback(pass_any))
        .route("/api/plugin/opened", axum::routing::post(crate::mods::bridge::opened).fallback(pass_any))
        .route("/api/plugin/ui", axum::routing::post(crate::mods::bridge::ui).fallback(pass_any))
        .route("/api/plugin/toast", axum::routing::post(crate::mods::bridge::toast).fallback(pass_any))
        .route("/api/plugin/pressed", axum::routing::post(crate::mods::bridge::pressed).fallback(pass_any))
        .route("/api/plugin/copied", axum::routing::post(crate::mods::bridge::copied).fallback(pass_any))
        .route("/api/plugin/focus-target", axum::routing::post(crate::mods::bridge::focus_target).fallback(pass_any))
        .route("/api/plugin/focused", axum::routing::post(crate::mods::bridge::focused).fallback(pass_any))
        .route("/api/plugin/scroll", axum::routing::post(crate::mods::bridge::scroll).fallback(pass_any))
        .merge(crate::plugin::routes::router(state.plugin_route_state()))
}

fn plugin_router(state: Arc<AppState>) -> Router {
    // A ponte local não expõe o restante da API nem dá acesso às rotas privadas.
    plugin_routes(&state).fallback(|| async { StatusCode::NOT_FOUND })
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
        "guest_port_bound": st.guest_bound.load(Ordering::Relaxed),
        "terminal_address": st.terminal_address.map(|a| a.to_string())}).to_string();
    let mut resp = ([(header::CONTENT_TYPE, "application/json")], body).into_response();
    cors(&headers, resp.headers_mut());
    resp
}

/// `GET /api/client-api`: só o dono.
async fn client_api(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    let mut resp = if owner { client_api_reply() } else { crate::share_reach::unauthorized(&st, &fwd.client_ip) };
    cors(req.headers(), resp.headers_mut());
    resp
}

pub(crate) fn client_api_reply() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], serde_json::json!({"api": hangar_api::CLIENT_API}).to_string()).into_response()
}

/// Quem pede e se é o dono, resolvidos uma vez por pedido.
pub(crate) fn gate(st: &AppState, peer: SocketAddr, req: &Request) -> (Forward, bool) {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    // A ponte privada do Codex com terminal recebe o pedido já autenticado pela porta do Python.
    if req.extensions().get::<crate::session_write::bridge::BridgedOwner>().is_some() { return (Forward { client_ip, https, guest: None }, true); }
    let token = auth::presented_token(req.headers(), req.uri().query(), req.method(), https);
    let owner = st.auth.is_owner(&client_ip, token.as_deref());
    (Forward { client_ip, https, guest: None }, owner)
}

/// Quem pede. Na 8766 só o convidado que o porteiro pôs (nunca `is_owner`, nem `X-Forwarded-For`);
/// na 8765, o dono pelo token.
pub(crate) fn who(st: &AppState, peer: SocketAddr, req: &Request) -> (Forward, Option<Caller>) {
    if on_guest_port(req) {
        return (forward_for(st, peer, req), caller(req).cloned());
    }
    let (fwd, owner) = gate(st, peer, req);
    (fwd, owner.then_some(Caller::Owner))
}

/// O repasse de quem pede, sem decidir se é o dono. Convidado vai como loopback para o Python
/// aceitar o cabeçalho assinado.
pub(crate) fn forward_for(st: &AppState, peer: SocketAddr, req: &Request) -> Forward {
    if on_guest_port(req) {
        let guest = caller(req).and_then(Caller::grant).cloned();
        let client_ip = if guest.is_some() { "127.0.0.1".into() } else { peer.ip().to_string() };
        return Forward { client_ip, https: false, guest };
    }
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    Forward { client_ip, https, guest: None }
}

/// Chegou pela 8766: o `GuestConn` só existe no listener do convidado.
pub(crate) fn on_guest_port(req: &Request) -> bool {
    req.extensions().get::<ConnectInfo<crate::share::gate::GuestConn>>().is_some()
}

/// O 404 do FastAPI, que os clientes já tratam.
pub(crate) fn not_found(headers: &HeaderMap) -> Response {
    let mut resp = (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "application/json")], r#"{"detail":"Not Found"}"#).into_response();
    cors(headers, resp.headers_mut());
    resp
}


pub(crate) async fn pass(st: &AppState, mut req: Request, fwd: &Forward) -> Response {
    // Pedido da ponte do Codex com terminal nunca volta ao Python: ele trata o que o Rust não atende.
    if req.extensions().get::<crate::session_write::bridge::BridgedOwner>().is_some() { return StatusCode::MISDIRECTED_REQUEST.into_response(); }
    if let Some(grant) = &fwd.guest {
        // Assinado aqui, não no porteiro: a escrita pode esperar a porta da sessão mais que a validade.
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        crate::share::forward::prepare_guest_request(&mut req, &st.cfg.internal_secret, grant, now);
        // Recusa de lá não entra no bloqueio do dono, que é por IP e o convidado do Funnel nem tem um.
        return proxy::forward(&st.http, st.cfg.upstream, req, fwd).await;
    }
    // Sem convidado decidido (rota aberta sem handler) nada da 8766 vai ao Python: nunca como dono.
    if on_guest_port(&req) {
        return not_found(req.headers());
    }
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
    // Anexos e contas do dono são do Rust; o anexo também do convidado da sessão (o porteiro já
    // limitou ao nível: par só baixa um arquivo).
    let uploads = crate::uploads::http::matches(req.method(), req.uri().path());
    // Valores próprios: um `&Request` vivo através de `await` tira o `Send` do handler.
    let (session, method) = (req.extensions().get::<SessionPath>().map(|p| p.session.clone()), req.method().clone());
    let session_ok = |who: &Option<Caller>| who.as_ref().is_some_and(|c| session.as_ref().is_some_and(|s| c.can(s, &method)));
    if (uploads || crate::accounts::http::matches(req.method(), req.uri().path()))
        && match who(&st, peer, &req).1 {
            Some(Caller::Owner) => true,
            guest => uploads && session_ok(&guest),
        }
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
    if crate::share_reach::matches(req.method(), req.uri().path()) && !on_guest_port(&req) {
        let (forward, owner) = gate(&st, peer, &req);
        let headers = req.headers().clone();
        let mut response = if owner {
            crate::share_reach::public(st, req).await
        } else {
            crate::share_reach::unauthorized(&st, &forward.client_ip)
        };
        cors(&headers, response.headers_mut());
        return response;
    }
    if crate::worktree_routes::matches(req.method(), req.uri().path()) {
        let (_forward, who) = who(&st, peer, &req);
        // Convidado segue ao Python, que o recusa como antes.
        if matches!(who, Some(Caller::Owner)) {
            let headers = req.headers().clone();
            let mut response = crate::worktree_routes::public(st, req).await;
            cors(&headers, response.headers_mut());
            return response;
        }
        if matches!(&who,Some(Caller::Guest(_)))
            && !matches!(req.uri().path(),"/api/worktrees"|"/api/worktrees/detail") {
            return crate::session_write::json_response(StatusCode::FORBIDDEN,serde_json::json!({"detail":"worktrees pertencem ao dono"}));
        }
    }
    if crate::workspace_routes::matches(req.method(), req.uri().path()) {
        let (forward, who) = who(&st, peer, &req);
        // Só o convite (não o par) alcança as rotas da sessão dele; `/api/fs/*` não tem sessão.
        let share = who.as_ref().and_then(Caller::grant).zip(session.as_ref())
            .is_some_and(|(g, s)| g.sessions.get(s) == Some(&crate::share::store::Kind::Share));
        if matches!(who, Some(Caller::Owner)) || share {
            let headers = req.headers().clone();
            let mut response = crate::workspace_routes::public(st, req, forward).await;
            cors(&headers, response.headers_mut());
            return response;
        }
    }
    let fwd = forward_for(&st, peer, &req);
    pass(&st, req, &fwd).await
}

/// Id de correlação do diário: cabeçalho x-hangar-req; o EventSource só manda ?diag_req.
pub(crate) fn diag_req(req: &Request) -> String {
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
    let (fwd, who) = who(&st, peer, &req);
    let name = match path {
        Ok(Path(n)) if req.method() == Method::GET && who.as_ref().is_some_and(|c| c.can(&n, req.method())) => n,
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
    let Ok(info) = st.info(&name).await else {
        return route_failed(&st, req.headers(), "rust.history_failed", &name, "internal_info", INFO_REASON);
    };
    // Sessão inexistente ou provedor fora do Rust: o Python é o dono.
    let Some(info) = info else { return pass(&st, req, &fwd).await };
    // Antes de qualquer repasse: sessão em erro ainda sem transcript também é reaberta.
    st.request_reopen(&name);
    let Some(mut hreq) = info.history_request(limit) else {
        return pass(&st, req, &fwd).await;
    };
    // Com o chat ao vivo aberto, vale o transcript que o hub já confirmou: uma leitura solta da
    // descoberta não pode mostrar outra conversa que a do canal ao vivo.
    let fresh = Binding::from_info(&info);
    if let Some(bound) = st.side.hubs.bound(&name).filter(|b| fresh.as_ref().is_some_and(|f| !b.changes_at_once(f))) {
        hreq.jsonl = bound.jsonl;
    }
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
    let (fwd, who) = who(&st, peer, &req);
    let name = match path {
        Ok(Path(n)) if req.method() == Method::GET && who.as_ref().is_some_and(|c| c.can(&n, req.method())) => n,
        _ => return pass(&st, req, &fwd).await,
    };
    // Convidado não recebe aviso de mod nem pedido de navegador, e não conta como app do dono.
    let guest = matches!(who, Some(Caller::Guest(_)));
    let Ok(info) = st.info(&name).await else {
        return route_failed(&st, req.headers(), "rust.events_failed", &name, "internal_info", INFO_REASON);
    };
    // Antes de qualquer repasse: sessão em erro ainda sem transcript também é reaberta.
    if info.is_some() { st.request_reopen(&name); }
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
    let lease = st.side.hubs.acquire_as(&name, binding, &st.side, guest);
    if let Some(info) = &info { lease.hub.learn_queue(info); }
    let hub = lease.hub.clone();
    let (tx, rx) = mpsc::channel::<Queued>(64);
    // Pedido de navegador ainda vivo: o aparelho do dono que abre a conversa agora também o recebe.
    if !guest && let Some(frame) = crate::nav_requests::chat_frame(&st, &name) { let _ = tx.try_send(frame.into()); }
    tokio::spawn(client_loop(lease, resume, tx));
    let mut resp = Response::new(Body::from_stream(device_stream(rx, hub, deltas, guest).map(Ok::<Bytes, Infallible>)));
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
/// `guest`: retrato e ao vivo sem `plugin_toast` e `nav` (texto do dono, url com o token dele).
fn device_stream(rx: mpsc::Receiver<Queued>, hub: Arc<Hub>, deltas: bool, guest: bool) -> impl futures_util::Stream<Item = Bytes> {
    futures_util::stream::unfold((rx, hub, None), move |(mut rx, hub, mut ui)| async move {
        loop {
            let item = rx.recv().await?;
            if let Some(frame) = hub.resolve(item, &mut ui, deltas).filter(|f| !(guest && crate::side::owner_only(f))) {
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

    /// Aparelho ligado a um hub sem fontes do estado: só o que o teste entrega chega.
    async fn device(dir: &std::path::Path) -> (SideCtx, std::pin::Pin<Box<dyn futures_util::Stream<Item = Bytes> + Send>>) {
        device_with(dir, false).await
    }

    async fn device_with(dir: &std::path::Path, deltas: bool) -> (SideCtx, std::pin::Pin<Box<dyn futures_util::Stream<Item = Bytes> + Send>>) {
        let ctx = SideCtx {
            watchers: Watchers::default(),
            hubs: Hubs::default(),
            locate: Arc::new(crate::locate::Unavailable),
            monitors: None,
            mods: Default::default(),
            runtime: Default::default(),
            queue_dir: None,
            problem_hold: std::time::Duration::ZERO,
            shown_problems: Default::default(),
            diag: crate::diag::DiagClient::new("127.0.0.1:9".parse().unwrap(), "s".into()),
        };
        let jsonl = dir.join("t.jsonl");
        std::fs::write(&jsonl, "").unwrap();
        let lease = ctx.hubs.acquire("s", Binding { provider: crate::transcript::Provider::Claude, jsonl, key: "k".into(), headless: false, life: None }, &ctx);
        let hub = lease.hub.clone();
        let (tx, rx) = mpsc::channel::<Queued>(64);
        tokio::spawn(client_loop(lease, None, tx));
        // O aparelho já assinou o canal: o que for entregue daqui em diante vai pela fila dele.
        tokio::time::timeout(Duration::from_secs(5), async {
            while hub.tx.receiver_count() == 0 { tokio::time::sleep(Duration::from_millis(5)).await; }
        }).await.unwrap();
        (ctx, Box::pin(device_stream(rx, hub, deltas, false)))
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
            key: "k".into(), headless: false, life: None }, &ctx);
        let hub = lease.hub.clone();
        let receivers = hub.tx.receiver_count();
        tokio::spawn(client_loop(lease, None, tx));
        let mut late = Box::pin(device_stream(rx, hub.clone(), true, false));
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
