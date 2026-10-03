//! Prévia ao vivo do bloco em andamento (`PreviewEvent`, backend/app/models.py:304).
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PreviewEvent {
    pub session: String,
    pub text: String,
    pub md: bool,
    pub full: bool,
    pub vivo: bool,
}
