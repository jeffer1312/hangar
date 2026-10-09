use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const MAX_FRAME: usize = 16 * 1024 * 1024;
pub const MAX_ENVELOPE: usize = 2 * MAX_FRAME + 1024;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClockSample {
    pub monotonic_s: f64,
    pub epoch_s: f64,
}

/// Barra o pedido de linha de status repetido: o mesmo dado só volta ao Python depois de
/// `FORMAT_REFRESH_S`, porque a cota que entra na linha muda sem mudar o dado.
#[derive(Default)]
pub struct FormatGate {
    last: Value,
    at: f64,
}

pub const FORMAT_REFRESH_S: f64 = 30.0;

impl FormatGate {
    pub fn due(&mut self, payload: &Value, now: f64) -> bool {
        if *payload == self.last && now - self.at < FORMAT_REFRESH_S {
            return false;
        }
        self.last = payload.clone();
        self.at = now;
        true
    }

    pub fn reset(&mut self) {
        self.last = Value::Null;
    }
}

pub use hangar_codex::proto::RequestId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Disposition { Accepted, Deferred, Rejected, Unknown }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteOutcome { Written, NotWritten, Unknown }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Input, Steer, SteerQueue, Interrupt, AnswerQuestions, Select, SetModel, SetEffort, SetServiceTier,
    SetPermissionMode, Compact, ListModels, ListSkills, ReadRateLimits, ReadSettings,
    SetMode, SkipQuestion, Restart, OpenTerminal, Reload, Commands, Cwd, Detach,
    VoiceOpen, VoiceRpc, VoiceRespond, VoiceClose,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCommand {
    pub operation_id: String,
    pub kind: OperationKind,
    pub payload: Value,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum EngineInput {
    Line(Value),
    WriteAck { operation_id: String, outcome: WriteOutcome },
    PolicyResult { request_id: RequestId, payload: Value },
    Tick,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    Write { frame: Value, operation_id: Option<String> },
    Publish { channel: String, data: Value },
    Policy { kind: String, request_id: RequestId, payload: Value },
    Reply { operation_id: String, disposition: Disposition, payload: Value },
    WakeQueue,
    /// A CLI consumiu um comando local (`/clear`, `/cost`…), que não vira linha `user` no transcript.
    ConfirmLocalCommands,
    StateChanged,
    /// Interface dos mods (superfície remota): quadro fora do diário, publicação, aviso e resposta aos apps.
    Surface { effect: crate::mods::model::SurfaceEffect },
    /// Linha no diário exportável (`/internal/diag`), uma por minuto por código.
    Diag { event: DiagEvent, code: String },
    Stop { reason: String },
    /// O processo da sessão sobe de novo na mesma conversa (reiniciar, trocar o sandbox). Quem sobe é
    /// o ator; `patch` vai ao arquivo da sessão antes de pedir o comando novo ao Python.
    Respawn { operation_id: String, reason: String, patch: Value, reply: Value },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagEvent { CodexVersion, CodexDecode, CodexBootstrap }

impl DiagEvent {
    pub fn event(self) -> &'static str { match self { Self::CodexVersion => "rust.codex_version", Self::CodexDecode => "rust.codex_decode", Self::CodexBootstrap => "rust.codex_bootstrap" } }
    pub fn reason(self) -> &'static str {
        match self {
            Self::CodexVersion => "versão do Codex diferente da conferida ou ilegível; campo renomeado pode faltar",
            Self::CodexDecode => "formato inesperado do Codex: notificação ignorada (ciclo de vida lido cru), resposta com o padrão, pedido mostrado pela linha crua",
            Self::CodexBootstrap => "o Codex recusou abrir a conversa na subida; a sessão não fica pronta e mostra o problema",
        }
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeReply {
    pub operation_id: String,
    pub disposition: Disposition,
    pub payload: Value,
}

/// Último valor da sessão sem terminal para o hub dela (`RuntimeRegistry::live`): a prévia sai
/// só por aqui, fora do `events`, e o feed do hub a publica coalescida.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LiveState {
    pub public_state: Value,
    pub preview: String,
    pub thinking: String,
    pub tool: String,
    /// Erro durável do ator: (código, frase).
    pub error: Option<(String, String)>,
}

pub type LiveSender = tokio::sync::watch::Sender<Option<std::sync::Arc<LiveState>>>;
pub type LiveReceiver = tokio::sync::watch::Receiver<Option<std::sync::Arc<LiveState>>>;

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeEvent {
    pub key: String,
    pub generation: u64,
    pub revision: u64,
    pub channel: String,
    pub data: Value,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanoBinding {
    pub pid: u32,
    pub escuta: String,
    pub token: String,
    pub versao: u32,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeTarget {
    pub key: String,
    pub generation: u64,
    pub name: String,
    pub provider: String,
    pub metadata: Value,
    pub binding: CanoBinding,
    pub lease_path: PathBuf,
    pub state_path: PathBuf,
    pub projection_dir: PathBuf,
    pub transcript: PathBuf,
    pub created: f64,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanoSnapshot {
    #[serde(rename = "type")]
    pub kind: String,
    pub versao: u32,
    pub pid: u32,
    pub init: Option<String>,
    pub aberto: bool,
    pub pendentes: Vec<String>,
    pub ultimo_result: Option<String>,
    pub rate_limit: Option<String>,
    pub stderr_tail: Vec<String>,
    pub saiu: Option<i64>,
    pub inflight: Value,
}

impl CanoSnapshot {
    pub fn parse(value: Value) -> Result<Self, RuntimeError> {
        if value.get("type").and_then(Value::as_str) != Some("cano_snapshot")
            || value.get("versao").and_then(Value::as_u64) != Some(2) {
            return Err(RuntimeError::new("snapshot_version", "snapshot do cano incompatível"));
        }
        let snapshot: Self = serde_json::from_value(value)
            .map_err(|_| RuntimeError::new("snapshot_shape", "snapshot do cano inválido"))?;
        for raw in snapshot.pendentes.iter().chain(snapshot.init.iter())
            .chain(snapshot.ultimo_result.iter()).chain(snapshot.rate_limit.iter()) {
            if raw.len() > MAX_FRAME || !serde_json::from_str::<Value>(raw)
                .is_ok_and(|v| v.is_object()) {
                return Err(RuntimeError::new("snapshot_line", "linha incompleta no snapshot"));
            }
        }
        Ok(snapshot)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RuntimeError {
    pub code: String,
    pub message: String,
}

impl RuntimeError {
    pub fn new(code: &str, message: &str) -> Self {
        Self { code: code.to_owned(), message: message.to_owned() }
    }
}

impl std::fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}
impl std::error::Error for RuntimeError {}
