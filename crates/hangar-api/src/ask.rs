//! Pergunta do AskUserQuestion do Claude com terminal (`AskQuestion`, backend/app/models.py:356-372).
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskOption {
    pub label: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub preview: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskQuestionItem {
    pub header: String,
    pub question: String,
    #[serde(rename = "multiSelect", default)]
    pub multi_select: bool,
    pub options: Vec<AskOption>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AskQuestion {
    pub questions: Vec<AskQuestionItem>,
}
