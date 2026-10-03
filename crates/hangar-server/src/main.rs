//! hangar-server: sobe como filho do Python (app/main.py), na porta pública.

#[tokio::main]
async fn main() {
    let cfg = match hangar_server::config::Config::from_env() {
        Ok(cfg) => cfg,
        Err(e) => {
            eprintln!("hangar-server: configuração inválida: {e}");
            std::process::exit(2);
        }
    };
    hangar_server::init_log(cfg.log_path.as_deref());
    hangar_server::install_panic_hook();
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
    match hangar_server::serve_until(listener, cfg, stop).await {
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
