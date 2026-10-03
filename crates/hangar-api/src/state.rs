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

/// `state` e `codex_mode` ficam em texto: o hangar-server só repassa este evento, e valor novo do
/// Python passa adiante igual. Tudo tem padrão porque o desktop lê o estado antes do primeiro evento.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StateEvent {
    pub session: String,
    pub state: String,
    pub codex_mode: Option<String>,
    pub codex_question: Option<Map<String, Value>>,
    pub codex_buffering: bool,
    pub claude_permission_mode: Option<String>,
    pub claude_previous_non_plan: Option<String>,
    pub claude_plan_pending: Option<Map<String, Value>>,
    pub label: Option<String>,
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
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
