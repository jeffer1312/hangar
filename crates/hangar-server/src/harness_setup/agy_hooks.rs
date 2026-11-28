//! A entrada do Hangar no `hooks.json` de cada conta do Antigravity (`<gemini>/config/hooks.json`): só onde o
//! `agy` já foi usado (a pasta de dados existe). A escrita, que preserva os hooks da pessoa, é do crate.
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

pub const MODE: &str = "agy-state-hook";
/// A guarda do multiplexador e do cofre no `PreToolUse`, com a resposta no formato do `agy`.
pub const GUARD_MODE: &str = "agy-guard-tmux";
/// A statusline da conta: é por ela que o `agy` passa a cota ao Hangar.
pub const STATUSLINE_MODE: &str = "agy-statusline";

/// Grava a entrada do Hangar num `hooks.json`; `true` quando mudou.
pub fn install(home: &Path, path: &Path) -> Result<bool, String> {
    let state = crate::provider_hooks::command(home, MODE)?;
    let guard = crate::provider_hooks::command(home, GUARD_MODE)?;
    hangar_antigravity::hooks::install(path, &state, &guard).map_err(|error| error.to_string())
}

/// Aviso da statusline da pessoa: uma vez por processo, porque toda abertura de sessão confere.
static TAKEN_LOGGED: AtomicBool = AtomicBool::new(false);

/// A versão do `agy` desta máquina; None sem o programa ou sem resposta.
pub fn installed_version(env: &crate::accounts::environment::AccountEnvironment) -> Option<(u32, u32, u32)> {
    let program = crate::cli_probe::resolve_program("agy", env)?;
    let process = hangar_antigravity::env::process_env(&hangar_antigravity::Account::default(), env.base.keys().cloned());
    hangar_antigravity::probe::version(&program, &env.home, &process, std::time::Duration::from_secs(10)).ok().flatten()
}

/// Ocupa o `statusLine` da conta; `true` quando mudou. Só com `agy` de versão conhecida e medida; a statusline
/// própria da pessoa fica, e a cota dessa conta não chega ao Hangar.
pub fn status_line(home: &Path, account: &hangar_antigravity::Account, version: Option<(u32, u32, u32)>) -> Result<bool, String> {
    use hangar_antigravity::settings::StatusLine;
    if version.is_none_or(|v| v < hangar_antigravity::version::STATUS_LINE_MIN) {
        tracing::debug!(?version, "agy sem versão medida para a statusline; a cota não é lida");
        return Ok(false);
    }
    let command = crate::provider_hooks::command(home, STATUSLINE_MODE)?;
    let path = account.settings_path(home);
    let ours = |command: &str| super::hooks::runs_subcommand(&serde_json::Value::from(command), STATUSLINE_MODE, cfg!(windows));
    match hangar_antigravity::settings::status_line(&path, &command, ours).map_err(|error| error.to_string())? {
        StatusLine::Written => Ok(true),
        StatusLine::Unchanged => Ok(false),
        StatusLine::Taken => {
            if !TAKEN_LOGGED.swap(true, Ordering::Relaxed) {
                tracing::warn!(code = "agy_statusline_taken", file = %path.display(),
                    "statusline do Antigravity é da pessoa; a cota da conta não chega ao Hangar");
            }
            Ok(false)
        }
    }
}

/// Os `hooks.json` e o `statusLine` gravados; erro de hook de uma conta não impede as outras e volta junto no fim.
/// O `statusLine` só existe na conta padrão (o `agy` com terminal ignora `GEMINI_HOME`), e falha nele vai ao log
/// sem derrubar o resultado dos hooks.
pub fn sync(home: &Path, version: Option<(u32, u32, u32)>) -> Result<Vec<String>, String> {
    let (mut changed, mut errors) = (Vec::new(), Vec::new());
    for account in hangar_antigravity::paths::known_accounts(home) {
        if !account.data_dir(home).is_dir() { continue; }
        let path = account.hooks_path(home);
        match install(home, &path) {
            Ok(true) => changed.push(path.display().to_string()),
            Ok(false) => {}
            Err(error) => errors.push(format!("{}: {error}", path.display())),
        }
        if account.is_default() {
            let settings = account.settings_path(home);
            match status_line(home, &account, version) {
                Ok(true) => changed.push(settings.display().to_string()),
                Ok(false) => {}
                Err(error) => tracing::warn!(code = "agy_statusline_not_written", file = %settings.display(), %error,
                    "statusline do Antigravity não gravada; a cota da conta não atualiza"),
            }
        }
    }
    if errors.is_empty() { Ok(changed) } else { Err(errors.join("; ")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MEASURED: Option<(u32, u32, u32)> = Some(hangar_antigravity::version::STATUS_LINE_MIN);

    #[test]
    fn an_unknown_or_older_agy_never_gets_the_status_line() {
        for version in [None, Some((1, 5, 18))] {
            let home = tempfile::tempdir().unwrap();
            std::fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
            assert_eq!(sync(home.path(), version).unwrap().len(), 1, "{version:?}: só o hooks.json");
            assert!(!home.path().join(".gemini/antigravity-cli/settings.json").exists(), "{version:?}");
        }
    }

    #[test]
    fn a_broken_settings_file_does_not_fail_the_hooks() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".gemini/antigravity-cli");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("settings.json"), "{ quebrado").unwrap();
        let changed = sync(home.path(), MEASURED).expect("os hooks seguem");
        assert_eq!(changed, [home.path().join(".gemini/config/hooks.json").display().to_string()]);
    }

    #[test]
    fn installs_only_where_agy_was_used() {
        let home = tempfile::tempdir().unwrap();
        // Sem a pasta de dados, nada nasce.
        assert_eq!(sync(home.path(), MEASURED).unwrap(), Vec::<String>::new());
        assert!(!home.path().join(".gemini/config/hooks.json").exists());
        std::fs::create_dir_all(home.path().join(".gemini/antigravity-cli")).unwrap();
        let changed = sync(home.path(), MEASURED).unwrap();
        assert_eq!(changed.len(), 2, "hooks.json e settings.json");
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(home.path().join(".gemini/config/hooks.json")).unwrap()).unwrap();
        assert!(doc["hangar"]["Stop"][0]["command"].as_str().unwrap().ends_with(MODE));
        assert!(doc["hangar"]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().ends_with(GUARD_MODE));
        assert!(doc["hangar"].get("PostToolUse").is_none());
        let settings: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
            home.path().join(".gemini/antigravity-cli/settings.json")).unwrap()).unwrap();
        assert!(settings["statusLine"]["command"].as_str().unwrap().ends_with(STATUSLINE_MODE));
        assert_eq!(settings["statusLine"]["stack_with_default"], true);
        assert_eq!(sync(home.path(), MEASURED).unwrap(), Vec::<String>::new(), "segunda vez não regrava");
    }

    #[test]
    fn sync_replaces_an_old_hangar_status_line_and_keeps_the_one_of_the_person() {
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join(".gemini/antigravity-cli");
        std::fs::create_dir_all(&data).unwrap();
        let settings = data.join("settings.json");
        let old = r#"{"statusLine":{"type":"command","command":"\"/velho/bin/hangar-server\" agy-statusline"}}"#;
        std::fs::write(&settings, old).unwrap();
        assert!(sync(home.path(), MEASURED).unwrap().contains(&settings.display().to_string()), "comando antigo do Hangar é trocado");
        let doc: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&settings).unwrap()).unwrap();
        let command = doc["statusLine"]["command"].as_str().unwrap();
        assert!(!command.contains("/velho/") && command.ends_with(STATUSLINE_MODE), "{command}");
        let own = r#"{"statusLine":{"type":"command","command":"echo agy-statusline"}}"#;
        std::fs::write(&settings, own).unwrap();
        assert!(!sync(home.path(), MEASURED).unwrap().contains(&settings.display().to_string()));
        assert_eq!(std::fs::read_to_string(&settings).unwrap(), own, "a barra da pessoa fica");
    }
}
