//! Estado da sessão Claude sem sinal: o que o `_state_stream` do Python mostra para a sessão parada, lido
//! do sidecar `~/.hangar/claude-headless/<nome>.json` e do fim do transcript. Só leitura.
use std::path::{Path, PathBuf};

use hangar_api::state::StateEvent;
use serde_json::{Value, json};

fn sidecar_path(name: &str, home: &Path) -> PathBuf {
    home.join(".hangar").join("claude-headless").join(format!("{}.json", crate::list::discover_other::sanitize_session_name(name)))
}

/// Pasta `projects` do Claude para a conta: a da sessão, ou a padrão (`CP_PROJECTS_DIR`, `CLAUDE_CONFIG_DIR`, `~/.claude`).
fn projects_dir(config_dir: Option<&str>, home: &Path) -> PathBuf {
    if let Some(dir) = config_dir.filter(|dir| !dir.is_empty()) { return Path::new(dir).join("projects"); }
    let env = |key: &str| std::env::var_os(key).filter(|value| !value.is_empty()).map(PathBuf::from);
    env("CP_PROJECTS_DIR").unwrap_or_else(|| env("CLAUDE_CONFIG_DIR").unwrap_or_else(|| home.join(".claude")).join("projects"))
}

/// O transcript esperado; o `EnterWorktree` o move para a pasta do cwd da worktree, e o id não repete entre pastas.
fn transcript_path(meta: &Value, home: &Path) -> Option<PathBuf> {
    let (cwd, sid) = (meta["cwd"].as_str()?, meta["session_id"].as_str()?);
    let base = projects_dir(meta["config_dir"].as_str(), home);
    let expected = base.join(hangar_workspace::worktrees::sanitize_cwd(cwd)).join(format!("{sid}.jsonl"));
    if expected.exists() { return Some(expected); }
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&base).ok()?.flatten().map(|entry| entry.path()).collect();
    folders.sort();
    Some(folders.into_iter().map(|folder| folder.join(format!("{sid}.jsonl"))).find(|path| path.exists()).unwrap_or(expected))
}

/// `_linha_parada`: o que a sessão escolheu na abertura e o contexto que o transcript mostra.
fn status_line(meta: &Value, home: &Path) -> Option<String> {
    use crate::runtime::local_policy::run_at;
    let usage = if meta["context_window"].as_f64().is_some_and(|window| window != 0.0) {
        transcript_path(meta, home)
            .and_then(|path| run_at("last_usage", &json!({}), &json!({"provider": "claude", "jsonl": path}), None, 0.0)?.ok())
            .map_or(Value::Null, |found| found["usage"].clone())
    } else { Value::Null };
    let payload = json!({"model": meta["model"], "effort": meta["effort"], "context_window": meta["context_window"], "usage": usage});
    let ctx = json!({"provider": "claude", "engine_account": meta["engine_account"], "config_dir": meta["config_dir"]});
    run_at("format_status", &payload, &ctx, None, 0.0)?.ok()?["status_line"].as_str().map(str::to_owned)
}

/// `None` = sem sidecar (o Python diria `dead`, mas só ele sabe se a sessão está só trocando de modo).
pub fn parked_state_at(name: &str, home: &Path) -> Option<StateEvent> {
    let path = sidecar_path(name, home);
    if !path.exists() { return None; }
    let meta = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()).filter(Value::is_object).unwrap_or_else(|| json!({}));
    let text = |key: &str| meta[key].as_str().map(str::to_owned);
    // O problema da última vida mora no sidecar: o Python o grava ao registrá-lo e o relê depois de reiniciar.
    let problem = meta["problema"].as_array().and_then(|pair| Some((pair.first()?.as_str()?.to_owned(), pair.get(1).and_then(Value::as_str).map(str::to_owned))));
    Some(StateEvent {
        session: name.into(), state: "idle".into(), headless: true,
        claude_permission_mode: text("permission_mode"), claude_previous_non_plan: text("previous_non_plan"),
        status_line: status_line(&meta, home),
        problema: problem.as_ref().map(|(code, _)| code.clone()), problema_detalhe: problem.and_then(|(_, detail)| detail),
        ..Default::default()
    })
}
