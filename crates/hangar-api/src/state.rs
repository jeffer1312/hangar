//! Estado ao vivo da sessão (`ShellVivo` e `StateEvent`, backend/app/models.py:247 e 254).
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Comando de fundo que a sessão deixou rodando.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ShellVivo {
    pub pid: i64,
    #[serde(default)]
    pub cmd: String,
    pub desde: Option<f64>,
}

/// Comando que o Codex ainda roda; `background` = o turno que o abriu já acabou.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodexCommand {
    pub id: String,
    pub process_id: String,
    pub command: String,
    pub started_at: f64,
    pub background: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CodexSubagent {
    pub agent_path: Option<String>,
    pub state: Option<String>,
    pub ended_at: Option<f64>,
    pub can_message: bool,
    pub message_blocked: Option<String>,
}

/// `state` e `codex_mode` ficam em texto: o hangar-server só repassa este evento, e valor novo do
/// Python passa adiante igual. Tudo tem padrão porque o desktop lê o estado antes do primeiro evento.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateEvent {
    pub session: String,
    pub state: String,
    pub codex_mode: Option<String>,
    pub codex_service_tier: Option<String>,
    pub codex_question: Option<Map<String, Value>>,
    pub codex_buffering: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub codex_commands: Vec<CodexCommand>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub codex_subagents: Option<std::collections::BTreeMap<String, CodexSubagent>>,
    pub claude_permission_mode: Option<String>,
    pub claude_previous_non_plan: Option<String>,
    pub claude_plan_pending: Option<Map<String, Value>>,
    pub label: Option<String>,
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_id: Option<String>,
    pub status_line: Option<String>,
    pub overlay: bool,
    pub login: bool,
    pub limited: bool,
    pub limit_reset: Option<String>,
    pub loop_status: Option<String>,
    pub loop_iter: Option<u32>,
    pub loop_max: Option<u32>,
    pub problema: Option<String>,
    pub problema_detalhe: Option<String>,
    pub headless: bool,
    pub recarregar_motivo: Option<String>,
    pub shells: Vec<ShellVivo>,
}

#[cfg(test)]
mod tests {
    use super::StateEvent;

    #[test]
    fn codex_service_tier_survives_state_round_trip() {
        let state: StateEvent = serde_json::from_value(serde_json::json!({
            "session": "codex", "state": "idle", "codex_service_tier": "priority"
        })).unwrap();
        assert_eq!(state.codex_service_tier.as_deref(), Some("priority"));
        assert_eq!(serde_json::to_value(state).unwrap()["codex_service_tier"], "priority");
        let old: StateEvent = serde_json::from_value(serde_json::json!({"state": "idle"})).unwrap();
        assert!(old.codex_service_tier.is_none());
    }
}
