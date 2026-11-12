//! Comandos de execução da sessão; mantém as preferências e o socket dedicado dos runners.
#![forbid(unsafe_code)]
mod runner;

pub use runner::{RunInfo, Runner, slug};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub code: Option<&'static str>,
    pub detail: String,
}
impl Error {
    pub fn new(status: u16, detail: impl Into<String>) -> Self {
        Self {
            status,
            code: None,
            detail: detail.into(),
        }
    }
    pub fn coded(status: u16, code: &'static str, detail: impl Into<String>) -> Self {
        Self {
            code: Some(code),
            ..Self::new(status, detail)
        }
    }
}
impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Self::new(500, e.to_string())
    }
}
#[derive(Clone)]
pub struct Service {
    pub preferences: PathBuf,
    pub program: Vec<String>,
    pub scope: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub windows: bool,
}
