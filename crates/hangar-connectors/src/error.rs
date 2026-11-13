//! Erros de administração (cadastro, catálogo) e de chamada pelo host.
use crate::protocol::{ConnectorError, ErrorCode};

/// Recusa do cadastro ou do catálogo. `code` é estável e vai às respostas HTTP e ao CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminError {
    code: &'static str,
    pub message: String,
}

impl AdminError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
    pub fn invalid(message: impl Into<String>) -> Self { Self::new("manifest_invalid", message) }
    pub fn not_found(id: &str) -> Self { Self::new("connector_not_found", format!("conector {id:?} não cadastrado")) }
    pub fn io(action: &str, error: &std::io::Error) -> Self {
        Self::new("registry_unavailable", format!("{action}: {:?}", error.kind()))
    }
    pub fn code(&self) -> &'static str { self.code }
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.message) }
}

impl std::error::Error for AdminError {}

/// Falha de uma leitura pelo host: o conector não pode ser usado, ou respondeu com erro.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallError {
    Admin(AdminError),
    Disabled(String),
    Connector(ConnectorError),
}

impl CallError {
    pub fn code(&self) -> &'static str {
        match self {
            CallError::Admin(error) => error.code(),
            CallError::Disabled(_) => "connector_disabled",
            CallError::Connector(error) => error.code.as_str(),
        }
    }

    pub fn message(&self) -> String {
        match self {
            CallError::Admin(error) => error.message.clone(),
            CallError::Disabled(id) => format!("conector {id:?} está desligado"),
            CallError::Connector(error) => error.message.clone(),
        }
    }

    pub fn connector(code: ErrorCode, message: impl Into<String>) -> Self { CallError::Connector(ConnectorError::new(code, message)) }
}

impl From<AdminError> for CallError { fn from(error: AdminError) -> Self { CallError::Admin(error) } }
impl From<ConnectorError> for CallError { fn from(error: ConnectorError) -> Self { CallError::Connector(error) } }

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code(), self.message()) }
}

impl std::error::Error for CallError {}
