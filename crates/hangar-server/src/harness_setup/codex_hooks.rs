//! Hooks próprios do Codex conservam comandos funcionais e a aprovação por índice.
use super::hooks::refers;
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
    if !home.is_dir() {
        return Ok(vec![]);
    }
    let path = home.join("hooks.json");
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.to_string()),
    };
    let mut data: Value = if raw.trim().is_empty() {
        json!({})
    } else {
        match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => return Ok(vec![]),
        }
    };
    if !data.is_object()
        || data
            .get("hooks")
            .is_some_and(|v| !v.is_null() && !v.is_object())
    {
        return Ok(vec![]);
    }
    if data["hooks"].is_null() {
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
                .any(|h| refers(&h["command"], script, true));
            if !found {
                let mut block = json!({"hooks":[{"type":"command","command":command}]});
                if guard {
                    block["matcher"] = json!("Bash");
                }
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
                        if refers(&hook["command"], script, true)
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
