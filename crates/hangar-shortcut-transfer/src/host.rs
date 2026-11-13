use std::collections::BTreeMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use serde_json::Value;

use hangar_core::path_markers::Roots;

/// Falha de uma operação: `Invalid` é recusa do pedido (400), `Io` é falha da máquina (500).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Invalid(String),
    Io(String),
}

/// O que o servidor sabe desta máquina, lido a cada pedido.
#[derive(Clone, Debug)]
pub struct Machine {
    pub roots: Roots,
    pub home: PathBuf,
    /// Ambiente do servidor: `SHELL` decide o dialeto e o shell do `verify`.
    pub env: BTreeMap<String, String>,
    /// Empacotar e instalar scripts só existe em Linux.
    pub linux: bool,
}

/// Estado da chave `shortcuts` antes da gravação de uma importação, para o desfazer.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    /// Valor gravado no arquivo; `None` = chave ausente.
    pub shortcuts: Option<Value>,
    /// Modo do `runtime-config.json`; `None` = arquivo ausente.
    pub mode: Option<u32>,
    pub parent_existed: bool,
}

/// O que o servidor fornece: a configuração com um escritor só e o PATH do shell de login.
pub trait Host: Send + Sync + 'static {
    fn machine(&self) -> Machine;
    /// Valor efetivo de `shortcuts` (arquivo ou ambiente), como a leitura da configuração.
    ///
    /// # Errors
    /// `Io` quando a configuração não pode ser lida.
    fn shortcuts(&self) -> Result<Value, Error>;
    /// Regras de forma da lista de atalhos, as mesmas da gravação.
    ///
    /// # Errors
    /// A mensagem da recusa.
    fn validate(&self, text: &str) -> Result<(), String>;
    /// # Errors
    /// Arquivo de configuração ilegível.
    fn snapshot(&self) -> Result<Snapshot, Error>;
    /// Grava só a chave `shortcuts`.
    ///
    /// # Errors
    /// `Invalid` para valor recusado, `Io` para falha de disco.
    fn save(&self, serialized: &str) -> Result<(), Error>;
    /// Volta `shortcuts` ao `before` só se a chave ainda é `serialized`; devolve o modo do arquivo.
    ///
    /// # Errors
    /// A gravação de volta falhou.
    fn undo(&self, serialized: &str, before: &Snapshot) -> Result<(), String>;
    fn login_path(&self) -> Pin<Box<dyn Future<Output = String> + Send + 'static>>;
}
