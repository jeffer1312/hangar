//! Falha com o mesmo código dos clientes TypeScript (`packages/core/src/connectAccount.ts` e `syncLogin.ts`):
//! a tela traduz pelo código, e o status HTTP vai junto quando houve resposta.
use std::fmt;

/// O motivo, com o texto de [`Code::as_str`] igual ao `code` do core.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Code {
    /// Pedido sem resposta (rede, prazo, redirecionamento recusado). Nunca é senha errada.
    NetworkFailed,
    /// Nome desconhecido ou senha recusada em todas as máquinas.
    CredentialsMismatch,
    ConnectServiceInsecure,
    ConnectRequestFailed,
    ConnectResponseInvalid,
    ConnectTooManyAttempts,
    ConnectDeviceRequired,
    ConnectDevicePending,
    ConnectDeviceRevoked,
    ConnectDeviceDenied,
    /// A máquina aprovou, mas o token (que sai uma vez só) não veio.
    ConnectTokenLost,
    ConnectApprovalExpired,
    ConnectSecureRandomUnavailable,
    SyncInsecure,
    SyncUnavailable,
    SyncRequestFailed,
    SyncTooManyAttempts,
    SyncVaultUnreadable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    code: Code,
    status: Option<u16>,
    cause: Option<String>,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NetworkFailed => "network_failed",
            Self::CredentialsMismatch => "credentials_mismatch",
            Self::ConnectServiceInsecure => "connect_service_insecure",
            Self::ConnectRequestFailed => "connect_request_failed",
            Self::ConnectResponseInvalid => "connect_response_invalid",
            Self::ConnectTooManyAttempts => "connect_too_many_attempts",
            Self::ConnectDeviceRequired => "connect_device_required",
            Self::ConnectDevicePending => "connect_device_pending",
            Self::ConnectDeviceRevoked => "connect_device_revoked",
            Self::ConnectDeviceDenied => "connect_device_denied",
            Self::ConnectTokenLost => "connect_token_lost",
            Self::ConnectApprovalExpired => "connect_approval_expired",
            Self::ConnectSecureRandomUnavailable => "connect_secure_random_unavailable",
            Self::SyncInsecure => "sync_insecure",
            Self::SyncUnavailable => "sync_unavailable",
            Self::SyncRequestFailed => "sync_request_failed",
            Self::SyncTooManyAttempts => "sync_too_many_attempts",
            Self::SyncVaultUnreadable => "sync_vault_unreadable",
        }
    }
}

impl Error {
    pub fn new(code: Code, status: Option<u16>) -> Self {
        Self { code, status, cause: None }
    }

    /// O que o transporte devolve quando o pedido não teve resposta; `cause` fica só para o log.
    pub fn network(cause: impl Into<String>) -> Self {
        Self { code: Code::NetworkFailed, status: None, cause: Some(cause.into()) }
    }

    pub fn code(&self) -> Code {
        self.code
    }

    pub fn status(&self) -> Option<u16> {
        self.status
    }

    /// Sem URL nem corpo: pode ir ao log.
    pub fn cause(&self) -> Option<&str> {
        self.cause.as_deref()
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.status {
            Some(status) => write!(f, "{} ({status})", self.code.as_str()),
            None => f.write_str(self.code.as_str()),
        }
    }
}

impl std::error::Error for Error {}

pub(crate) fn fail(code: Code) -> Error {
    Error::new(code, None)
}

pub(crate) fn fail_with(code: Code, status: u16) -> Error {
    Error::new(code, Some(status))
}
