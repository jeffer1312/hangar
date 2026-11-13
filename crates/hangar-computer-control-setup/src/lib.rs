//! Computer Use (MCP `hangar-computer-control`): caminhos, leitura dos `.claude.json`, alvos, SSH e
//! instalação/atualização do binário.
//! Nada aqui lê `$HOME` nem a lista de contas: o servidor passa os dois em `Paths`.
#![forbid(unsafe_code)]

mod files;
mod install;
mod ssh;
mod state;
mod targets;

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub use files::{FileEntry, known_entry, read_entries};
pub use install::{Github, Policy, REPO, Swapped, install_if_missing, update_tick};
pub use ssh::{ssh_hosts, test_host, windows_setup};
pub use state::{Facts, state};
pub use targets::{TargetRequest, Transport, create_target, targets};

/// Nome da entrada em `mcpServers` e do binário.
pub const NAME: &str = "hangar-computer-control";

/// Erro de domínio no envelope `{"detail": {code, msg, params}}`.
#[derive(Debug)]
pub struct Error {
    pub status: u16,
    pub code: &'static str,
    pub msg: String,
    pub params: Value,
}

/// Caminhos de uma máquina. `accounts` são as pastas de conta Claude fora de `~/.claude`.
#[derive(Clone, Debug)]
pub struct Paths {
    pub home: PathBuf,
    pub accounts: Vec<PathBuf>,
}

impl Error {
    /// Falha de disco ou de ambiente fora do previsto: 500 com código, nunca o texto cru do servidor.
    pub fn io(error: impl std::fmt::Display) -> Self {
        let error = error.to_string();
        Self { status: 500, code: "erro_computer_control_io", msg: error.clone(), params: json!({"error": error}) }
    }

    pub(crate) fn at(path: &Path, error: &std::io::Error) -> Self {
        let error = strerror(error);
        Self {
            status: 500,
            code: "erro_computer_control_io",
            msg: format!("{}: {error}", path.display()),
            params: json!({"file": path.to_string_lossy(), "error": error}),
        }
    }

    pub(crate) fn read(path: &Path, error: &str, msg: String) -> Self {
        Self {
            status: 500,
            code: "erro_computer_control_read",
            msg,
            params: json!({"file": path.to_string_lossy(), "error": error}),
        }
    }

    pub(crate) fn plain(status: u16, code: &'static str, msg: &str) -> Self {
        Self { status, code, msg: msg.into(), params: json!({}) }
    }
}

impl Paths {
    pub fn new(home: PathBuf, accounts: Vec<PathBuf>) -> Self {
        Self { home, accounts }
    }
    pub fn main_file(&self) -> PathBuf {
        self.home.join(".claude.json")
    }
    /// Entrada guardada ao desligar: a marca de "desligado à mão".
    pub fn parked_file(&self) -> PathBuf {
        self.home.join(".hangar").join("computer-control.json")
    }
    pub fn install_dir(&self) -> PathBuf {
        self.home.join(".hangar").join("computer-control")
    }
    pub fn targets_dir(&self) -> PathBuf {
        self.install_dir().join("targets")
    }
    pub fn install_json(&self) -> PathBuf {
        self.install_dir().join("install.json")
    }
    /// A última instalação automática que falhou; some quando uma dá certo.
    pub fn install_error_file(&self) -> PathBuf {
        self.install_dir().join("install-error.json")
    }
    pub fn exe(&self) -> PathBuf {
        self.install_dir().join("windows-agent.exe")
    }
    /// No Windows o MCP é o próprio `windows-agent.exe`.
    pub fn binary(&self) -> PathBuf {
        if cfg!(windows) { self.exe() } else { self.install_dir().join(NAME) }
    }
    pub fn ssh_config(&self) -> PathBuf {
        self.home.join(".ssh").join("config")
    }
    /// O principal sempre; de cada conta, só o `.claude.json` que já existe.
    pub fn config_files(&self) -> Vec<PathBuf> {
        let accounts = self.accounts.iter().map(|dir| dir.join(".claude.json")).filter(|p| p.is_file());
        std::iter::once(self.main_file()).chain(accounts).collect()
    }
}

/// Há release do hcu para esta plataforma (Windows e Linux x86_64).
pub fn supported() -> bool {
    cfg!(windows) || (cfg!(target_os = "linux") && cfg!(target_arch = "x86_64"))
}

/// O `strerror` do Python: a mensagem do sistema sem o " (os error N)" do Rust.
fn strerror(error: &std::io::Error) -> String {
    let text = error.to_string();
    match text.rfind(" (os error ") {
        Some(at) if text.ends_with(')') => text[..at].to_owned(),
        _ => text,
    }
}
