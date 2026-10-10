//! Dono da chamada de voz no servidor. Por ora só sabe onde mora a configuração e quais contas Codex valem.
use super::settings::{self, VoiceSettings};
use crate::accounts::AccountService;
use std::path::{Path, PathBuf};

pub struct VoiceHub {
    home: PathBuf,
    claude_dir: PathBuf,
    /// Contas do teste; `None` pergunta ao catálogo.
    accounts: Option<Vec<String>>,
}

impl Default for VoiceHub {
    fn default() -> Self { Self::new() }
}

impl VoiceHub {
    pub fn new() -> Self {
        let home = std::env::home_dir().unwrap_or_default();
        let claude_dir = settings::claude_dir(&home);
        Self { home, claude_dir, accounts: None }
    }

    pub fn for_test(home: PathBuf, claude_dir: PathBuf, accounts: Option<Vec<String>>) -> Self {
        Self { home, claude_dir, accounts }
    }

    pub fn home(&self) -> &Path { &self.home }

    pub fn claude_dir(&self) -> &Path { &self.claude_dir }

    /// `Account.id` é o nome que as sessões gravam em `codex_account`.
    pub fn account_ids(&self, accounts: &AccountService) -> Vec<String> {
        if let Some(ids) = &self.accounts { return ids.clone(); }
        match accounts.visible_codex_accounts() {
            Ok(list) => list.into_iter().map(|a| a.id).collect(),
            Err(e) => { super::log(format!("voice codex accounts unreadable status={}", e.status)); Vec::new() }
        }
    }

    /// `(ativa, cliente dono)`; sem chamada ainda.
    pub fn call_status(&self) -> (bool, Option<String>) { (false, None) }

    /// ponytail: vazio até existir a chamada viva, que passa a trocar os modelos na hora.
    pub fn apply_settings(&self, _: &VoiceSettings) {}
}
