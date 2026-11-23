//! `hangar-server open-cli <claude|pi|omp|kimi> [args do CLI]`: o pedido que o `scripts/hangar-open`
//! manda ao lançador. Monta o `POST /api/sessions` dos argumentos do CLI, chama `hangar-server open` e,
//! aberta a sessão, imprime só o nome dela; o atalho de shell então anexa o terminal (`tmux attach`).
//!
//! Saída 100 = os argumentos não cabem no pedido, ou o servidor não abriu nada (antigo ou sem token):
//! o wrapper roda o CLI cru. Qualquer outra saída diferente de 0 é erro já mostrado; o wrapper não
//! abre uma segunda vez, por isso um 100 vindo do servidor vira 1.
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};

use serde_json::{Map, Value};

/// Os argumentos não cabem no pedido: o CLI roda cru.
pub const RAW: u8 = 100;

/// Flag do CLI -> chave do pedido. O que não estiver aqui vai cru.
fn flags(provider: &str) -> Option<&'static [(&'static str, &'static str)]> {
    Some(match provider {
        "claude" => &[("--model", "model"), ("--effort", "effort"), ("--permission-mode", "permission_mode")],
        "pi" => &[("--model", "model"), ("--thinking", "effort")],
        "omp" => &[("--model", "model"), ("--thinking", "effort"), ("--profile", "omp_profile")],
        "kimi" => &[("--model", "model")],
        "antigravity" => &[("--model", "model"), ("--mode", "permission_mode")],
        _ => return None,
    })
}

/// Variável do ambiente -> chave do pedido, só com valor não vazio.
fn env_keys(provider: &str) -> &'static [(&'static str, &'static str)] {
    match provider {
        "claude" => &[("CLAUDE_CONFIG_DIR", "config_dir"), ("CP_ENGINE", "engine")],
        "omp" => &[("OMP_PROFILE", "omp_profile")],
        _ => &[],
    }
}

/// Subcomando do claude (`claude --help`, seção Commands): só vale como PRIMEIRO argumento e roda
/// cru; virar sessão faria dele o prompt inicial. pi/omp/kimi já saem crus no próprio wrapper.
const CLAUDE_SUBCOMMANDS: &[&str] = &[
    "agents", "attach", "auth", "auto-mode", "doctor", "gateway", "import", "install", "logs", "mcp",
    "plugin", "plugins", "purge", "respawn", "rm", "setup-token", "stop", "kill", "ultrareview", "update", "upgrade",
];

/// O pedido, ou o código de saída quando ele não sai (`RAW` com a frase já escrita).
pub fn request(provider: &str, args: &[String], vars: &BTreeMap<String, String>, cwd: &Path, err: &mut dyn Write) -> Result<Map<String, Value>, u8> {
    let Some(table) = flags(provider) else {
        let _ = writeln!(err, "hangar: provider desconhecido '{provider}': use claude, pi, omp, kimi ou antigravity");
        return Err(1);
    };
    if provider == "claude" && args.first().is_some_and(|first| CLAUDE_SUBCOMMANDS.contains(&first.as_str())) {
        return Err(RAW);
    }
    let uncovered = |arg: &str, err: &mut dyn Write| {
        let _ = writeln!(err, "hangar: o argumento '{arg}' não cabe no pedido do Hangar; abrindo o {provider} sem o Hangar");
        RAW
    };
    // O `PWD` é o caminho lógico (mantém o link); sem ele, o da pasta como o sistema a vê.
    let cwd = vars.get("PWD").filter(|pwd| !pwd.is_empty()).cloned()
        .unwrap_or_else(|| cwd.to_string_lossy().into_owned());
    let mut body = Map::new();
    body.insert("provider".into(), Value::from(provider));
    body.insert("cwd".into(), cwd.into());
    body.insert("headless".into(), false.into());
    for (var, key) in env_keys(provider) {
        if let Some(value) = vars.get(*var).filter(|value| !value.is_empty()) {
            body.insert((*key).into(), value.clone().into());
        }
    }
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        let (flag, value) = match arg.split_once('=') {
            Some((flag, value)) => (flag, Some(value)),
            None => (arg.as_str(), None),
        };
        if let Some((_, key)) = table.iter().find(|(name, _)| *name == flag) {
            let value = match value {
                Some(value) => value.to_owned(),
                None => match rest.next() {
                    Some(next) => next.clone(),
                    None => return Err(uncovered(arg.as_str(), err)),
                },
            };
            body.insert((*key).into(), value.into());
        } else if matches!(provider, "claude" | "antigravity") && arg == "--dangerously-skip-permissions" {
            body.insert("permission_mode".into(), "bypassPermissions".into());
        } else if arg.starts_with('-') || body.contains_key("initial_prompt") {
            return Err(uncovered(arg.as_str(), err));
        } else {
            body.insert("initial_prompt".into(), arg.clone().into());
        }
    }
    Ok(body)
}

/// `args` = `[provider, args do CLI...]`. Aberta a sessão, escreve o nome dela em `out` e devolve 0.
pub fn run(args: &[String], vars: &BTreeMap<String, String>, cwd: &Path, server: &Path, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let Some((provider, args)) = args.split_first() else {
        let _ = writeln!(err, "hangar: uso: hangar-open <claude|pi|omp|kimi|antigravity> [args do CLI]");
        return 1;
    };
    let body = match request(provider, args, vars, cwd, err) {
        Ok(body) => body,
        Err(code) => return code,
    };
    let json = hangar_core::pycompat::pyjson::dumps(&Value::Object(body), false);
    // O stderr do `open` passa ao vivo (a espera pela conta aparece enquanto acontece); só o stdout,
    // onde vem o nome, é lido.
    let child = Command::new(server).arg("open").arg(&json)
        .stdin(Stdio::inherit()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn();
    let opened = match child.and_then(|child| child.wait_with_output()) {
        Ok(opened) => opened,
        Err(error) => {
            let _ = writeln!(err, "hangar: não consegui rodar o hangar-server ({error}); abrindo o {provider} sem o Hangar");
            return RAW;
        }
    };
    let stdout = String::from_utf8_lossy(&opened.stdout);
    let Some(code) = opened.status.code() else {
        let _ = writeln!(err, "hangar: o hangar-server parou{} sem abrir a sessão", signal_text(&opened.status));
        return 1;
    };
    // 2 = hangar-server antigo, sem `open`, ou entrada ilegível; 4 = sem token no .env. Nada foi enviado.
    if code == 2 || code == 4 {
        let _ = writeln!(err, "hangar: o hangar-server não abriu a sessão (saída {code}); abrindo o {provider} sem o Hangar");
        return RAW;
    }
    if code != 0 {
        // Saída 3 (resolvido mas não aberto aqui) traz o spec no stdout.
        if !stdout.trim().is_empty() {
            let _ = writeln!(err, "{}", stdout.trim());
        }
        return match u8::try_from(code) { Ok(code) if code != RAW => code, _ => 1 };
    }
    let name = hangar_core::pycompat::pyjson::loads_lossless(stdout.trim())
        .and_then(|value| value.as_object().and_then(|object| object.get("name")).map(hangar_core::pycompat::py::py_str));
    let Some(name) = name else {
        let _ = writeln!(err, "hangar: resposta do hangar-server sem o nome da sessão: {}", stdout.trim());
        return 1;
    };
    let _ = writeln!(out, "{name}");
    0
}

#[cfg(unix)]
fn signal_text(status: &ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    status.signal().map(|signal| format!(" pelo sinal {signal}")).unwrap_or_default()
}

#[cfg(not(unix))]
fn signal_text(_status: &ExitStatus) -> String {
    String::new()
}

/// O subcomando, com este binário como o `hangar-server open`.
pub fn cli(args: &[String]) -> std::process::ExitCode {
    let vars: BTreeMap<String, String> = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect();
    let cwd = std::env::current_dir().unwrap_or_default();
    // O mesmo comando que o atalho chamou (ele passa o caminho absoluto que achou), como o `open` do
    // Python ia ao binário escolhido; chamado de outro jeito, este executável.
    let invoked = std::env::args_os().next().map(std::path::PathBuf::from).filter(|path| path.is_absolute() && path.is_file());
    let server = match invoked.map_or_else(std::env::current_exe, Ok) {
        Ok(server) => server,
        Err(error) => {
            eprintln!("hangar: não consegui achar o próprio hangar-server ({error}); abrindo sem o Hangar");
            return std::process::ExitCode::from(RAW);
        }
    };
    let code = run(args, &vars, &cwd, &server, &mut std::io::stdout(), &mut std::io::stderr());
    std::process::ExitCode::from(code)
}
