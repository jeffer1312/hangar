//! O catálogo HTTP e a abertura compartilham descoberta, logins e preferência do provedor.
use crate::{
    accounts::AccountService,
    provider_discovery::Providers,
    session_open::{AgentProvider, OpenError, resolve::ProviderCatalog},
};
use serde_json::{Value, json};

pub struct Snapshot {
    pub probes: Providers,
    pub selection: ProviderCatalog,
}
impl Snapshot {
    pub fn from_states(probes: Providers, logins: &[Option<bool>], engines: bool,
        codex: &[String], remembered: Option<String>) -> Self {
        let available = AgentProvider::ALL.into_iter()
            .filter(|provider| probes.get(provider.name()).is_some_and(|probe|probe.available))
            .collect();
        let selection = crate::session_open::facts::provider_catalog(available,logins,engines,codex,remembered);
        Self { probes, selection }
    }
    pub fn response(&self) -> Value {
        let chosen = crate::session_open::resolve::choose_provider(&self.selection);
        Value::Object(self.probes.iter().map(|(name,probe)| (name.clone(),json!({
            "disponivel":probe.available,"motivo":probe.reason,"default":name==chosen.name(),
        }))).collect())
    }
}

pub async fn read(service: &AccountService) -> Result<Snapshot,OpenError> {
    let probes = crate::provider_discovery::providers(&service.env).await
        .map_err(|error|OpenError::new(503,"erro_provedores_indisponiveis",error))?;
    let available = |name:&str| probes.get(name).is_some_and(|probe|probe.available);
    let mut logins = Vec::new();
    if available("claude") {
        match service.claude_accounts() {
            Ok(accounts) => {
                for (_,account) in accounts {
                    logins.push(service.read_claude_auth(&account).await["loggedIn"].as_bool());
                }
            }
            Err(error) => tracing::warn!(code=error.code,"catálogo de provedores: login Claude ilegível"),
        }
    }
    let codex = if available("codex") {
        let accounts = service.visible_codex_accounts()
            .map_err(|error|OpenError::new(error.status,error.code,error.message).with_params(error.params))?;
        futures_util::future::join_all(accounts.iter().map(|account|service.read_codex_auth_fast(account)))
            .await.into_iter().map(|state|state["status"].as_str().unwrap_or("unavailable").to_owned()).collect()
    } else { Vec::new() };
    let runtime = crate::runtime::codex_launch::runtime_config(&service.env);
    let remembered = runtime["last_session_provider"].as_str().map(str::to_owned);
    Ok(Snapshot::from_states(probes,&logins,!super::store::state(&service.env).0.is_empty(),&codex,remembered))
}
