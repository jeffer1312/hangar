//! Abertura do editor, shell separado e terminal externo de uma sessão.
#![forbid(unsafe_code)]

pub mod editor;
pub mod external;
pub mod shell;

use serde_json::Value;

#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub code: &'static str,
    pub msg: String,
    pub params: Value,
}
