use serde_json::json;

use crate::{
    AgentProvider, CodexCatalogError, CodexLogin, CreatorFacts, Defaults, OpenCaller, OpenError,
    OpenFacts, PermissionMode, ProviderCatalog, ServerDefaults, ServiceTier, SessionOpenRequest,
    SessionOptions, SpentAccounts, resolve_open,
};

struct KimiFacts;
impl OpenFacts for KimiFacts {
    fn providers(&self) -> Result<ProviderCatalog, OpenError> {
        Ok(ProviderCatalog {
            available: vec![AgentProvider::Kimi],
            connected: vec![AgentProvider::Kimi],
            ..Default::default()
        })
    }
    fn creator(&self, _: &str) -> Result<Option<CreatorFacts>, OpenError> {
        Ok(Some(CreatorFacts {
            provider: AgentProvider::Kimi,
            headless: false,
            config_dir: Ok(None),
            permission_mode: None,
            codex_account: None,
        }))
    }
    fn defaults(&self) -> ServerDefaults {
        ServerDefaults {
            headless: false,
            jev: false,
        }
    }
    fn saved_defaults(
        &self,
        _: AgentProvider,
        _: &str,
        _: &str,
    ) -> Result<Option<Defaults>, String> {
        Ok(Some(Defaults {
            model: String::new(),
            effort: String::new(),
            permission: "auto".into(),
        }))
    }
    fn session_options(&self) -> Result<Option<SessionOptions>, String> {
        Ok(None)
    }
    fn codex_logins(&self) -> Result<Vec<CodexLogin>, OpenError> {
        Ok(vec![])
    }
    fn quotas(&self, _: bool) -> Vec<serde_json::Value> {
        vec![]
    }
    fn spent_accounts(&self) -> SpentAccounts {
        Default::default()
    }
    fn quota_unknown(&self, _: &str, _: &str) {}
    fn existing_dir(&self, path: &str) -> Option<String> {
        Some(path.into())
    }
    fn config_dirs(&self) -> Result<Vec<String>, OpenError> {
        Ok(vec![])
    }
    fn engines(&self) -> Vec<String> {
        vec![]
    }
    fn engine_account(
        &self,
        _: &str,
        _: Option<&str>,
    ) -> Result<(Option<String>, Option<String>), OpenError> {
        Ok((None, None))
    }
    fn supports_fast(&self, _: Option<&str>, _: Option<&str>) -> bool {
        false
    }
    fn supports_ultrafast(&self, _: Option<&str>, _: Option<&str>, _: Option<&str>) -> bool {
        false
    }
    fn fixed_engine_model(
        &self,
        _: &str,
        _: &str,
        model: Option<&str>,
    ) -> Result<String, OpenError> {
        Ok(model.unwrap_or_default().into())
    }
    fn engine_window(&self, _: &str, _: &str, _: Option<&str>) -> Result<Option<u64>, OpenError> {
        Ok(None)
    }
    fn codex_home(&self, _: &str) -> Result<String, OpenError> {
        Ok("/codex".into())
    }
    fn codex_choice(
        &self,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<&str>,
        _: Option<ServiceTier>,
    ) -> Result<(), CodexCatalogError> {
        Ok(())
    }
    fn account_mode(&self, _: Option<&str>) -> Result<Option<String>, String> {
        Ok(None)
    }
    fn name_in_use(&self, _: &str) -> Result<bool, OpenError> {
        Ok(false)
    }
    fn name_claimed(&self, _: &str, _: bool) -> bool {
        false
    }
    fn name_has_leftovers(&self, _: &str) -> Result<bool, OpenError> {
        Ok(false)
    }
    fn transfer_busy(&self, _: &str) -> Result<bool, OpenError> {
        Ok(false)
    }
    fn real_path(&self, path: &str) -> std::path::PathBuf {
        path.into()
    }
    fn new_key(&self) -> String {
        "a".repeat(32)
    }
    fn now(&self) -> f64 {
        1.0
    }
}

#[test]
fn kimi_permissions_saved_default_applies_when_inheritance_has_no_mode() {
    for explicit in [None, Some("yolo")] {
        let body = json!({"name":"fixture","cwd":"/fixture","provider":"kimi","inherit":true,"creator":"parent","permission_mode":explicit});
        let request = SessionOpenRequest::parse(&serde_json::to_vec(&body).unwrap()).unwrap();
        let resolved = resolve_open(&request, &OpenCaller::Owner, &KimiFacts).unwrap();
        assert_eq!(
            resolved.spec.permission_mode,
            Some(if explicit.is_some() {
                PermissionMode::Yolo
            } else {
                PermissionMode::Auto
            })
        );
    }
}
