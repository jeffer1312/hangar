//! Contratos de arquivos e falhas, comuns aos dois clientes Rust.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceError {
    pub status: u16,
    pub detail: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

impl std::fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "workspace error {}", self.status)
    }
}
impl std::error::Error for WorkspaceError {}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContent {
    pub path: String,
    pub text: String,
    pub size: u64,
    pub truncated: bool,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    pub changed: Option<String>,
    pub add: u64,
    pub del: u64,
}
