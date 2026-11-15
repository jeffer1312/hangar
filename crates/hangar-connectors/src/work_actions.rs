//! Catálogo de mensagens de ação do conector; a execução pertence à sessão do ticket.
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::manifest::render_prompt;
use crate::protocol::ConnectorError;
use crate::work_tracking::TicketSummary;

pub const CAPABILITY: &str = "work_actions";
pub const VERSION: u32 = 1;
pub const LIST: &str = "work_actions.list";

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub description: String,
    pub prompt_template: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct Catalog {
    pub actions: Vec<Action>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolvedAction {
    #[serde(flatten)]
    pub action: Action,
    pub prompt: String,
    pub fingerprint: String,
}

impl Catalog {
    pub fn from_connector(value: Value) -> Result<Self, ConnectorError> {
        let catalog: Self = serde_json::from_value(value)
            .map_err(|error| ConnectorError::protocol(format!("ações fora do contrato: {error}")))?;
        if catalog.actions.len() > 64 { return Err(ConnectorError::protocol("catálogo acima de 64 ações")); }
        let mut ids = HashSet::new();
        for action in &catalog.actions {
            if !crate::manifest::valid_id(&action.id) || !ids.insert(action.id.as_str()) {
                return Err(ConnectorError::protocol("id de ação inválido ou repetido"));
            }
            for (name, value, max, required) in [("label", action.label.as_str(), 200, true),
                ("description", action.description.as_str(), 1000, false), ("prompt_template", action.prompt_template.as_str(), 4096, true)] {
                if value.contains('\0') || value.chars().count() > max || required && value.trim().is_empty() {
                    return Err(ConnectorError::protocol(format!("{name} fora do contrato")));
                }
            }
            if action.stage.as_ref().is_some_and(|id| id.len() > 32 || !crate::manifest::valid_id(id)) {
                return Err(ConnectorError::protocol("etapa sugerida fora do formato"));
            }
        }
        Ok(catalog)
    }
}

impl Action {
    pub fn resolve(self, ticket: &TicketSummary) -> Result<ResolvedAction, ConnectorError> {
        let prompt = render_prompt(&self.prompt_template, ticket)?;
        let bytes = serde_json::to_vec(&(&self, &prompt))
            .map_err(|_| ConnectorError::protocol("não foi possível identificar a ação"))?;
        let fingerprint = hangar_core::credentials::hash(&bytes);
        Ok(ResolvedAction { action: self, prompt, fingerprint })
    }
}
