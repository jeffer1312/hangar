//! Mensagem da conversa (`ChatEvent`, backend/app/models.py:191).
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// `ChatKind` de models.py:9. `Other` guarda o tipo que este crate ainda não conhece: quem lê não
/// quebra e o valor volta igual quando é reenviado.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChatKind {
    UserMsg,
    AssistantMsg,
    ToolUse,
    ToolResult,
    Thinking,
    Notice,
    Other(String),
}

impl ChatKind {
    pub fn as_str(&self) -> &str {
        match self {
            Self::UserMsg => "user_msg",
            Self::AssistantMsg => "assistant_msg",
            Self::ToolUse => "tool_use",
            Self::ToolResult => "tool_result",
            Self::Thinking => "thinking",
            Self::Notice => "notice",
            Self::Other(kind) => kind,
        }
    }
}

impl From<&str> for ChatKind {
    fn from(kind: &str) -> Self {
        match kind {
            "user_msg" => Self::UserMsg,
            "assistant_msg" => Self::AssistantMsg,
            "tool_use" => Self::ToolUse,
            "tool_result" => Self::ToolResult,
            "thinking" => Self::Thinking,
            "notice" => Self::Notice,
            other => Self::Other(other.to_owned()),
        }
    }
}

// Vazio, como o `String` que o desktop usava antes do crate.
impl Default for ChatKind {
    fn default() -> Self {
        Self::Other(String::new())
    }
}

impl PartialEq<&str> for ChatKind {
    fn eq(&self, other: &&str) -> bool {
        self.as_str() == *other
    }
}

impl Serialize for ChatKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ChatKind {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self::from(String::deserialize(deserializer)?.as_str()))
    }
}

/// Um trecho do `structuredPatch` (`patch` do models.py): linhas com prefixo ` `, `-` ou `+`.
/// Posições em `u32`: o Python recusa o que passa disso, para os dois parsers ficarem iguais.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PatchHunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<String>,
}

/// Campos na ordem de models.py: o JSON sai com as chaves na ordem do `model_dump_json`.
/// Contagens sem sinal porque o Python nunca manda negativo, e são os tipos que o desktop já lia.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatEvent {
    pub kind: ChatKind,
    pub id: String,
    pub text: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<Map<String, Value>>,
    pub tool_use_id: Option<String>,
    pub result: Option<String>,
    pub is_error: Option<bool>,
    /// Só em `tool_result` de Edit/Write do Claude: os trechos do `structuredPatch`, com a linha real do arquivo.
    pub patch: Option<Vec<PatchHunk>>,
    /// Só em `tool_result` do Agent lançado em segundo plano: o id do subagente, do campo estruturado.
    pub bg_agent_id: Option<String>,
    pub ts: Option<f64>,
    pub cache_read: Option<u64>,
    pub cache_ttl_s: Option<u64>,
    pub desistiu: Option<bool>,
    pub hook_error: Option<String>,
    pub skill: Option<Map<String, Value>>,
    pub orq: Option<Map<String, Value>>,
    pub queued_delivered: Option<bool>,
    pub queued_ts: Option<f64>,
    pub queued_confirmed: Option<bool>,
    pub image_count: Option<u32>,
    /// Byte logo após a linha do transcript: vira o `id:` do SSE e nunca vai no JSON (`exclude=True`).
    #[serde(skip)]
    pub offset: Option<u64>,
}
