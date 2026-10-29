//! Reconciliação dos comandos instalados, preservando a ordem dos hooks pessoais.
use crate::accounts::{AccountService, Provider};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};
static LOCK: Mutex<()> = Mutex::new(());

fn tokens(command: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    for c in command.chars() {
        if quote == Some(c) {
            quote = None;
        } else if quote.is_none() && (c == '\'' || c == '"') {
            quote = Some(c);
        } else if quote.is_none() && c.is_whitespace() {
            if !current.is_empty() {
                result.push(std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        result.push(current);
    }
    result
}
pub fn refers(command: &Value, script: &str, basename: bool) -> bool {
    command.as_str().is_some_and(|c| {
        tokens(c).iter().any(|t| {
            if basename {
                t.rsplit(['/', '\\']).next() == script.rsplit(['/', '\\']).next()
            } else {
                t == script
            }
        })
    })
}
pub fn sync_hook(
    data: &mut Value,
    event: &str,
    command: Option<&str>,
    matcher: Option<&str>,
    script: &str,
) -> bool {
    let Some(root) = data.as_object_mut() else {
        return false;
    };
    let hooks = root.entry("hooks").or_insert_with(|| json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return false;
    };
    let events = hooks.entry(event).or_insert_with(|| json!([]));
    let Some(events) = events.as_array_mut() else {
        return false;
    };
    let mut changed = false;
    let mut kept = command.is_none();
    events.retain_mut(|block| {
        let Some(entries) = block.get_mut("hooks").and_then(Value::as_array_mut) else {
            return true;
        };
        let before = entries.len();
        entries.retain_mut(|hook| {
            if !hook.is_object() || !refers(&hook["command"], script, true) {
                return true;
            }
            if kept {
                changed = true;
                return false;
            }
            kept = true;
            if hook["command"].as_str() != command {
                hook["command"] = json!(command);
                changed = true;
            }
            hook.as_object_mut()
                .unwrap()
                .entry("type")
                .or_insert(json!("command"));
            true
        });
        before == entries.len() || !entries.is_empty()
    });
    if !kept {
        let mut block = json!({"hooks":[{"type":"command","command":command}]});
        if let Some(matcher) = matcher {
            block["matcher"] = json!(matcher);
        }
        events.push(block);
        changed = true;
    }
    changed
}
fn guard(dir: &Path, script: &Path) -> PathBuf {
    let path = dir.join("hooks/guard_tmux.py");
    let result = (|| -> std::io::Result<()> {
        fs::create_dir_all(path.parent().unwrap())?;
        if path.is_symlink()
            && fs::canonicalize(&path)
                .ok()
                .zip(fs::canonicalize(script).ok())
                .is_some_and(|(a, b)| a == b)
        {
            return Ok(());
        }
        if fs::symlink_metadata(&path).is_ok() {
            fs::remove_file(&path)?;
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(script, &path)?;
        #[cfg(windows)]
        std::os::windows::fs::symlink_file(script, &path)?;
        Ok(())
    })();
    if result.is_err() {
        return script.into();
    }
    fs::canonicalize(path.parent().unwrap())
        .unwrap_or_else(|_| path.parent().unwrap().into())
        .join("guard_tmux.py")
}
fn safe_command(python: &Path, script: &Path, event: &str, telemetry: bool) -> String {
    let command = format!("\"{}\" \"{}\"", python.display(), script.display());
    if !telemetry {
        return command;
    }
    if ["SessionStart", "UserPromptSubmit"].contains(&event) {
        let name = script.file_stem().unwrap_or_default().to_string_lossy();
        format!(
            "{command} || echo \"[hangar] O hook {name} do Hangar falhou em {event} e foi ignorado. AVISE O USUARIO agora: parte do app pode estar sem estado, pareamento ou navegador. Para ver o erro, rode o comando do hook a mao.\""
        )
    } else {
        format!("{command} || exit 0")
    }
}
pub fn sync(service: &AccountService, repo: &Path) -> Value {
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let python = crate::install_root::python_exe(repo, cfg!(windows));
    let mut dirs: Vec<_> = service
        .snapshot(Provider::Claude)
        .unwrap_or_default()
        .into_iter()
        .map(|a| a.home)
        .chain([service.env.claude_base.clone()])
        .collect();
    dirs.sort_by_key(|d| (d.file_name().is_none_or(|n| n != ".claude"), d.clone()));
    dirs.dedup();
    let mut seen = BTreeSet::new();
    let mut touched = Vec::new();
    for dir in dirs {
        if !dir.is_dir() {
            continue;
        }
        let settings = dir.join("settings.json");
        let path = fs::canonicalize(&settings).unwrap_or(settings);
        if !seen.insert(path.clone()) {
            continue;
        }
        let result = (|| -> Result<bool, String> {
            let text = match fs::read_to_string(&path) {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
                Err(e) => return Err(e.to_string()),
            };
            let mut data: Value = if text.trim().is_empty() {
                json!({})
            } else {
                serde_json::from_str(&text).map_err(|e| e.to_string())?
            };
            if !data.is_object()
                || data
                    .get("hooks")
                    .is_some_and(|h| !h.is_null() && !h.is_object())
            {
                return Ok(false);
            }
            if data["hooks"].is_null() {
                data["hooks"] = json!({});
            }
            let mut changed = false;
            for (script, events, matcher) in [
                (
                    "state_hook.py",
                    vec![
                        "UserPromptSubmit",
                        "PreToolUse",
                        "PostToolUse",
                        "Notification",
                        "Stop",
                        "SessionStart",
                    ],
                    None,
                ),
                ("preview_hook.py", vec!["MessageDisplay", "Stop"], None),
                (
                    "subagent_hook.py",
                    vec!["SubagentStart", "SubagentStop"],
                    None,
                ),
                ("nav_hook.py", vec!["UserPromptSubmit"], None),
                (
                    "askq_capture.py",
                    vec!["PreToolUse", "PostToolUse", "PostToolUseFailure"],
                    Some("AskUserQuestion"),
                ),
            ] {
                for event in events {
                    let command = safe_command(
                        &python,
                        &repo.join("backend/hooks").join(script),
                        event,
                        true,
                    );
                    changed |= sync_hook(&mut data, event, Some(&command), matcher, script);
                }
            }
            let command = safe_command(
                &python,
                &guard(&dir, &repo.join("backend/hooks/guard_tmux.py")),
                "PreToolUse",
                false,
            );
            changed |= sync_hook(
                &mut data,
                "PreToolUse",
                Some(&command),
                Some("Bash"),
                "guard_tmux.py",
            );
            changed |= sync_hook(&mut data, "SessionStart", None, None, "pair_hook.py");
            if let Ok(exe) = std::env::current_exe() {
                let command = format!(
                    "\"{}\" pair-context \"{}\" || exit 0",
                    exe.display(),
                    service.env.claude_base.join(".hangar-pair").display()
                );
                changed |= sync_hook(
                    &mut data,
                    "SessionStart",
                    Some(&command),
                    Some("startup|resume|clear|compact"),
                    "pair-context",
                );
            }
            if changed {
                super::files::write_preserving(
                    &path,
                    &format!(
                        "{}\n",
                        crate::transcript::pyjson::dumps_indent(&data, 2, false)
                    ),
                )?;
            }
            Ok(changed)
        })();
        match result {
            Ok(true) => touched.push(path),
            Err(error) => tracing::warn!(%error,"Hooks não atualizados"),
            _ => {}
        }
    }
    json!({"claude":touched})
}
