//! Abertura de sessão única: `SessionOpenRequest` (o pedido no fio, o `CreateBody` do Python mais
//! `inherit`) → `resolve_open()` → `SessionSpec` (tudo decidido, gravado por sessão). Reabrir é o
//! spec gravado ⊕ `SessionChange`. Spec: `docs/superpowers/specs/2026-10-15-unified-session-open.md`.
pub mod adopt;
pub mod agents;
pub mod archive;
pub mod claude;
pub mod cli;
pub mod codex;
#[cfg(target_os = "linux")]
#[doc(hidden)]
pub mod contract;
pub mod facts;
pub mod legacy_restore;
pub mod local;
pub mod open;
pub mod record;
pub mod resolve;
pub mod restore;
pub mod server;
pub mod terminal;
#[cfg(test)]
mod tests;

pub use resolve::{OpenCaller, OpenFacts, Resolved, resolve_open};
pub use server::create;

use indexmap::IndexMap;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};

/// Agente da sessão. O `session_write::Provider` (Claude e Codex) é o recorte que o Rust escreve.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentProvider { Claude, Codex, Pi, Omp, Kimi }

impl AgentProvider {
    /// Também a ordem de preferência do provedor padrão (`session_defaults.choose_provider`).
    pub const ALL: [AgentProvider; 5] = [Self::Claude, Self::Codex, Self::Pi, Self::Omp, Self::Kimi];

    pub fn name(self) -> &'static str {
        match self { Self::Claude => "claude", Self::Codex => "codex", Self::Pi => "pi", Self::Omp => "omp", Self::Kimi => "kimi" }
    }

    pub fn parse(name: &str) -> Option<Self> { Self::ALL.into_iter().find(|p| p.name() == name) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface { Terminal, Headless }

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ServiceTier { Default, Priority }

impl ServiceTier {
    pub fn name(self) -> &'static str { match self { Self::Default => "default", Self::Priority => "priority" } }
}

/// Modo de permissão: os do `claude --help` e os do picker `/permissions` do Codex.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PermissionMode {
    #[serde(rename = "acceptEdits")] AcceptEdits,
    #[serde(rename = "auto")] Auto,
    #[serde(rename = "bypassPermissions")] BypassPermissions,
    #[serde(rename = "manual")] Manual,
    #[serde(rename = "dontAsk")] DontAsk,
    #[serde(rename = "plan")] Plan,
    #[serde(rename = "Ask for approval")] AskForApproval,
    #[serde(rename = "Approve for me")] ApproveForMe,
    #[serde(rename = "Full Access")] FullAccess,
}

impl PermissionMode {
    /// Na ordem do `MODOS_PERMISSAO_CLAUDE`: a mensagem de recusa lista assim.
    pub const CLAUDE: [Self; 6] = [Self::AcceptEdits, Self::Auto, Self::BypassPermissions, Self::Manual, Self::DontAsk, Self::Plan];
    pub const CODEX: [Self; 3] = [Self::AskForApproval, Self::ApproveForMe, Self::FullAccess];

    pub fn name(self) -> &'static str {
        match self {
            Self::AcceptEdits => "acceptEdits", Self::Auto => "auto", Self::BypassPermissions => "bypassPermissions",
            Self::Manual => "manual", Self::DontAsk => "dontAsk", Self::Plan => "plan",
            Self::AskForApproval => "Ask for approval", Self::ApproveForMe => "Approve for me", Self::FullAccess => "Full Access",
        }
    }

    pub fn claude(name: &str) -> Option<Self> { Self::CLAUDE.into_iter().find(|m| m.name() == name) }

    /// Sem diferenciar maiúscula nem espaço nas pontas, como o `sem_terminal.MODOS` do Python.
    pub fn codex(name: &str) -> Option<Self> {
        let name = name.trim();
        Self::CODEX.into_iter().find(|m| m.name().eq_ignore_ascii_case(name))
    }
}

/// De onde vem a conta, credencial e configuração da sessão. Tipo fechado: cada provedor tem a sua.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Account {
    /// Conta padrão do provedor (Claude: a do servidor; Pi, omp e Kimi sem pasta própria).
    Default,
    /// Pasta de conta Claude escolhida; Pi e omp leem a mesma pasta.
    Claude { config_dir: String },
    Codex { account_id: String },
    Omp { profile: String, config_dir: Option<String> },
    /// Claude num motor do `engines.json`; `account` é a conta ChatGPT fixa do CLIProxyAPI.
    /// `context_window_unknown`: o catálogo do motor não respondeu na abertura; a janela é
    /// relida a cada reabertura (`SessionSpec::refresh_engine_window`).
    Engine {
        name: String, account: Option<String>, config_dir: Option<String>, context_window: Option<u64>,
        #[serde(default, skip_serializing_if = "is_false")]
        context_window_unknown: bool,
    },
}

impl Account {
    /// Pasta de conta Claude que a sessão lê (`None` = a padrão).
    pub fn config_dir(&self) -> Option<&str> {
        match self {
            Self::Claude { config_dir } => Some(config_dir),
            Self::Omp { config_dir, .. } | Self::Engine { config_dir, .. } => config_dir.as_deref(),
            Self::Default | Self::Codex { .. } => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AccountSource { Explicit, Inherited, Default, Quota }

impl AccountSource {
    /// `account_source` do `CreatedSessionInfo` (`models.py:224`): só herança e cota aparecem.
    pub fn wire(self) -> Option<&'static str> {
        match self { Self::Inherited => Some("inherited"), Self::Quota => Some("quota"), Self::Explicit | Self::Default => None }
    }
}

/// `ClaudeCustomizationBody`: só plugins, skills e skills bloqueadas; nada de `env` livre.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaudeCustomizations {
    #[serde(default)]
    pub plugins: IndexMap<String, bool>,
    #[serde(default)]
    pub skills: IndexMap<String, bool>,
    #[serde(default)]
    pub blocked_skills: Vec<String>,
}

/// O pedido de abertura no fio: os 23 campos do `CreateBody` (`api.py`) mais `inherit`.
/// Ausente e `null` são casos diferentes onde o Python os distingue: `provider` ausente é o padrão
/// do servidor e `null` é recusado; booleanos não aceitam `null` nem texto.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionOpenRequest {
    /// Omitido: o servidor deriva da pasta.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub new_branch: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_dir: Option<String>,
    #[serde(default, deserialize_with = "present", skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub remember_provider: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_account: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<ServiceTier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_customizations: Option<ClaudeCustomizations>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jev: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub omp_profile: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub read_only: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headless: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creator: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inherit: Option<bool>,
    /// Liga a sessão nova à criadora: entra no grupo dela.
    #[serde(default, skip_serializing_if = "is_false")]
    pub link: bool,
    /// A reserva pré-aquecida (`PUT /api/sessions/prewarm`) que esta abertura adota se couber.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prewarm: Option<String>,
    /// Fora do fio: o nome derivado da pasta que só uma reserva pré-aquecida segura fica para esta
    /// abertura (a reserva sai). A abertura do dono liga; a resolução da própria reserva não.
    #[serde(skip)]
    pub take_reserved_name: bool,
    /// Fora do fio: a conversa retomada mora na pasta desta conta e não muda com ela (Codex), então
    /// a regra dos 95% não a troca.
    #[serde(skip)]
    pub account_bound: bool,
}

fn is_false(value: &bool) -> bool { !value }

/// Presente vira `Some`; `null` é recusado (o `provider: str` do Pydantic).
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

impl SessionOpenRequest {
    /// Corpo estrito; o que não serve é 422 `erro_corpo_invalido`, sem nada decidido.
    pub fn parse(body: &[u8]) -> Result<Self, OpenError> {
        // Valor primeiro: chave repetida fica com o último valor, como o `json.loads`.
        let value: Value = serde_json::from_slice(body).map_err(|e| OpenError::invalid_body(&e.to_string()))?;
        let request: Self = serde_json::from_value(value).map_err(|e| OpenError::invalid_body(&e.to_string()))?;
        // `min_length=1` do Pydantic: só o texto vazio cai, espaço passa.
        let short = [("name", request.name.as_ref()), ("cwd", Some(&request.cwd)), ("branch", request.branch.as_ref()),
            ("base", request.base.as_ref()), ("creator", request.creator.as_ref())]
            .into_iter().find(|(_, value)| value.is_some_and(|v| v.is_empty()));
        if let Some((field, _)) = short {
            return Err(OpenError::invalid_body(&format!("{field}: texto vazio")));
        }
        Ok(request)
    }

    /// O vínculo pedido, decidido antes de abrir; `Err` recusa a abertura.
    pub fn link_plan(&self, caller: &OpenCaller) -> Result<LinkPlan, OpenError> {
        if !self.link { return Ok(LinkPlan::No); }
        // O grupo é local: do outro lado, a criadora seria uma sessão homônima de lá.
        if [self.name.as_deref(), self.creator.as_deref()].into_iter().flatten().any(crate::groups::local::is_remote) {
            return Err(OpenError::new(400, "erro_link_remoto", "a ligação só vale entre sessões desta máquina"));
        }
        // O convidado nunca entra no grupo de uma sessão do dono.
        if matches!(caller, OpenCaller::Guest { .. }) {
            return Err(OpenError::new(403, "erro_link_convidado", "convidado não liga sessões"));
        }
        Ok(match self.creator.as_deref() {
            Some(creator) => LinkPlan::Join(creator.to_owned()),
            None => LinkPlan::Unlinked("erro_link_sem_criador", "sem sessão criadora, a sessão abriu sem ligação"),
        })
    }
}

/// O que fazer com o vínculo depois de abrir: nada, entrar no grupo da criadora, ou só avisar
/// (código e detalhe do `link_error`).
#[derive(Clone, Debug, PartialEq)]
pub enum LinkPlan { No, Join(String), Unlinked(&'static str, &'static str) }

/// Recusa da abertura no envelope do Python: `{"detail": {code, params, msg}}`, ou `{"detail": msg}`
/// nos 400 que o Python manda sem código (`model_args`).
#[derive(Clone, Debug, PartialEq)]
pub struct OpenError {
    pub status: u16,
    pub code: Option<&'static str>,
    pub msg: String,
    pub params: Value,
}

impl OpenError {
    pub fn new(status: u16, code: &'static str, msg: impl Into<String>) -> Self {
        Self { status, code: Some(code), msg: msg.into(), params: json!({}) }
    }

    pub fn with_params(mut self, params: Value) -> Self {
        self.params = params;
        self
    }

    pub fn plain(status: u16, msg: impl Into<String>) -> Self {
        Self { status, code: None, msg: msg.into(), params: json!({}) }
    }

    pub fn invalid_body(detail: &str) -> Self {
        Self::new(422, "erro_corpo_invalido", format!("Pedido em formato inválido: {detail}"))
    }

    pub fn body(&self) -> Value {
        match self.code {
            Some(code) => crate::session_write::detail_body(code, &self.msg, self.params.clone()),
            None => json!({"detail": self.msg}),
        }
    }
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.status, self.code.unwrap_or("-"), self.msg)
    }
}

impl std::error::Error for OpenError {}

/// Versão do registro gravado: leitor de versão desconhecida recusa em vez de adivinhar.
pub const SPEC_VERSION: u32 = 1;

/// Worktree pedida; `path` só existe depois que a abertura a cria.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorktreeSpec {
    pub branch: String,
    pub new_branch: bool,
    pub base: Option<String>,
    pub path: Option<String>,
}

/// A sessão decidida: nada fica implícito. Ausente em `model`, `effort` e `service_tier` quer dizer
/// "o padrão do binário"; o modo de permissão do Claude e do Codex sai sempre explícito.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionSpec {
    pub version: u32,
    pub key: String,
    pub name: String,
    pub cwd: String,
    pub provider: AgentProvider,
    pub surface: Surface,
    pub account: Account,
    pub account_source: AccountSource,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub service_tier: Option<ServiceTier>,
    pub permission_mode: Option<PermissionMode>,
    pub subagent_model: Option<String>,
    pub customizations: Option<ClaudeCustomizations>,
    pub jev: bool,
    pub read_only: bool,
    pub worktree: Option<WorktreeSpec>,
    pub creator: Option<String>,
    pub created_at: f64,
    /// A conversa com que a vida de agora nasceu (`--session-id`/`--resume` do Claude, o id do Pi e do
    /// omp, a do Kimi retomado): o restore depois do reboot volta nela.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// O nome veio da pasta (`folder_name`), não de alguém. Ausente = registro anterior à marca: vale o
    /// padrão do nome (`rename_suggestion::is_generic`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_auto: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_suggestion: Option<String>,
    /// A sugestão saiu com pouco contexto, de uma conversa com este número de pedidos: um pedido a
    /// mais a refaz.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rename_weak_at: Option<u32>,
    /// "Agora não" do dono: a sugestão não volta nesta sessão.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rename_dismissed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeReason { ModeSwitch, AccountSwitch, Bypass, Reload, Resume, RebootRestore, ParkWake, LimitSwitch }

/// O que uma reabertura muda; o campo ausente fica como está no spec gravado.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SessionChange {
    pub reason: ChangeReason,
    #[serde(default)]
    pub surface: Option<Surface>,
    #[serde(default)]
    pub account: Option<Account>,
    #[serde(default)]
    pub permission_mode: Option<PermissionMode>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    #[serde(default)]
    pub service_tier: Option<ServiceTier>,
}

impl SessionSpec {
    /// Janela do motor que ficou desconhecida é lida de novo; o catálogo ainda fora a deixa
    /// desconhecida, e a sessão abre sem ela, como na criação.
    pub fn refresh_engine_window(&mut self, facts: &dyn OpenFacts) {
        let Account::Engine { name, account, context_window, context_window_unknown: unknown @ true, .. } = &mut self.account else { return };
        let Some(model) = self.model.as_deref().filter(|m| !m.is_empty()) else { return };
        if let Ok(window) = facts.engine_window(name, model, account.as_deref()) {
            *context_window = window;
            *unknown = false;
        }
    }

    /// Read-only estacionada à espera do terminal protegido: trocar de conta a reabre no terminal.
    pub fn parked(&self) -> bool { self.read_only && self.surface == Surface::Headless }

    /// Reabrir = spec gravado ⊕ mudança.
    pub fn with(&self, change: &SessionChange) -> SessionSpec {
        let mut next = self.clone();
        if let Some(surface) = change.surface { next.surface = surface; }
        if let Some(account) = &change.account { next.account = account.clone(); }
        if let Some(mode) = change.permission_mode { next.permission_mode = Some(mode); }
        if let Some(model) = &change.model { next.model = Some(model.clone()); }
        if let Some(effort) = &change.effort { next.effort = Some(effort.clone()); }
        if let Some(tier) = change.service_tier { next.service_tier = Some(tier); }
        next
    }
}
