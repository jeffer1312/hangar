//! A borda distingue rejeição de valor de falha interna, sem interpretar a mensagem.
#[derive(Debug)]
pub struct RuntimeError {
    pub kind: &'static str,
    pub message: String,
    pub os_error: Option<i32>,
    pub operation: Option<StorageOperation>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageOperation { Read, Write }
impl RuntimeError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), os_error: None, operation: None }
    }
    pub fn storage(mut self, operation: StorageOperation) -> Self {
        self.operation = Some(operation);
        self
    }
    pub fn io(error: &std::io::Error, operation: StorageOperation) -> Self {
        let mut failure = Self::new("OSError", error.to_string()).storage(operation);
        failure.os_error = error.raw_os_error();
        failure
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
