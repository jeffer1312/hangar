//! Identidade do terminal e resultado da entrega usados pelos condutores.
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TerminalBinding {
    pub name: String,
    pub pane: String,
    pub conversation: String,
    pub generation: u64,
    pub created: u64,
    pub mux_argv: Vec<String>,
    pub windows: bool,
    pub clipboard_lock_path: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Disposition { Accepted, Deferred, Rejected, Unknown }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStage { Validate, Identity, Ready, Composer, Native, Plugin, Write, InputProof, Submit, SubmitProof, Cleanup, Control, Select, Answer, Steer }
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Cleanup { NotNeeded, Proved, Unproved }
/// Onde ficou o rascunho do dono que o escritor guardou para entregar.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DraftOutcome { Returned, Stashed, Unverified }
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeliveryResult {
    pub disposition: Disposition,
    pub stage: DeliveryStage,
    pub cleanup: Cleanup,
    pub native: bool,
    pub message_id: Option<String>,
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft: Option<DraftOutcome>,
    /// Aceita com a sessão parada: o turno começou com ela, não ficou na fila interna do Claude.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub consumed: bool,
}
impl DeliveryResult {
    pub fn new(disposition: Disposition, stage: DeliveryStage, code: &str) -> Self {
        Self { disposition, stage, cleanup: Cleanup::NotNeeded, native: false, message_id: None, code: code.into(), draft: None, consumed: false }
    }
}
