use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const MAX_FRAME: usize = 16 * 1024 * 1024;
pub const MAX_ENVELOPE: usize = 2 * MAX_FRAME + 1024;

/// Identidade estável do trabalho em andamento; prévias e alterações da fila não a modificam.
pub fn turn_marker(view: &Value) -> Option<Value> {
    if view["terminal"] == true {
        return view["working_revision"].as_u64().filter(|revision| *revision > 0)
            .map(|revision| serde_json::json!({"working_revision": revision}));
    }
    if view["in_progress"] != true { return None; }
    for key in ["turn_id", "turn_started"] {
        let value = &view[key];
        if value.is_number() || value.as_str().is_some_and(|s| !s.is_empty()) {
            return Some(serde_json::json!({(key): value}));
        }
    }
    None
}

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
    SetPermissionMode, Compact, Clear, ListModels, ListSkills, ReadRateLimits, ReadSettings,
    SetMode, SkipQuestion, Restart, OpenTerminal, Reload, Cwd, Detach,
    /// Interna: só o ator a cria, a partir da entrada reservada que a fila devolveu.
    DispatchAsyncAnswer,
}

/// Resposta a uma pergunta assíncrona do Codex com terminal, já formada e guardada na fila como
/// entrada própria até a entrega. `operation_id` é o da operação persistida (igual à entrada).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ReservedAsyncAnswer {
    pub request_id: String,
    pub interaction_id: String,
    pub entry_id: String,
    pub operation_id: String,
    pub text: String,
    /// Orientação só vale no turno em que a pessoa respondeu; depois dele, espera um turno novo.
    #[serde(default)]
    pub steer_turn: Option<String>,
}

impl ReservedAsyncAnswer {
    /// Reservada não é entregue: quem respondeu recebe o resultado incerto, nunca "permitido".
    pub fn reply_payload(&self) -> Value {
        serde_json::json!({"reserved":true,"entry_id":self.entry_id,"decision_confirmed":false})
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Effect {
    Write { frame: Value, operation_id: Option<String> },
    Publish { channel: String, data: Value },
    Policy { kind: String, request_id: RequestId, payload: Value },
    /// Sidecar Claude e vínculo da conversa: gravados localmente sob a lease da fila.
    PersistClaude { operation_id:Option<String>, patch:Value },
    Reply { operation_id: String, disposition: Disposition, payload: Value },
    WakeQueue,
    /// A CLI consumiu um comando local (`/clear`, `/cost`…), que não vira linha `user` no transcript.
    ConfirmLocalCommands,
    /// Claude sem terminal: o eco (`isReplay`, mesmo `uuid`) da mensagem escrita por esta operação saiu, então
    /// ela entrou na conversa (o prompt barrado por hook não ecoa).
    InputEchoed { operation_id: String },
    StateChanged,
    /// Interface dos mods (superfície remota): quadro fora do diário, publicação, aviso e resposta aos apps.
    Surface { effect: crate::mods::model::SurfaceEffect },
    /// Linha no diário exportável (`/internal/diag`), uma por minuto por código.
    Diag { event: DiagEvent, code: String },
    Stop { reason: String },
    /// O processo da sessão sobe de novo na mesma conversa (reiniciar, trocar o sandbox). Quem sobe é
    /// o ator; `patch` vai ao arquivo da sessão antes de pedir o comando novo ao Python.
    Respawn { operation_id: String, reason: String, patch: Value, reply: Value },
    /// Grava na fila, numa atualização só, a entrada e a operação `DispatchAsyncAnswer`; quem
    /// respondeu (`operation_id`) só recebe a resposta depois de gravada.
    ReserveAsyncAnswer { operation_id: String, reservation: ReservedAsyncAnswer, command: RuntimeCommand },
    /// O turno parou no limite de uso da conta (não no aviso temporário): quem troca de conta decide.
    UsageLimit { limit: UsageLimit },
}

/// Janela esgotada (`five_hour`, `seven_day`, …) e quando ela volta (epoch em segundos).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UsageLimit { pub window: String, pub resets_at: Option<f64> }

impl UsageLimit {
    /// O `rate_limit_info` do Claude (`rateLimitType`, `resetsAt`).
    pub fn from_info(info: &Value) -> Self {
        Self { window: info["rateLimitType"].as_str().unwrap_or("unknown").into(), resets_at: info["resetsAt"].as_f64() }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagEvent { CodexDecode, CodexBootstrap, CodexConnectionClosed, ClaudeSidecarAbsent, CanoProtocolInvalid, CanoReconnectStopped, CanoOutputLost }

impl DiagEvent {
    pub fn event(self) -> &'static str {
        match self {
            Self::CodexDecode => "rust.codex_decode", Self::CodexBootstrap => "rust.codex_bootstrap",
            Self::CodexConnectionClosed => "rust.codex_connection_closed", Self::ClaudeSidecarAbsent => "rust.claude_sidecar_absent",
            Self::CanoProtocolInvalid => "rust.cano_protocol_invalid", Self::CanoReconnectStopped => "rust.cano_reconnect_stopped",
            Self::CanoOutputLost => "rust.cano_output_lost",
        }
    }
    pub fn reason(self) -> &'static str {
        match self {
            Self::CodexDecode => "formato inesperado do Codex: notificação ignorada (ciclo de vida lido cru), resposta com o padrão, pedido mostrado pela linha crua",
            Self::CodexBootstrap => "o Codex recusou abrir a conversa na subida; a sessão não fica pronta e mostra o problema",
            Self::CodexConnectionClosed => "a conexão de controle com o app-server do Codex com terminal caiu; o ator reconecta ao mesmo processo",
            Self::ClaudeSidecarAbsent => "o sidecar da sessão sumiu (fechada pelo nome); a gravação do estado não foi feita",
            Self::CanoProtocolInvalid => "o cano mandou um envelope que o servidor não conhece; o ator religou ao mesmo processo",
            Self::CanoReconnectStopped => "a religação ao cano parou: falta o que prova que o processo é o desta vida",
            Self::CanoOutputLost => "o cano parou de repassar a saída do Claude; o fim dos turnos desta vida vem do transcript",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
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
    pub binding: Option<crate::interactions::InteractionBinding>,
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
