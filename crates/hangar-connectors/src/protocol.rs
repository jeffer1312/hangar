//! Transporte entre o host e o conector: JSON-RPC 2.0, uma mensagem por linha no stdin/stdout.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: u32 = 1;
pub const MANIFEST_VERSION: u32 = 1;
/// Teto de uma mensagem, nos dois sentidos. Maior que isso é recusado, nunca cortado.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024 * 1024;

pub const INITIALIZE: &str = "initialize";
pub const SHUTDOWN: &str = "shutdown";

/// Capacidades conhecidas e a versão de cada uma que o host entende.
pub const CAPABILITIES: [(&str, u32); 1] = [(crate::work_tracking::CAPABILITY, crate::work_tracking::VERSION)];

/// Erros públicos do contrato. O nome em `snake_case` é o que viaja em `error.data.code` e nas
/// respostas HTTP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidParams,
    UnsupportedCapability,
    UnsupportedVersion,
    InvalidCursor,
    AuthRequired,
    PermissionDenied,
    NotFound,
    Unavailable,
    Timeout,
    ProtocolError,
    PayloadTooLarge,
}

impl ErrorCode {
    pub const ALL: [ErrorCode; 11] = [
        ErrorCode::InvalidParams, ErrorCode::UnsupportedCapability, ErrorCode::UnsupportedVersion,
        ErrorCode::InvalidCursor, ErrorCode::AuthRequired, ErrorCode::PermissionDenied, ErrorCode::NotFound,
        ErrorCode::Unavailable, ErrorCode::Timeout, ErrorCode::ProtocolError, ErrorCode::PayloadTooLarge,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::InvalidParams => "invalid_params",
            ErrorCode::UnsupportedCapability => "unsupported_capability",
            ErrorCode::UnsupportedVersion => "unsupported_version",
            ErrorCode::InvalidCursor => "invalid_cursor",
            ErrorCode::AuthRequired => "auth_required",
            ErrorCode::PermissionDenied => "permission_denied",
            ErrorCode::NotFound => "not_found",
            ErrorCode::Unavailable => "unavailable",
            ErrorCode::Timeout => "timeout",
            ErrorCode::ProtocolError => "protocol_error",
            ErrorCode::PayloadTooLarge => "payload_too_large",
        }
    }

    pub fn parse(text: &str) -> Option<ErrorCode> { ErrorCode::ALL.into_iter().find(|code| code.as_str() == text) }

    /// Código numérico do JSON-RPC para quem escreve o conector; o tipado vai em `data.code`.
    pub fn rpc_code(self) -> i64 {
        match self {
            ErrorCode::InvalidParams | ErrorCode::InvalidCursor => -32602,
            ErrorCode::UnsupportedCapability => -32601,
            _ => -32000,
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
}

/// Erro de uma chamada ao conector. A mensagem vai a quem pediu; o diagnóstico registra só o código.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConnectorError {
    pub code: ErrorCode,
    pub message: String,
}

impl ConnectorError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self { Self { code, message: message.into() } }
    pub fn protocol(message: impl Into<String>) -> Self { Self::new(ErrorCode::ProtocolError, message) }
    pub fn invalid_params(message: impl Into<String>) -> Self { Self::new(ErrorCode::InvalidParams, message) }

    /// Objeto `error` do JSON-RPC, como o conector deve responder.
    pub fn to_rpc(&self) -> Value {
        json!({"code": self.code.rpc_code(), "message": self.message, "data": {"code": self.code.as_str()}})
    }

    /// Lê o `error` que o conector mandou. Sem `data.code` conhecido, vale o código numérico padrão.
    pub fn from_rpc(error: &Value) -> Self {
        let message = error.get("message").and_then(Value::as_str).unwrap_or("").chars().take(500).collect::<String>();
        let typed = error.pointer("/data/code").and_then(Value::as_str).and_then(ErrorCode::parse);
        let code = typed.unwrap_or(match error.get("code").and_then(Value::as_i64) {
            Some(-32602) => ErrorCode::InvalidParams,
            Some(-32601) => ErrorCode::UnsupportedCapability,
            _ => ErrorCode::ProtocolError,
        });
        Self::new(code, if message.is_empty() { code.as_str().to_owned() } else { message })
    }
}

impl std::fmt::Display for ConnectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.message) }
}

impl std::error::Error for ConnectorError {}

/// Pedido do host ao conector.
pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Resposta de sucesso, para quem escreve conector em Rust.
pub fn success(id: &Value, result: Value) -> Value { json!({"jsonrpc": "2.0", "id": id, "result": result}) }

/// Resposta de erro, para quem escreve conector em Rust.
pub fn failure(id: &Value, error: &ConnectorError) -> Value { json!({"jsonrpc": "2.0", "id": id, "error": error.to_rpc()}) }

/// Parâmetros de `initialize` mandados pelo host.
pub fn initialize_params() -> Value {
    json!({"protocol_version": PROTOCOL_VERSION, "host": {"name": "hangar"}})
}

/// Resposta de `initialize`: o conector confirma versão, identidade e capacidades.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct InitializeResult {
    pub protocol_version: u32,
    pub id: String,
    pub capabilities: std::collections::BTreeMap<String, u32>,
}
