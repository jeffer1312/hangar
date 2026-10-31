//! hangar-server: sobe como filho do Python (app/main.py), na porta pública.

fn main() -> std::process::ExitCode {
    let hook_args: Vec<_> = std::env::args_os().skip(1).collect();
    if let Some(mode) = hook_args.first().and_then(|arg| arg.to_str())
        && matches!(mode, "codex-hook-allow" | "codex-hook-json")
    {
        std::process::exit(hangar_server::codex_setup::hook_command::run(mode, &hook_args[1..]));
    }
    // `open`/`resolve`: o lançador como comando de terminal, com runtime próprio (fora do `run`).
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    if matches!(args.first().map(String::as_str),Some("panel-data"|"panel-action")){
        return hangar_server::machine_peers::panel::cli(&args[0],&args[1..]);
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
    run();
    std::process::ExitCode::SUCCESS
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
async fn run() {
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
    hangar_server::init_log(cfg.log_path.as_deref());
    hangar_server::install_panic_hook();
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
            std::process::exit(1);
        }
    };
    tracing::info!(listen = %cfg.listen, upstream = %cfg.upstream, version = env!("CARGO_PKG_VERSION"), "hangar-server de pé");
    // O Python segura o cano do stdin; fechou = pai morreu. Vale igual em Linux, Windows e macOS.
    let stop = hangar_server::parent_gone(tokio::io::stdin());
    #[allow(unused_mut)]
    let home = hangar_server::accounts::environment::AccountEnvironment::capture().home;
    let server_id = match hangar_server::machine_peers::startup_identity(&home, hangar_server::session_open::cli::backend_dir().as_deref()) {
        Ok(value) => value,
        Err(error) => { eprintln!("hangar-server: identidade indisponível: {error}"); std::process::exit(1); }
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
    let startup = state.startup.clone();
    let service = hangar_server::update::git_engine::Service::from_state(&state);
    let startup_environment = state.accounts.env.clone();
    let startup_prewarm = state.prewarm.clone();
    let _preparation = tokio::spawn(async move {
        match service {
            Ok(service) => match tokio::task::spawn_blocking(move || service.startup_outcome()).await {
                Ok(outcome) => if let Some(message) = outcome.message() { eprintln!("{message}"); },
                Err(error) => tracing::warn!(code="startup_steps_failed", %error, "preparação inicial interrompida"),
            },
            Err(error) => tracing::warn!(code="startup_config_failed", %error, "preparação inicial sem configuração"),
        }
        startup_prewarm.clean_receipts().await;
        hangar_server::startup::prepare_sessions(startup_environment).await;
        startup.finish();
    });
    let prewarm = state.prewarm.clone();
    // A porta do convidado não entra no bind fatal acima: o filho anterior pode ainda segurá-la.
    let served = hangar_server::serve_until_with_guest(listener, state, guest.listen, stop).await;
    prewarm.discard_all("saida_do_servidor").await;
    match served {
        Ok(()) => {
            tracing::info!("stdin fechou: o backend saiu, hangar-server sai junto");
            std::process::exit(0);
        }
        Err(e) => {
            tracing::error!("hangar-server parou: {e}");
            std::process::exit(1);
        }
    }
}
