use serde::Deserialize;
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SessionInfo {
    pub name: String,
    pub cwd: Option<String>,
    pub jsonl: Option<String>,
    #[serde(default)] pub provider: String,
    #[serde(default)] pub headless: bool,
    #[serde(default)] pub state: String,
    pub tracked: Option<bool>,
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
    /// Perguntas esperando resposta fora do terminal (as assíncronas do Codex); a aba mostra "? N".
    #[serde(default)] pub pending_questions: u32,
    pub problema: Option<String>,
    pub label: Option<String>,
    pub last_activity: Option<f64>,
    pub branch: Option<String>,
    pub git_added: Option<i64>,
    pub git_removed: Option<i64>,
    pub git_dirty: Option<i64>,
    pub status_line: Option<String>,
    pub loop_status: Option<String>,
    pub loop_iter: Option<u32>,
    pub loop_max: Option<u32>,
    pub limited: Option<bool>,
    pub limit_reset: Option<String>,
}

impl SessionInfo {
    pub fn readable(&self) -> bool { self.tracked != Some(false) && self.jsonl.is_some() }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct ChatEvent {
    pub kind: String,
    pub id: String,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Value>,
    pub tool_use_id: Option<String>,
    pub result: Option<String>,
    pub is_error: Option<bool>,
    pub ts: Option<f64>,
    pub queued_delivered: Option<bool>,
    pub queued_confirmed: Option<bool>,
    pub queued_ts: Option<f64>,
    pub desistiu: Option<bool>,
    pub hook_error: Option<String>,
    pub image_count: Option<u32>,
}

impl ChatEvent {
    pub fn queued(&self) -> bool { self.id.starts_with("queued-") }
    pub fn body(&self) -> String {
        self.text.clone().or_else(|| self.result.clone()).unwrap_or_else(|| {
            self.tool_input.as_ref().map(|v| serde_json::to_string_pretty(v).unwrap_or_default()).unwrap_or_default()
        })
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SessionState {
    #[serde(default)] pub state: String,
    pub label: Option<String>,
    pub question: Option<String>,
    pub options: Option<Vec<String>>,
    pub problema: Option<String>,
    pub problema_detalhe: Option<String>,
    pub login: Option<bool>,
    pub claude_plan_pending: Option<PlanPending>,
    pub status_line: Option<String>,
    pub codex_mode: Option<String>,
    pub claude_permission_mode: Option<String>,
    pub claude_previous_non_plan: Option<String>,
    pub recarregar_motivo: Option<String>,
    pub limited: Option<bool>,
    pub limit_reset: Option<String>,
    pub loop_status: Option<String>,
    pub loop_iter: Option<u32>,
    pub loop_max: Option<u32>,
}

/// Evento SSE `stats`: só turns/steps/in/out são garantidos; o resto aparece quando o backend mede.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Stats {
    #[serde(default)] pub turns: u64,
    #[serde(default)] pub steps: u64,
    #[serde(default)] pub in_tok: u64,
    #[serde(default)] pub out_tok: u64,
    pub llm_ms: Option<f64>,
    pub tool_ms: Option<f64>,
    pub tok_s: Option<f64>,
    pub cache_pct: Option<f64>,
    pub ttft_ms: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct PlanPending {
    #[serde(default)] pub plan: String,
    pub path: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct AskOption {
    #[serde(default)] pub label: String,
    #[serde(default)] pub description: String,
    #[serde(default)] pub preview: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AskItem {
    pub id: Option<String>,
    #[serde(default)] pub header: String,
    #[serde(default)] pub question: String,
    #[serde(default)] pub multi_select: bool,
    #[serde(default)] pub options: Vec<AskOption>,
    #[serde(default)] pub is_other: bool,
    #[serde(default)] pub is_secret: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct AskPayload {
    pub provider: Option<String>,
    // Valor cru: o Codex recusa id com outro tipo JSON (número × texto).
    pub request_id: Option<Value>,
    #[serde(default)] pub questions: Vec<AskItem>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Preview {
    #[serde(default)] pub text: String,
    #[serde(default)] pub md: bool,
    #[serde(default)] pub full: bool,
    #[serde(default)] pub vivo: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Delivery {
    pub ok: bool,
    #[serde(default)] pub delivered: bool,
    #[serde(default)] pub steered: bool,
    #[serde(default)] pub native: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CommandInfo {
    pub name: String,
    #[serde(default)] pub display: String,
    pub description: Option<String>,
    pub argument_hint: Option<String>,
    #[serde(default)] pub source: String,
    #[serde(default)] pub destructive: bool,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
pub struct Uploaded {
    pub path: String,
    #[serde(default)] pub frames: Vec<String>,
    pub transcript: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct UploadFile {
    pub filename: String,
    #[serde(default)] pub size: u64,
    #[serde(default)] pub mtime: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Steered {
    #[serde(default)] pub promoted: bool,
    #[serde(default)] pub confirmed: u32,
    #[serde(default)] pub queued_ids: Vec<String>,
}
