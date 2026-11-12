// crates/hangar-server/src/routes.rs
//! Rotas do hangar-server: saúde, custos, lista de sessões, histórico e chat ao vivo do Claude e do
//! Codex para o dono; todo o resto é repasse ao Python. Falha do Rust nessas rotas é 503 com código, nunca repasse.
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

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
use crate::share::caller::{Caller, SessionPath};
use crate::share::pair_routes::PairState;
use crate::side::{Binding, Hub, Hubs, Lease, Out, Queued, SideCtx};
use crate::tail::{self, Watchers};
use crate::transcript::{InternalInfo, SKIPPED_LINES, history_etag, merged_history_with_notices};

const PING_EVERY: Duration = Duration::from_secs(10);
const COMMENT_EVERY: Duration = Duration::from_secs(15);
const SEND_TIMEOUT: Duration = Duration::from_secs(30);

pub struct AppState {
    pub desktop_theme: Arc<hangar_desktop_theme::DesktopTheme>,
    pub startup: crate::startup::Readiness,
    pub startup_producers: Arc<crate::startup::Producers>,
    pub config_sync_context: Option<crate::config_sync::export::Context>,
    pub migration_branch: Arc<crate::migration_branch::Service>,
    settings_snapshot: std::sync::OnceLock<Result<Arc<crate::machine_peers::dotenv::SettingsSnapshot>,String>>,
    pub login_guests: Arc<crate::login_guests::Service>,
    pub omp_sync: Arc<std::sync::OnceLock<Arc<crate::omp_sync::Service>>>,
    pub codex_setup: Arc<std::sync::OnceLock<Result<Arc<crate::codex_setup::service::Service>, String>>>,
    pub harness_installer: Arc<crate::harness_setup::installer::Installer>,
    pub harness_probe: crate::cli_probe::Probe,
    pub interactions: Arc<crate::interactions::InteractionStore>,
    pub dictation_models: Arc<crate::dictation::catalog::Models>,
    /// Vagas para processos e chamadas de organização do ditado (catálogo sem cache e execução).
    pub dictation_slots: Arc<tokio::sync::Semaphore>,
    pub transcription: Arc<crate::transcription::service::TranscriptionService>,
    pub transcription_slots: Arc<tokio::sync::Semaphore>,
    pub accounts: crate::accounts::AccountService,
    pub command_catalog: Arc<crate::commands::Service>,
    pub local_commands: Arc<dyn hangar_core::process::Commands>,
    pub cfg: Config,
    pub auth: Auth,
    pub remote_browser: Arc<crate::remote_browser::layout::Layouts>,
    pub http: HttpClient,
    pub side: SideCtx,
    pub terminal: crate::terminal_control::TerminalPool,
    pub terminal_address: Option<SocketAddr>,
    /// Segundo socket da porta pública, em `[::]` dual-stack; atende como o primeiro.
    pub public_v6: Option<TcpListener>,
    /// A porta do front (`front_address`), decidida só na subida real (`main.rs`).
    pub front: Option<SocketAddr>,
    /// A entrada do Connect (`config::connect_listen`), decidida só na subida real (`main.rs`).
    pub connect: Option<SocketAddr>,
    /// A porta da prévia (`config::preview_listen`), decidida só na subida real (`main.rs`).
    pub preview: Option<SocketAddr>,
    /// A prévia de porta aberta e quem entrou nela.
    pub connect_previews: Arc<crate::connect::preview::Previews>,
    /// A conferência de fora da primeira prévia; o teste troca o DNS e a internet.
    pub connect_preview_reach: Arc<crate::share::owner_routes::Reach>,
    /// Credenciais de dono por aparelho (`~/.hangar/connect`); o teste troca por uma pasta temporária.
    pub connect_devices: Arc<crate::connect::devices::Store>,
    /// `frpc` e Caddy da ligação desta máquina; liga só na subida real (`main.rs`) ou pela rota.
    pub connect_supervisor: Arc<crate::connect::supervisor::Supervisor>,
    pub workspace_slots: Arc<tokio::sync::Semaphore>,
    pub workspace_read_slots: Arc<tokio::sync::Semaphore>,
    pub workspace_meta_slots: Arc<tokio::sync::Semaphore>,
    pub diag: crate::diag::DiagClient,
    pub costs: Arc<crate::costs::collect::Collector>,
    /// Índice de conversas da busca e do Arquivo; o produtor liga só na subida real (`main.rs`).
    pub search: crate::search::Index,
    /// O `claude -p` do Perguntar; `None` = o CLI do PATH (o teste troca por um falso).
    pub claude_p: Option<Arc<dyn crate::claude_p::Runner>>,
    pub fx:Arc<crate::costs::fx::Fx>,
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
    /// Aparelhos que já abriram a lista do dono (`~/.hangar/devices.json`); o teste troca por uma pasta temporária.
    pub devices: Arc<crate::devices::Registry>,
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
    /// Abertura do navegador do login Claude e escuta do retorno nesta máquina.
    pub login_browser: Arc<crate::accounts::login_browser::LoginBrowser>,
    /// Id desta máquina entre as do dono (`CP_SERVER_ID`); vazio = sem id.
    pub server_id: String,
    pub machine_identity: std::sync::RwLock<Option<String>>,
    /// Voz ao vivo; o teste troca por um hub com pastas temporárias.
    pub voice: Arc<crate::voice::hub::VoiceHub>,
    pub speech: Arc<crate::speech::Service>,
    /// Convites e par externo; `None` sem `HANGAR_SHARES_FILE` (o Python segue dono).
    pub share: Option<Arc<crate::share::ShareState>>,
    /// A porta do convidado já está escutando; a saúde anuncia.
    pub guest_bound: std::sync::atomic::AtomicBool,
    /// Instalar o SideStore no iPhone/iPad do cabo; o teste troca aparelho e Apple por falsos.
    pub device_install: Arc<crate::device_install::job::DeviceInstall>,
    /// Reservas pré-aquecidas do Claude sem terminal (tela de nova conversa aberta).
    pub prewarm: Arc<crate::prewarm::Store>,
    /// Base das releases do pacote que o `GET /api/atualizacao` consulta; o teste troca.
    pub update_release_base: String,
    /// Base do worker do "Reportar problema" (`/api/relatorio`); o teste troca por um falso.
    pub diag_report_base: String,
    /// Cofre de senhas desta máquina; o teste troca o chaveiro por um falso.
    pub vault: Arc<crate::vault::Vault>,
    /// Pedidos do cofre (aprovação e cadastro) na memória; o `hub` publica o evento `vault` deles.
    pub vault_requests: Arc<crate::vault::requests::Requests>,
    /// Cadeado das mudanças em plugins do Claude e o último `check` bem-sucedido.
    pub claude_plugins: Arc<crate::plugins::Service>,
}

impl AppState {
    /// O boot inicializa antes de servir; o lazy permite injetar contas nas provas isoladas.
    pub fn settings_snapshot(&self)->Result<Arc<crate::machine_peers::dotenv::SettingsSnapshot>,String>{
        self.settings_snapshot.get_or_init(||{
            crate::machine_peers::dotenv::SettingsSnapshot::capture(
                &self.accounts.env.home,crate::session_open::cli::backend_dir_for_home(&self.accounts.env.home).as_deref(),
                crate::machine_peers::dotenv::SettingsEnvironment::from_pairs(self.accounts.env.ordered_pairs()),
            ).map(Arc::new)
        }).clone()
    }

    /// Ligação concreta das fontes das nove rotas do plugin.
    pub fn plugin_route_state(self: &Arc<Self>) -> Arc<crate::plugin::routes::RouteState> {
        Arc::new(crate::plugin::routes::RouteState { store: self.plugin.clone(), secret: self.cfg.auth_token.clone(),
            trusted: self.cfg.trusted.clone(), sources: Arc::new(crate::plugin::sources::NativeSources::new(self)) })
    }

    pub fn new(cfg: Config) -> AppState {
        Self::with_server_id(cfg, String::new())
    }

    /// Identidade publicada nesta vida do servidor.
    pub fn current_server_id(&self) -> String {
        self.machine_identity.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_else(|| self.server_id.clone())
    }

    pub fn set_server_id(&self, id: String) {
        if let Some(groups) = &self.groups { groups.set_server_id(id.clone()); }
        self.list.tags.bind(self.accounts.clone(), id.clone());
        *self.machine_identity.write().unwrap_or_else(|e| e.into_inner()) = Some(id);
    }

    pub fn with_server_id(cfg: Config, server_id: String) -> AppState {
        Self::build(cfg, crate::terminal_control::TerminalPool::new(), server_id)
    }

    pub fn with_terminal_pool(cfg: Config, terminal: crate::terminal_control::TerminalPool) -> AppState {
        Self::build(cfg, terminal, String::new())
    }

    fn build(cfg: Config, terminal: crate::terminal_control::TerminalPool, server_id: String) -> AppState {
        let env = crate::accounts::environment::AccountEnvironment::capture();
        let list_env = crate::list::bridge::ListEnv::from_env();
        let scopes = list_env.dirs.clone().zip(crate::install_root::root(&env.home))
            .map(|(dirs, repo)| crate::costs::scopes::NativeScopes::new(env, dirs, crate::list::links::resolve_lenient(&repo)));
        let costs = Arc::new(crate::costs::collect::Collector::new(
            crate::costs::index::default_dir(), crate::costs::pricing::default_dir(),
            crate::costs::areas::default_map_file(),
            Arc::new(scopes),
        ));
        Self::with_parts_env(cfg, terminal, costs, Arc::new(crate::costs::fx::Fx::new()), server_id, list_env)
    }

    pub fn with_parts(cfg: Config, terminal: crate::terminal_control::TerminalPool,
                      costs: Arc<crate::costs::collect::Collector>, fx: Arc<crate::costs::fx::Fx>, server_id: String) -> AppState {
        Self::with_parts_env(cfg, terminal, costs, fx, server_id, crate::list::bridge::ListEnv::from_env())
    }

    fn with_parts_env(cfg: Config, terminal: crate::terminal_control::TerminalPool,
                      costs: Arc<crate::costs::collect::Collector>, fx: Arc<crate::costs::fx::Fx>, server_id: String,
                      list_env: crate::list::bridge::ListEnv) -> AppState {
        crate::transcript::set_orq_parser(crate::orchestration::timeline::parse_obj);
        let http = proxy::client();
        let mods = crate::mods::state::Mods::default();
        let interactions = Arc::new(crate::interactions::InteractionStore::default());
        let plugin = Arc::new(crate::plugin::Store::new(interactions.clone()));
        let facts = crate::list::facts::FactsClient::new(cfg.upstream, cfg.internal_secret.clone());
        let list = Arc::new(crate::list::bridge::ListBridge::new(list_env, facts));
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
        // O mesmo `~` do app nativo e do Python (USERPROFILE no Windows), que gravam `~/.hangar/nav`.
        let hangar_dir = std::env::home_dir().unwrap_or_default().join(".hangar");
        let connect_devices = Arc::new(crate::connect::devices::Store::new(hangar_dir.join("connect"), None));
        let connect_supervisor = Arc::new(crate::connect::supervisor::Supervisor::new(hangar_dir.join("connect"),
            crate::runtime::local_policy::log_base().unwrap_or_else(|| hangar_dir.join("logs")), connect_devices.clone(), cfg.listen.port()));
        let share = match crate::config::GuestEnv::from_env() {
            Ok(guest) => crate::share::from_env(guest.shares_file, list.env().dirs.as_ref(), list.env().capture_program.clone(), Some(connect_supervisor.clone())),
            Err(e) => { tracing::error!("{e}"); None }
        };
        if let Some(share) = &share { list.set_shares(share.clone()); }
        let device_install = {
            let env = crate::accounts::environment::AccountEnvironment::capture();
            Arc::new(crate::device_install::job::DeviceInstall::new(
                Arc::new(crate::device_install::UsbLink::new(&hangar_dir)), Arc::new(crate::device_install::apple::IsideloadSigner), &hangar_dir)
                .with_config(Arc::new(move || crate::runtime::codex_launch::runtime_config(&env))))
        };
        let groups = crate::groups::from_env(list.env().dirs.as_ref(), server_id.clone(),
            Arc::new(crate::groups::orq::PythonOrq::new(cfg.upstream, cfg.internal_secret.clone(), http.clone())
                .with_paths(list.env().dirs.as_ref().map(|d| d.home.clone()).unwrap_or_default(),
                    list.env().dirs.as_ref().map(|d| d.claude.join(".hangar-pair")).unwrap_or_default())
                .with_mux(list.env().capture_program.to_string_lossy().into_owned())), list.clone(), diag.clone());
        let vault_requests = Arc::new(crate::vault::requests::Requests::default());
        let accounts = crate::accounts::AccountService::new(crate::accounts::environment::AccountEnvironment::capture());
        let search = crate::search::Index::server(accounts.clone());
        AppState { desktop_theme: Arc::new(hangar_desktop_theme::DesktopTheme::system()), local_commands: Arc::new(crate::auxiliary::NativeCommands(accounts.env.clone())), startup: Default::default(), startup_producers: Arc::default(), settings_snapshot: std::sync::OnceLock::new(), login_guests: Arc::new(crate::login_guests::Service::uninitialized(&accounts.env.home,&accounts.env.claude_base)), accounts, search, claude_p: None, command_catalog: Arc::default(), groups, server_id, machine_identity: std::sync::RwLock::new(None), peers: Arc::new(crate::groups::peers_from_env()), auth: Auth::configured(&cfg.auth_token), http, side, cfg, terminal, terminal_address: None, public_v6: None, front: None, connect: None, preview: None, connect_previews: Arc::default(), connect_preview_reach: Arc::new(crate::share::owner_routes::Reach::preview()), diag,
            migration_branch: Arc::default(),
            workspace_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            login_browser: Arc::default(),
            interactions, plugin,
            remote_browser: Arc::default(),
            transcription: Arc::default(),
            config_sync_context: None,
            omp_sync: Arc::default(),
            codex_setup: Arc::default(),
            harness_installer: Arc::default(),
            harness_probe: Default::default(),
            dictation_models: Arc::default(),
            dictation_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            transcription_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            workspace_read_slots: Arc::new(tokio::sync::Semaphore::new(8)),
            workspace_meta_slots: Arc::new(tokio::sync::Semaphore::new(4)),
            costs, fx, reports: Arc::new(crate::costs::ReportCache::default()),
            origins_home: std::path::PathBuf::from(std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).unwrap_or_default()),
            prewarm: Arc::new(crate::prewarm::Store::server(&hangar_dir)),
            devices: Arc::new(crate::devices::Registry::new(hangar_dir.join("devices.json"))),
            connect_devices,
            connect_supervisor,
            update_release_base: crate::update::RELEASE_BASE.to_owned(),
            diag_report_base: crate::diag_report::WORKER_BASE.to_owned(),
            vault: Arc::new(crate::vault::Vault::system()),
            hub: Arc::new(crate::list::hub::ListHub::with_vault(vault_requests.feed())),
            vault_requests,
            claude_plugins: Arc::default(),
            hangar_dir, device_install,
            upload_naming: Default::default(),
            origins: std::sync::Mutex::new(indexmap::IndexMap::new()),
            list, state,
            term: Arc::default(),
            mods,
            pages: Arc::new(crate::pages::store::Store::new(crate::pages::store::Store::default_root())),
            mermaid: Arc::new(crate::mermaid::cache::Cache::new(crate::mermaid::cache::Cache::default_root())),
            artifacts: Arc::default(),
            chromium: crate::pages::chrome::find,
            write_gate_wait: std::time::Duration::from_secs(30),
            btw_programs: Default::default(),
            voice: Arc::new(crate::voice::hub::VoiceHub::new()),
            speech: Arc::default(),
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

pub(crate) fn route_failed(st: &AppState, req: &HeaderMap, event: &'static str, name: &str, code: &'static str, reason: &'static str) -> Response {
    hangar_core::edge::route_failed(&st.diag, req, event, name, code, reason)
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
    crate::omp_sync::validate_environment(&crate::push::server_env()).map_err(std::io::Error::other)?;
    let guest = guest.filter(|addr| state.share.is_some() || {
        tracing::error!(listen = %addr, "porta do convidado pedida sem HANGAR_SHARES_FILE: fica fechada");
        false
    });
    let plugin = crate::plugin_listener::bind(&listener).await?;
    // Bind LAN específico não recebe tráfego de loopback: a observação tem uma porta própria.
    let private = TcpListener::bind("127.0.0.1:0").await?;
    state.terminal_address = Some(private.local_addr()?);
    state.voice.set_self(listener.local_addr()?, &state.cfg.auth_token);
    state.connect_previews.reserve([listener.local_addr()?.port(), private.local_addr()?.port(), state.cfg.upstream.port()].into_iter()
        .chain(plugin.as_ref().and_then(|plugin| plugin.local_addr().ok()).map(|addr| addr.port()))
        .chain([guest, state.front, state.connect, state.preview].into_iter().flatten().map(|addr| addr.port())));
    let public_v6 = state.public_v6.take();
    let state = Arc::new(state);
    if let Some(registry) = state.side.runtime.get() {
        let mut other = crate::runtime::other_terminal::OtherOptions::new(state.terminal.clone(),
            crate::provider_models::inbox::INBOX.clone(), state.list.env().procs.clone(), state.interactions.clone());
        other.preview = Some(std::sync::Arc::new(crate::state::other_preview::RuntimePreview::new(state.side.hubs.downgrade())));
        other.after_closed = Some(crate::session_write::bridge::other_cleanup(std::sync::Arc::downgrade(&state)));
        registry.bind_other(other, crate::runtime::other_terminal::discovery(state.list.clone())).map_err(std::io::Error::other)?;
    }
    let main_startup = state.startup.prepares();
    let producers = state.startup_producers.clone();
    let _producer_scope = producers.register(tokio::spawn({
        let state = state.clone();
        async move {
            state.startup.wait().await;
            let _maintenance = main_startup.then(|| crate::AbortOnDrop(tokio::spawn(crate::startup::maintenance(state.clone()))));
    // Ligado com o estado final: o evento lê as mesmas contas que a rota `/api/tags`.
    state.list.tags.bind(state.accounts.clone(), state.server_id.clone());
    if crate::config::Config::runtime_instance().ok().flatten().is_some() {
        let omp_sync = crate::omp_sync::start(state.clone());
        let _ = state.omp_sync.set(omp_sync.clone());
        omp_sync.ready().await;
        crate::provider_models::inbox::install_endpoint(state.clone()).await;
        crate::provider_models::warm();
        let policy_home = state.accounts.env.home.clone();
        let policy_claude = state.accounts.env.claude_base.clone();
        tokio::task::spawn_blocking(move || crate::orchestration::policy::migrate(&policy_home, &policy_claude)).await
            .unwrap_or_else(|error| { tracing::warn!(%error, "migração de política interrompida"); false });
    }
    let _worktree_sweep=crate::AbortOnDrop(crate::worktree_policy::spawn(state.clone()));
    let _keyring_watch = crate::AbortOnDrop(crate::keyring::spawn(state.list.keyring.clone()));
    let _vault_backup = crate::AbortOnDrop(crate::vault::backup::spawn(state.clone()));
    let _vault_files = crate::AbortOnDrop(tokio::spawn(crate::vault::files::sweep_at_start()));
    let _auto_report = crate::AbortOnDrop(crate::diag_report::auto::spawn(state.clone()));
    // Abortada na saída: a tarefa segura o estado do servidor, que sobreviveria a ele.
    // As sessões com terminal que o multiplexador perdeu voltam pelo spec; antes da varredura de grupos, que
    // conta como vivos os nomes à espera dele.
    let _session_restore = crate::session_open::restore::spawn(state.clone()).map(crate::AbortOnDrop);
    let _group_sweep = crate::groups::sweep::spawn(state.clone()).map(crate::AbortOnDrop);
    let _link_return = crate::groups::link_return::spawn(state.clone()).map(crate::AbortOnDrop);
    let _session_sleep = crate::AbortOnDrop(crate::session_sleep::spawn(state.clone()));
    let _federation = crate::groups::federation::spawn(state.clone()).map(crate::AbortOnDrop);
    // O aviso no celular é decidido aqui, com ou sem lista aberta.
    let _push_watch: Vec<crate::AbortOnDrop> = crate::push::start(state.clone()).into_iter().map(crate::AbortOnDrop).collect();
    // Assina as transições depois do vigia de avisos, que as publica.
    let _loop_watch: Vec<crate::AbortOnDrop> = crate::loops::watch::start(state.clone()).into_iter().map(crate::AbortOnDrop).collect();
    let _limit_switch = crate::session_write::limit_switch::spawn(state.clone()).map(crate::AbortOnDrop);
    // Sem a fotografia do Python a varredura não revoga nada (ver `life::sweep`); o túnel religa já.
    let _share_sweep = state.share.clone().map(|share| crate::AbortOnDrop(tokio::spawn(
        crate::share::owner_routes::sweep_loop(share, crate::share::owner_routes::SWEEP_EVERY))));
            std::future::pending::<()>().await;
        }
    }));
    let v6_state = state.clone();
    let plugin_state = state.clone();
    let guest_state = state.clone();
    let (front_state, front) = (state.clone(), state.front);
    let (connect_state, connect) = (state.clone(), state.connect);
    let (preview_state, preview) = (state.clone(), state.preview);
    // Uma só para as portas: convite e par veem as mesmas lojas e a mesma conferência.
    let pair = pair_state(&state);
    let guest_pair = pair.clone();
    let front_pair = pair.clone();
    let connect_pair = pair.clone();
    let result = tokio::select! {
        // Nunca terminam, como a do front: sem elas, a 8765 e o Tailscale seguem de pé.
        () = keep_listening(connect, "connect_port_failed",
            |addr| serve_connect(addr, connect_state.clone(), connect_pair.clone()),
            |addr, e| connect_state.connect_supervisor.port_failed(format!("porta do Connect {} indisponível ({e})", addr.port()))) => Ok(()),
        () = keep_listening(preview, "connect_preview_port_failed",
            |addr| serve_preview(addr, preview_state.clone()),
            |addr, e| preview_state.connect_supervisor.preview_port_failed(format!("porta {} indisponível ({e})", addr.port()))) => Ok(()),
        // Nunca termina: a porta do front é extra, e cair nela não derruba o servidor.
        () = async {
            let Some(addr) = front else { return std::future::pending().await };
            let listener = bind_when_free(addr).await;
            tracing::info!(listen = %addr, "porta do front servida pelo hangar-server");
            if let Err(e) = axum::serve(listener.tap_io(crate::nodelay), router_with(front_state.clone(), front_pair.as_ref())
                .into_make_service_with_connect_info::<SocketAddr>()).await {
                tracing::warn!(listen = %addr, "porta do front parou: {e}");
            }
            std::future::pending().await
        } => Ok(()),
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
    };
    result
}

/// A porta do front (`CP_FRONT_PORT`) de quem servia a tela pelo `vite preview`: na máquina migrada
/// (pacote ou modo desenvolvedor) esta porta entrega a mesma tela e a mesma API, e o navegador
/// guarda o que já tinha nessa origem. Máquina no git segue com o `vite`.
pub fn front_address(home: &std::path::Path, listen: SocketAddr) -> Option<SocketAddr> {
    if std::fs::symlink_metadata(crate::install_root::current(home)).is_err() { return None; }
    let port = crate::user_data::env_port(home, None, "CP_FRONT_PORT")
        .inspect_err(|e| tracing::warn!("porta do front ilegível: {e}")).ok().flatten()?;
    // Se o front ganhasse a porta, o Caddy entregaria a internet com o par real, que é local.
    if port == crate::connect::CONNECT_PORT {
        tracing::warn!(code = "front_port_is_connect", port, "CP_FRONT_PORT é a porta do Connect: a porta do front fica fechada");
        return None;
    }
    (port != 0 && port != listen.port()).then(|| SocketAddr::new(listen.ip(), port))
}

/// Serve a porta para sempre: cai, avisa quem precisa saber e tenta de novo. Porta que chegou a servir
/// volta a tentar logo; a espera longa é só para quem nem abriu.
async fn keep_listening<F, Fut>(addr: Option<SocketAddr>, code: &'static str, mut serve: F, failed: impl Fn(SocketAddr, &std::io::Error))
where F: FnMut(SocketAddr) -> Fut, Fut: std::future::Future<Output = std::io::Result<()>> {
    let Some(addr) = addr else { return std::future::pending().await };
    let mut wait = Duration::from_millis(250);
    let mut failing = false;
    loop {
        let tried = Instant::now();
        if let Err(e) = serve(addr).await {
            failed(addr, &e);
            if !failing {
                tracing::warn!(code, listen = %addr, "porta indisponível, tentando de novo: {e}");
                failing = true;
            }
        }
        if tried.elapsed() > Duration::from_secs(5) { wait = Duration::from_millis(250); }
        tokio::time::sleep(wait).await;
        wait = (wait * 2).min(Duration::from_secs(5));
    }
}

/// Liga quando a porta solta: o `vite` que a migração desligou pode ainda segurá-la por instantes.
async fn bind_when_free(addr: SocketAddr) -> TcpListener {
    let mut warned = false;
    loop {
        match TcpListener::bind(addr).await {
            Ok(listener) => return listener,
            Err(e) => {
                if !warned { tracing::warn!(listen = %addr, "porta do front ocupada, tentando de novo: {e}"); warned = true; }
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        }
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

/// O roteador do dono atrás de `rewrite_peer`. Só em loopback: quem fala com ela é o Caddy desta máquina.
pub async fn serve_connect(addr: SocketAddr, state: Arc<AppState>, pair: Option<Arc<PairState>>) -> std::io::Result<()> {
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "entrada do Connect fora do loopback"));
    }
    let listener = guest_socket(addr)?;
    state.connect_supervisor.port_opened();
    tracing::info!(listen = %addr, "entrada do Connect de pé");
    axum::serve(listener, router_with(state, pair.as_ref())
        .layer(axum::middleware::map_request(crate::connect::gate::rewrite_peer))
        .into_make_service_with_connect_info::<crate::connect::gate::ConnectConn>()).await
}

/// A prévia não usa nada do servidor. O site do Caddy só entra depois que esta porta abriu aqui.
pub async fn serve_preview(addr: SocketAddr, state: Arc<AppState>) -> std::io::Result<()> {
    if !addr.ip().is_loopback() {
        return Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "prévia do Connect fora do loopback"));
    }
    let listener = guest_socket(addr)?;
    state.connect_supervisor.preview_port_opened();
    tracing::info!(listen = %addr, "porta da prévia de pé");
    axum::serve(listener, crate::connect::preview::router(state.connect_previews.clone()).into_make_service()).await
}

async fn serve_guest(addr: SocketAddr, state: Arc<AppState>, pair: Option<Arc<PairState>>) -> std::io::Result<()> {
    let listener = bind_guest(addr).await;
    state.guest_bound.store(true, Ordering::Relaxed);
    state.connect_supervisor.guest_port_opened();
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
    // ponytail: `GuestConn` e `ConnectConn` só casam com `TcpListener` puro (sem `tap_io`); a conexão aceita herda
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
    session_routes(pass_any).fallback(pass_any).with_state(state.clone())
        .merge(crate::share::guest_routes::guest_router(share.clone(), crate::share::guest_routes::owner_name()))
        .merge(crate::share::pair_routes::guest_pair_router(pair))
        .layer(axum::middleware::from_fn_with_state(Arc::new(crate::share::gate::Gate::new(share)), crate::share::gate::guard))
        .layer(axum::middleware::from_fn_with_state(state, crate::startup::guard))
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
    let (fwd, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
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
        .route("/api/sessions/{name}/nav-remoto", get(crate::remote_browser::ws::public).head(fallback.clone()).fallback(fallback.clone()))
        // Outro método nessas rotas (preflight OPTIONS, HEAD) segue ao Python.
        .route("/api/sessions", get(crate::list::hub::list).post(crate::session_open::create).fallback(fallback.clone()))
        .route("/api/sessions/events", get(crate::list::hub::events).fallback(fallback.clone()))
        .route("/api/sessions/{name}/history", get(history).fallback(fallback.clone()))
        .route("/api/sessions/{name}/events", get(events).fallback(fallback.clone()))
        .route("/api/sessions/{name}/shell", axum::routing::any(crate::session_tools::public))
        .route("/api/sessions/{name}/shell/", axum::routing::any(crate::session_tools::public))
        .route("/api/sessions/{name}/open-editor", axum::routing::any(crate::session_tools::public))
        .route("/api/sessions/{name}/open-editor/", axum::routing::any(crate::session_tools::public))
        .route("/api/sessions/{name}/open-terminal", axum::routing::any(crate::session_tools::public))
        .route("/api/sessions/{name}/open-terminal/", axum::routing::any(crate::session_tools::public))
        .route("/api/sessions/{name}/workflows", axum::routing::any(workflows))
        .route("/api/sessions/{name}/workflows/", axum::routing::any(workflows))
        .route("/api/sessions/{name}/workflows/{*rest}", axum::routing::any(workflows))
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
        // Trocar o nome da sessão: inteiro no Rust; o convidado com acesso completo também.
        .route("/api/sessions/{name}/rename", axum::routing::post(crate::session_rename::route).fallback(fallback.clone()))
        .route("/api/sessions/{name}/reopen", axum::routing::post(crate::session_open::fallen::reopen_route).fallback(fallback.clone()))
        .route("/api/sessions/{name}/dismiss", axum::routing::post(crate::session_open::fallen::dismiss_route).fallback(fallback.clone()))
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
        .route("/api/sessions/{name}/codex/terminate", axum::routing::post(crate::session_write::codex::terminate).fallback(fallback.clone()))
        .route("/api/orq/{exec_id}/conductor", get(crate::orq_conductor::public).fallback(fallback.clone()))
        .route("/api/sessions/{name}/navegador", get(crate::remote_browser::url::route).head(fallback.clone()).fallback(fallback.clone()))
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
        .route("/__hangar_server/dictation/{operation}", axum::routing::post(crate::dictation::routes::private))
        .route("/__hangar_server/claude/customizations", axum::routing::post(crate::claude_customizations::private))
        .route("/__hangar_server/forwarded", axum::routing::any(crate::forwarded::private))
        .route("/__hangar_server/list", axum::routing::post(crate::list::bridge::private))
        .route("/__hangar_server/accounts", axum::routing::post(crate::accounts::http::private))
        .route("/__hangar_server/quotas", axum::routing::post(crate::accounts::quotas::private))
        .route("/__hangar_server/accounts/claude", axum::routing::post(crate::accounts::http::private_claude))
        .route("/__hangar_server/accounts/public", axum::routing::any(crate::accounts::http::private_public))
        .route("/__hangar_server/groups", axum::routing::post(crate::groups::bridge::private))
        .route("/__hangar_server/codex/{*rest}", axum::routing::any(crate::session_write::bridge::codex))
        .route("/__hangar_server/claude/{*rest}", axum::routing::any(crate::session_write::bridge::claude))
        .route("/__hangar_server/jev/aux", axum::routing::any(crate::jev_aux::private))
        .route("/__hangar_server/subagents", get(crate::subagents::private))
        .route("/__hangar_server/share", axum::routing::post(crate::share::bridge::private))
        .layer(axum::middleware::from_fn(crate::migration_status::count_bridge));
    // Painel e canal do estado ficam fora da contagem: conexões longas, não chamadas da ponte.
    let share = private_share_router(&state, pair);
    router.route("/__hangar_server/term", get(crate::term::private_ws))
        .route("/__hangar_server/state/{name}/events", get(crate::side::private_events))
        .route("/__hangar_server/remote-browser", get(crate::remote_browser::ws::private))
        .with_state(state.clone())
        .merge(share)
        .layer(axum::middleware::from_fn_with_state(state, crate::startup::guard))
}

pub fn router(state: Arc<AppState>) -> Router {
    let pair = pair_state(&state);
    router_with(state, pair.as_ref())
}

fn router_with(state: Arc<AppState>, pair: Option<&Arc<PairState>>) -> Router {
    let api = public_routes(state.clone(), pair);
    // O MCP chama as tools pelas mesmas rotas que o `hangar-send` chama: o roteador público inteiro.
    api.clone().merge(crate::mcp::router(state.clone(), api))
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::login_guests::gate::guard))
        .layer(axum::middleware::from_fn(request_auth))
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::login_guests::body_limit))
        .layer(axum::middleware::from_fn(crate::login_guests::public_contract))
        .layer(axum::middleware::from_fn(crate::config_sync::runtime_http::cors_layer))
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::connect::gate::device_token_layer))
        .layer(axum::middleware::from_fn_with_state(state, crate::startup::guard))
}

fn public_routes(state: Arc<AppState>, pair: Option<&Arc<PairState>>) -> Router {
    session_routes(pass_any)
        .route("/api/credenciais/cookie", axum::routing::any(crate::auxiliary::handle))
        .route("/api/credenciais/cookie/", axum::routing::any(crate::auxiliary::handle))
        .route("/api/deploy/github-webhook", axum::routing::any(crate::auxiliary::handle))
        .route("/api/deploy/github-webhook/", axum::routing::any(crate::auxiliary::handle))
        .route("/api/preview", axum::routing::any(crate::auxiliary::handle))
        .route("/api/preview/", axum::routing::any(crate::auxiliary::handle))
        .route("/api/me", axum::routing::any(crate::login_guests::route))
        .route("/api/guests", axum::routing::any(crate::login_guests::route))
        .route("/api/guests/{*rest}", axum::routing::any(crate::login_guests::route))
        .route("/api/sync/{*rest}", axum::routing::any(crate::login_guests::route))
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
        // Loop e `then`: a guarda delimita a sessão e a política permite cada operação.
        .route("/api/sessions/{name}/then", axum::routing::any(crate::loops::http::route))
        .route("/api/sessions/{name}/loop", axum::routing::any(crate::loops::http::route))
        .route("/api/sessions/{name}/loop/refine", axum::routing::any(crate::loops::http::route))
        .route("/api/sessions/{name}/loop/resolve", axum::routing::any(crate::loops::http::route))
        .route("/api/sessions/{name}/bastao", axum::routing::any(crate::session_open::baton::serve))
        .route("/api/sessions/{name}/bastao/dossie", axum::routing::any(crate::session_open::baton::serve))
        // Retomar do Arquivo e o `/resume` ao vivo: a abertura e a reabertura do Rust, com a conversa que já existe.
        .route("/api/archive/{project}/{session_id}/resume", axum::routing::post(crate::session_open::server::archive_resume).fallback(pass_any))
        // Ler a conversa do Arquivo e a imagem dela: só o dono; o resto segue ao Python, que recusa.
        // O Arquivo (pastas, recentes, uma pasta, um cwd): só o dono; `recent` antes da pasta.
        .route("/api/archive", get(crate::archive_list::folders).fallback(pass_any))
        .route("/api/archive/recent", get(crate::archive_list::recent).fallback(pass_any))
        .route("/api/archive/{project}", get(crate::archive_list::folder).fallback(pass_any))
        .route("/api/archive-por-cwd", get(crate::archive_list::by_cwd).fallback(pass_any))
        .route("/api/archive/{project}/{session_id}/history", get(crate::archive_history::history).fallback(pass_any))
        .route("/api/archive/{project}/{session_id}/transcript-image/{uuid}/{idx}", get(crate::archive_history::image).fallback(pass_any))
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
        .route("/api/search", get(crate::search::http::route).fallback(pass_any))
        .route("/api/ask-history", axum::routing::post(crate::search::ask::route).fallback(pass_any))
        .route("/api/search/context", get(crate::search_context::route).fallback(pass_any))
        .route("/api/migration/status", get(crate::migration_status::status).fallback(pass_any))
        // Novidades e avisos da próxima atualização: só o dono, nunca repassado (o Python não tem a rota).
        .route("/api/update/notes", get(crate::update_notes::route))
        // O que as atualizações já trouxeram e cada aparelho ainda não viu: só o dono, nunca repassado.
        .route("/api/update/arrived", get(crate::update_arrived::unseen))
        .route("/api/update/arrived/seen", axum::routing::post(crate::update_arrived::seen))
        .route("/api/claude/defaults", axum::routing::post(crate::claude_defaults::save).fallback(pass_any))
        .route("/api/session-defaults", axum::routing::get(crate::session_defaults::handle).put(crate::session_defaults::handle))
        // O que já foi visto do tour do nativo: só o dono, nunca repassado (o Python não tem a rota).
        .route("/api/tour-progress", axum::routing::get(crate::tour_progress::handle).put(crate::tour_progress::handle))
        // Plugins do Claude da pasta principal: só o dono, nunca repassados (o Python não tem as rotas).
        .route("/api/plugins", get(crate::plugins::state))
        .route("/api/plugins/check", axum::routing::post(crate::plugins::check))
        .route("/api/plugins/available", get(crate::plugins::available))
        .route("/api/plugins/marketplaces", axum::routing::post(crate::plugins::marketplace_add))
        .route("/api/plugins/marketplaces/{name}", axum::routing::delete(crate::plugins::marketplace_remove))
        .route("/api/plugins/marketplaces/{name}/update", axum::routing::post(crate::plugins::marketplace_update))
        .route("/api/plugins/marketplaces/{name}/auto-update", axum::routing::put(crate::plugins::auto_update))
        .route("/api/plugins/{id}", axum::routing::delete(crate::plugins::uninstall))
        .route("/api/plugins/{id}/details", get(crate::plugins::details))
        .route("/api/plugins/{id}/install", axum::routing::post(crate::plugins::install))
        .route("/api/plugins/{id}/update", axum::routing::post(crate::plugins::update))
        .route("/api/plugins/{id}/enable", axum::routing::post(crate::plugins::enable))
        .route("/api/plugins/{id}/disable", axum::routing::post(crate::plugins::disable))
        // Sugestão de nome da sessão genérica: só o dono, nunca repassada (o Python não tem a rota).
        .route("/api/sessions/{name}/rename-suggestion", axum::routing::post(crate::dictation::rename_suggestion::route).delete(crate::dictation::rename_suggestion::route))
        .route("/api/peers", axum::routing::any(crate::machine_peers::handle))
        .route("/api/peers/{*path}", axum::routing::any(crate::machine_peers::handle))
        .route("/api/alcance", axum::routing::any(crate::machine_peers::handle))
        .route("/api/alcance/pareamento", axum::routing::any(crate::machine_peers::handle))
        // Ocultar da lista: só o dono, nunca repassada (o Python não tem a rota).
        .route("/api/sessions/{name}/hidden", axum::routing::put(crate::session_visibility::route).delete(crate::session_visibility::route))
        .route("/api/session-auto-hide", axum::routing::get(crate::session_visibility::auto::route).put(crate::session_visibility::auto::route))
        .route("/api/tags", axum::routing::get(crate::tags::handle).put(crate::tags::handle))
        .route("/api/config-sync/manifest", axum::routing::get(crate::config_sync::http::handle))
        .route("/api/config-sync/bundle", axum::routing::get(crate::config_sync::http::handle))
        .route("/api/config-sync/apply", axum::routing::any(crate::config_sync::apply_http::handle))
        .route("/api/push/expo/subscribe", axum::routing::post(crate::push_expo::subscribe))
        .route("/api/push", axum::routing::any(crate::push::http::route))
        .route("/api/push/{*path}", axum::routing::any(crate::push::http::route))
        .route("/api/push/vapid", axum::routing::any(crate::push::http::route))
        .route("/api/push/settings", axum::routing::any(crate::push::http::route))
        .route("/api/push/subscribe", axum::routing::any(crate::push::http::route))
        .route("/api/push/mute", axum::routing::any(crate::push::http::route))
        .route("/api/push/quiet-hours", axum::routing::any(crate::push::http::route))
        .route("/api/presence", get(crate::presence::route).fallback(pass_any))
        // Mesmo caminho: o axum junta o POST à rota de cima (um repasse só, o dela).
        .route("/api/presence", axum::routing::post(crate::presence::route))
        .route("/api/presence/heartbeat", axum::routing::post(crate::presence::route).fallback(pass_any))
        .route("/api/jev/aux/cliproxy", get(crate::jev_aux::cliproxy).fallback(pass_any))
        .route("/api/jev/aux/models", axum::routing::post(crate::jev_aux::models).fallback(pass_any))
        .route("/api/voice", get(crate::voice::routes::ws).options(crate::voice::routes::preflight))
        .route("/api/voice/settings", get(crate::voice::routes::settings_route).options(crate::voice::routes::preflight))
        // Mesmo caminho: o axum junta o PUT à rota de cima (um repasse só, o dela).
        .route("/api/voice/settings", axum::routing::put(crate::voice::routes::settings_route))
        .route("/api/voice/memory", get(crate::voice::routes::memory_route).options(crate::voice::routes::preflight))
        .route("/api/voice/memory/{id}", axum::routing::delete(crate::voice::routes::memory_route).options(crate::voice::routes::preflight))
        // Grupos (`/pair`, `/group-message`, `/pair/contract`, `/pair-remote`, `/unpair-remote`).
        .merge(crate::groups::routes::router())
        .merge(crate::connect::routes::router())
        // Planos e catálogo de plugins e skills: o dono no Rust; o resto segue ao Python.
        .merge(crate::plans::routes::router())
        .merge(hangar_desktop_theme::router(hangar_core::edge::Edge { trusted: state.cfg.trusted.clone(), auth: state.auth.clone(), diag: state.diag.clone() }, state.desktop_theme.clone()))
        .route("/api/claude/customizations", get(crate::claude_customizations::catalog_route).fallback(pass_any))
        // Senha do chaveiro: só o dono, nunca repassada; só o preflight do CORS vai ao Python.
        .route("/api/keyring/unlock", axum::routing::post(crate::keyring::unlock_route).options(pass_any))
        // Cofre de senhas: só o dono, nunca repassado; só o preflight do CORS vai ao Python.
        .route("/api/vault/status", get(crate::vault::http::status).options(pass_any))
        .route("/api/vault/config", axum::routing::put(crate::vault::http::config).options(pass_any))
        .route("/api/vault/entries", get(crate::vault::http::entries).post(crate::vault::http::entries).options(pass_any))
        .route("/api/vault/entries/{id}", axum::routing::patch(crate::vault::http::entry).delete(crate::vault::http::entry).options(pass_any))
        .route("/api/vault/entries/{id}/password", get(crate::vault::http::password).options(pass_any))
        .route("/api/vault/use", axum::routing::post(crate::vault::http::use_entry).options(pass_any))
        .route("/api/vault/ask-entry", axum::routing::post(crate::vault::http::ask_entry).options(pass_any))
        .route("/api/vault/requests/{id}", axum::routing::post(crate::vault::http::request).delete(crate::vault::http::request).options(pass_any))
        .route("/api/vault/requests/{id}/wait", get(crate::vault::http::wait).options(pass_any))
        .route("/api/vault/recovery", axum::routing::post(crate::vault::http::recovery).options(pass_any))
        .route("/api/vault/restore", axum::routing::post(crate::vault::http::restore).options(pass_any))
        .route("/api/vault/backup/{server_id}", get(crate::vault::http::backup_copy).put(crate::vault::http::backup_copy).options(pass_any))
        // Relatório de diagnóstico: só o dono, nunca repassado; só o preflight do CORS vai ao Python.
        .route("/api/diag/report", axum::routing::post(crate::diag_report::report_route).options(pass_any))
        // SideStore no iPhone/iPad do cabo: só o dono, nunca repassado (a senha da Apple ID não vai ao Python).
        .route("/api/device-install/devices", get(crate::device_install::routes::devices))
        .route("/api/device-install/progress", get(crate::device_install::routes::progress))
        .route("/api/device-install/trust", axum::routing::post(crate::device_install::routes::trust))
        .route("/api/device-install/start", axum::routing::post(crate::device_install::routes::start))
        .route("/api/device-install/code", axum::routing::post(crate::device_install::routes::code))
        .route("/api/device-install/certs", axum::routing::post(crate::device_install::routes::certs))
        // Tela web ligada ou não: só o dono, nunca repassado.
        .route("/api/web-ui", get(crate::ui::setting).put(crate::ui::setting))
        // Consentimento do relatório automático da atualização: só o dono, nunca repassado.
        .route("/api/auto-report", get(crate::diag_report::auto::setting).put(crate::diag_report::auto::setting))
        // Atualização do pacote e do checkout, com a mesma decisão do dono.
        .route("/api/atualizacao", get(crate::update::http::status).fallback(pass_any))
        .route("/api/atualizacao/iniciar", axum::routing::post(crate::update::http::start).fallback(pass_any))
        .route("/api/atualizacao/reiniciar", axum::routing::post(crate::update::http::restart).fallback(pass_any))
        .route("/api/atualizacao/dev-mode", axum::routing::post(crate::update::http::switch_dev_mode).fallback(pass_any))
        // Aviso importante aceito na tela: só do dono, em todo modo, nunca repassado.
        .route("/api/update/accept", axum::routing::post(crate::update::http::accept))
        // Canal de testes: canal publicado no pacote ou branch do checkout.
        .route("/api/update-channel", axum::routing::any(crate::update::git_engine::http::channel))
        // A tela web é servida aqui; o que não é dela segue ao Python.
        .fallback(crate::ui::fallback)
        .layer(axum::middleware::from_fn(crate::migration_status::count_public))
        .with_state(state.clone())
        .merge(owner_share_router(&state, pair))
        .layer(axum::middleware::from_fn(request_auth))
        .layer(axum::middleware::from_fn_with_state(state.clone(),crate::engines::cors_layer))
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
        .layer(axum::middleware::from_fn(request_auth))
        .layer(axum::middleware::from_fn_with_state(state.clone(), crate::startup::guard))
        .with_state(state)
}

async fn health(State(st): State<Arc<AppState>>, headers: HeaderMap) -> Response {
    let owns: Vec<_> = crate::session_write::table::owned_modes().into_iter()
        .map(|(provider, headless)| serde_json::json!({"provider": provider.name(), "headless": headless})).collect();
    let mut body = serde_json::json!({"ok": true, "version": env!("CARGO_PKG_VERSION"),
        "protocol": crate::INTERNAL_PROTOCOL,
        // A prova de vida do atualizador (`update::HttpHealth`): pid novo = processo novo.
        "pid": std::process::id(),
        "owns": owns,
        // O painel de terminal real é do Rust em todas as plataformas.
        "terminal_panel": true,
        // Sem as pastas da lista não há serviço de grupos: as rotas seguem ao Python, que fica dono.
        "groups": st.groups.is_some(),
        "guest_port_bound": st.guest_bound.load(Ordering::Relaxed),
        "terminal_address": st.terminal_address.map(|a| a.to_string())});
    // Versão e commit do pacote de onde este processo subiu; ausentes no modo desenvolvedor.
    if let Some(package) = crate::update::running_package() {
        body["package_version"] = package.version.clone().into();
        body["package_commit"] = package.commit.clone().into();
    }
    let mut resp = ([(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response();
    cors(&headers, resp.headers_mut());
    resp
}

/// `GET /api/client-api`: só o dono.
async fn client_api(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let mut resp = if owner { client_api_reply() } else { crate::share_reach::unauthorized(&st, &fwd.client_ip).await };
    cors(req.headers(), resp.headers_mut());
    resp
}

pub(crate) fn client_api_reply() -> Response {
    ([(header::CONTENT_TYPE, "application/json")], serde_json::json!({"api": hangar_api::CLIENT_API}).to_string()).into_response()
}

use hangar_core::edge::RequestAuth;

async fn workflows(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if let Some(response) = hangar_core::http_cors::preflight(req.method(), req.headers()) {
        return response;
    }
    let method = req.method().clone();
    let headers = req.headers().clone();
    let mut response = workflows_reply(&st, peer, req).await;
    hangar_core::http_cors::apply(&headers, &mut response);
    hangar_core::http_route_response::suppress_head(&method, &mut response);
    st.diag.report_response("rust.workflows_failed", "", &response, "a leitura do workflow falhou");
    response
}

async fn workflows_reply(st: &AppState, peer: SocketAddr, req: Request) -> Response {
    use hangar_core::http::{detail, json_response};
    use serde_json::json;

    let path = percent_encoding::percent_decode_str(req.uri().path()).decode_utf8_lossy();
    let decision = hangar_core::http_contract::resolve(hangar_workflows::ROUTES, req.method().as_str(), &path);
    let (_, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    if let Some(response) = hangar_core::http_route_response::response(
        &decision, req.method(), req.uri(), req.headers(), https, &st.cfg.listen.to_string(),
    ) {
        return response;
    }
    match crate::share::caller::caller(&req) {
        Some(Caller::Guest(grant)) => {
            if !crate::share::caller::guest_allowed(req.method(), req.uri().path(), grant) {
                return detail(StatusCode::FORBIDDEN, "erro_fora_do_convite", "fora da sessão compartilhada", json!({}));
            }
        }
        _ => if let Err(response) = owner_gate(st, peer, &req).await { return response; },
    }
    let parts = path.split('/').collect::<Vec<_>>();
    let name = parts[3];
    let missing = || detail(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "session or transcript not found", json!({}));
    let info = match tokio::time::timeout(crate::locate::LOCATE_TIMEOUT, st.info(name)).await {
        Ok(Ok(Some(info))) => info,
        Ok(Ok(None)) => return missing(),
        Ok(Err(error)) => return detail(StatusCode::SERVICE_UNAVAILABLE, error.0, "a descoberta da sessão falhou", json!({})),
        Err(_) => return detail(StatusCode::SERVICE_UNAVAILABLE, "locate_timeout", "a lista de sessões não respondeu a tempo", json!({})),
    };
    let Some(transcript) = info.jsonl.filter(|path| !path.as_os_str().is_empty()) else { return missing() };
    let run_id = parts.get(5).map(|part| (*part).to_owned());
    let agent_id = parts.get(7).map(|part| (*part).to_owned());
    let missing_code = if agent_id.is_some() { "erro_agente_inexistente" } else { "erro_workflow_inexistente" };
    let missing_message = if agent_id.is_some() { "agent not found" } else { "workflow run not found" };
    match tokio::task::spawn_blocking(move || {
        match (run_id, agent_id) {
            (Some(run), Some(agent)) => hangar_workflows::get_agent(&transcript, &run, &agent),
            (Some(run), None) => hangar_workflows::get_workflow(&transcript, &run),
            _ => hangar_workflows::list_workflows(&transcript).map(Some),
        }
    }).await {
        Ok(Ok(Some(value))) => json_response(StatusCode::OK, value),
        Ok(Ok(None)) => detail(StatusCode::NOT_FOUND, missing_code, missing_message, json!({})),
        _ => hangar_core::http_cors::internal_error(),
    }
}

async fn request_auth(mut req: Request, next: axum::middleware::Next) -> Response {
    // O porteiro externo e o roteador interno compartilham a mesma admissão.
    if req.extensions().get::<RequestAuth>().is_none() {
        req.extensions_mut().insert(RequestAuth::default());
    }
    // Cliente nenhum escolhe uma admissão anterior.
    req.headers_mut().remove("x-hangar-auth-receipt");
    req.headers_mut().remove(crate::proxy::AUTH_EMPTY_QUERY);
    next.run(req).await
}

pub(crate) fn gate(st: &AppState, peer: SocketAddr, req: &Request)
    -> impl std::future::Future<Output = Result<(Forward, bool), auth::AuthError>> + Send + use<> {
    hangar_core::edge::gate(&st.cfg.trusted, &st.auth, peer, req)
}

pub(crate) fn owner_gate(st: &AppState, peer: SocketAddr, req: &Request)
    -> impl std::future::Future<Output = Result<Forward, Response>> + Send + use<> {
    hangar_core::edge::owner_gate(&st.cfg.trusted, &st.auth, peer, req)
}

/// Quem não pode pedir uma rota que o Python não tem mais: a recusa sai daqui, contando a falha de
/// token, porque o 404 do Python responderia antes de autenticar.
pub(crate) async fn refuse_without_python(st: &AppState, peer: SocketAddr, req: Request, who: Option<&Caller>) -> Response {
    let headers = req.headers().clone();
    // Convidado já decidido por um porteiro não errou token: recusar sem contar falha.
    let guest = matches!(who.or(crate::share::caller::caller(&req)), Some(Caller::Guest(_)));
    let mut response = match guest {
        true => hangar_core::http::detail(StatusCode::FORBIDDEN, "erro_fora_do_convite",
            "fora da sessão compartilhada", serde_json::json!({})),
        false => match owner_gate(st, peer, &req).await {
            Err(response) => response,
            Ok(fwd) => return pass(st, req, &fwd).await,
        },
    };
    cors(&headers, response.headers_mut());
    response
}

pub(crate) fn who(st: &AppState, peer: SocketAddr, req: &Request)
    -> impl std::future::Future<Output = Result<(Forward, Option<Caller>), auth::AuthError>> + Send + use<> {
    hangar_core::edge::who(&st.cfg.trusted, &st.auth, peer, req)
}

pub(crate) fn forward_for(st: &AppState, peer: SocketAddr, req: &Request) -> Forward {
    hangar_core::edge::forward_for(&st.cfg.trusted, peer, req)
}

pub(crate) use hangar_core::edge::on_guest_port;

pub(crate) use hangar_core::http::{cors, gzip_with_minimum, maybe_gzip, not_found};

/// Chegou pela entrada do Connect: o `ConnectConn` só existe nesse listener.
pub(crate) fn on_connect_port(req: &Request) -> bool {
    req.extensions().get::<ConnectInfo<crate::connect::gate::ConnectConn>>().is_some()
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
    let cached = req.extensions().get::<RequestAuth>().cloned();
    let mut admitted_forward = fwd.clone();
    if admitted_forward.auth_receipt.is_none() {
        if let Some(cached) = cached {
            admitted_forward.auth_receipt = cached.receipt().await;
        }
    }
    let upgrade = req.headers().contains_key(header::UPGRADE);
    let configuration = req.uri().path() == "/api/config"
        && matches!(*req.method(), Method::GET | Method::POST | Method::PATCH);
    if configuration { req.headers_mut().remove(header::ACCEPT_ENCODING); }
    let mut resp = proxy::forward(&st.http, st.cfg.upstream, req, &admitted_forward).await;
    if configuration { resp = crate::speech::configuration_reply(st, resp).await; }
    resp.extensions_mut().insert(crate::migration_status::Forwarded);
    if !st.auth.shared() { match resp.status() {
        StatusCode::UNAUTHORIZED => if let Err(e) = st.auth.record_failure(&fwd.client_ip).await { return auth::unavailable(e); },
        // WebSocket recusado antes do aceite chega como 403 e o Python já contou a falha.
        // Um 403 de origem também conta: só desliga o atalho, o lado seguro.
        StatusCode::FORBIDDEN if upgrade => if let Err(e) = st.auth.record_failure(&fwd.client_ip).await { return auth::unavailable(e); },
        StatusCode::TOO_MANY_REQUESTS => if let Err(e) = st.auth.mark_blocked_async(&fwd.client_ip).await { return auth::unavailable(e); },
        _ => {}
    } }
    resp
}

// O porteiro concede os catálogos globais só a convite, nunca a um par de leitura.
fn invited_engine_catalog(caller: &Option<Caller>, req: &Request) -> bool {
    on_guest_port(req) && req.method() == Method::GET
        && matches!(req.uri().path(), "/api/engines" | "/api/model-options")
        && matches!(caller, Some(Caller::Guest(grant)) if grant.invite)
}

fn login_catalog_read(req: &Request) -> bool {
    req.extensions().get::<crate::login_guests::gate::Login>().is_some()
        && req.method() == Method::GET
        && matches!(req.uri().path(), "/api/engines" | "/api/providers" | "/api/model-options")
}

pub(crate) async fn pass_any(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut req: Request,
) -> Response {
    if crate::speech::http::matches(req.method(), req.uri().path()) {
        return crate::speech::http::public(st, peer, req).await;
    }
    if crate::auxiliary::owns_path(req.uri().path()) {
        return crate::auxiliary::handle(State(st), ConnectInfo(peer), req).await;
    }
    let decoded_path=percent_encoding::percent_decode_str(req.uri().path()).decode_utf8_lossy();
    if crate::worktree_fetch::owns(&decoded_path) {
        return crate::worktree_fetch::route(st,peer,req).await;
    }
    if crate::harness_setup::http::matches(&decoded_path) {
        return crate::harness_setup::routing::handle(st, peer, req).await;
    }
    if crate::config_sync::runtime_http::owns_path(req.uri().path()) {
        return crate::config_sync::runtime_http::handle(State(st),ConnectInfo(peer),req).await;
    }
    if crate::update::git_engine::http::owns_channel_path(&decoded_path)||crate::update::git_engine::http::owns_update_path(&decoded_path){
        return crate::update::git_engine::http::channel(State(st),ConnectInfo(peer),req).await;
    }
    if crate::machine_peers::owns_http_path(&decoded_path){
        return crate::machine_peers::handle(State(st),ConnectInfo(peer),req).await;
    }
    if crate::push::http::owns_path(req.uri().path()) {
        return crate::push::http::route(State(st),ConnectInfo(peer),req).await;
    }
    if crate::loops::http::owns_path(req.uri().path()) {
        return crate::loops::http::route(State(st),ConnectInfo(peer),req).await;
    }
    if crate::login_guests::owns_path(req.uri().path()) {
        return crate::login_guests::route(State(st),ConnectInfo(peer),req).await;
    }
    if crate::dictation::http::matches(req.method(), req.uri().path()) {
        return crate::dictation::http::public(st, peer, req).await;
    }
    if crate::provider_models::inbox_http_request(req.method(), req.uri().path(), req.headers()) {
        return crate::session_write::json_response(StatusCode::NOT_FOUND, serde_json::json!({"detail":"Not Found"}));
    }
    if let Some(mut response) = crate::provider_info::wrong_method(req.method(), req.uri().path(), req.uri().query()) {
        cors(req.headers(), response.headers_mut());
        return response;
    }
    if crate::engines::model_http::candidate(req.method(),req.uri().path()) {
        if !on_guest_port(&req) && req.extensions().get::<crate::login_guests::gate::Login>().is_none() {
            if let Err(mut response) = owner_gate(&st, peer, &req).await {
                crate::http_cors::apply(req.headers(), &mut response);
                return response;
            }
        }
        let (forward,caller)=match who(&st,peer,&req).await {Ok(value)=>value, Err(e)=>return crate::auth::unavailable(e)};
        let session_read = req.extensions().get::<SessionPath>().is_some_and(|path|
            caller.as_ref().is_some_and(|who| who.can(&path.session, req.method())));
        let catalog_read = invited_engine_catalog(&caller, &req) || login_catalog_read(&req);
        if !matches!(caller,Some(Caller::Owner)) && !session_read && !catalog_read {
            return if caller.is_some() {StatusCode::FORBIDDEN.into_response()} else {crate::share_reach::unauthorized(&st,&forward.client_ip).await};
        }
        let (method,path,query)=(req.method().clone(),req.uri().path().to_owned(),req.uri().query().map(str::to_owned));
        if let Some(mut response)=crate::engines::model_http::response(&st,&method,&path,query.as_deref()).await {
            crate::http_cors::apply(req.headers(),&mut response);return response;
        }
    }
    let models = crate::provider_models::matches(req.method(), req.uri().path())
        || crate::provider_models::catalog_request(req.method(), req.uri().path(), req.uri().query());
    let orchestration = crate::orchestration::matches(req.method(), req.uri().path());
    let sync = crate::omp_sync::matches(req.method(), req.uri().path());
    let engines = crate::engines::matches(req.method(), req.uri().path());
    if engines && !on_guest_port(&req) && !login_catalog_read(&req) {
        if let Err(mut response) = owner_gate(&st, peer, &req).await {
            crate::http_cors::apply(req.headers(), &mut response);
            return response;
        }
    }
    if models || orchestration || sync || engines {
        let (forward, caller) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
        let allowed = matches!(&caller, Some(Caller::Owner))
            || ((engines || models) && login_catalog_read(&req))
            || (engines && invited_engine_catalog(&caller, &req))
            || (models && req.extensions().get::<SessionPath>()
            .is_some_and(|path| caller.as_ref().is_some_and(|who| who.can(&path.session, req.method()))));
        if !allowed {
            return if caller.is_some() { StatusCode::FORBIDDEN.into_response() }
                else { crate::share_reach::unauthorized(&st, &forward.client_ip).await };
        }
        req.extensions_mut().insert(ConnectInfo(peer));
        let headers = req.headers().clone();
        let mut response = if engines { crate::engines::serve(&st, req).await }
            else if models { crate::provider_models::serve(&st, req).await }
            else if orchestration { crate::orchestration::serve(&st, req).await }
            else if let Some(service) = st.omp_sync.get() { crate::omp_sync::serve(&st, service, req).await }
            else { crate::session_write::json_response(StatusCode::SERVICE_UNAVAILABLE, serde_json::json!({"code":"omp_sync_unavailable"})) };
        if engines { crate::http_cors::apply(&headers, &mut response); }
        else { cors(&headers, response.headers_mut()); }
        return response;
    }
    // Anexos e contas do dono são do Rust; o anexo também do convidado da sessão (o porteiro já
    // limitou ao nível: par só baixa um arquivo).
    let uploads = crate::uploads::http::matches(req.method(), req.uri().path());
    // Valores próprios: um `&Request` vivo através de `await` tira o `Send` do handler.
    let (session, method) = (req.extensions().get::<SessionPath>().map(|p| p.session.clone()), req.method().clone());
    let session_ok = |who: &Option<Caller>| who.as_ref().is_some_and(|c| session.as_ref().is_some_and(|s| c.can(s, &method)));
    if (uploads || crate::accounts::http::matches(req.method(), req.uri().path()))
        && match (match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) }).1 {
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
        let (forward, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
        let headers = req.headers().clone();
        let mut response = if owner {
            crate::share_reach::public(st, req).await
        } else {
            crate::share_reach::unauthorized(&st, &forward.client_ip).await
        };
        cors(&headers, response.headers_mut());
        return response;
    }
    if crate::session_open::baton::matches(req.uri().path()) {
        return crate::session_open::baton::serve(State(st), ConnectInfo(peer), req).await;
    }
    if crate::worktree_routes::matches(req.method(), req.uri().path()) {
        return crate::worktree_routes::route(st, peer, req).await;
    }
    if crate::workspace_routes::matches(req.method(), req.uri().path()) {
        return crate::workspace_routes::public(st, peer, req).await;
    }
    if crate::shortcuts::http::matches(req.uri().path()) {
        return crate::shortcuts::http::public(st, peer, req).await;
    }
    if crate::costs_routes::usage_path(req.uri().path()) {
        return crate::costs_routes::usage(State(st), ConnectInfo(peer), req).await;
    }
    if crate::diag::version_matches(req.method(), req.uri().path()) {
        return crate::diag::version_route(st, peer, req).await;
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

fn native_history_limit(raw: &str) -> Result<Option<usize>, ()> {
    let raw = raw.trim();
    let (negative, unsigned) = if let Some(rest) = raw.strip_prefix('-') { (true, rest) }
        else { (false, raw.strip_prefix('+').unwrap_or(raw)) };
    let integer = match unsigned.split_once('.') {
        Some((integer, fraction)) if !fraction.is_empty() && fraction.bytes().all(|byte| byte == b'0') => integer,
        Some(_) => return Err(()),
        None => unsigned,
    };
    let mut digit = false;
    for byte in integer.bytes() {
        if byte.is_ascii_digit() { digit = true; }
        else if byte == b'_' && digit { digit = false; }
        else { return Err(()); }
    }
    if !digit { return Err(()); }
    if negative { return Ok(None); }
    // Limite maior que o arquivo equivale ao histórico inteiro, inclusive além do usize.
    Ok(integer.replace('_', "").parse::<usize>().ok().filter(|limit| *limit > 0))
}

fn invalid_history_limit(raw: String, headers: &axum::http::HeaderMap) -> Response {
    let mut response = crate::session_write::json_response(StatusCode::UNPROCESSABLE_ENTITY, serde_json::json!({"detail":[{
        "type":"int_parsing","loc":["query","limit"],"msg":"Input should be a valid integer, unable to parse string as an integer","input":raw
    }]}));
    cors(headers, response.headers_mut());
    response
}

pub(crate) async fn history(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    req: Request,
) -> Response {
    let (fwd, who) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let name = match path {
        Ok(Path(n)) if req.method() == Method::GET && who.as_ref().is_some_and(|c| c.can(&n, req.method())) => n,
        _ => return pass(&st, req, &fwd).await,
    };
    let mut selected_info = None;
    let limit = match auth::query_param(req.uri().query(), "limit") {
        None => None,
        Some(v) => match v.parse::<i64>() {
            // Como o Python: `limit <= 0` é o histórico inteiro (api.py:2918), nunca um 400.
            Ok(n) if n > 0 => usize::try_from(n).ok(),
            Ok(_) => None,
            // Texto ou vazio fica com o FastAPI e o 422 dele.
            Err(_) => {
                let info = match st.info(&name).await {
                    Ok(info) => info,
                    Err(_) => return route_failed(&st, req.headers(), "rust.history_failed", &name, "internal_info", INFO_REASON),
                };
                if info.as_ref().is_some_and(|info| matches!(info.provider.as_str(), "pi" | "omp" | "kimi")) {
                    let parsed = match native_history_limit(&v) { Ok(limit) => limit, Err(()) => return invalid_history_limit(v, req.headers()) };
                    selected_info = info;
                    parsed
                } else if req.extensions().get::<crate::forwarded::ReadGuest>().is_some() {
                    return invalid_history_limit(v, req.headers());
                } else {
                    return pass(&st, req, &fwd).await;
                }
            }
        },
    };
    let found = match selected_info { Some(info) => Ok(Some(info)), None => st.info(&name).await };
    let Ok(info) = found else {
        return route_failed(&st, req.headers(), "rust.history_failed", &name, "internal_info", INFO_REASON);
    };
    // Sessão inexistente ou provedor fora do Rust: o Python é o dono.
    let Some(info) = info else { return pass(&st, req, &fwd).await };
    // Antes de qualquer repasse: sessão em erro ainda sem transcript também é reaberta.
    st.request_reopen(&name);
    if matches!(info.provider.as_str(), "pi" | "omp" | "kimi") && info.jsonl.is_none() {
        let mut response = crate::session_write::json_response(StatusCode::OK, serde_json::json!([]));
        cors(req.headers(), response.headers_mut());
        return response;
    }
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
        let evs = merged_history_with_notices(&hreq)?;
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

pub(crate) async fn events(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    path: Result<Path<String>, PathRejection>,
    req: Request,
) -> Response {
    let (fwd, who) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let name = match path {
        Ok(Path(n)) if req.method() == Method::GET && who.as_ref().is_some_and(|c| c.can(&n, req.method())) => n,
        Ok(_) if req.method() == Method::GET => return refuse_without_python(&st, peer, req, who.as_ref()).await,
        _ => return pass(&st, req, &fwd).await,
    };
    // Convidado não recebe aviso de mod nem pedido de navegador, e não conta como app do dono.
    let guest = matches!(who, Some(Caller::Guest(_))) || req.extensions().get::<crate::forwarded::ReadGuest>().is_some_and(|guest| guest.0);
    let started = std::time::Instant::now();
    let info = match st.info(&name).await {
        Ok(info) => info,
        Err(crate::locate::InfoFailed(cause)) => {
            // A etapa da descoberta que não respondeu e o tempo: `internal_info` sozinho não diz qual.
            st.diag.report_with("rust.events_failed", &name, "internal_info", INFO_REASON,
                serde_json::json!({"etapa": cause, "ms": started.elapsed().as_millis() as u64}));
            return route_failed(&st, req.headers(), "rust.events_failed", &name, "internal_info", INFO_REASON);
        }
    };
    // Antes de qualquer repasse: sessão em erro ainda sem transcript também é reaberta.
    if info.is_some() { st.request_reopen(&name); }
    // Todo provedor é do Rust: sem ligação é sessão inexistente ou ainda sem conversa.
    let Some(binding) = info.as_ref().and_then(Binding::from_info) else {
        let mut response = hangar_core::http::detail(StatusCode::NOT_FOUND, "erro_sessao_inexistente",
            "session or transcript not found", serde_json::json!({}));
        cors(req.headers(), response.headers_mut());
        return response;
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


#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[tokio::test]
    async fn migrated_provider_routes_authenticate_before_validation_and_never_fall_back() {
        let state = state_with_upstream().await;
        let peer = "127.0.0.1:5000".parse().unwrap();
        for path in ["/api/sessions/demo/pi/model", "/api/sessions/demo/kimi/model", "/api/sessions/demo/orq/papeis"] {
            for (token, expected) in [("errado", StatusCode::UNAUTHORIZED), ("dono", StatusCode::UNPROCESSABLE_ENTITY)] {
                let mut req = request(path, "198.51.100.50", false, token);
                *req.method_mut() = Method::POST;
                req.headers_mut().insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
                *req.body_mut() = axum::body::Body::from("[]");
                let response = pass_any(State(state.clone()), ConnectInfo(peer), req).await;
                assert_eq!(response.status(), expected, "{path}, token {token}");
            }
        }
        let response = pass_any(State(state.clone()), ConnectInfo(peer), request("/api/omp/plugin-sync", "198.51.100.51", false, "dono")).await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        let body = axum::body::to_bytes(response.into_body(), 4096).await.unwrap();
        assert_eq!(serde_json::from_slice::<serde_json::Value>(&body).unwrap(), serde_json::json!({"code":"omp_sync_unavailable"}));
        for method in [Method::GET, Method::POST, Method::OPTIONS, Method::HEAD] {
            let mut req = request("/api/pi/inbox", "198.51.100.53", false, "errado");
            *req.method_mut() = method;
            let response = pass_any(State(state.clone()), ConnectInfo(peer), req).await;
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    #[tokio::test]
    async fn migrated_provider_forwarding_checks_the_private_secret_and_preserves_guest_limits() {
        let state = state_with_upstream().await;
        let peer = "127.0.0.1:5000".parse().unwrap();
        for (path, guest, secret, expected) in [
            ("/api/sessions/demo/pi/model", false, "errado", StatusCode::NOT_FOUND),
            ("/api/sessions/demo/pi/model", true, "s", StatusCode::UNPROCESSABLE_ENTITY),
            ("/api/sessions/demo/orq/papeis", true, "s", StatusCode::NOT_FOUND),
            ("/api/sessions/demo/orq/papeis", false, "s", StatusCode::UNPROCESSABLE_ENTITY),
        ] {
            let req = Request::builder().method(Method::POST).uri("/__hangar_server/forwarded")
                .header("x-hangar-internal", secret).header("x-hangar-path", path)
                .header("x-hangar-forwarded-guest", if guest { "1" } else { "0" })
                .header(header::CONTENT_TYPE, "application/json")
                .body(axum::body::Body::from("[]")).unwrap();
            let response = crate::forwarded::private(State(state.clone()), ConnectInfo(peer), req).await;
            assert_eq!(response.status(), expected, "{path}, convidado {guest}, segredo {secret}");
        }
    }

    #[tokio::test]
    async fn orchestration_history_uses_the_same_parser_on_public_and_forwarded_ports() {
        const NAME: &str = "routes::tests::orchestration_history_uses_the_same_parser_on_public_and_forwarded_ports";
        if std::env::var_os("ORQ_REGISTRY_PROBE").is_none() {
            // O leitor do orq é global ao processo: num filho, só o AppState pode tê-lo registrado.
            let out = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", NAME, "--test-threads=1"]).env("ORQ_REGISTRY_PROBE", "1").output().unwrap();
            let stdout = String::from_utf8_lossy(&out.stdout);
            assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{}", String::from_utf8_lossy(&out.stderr));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let timeline = dir.path().join("timeline.jsonl");
        let lines = crate::orchestration::orq_cases::CASES.iter().map(|(input, _)| format!("{input}\n")).collect::<String>();
        std::fs::write(&timeline, lines).unwrap();
        let mut state = Arc::try_unwrap(state_with_upstream().await).ok().unwrap();
        state.side.locate = Arc::new(crate::locate::Fixed(Some(InternalInfo {
            provider: "orq".into(), jsonl: Some(timeline), session_key: "run".into(),
            history: serde_json::Value::Null, headless: false, lifecycle_id: Some("life".into()), cwd: None,
        })));
        let state = Arc::new(state);
        let peer = "127.0.0.1:5000".parse().unwrap();
        let public = history(State(state.clone()), ConnectInfo(peer), Ok(Path("run-orq".into())),
            request("/api/sessions/run-orq/history", "198.51.100.52", false, "dono")).await;
        assert_eq!(public.status(), StatusCode::OK);
        let expected = axum::body::to_bytes(public.into_body(), 1024 * 1024).await.unwrap();
        assert!(!serde_json::from_slice::<Vec<serde_json::Value>>(&expected).unwrap().is_empty());
        for guest in [false, true] {
            let req = Request::builder().uri("/__hangar_server/forwarded")
                .header("x-hangar-internal", "s").header("x-hangar-path", "/api/sessions/run-orq/history")
                .header("x-hangar-forwarded-guest", if guest { "1" } else { "0" })
                .body(axum::body::Body::empty()).unwrap();
            let response = crate::forwarded::private(State(state.clone()), ConnectInfo(peer), req).await;
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(axum::body::to_bytes(response.into_body(), 1024 * 1024).await.unwrap(), expected);
        }
        let req = Request::builder().uri("/__hangar_server/forwarded?limit=invalido")
            .header("x-hangar-internal", "s").header("x-hangar-path", "/api/sessions/run-orq/history")
            .body(axum::body::Body::empty()).unwrap();
        let response = crate::forwarded::private(State(state), ConnectInfo(peer), req).await;
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

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
        st.auth.is_owner(ip, Some(b"dono"))
    }

    #[tokio::test]
    async fn websocket_403_and_429_from_python_lock_the_shortcut() {
        let st = state_with_upstream().await;
        let peer: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let fwd = |ip: &str| forward_for(&st, peer, &request("/x", ip, false, "x"));

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
