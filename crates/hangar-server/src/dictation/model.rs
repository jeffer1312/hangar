use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OrganizationMode {
    #[default]
    None,
    Harness,
    ExternalApi,
}

impl OrganizationMode {
    pub fn saved(value: &str) -> Self {
        match value {
            "harness" => Self::Harness,
            "external_api" => Self::ExternalApi,
            _ => Self::None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum DictationStyle {
    #[serde(rename = "limpar")]
    Clean,
    #[serde(rename = "prosa")]
    #[default]
    Prose,
    #[serde(rename = "briefing")]
    Briefing,
}
impl DictationStyle {
    pub fn id(self) -> &'static str {
        match self {
            Self::Clean => "limpar",
            Self::Prose => "prosa",
            Self::Briefing => "briefing",
        }
    }
    pub fn effective(self, raw: &str) -> Self {
        if self == Self::Briefing && raw.split_whitespace().count() < 40 {
            Self::Prose
        } else {
            self
        }
    }
    pub fn timeout(self) -> std::time::Duration {
        std::time::Duration::from_secs(match self {
            Self::Clean => 60,
            Self::Prose => 90,
            Self::Briefing => 120,
        })
    }
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct OrganizationConfig {
    pub dictation_organization_mode: String,
    pub dictation_claude_model: String,
    pub dictation_codex_model: String,
    pub dictation_include_recent_messages: bool,
    pub ditado_estilo: String,
    pub llm_base_url: String,
    pub llm_api_key: String,
    pub llm_model: String,
    pub llm_reasoning_effort: String,
    pub llm_briefing_base_url: String,
    pub llm_briefing_api_key: String,
    pub llm_briefing_model: String,
}

#[derive(Clone, Default, Deserialize)]
#[serde(default)]
pub struct OrganizationRequest {
    #[serde(alias = "texto")]
    pub raw: String,
    #[serde(alias = "organization_mode")]
    pub mode: Option<OrganizationMode>,
    #[serde(alias = "estilo")]
    pub style: Option<DictationStyle>,
    pub session: Option<String>,
    pub generation: Option<String>,
    pub model: Option<String>,
    pub account: Option<String>,
    pub include_recent_messages: Option<bool>,
    pub recent_messages: Option<Vec<ReferenceMessage>>,
    #[serde(default = "allow_harness")]
    pub harness_allowed: bool,
}
fn allow_harness() -> bool {
    true
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ReferenceMessage {
    pub role: String,
    pub text: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct OrganizationResult {
    pub text: String,
    pub raw: String,
    pub organization_mode: OrganizationMode,
    pub estilo_aplicado: String,
    pub aviso: Option<String>,
    pub organization_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recent_messages: Option<Vec<ReferenceMessage>>,
}
impl OrganizationResult {
    pub fn original(raw: String, mode: OrganizationMode) -> Self {
        Self {
            text: raw.clone(),
            raw,
            organization_mode: mode,
            estilo_aplicado: "cru".into(),
            aviso: None,
            organization_code: None,
            recent_messages: None,
        }
    }
    pub fn failed(mut self, code: &str, detail: &str) -> Self {
        self.organization_code = Some(code.into());
        self.aviso = Some(detail.into());
        self
    }
}
