//! Identidade e interface de E/S do condutor.
use std::path::PathBuf;

use futures_util::future::BoxFuture;
use hangar_core::ids::session_life;
use hangar_core::runtime_reply::RuntimeError;
use hangar_core::session_dirs::SessionDirectories;
use hangar_core::terminal_delivery::TerminalBinding;
use hangar_interactions::InteractionTarget;
use serde_json::Value;

#[derive(Clone, Debug)]
pub struct OtherTarget {
    pub key: String,
    pub generation: u64,
    pub name: String,
    pub provider: hangar_transcript::Provider,
    pub binding: TerminalBinding,
    pub pane_address: String,
    pub dirs: SessionDirectories,
    pub cwd: String,
    pub pane_pid: i64,
    pub agent_pid: i64,
    pub agent_birth: f64,
    pub transcript: PathBuf,
    pub state_path: PathBuf,
    pub lease_path: PathBuf,
    pub projection_dir: PathBuf,
    pub config_dirs: Vec<PathBuf>,
    pub created: f64,
}
/// Identidade pública opaca, distinta para cada geração da mesma chave.
pub fn lifecycle_id(key: &str, generation: u64) -> String {
    format!(
        "{}:g:{generation}",
        session_life(Some(key), None).unwrap_or_default()
    )
}
impl OtherTarget {
    pub fn lifecycle_id(&self) -> String {
        lifecycle_id(&self.key, self.generation)
    }
    pub fn same_life(&self, other: &Self) -> bool {
        self.key == other.key
            && self.generation == other.generation
            && self.name == other.name
            && self.provider == other.provider
            && self.binding == other.binding
            && self.agent_pid == other.agent_pid
            && self.agent_birth == other.agent_birth
    }
}

#[derive(Clone, Debug)]
pub struct OtherFrame {
    pub text: String,
    pub typed: Option<String>,
    pub styled: Option<String>,
    pub editor: Option<String>,
}
impl OtherFrame {
    pub fn from_styled(styled: String) -> Self {
        Self {
            text: hangar_other_terminal_policy::unstyle(&styled, true),
            typed: Some(hangar_other_terminal_policy::unstyle(&styled, false)),
            styled: Some(styled),
            editor: None,
        }
    }
}
#[derive(Clone, Debug)]
pub struct OtherWriteGuard {
    pub target: InteractionTarget,
    pub semantic: Value,
    /// A tecla que responde a pergunta: depois dela a pergunta some, e isso é a entrega.
    pub closing: bool,
}
/// Fronteira de E/S: testes não substituem fila, interações nem regras do próprio runtime.
pub trait OtherIo: Send + Sync {
    fn prove<'a>(&'a self, target: &'a OtherTarget) -> BoxFuture<'a, Result<bool, RuntimeError>>;
    /// A prova de quem só observa: pode aceitar leitura recente do sistema. Quem escreve usa `prove`.
    fn prove_observed<'a>(&'a self, target: &'a OtherTarget) -> BoxFuture<'a, Result<bool, RuntimeError>> {
        self.prove(target)
    }
    fn capture<'a>(&'a self) -> BoxFuture<'a, Result<OtherFrame, RuntimeError>>;
    fn write<'a>(
        &'a self,
        target: &'a OtherTarget,
        value: &'a str,
        literal: bool,
        guard: Option<&'a OtherWriteGuard>,
    ) -> BoxFuture<'a, Result<(), RuntimeError>>;
    /// Modo estruturado do agente; sem arquivo de conversa, o modo ainda é desconhecido.
    fn kimi_permission(&self) -> BoxFuture<'_, Result<Option<String>, RuntimeError>>;
    fn release<'a>(&'a self) -> BoxFuture<'a, ()>;
}
pub fn failed(code: &str) -> RuntimeError {
    RuntimeError::new(
        code,
        "não foi possível comprovar a operação desta vida do terminal",
    )
}
