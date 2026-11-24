//! Comando e ambiente do `agy acp` que o Rust sobe no cano. O `agy acp` não tem `--model` nem `--mode`: o
//! modelo e o modo vão por `session/set_model` e `session/set_mode` depois de abrir; só o `bypassPermissions`
//! é flag (`--dangerously-skip-permissions`). O ambiente é segredo: sai daqui só para o `process::spawn`.
use super::codex_launch::Launch;
use super::protocol::RuntimeError;
use crate::accounts::environment::AccountEnvironment;
use serde_json::Value;

/// A conta do sidecar: `gemini_home` ausente é a padrão (`~/.gemini`); pasta que não é de conta do Hangar recusa.
pub fn account(meta:&Value) -> Result<hangar_antigravity::Account,RuntimeError> {
    match &meta["gemini_home"] {
        Value::Null => Ok(hangar_antigravity::Account::default()),
        Value::String(dir) if dir.is_empty() => Ok(hangar_antigravity::Account::default()),
        Value::String(dir) if hangar_antigravity::paths::is_account_dir(std::path::Path::new(dir)) =>
            Ok(hangar_antigravity::Account::extra(dir.into())),
        _ => Err(RuntimeError::new("antigravity_account_invalid","a conta Google do sidecar não é uma pasta de conta do Hangar")),
    }
}

/// Modo gravado: ausente é o padrão da abertura (`manual`, o `default` do agy); texto fora da lista recusa,
/// para nunca abrir com `--dangerously-skip-permissions` por engano.
pub fn stored_mode(meta:&Value) -> Result<crate::session_open::PermissionMode,RuntimeError> {
    use crate::session_open::PermissionMode;
    match &meta["permission_mode"] {
        Value::Null => Ok(PermissionMode::Manual),
        Value::String(mode) => PermissionMode::antigravity(mode.trim()).ok_or_else(||RuntimeError::new("launch_permission_mode",
            &format!("modo de permissão desconhecido no sidecar: {mode:?}; a sessão não foi aberta"))),
        _ => Err(RuntimeError::new("launch_permission_mode","modo de permissão do sidecar não é texto")),
    }
}

/// O `modeId` do `session/set_mode` para o modo do Hangar; `bypassPermissions` não tem modo (é a flag).
pub fn acp_mode(mode:crate::session_open::PermissionMode) -> Option<&'static str> {
    use crate::session_open::PermissionMode as M;
    match mode { M::Manual => Some("default"), M::AcceptEdits => Some("accept-edits"), M::Auto => Some("auto"), M::Plan => Some("plan"), _ => None }
}

pub fn argv(meta:&Value) -> Result<Vec<String>,RuntimeError> {
    let mut argv = vec!["agy".to_owned(),"acp".to_owned()];
    if stored_mode(meta)? == crate::session_open::PermissionMode::BypassPermissions { argv.push("--dangerously-skip-permissions".into()); }
    Ok(argv)
}

/// O ambiente do servidor sem o pane de quem o subiu, com a conta Google da sessão, a chave dela e o Jev da abertura.
pub fn launch_env(base:&AccountEnvironment,meta:&Value) -> Result<Launch,RuntimeError> {
    let field = |name:&str|meta[name].as_str().filter(|value|!value.is_empty())
        .ok_or_else(||RuntimeError::new("launch_meta",&format!("sidecar sem `{name}`")));
    let (name,key) = (field("name")?,field("key")?);
    let account = account(meta)?;
    let process = hangar_antigravity::env::process_env(&account,base.base.keys().cloned());
    let mut env = base.base.clone();
    for unset in process.unset.iter().chain(["TMUX".to_owned(),"TMUX_PANE".to_owned()].iter()) { env.remove(unset); }
    env.extend(process.set);
    env.insert("CP_SESSION_NAME".into(),name.into());
    env.insert("CP_SESSION_KEY".into(),key.into());
    env.insert("HANGAR_CANO_KEY".into(),key.into());
    env.insert("HANGAR_CANO_OWNER".into(),base.home.to_string_lossy().into_owned());
    // O `mcp-stdio` repassa o HANGAR_JEV ao servidor; sem ele, browser_goal e browser_check recusam.
    env.extend(super::codex_launch::jev_env(meta["jev"].as_bool() == Some(true),&super::codex_launch::runtime_config(base)));
    Ok(Launch { program:argv(meta)?,env })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn base() -> AccountEnvironment {
        AccountEnvironment::from_pairs([("HOME","/h"),("PATH","/bin"),("TMUX","/tmp/t,1,0"),("TMUX_PANE","%3"),
            ("SSH_CONNECTION","10.0.0.1 22 10.0.0.2 5000"),("GEMINI_API_KEY","x"),("GOOGLE_CLOUD_REGION","r")])
    }

    #[test]
    fn the_default_account_runs_agy_acp_without_the_operator_pane_or_inherited_google_settings() {
        let launch = launch_env(&base(),&json!({"name":"ag","key":"k1"})).unwrap();
        assert_eq!(launch.program,["agy","acp"]);
        for gone in ["TMUX","TMUX_PANE","SSH_CONNECTION","GEMINI_API_KEY","GOOGLE_CLOUD_REGION","GEMINI_HOME"] {
            assert!(!launch.env.contains_key(gone),"{gone} herdado");
        }
        assert_eq!(launch.env["AGY_CLI_DISABLE_AUTO_UPDATE"],"1");
        assert_eq!((launch.env["CP_SESSION_KEY"].as_str(),launch.env["HANGAR_CANO_KEY"].as_str()),("k1","k1"));
        assert_eq!(launch.env["HANGAR_CANO_OWNER"],"/h");
        assert_eq!(launch.env["PATH"],"/bin");
        assert_eq!(launch.env["HANGAR_JEV"],"off");
    }

    #[test]
    fn the_jev_chosen_at_opening_reaches_agy_acp_with_the_configured_destination() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".claude")).unwrap();
        std::fs::write(home.path().join(".claude/runtime-config.json"),json!({"jev_api_key":"tsk-fake"}).to_string()).unwrap();
        let base = AccountEnvironment::from_pairs([("HOME",&*home.path().to_string_lossy()),("PATH","/bin")]);
        let on = launch_env(&base,&json!({"name":"ag","key":"k1","jev":true})).unwrap();
        assert_eq!((on.env["HANGAR_JEV"].as_str(),on.env["TYPESAFE_API_KEY"].as_str()),("on","tsk-fake"));
        let off = launch_env(&base,&json!({"name":"ag","key":"k1","jev":false})).unwrap();
        assert_eq!(off.env["HANGAR_JEV"],"off");
        assert!(!off.env.contains_key("TYPESAFE_API_KEY"),"desligado não leva a chave");
    }

    #[test]
    fn an_extra_account_gets_its_folder_and_bypass_is_the_only_flag() {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".antigravity-trabalho");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(hangar_antigravity::paths::MARKER),"").unwrap();
        let meta = json!({"name":"ag","key":"k1","gemini_home":dir,"permission_mode":"bypassPermissions"});
        let launch = launch_env(&base(),&meta).unwrap();
        assert_eq!(launch.program,["agy","acp","--dangerously-skip-permissions"]);
        assert_eq!(launch.env["GEMINI_HOME"],dir.to_string_lossy());
        assert_eq!(launch.env["SSH_CONNECTION"],hangar_antigravity::env::SEPARATE_LOGIN);
        for mode in ["manual","acceptEdits","auto","plan"] {
            assert_eq!(argv(&json!({"permission_mode":mode})).unwrap(),["agy","acp"],"{mode}");
        }
    }

    #[test]
    fn a_torn_sidecar_never_opens() {
        assert_eq!(argv(&json!({"permission_mode":"Full Access"})).unwrap_err().code,"launch_permission_mode");
        assert_eq!(launch_env(&base(),&json!({"name":"ag","key":"k1","gemini_home":"/etc"})).unwrap_err().code,"antigravity_account_invalid");
        assert_eq!(launch_env(&base(),&json!({"name":"ag"})).unwrap_err().code,"launch_meta");
    }
}
