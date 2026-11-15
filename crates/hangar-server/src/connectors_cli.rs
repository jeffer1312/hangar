//! `hangar-server connectors`: liga o CLI do `hangar-connectors` ao servidor desta máquina (endereço e
//! senha do dono no `.env`) ou, com `--registry <arquivo>`, a um host isolado, sem subir backend.
use crate::machine_peers::dotenv::{SettingsEnvironment, SettingsSnapshot};
use crate::machine_peers::pairing::Settings;
use hangar_connectors::cli::{Target, USAGE};
use hangar_connectors::registry::Registry;
use hangar_connectors::{Host, HostOptions};

pub fn cli(args: &[String]) -> std::process::ExitCode {
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => { eprintln!("hangar-server connectors: runtime: {:?}", error.kind()); return 1.into(); }
    };
    let (target, rest) = match args {
        [flag, path, rest @ ..] if flag == "--registry" => {
            (Target::Local(Host::new(Registry::new(std::path::PathBuf::from(path)), HostOptions::default())), rest)
        }
        [flag, ..] if flag == "--registry" => { eprintln!("{USAGE}"); return 2.into(); }
        _ => match server() {
            Ok(target) => (target, args),
            Err(error) => { eprintln!("hangar-server connectors: {error}"); return 1.into(); }
        },
    };
    let code = runtime.block_on(async {
        let code = hangar_connectors::cli::run(&target, rest).await;
        if let Target::Local(host) = &target { host.shutdown().await; }
        code
    });
    code.into()
}

/// O servidor desta máquina pelo loopback, salvo quando ele escuta só num IP da rede.
fn server() -> Result<Target, String> {
    let home = std::env::home_dir().unwrap_or_default();
    let backend = crate::session_open::cli::backend_dir();
    let snapshot = SettingsSnapshot::capture(&home, backend.as_deref(), SettingsEnvironment::capture())
        .map_err(|error| format!("configuração do Hangar ilegível: {error}"))?;
    let settings = Settings::from_snapshot(&snapshot)?;
    let host = match settings.bind.as_str() {
        "" | "127.0.0.1" | "localhost" | "::1" | "0.0.0.0" | "::" | "auto" => "127.0.0.1".to_owned(),
        bind if bind.contains(':') => format!("[{bind}]"),
        bind => bind.to_owned(),
    };
    Ok(Target::Server { base: format!("http://{host}:{}", settings.port), token: settings.token })
}
