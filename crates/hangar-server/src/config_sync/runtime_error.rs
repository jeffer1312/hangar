//! A borda distingue rejeição de valor de falha interna, sem interpretar a mensagem.
#[derive(Debug)]
pub struct RuntimeError {
    pub kind: &'static str,
    pub message: String,
}
impl RuntimeError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
    pub fn is_validation(&self) -> bool {
        matches!(self.kind, "ValueError" | "UnicodeEncodeError")
    }
}
impl From<String> for RuntimeError {
    fn from(message: String) -> Self { Self::new("ValueError", message) }
}
impl From<&str> for RuntimeError {
    fn from(message: &str) -> Self { Self::new("ValueError", message) }
}
