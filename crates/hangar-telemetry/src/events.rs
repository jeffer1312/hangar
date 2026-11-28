//! Eventos e dimensões do pacote. Toda dimensão é enum daqui, bool, `Code` conferido contra uma lista
//! ou versão de release: `DimValue` não tem construtor a partir de texto, então nada livre sai.
use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::wire::is_name;

macro_rules! named_enum {
    ($(#[$meta:meta])* $name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub enum $name { $($variant),+ }

        impl $name {
            pub const ALL: &[$name] = &[$($name::$variant),+];

            pub const fn as_str(self) -> &'static str {
                match self { $($name::$variant => $wire),+ }
            }

            /// O nome no fio de volta ao valor.
            pub fn parse(value: &str) -> Option<$name> {
                $name::ALL.iter().copied().find(|known| known.as_str() == value)
            }
        }
    };
}

macro_rules! dim_enum {
    ($(#[$meta:meta])* $name:ident { $($body:tt)+ }) => {
        named_enum!($(#[$meta])* $name { $($body)+ });

        impl From<$name> for DimValue {
            fn from(value: $name) -> Self { DimValue(Cow::Borrowed(value.as_str())) }
        }
    };
}

named_enum!(Event {
    AppVisibleMinutes => "app_visible_minutes",
    MessageSent => "message_sent",
    TurnFinished => "turn_finished",
    SessionOpened => "session_opened",
    FeatureUsed => "feature_used",
    McpTool => "mcp_tool",
    VoiceCall => "voice_call",
    Dictation => "dictation",
    ProblemShown => "problem_shown",
    ProblemHidden => "problem_hidden",
    SessionOpenFailed => "session_open_failed",
    SendFailed => "send_failed",
    SessionFallen => "session_fallen",
    ServerExit => "server_exit",
    LimitSwitch => "limit_switch",
    ClientFatal => "client_fatal",
    UpdateFinished => "update_finished",
    UpdateStep => "update_step",
    UpdateFailedCode => "update_failed_code",
    Peers => "peers",
    ClientRoute => "client_route",
    ServerStartup => "server_startup",
    ChatOpen => "chat_open",
    SessionPlaced => "session_placed",
    LogDropped => "log_dropped",
    Settings => "settings",
    DiaryDropped => "diary_dropped",
    SessionReady => "session_ready",
    SessionFirstSend => "session_first_send",
    FirstReply => "first_reply",
    AppReady => "app_ready",
});

named_enum!(
    /// Nível de uma linha de log que vai no pacote.
    LogLevel { Warn => "warn", Error => "error" }
);

/// Valor de uma dimensão; só nasce dos tipos deste módulo.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DimValue(Cow<'static, str>);

impl DimValue {
    pub const OTHER: DimValue = DimValue(Cow::Borrowed("other"));

    /// Versão de release: três ou quatro segmentos numéricos; outra forma vira `other`.
    pub fn version(value: &str) -> DimValue {
        let parts = value.split('.');
        let numeric = parts.clone().all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()));
        if is_name(value) && numeric && (3..=4).contains(&parts.count()) {
            DimValue(Cow::Owned(value.to_owned()))
        } else {
            DimValue::OTHER
        }
    }

    pub fn as_str(&self) -> &str { &self.0 }
}

impl From<bool> for DimValue {
    fn from(value: bool) -> Self { DimValue(Cow::Borrowed(if value { "true" } else { "false" })) }
}

impl From<Code> for DimValue {
    fn from(code: Code) -> Self { DimValue(Cow::Borrowed(code.0)) }
}

/// Dimensões de um contador, por chave.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Dims(BTreeMap<&'static str, DimValue>);

impl Dims {
    pub fn new() -> Dims { Dims::default() }

    pub fn with(mut self, key: &'static str, value: impl Into<DimValue>) -> Dims {
        debug_assert!(is_name(key), "chave de dimensão fora do formato: {key}");
        self.0.insert(key, value.into());
        self
    }

    pub fn into_wire(self) -> BTreeMap<String, String> {
        self.0.into_iter().map(|(key, value)| (key.to_owned(), value.0.into_owned())).collect()
    }
}

/// Código conferido contra uma lista conhecida; o que não está nela sai `other`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Code(&'static str);

impl Code {
    pub const OTHER: Code = Code("other");
    pub const ENGINE: Code = Code("engine");

    /// Código que já é texto fixo do binário (nunca texto de fora); fora do formato do receptor vira `other`.
    pub const fn fixed(code: &'static str) -> Code {
        if is_name(code) { Code(code) } else { Code::OTHER }
    }

    /// Item da lista fora do formato do receptor também vira `other`: ele recusaria o pacote inteiro.
    pub fn known(value: &str, list: &[&'static str]) -> Code {
        list.iter().copied().find(|known| *known == value && is_name(known)).map_or(Code::OTHER, Code)
    }

    pub const fn as_str(self) -> &'static str { self.0 }
}

dim_enum!(Provider {
    Claude => "claude", Codex => "codex", Pi => "pi", Omp => "omp", Kimi => "kimi",
    Antigravity => "antigravity", Other => "other",
});

dim_enum!(Surface { Terminal => "terminal", Headless => "headless" });

dim_enum!(
    /// Quem mandou o evento; `Server` só no retrato de preferências do próprio servidor.
    ClientKind { Ios => "ios", Android => "android", Pwa => "pwa", Native => "native", Server => "server", Other => "other" }
);

dim_enum!(
    /// Quem escreveu a mensagem: a pessoa, ou outra sessão (texto que casa o `FROM` do `input`).
    SenderOrigin { Person => "person", Session => "session" }
);

dim_enum!(
    /// Quem pediu a sessão: a pessoa pela tela, um agente (`creator`), a restauração depois de reboot ou uma tarefa agendada.
    OpenedBy { Person => "person", Agent => "agent", Restore => "restore", Scheduled => "scheduled", Other => "other" }
);

dim_enum!(UpdateStepName {
    Download => "download", Swap => "swap", Steps => "steps", Restart => "restart", Other => "other",
});

impl UpdateStepName {
    /// A etapa com o nome que o `estado.json` da atualização grava.
    pub fn from_stage(stage: &str) -> UpdateStepName {
        match stage {
            "baixar" => UpdateStepName::Download,
            "trocar" => UpdateStepName::Swap,
            "passos" => UpdateStepName::Steps,
            "reiniciar" => UpdateStepName::Restart,
            _ => UpdateStepName::Other,
        }
    }
}

dim_enum!(UpdateOrigin { Manual => "manual", Auto => "auto", Other => "other" });

dim_enum!(
    /// Onde a abertura com `machine: "auto"` ficou: nesta máquina, em outra, ou recusada (Ask, máquina fora, clone).
    PlacementOutcome { Here => "here", Elsewhere => "elsewhere", Refused => "refused" }
);

/// O `code` de cada `ReasonCode` do `hangar-placement`; motivo novo sai `other` até entrar aqui.
pub const PLACEMENT_REASONS: &[&str] = &[
    "same_folder", "unpublished", "plain_dir", "project_rule", "local_within_limits", "least_loaded", "no_candidate", "not_balanced",
];

dim_enum!(PeerKind {
    Lan => "lan", Tailscale => "tailscale", Connect => "connect", Public => "public", Disabled => "disabled",
});

// ponytail: lista curta, sem id completo de modelo; id novo cai em `other` até entrar aqui.
const CLAUDE_MODELS: &[&str] = &["default", "best", "opus", "opusplan", "sonnet", "haiku", "fable"];
const CODEX_MODELS: &[&str] = &[
    "gpt-6.2-sol", "gpt-6.1-sol", "gpt-6-sol", "gpt-6-astra", "gpt-6-luna", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna",
];
const KIMI_MODELS: &[&str] = &["kimi-k3"];
const ANTIGRAVITY_MODELS: &[&str] = &["gemini-4-neon", "gemini-3.8-flash"];

/// Modelo da sessão. Motor de `engines.json` sai só `engine`: o nome dele é escolha da pessoa.
pub fn model(provider: Provider, value: &str, engine: bool) -> Code {
    if engine {
        return Code::ENGINE;
    }
    let list = match provider {
        Provider::Claude => CLAUDE_MODELS,
        Provider::Codex => CODEX_MODELS,
        Provider::Kimi => KIMI_MODELS,
        Provider::Antigravity => ANTIGRAVITY_MODELS,
        Provider::Pi | Provider::Omp | Provider::Other => &[],
    };
    Code::known(value, list)
}

const ON_OFF: &[&str] = &["on", "off"];

/// As preferências do retrato diário (`Event::Settings`) e os valores de cada uma. Os clientes já mandam
/// nestes nomes; valor livre (nome de fonte, cor, modelo) não tem nome aqui e sai `other`.
pub const SETTINGS: &[(&str, &[&str])] = &[
    ("theme", &["auto", "light", "dark", "desktop"]),
    ("panels", &["attached", "floating"]),
    ("palette", &["classic", "neutral"]),
    ("background", &["plain", "texture", "aurora", "image", "desktop"]),
    ("background_effect", &["none", "dither", "ascii", "halftone", "scanlines"]),
    ("wallpaper", &["window", "glass"]),
    ("surface_material", &["glass", "opaque"]),
    ("reading", &["auto", "none", "text", "sheet"]),
    ("font", &["system", "mono", "custom"]),
    ("code_font", &["jetbrains_mono", "system", "custom"]),
    // Comparado ao tamanho de fábrica do cliente: menor, igual ou maior.
    ("text_size", &["small", "normal", "large"]),
    ("tool_look", &["classic", "chips", "tree", "terminal", "claude_desktop"]),
    ("thinking_tools", &["none", "search", "all"]),
    ("thinking_collapsed", ON_OFF),
    ("task_list", ON_OFF),
    ("table_chart", ON_OFF),
    ("ask_highlight", &["accent", "amber"]),
    ("delivery", &["queue", "direct", "jev", "conversation"]),
    ("hands_free", ON_OFF),
    ("compact_style", ON_OFF),
    ("navigation", &["tabs", "bottom_tabs", "conversations", "sidebar"]),
    ("sidebar_compact", ON_OFF),
    ("sidebar_height", &["full", "content"]),
    ("sidebar_group", &["none", "project", "machine"]),
    ("tabs_full", ON_OFF),
    ("list_group", &["none", "server", "project"]),
    ("list_organize", &["sessions", "conversations"]),
    ("home_screen", &["sessions", "new_conversation"]),
    ("tablet_bar", &["left", "bottom", "top"]),
    ("language", &["system", "pt", "en"]),
    ("tips", ON_OFF),
    ("tour", &["done", "skipped", "none"]),
    ("conversation_beta", ON_OFF),
    ("code_mode", ON_OFF),
    ("default_provider", &["claude", "codex", "pi", "omp", "kimi", "antigravity"]),
    ("default_effort", &["unset", "off", "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"]),
    ("auto_report", &["on", "off", "unanswered"]),
    ("dictation_org", &["none", "harness", "external_api"]),
    ("dictation_provider", &["openai", "groq", "elevenlabs", "whisper_cpp", "none"]),
    ("connect", ON_OFF),
    ("computer_use", ON_OFF),
    ("whatsapp", &["installed", "no"]),
    ("jev", &["configured", "no"]),
];

/// A preferência e o valor dela pela lista `SETTINGS`: preferência fora da lista é `None`, valor fora sai `other`.
pub fn setting(name: &str, value: &str) -> Option<(Code, Code)> {
    SETTINGS.iter().find(|(known, _)| *known == name).map(|(known, values)| (Code::fixed(known), Code::known(value, values)))
}

/// Método e modelo de rota do axum (`MatchedPath`) que contam como `feature_used`, e o código no fio.
/// Conta o recurso usado, não a consulta: leitura que os clientes repetem (lista, `/history`,
/// `/events`, custos, status, plugins, cofre…) fica fora, e o método separa a escrita da leitura do
/// mesmo modelo. Fora também: WebSocket (nunca responde 2xx), rotas do plugin, de máquina para
/// máquina, as atendidas pelo `dispatch` de reserva (sem `MatchedPath`), as de curinga
/// (`{*rest}`, que não separam o recurso) e as já contadas por outro evento (`/input`, voz, ditado).
pub const FEATURE_ROUTES: &[(&str, &str, &str)] = &[
    ("POST", "/api/sessions/{name}/answer", "session.answer"),
    ("POST", "/api/sessions/{name}/select", "session.select"),
    ("POST", "/api/sessions/{name}/select/submit", "session.select"),
    ("POST", "/api/sessions/{name}/question/skip", "session.question_skip"),
    ("POST", "/api/sessions/{name}/interrupt", "session.interrupt"),
    ("POST", "/api/sessions/{name}/tasks/{task_id}/stop", "session.task_stop"),
    ("POST", "/api/sessions/{name}/btw", "session.btw"),
    ("POST", "/api/sessions/{name}/rename", "session.rename"),
    ("POST", "/api/sessions/{name}/reopen", "session.reopen"),
    ("POST", "/api/sessions/{name}/dismiss", "session.dismiss"),
    ("DELETE", "/api/sessions/{name}", "session.close"),
    ("POST", "/api/sessions/{name}/resume", "session.resume"),
    ("POST", "/api/sessions/{name}/recarregar", "session.reload"),
    ("POST", "/api/sessions/{name}/modo-execucao", "session.surface"),
    ("POST", "/api/sessions/{name}/conta", "session.account"),
    ("POST", "/api/sessions/{name}/model", "session.model"),
    ("POST", "/api/sessions/{name}/model-effort", "session.model"),
    ("POST", "/api/sessions/{name}/engine/model", "session.model"),
    ("POST", "/api/sessions/{name}/engine/provider", "session.provider"),
    ("POST", "/api/sessions/{name}/permission-mode", "session.permission_mode"),
    ("POST", "/api/sessions/{name}/codex/mode", "session.codex_mode"),
    ("POST", "/api/sessions/{name}/codex/plan/implement", "session.plan_implement"),
    ("POST", "/api/sessions/{name}/service-tier", "session.service_tier"),
    ("POST", "/api/sessions/{name}/notice/fix", "session.notice_fix"),
    ("POST", "/api/sessions/{name}/queue/reorder", "session.queue"),
    ("PATCH", "/api/sessions/{name}/queue/{entry_id}", "session.queue"),
    ("DELETE", "/api/sessions/{name}/queue/{entry_id}", "session.queue"),
    ("POST", "/api/sessions/{name}/queue/{entry_id}/steer", "session.queue"),
    ("PUT", "/api/sessions/{name}/hidden", "session.hide"),
    ("POST", "/api/sessions/{name}/shell", "session.shell"),
    ("POST", "/api/sessions/{name}/open-editor", "session.open_editor"),
    ("POST", "/api/sessions/{name}/open-terminal", "session.open_terminal"),
    ("GET", "/api/sessions/{name}/navegador", "session.browser"),
    ("POST", "/api/sessions/{name}/loop", "session.loop"),
    ("PUT", "/api/sessions/{name}/then", "session.then"),
    ("POST", "/api/sessions/{name}/subagents/{agent_id}/message", "session.subagent_message"),
    ("POST", "/api/sessions/{name}/ports/{port}/stop", "session.port_stop"),
    ("POST", "/api/sessions/{name}/pair", "session.pair"),
    ("POST", "/api/sessions/{name}/pair-remote", "session.pair"),
    ("POST", "/api/sessions/{name}/pair-invite", "session.pair_external"),
    ("POST", "/api/sessions/{name}/group-message", "session.group_message"),
    ("POST", "/api/sessions/{name}/share", "session.share"),
    ("GET", "/api/search", "search"),
    ("POST", "/api/ask-history", "ask_history"),
    ("POST", "/api/archive/{project}/{session_id}/resume", "archive.resume"),
    ("POST", "/api/broadcast", "broadcast"),
    ("POST", "/api/plugins/{id}/install", "plugins.install"),
    ("POST", "/api/vault/entries", "vault.add"),
    ("POST", "/api/vault/use", "vault.use"),
    ("POST", "/api/keyring/unlock", "keyring_unlock"),
    ("POST", "/api/diag/report", "diag_report"),
    ("POST", "/api/device-install/start", "device_install"),
    ("POST", "/api/config-sync/apply", "config_sync"),
    ("PUT", "/api/tags", "tags"),
    ("POST", "/api/push/mute", "push.mute"),
];


// ---- Contexto das falhas: ambiente da máquina, retrato na hora e causa específica ----

/// Faixa de quantidade (sessões, contas).
pub const COUNT_BANDS: &[&str] = &["0", "1", "2-3", "4-9", "ge10"];
/// Faixa de idade (servidor de pé, sessão).
pub const AGE_BANDS: &[&str] = &["lt1m", "lt10m", "lt1h", "lt1d", "ge1d"];
const UP_VALUES: &[&str] = &["lt1m", "lt10m", "lt1h", "lt1d", "ge1d", "down"];
const MEM_BANDS: &[&str] = &["lt5", "lt10", "lt20", "lt40", "ge40"];
const DISK_BANDS: &[&str] = &["lt1g", "lt5g", "lt20g", "ge20g"];
const LOAD_BANDS: &[&str] = &["lt0_5", "lt1", "lt2", "ge2"];
const CPU_PCT_BANDS: &[&str] = &["lt50", "lt80", "ge80"];
const CPU_BANDS: &[&str] = &["1-2", "3-4", "5-8", "9-16", "17-32", "ge33"];
const RAM_BANDS: &[&str] = &["lt4g", "4-8g", "8-16g", "16-32g", "32-64g", "ge64g"];
const STATES: &[&str] = &["working", "idle", "awaiting_input", "other"];
const RESUMED: &[&str] = &["no", "lt5s", "lt30s"];
const OS_KINDS: &[&str] = &["linux", "windows", "macos", "other"];
/// Distribuições conhecidas pelo `ID` do `/etc/os-release`; Windows e macOS levam o próprio nome.
const OS_NAMES: &[&str] = &[
    "arch", "cachyos", "manjaro", "endeavouros", "ubuntu", "debian", "linuxmint", "pop", "fedora", "nixos",
    "opensuse-tumbleweed", "opensuse-leap", "rhel", "centos", "rocky", "almalinux", "alpine", "gentoo", "void", "kali",
    "elementary", "zorin", "garuda", "windows_10", "windows_11", "macos",
];
const MUXES: &[&str] = &["tmux", "psmux", "none"];
/// Tipos de `io::ErrorKind` que distinguem a causa sem texto.
pub const IO_KINDS: &[&str] = &[
    "not_found", "permission_denied", "connection_refused", "connection_reset", "broken_pipe", "already_exists",
    "would_block", "invalid_input", "invalid_data", "timed_out", "write_zero", "interrupted", "unexpected_eof",
    "out_of_memory", "storage_full", "other",
];

/// `io::ErrorKind` no nome fechado.
pub fn io_kind(kind: std::io::ErrorKind) -> Code {
    use std::io::ErrorKind as K;
    Code::fixed(match kind {
        K::NotFound => "not_found", K::PermissionDenied => "permission_denied", K::ConnectionRefused => "connection_refused",
        K::ConnectionReset => "connection_reset", K::BrokenPipe => "broken_pipe", K::AlreadyExists => "already_exists",
        K::WouldBlock => "would_block", K::InvalidInput => "invalid_input", K::InvalidData => "invalid_data",
        K::TimedOut => "timed_out", K::WriteZero => "write_zero", K::Interrupted => "interrupted",
        K::UnexpectedEof => "unexpected_eof", K::OutOfMemory => "out_of_memory", K::StorageFull => "storage_full",
        _ => "other",
    })
}

/// Como o valor de uma chave do ambiente ou do retrato é conferido.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    List(&'static [&'static str]),
    /// Número de versão público: 1 a 4 segmentos numéricos, com letra final opcional, ou `absent`.
    Number,
    /// Lugar de um pânico no código do Hangar (`crates.hangar-server.src.x:12`) ou `dependency`.
    Site,
}

/// O ambiente da máquina que vai em `Batch::machine`.
pub const MACHINE: &[(&str, Kind)] = &[
    ("os", Kind::List(OS_KINDS)), ("os_name", Kind::List(OS_NAMES)), ("os_version", Kind::Number),
    ("cpus", Kind::List(CPU_BANDS)), ("ram_total", Kind::List(RAM_BANDS)),
    ("mux", Kind::List(MUXES)), ("mux_version", Kind::Number),
    ("cli.claude", Kind::Number), ("cli.codex", Kind::Number), ("cli.pi", Kind::Number), ("cli.omp", Kind::Number),
    ("cli.kimi", Kind::Number), ("cli.antigravity", Kind::Number),
    ("accounts.claude", Kind::List(COUNT_BANDS)), ("accounts.codex", Kind::List(COUNT_BANDS)),
    ("accounts.pi", Kind::List(COUNT_BANDS)), ("accounts.omp", Kind::List(COUNT_BANDS)),
    ("accounts.kimi", Kind::List(COUNT_BANDS)), ("accounts.antigravity", Kind::List(COUNT_BANDS)),
    ("connect", Kind::List(ON_OFF)), ("tailscale", Kind::List(ON_OFF)),
];

/// O retrato de cada falha (`Record::ctx_first`/`ctx_last`). `sessions.<provider>.<surface>` também vale.
pub const CONTEXT: &[(&str, Kind)] = &[
    ("up", Kind::List(UP_VALUES)), ("mem_free", Kind::List(MEM_BANDS)), ("disk_free", Kind::List(DISK_BANDS)),
    ("load", Kind::List(LOAD_BANDS)), ("cpu", Kind::List(CPU_PCT_BANDS)),
    ("sessions", Kind::List(COUNT_BANDS)), ("working", Kind::List(COUNT_BANDS)),
    ("provider", Kind::List(&["claude", "codex", "pi", "omp", "kimi", "antigravity", "other"])),
    ("surface", Kind::List(&["terminal", "headless"])), ("model", Kind::List(ALL_MODELS)),
    ("age", Kind::List(AGE_BANDS)), ("state", Kind::List(STATES)), ("cli", Kind::Number),
    ("client", Kind::List(&["ios", "android", "pwa", "native", "server", "other"])), ("client_version", Kind::Number),
    ("resumed", Kind::List(RESUMED)),
    ("accounts", Kind::List(COUNT_BANDS)), ("no_login", Kind::List(COUNT_BANDS)), ("spent", Kind::List(COUNT_BANDS)),
    ("quota_unread", Kind::List(COUNT_BANDS)), ("full", Kind::List(COUNT_BANDS)),
    ("io", Kind::List(IO_KINDS)), ("panic_at", Kind::Site),
    ("exit", Kind::List(&["code_0", "code_1", "code_other", "sigkill", "sigterm", "sigsegv", "sigabrt", "signal_other", "none"])),
];

const ALL_MODELS: &[&str] = &[
    "default", "best", "opus", "opusplan", "sonnet", "haiku", "fable",
    "gpt-6.2-sol", "gpt-6.1-sol", "gpt-6-sol", "gpt-6-astra", "gpt-6-luna", "gpt-5.6-sol", "gpt-5.6-terra", "gpt-5.6-luna",
    "kimi-k3", "gemini-4-neon", "gemini-3.8-flash", "engine", "other",
];

/// O valor que sai para `key` numa das tabelas: `None` para chave desconhecida (fica fora), `other` para
/// valor fora da lista.
pub fn closed(table: &[(&str, Kind)], key: &str, value: &str) -> Option<String> {
    let kind = table.iter().find(|(known, _)| *known == key).map(|(_, kind)| *kind)
        .or_else(|| table.iter().any(|(known, _)| *known == "sessions").then(|| sessions_key(key)).flatten())?;
    Some(match kind {
        Kind::List(values) => Code::known(value, values).as_str().to_owned(),
        Kind::Number => if value == "absent" || is_number(value) { value.to_owned() } else { "other".to_owned() },
        Kind::Site => if value == "dependency" || (value.starts_with("crates.") && is_name(value)) { value.to_owned() } else { "other".to_owned() },
    })
}

fn sessions_key(key: &str) -> Option<Kind> {
    let mut parts = key.strip_prefix("sessions.")?.split('.');
    let (provider, surface) = (parts.next()?, parts.next()?);
    (parts.next().is_none() && Provider::parse(provider).is_some() && Surface::parse(surface).is_some()).then_some(Kind::List(COUNT_BANDS))
}

fn is_number(value: &str) -> bool {
    let digits = value.strip_suffix(|c: char| c.is_ascii_lowercase()).unwrap_or(value);
    let parts: Vec<&str> = digits.split('.').collect();
    value.len() <= 24 && (1..=4).contains(&parts.len()) && parts.iter().all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
}

/// Faixas: os limites são os do nome (`lt1m` = menos de um minuto).
pub fn count_band(n: u64) -> Code { Code::fixed(match n { 0 => "0", 1 => "1", 2..=3 => "2-3", 4..=9 => "4-9", _ => "ge10" }) }
pub fn age_band(secs: u64) -> Code {
    Code::fixed(match secs { 0..60 => "lt1m", 60..600 => "lt10m", 600..3600 => "lt1h", 3600..86_400 => "lt1d", _ => "ge1d" })
}
pub fn mem_free_band(available: u64, total: u64) -> Option<Code> {
    let pct = (total > 0).then(|| available.saturating_mul(100) / total)?;
    Some(Code::fixed(match pct { 0..5 => "lt5", 5..10 => "lt10", 10..20 => "lt20", 20..40 => "lt40", _ => "ge40" }))
}
pub fn disk_free_band(bytes: u64) -> Code {
    const G: u64 = 1 << 30;
    Code::fixed(match bytes { b if b < G => "lt1g", b if b < 5 * G => "lt5g", b if b < 20 * G => "lt20g", _ => "ge20g" })
}
/// Carga de 1 min dividida pelos núcleos.
pub fn load_band(load1: f64, cpus: u32) -> Option<Code> {
    let per = (cpus > 0 && load1.is_finite()).then(|| load1 / f64::from(cpus))?;
    Some(Code::fixed(if per < 0.5 { "lt0_5" } else if per < 1.0 { "lt1" } else if per < 2.0 { "lt2" } else { "ge2" }))
}
pub fn cpu_pct_band(pct: f32) -> Code { Code::fixed(if pct < 50.0 { "lt50" } else if pct < 80.0 { "lt80" } else { "ge80" }) }
pub fn cpus_band(n: u32) -> Code { Code::fixed(match n { 0..=2 => "1-2", 3..=4 => "3-4", 5..=8 => "5-8", 9..=16 => "9-16", 17..=32 => "17-32", _ => "ge33" }) }
pub fn ram_band(bytes: u64) -> Code {
    const G: u64 = 1 << 30;
    Code::fixed(match bytes / G { 0..4 => "lt4g", 4..8 => "4-8g", 8..16 => "8-16g", 16..32 => "16-32g", 32..64 => "32-64g", _ => "ge64g" })
}

/// Falha que o servidor percebe sozinho: código, evento do diário, causas possíveis e descrição fixa.
pub struct Health { pub code: &'static str, pub event: &'static str, pub causes: &'static [&'static str], pub description: &'static str }

/// As falhas de saúde do servidor. Causa fora da lista do código sai `other`.
pub const HEALTH: &[Health] = &[
    Health { code: "state_facts_unavailable", event: "health.state", description: "O estado da sessão não pôde ser lido; ficou o último valor.",
        causes: &["runtime_unbound", "binding_missing", "binding_changed", "plugin_store_unavailable", "plugin_facts_invalid",
            "permission_store_poisoned", "askq_sidecar_unreadable", "askq_sidecar_malformed", "askq_race", "askq_lock_poisoned",
            "askq_frame_missing", "askq_timeout", "plan_notice_missing", "interaction_id_missing", "files_dirs_missing",
            "files_timeout", "task_panic", "dead_unproven", "plugin_question_invalid", "feed_binding_changed"] },
    Health { code: "terminal_observacao_falhou", event: "health.state", description: "A tela do terminal da sessão não pôde ser lida.",
        causes: &["runtime_unbound", "binding_missing", "binding_changed", "permission_store_poisoned", "observer_spawn_failed",
            "startup_timeout", "command_timeout", "capture_timeout", "observer_died", "observer_io", "mux_command_failed",
            "protocol_error", "target_changed", "target_invalid", "pool_saturated", "mux_unavailable", "mux_process_failed"] },
    Health { code: "permission_observe_failed", event: "health.state", description: "O modo de permissão da sessão não pôde ser confirmado.",
        causes: &["runtime_unbound", "binding_missing", "binding_changed", "frame_lock_poisoned", "store_poisoned",
            "capture_stale", "mode_unconfirmed", "mode_unknown"] },
    Health { code: "plugin_identity_missing", event: "health.plugin", description: "O plugin da sessão Claude não foi reconhecido pelo servidor.",
        causes: &["no_runtime_entry", "duplicate_entry", "terminal_target_stale", "headless_unbound", "name_lookup_failed",
            "key_not_found", "identity_incomplete", "name_mismatch", "key_mismatch", "token_without_key",
            "pull_without_conversation", "pull_not_owner", "adoption_cooldown", "adoption_refused"] },
    Health { code: "work_review_refresh_failed", event: "health.work_review", description: "A revisão do trabalho não pôde ser relida no conector.",
        causes: &["timeout", "unavailable", "auth_required", "permission_denied", "protocol_error", "invalid_params",
            "connector_disabled", "registry_invalid", "registry_unavailable", "mission_unavailable", "mission_unreadable"] },
    Health { code: "limit_switch_no_room", event: "health.limit_switch", description: "A sessão bateu o limite e nenhuma outra conta tinha folga.",
        causes: &["no_other_account", "no_login", "all_spent", "quota_unread", "quota_no_window", "all_full"] },
    Health { code: "headless_processo_caiu", event: "health.headless", description: "O processo da sessão sem terminal terminou sozinho.",
        causes: &["sigkill", "exit_nonzero", "signal_other", "exit_while_working", "cano_died_socket_down", "pipe_closed_no_rc",
            "respawn_failed", "not_logged_in"] },
    Health { code: "headless_caiu", event: "health.headless", description: "O processo da sessão sem terminal terminou sozinho.",
        causes: &["sigkill", "exit_nonzero", "signal_other", "exit_while_working", "cano_died_socket_down", "pipe_closed_no_rc",
            "respawn_failed", "not_logged_in"] },
    Health { code: "server_exit", event: "health.server_exit", description: "O servidor do Hangar parou.",
        causes: &["out_of_memory", "timeout", "signal", "exit_code", "core_dump", "watchdog", "resources", "start_limit_hit",
            "serve_failed", "panic", "none"] },
    Health { code: "erro_criacao_sessao", event: "health.session_open", description: "A sessão pedida não pôde ser criada.",
        causes: &["name_invalid", "headless_provider_unsupported", "headless_read_only", "omp_profile_invalid",
            "permission_mode_invalid", "fast_unsupported", "ultrafast_refused", "branch_name_missing", "root_not_allowed",
            "read_only_unavailable", "read_only_refused", "worktree_undo_failed", "worktree_failed", "git_status_failed",
            "account_invalid", "account_lock_io", "engine_missing", "launcher_missing", "choices_unwritten", "prewarm_unwritten",
            "record_unwritten", "sidecar_unwritten", "first_message_unqueued", "service_tier_unchecked",
            "tmux_new_session_failed", "terminal_died_at_birth", "session_id_invalid", "resume_transcript_missing",
            "provider_not_here", "transfer_index_invalid", "owner_unregistered", "task_panic", "baton_invalid", "prewarm_failed"] },
];

/// Causas de `mcp.tool` com código `tool_failed`: o lugar onde a ferramenta parou.
pub const MCP_CAUSES: &[&str] = &[
    "caller_unknown", "voice_needs_session", "self_target", "invalid_verb_args", "unknown_verb", "shot_dir_failed",
    "desktop_registry_unreadable", "desktop_app_restarting", "desktop_app_closed", "desktop_app_dead", "desktop_app_unreachable",
    "desktop_app_timeout", "browser_not_open", "browser_not_ready", "browser_paused", "browser_verb_refused", "jev_off", "jev_unconfigured",
    "browser_host_none", "browser_host_stale", "browser_host_not_allowed", "browser_host_gone", "browser_host_timeout",
    "browser_host_verb_unsupported", "browser_host_app_outdated",
    "page_gone", "browser_starting", "browser_start_failed", "app_closing", "app_no_answer", "cdp_closed", "dialog_open",
    "queue_deadline", "verb_unconfirmed", "target_missing", "target_ambiguous", "ref_invalid", "wait_timeout",
    "voice_has_no_child", "child_remote_unsupported", "groups_unavailable", "group_file_unreadable", "erro_nao_ligada",
    "erro_permissao_do_dono", "erro_sem_resposta", "erro_resposta_nao_entregue", "erro_envio_falhou",
    "erro_interacao_desatualizada", "erro_sessao_inexistente", "runtime_unhealthy", "erro_corpo_invalido", "locate_timeout",
    "erro_aguarde_clear", "erro_sem_turno", "erro_par_endereco_ambiguo", "erro_par_encerrado", "session_transfer_busy",
    "server_id_missing", "peer_unknown", "peer_refused", "peer_transport", "peer_old_version", "peer_identity_mismatch",
    "headless_provider_unsupported", "invalid_target_name", "target_conflict", "remote_inherit_refused", "remote_account_unknown",
    "placement_forward_refused", "placement_machine_unknown", "placement_uncertain", "erro_nome_em_uso", "erro_cwd_inexistente",
    "erro_conta_sem_login", "erro_provedor_offline", "link_failed",
    "http_400", "http_403", "http_404", "http_409", "http_410", "http_422", "http_429", "http_500", "http_502", "http_503", "http_504",
];

/// Códigos que os apps mandam nas falhas de rede e de lista; status HTTP vira `http_4xx`/`http_5xx`.
pub const CLIENT_CODES: &[&str] = &[
    "route_timeout", "aborted", "type_error", "panic", "rede", "timeout", "silencio", "servidor_fechou", "abertura",
    "http_4xx", "http_5xx", "primeiro_quadro_timeout", "stream_fechado", "stream_interrompido", "json_invalido",
    "produtor_falhou", "conexao", "sincronia", "par_attach",
];

/// O código de recurso de um pedido; `None` quando ele não conta.
pub fn feature(method: &str, route: &str) -> Option<Code> {
    FEATURE_ROUTES.iter().find(|(m, template, _)| *m == method && *template == route).map(|(_, _, code)| Code(*code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_values_pass_only_through_their_table() {
        assert_eq!(closed(CONTEXT, "mem_free", "lt10").as_deref(), Some("lt10"));
        assert_eq!(closed(CONTEXT, "mem_free", "12%").as_deref(), Some("other"));
        assert_eq!(closed(CONTEXT, "nome_da_sessao", "x"), None, "chave fora da tabela fica fora");
        assert_eq!(closed(CONTEXT, "sessions.claude.headless", "2-3").as_deref(), Some("2-3"));
        assert_eq!(closed(CONTEXT, "sessions.meu-motor.headless", "2-3"), None);
        assert_eq!(closed(CONTEXT, "model", "meu-motor").as_deref(), Some("other"));
        assert_eq!(closed(MACHINE, "sessions.claude.terminal", "1"), None, "o padrão de sessões só vale no retrato");
        assert_eq!(closed(MACHINE, "cli.claude", "2.1.195").as_deref(), Some("2.1.195"));
        assert_eq!(closed(MACHINE, "mux_version", "3.5a").as_deref(), Some("3.5a"));
        assert_eq!(closed(MACHINE, "cli.codex", "absent").as_deref(), Some("absent"));
        assert_eq!(closed(MACHINE, "cli.codex", "codex-cli 0.120.0").as_deref(), Some("other"));
        assert_eq!(closed(MACHINE, "os_version", "24.04").as_deref(), Some("24.04"));
        assert_eq!(closed(MACHINE, "os_name", "Minha Distro").as_deref(), Some("other"));
        assert_eq!(closed(CONTEXT, "panic_at", "crates.hangar-server.src.runtime.claude:12").as_deref(), Some("crates.hangar-server.src.runtime.claude:12"));
        assert_eq!(closed(CONTEXT, "panic_at", "home.jefferson.cargo:1").as_deref(), Some("other"));
        for (table, _) in [(MACHINE, 0), (CONTEXT, 1)] {
            for (key, kind) in table {
                assert!(is_name(key), "{key}");
                if let Kind::List(values) = kind { assert!(values.iter().all(|v| is_name(v)), "{key}"); }
            }
        }
        for health in HEALTH {
            assert!(is_name(health.code) && is_name(health.event) && health.causes.iter().all(|c| is_name(c)), "{}", health.code);
        }
        assert!(MCP_CAUSES.iter().chain(CLIENT_CODES).all(|c| is_name(c)));
    }

    #[test]
    fn bands_follow_their_names() {
        assert_eq!(count_band(0).as_str(), "0");
        assert_eq!(count_band(3).as_str(), "2-3");
        assert_eq!(count_band(10).as_str(), "ge10");
        assert_eq!(age_band(59).as_str(), "lt1m");
        assert_eq!(age_band(3600).as_str(), "lt1d");
        assert_eq!(mem_free_band(9, 100).map(Code::as_str), Some("lt10"));
        assert_eq!(mem_free_band(1, 0), None);
        assert_eq!(disk_free_band(4 << 30).as_str(), "lt5g");
        assert_eq!(load_band(6.0, 4).map(Code::as_str), Some("lt2"));
        assert_eq!(cpus_band(12).as_str(), "9-16");
        assert_eq!(ram_band(31 << 30).as_str(), "16-32g");
        assert_eq!(io_kind(std::io::ErrorKind::TimedOut).as_str(), "timed_out");
    }

    #[test]
    fn unknown_code_is_other() {
        assert_eq!(Code::known("x", &["a"]), Code::OTHER);
        assert_eq!(Code::known("a", &["a"]).as_str(), "a");
        assert_eq!(Code::known("Bad", &["Bad"]), Code::OTHER);
        assert_eq!(Code::known("", &[]), Code::OTHER);
        const FIXED: Code = Code::fixed("session.answer");
        assert_eq!(FIXED.as_str(), "session.answer");
        assert_eq!(Code::fixed("Bad"), Code::OTHER);
        assert_eq!(Code::fixed(""), Code::OTHER);
    }

    #[test]
    fn engine_never_leaves_its_name() {
        assert_eq!(model(Provider::Claude, "meu-motor", true), Code::ENGINE);
        assert_eq!(model(Provider::Claude, "opus", true), Code::ENGINE);
        assert_eq!(model(Provider::Claude, "opus", false).as_str(), "opus");
        assert_eq!(model(Provider::Claude, "meu-motor", false), Code::OTHER);
        assert_eq!(model(Provider::Pi, "anthropic/claude-x", false), Code::OTHER);
    }

    #[test]
    fn every_wire_name_fits_the_receiver_format() {
        let mut names: Vec<&str> = Event::ALL.iter().map(|e| e.as_str()).collect();
        names.extend(Provider::ALL.iter().map(|v| v.as_str()));
        names.extend(Surface::ALL.iter().map(|v| v.as_str()));
        names.extend(ClientKind::ALL.iter().map(|v| v.as_str()));
        names.extend(SenderOrigin::ALL.iter().map(|v| v.as_str()));
        names.extend(OpenedBy::ALL.iter().map(|v| v.as_str()));
        names.extend(UpdateStepName::ALL.iter().map(|v| v.as_str()));
        names.extend(UpdateOrigin::ALL.iter().map(|v| v.as_str()));
        names.extend(PeerKind::ALL.iter().map(|v| v.as_str()));
        names.extend(PlacementOutcome::ALL.iter().map(|v| v.as_str()));
        names.extend(PLACEMENT_REASONS);
        names.extend(LogLevel::ALL.iter().map(|v| v.as_str()));
        names.extend(FEATURE_ROUTES.iter().map(|(_, _, code)| *code));
        names.extend([CLAUDE_MODELS, CODEX_MODELS, KIMI_MODELS, ANTIGRAVITY_MODELS].concat());
        for (setting, values) in SETTINGS {
            names.push(setting);
            names.extend(values.iter().copied());
        }
        for name in names {
            assert!(is_name(name), "{name}");
        }
        let unique: std::collections::BTreeSet<&str> = Event::ALL.iter().map(|e| e.as_str()).collect();
        assert_eq!(unique.len(), Event::ALL.len());
    }

    #[test]
    fn settings_pass_only_through_their_list() {
        let pair = |name, value| setting(name, value).map(|(s, v)| (s.as_str(), v.as_str()));
        assert_eq!(pair("theme", "dark"), Some(("theme", "dark")));
        assert_eq!(pair("theme", "#ff0000"), Some(("theme", "other")));
        assert_eq!(pair("font", "custom"), Some(("font", "custom")));
        assert_eq!(pair("font", "Fira Code"), Some(("font", "other")), "nome de fonte nunca sai");
        assert_eq!(pair("default_provider", "meu-motor"), Some(("default_provider", "other")));
        assert_eq!(pair("modelo", "opus"), None);
        let unique: std::collections::BTreeSet<&str> = SETTINGS.iter().map(|(s, _)| *s).collect();
        assert_eq!(unique.len(), SETTINGS.len());
    }

    #[test]
    fn feature_counts_only_listed_method_and_template() {
        assert_eq!(feature("GET", "/api/sessions/{name}/navegador").map(Code::as_str), Some("session.browser"));
        assert_eq!(feature("GET", "/api/sessions/{name}/history"), None);
        assert_eq!(feature("GET", "/api/sessions/minha-sessao/navegador"), None);
        assert_eq!(feature("PUT", "/api/tags").map(Code::as_str), Some("tags"));
        assert_eq!(feature("GET", "/api/tags"), None);
        assert_eq!(feature("GET", "/api/vault/entries"), None);
        let mut seen = std::collections::BTreeSet::new();
        for (method, template, _) in FEATURE_ROUTES {
            assert!(matches!(*method, "GET" | "POST" | "PUT" | "PATCH" | "DELETE"), "{method}");
            assert!(seen.insert((*method, *template)), "repetida: {method} {template}");
        }
    }

    #[test]
    fn parse_and_fallbacks() {
        assert_eq!(Provider::parse("codex"), Some(Provider::Codex));
        assert_eq!(Provider::parse("Codex"), None);
        assert_eq!(Event::parse("message_sent"), Some(Event::MessageSent));
        assert_eq!(UpdateOrigin::parse("cron").unwrap_or(UpdateOrigin::Other), UpdateOrigin::Other);
        assert_eq!(UpdateStepName::from_stage("baixar"), UpdateStepName::Download);
        assert_eq!(UpdateStepName::from_stage("prova"), UpdateStepName::Other);
        assert_eq!(DimValue::version("0.42.0").as_str(), "0.42.0");
        assert_eq!(DimValue::version("../etc"), DimValue::OTHER);
        assert_eq!(DimValue::version("dev build"), DimValue::OTHER);
        assert_eq!(DimValue::version("0.1.0.9010").as_str(), "0.1.0.9010");
        assert_eq!(DimValue::version("1-cliente-acme"), DimValue::OTHER);
        assert_eq!(DimValue::version("1.2.3-acme"), DimValue::OTHER);
        assert_eq!(DimValue::from(true).as_str(), "true");
    }
}
