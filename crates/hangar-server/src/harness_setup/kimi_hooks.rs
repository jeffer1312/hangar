//! Reconcilia os comandos do Hangar sem reescrever o TOML pessoal.
use crate::accounts::environment::AccountEnvironment;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

pub const ENTRIES: &[(&str, Option<&str>, &str)] = &[
    ("SessionStart", None, "kimi-state-hook"),
    ("UserPromptSubmit", None, "kimi-state-hook"),
    ("TurnStarted", None, "kimi-state-hook"),
    ("Stop", None, "kimi-state-hook"),
    ("Interrupt", None, "kimi-state-hook"),
    ("PermissionRequest", None, "kimi-state-hook"),
    ("PreToolUse", Some("AskUserQuestion"), "kimi-state-hook"),
    ("PostToolUse", Some("AskUserQuestion"), "kimi-state-hook"),
    ("PreToolUse", Some("Bash"), "guard-tmux"),
];

#[derive(Deserialize)]
struct Config {
    #[serde(default)]
    hooks: Vec<Hook>,
}
#[derive(Deserialize)]
struct Hook {
    #[serde(default)]
    command: Option<toml::Spanned<toml::Value>>,
}

pub fn installed(command: &str, mode: &str) -> bool {
    crate::provider_hooks::command(mode).is_ok_and(|expected| {
        super::hooks::tokens(command, cfg!(windows))
            == super::hooks::tokens(&expected, cfg!(windows))
    })
}
fn native_command(command: &str, mode: &str) -> bool {
    let tokens = super::hooks::tokens(command, cfg!(windows));
    tokens.len() == 2
        && tokens[1] == mode
        && tokens[0].rsplit(['/', '\\']).next().is_some_and(|name| {
            matches!(
                name,
                "hangar-server" | "hangar-server.exe" | "hangar-server-command"
            )
        })
}

pub fn sync(env: &AccountEnvironment, _repo: &Path) -> Result<Vec<String>, String> {
    let home = super::files::directory(env, "kimi");
    if !home.is_dir() {
        return Ok(vec![]);
    }
    let path = home.join("config.toml");
    let raw = match fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let mut data: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        toml::from_str(&raw).map_err(|_| "config.toml inválido; hooks preservados")?
    };
    // Arrays inline não admitem acrescentar [[hooks]] sem redefinir a mesma chave.
    if data.get("hooks").is_some() && !raw.lines().any(|l| l.trim().starts_with("[[hooks]]")) {
        return Err("hooks inline; configuração preservada".into());
    }
    let parsed: Config =
        toml::from_str(&raw).map_err(|_| "config.toml inválido; hooks preservados")?;
    let mut replacements = Vec::new();
    for hook in parsed.hooks {
        let Some(command) = hook.command else {
            continue;
        };
        let Some(text) = command.get_ref().as_str() else {
            continue;
        };
        for (legacy, mode) in [
            ("kimi_state_hook.py", "kimi-state-hook"),
            ("guard_tmux.py", "guard-tmux"),
        ] {
            if super::hooks::refers_platform(&json!(text), legacy, true, cfg!(windows))
                || native_command(text, mode)
            {
                replacements.push((
                    command.span(),
                    serde_json::to_string(&crate::provider_hooks::command(mode)?)
                        .map_err(|e| e.to_string())?,
                ));
                break;
            }
        }
    }
    let mut out = raw.clone();
    for (span, replacement) in replacements.into_iter().rev() {
        out.replace_range(span, &replacement);
    }
    if out != raw {
        data = toml::from_str(&out)
            .map_err(|_| "migração de hooks inválida; configuração preservada")?;
    }
    for &(event, matcher, mode) in ENTRIES {
        let exists = data["hooks"].as_array().into_iter().flatten().any(|h| {
            h["event"] == event
                && h["matcher"].as_str() == matcher
                && h["command"].as_str().is_some_and(|c| installed(c, mode))
        });
        if exists {
            continue;
        }
        if !out.is_empty() && !out.ends_with('\n') {
            out.push('\n');
        }
        out += &format!("\n# hangar: integração da sessão.\n[[hooks]]\nevent = \"{event}\"\n");
        if let Some(matcher) = matcher {
            out += &format!("matcher = \"{matcher}\"\n");
        }
        out += &format!(
            "command = {}\ntimeout = 5\n",
            serde_json::to_string(&crate::provider_hooks::command(mode)?)
                .map_err(|e| e.to_string())?
        );
    }
    if out == raw {
        return Ok(vec![]);
    }
    if !raw.trim().is_empty() {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(home.join("config.toml.bak-hangar")) {
            Ok(mut file) => file.write_all(raw.as_bytes()).map_err(|e| e.to_string())?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    super::files::write_preserving(&path, &out)?;
    Ok(vec![path.to_string_lossy().into_owned()])
}
