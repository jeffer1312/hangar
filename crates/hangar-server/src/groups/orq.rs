//! Fatos da orquestração lidos no cofre.
use serde_json::{Value, json};

use super::service::{BoxFuture, OrqFacts, OrqPhase, PromoteError};
use crate::routes::AppState;

#[derive(Clone)]
pub struct OrqVault {
    home: std::path::PathBuf,
    pair: std::path::PathBuf,
    mux: String,
}

impl OrqVault {
    pub fn new() -> Self {
        let env = crate::accounts::environment::AccountEnvironment::capture();
        Self {
            home: env.home,
            pair: env.claude_base.join(".hangar-pair"),
            mux: if cfg!(windows) { "psmux" } else { "tmux" }.into(),
        }
    }
    pub fn with_paths(mut self, home: std::path::PathBuf, pair: std::path::PathBuf) -> Self {
        self.home = home;
        self.pair = pair;
        self
    }

    pub fn with_mux(mut self, mux: String) -> Self {
        self.mux = mux;
        self
    }

    pub fn from_state(st: &AppState) -> Self {
        Self::new()
        .with_mux(st.list.env().capture_program.to_string_lossy().into_owned())
        .with_paths(
            st.accounts.env.home.clone(),
            st.groups
                .as_ref()
                .map(|g| g.pair_root().to_owned())
                .unwrap_or_else(|| st.accounts.env.claude_base.join(".hangar-pair")),
        )
    }
}

impl OrqFacts for OrqVault {
    fn phase<'a>(&'a self, gid: &'a str) -> BoxFuture<'a, OrqPhase> {
        Box::pin(async move {
            let home = self.home.clone();
            let gid = gid.to_owned();
            match tokio::task::spawn_blocking(move || {
                crate::orchestration::runs::group_phase(&home, &gid)
            })
            .await
            {
                Ok(Some(phase)) if phase == "live" => OrqPhase::Live,
                Ok(Some(phase)) if phase == "ended" => OrqPhase::Ended,
                Ok(None) => OrqPhase::NotStarted,
                _ => OrqPhase::Unknown,
            }
        })
    }
    fn promote<'a>(
        &'a self,
        name: &'a str,
        gid: &'a str,
    ) -> BoxFuture<'a, Result<(), PromoteError>> {
        Box::pin(async move {
            let (home, pair, name, gid, mux) = (
                self.home.clone(),
                self.pair.clone(),
                name.to_owned(),
                gid.to_owned(),
                self.mux.clone(),
            );
            tokio::task::spawn_blocking(move || {
                let _lock = crate::orchestration::context::WRITE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                use crate::orchestration::context::PromotionError;
                crate::orchestration::context::promote_classified(&home, &pair, &name, &gid, None, &mux)
                    .map_err(|error| match error {
                        PromotionError::IdentityUnavailable(message) => PromoteError::IdentityUnavailable(message),
                        PromotionError::Conflict(message) | PromotionError::Changed(message) => PromoteError::Conflict(message),
                    })
            })
            .await
            .map_err(|_| PromoteError::Unavailable("groups_orq_promote_failed".into()))?
        })
    }
}

/// Envelope de lista que não respondeu (`erro_lista_indisponivel`): nunca "ninguém".
pub(crate) fn list_unavailable(code: &str) -> Value {
    json!({"code": "erro_lista_indisponivel", "params": {"detalhe": code},
        "msg": format!("a lista de sessões está indisponível ({code}), não vazia")})
}

/// Os de `names` que são a linha do orquestrador (a recusa `_recusa_orq` do Python).
pub async fn is_orchestrator(st: &AppState, names: &[String]) -> Result<Vec<String>, Value> {
    let home = st.accounts.env.home.clone();
    let names = names.to_vec();
    tokio::task::spawn_blocking(move || {
        crate::orchestration::runs::active(&home, crate::orchestration::runs::now()).map(|rows| {
            names
                .into_iter()
                .filter(|name| rows.iter().any(|row| row["name"] == name.as_str()))
                .collect()
        })
    })
    .await
    .map_err(|_| list_unavailable("groups_orq_names_failed"))?
    .map_err(|_| list_unavailable("groups_orq_names_unavailable"))
}
