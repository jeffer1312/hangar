//! A entrada do Hangar no `hooks.json` de cada conta do Antigravity (`<gemini>/config/hooks.json`): só onde o
//! `agy` já foi usado (a pasta de dados existe). A escrita, que preserva os hooks da pessoa, é do crate.
use std::path::Path;

pub const MODE: &str = "agy-state-hook";
/// A guarda do multiplexador e do cofre no `PreToolUse`, com a resposta no formato do `agy`.
pub const GUARD_MODE: &str = "agy-guard-tmux";

/// Grava a entrada do Hangar num `hooks.json`; `true` quando mudou.
pub fn install(home: &Path, path: &Path) -> Result<bool, String> {
    let state = crate::provider_hooks::command(home, MODE)?;
    let guard = crate::provider_hooks::command(home, GUARD_MODE)?;
    hangar_antigravity::hooks::install(path, &state, &guard).map_err(|error| error.to_string())
}

/// Os `hooks.json` gravados; erro de uma conta não impede as outras e volta junto no fim.
pub fn sync(home: &Path) -> Result<Vec<String>, String> {
    let (mut changed, mut errors) = (Vec::new(), Vec::new());
    for account in hangar_antigravity::paths::known_accounts(home) {
        if !account.data_dir(home).is_dir() { continue; }
        let path = account.hooks_path(home);
        match install(home, &path) {
            Ok(true) => changed.push(path.display().to_string()),
            Ok(false) => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
    }
    if errors.is_empty() { Ok(changed) } else { Err(errors.join("; ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installs_only_where_agy_was_used() {
        let home = tempfile::tempdir().unwrap();
        // Sem a pasta de dados, nada nasce.
        assert_eq!(sync(home.path()).unwrap(), Vec::<String>::new());
        assert!(!home.path().join(".gemini/config/hooks.json").exists());
        std::fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        let changed = sync(home.path()).unwrap();
        assert_eq!(changed.len(), 1);
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.path().join(".gemini/config/hooks.json")).unwrap()).unwrap();
        assert!(doc["hangar"]["Stop"][0]["command"].as_str().unwrap().ends_with(MODE));
        assert!(doc["hangar"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().ends_with(GUARD_MODE));
        assert!(doc["hangar"].get("PostToolUse").is_none());
        assert_eq!(sync(home.path()).unwrap(), Vec::<String>::new(), "segunda vez não regrava");
    }
}
