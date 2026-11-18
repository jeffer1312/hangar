//! Hooks próprios do Codex: `nav-hook` e `guard-tmux` do `hangar-server`, uma entrada nossa por evento.
//! O hook de estado saiu (a lista do Codex lê o estado nativo). Comando alterado no Codex é hook não
//! aprovado: o lançador e o ator Codex confiam de novo nos pendentes.
use super::hooks::{refers_platform, remove_hook, sync_hook_any};
use serde_json::{Value, json};
use std::{fs, path::Path};

/// No Windows o Codex roda o hook pelo PowerShell, onde `"exe" "arg"` é erro de sintaxe: `& "exe" …`.
/// O guarda leva o código de saída ao Codex (2 barra); os outros nunca barram.
pub fn command(program: &Path, mode: &str, windows: bool, guard: bool) -> String {
    let base = format!("\"{}\" {mode}", program.display());
    match (windows, guard) {
        (true, true) => format!("& {base} ; exit $LASTEXITCODE"),
        (true, false) => format!("& {base} ; exit 0"),
        (false, true) => base,
        (false, false) => format!("{base} || exit 0"),
    }
}

/// `home` = pasta da conta Codex; `user_home` = casa do usuário, onde mora o lançador estável.
pub fn sync(home: &Path, user_home: &Path, windows: bool) -> Result<Vec<String>, String> {
    sync_impl(home, user_home, windows, false)
}

pub fn sync_checked(home: &Path, user_home: &Path, windows: bool) -> Result<Vec<String>, String> {
    sync_impl(home, user_home, windows, true)
}

fn sync_impl(
    home: &Path,
    user_home: &Path,
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
    let program = crate::codex_setup::hook_command::hook_server(user_home)?;
    let (allow, _) = crate::codex_setup::hook_command::allow_wrapper()?;
    let touched = reconcile(&mut data, &program, &allow, windows);
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

/// Os hooks do Hangar num `hooks.json` já lido; devolve os eventos alterados. `allow` é o programa do
/// `codex-hook-allow` que a integração já grava.
pub fn reconcile(data: &mut Value, program: &Path, allow: &str, windows: bool) -> Vec<String> {
    let mut touched = Vec::new();
    let mut mark = |event: &str, changed: bool| {
        if changed && !touched.iter().any(|e| e == event) {
            touched.push(event.to_owned());
        }
    };
    // O Python só gravou o script na pasta `backend/hooks` do Hangar; homônimo em outra pasta é pessoal.
    let events: Vec<String> = data["hooks"].as_object().map(|events| events.keys().cloned().collect()).unwrap_or_default();
    for event in &events {
        mark(event, remove_hook(data, event, &["backend/hooks/state_hook.py", "state-hook"], windows));
    }
    let nav = command(program, "nav-hook", windows, false);
    mark("UserPromptSubmit", sync_hook_any(data, "UserPromptSubmit", Some(&nav), None, &["backend/hooks/nav_hook.py", "nav-hook"], windows));
    let guard = command(program, "guard-tmux", windows, true);
    mark("PreToolUse", sync_hook_any(data, "PreToolUse", Some(&guard), Some("Bash"), &["backend/hooks/guard_tmux.py", "guard-tmux"], windows));
    for (event, changed) in repair_foreign(data, program, allow, windows) {
        mark(&event, changed);
    }
    touched
}

/// Entradas que vieram do Claude e apontam para o que deixou de existir: o `pair-context` de outro
/// binário (um de teste) passa ao programa dos hooks, e o `codex-hook-allow.py` de um checkout ao
/// `codex-hook-allow` do servidor.
fn repair_foreign(data: &mut Value, program: &Path, allow: &str, windows: bool) -> Vec<(String, bool)> {
    let mut touched = Vec::new();
    let Some(events) = data.get_mut("hooks").and_then(Value::as_object_mut) else { return touched };
    for (event, groups) in events.iter_mut() {
        for group in groups.as_array_mut().into_iter().flatten() {
            for hook in group.get_mut("hooks").and_then(Value::as_array_mut).into_iter().flatten() {
                let Some(text) = hook.get("command").and_then(Value::as_str).map(str::to_owned) else { continue };
                let replacement = if refers_platform(&json!(text), "pair-context", true, windows) {
                    with_program(&text, program, windows)
                } else if refers_platform(&json!(text), "codex-hook-allow.py", true, windows) {
                    let single = json!({"hooks": {"PreToolUse": [{"hooks": [hook.clone()]}]}});
                    let fixed = crate::codex_setup::normalize::hooks(&single, allow, "codex-hook-allow", windows);
                    fixed["hooks"]["PreToolUse"][0]["hooks"][0]["command"].as_str().map(str::to_owned)
                } else {
                    None
                };
                if let Some(replacement) = replacement.filter(|replacement| *replacement != text) {
                    hook["command"] = json!(replacement);
                    touched.push((event.clone(), true));
                }
            }
        }
    }
    touched
}

/// Troca só o programa (primeiro token, com ou sem `& ` do PowerShell) e mantém os argumentos.
fn with_program(text: &str, program: &Path, windows: bool) -> Option<String> {
    let (prefix, rest) = match text.strip_prefix("& ") {
        Some(rest) if windows => ("& ", rest),
        _ => ("", text),
    };
    let tail = if let Some(quoted) = rest.strip_prefix('"') {
        &quoted[quoted.find('"')? + 1..]
    } else {
        &rest[rest.find(char::is_whitespace)?..]
    };
    // Só programa do Hangar: comando pessoal que por acaso cita `pair-context` fica como está.
    let current = super::hooks::tokens(rest, windows).into_iter().next()?;
    if !super::hooks::hangar_program(&current) {
        return None;
    }
    Some(format!("{prefix}\"{}\"{tail}", program.display()))
}
