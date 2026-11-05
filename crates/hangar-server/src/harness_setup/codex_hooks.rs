//! Hooks próprios do Codex conservam comandos funcionais e a aprovação por índice.
use super::hooks::refers_platform;
use serde_json::{Value, json};
use std::{fs, path::Path};
fn command(python: &Path, script: &Path, windows: bool, guard: bool) -> String {
    let base = format!("\"{}\" \"{}\"", python.display(), script.display());
    match (windows, guard) {
        (true, true) => format!("& {base} ; exit $LASTEXITCODE"),
        (true, false) => format!("& {base} ; exit 0"),
        (false, true) => base,
        (false, false) => format!("{base} || exit 0"),
    }
}
pub fn sync(home: &Path, repo: &Path, windows: bool) -> Result<Vec<String>, String> {
    sync_impl(home, repo, windows, false)
}

pub fn sync_checked(home: &Path, repo: &Path, windows: bool) -> Result<Vec<String>, String> {
    sync_impl(home, repo, windows, true)
}

fn sync_impl(
    home: &Path,
    repo: &Path,
    windows: bool,
    checked: bool,
) -> Result<Vec<String>, String> {
    if !home.is_dir() {
        return Ok(vec![]);
    }
    let path = home.join("hooks.json");
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) if checked && e.kind() == std::io::ErrorKind::InvalidData => {
            return Err("hooks.json não está em UTF-8".into());
        }
        Err(e) => return Err(e.to_string()),
    };
    let mut data: Value = if crate::accounts::quotas::other::py_strip(&raw).is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) if checked => return Err("hooks.json inválido; hooks preservados".into()),
            Err(_) => return Ok(vec![]),
        }
    };
    if !data.is_object()
        || data
            .get("hooks")
            .is_some_and(|v| !v.is_object())
    {
        return if checked {
            Err("hooks.json inválido; hooks preservados".into())
        } else {
            Ok(vec![])
        };
    }
    if data.get("hooks").is_none() {
        data["hooks"] = json!({});
    }
    // O argumento windows permite provar as formas de comando sem iniciar PowerShell.
    let python = crate::install_root::python_exe(repo, cfg!(windows));
    let mut touched = Vec::new();
    for (script, events, guard) in [
        (
            "state_hook.py",
            vec![
                "SessionStart",
                "UserPromptSubmit",
                "PreToolUse",
                "PostToolUse",
                "Stop",
            ],
            false,
        ),
        ("nav_hook.py", vec!["UserPromptSubmit"], false),
        ("guard_tmux.py", vec!["PreToolUse"], true),
    ] {
        let command = command(
            &python,
            &repo.join("backend/hooks").join(script),
            windows,
            guard,
        );
        for event in events {
            let groups = data["hooks"]
                .as_object_mut()
                .unwrap()
                .entry(event)
                .or_insert_with(|| json!([]));
            let Some(groups) = groups.as_array_mut() else {
                continue;
            };
            let found = groups
                .iter()
                .flat_map(|g| g["hooks"].as_array().into_iter().flatten())
                .any(|h| refers_platform(&h["command"], script, true, windows));
            if !found {
                let block = if guard {
                    json!({"matcher":"Bash","hooks":[{"type":"command","command":command}]})
                } else {
                    json!({"hooks":[{"type":"command","command":command}]})
                };
                groups.push(block);
                if !touched.iter().any(|e| e == event) {
                    touched.push(event.to_owned());
                }
            }
        }
        if windows && script != "nav_hook.py" {
            for (event, groups) in data["hooks"].as_object_mut().unwrap() {
                for group in groups.as_array_mut().into_iter().flatten() {
                    for hook in group
                        .get_mut("hooks")
                        .and_then(Value::as_array_mut)
                        .into_iter()
                        .flatten()
                    {
                        if refers_platform(&hook["command"], script, true, windows)
                            && !hook["command"]
                                .as_str()
                                .unwrap_or("")
                                .trim_start()
                                .starts_with('&')
                        {
                            hook["command"] = json!(command);
                            if !touched.contains(event) {
                                touched.push(event.clone());
                            }
                        }
                    }
                }
            }
        }
    }
    touched.extend(repair_legacy(&mut data["hooks"], repo, &python, windows));
    let mut seen = std::collections::BTreeSet::new();
    touched.retain(|event| seen.insert(event.clone()));
    if !touched.is_empty() {
        let resolved = fs::canonicalize(&path).unwrap_or(path);
        super::files::write_preserving(
            &resolved,
            &format!(
                "{}\n",
                crate::transcript::pyjson::dumps_indent(&data, 2, false)
            ),
        )?;
    }
    Ok(touched)
}

pub fn repair_legacy(hooks: &mut Value, repo: &Path, python: &Path, windows: bool) -> Vec<String> {
    let legacy = repo.join(if windows { "backend/.venv/Scripts/python.exe" } else { "backend/.venv/bin/python" });
    if legacy.is_file() || !python.is_file() { return Vec::new(); }
    let commands = [
        ("state_hook.py", command(python, &repo.join("backend/hooks/state_hook.py"), windows, false)),
        ("nav_hook.py", command(python, &repo.join("backend/hooks/nav_hook.py"), windows, false)),
        ("guard_tmux.py", command(python, &repo.join("backend/hooks/guard_tmux.py"), windows, true)),
    ];
    let mut touched = Vec::new();
    for (event, groups) in hooks.as_object_mut().into_iter().flatten() {
        for group in groups.as_array_mut().into_iter().flatten() {
            for hook in group.get_mut("hooks").and_then(Value::as_array_mut).into_iter().flatten() {
                let Some(text) = hook.get("command").and_then(Value::as_str) else { continue; };
                let tokens = super::hooks::tokens(text, windows);
                let program = if tokens.first().is_some_and(|v|v=="&") && tokens.len()>1 {&tokens[1]} else {tokens.first().map(String::as_str).unwrap_or("")};
                if Path::new(program.trim_matches(['\"','\''])) != legacy { continue; }
                for (script, replacement) in &commands {
                    if refers_platform(&hook["command"], script, true, windows) {
                        hook["command"] = json!(replacement);
                        touched.push(event.clone());
                        break;
                    }
                }
            }
        }
    }
    touched
}
