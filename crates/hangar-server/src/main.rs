//! hangar-server: sobe como filho do Python (app/main.py), na porta pública.

fn main() -> std::process::ExitCode {
    // `open`/`resolve`: o lançador como comando de terminal, com runtime próprio (fora do `run`).
    let args: Vec<String> = std::env::args_os().skip(1).map(|a| a.to_string_lossy().into_owned()).collect();
    if args.first().is_some_and(|arg| arg == "orq-storage") {
        return hangar_server::orq_storage::cli(&args[1..]);
    }
    if matches!(args.first().map(String::as_str), Some("open" | "resolve")) {
        return hangar_server::session_open::cli::run(&args);
    }
    // `conta`: a reconciliação das contas Claude que o `hangar-conta` do shell chama, sem o servidor.
    if args.first().map(String::as_str) == Some("conta") {
        return hangar_server::accounts::claude_config::cli(&args[1..]);
    }
    // Hook de SessionStart: só disco, nunca sobe o servidor.
    if args.first().map(String::as_str) == Some("pair-context") {
        return hangar_server::groups::context_cmd::run(&args);
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
    // O `hangar-server open` instalado em `~/.hangar/bin` acha o `.env` por aqui (o servidor roda na `backend/`).
    if let Ok(backend) = std::env::current_dir() && backend.join(".env").is_file() {
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
    let mut state = hangar_server::routes::AppState::with_server_id(cfg, std::env::var("HANGAR_SERVER_ID").unwrap_or_default());
    #[cfg(windows)]
    if state.cfg.listen.ip() == std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED) {
        // Falhar aqui não derruba o servidor: fica como antes, só sem a guarda do IPv6.
        match dual_stack(state.cfg.listen.port()) {
            Ok(guard) => state.public_v6 = Some(guard),
            Err(e) => tracing::warn!(error = %e, "porta pública sem a guarda dual-stack: outro programa pode ocupá-la em IPv6"),
        }
    }
    state.costs.schedule_warmup(std::time::Duration::from_secs(30));
    hangar_server::pages::routes::spawn_sweep(state.list.clone(), state.pages.clone());
    // Só aqui, nunca no `serve_until`: os testes sobem servidores contra o tmux de verdade.
    #[cfg(unix)]
    tokio::spawn({
        let term = state.term.clone();
        async move { term.restore_after_crash().await }
    });
    // Reservas pré-aquecidas que um servidor que caiu deixou (todo sistema: a varredura abaixo é só Linux
    // e não apaga transcript nem marcadores). Só aqui, pelo mesmo motivo do tmux acima.
    let prewarm = state.prewarm.clone();
    let cleaned = prewarm.clean_receipts().await;
    if cleaned > 0 { tracing::info!(cleaned, "reservas pré-aquecidas que sobraram foram limpas"); }
    let _prewarm_sweeper = prewarm.spawn_sweeper();
    // Órfãos do cano têm dono só: com o runtime ligado, o Rust varre uma vez, antes de anunciar o
    // gateway (o Python só abre sessões depois do anúncio). Só aqui, pelo mesmo motivo do tmux acima.
    // `CP_RUST_NO_ORPHAN_SWEEP=1`: o teste que sobe este binário de verdade nunca varre processos da máquina.
    if matches!(hangar_server::config::Config::runtime_instance(), Ok(Some(_)))
        && std::env::var_os("CP_RUST_NO_ORPHAN_SWEEP").is_none_or(|value| value != "1") {
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        if home.is_none() { tracing::warn!("HOME ausente: varredura de canos órfãos pulada"); }
        if let Some(home) = home {
            let swept = tokio::task::spawn_blocking(move || {
                let base = home.join(".hangar");
                hangar_server::runtime::process::sweep_orphans(&base.join("claude-headless"), &base.join("codex-sessions"),
                    &home.to_string_lossy())
            }).await.unwrap_or_else(|e| { tracing::warn!(error = %e, "varredura de canos órfãos falhou"); None });
            if let Some(count) = swept.filter(|count| *count > 0) { tracing::info!(count, "canos de sessão já encerrada finalizados"); }
        }
    }
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
