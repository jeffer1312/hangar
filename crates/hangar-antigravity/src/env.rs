//! Ambiente do processo `agy` de uma conta. O tmux não tira variável do ambiente dele por `-e`, então quem lança
//! o pane aplica `unset` com `env -u`.
use crate::Account;

/// O `agy` 1.5 guarda o login da pessoa num item só do chaveiro do sistema, igual para toda pasta; com
/// `SSH_CONNECTION` presente ele grava no arquivo da pasta. É o que separa uma conta adicional da padrão.
pub const SEPARATE_LOGIN: &str = "127.0.0.1 0 127.0.0.1 0";

/// Credenciais e escolhas herdadas que trocariam a conta ou o modo de autenticação por baixo do Hangar.
const INHERITED: [&str; 12] = [
    "GEMINI_API_KEY", "GOOGLE_API_KEY", "GOOGLE_APPLICATION_CREDENTIALS", "GOOGLE_CLOUD_PROJECT", "GOOGLE_CLOUD_LOCATION",
    "GOOGLE_CLOUD_QUOTA_PROJECT", "GOOGLE_GENAI_USE_VERTEXAI", "GCLOUD_PROJECT", "GEMINI_HOME", "AGY_ACP_FORCE_FILE_STORAGE",
    "AGY_ADC_AUTH", "BROWSER",
];
const INHERITED_PREFIXES: [&str; 2] = ["GOOGLE_CLOUD_", "AGY_ACP_"];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProcessEnv {
    pub set: Vec<(String, String)>,
    pub unset: Vec<String>,
}

impl ProcessEnv {
    /// `env -u A -u B` na frente do comando do pane.
    pub fn env_prefix(&self) -> Vec<String> {
        if self.unset.is_empty() { return Vec::new(); }
        let mut argv = vec!["env".to_owned()];
        for key in &self.unset { argv.extend(["-u".to_owned(), key.clone()]); }
        argv
    }

    /// O argv do pane com o `env -u` na frente. O psmux não tem `env`: no Windows o pane herda de quem chama.
    pub fn command(&self, windows: bool, argv: Vec<String>) -> Vec<String> {
        if windows { return argv; }
        let mut command = self.env_prefix();
        command.extend(argv);
        command
    }
}

/// `inherited` são os nomes do ambiente de quem lança: entram no `unset` os que casam com os prefixos.
pub fn process_env(account: &Account, inherited: impl IntoIterator<Item = String>) -> ProcessEnv {
    let mut unset: Vec<String> = INHERITED.iter().map(|k| (*k).to_owned()).collect();
    for key in inherited {
        if INHERITED_PREFIXES.iter().any(|p| key.starts_with(p)) && !unset.contains(&key) { unset.push(key); }
    }
    let mut set = vec![("AGY_CLI_DISABLE_AUTO_UPDATE".to_owned(), "1".to_owned())];
    match &account.gemini_home {
        Some(dir) => {
            set.push(("GEMINI_HOME".to_owned(), dir.to_string_lossy().into_owned()));
            set.push(("SSH_CONNECTION".to_owned(), SEPARATE_LOGIN.to_owned()));
            set.push(("AGY_ACP_FORCE_FILE_STORAGE".to_owned(), "1".to_owned()));
        }
        // Servidor tmux nascido de login SSH passaria um SSH_CONNECTION real e isolaria a conta padrão.
        None => unset.push("SSH_CONNECTION".to_owned()),
    }
    unset.retain(|key| !set.iter().any(|(k, _)| k == key));
    ProcessEnv { set, unset }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn value<'a>(env: &'a ProcessEnv, key: &str) -> Option<&'a str> {
        env.set.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    #[test]
    fn the_default_account_drops_an_inherited_ssh_connection_and_never_sets_a_home() {
        let env = process_env(&Account::default(), ["GOOGLE_CLOUD_REGION".to_owned(), "PATH".to_owned()]);
        assert_eq!(value(&env, "AGY_CLI_DISABLE_AUTO_UPDATE"), Some("1"));
        assert_eq!(value(&env, "GEMINI_HOME"), None);
        assert!(env.unset.contains(&"SSH_CONNECTION".to_owned()));
        assert!(env.unset.contains(&"GEMINI_HOME".to_owned()));
        assert!(env.unset.contains(&"GOOGLE_CLOUD_REGION".to_owned()));
        assert!(!env.unset.contains(&"PATH".to_owned()));
    }

    #[test]
    fn an_extra_account_gets_its_folder_and_a_separate_login() {
        let env = process_env(&Account::extra(PathBuf::from("/h/.antigravity-x")), []);
        assert_eq!(value(&env, "GEMINI_HOME"), Some("/h/.antigravity-x"));
        assert_eq!(value(&env, "SSH_CONNECTION"), Some(SEPARATE_LOGIN));
        assert_eq!(value(&env, "AGY_ACP_FORCE_FILE_STORAGE"), Some("1"));
        assert!(!env.unset.iter().any(|k| k == "GEMINI_HOME" || k == "SSH_CONNECTION" || k == "AGY_ACP_FORCE_FILE_STORAGE"));
    }

    #[test]
    fn the_prefix_unsets_each_variable() {
        let env = ProcessEnv { set: vec![], unset: vec!["A".into(), "B".into()] };
        assert_eq!(env.env_prefix(), ["env", "-u", "A", "-u", "B"]);
        assert!(ProcessEnv::default().env_prefix().is_empty());
        assert_eq!(env.command(false, vec!["agy".into()]), ["env", "-u", "A", "-u", "B", "agy"]);
        assert_eq!(env.command(true, vec!["agy".into()]), ["agy"]);
    }
}
