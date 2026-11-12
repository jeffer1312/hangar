//! Abre o emulador configurado e confirma que ele não morreu logo após o exec.
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::process::Command;

use crate::Error;

const EMULATORS: [&str; 6] = [
    "wezterm",
    "kitty",
    "alacritty",
    "konsole",
    "gnome-terminal",
    "xterm",
];

/// Abre uma janela independente anexada ao alvo exato do multiplexador.
///
/// # Errors
///
/// Retorna os códigos legados quando o emulador falta, não pode ser executado ou sai com erro.
pub async fn open(
    name: &str,
    env: &BTreeMap<String, String>,
    scope: Vec<String>,
) -> Result<Value, Error> {
    let configured = env.get("CP_TERMINAL").filter(|value| !value.is_empty());
    let emulator = if let Some(name) = configured {
        if !EMULATORS.contains(&name.as_str()) || executable(name, env).is_none() {
            return Err(Error {
                status: 503,
                code: "erro_terminal_invalido",
                msg: format!("CP_TERMINAL={name:?} nao encontrado no PATH ou nao suportado"),
                params: json!({"nome": name}),
            });
        }
        name.as_str()
    } else {
        EMULATORS
            .into_iter()
            .find(|name| executable(name, env).is_some())
            .ok_or_else(|| Error {
                status: 503,
                code: "erro_terminal_ausente",
                msg: "nenhum emulador de terminal encontrado no PATH".into(),
                params: json!({}),
            })?
    };
    let mut argv = scope;
    argv.push(emulator.into());
    match emulator {
        "wezterm" => argv.extend(["start".into(), "--".into()]),
        "kitty" => {}
        "gnome-terminal" => argv.push("--".into()),
        _ => argv.push("-e".into()),
    }
    argv.extend([
        "tmux".into(),
        "attach".into(),
        "-t".into(),
        format!("={name}:"),
    ]);
    // Arquivo comum não enche um pipe sem leitor enquanto a janela permanece aberta.
    let mut stderr = tempfile::tempfile().map_err(internal_error)?;
    let mut command = Command::new(&argv[0]);
    command
        .args(&argv[1..])
        .env_clear()
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr.try_clone().map_err(internal_error)?);
    if let Some(display) = wayland_display(env) {
        command.env("WAYLAND_DISPLAY", display);
    }
    hangar_core::terminal_process::detach_launch(&mut command);
    let mut child = command
        .spawn()
        .map_err(|error| spawn_error(error, &argv[0]))?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    let status = child.try_wait().map_err(internal_error)?;
    if let Some(status) = status {
        if !status.success() {
            stderr.seek(SeekFrom::Start(0)).map_err(internal_error)?;
            let mut bytes = Vec::new();
            stderr.read_to_end(&mut bytes).map_err(internal_error)?;
            let text = String::from_utf8_lossy(&bytes).trim().to_owned();
            let text = if text.is_empty() {
                format!("codigo {}", exit_code(status))
            } else {
                text
            };
            return Err(Error {
                status: 503,
                code: "erro_terminal_saiu_cedo",
                msg: format!("emulador de terminal saiu logo apos abrir: {text}"),
                params: json!({"saida": text}),
            });
        }
    } else {
        // A janela pode sobreviver ao servidor; apenas recolhemos seu processo quando terminar.
        tokio::spawn(async move {
            let _ = child.wait().await;
        });
    }
    Ok(json!({"ok": true}))
}

fn spawn_error(error: std::io::Error, program: &str) -> Error {
    let text = error.to_string();
    #[cfg(unix)]
    let text = if let Some(number) = error.raw_os_error() {
        let suffix = format!(" (os error {number})");
        format!(
            "[Errno {number}] {}: '{program}'",
            text.strip_suffix(&suffix).unwrap_or(&text)
        )
    } else {
        text
    };
    #[cfg(not(unix))]
    let _ = program;
    Error {
        status: 503,
        code: "erro_terminal_abertura_falhou",
        msg: format!("falha ao abrir o emulador de terminal: {text}"),
        params: json!({"erro": text}),
    }
}

fn internal_error(_: std::io::Error) -> Error {
    Error {
        status: 500,
        code: "",
        msg: "Internal Server Error".into(),
        params: Value::Null,
    }
}

fn executable(name: &str, env: &BTreeMap<String, String>) -> Option<PathBuf> {
    #[cfg(unix)]
    {
        std::env::split_paths(
            env.get("PATH")
                .map(String::as_str)
                .unwrap_or("/bin:/usr/bin"),
        )
        .map(|dir| dir.join(name))
        .find(|path| {
            path.metadata().is_ok_and(|meta| {
                meta.is_file() && hangar_core::terminal_process::can_execute(path)
            })
        })
    }
    #[cfg(not(unix))]
    {
        hangar_core::process::resolve_program(name, env)
    }
}

fn wayland_display(env: &BTreeMap<String, String>) -> Option<String> {
    if let Some(display) = env.get("WAYLAND_DISPLAY").filter(|value| !value.is_empty()) {
        return Some(display.clone());
    }
    let directory = env.get("XDG_RUNTIME_DIR")?;
    std::fs::read_dir(Path::new(directory))
        .ok()?
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.starts_with("wayland-") && !name.ends_with(".lock"))
        .min()
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        -status.signal().unwrap_or(1)
    }
    #[cfg(not(unix))]
    {
        1
    }
}
