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
    LogDropped => "log_dropped",
    Settings => "settings",
    DiaryDropped => "diary_dropped",
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

/// O código de recurso de um pedido; `None` quando ele não conta.
pub fn feature(method: &str, route: &str) -> Option<Code> {
    FEATURE_ROUTES.iter().find(|(m, template, _)| *m == method && *template == route).map(|(_, _, code)| Code(*code))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        names.extend(UpdateStepName::ALL.iter().map(|v| v.as_str()));
        names.extend(UpdateOrigin::ALL.iter().map(|v| v.as_str()));
        names.extend(PeerKind::ALL.iter().map(|v| v.as_str()));
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
