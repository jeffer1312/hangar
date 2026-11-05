//! Acrescenta apenas entradas ausentes, conservando o TOML e o backup do usuário.
use crate::accounts::environment::AccountEnvironment;
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

pub fn sync(env: &AccountEnvironment, repo: &Path) -> Result<Vec<String>, String> {
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
    let data: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        toml::from_str(&raw).map_err(|_| "config.toml inválido; hooks preservados")?
    };
    let python = crate::install_root::python_exe(repo, cfg!(windows));
    let mut out = raw.clone();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    let mut changed = false;
    for (script, entries, header) in [
        (
            "kimi_state_hook.py",
            vec![
                ("SessionStart", None),
                ("UserPromptSubmit", None),
                ("TurnStarted", None),
                ("Stop", None),
                ("Interrupt", None),
                ("PermissionRequest", None),
                ("PreToolUse", Some("AskUserQuestion")),
                ("PostToolUse", Some("AskUserQuestion")),
            ],
            "marcadores de estado e vínculo da sessão",
        ),
        (
            "guard_tmux.py",
            vec![("PreToolUse", Some("Bash"))],
            "proteção do servidor tmux compartilhado",
        ),
    ] {
        let script = repo
            .join("backend/hooks")
            .join(script)
            .to_string_lossy()
            .into_owned();
        let command = format!("\"{}\" \"{script}\"", python.display());
        let missing: Vec<_> = entries
            .into_iter()
            .filter(|(event, matcher)| {
                !data["hooks"].as_array().into_iter().flatten().any(|h| {
                    h["event"] == *event
                        && h["matcher"].as_str() == *matcher
                        && super::hooks::refers_platform(&h["command"], &script, false, false)
                })
            })
            .collect();
        if missing.is_empty() {
            continue;
        }
        if data.get("hooks").is_some() && !raw.contains("[[hooks]]") {
            return Err("hooks inline; configuração preservada".into());
        }
        changed = true;
        out += &format!(
            "\n# hangar: {header}.\n# Remova este bloco para desligar estes hooks do Hangar.\n"
        );
        for (event, matcher) in missing {
            out += &format!("[[hooks]]\nevent = \"{event}\"\n");
            if let Some(matcher) = matcher {
                out += &format!("matcher = \"{matcher}\"\n");
            }
            // String básica também aceita apóstrofo no caminho de instalação.
            out += &format!(
                "command = {}\ntimeout = 5\n\n",
                serde_json::to_string(&command).map_err(|e| e.to_string())?
            );
        }
    }
    if !changed {
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
    crate::runtime::queue::atomic_write(&path, out.as_bytes()).map_err(|e| e.to_string())?;
    Ok(vec![path.to_string_lossy().into_owned()])
}
