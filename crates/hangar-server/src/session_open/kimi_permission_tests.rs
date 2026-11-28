//! O comando público de abertura respeita a escolha e não a reaplica na retomada.
use super::{Account, AccountSource, AgentProvider, PermissionMode, SessionSpec, Surface, SPEC_VERSION, agents};
use serde_json::json;

#[test]
fn kimi_permissions_open_only_passes_flags_to_new_conversations() {
    let home = tempfile::tempdir().unwrap();
    let ctx = agents::AgentContext { home: home.path(), server_env: &Default::default(), runtime_config: &json!({}), read_only: None, windows: false };
    for (mode, flag) in [("yolo", "--yolo"), ("auto", "--auto")] {
        let spec = SessionSpec {
            version: SPEC_VERSION, key: "k".into(), name: "s".into(), cwd: home.path().to_string_lossy().into_owned(), provider: AgentProvider::Kimi,
            surface: Surface::Terminal, account: Account::Default, account_source: AccountSource::Default,
            model: None, effort: None, service_tier: None, permission_mode: PermissionMode::kimi(mode),
            subagent_model: None, customizations: None, jev: false, code_mode: false,
            read_only: false, worktree: None, creator: None, created_at: 1.0, conversation: None,
            name_auto: None, rename_suggestion: None, rename_weak_at: None, rename_dismissed: false, hooks_off: false,
            auto_continue_off: false,
        };
        let pane = agents::pane(&spec, agents::Conversation::New("new"), &ctx).unwrap();
        assert!(pane.command.iter().any(|arg| arg == flag));
        let resumed = agents::pane(&spec, agents::Conversation::Resume("session_11111111-1111-4111-8111-111111111111"), &ctx).unwrap();
        assert!(!resumed.command.iter().any(|arg| arg == flag), "retomar preserva o modo gravado pelo Kimi");
    }
    assert!(PermissionMode::kimi("bypassPermissions").is_none());
    assert!(PermissionMode::kimi("manual").is_none());
}


struct KimiFacts;
impl super::resolve::OpenFacts for KimiFacts {
    fn providers(&self) -> Result<super::resolve::ProviderCatalog, super::OpenError> { Ok(super::resolve::ProviderCatalog { available: vec![AgentProvider::Kimi], connected: vec![AgentProvider::Kimi], ..Default::default() }) }
    fn creator(&self, _: &str) -> Result<Option<super::resolve::CreatorFacts>, super::OpenError> { Ok(Some(super::resolve::CreatorFacts { provider: AgentProvider::Kimi, headless: false, config_dir: Ok(None), permission_mode: None, codex_account: None })) }
    fn defaults(&self) -> super::resolve::ServerDefaults { super::resolve::ServerDefaults { headless: false, jev: false } }
    fn saved_defaults(&self, _: AgentProvider, _: &str, _: &str) -> Result<Option<crate::session_defaults::Defaults>, String> { Ok(Some(crate::session_defaults::Defaults { model: String::new(), effort: String::new(), permission: "auto".into() })) }
    fn session_options(&self) -> Result<Option<crate::session_defaults::SessionOptions>, String> { Ok(None) }
    fn codex_logins(&self) -> Result<Vec<super::resolve::CodexLogin>, super::OpenError> { Ok(vec![]) }
    fn quotas(&self, _: bool) -> Vec<serde_json::Value> { vec![] }
    fn spent_accounts(&self) -> crate::session_write::limit_switch::SpentAccounts { Default::default() }
    fn quota_unknown(&self, _: &str, _: &str) {}
    fn existing_dir(&self, path: &str) -> Option<String> { Some(path.into()) }
    fn config_dirs(&self) -> Result<Vec<String>, super::OpenError> { Ok(vec![]) }
    fn engines(&self) -> Vec<String> { vec![] }
    fn engine_account(&self, _: &str, _: Option<&str>) -> Result<(Option<String>, Option<String>), super::OpenError> { Ok((None, None)) }
    fn supports_fast(&self, _: Option<&str>, _: Option<&str>) -> bool { false }
    fn supports_ultrafast(&self, _: Option<&str>, _: Option<&str>, _: Option<&str>) -> bool { false }
    fn fixed_engine_model(&self, _: &str, _: &str, model: Option<&str>) -> Result<String, super::OpenError> { Ok(model.unwrap_or_default().into()) }
    fn engine_window(&self, _: &str, _: &str, _: Option<&str>) -> Result<Option<u64>, super::OpenError> { Ok(None) }
    fn codex_home(&self, _: &str) -> Result<String, super::OpenError> { Ok("/codex".into()) }
    fn codex_choice(&self, _: Option<&str>, _: Option<&str>, _: Option<&str>, _: Option<super::ServiceTier>) -> Result<(), super::resolve::CodexCatalogError> { Ok(()) }
    fn account_mode(&self, _: Option<&str>) -> Result<Option<String>, String> { Ok(None) }
    fn name_in_use(&self, _: &str) -> Result<bool, super::OpenError> { Ok(false) }
    fn name_has_leftovers(&self, _: &str) -> Result<bool, super::OpenError> { Ok(false) }
    fn transfer_busy(&self, _: &str) -> Result<bool, super::OpenError> { Ok(false) }
    fn real_path(&self, path: &str) -> std::path::PathBuf { path.into() }
    fn new_key(&self) -> String { "a".repeat(32) }
    fn now(&self) -> f64 { 1.0 }
}

#[test]
fn kimi_permissions_saved_default_applies_when_inheritance_has_no_mode() {
    let home = tempfile::tempdir().unwrap();
    for explicit in [None, Some("yolo")] {
        let body = json!({"name":"fixture","cwd":home.path().to_string_lossy(),"provider":"kimi","inherit":true,"creator":"parent","permission_mode":explicit});
        let request = super::SessionOpenRequest::parse(&serde_json::to_vec(&body).unwrap()).unwrap();
        let resolved = super::resolve::resolve_open(&request, &super::resolve::OpenCaller::Owner, &KimiFacts).unwrap();
        assert_eq!(resolved.spec.permission_mode, Some(if explicit.is_some() { PermissionMode::Yolo } else { PermissionMode::Auto }));
    }
}
