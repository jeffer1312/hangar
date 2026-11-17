//! Retrato da tela Status (`GET /api/status` e cada evento `status` do SSE), lido pelo servidor,
//! pelo app nativo e pelo `packages/core`. Horários em RFC 3339 UTC, memória e disco em bytes, CPU
//! em porcentagem de um núcleo (100 = um núcleo cheio, como o `sysinfo`). Bloco ou campo ausente é
//! "sem fonte": o cliente esconde ou mostra "—", nunca um zero inventado.
use serde::{Deserialize, Serialize};

/// Raiz do retrato. Só `generated_at` e `attention` são obrigatórios: servidor que ainda não lê um
/// bloco simplesmente não o manda.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StatusSnapshot {
    pub generated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server: Option<ServerBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exits: Option<ExitsBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update: Option<UpdateBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machines: Option<Vec<Machine>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sessions: Option<SessionsBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slow: Option<SlowBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outside: Option<OutsideBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leftovers: Option<LeftoversBlock>,
    #[serde(default)]
    pub attention: Vec<Attention>,
}

/// Ponto de série, em ordem de tempo. A unidade de `v` é a do campo que guarda a série.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub ts: String,
    pub v: f64,
}

/// O processo `hangar-server` desta máquina.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ServerBlock {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<String>,
    pub pid: u32,
    pub started_at: String,
    pub rss_bytes: u64,
    /// A CPU só tem valor a partir da segunda amostra.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
    /// Última hora, um ponto por minuto; `v` em bytes.
    #[serde(default)]
    pub rss_series: Vec<Point>,
    /// Última hora, um ponto por minuto; `v` em porcentagem.
    #[serde(default)]
    pub cpu_series: Vec<Point>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_avg_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_avg_pct: Option<f64>,
}

/// Paradas do servidor nos últimos 7 dias.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ExitsBlock {
    #[serde(default)]
    pub items: Vec<Exit>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitKind {
    Crash,
    Update,
    Requested,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Exit {
    pub ts: String,
    pub kind: ExitKind,
    /// Código da causa; o texto sai da chave `status_exit_cause_<cause>` no cliente.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(default)]
    pub error_lines: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateBlock {
    pub channel: String,
    pub installed: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub available: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_result: Option<UpdateResult>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct UpdateResult {
    pub ts: String,
    pub ok: bool,
    /// Passo em que falhou; ausente quando deu certo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MachineState {
    Ok,
    Down,
    Unknown,
}

/// Uma máquina do `peers.json`, com o app nativo que bate presença nela.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Machine {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub this_machine: bool,
    pub state: MachineState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub down_since: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latency_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_version: Option<String>,
    #[serde(default)]
    pub outdated: bool,
    /// Respondeu pelo endereço da rede local, não pelo Tailscale.
    #[serde(default)]
    pub lan: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppReport>,
}

/// O que o app nativo informou na última presença.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppReport {
    pub version: String,
    pub rss_bytes: u64,
    pub cpu_pct: f64,
    pub platform: String,
    pub host: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionsBlock {
    pub open: u32,
    /// Chave = provider (`claude`, `codex`, `pi`, `omp`, `kimi`).
    #[serde(default)]
    pub by_provider: Vec<Usage>,
    /// Chave = grupo de processo (`claude`, `codex_app_server`, `pi_omp_kimi`, `cano`, `tmux`).
    #[serde(default)]
    pub by_process: Vec<Usage>,
    #[serde(default)]
    pub dead: Vec<DeadSession>,
    #[serde(default)]
    pub asleep: u32,
    #[serde(default)]
    pub queued: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oldest_queued_secs: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    pub key: String,
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_pct: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeadSession {
    pub name: String,
    /// Mesmo código da linha `dead` da lista (`terminal_out_of_memory`, `terminal_died`, …).
    pub cause: String,
}

/// Lentidão e erros das últimas 24 h, tirados do diário de uso.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlowBlock {
    /// 24 faixas de uma hora, a última é a hora corrente.
    #[serde(default)]
    pub hours: Vec<HourBucket>,
    #[serde(default)]
    pub top_routes: Vec<RouteStat>,
    #[serde(default)]
    pub watchdog: Vec<WatchdogEvent>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HourBucket {
    pub hour_start: String,
    pub slow: u32,
    pub errors: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RouteStat {
    /// Método e caminho em forma de modelo (`POST /api/sessions/{name}/input`).
    pub route: String,
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worst_ms: Option<u64>,
    pub last_code: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchdogKind {
    Late,
    Withheld,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WatchdogEvent {
    pub ts: String,
    pub kind: WatchdogKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u64>,
}

/// Memória, disco e carga desta máquina.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HostBlock {
    pub mem_total: u64,
    pub mem_available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// Ausente no Windows, que não tem carga média.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load: Option<[f64; 3]>,
    pub cpus: u32,
    pub disk_home: DiskUse,
    /// Ausente quando o temporário não é um disco próprio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_tmp: Option<DiskUse>,
    pub dir_hangar: DirSize,
    pub dir_build_cache: DirSize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskUse {
    pub used: u64,
    pub total: u64,
}

/// Tamanho de pasta custa caro: `computing` diz que a conta está em andamento, e `bytes` é o último
/// valor conhecido (ausente se nunca terminou).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirSize {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    #[serde(default)]
    pub computing: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutsideBlock {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect: Option<ConnectState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub funnel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keyring_locked: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectState {
    pub enabled: bool,
    pub running: bool,
}

/// Processos que sobraram sem dono.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LeftoversBlock {
    /// Nomes das sessões do multiplexador sem sessão do Hangar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux_orphans: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cano_orphans: Option<u32>,
    /// Processos Python ainda escutando na porta do servidor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub python_alive: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Bad,
    Warn,
}

/// Bloco da tela para onde o ponto de atenção leva.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttentionTarget {
    Server,
    Exits,
    Machines,
    Sessions,
    Slow,
    Host,
    Outside,
}

/// O texto é montado no cliente pela chave `status_attention_<code>` com `args`; nunca vem pronto,
/// para sair no idioma do aparelho.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Attention {
    pub severity: Severity,
    pub code: String,
    pub target: AttentionTarget,
    #[serde(default)]
    pub args: Vec<String>,
}
