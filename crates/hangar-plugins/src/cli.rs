//! Fronteira do processo `claude`: o servidor implementa, os testes trocam por um falso.
use std::future::Future;
use std::time::Duration;

/// Teto de `list`, `details`, `enable`, `disable`, `uninstall` e `marketplace remove`.
pub(crate) const SHORT: Duration = Duration::from_secs(20);
/// Teto de `list --available`, que consulta os catálogos.
pub(crate) const AVAILABLE: Duration = Duration::from_secs(60);
/// Teto do que baixa: `install`, `update`, `marketplace add|update` e o `check`.
pub(crate) const DOWNLOAD: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliOutput {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliError {
    /// Sem o executável `claude`.
    Missing,
    Timeout,
    Spawn(String),
}

pub trait CliRunner: Send + Sync {
    /// Roda `claude <args>` sem shell. `args` já começa em `plugin`.
    fn run(&self, args: &[&str], timeout: Duration) -> impl Future<Output = Result<CliOutput, CliError>> + Send;
}
