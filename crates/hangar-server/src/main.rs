//! hangar-server: o servidor do Hangar, na porta pública.

fn main() -> std::process::ExitCode {
    let hook_args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(mode) = hook_args.first().and_then(|arg| arg.to_str())
        && matches!(mode, "codex-hook-allow" | "codex-hook-json")
    {
        std::process::exit(hangar_server::codex_setup::hook_command::run(mode, &hook_args[1..]));
    }
    if matches!(hook_args.first().and_then(|arg| arg.to_str()), Some("kimi-state-hook" | "guard-tmux")) {
        return hangar_server::provider_hooks::run(hook_args[0].to_str().unwrap());
    }
    // `open`/`resolve`: o lançador como comando de terminal, com runtime próprio (fora do `run`).
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    if args.first().map(String::as_str) == Some("usage") {
        return hangar_server::usage_cli::cli(&args[1..]);
    }
    // Conectores: pelo servidor em execução ou, com `--registry`, num host isolado.
    if args.first().map(String::as_str) == Some("connectors") {
        return hangar_server::connectors_cli::cli(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("peers") {
        return hangar_server::machine_peers::cli::cli(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("pairing") {
        return hangar_server::machine_peers::pairing::cli(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("doctor") {
        return hangar_server::machine_peers::doctor::cli(&args[1..]);
    }
    if matches!(args.first().map(String::as_str), Some("orq-info" | "provider-info")) {
        return hangar_server::provider_info::cli(&args[0], &args[1..]);
    }
    if args.first().is_some_and(|arg| arg == "orq-storage") {
        return hangar_server::orq_storage::cli(&args[1..]);
    }
    if matches!(args.first().map(String::as_str), Some("open" | "resolve")) {
        return hangar_server::session_open::cli::run(&args);
    }
    // O serviço sobe o backend por aqui: a versão real para onde o `current` aponta na largada.
    if args.first().map(String::as_str) == Some("launch") {
        return hangar_server::install_root::launch(&args[1..]);
    }
    // A migração única do motor `git` para o pacote (ou para o modo desenvolvedor), com volta.
    if args.first().map(String::as_str) == Some("migrate-package") {
        return hangar_server::update::migrate::cli(&args[1..]);
    }
    // Instalação nova direto pelo pacote, sem git (o bootstrap.sh chama).
    if args.first().map(String::as_str) == Some("install-package") {
        return hangar_server::update::install::cli(&args[1..]);
    }
    // Liga e desliga o modo desenvolvedor numa máquina já migrada.
    if args.first().map(String::as_str) == Some("rust-release") {
        return hangar_server::update::git_engine::releases::cli(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("checkout-update") {
        return hangar_server::update::git_engine::launch::cli(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("dev-mode") {
        return hangar_server::update::dev_mode::cli(&args[1..]);
    }
    // O executor destacado da atualização: processo e runtime próprios, fora do servidor que reinicia.
    if args.first().map(String::as_str) == Some("update") {
        return hangar_server::update::executor::cli(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("harness-setup") {
        return hangar_server::harness_setup::cli::run(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("engine") {
        return hangar_server::engines::cli::run(&args[1..]);
    }
    // `conta`: a reconciliação das contas Claude que o `hangar-conta` do shell chama, sem o servidor.
    if args.first().map(String::as_str) == Some("conta") {
        return hangar_server::accounts::claude_config::cli(&args[1..]);
    }
    // Regrava o snapshot versionado de tarifas a partir do models.dev.
    if args.first().map(String::as_str) == Some("pricing-snapshot") {
        return hangar_server::costs::pricing_refresh::cli(&args[1..]);
    }
    // O `hangar-send` inteiro; o atalho `scripts/hangar-send` passa a raiz da cópia dele.
    if args.first().map(String::as_str) == Some("send") {
        return hangar_server::send_cli::cli(&args[1..]);
    }
    // O pedido do `scripts/hangar-open`: monta, chama o `open` e imprime o nome para o atalho anexar.
    if args.first().map(String::as_str) == Some("open-cli") {
        return hangar_open_cli::cli(&args[1..]);
    }
    // Registro do MCP `hangar`, agentes das skills e bloco do CLAUDE.md, pelo instalador e pelos passos.
    match args.first().map(String::as_str) {
        Some("mcp-headers") => return hangar_registration::mcp_headers::cli(&args[1..]),
        Some("mcp-register") => return hangar_registration::mcp_register::cli(&args[1..]),
        Some("agents-install") => return hangar_registration::agents_install::cli(&args[1..]),
        Some("claude-md-block") => return hangar_registration::claude_md_block::cli(&args[1..]),
        _ => {}
    }
    // Hook de SessionStart: só disco, nunca sobe o servidor.
    if args.first().map(String::as_str) == Some("pair-context") {
        return hangar_server::groups::context_cmd::run(&args);
    }
    // O servidor sobe sem argumento (ou com o do relançamento); qualquer outro é subcomando que este
    // binário não tem, e subir como servidor esconderia isso atrás de "configuração inválida".
    if let Some(first) = args.first() && first != hangar_server::runtime::claude_relaunch::RELAUNCH_FLAG {
        eprintln!("hangar-server: subcomando desconhecido: {first}");
        return std::process::ExitCode::from(2);
    }
    // O glibc exige a configuração antes de o runtime criar threads.
    unsafe { hangar_server::tune_allocator(); }
    // Ainda sem threads: mexer no ambiente aqui é seguro, e as sessões não herdam o canal do systemd.
    let notifier = hangar_boot::notify::Notifier::from_lookup(|key| std::env::var(key).ok(), std::process::id());
    for key in ["NOTIFY_SOCKET", "WATCHDOG_USEC", "WATCHDOG_PID"] { unsafe { std::env::remove_var(key) } }
    if args.is_empty() && let Err(code) = standalone() {
        return code;
    }
    run(notifier);
    std::process::ExitCode::SUCCESS
}

/// O servidor monta o próprio ambiente: `.env` + processo, instância e segredo novos, porta do
/// convidado e log. O resto do servidor lê o ambiente como sempre.
fn standalone() -> Result<(), std::process::ExitCode> {
    // Antes do log desta vida: a falha de gravar no diário só aparece pelo stderr.
    let refuse = |code: &'static str, exit: u8, message: String| {
        eprintln!("hangar-server: {message}");
        hangar_server::diag::record_before_log(
            &[serde_json::json!({"evento": "hangar_server.partida", "nivel": "erro", "codigo": code})], "subida");
        std::process::ExitCode::from(exit)
    };
    let home = hangar_server::accounts::environment::AccountEnvironment::capture().home;
    let backend = std::env::current_dir().ok().filter(|dir| hangar_server::session_open::cli::is_backend(dir))
        .or_else(hangar_server::session_open::cli::backend_dir);
    let env_file = hangar_core::user_data::read_env_raw(&home, backend.as_deref())
        .map_err(|e| refuse("env_unreadable", 2, format!(".env ilegível: {e}")))?;
    // Só o número da linha: o valor pode ser o token.
    for issue in env_file.as_deref().map(hangar_core::user_data::env_file_issues).unwrap_or_default() {
        eprintln!("hangar-server: env_line_ignored: .env {issue}");
    }
    let mut environ: Vec<(String, String)> = Vec::new();
    for (key, value) in std::env::vars_os() {
        match (key.into_string(), value.into_string()) {
            (Ok(key), Ok(value)) => environ.push((key, value)),
            // Só as `CP_*` são configuração: as outras fora de UTF-8 não mudam nada aqui.
            (Ok(key), Err(_)) if key.to_ascii_uppercase().starts_with("CP_") =>
                eprintln!("hangar-server: env_not_utf8: {key} fora de UTF-8 ignorada; vale o .env ou o padrão"),
            _ => {}
        }
    }
    let settings = hangar_boot::settings::Settings::resolve(env_file.as_deref(), &environ)
        .map_err(|e| refuse("config_invalid", 2, format!("configuração inválida: {e}")))?;
    let claude = std::env::var_os("CLAUDE_CONFIG_DIR").map(std::path::PathBuf::from);
    let log_base = hangar_core::paths::log_base();
    let places = hangar_boot::settings::Places { home: &home, claude_config: claude.as_deref(), log_base: log_base.as_deref() };
    let keep = std::env::var_os(hangar_boot::settings::KEEP_INHERITED).is_some_and(|value| value == "1");
    let boot = hangar_boot::settings::standalone_env(&settings, &places, |key| keep && std::env::var_os(key).is_some())
        .map_err(|message| refuse("startup_refused", 1, message))?;
    if let Some(warning) = boot.warning { println!("{warning}"); }
    for note in boot.notes { eprintln!("hangar-server: {note}"); }
    for key in boot.remove { unsafe { std::env::remove_var(key) } }
    for (key, value) in boot.vars { unsafe { std::env::set_var(key, value) } }
    Ok(())
}

/// No Windows `0.0.0.0` não impede outro programa de abrir a mesma porta em `[::]` dual-stack (Node,
/// `python -m http.server`), e ele passa a responder no lugar do Hangar. Um segundo socket dual-stack
/// na mesma porta fecha essa brecha e atende igual.
#[cfg(windows)]
fn dual_stack(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV6, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_only_v6(false)?;
    socket.set_nonblocking(true)?;
    socket.bind(&std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())?;
    socket.listen(1024)?;
    tokio::net::TcpListener::from_std(socket.into())
}

#[cfg(all(test, windows))]
mod tests {
    fn dual_bind(port: u16) -> std::io::Result<()> {
        let socket = socket2::Socket::new(socket2::Domain::IPV6, socket2::Type::STREAM, None)?;
        socket.set_only_v6(false)?;
        socket.bind(&std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())
    }

    #[tokio::test]
    async fn ipv4_plus_dual_stack_hold_the_port_against_both_kinds_of_server() {
        let public = tokio::net::TcpListener::bind("0.0.0.0:0").await.unwrap();
        let port = public.local_addr().unwrap().port();
        assert!(dual_bind(port).is_ok(), "sem o segundo socket, um dual-stack pega a porta");
        let _guard = super::dual_stack(port).unwrap();
        assert!(dual_bind(port).is_err());
        assert!(std::net::TcpListener::bind(("0.0.0.0", port)).is_err());
        assert!(tokio::net::TcpStream::connect(("127.0.0.1", port)).await.is_ok());
    }
}

#[tokio::main]
async fn run(notifier: hangar_boot::notify::Notifier) {
    let args:Vec<_>=std::env::args_os().collect();
    match hangar_server::runtime::claude_relaunch::helper_args(&args) {
        // Só o `kind`: a mensagem pode trazer caminho ou ambiente do relançamento.
        Ok(Some(endpoint))=> match hangar_server::runtime::claude_relaunch::helper(endpoint).await {
            Ok(code)=>std::process::exit(code),
            Err(e)=>{ eprintln!("hangar-server: relaunch helper failed: {:?}", e.kind()); std::process::exit(126) },
        },
        Err(e)=>{ eprintln!("hangar-server: relaunch helper args invalid: {:?}", e.kind()); std::process::exit(126) },
        Ok(None)=>{},
    }
    let cfg = match hangar_server::config::Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("hangar-server: configuração inválida: {e}");
            std::process::exit(2);
        }
    };
    let guest = match hangar_server::config::GuestEnv::from_env() {
        Ok(guest) => guest,
        Err(e) => {
            eprintln!("hangar-server: configuração inválida: {e}");
            std::process::exit(2);
        }
    };
    // Antes do log desta vida: a queda da anterior sai das últimas linhas dele.
    let hangar_dir = hangar_server::accounts::environment::AccountEnvironment::capture().home.join(".hangar");
    hangar_server::diag::record_boot(&hangar_dir, cfg.log_path.as_deref());
    hangar_server::init_log(cfg.log_path.as_deref());
    hangar_server::install_panic_hook();
    if let Some(code) = notifier.rejected {
        tracing::warn!(code, "variável do watchdog do systemd recusada; o servidor segue sem watchdog");
    }
    // A atualização apenas agenda: a poda precisa do binário novo, depois do download.
    tokio::spawn(async {
        let home = hangar_server::accounts::environment::AccountEnvironment::capture().home;
        match tokio::task::spawn_blocking(move || hangar_server::orq_storage::prune_requested(&home)).await {
            Ok(Ok(true)) => tracing::info!(code = "orq_archive_pruned", "poda solicitada pela atualização concluída; resultado no recibo"),
            Ok(Ok(false)) => {},
            Ok(Err(error)) => tracing::warn!(code = "orq_archive_prune_failed", kind = ?error.kind(), "poda não concluída; pedido conservado para a próxima inicialização"),
            Err(_) => tracing::warn!(code = "orq_archive_prune_failed", "tarefa da poda falhou; pedido conservado"),
        }
    });
    // Backend morto sem saída limpa deixa a pasta do contador de login para trás; fora do caminho da subida.
    tokio::spawn(async {
        // O servidor de ensaio não varre a máquina, como em `startup.rs`.
        if std::env::var_os("CP_RUST_NO_ORPHAN_SWEEP").is_some_and(|value| value == "1") { return; }
        let root = hangar_server::accounts::environment::AccountEnvironment::capture().home.join(".hangar");
        match tokio::task::spawn_blocking(move || hangar_auth::sweep_orphans(&root, None, std::time::SystemTime::now())).await {
            Ok(Ok(outcome)) if outcome.failed > 0 || outcome.skipped > 0 => tracing::warn!(code = "auth_orphans_kept", removed = outcome.removed, failed = outcome.failed,
                skipped = outcome.skipped, first = ?outcome.first_failure, "pastas do contador de login que a varredura não tirou ou não conferiu"),
            Ok(Ok(outcome)) if outcome.removed > 0 => tracing::info!(code = "auth_orphans_removed", removed = outcome.removed, "pastas órfãs do contador de login apagadas"),
            Ok(Ok(_)) => {},
            Ok(Err(error)) => tracing::warn!(code = "auth_orphans_unreadable", kind = ?error.kind(), "pasta do Hangar ilegível; varredura do contador de login não rodou"),
            Err(_) => tracing::warn!(code = "auth_orphans_failed", "tarefa da varredura do contador de login falhou"),
        }
    });
    // O `hangar-server open` instalado em `~/.hangar/bin` acha a `backend/` por aqui (o servidor roda nela).
    if let Ok(backend) = std::env::current_dir() && hangar_server::session_open::cli::is_backend(&backend) {
        let home = hangar_server::accounts::environment::AccountEnvironment::capture().home;
        if let Err(error) = hangar_server::session_open::cli::publish_backend_dir(&home, &backend) {
            tracing::warn!(kind = ?error.kind(), "pasta do backend não anotada; o comando de terminal instalado não acha o .env");
        }
    }
    let listener = match tokio::net::TcpListener::bind(cfg.listen).await {
        Ok(l) => l,
        Err(e) => {
            tracing::error!(listen = %cfg.listen, "porta pública indisponível: {e}");
            eprintln!("hangar-server: porta {} indisponível: {e}", cfg.listen);
            refused_boot("port_unavailable");
            std::process::exit(1);
        }
    };
    tracing::info!(listen = %cfg.listen, version = env!("CARGO_PKG_VERSION"), "hangar-server de pé");
    let stopped = std::sync::Arc::new(std::sync::OnceLock::new());
    // A marca de pé e a parada sob a mesma trava: parada que chega antes do "pronto" impede a marca.
    let stop_recorded = std::sync::Arc::new(std::sync::Mutex::new(false));
    let requested = hangar_server::stop_requested();
    let stop = {
        let stopped = stopped.clone();
        #[cfg(windows)]
        let (recorded, dir) = (stop_recorded.clone(), hangar_dir.clone());
        async move {
            let cause = requested.await;
            let _ = stopped.set(cause);
            // No Windows a parada limpa vem antes do desligamento lento: o fechamento do console dá
            // ~5 s até o kill. No resto ela espera o servidor terminar (abaixo): um desligamento que
            // trava e leva o SIGKILL do `TimeoutStopSec` tem de contar como queda.
            #[cfg(windows)]
            if let Err(error) = tokio::task::spawn_blocking(move || record_stop_once(&recorded, &dir, cause, false)).await {
                tracing::warn!(code = "server_stop_record_failed", %error, "tarefa que grava a parada limpa falhou");
            }
        }
    };
    #[allow(unused_mut)]
    let home = hangar_server::accounts::environment::AccountEnvironment::capture().home;
    let server_id = match hangar_server::machine_peers::startup_identity(&home, hangar_server::session_open::cli::backend_dir().as_deref()) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("hangar-server: identidade indisponível: {error}");
            refused_boot("identity_unavailable");
            std::process::exit(1);
        }
    };
    #[allow(unused_mut)]
    let mut state = hangar_server::routes::AppState::with_server_id(cfg, server_id);
    #[cfg(windows)]
    if state.cfg.listen.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED) {
        // Falhar aqui não derruba o servidor: fica como antes, só sem a guarda do IPv6.
        match dual_stack(state.cfg.listen.port()) {
            Ok(guard) => state.public_v6 = Some(guard),
            Err(e) => tracing::warn!(error = %e, "porta pública sem a guarda dual-stack: outro programa pode ocupá-la em IPv6"),
        }
    }
    state.startup = hangar_server::startup::Readiness::pending();
    state.front = hangar_server::routes::front_address(&state.accounts.env.home, state.cfg.listen);
    state.connect = hangar_server::config::connect_listen(state.cfg.listen);
    state.preview = hangar_server::config::preview_listen(state.cfg.listen);
    let startup = state.startup.clone();
    let service = hangar_server::update::git_engine::Service::from_state(&state);
    let startup_environment = state.accounts.env.clone();
    let startup_prewarm = state.prewarm.clone();
    let startup_accounts = state.accounts.clone();
    let (alive_state, listen) = (state.state.clone(), state.cfg.listen);
    let (ready_recorded, ready_dir, ready_notifier) = (stop_recorded.clone(), hangar_dir.clone(), notifier.clone());
    let ready_stopped = stopped.clone();
    // Pronto quando a porta pública atende, antes da preparação demorada (hooks, sessões): uma máquina
    // cheia não estoura o `TimeoutStartSec` e vira um laço de religadas sem queda contada.
    let _ready = tokio::spawn(async move {
        hangar_server::watchdog::wait_serving(listen).await;
        // Daqui em diante sair sem a parada gravada é queda. Parada pedida antes de pronto não marca
        // de pé: fora do Windows a parada só é gravada depois que o servidor termina, e a marca ficaria até lá.
        let marked = tokio::task::spawn_blocking(move || {
            let recorded = ready_recorded.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if *recorded || ready_stopped.get().is_some() { return false; }
            hangar_server::diag::record_running(&ready_dir);
            true
        }).await;
        let marked = match marked {
            Ok(marked) => marked,
            Err(error) => {
                tracing::error!(code = "server_ready_failed", %error, "tarefa da marca de pé falhou: sem READY=1 nem watchdog, o systemd encerra pelo prazo de subida");
                false
            }
        };
        if !marked { return; }
        if let Err(error) = ready_notifier.send("READY=1") {
            tracing::warn!(code = "systemd_notify_failed", kind = ?error.kind(), "aviso de pronto ao systemd não saiu");
        }
        if let Some(every) = ready_notifier.watchdog {
            tokio::spawn(hangar_server::watchdog::notify_alive(ready_notifier, every,
                hangar_server::watchdog::server_probes(listen, alive_state), hangar_server::watchdog::Policy::SERVER));
        }
    });
    let _preparation = tokio::spawn(async move {
        match service {
            Ok(service) => match tokio::task::spawn_blocking(move || service.startup_outcome()).await {
                Ok(outcome) => {
                    // A saúde vem antes: o servidor segue subindo, mas a falha fica no log com código.
                    if let hangar_server::update::git_engine::periodic::StartupOutcome::Failed(error) = &outcome {
                        tracing::warn!(code = "startup_steps_failed", %error, "passo de atualização da subida falhou; o servidor segue subindo");
                    }
                    if let Some(message) = outcome.message() { eprintln!("{message}"); }
                }
                Err(error) => tracing::warn!(code="startup_steps_failed", %error, "preparação inicial interrompida"),
            },
            Err(error) => tracing::warn!(code="startup_config_failed", %error, "preparação inicial sem configuração"),
        }
        // Antes de liberar as rotas: o `settings.json` não pode ter dois escritores no começo da vida.
        // O servidor de ensaio não grava hooks da máquina, como na varredura dos canos.
        if std::env::var_os("CP_RUST_NO_ORPHAN_SWEEP").is_none_or(|value| value != "1")
            && let Err(error) = tokio::task::spawn_blocking(move || hangar_server::harness_setup::sync_startup(&startup_accounts)).await {
            tracing::warn!(code = "harness_startup_failed", %error, "tarefa dos hooks e skills da subida falhou");
        }
        startup_prewarm.clean_receipts().await;
        hangar_server::startup::prepare_sessions(startup_environment).await;
        startup.finish();
    });
    let prewarm = state.prewarm.clone();
    let connect = state.connect_supervisor.clone();
    // Fora do caminho da subida: baixar `frpc` e Caddy não segura a porta.
    let resuming = connect.clone();
    tokio::spawn(async move { resuming.resume().await });
    // A porta do convidado não entra no bind fatal acima: o filho anterior pode ainda segurá-la.
    let served = hangar_server::serve_until_with_guest(listener, state, guest.listen, stop).await;
    let _ = notifier.send("STOPPING=1");
    connect.stop().await;
    prewarm.discard_all("saida_do_servidor").await;
    match served {
        Ok(()) => {
            let cause = stopped.get().copied().unwrap_or("desconhecida");
            tracing::info!(cause, "parada pedida: hangar-server sai");
            // Fora do Windows a parada limpa só depois do desligamento inteiro (ver `stop`).
            #[cfg(not(windows))]
            record_stop_once(&stop_recorded, &hangar_dir, cause, false);
            std::process::exit(0);
        }
        Err(e) => {
            tracing::error!("hangar-server parou: {e}");
            record_stop_once(&stop_recorded, &hangar_dir, "serve_failed", true);
            std::process::exit(1);
        }
    }
}

/// A parada desta vida, uma vez só (sinal e falha do servidor podem chegar os dois).
fn record_stop_once(recorded: &std::sync::Mutex<bool>, dir: &std::path::Path, cause: &str, failed: bool) {
    let mut recorded = recorded.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if *recorded { return; }
    *recorded = true;
    hangar_server::diag::record_stop(dir, cause, failed);
}

/// Saída antes de a vida ficar pronta (porta tomada, identidade): não é queda, e o diário diz por quê.
fn refused_boot(code: &'static str) {
    hangar_server::diag::record_blocking(hangar_server::diag::diary_dir().as_deref(),
        &[serde_json::json!({"evento": "hangar_server.partida", "nivel": "erro", "codigo": code})], "subida");
}
