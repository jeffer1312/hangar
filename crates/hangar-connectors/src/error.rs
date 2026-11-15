//! Erros de administração (cadastro, catálogo) e de chamada pelo host.
use crate::protocol::{ConnectorError, ErrorCode};

/// Tipo de recusa do cadastro ou do catálogo; `as_str` é o código estável das respostas e do CLI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdminKind {
    ManifestInvalid,
    ManifestUnreadable,
    UnsupportedVersion,
    UnsupportedCapability,
    ConnectorExists,
    ConnectorNotFound,
    RegistryInvalid,
    RegistryUnavailable,
}

impl AdminKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AdminKind::ManifestInvalid => "manifest_invalid",
            AdminKind::ManifestUnreadable => "manifest_unreadable",
            AdminKind::UnsupportedVersion => "unsupported_version",
            AdminKind::UnsupportedCapability => "unsupported_capability",
            AdminKind::ConnectorExists => "connector_exists",
            AdminKind::ConnectorNotFound => "connector_not_found",
            AdminKind::RegistryInvalid => "registry_invalid",
            AdminKind::RegistryUnavailable => "registry_unavailable",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AdminError {
    pub kind: AdminKind,
    pub message: String,
}

impl AdminError {
    pub fn new(kind: AdminKind, message: impl Into<String>) -> Self { Self { kind, message: message.into() } }
    pub fn invalid(message: impl Into<String>) -> Self { Self::new(AdminKind::ManifestInvalid, message) }
    pub fn not_found(id: &str) -> Self { Self::new(AdminKind::ConnectorNotFound, format!("conector {id:?} não cadastrado")) }
    pub fn io(action: &str, error: &std::io::Error) -> Self {
        Self::new(AdminKind::RegistryUnavailable, format!("{action}: {:?}", error.kind()))
    }
    pub fn code(&self) -> &'static str { self.kind.as_str() }
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code(), self.message) }
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
