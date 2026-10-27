//! Servidor do Hangar: lê conversas e custos das sessões e repassa o restante ao backend Python.
pub mod accounts;
pub mod artifacts;
pub mod auth;
pub mod config;
pub mod commands;
pub mod orq_conductor;
pub mod orq_storage;
pub mod claude_customizations;
pub mod jev_aux;
pub mod keyring;
pub mod claude_defaults;
pub mod presence;
pub mod watchdog;
pub mod costs;
pub mod costs_routes;
pub mod costs_failure;
pub mod device_install;
pub mod diag;
pub mod display_env;
pub mod groups;
pub mod interactions;
pub mod list;
pub mod locate;
pub mod loops;
pub mod mcp;
pub mod mermaid;
pub mod migration_status;
pub mod mods;
pub mod nav_requests;
pub mod pages;
pub mod prewarm;
pub(crate) mod queue_feed;
pub mod plugin;
pub mod problem_gate;
pub mod proxy;
pub mod query;
pub mod queue_panel;
mod plugin_listener;
pub mod routes;
pub mod runtime;
pub mod session_open;
pub mod session_rename;
pub mod session_defaults;
pub mod push;
pub mod push_expo;
pub mod session_media;
pub mod session_write;
pub mod share_reach;
pub mod side;
pub mod share;
pub mod state;
pub mod stats;
pub mod subagents;
pub mod tail;
pub mod term;
pub mod terminal_state;
pub mod terminal_control;
pub mod terminal_input;
pub mod permission_control;
pub mod model_picker;
pub mod terminal_facts;
mod terminal_process;
mod thumbnail;
pub mod terminal_routes;
pub mod transcript;
pub mod transcription;
pub mod update_notes;
pub mod dictation;
pub mod workspace_routes;
pub mod uploads;
pub mod voice;
pub mod plans;
mod forwarded;
pub mod worktree_routes;
pub mod worktree_policy;
mod warn_limit;

/// Versão do contrato com o Python (rotas `/internal`, canal privado do estado, ambiente). O
/// Python (`RUST_SERVER_PROTOCOL`) recusa um binário de outra versão e atende sozinho.
pub const INTERNAL_PROTOCOL: u32 = 90;

/// Todo cliente HTTP do servidor nasce aqui. Uma dependência liga `gzip`, `http2` e (no Windows)
/// `native-tls` no reqwest, e o Cargo estende isso a todo cliente: sem estes desligamentos ele
/// passaria a descomprimir sozinho, negociar HTTP/2 e usar o TLS do sistema.
pub fn http_client() -> reqwest::ClientBuilder {
    reqwest::Client::builder().no_gzip().http1_only().tls_backend_rustls()
}

/// Todo socket TCP do servidor, aceito ou aberto. Sem isso o Nagle segura o último pedaço de uma
/// resposta em pedaços até o ACK atrasado do outro lado; o asyncio do Python já liga sozinho.
pub(crate) fn nodelay(tcp: &mut tokio::net::TcpStream) {
    if let Err(e) = tcp.set_nodelay(true) {
        tracing::warn!("TCP_NODELAY não ligou: {e}");
    }
}

/// Lê o cano até o fim ou erro. O Python segura a outra ponta; fechou = pai morreu.
pub async fn parent_gone<R: tokio::io::AsyncRead + Unpin>(mut pipe: R) {
    use tokio::io::AsyncReadExt;
    let mut buf = [0u8; 64];
    while let Ok(n) = pipe.read(&mut buf).await {
        if n == 0 {
            return;
        }
    }
}

/// `routes::serve` até `stop` terminar. Sem esperar as conexões abertas: SSE nunca fecha sozinho.
/// `Ok(())` só quando parou por `stop`.
pub async fn serve_until(
    listener: tokio::net::TcpListener,
    cfg: config::Config,
    stop: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    serve_until_with_state(listener, routes::AppState::new(cfg), stop).await
}

pub async fn serve_until_with_state(
    listener: tokio::net::TcpListener,
    state: routes::AppState,
    stop: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    serve_until_with_guest(listener, state, None, stop).await
}

/// `guest`: porta do convidado (`HANGAR_SERVER_GUEST_LISTEN`), ligada sem segurar a principal.
pub async fn serve_until_with_guest(
    listener: tokio::net::TcpListener,
    state: routes::AppState,
    guest: Option<std::net::SocketAddr>,
    stop: impl std::future::Future<Output = ()>,
) -> std::io::Result<()> {
    let cfg = state.cfg.clone();
    let transcription = state.transcription.clone();
    let codex_logins=state.accounts.codex_logins.clone();
    let codex_readers=state.accounts.codex_readers.clone();
    let device_logins=state.accounts.device_logins.clone();
    let mut runtime_registry=None;
    let mut refresh_loop=None;
    let result=async {
    if let Some(home) = std::env::home_dir() {
        transcription.recover_on_startup(&home).await;
    }
    if let Some(instance) = config::Config::runtime_instance().map_err(std::io::Error::other)? {
        let windows = accounts::claude_login::WindowClient::new(cfg.upstream, cfg.internal_secret.clone(), instance.clone())
            .map_err(|error| std::io::Error::other(error.code))?;
        // Sem catálogo legível não há janela a fechar; isso não pode impedir o servidor de subir.
        if let Err(error) = state.accounts.recover_claude_logins(windows).await {
            tracing::warn!(code = "claude_login_recovery_failed", reason = error.code, "catálogo de contas ilegível na subida");
        }
        let private = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = private.local_addr()?.port();
        let registry = std::sync::Arc::new(runtime::gateway::RuntimeRegistry::new(cfg.upstream,
            cfg.internal_secret.clone(),instance.clone()).with_mods(state.mods.clone()).with_accounts(state.accounts.clone())
            .with_interactions(state.interactions.clone()).with_plugin(state.plugin.clone())
            .with_terminal_states(state.list.published.clone()).with_terms(state.term.clone())
            .with_permissions(state.state.permissions.clone())
            .with_plugin_bridge(session_open::claude::PluginBridge { port: cfg.listen.port(), secret: cfg.auth_token.clone() }));
        registry.bind_lifecycle();
        state.state.lifecycle.set(registry.clone()).map_err(|_|std::io::Error::other("state_lifecycle_already_bound"))?;
        runtime_registry=Some(registry.clone());
        state.list.set_runtime(registry.clone());
        state.list.set_plugin_runtime(registry.clone());
        let _ = state.state.runtime.set(registry.clone());
        // O hub do chat pergunta ao registro quem é dono do Codex com terminal, e o registro o avisa na troca.
        registry.bind_hubs(state.side.hubs.downgrade());
        let _ = state.side.runtime.set(registry.clone());
        let _ = state.accounts.quotas.runtime.set(registry.clone());
        let _ = state.accounts.quotas.diag.set(state.diag.clone());
        let quota_bridge=accounts::bridge::AccountsBridge::new(cfg.upstream,cfg.internal_secret.clone(),instance.clone())
            .map_err(std::io::Error::other)?;
        refresh_loop=Some(accounts::claude_refresh::start(state.accounts.clone(),quota_bridge,registry.clone()));
        let _watchdog = std::env::home_dir().map(|home| AbortOnDrop(tokio::spawn(
            watchdog::run(state.http.clone(), cfg.upstream, cfg.internal_secret.clone(), home))));
        // A troca de conta que o Rust anterior deixou no meio, antes de qualquer sessão subir pelo spec.
        { let env=registry.codex_environment();
          if tokio::task::spawn_blocking(move||runtime::claude_reopen::settle_switches(&env)).await.is_err() {
              tracing::warn!("trocas de conta interrompidas não foram conferidas");
          } }
        // A vigia do Codex com terminal precisa das pastas da lista (sidecars e fila).
        let _codex_watch = match state.list.env().dirs.clone() {
            Some(dirs) => {
                let watcher = std::sync::Arc::new(runtime::codex_terminal::Watcher::new(runtime::codex_terminal::WatchConfig {
                    home: dirs.home.clone(), queue_dir: dirs.claude.join(".hangar-queue"),
                    mux: list::mux::Mux::default(), mux_program: state.list.env().capture_program.clone() }));
                registry.bind_watcher(watcher.clone());
                // Presas ao desligamento: uma adoção tardia abriria sessão depois do `registry.shutdown()`.
                // Os Claude com terminal que já tinham fila aqui: o Rust os adota, sem esperar o Python.
                let claude_terminals = { let (watcher,registry)=(watcher.clone(),registry.clone());
                  AbortOnDrop(tokio::spawn(async move { watcher.register_claude_terminals(&registry).await; })) };
                // E os Codex sem terminal com cano vivo ou mensagem esperando.
                let codex_headless = { let (watcher,registry)=(watcher.clone(),registry.clone());
                  AbortOnDrop(tokio::spawn(async move { watcher.register_codex_headless(&registry).await; })) };
                vec![claude_terminals,codex_headless,AbortOnDrop(tokio::spawn(watcher.run(registry.clone())))]
            }
            _ => Vec::new(),
        };
        // As sessões Claude sem terminal que já existem: o Rust as reconhece sozinho, sem esperar o Python.
        let _claude_boot = AbortOnDrop(tokio::spawn(registry.clone().register_claude_headless()));
        let _claude_park = AbortOnDrop(tokio::spawn(registry.clone().watch_idle_claude()));
        println!("{}",runtime::gateway::startup_line(INTERNAL_PROTOCOL,&instance,port));
        let gateway = runtime::gateway::serve(private,registry.clone(),cfg.internal_secret.clone(),instance,INTERNAL_PROTOCOL);
        let result = tokio::select! {
            result = routes::serve_with_guest(listener,state,guest) => result,
            result = gateway => result,
            () = stop => Ok(()),
        };
        return result;
    }
    tokio::select! {
        r = routes::serve_with_guest(listener, state, guest) => r,
        () = stop => Ok(()),
    }
    }.await;
    transcription.shutdown().await;
    if let Some((stop,task))=refresh_loop {
        let _=stop.send(true);
        let _=task.await;
    }
    tokio::join!(codex_readers.close(),codex_logins.close(),device_logins.close());
    if let Some(registry)=runtime_registry {
        registry.shutdown().await.map_err(|error|std::io::Error::other(error.code))?;
    }
    result
}

pub(crate) struct AbortOnDrop(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) { self.0.abort(); }
}

/// Linha do log de um pânico: só local e thread. A mensagem do pânico pode citar texto de conversa.
fn panic_line(loc: Option<&std::panic::Location<'_>>, thread: Option<&str>) -> String {
    let at = loc.map_or_else(|| "?".to_string(), |l| format!("{}:{}", l.file(), l.line()));
    format!("pânico em {at} (thread {})", thread.unwrap_or("?"))
}

/// Fixa o limite de devolução de memória do glibc. Sem isso, o limite cresce sozinho depois de
/// cada bloco grande liberado e a memória das leituras paralelas do índice de custos fica retida.
/// Em plataformas sem glibc, não faz nada.
///
/// # Segurança
///
/// No Linux GNU, deve ser chamada durante a inicialização do processo, antes de criar qualquer
/// outra thread, inclusive as do runtime Tokio. O glibc altera parâmetros do alocador sem
/// sincronização com as threads que podem lê-los durante alocações.
pub unsafe fn tune_allocator() {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    {
        unsafe extern "C" { fn mallopt(param: i32, value: i32) -> i32; }
        const M_TRIM_THRESHOLD: i32 = -1;
        // A pré-condição do chamador impede alocações concorrentes durante a configuração do glibc.
        unsafe { mallopt(M_TRIM_THRESHOLD, 128 * 1024); }
    }
}

/// Troca o hook padrão, que imprime a mensagem no stderr (o Python o herda) mesmo quando o
/// pânico é capturado. Chamar depois de `init_log`, para a linha sair no mesmo destino.
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let t = std::thread::current();
        tracing::error!("{}", panic_line(info.location(), t.name()));
    }));
}

const LOG_MAX_BYTES: u64 = 4 * 1024 * 1024;
const LOG_BACKUPS: u32 = 3;

/// Log em arquivo (HANGAR_SERVER_LOG) ou no stderr. Nunca recebe texto de conversa.
pub fn init_log(path: Option<&std::path::Path>) {
    if let Some(p) = path {
        rotate_log(p, LOG_MAX_BYTES);
    }
    // Sem estes avisos, o log ia para o stderr (o Python o herda) sem dizer por quê.
    let file = path.and_then(|p| match std::fs::OpenOptions::new().create(true).append(true).open(p) {
        Ok(f) => Some(f),
        Err(e) => {
            eprintln!("hangar-server: log em {} não abriu ({e}); segue no stderr", p.display());
            None
        }
    });
    let builder = tracing_subscriber::fmt().with_target(false);
    let init = match file {
        Some(f) => builder.with_writer(std::sync::Mutex::new(f)).try_init(),
        None => builder.with_writer(std::io::stderr).try_init(),
    };
    if let Err(e) = init {
        eprintln!("hangar-server: log não iniciou: {e}");
    }
}

/// Mesmo teto dos logs privados do Python (4 MB, três cópias `.1`..`.3`), conferido ao subir.
// ponytail: só na subida; um processo que viva muito pode passar do teto até o próximo início.
// Girar em execução pede um writer próprio, se o arquivo crescer assim na prática.
fn rotate_log(path: &std::path::Path, max: u64) {
    if std::fs::metadata(path).map_or(true, |m| m.len() < max) {
        return;
    }
    let numbered = |i: u32| {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{i}"));
        std::path::PathBuf::from(s)
    };
    // remove antes do rename: no Windows o rename não sobrescreve.
    let _ = std::fs::remove_file(numbered(LOG_BACKUPS));
    for i in (1..LOG_BACKUPS).rev() {
        let _ = std::fs::rename(numbered(i), numbered(i + 1));
    }
    let _ = std::fs::rename(path, numbered(1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_line_has_location_and_thread_only() {
        let loc = std::panic::Location::caller();
        let line = panic_line(Some(loc), Some("worker"));
        assert!(line.contains(&format!("{}:{}", loc.file(), loc.line())));
        assert!(line.contains("worker"));
        let secret = "texto-da-conversa-xyz";
        assert!(!line.contains(secret));
        assert_eq!(panic_line(None, None), "pânico em ? (thread ?)");
    }

    #[test]
    fn log_rotates_past_the_limit_keeping_three_copies() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("hangar-server.log");
        let read = |n: &str| std::fs::read_to_string(dir.path().join(n)).ok();
        std::fs::write(&p, "abc").unwrap();
        rotate_log(&p, 10);
        assert_eq!(read("hangar-server.log").as_deref(), Some("abc"), "abaixo do teto fica");
        for round in ["g1", "g2", "g3", "g4"] {
            std::fs::write(&p, format!("{round}-cheio-demais")).unwrap();
            rotate_log(&p, 10);
        }
        assert_eq!(read("hangar-server.log"), None);
        assert_eq!(read("hangar-server.log.1").as_deref(), Some("g4-cheio-demais"));
        assert_eq!(read("hangar-server.log.3").as_deref(), Some("g2-cheio-demais"));
        assert_eq!(read("hangar-server.log.4"), None);
    }
}

/// Grava um executável de teste sem abrir o arquivo para escrita neste processo: quem grava é um
/// `install` à parte. Gravado aqui, um filho de outro teste nascido nesse instante herdaria o descritor
/// de escrita, e rodar o arquivo daria "Text file busy" (ETXTBSY).
#[cfg(all(unix, any(test, target_os = "linux")))]
pub(crate) fn write_test_executable(path: &std::path::Path, content: impl AsRef<[u8]>) {
    let source = path.with_extension("conteudo");
    std::fs::write(&source, content).unwrap();
    let status = std::process::Command::new("install").arg("-m").arg("755").arg(&source).arg(path).status().unwrap();
    assert!(status.success(), "install não gravou {}", path.display());
    std::fs::remove_file(&source).unwrap();
}
