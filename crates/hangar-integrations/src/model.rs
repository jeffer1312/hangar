//! Modelos de domínio e regras de validação das integrações MCP.

use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use hangar_core::ids::valid_slug;

const MAX_SLUG_BYTES: usize = 64;
const MAX_NAME_BYTES: usize = 200;
const MAX_PRESET_BYTES: usize = 128;
const MAX_URL_BYTES: usize = 2048;
const MAX_HEADER_NAME_BYTES: usize = 256;
const MAX_ARG_BYTES: usize = 4096;
const MAX_CLIENT_ID_BYTES: usize = 512;
const MAX_PROJECT_BYTES: usize = 4096;
const MAX_TOOL_NAME_BYTES: usize = 256;

/// Erro de validação de domínio ao criar ou atualizar uma integração.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ValidationError {
    InvalidId,
    InvalidName,
    InvalidPreset,
    InvalidTransport(String),
    InvalidAuth(String),
    InvalidProject,
    InvalidAllowedTool,
    InvalidTimestamp,
}

/// Configuração persistida de uma integração MCP (sempre livre de segredos).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IntegrationConfig {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    pub transport: Transport,
    pub auth: AuthConfig,
    #[serde(default)]
    pub policy: PermissionPolicy,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub allowed_tools: BTreeSet<String>,
    pub created_at: f64,
    pub updated_at: f64,
}

/// Transporte de comunicação com o servidor MCP externo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Transport {
    RemoteHttp {
        url: String,
        #[serde(default)]
        header_names: Vec<String>,
    },
    LocalStdio {
        command: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cwd: Option<PathBuf>,
        #[serde(default)]
        env_keys: Vec<String>,
    },
}

/// Configuração pública de autenticação (chaves e tokens ficam somente em `secrets.bin`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AuthConfig {
    None,
    Bearer,
    #[serde(rename = "oauth_dcr")]
    OAuthDcr,
    #[serde(rename = "oauth_manual_client")]
    OAuthManualClient {
        client_id: String,
        #[serde(default)]
        scopes: Vec<String>,
    },
}

/// Política de aprovação das ferramentas expostas pela integração.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPolicy {
    #[default]
    ReadFreeAskWrite,
    AllowAll,
    ReadOnly,
}

/// Estado operacional calculado para uma integração.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationStatus {
    Ready,
    AuthRequired,
    AuthExpired,
    Disabled,
    Error,
}

/// Nível efetivo de acesso de uma ferramenta após aplicar política e liberações fixas.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolAccess {
    Read,
    AllowedAlways,
    AskWrite,
    Blocked,
}

/// Resumo público de uma ferramenta descoberta no servidor MCP.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSummary {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    pub effective_access: ToolAccess,
}

/// Decisão do usuário para um pedido de aprovação de execução de ferramenta.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalDecision {
    AllowOnce,
    AllowAlways,
    Deny,
}

impl IntegrationConfig {
    /// Valida todos os campos de configuração antes de persistir em `config.json`.
    ///
    /// # Errors
    ///
    /// Retorna [`ValidationError`] quando o `id`, nome, preset, transporte, autenticação,
    /// projetos, ferramentas liberadas ou timestamps violarem os limites do domínio.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !valid_slug(&self.id, MAX_SLUG_BYTES) {
            return Err(ValidationError::InvalidId);
        }
        if !valid_bounded_text(&self.name, MAX_NAME_BYTES) {
            return Err(ValidationError::InvalidName);
        }
        if let Some(preset) = &self.preset
            && !valid_bounded_text(preset, MAX_PRESET_BYTES)
        {
            return Err(ValidationError::InvalidPreset);
        }
        self.transport.validate()?;
        self.auth.validate()?;
        if self
            .projects
            .iter()
            .any(|project| !valid_bounded_text(project, MAX_PROJECT_BYTES))
        {
            return Err(ValidationError::InvalidProject);
        }
        if self.allowed_tools.iter().any(|tool| !valid_tool_name(tool)) {
            return Err(ValidationError::InvalidAllowedTool);
        }
        if !self.created_at.is_finite() || !self.updated_at.is_finite() {
            return Err(ValidationError::InvalidTimestamp);
        }
        Ok(())
    }
}

impl Transport {
    /// Valida os parâmetros públicos do transporte sem aceitar segredos ou entradas malformadas.
    ///
    /// # Errors
    ///
    /// Retorna [`ValidationError::InvalidTransport`] se a URL, cabeçalhos, comando, pasta ou
    /// nomes de variáveis de ambiente estiverem inválidos.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::RemoteHttp { url, header_names } => {
                if !valid_http_url(url) {
                    return Err(ValidationError::InvalidTransport(
                        "URL deve usar http:// ou https:// com host válido e sem espaços/NUL"
                            .into(),
                    ));
                }
                if header_names.iter().any(|name| !valid_header_name(name)) {
                    return Err(ValidationError::InvalidTransport(
                        "nome de cabeçalho HTTP inválido".into(),
                    ));
                }
                Ok(())
            }
            Self::LocalStdio {
                command,
                cwd,
                env_keys,
            } => {
                if command.is_empty() || command[0].trim().is_empty() {
                    return Err(ValidationError::InvalidTransport(
                        "comando stdio não pode estar vazio".into(),
                    ));
                }
                if command.iter().any(|part| {
                    part.is_empty() || part.contains('\0') || part.len() > MAX_ARG_BYTES
                }) {
                    return Err(ValidationError::InvalidTransport(
                        "argumento do comando vazio, com NUL ou longo demais".into(),
                    ));
                }
                if let Some(dir) = cwd
                    && (dir.as_os_str().is_empty() || dir.to_string_lossy().contains('\0'))
                {
                    return Err(ValidationError::InvalidTransport(
                        "diretório de trabalho inválido".into(),
                    ));
                }
                if env_keys.iter().any(|key| !valid_env_key(key)) {
                    return Err(ValidationError::InvalidTransport(
                        "nome de variável de ambiente inválido".into(),
                    ));
                }
                Ok(())
            }
        }
    }
}

impl AuthConfig {
    /// Valida a configuração pública de autenticação.
    ///
    /// # Errors
    ///
    /// Retorna [`ValidationError::InvalidAuth`] se `OAuthManualClient` tiver `client_id` ou
    /// escopo vazio, com NUL ou acima do limite.
    pub fn validate(&self) -> Result<(), ValidationError> {
        match self {
            Self::None | Self::Bearer | Self::OAuthDcr => Ok(()),
            Self::OAuthManualClient { client_id, scopes } => {
                if !valid_bounded_text(client_id, MAX_CLIENT_ID_BYTES) {
                    return Err(ValidationError::InvalidAuth(
                        "client_id vazio, com NUL ou longo demais".into(),
                    ));
                }
                if scopes
                    .iter()
                    .any(|scope| !valid_bounded_text(scope, MAX_ARG_BYTES))
                {
                    return Err(ValidationError::InvalidAuth(
                        "escopo OAuth vazio, com NUL ou longo demais".into(),
                    ));
                }
                Ok(())
            }
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidId => write!(f, "id inválido: use slug [a-z][a-z0-9-]{{0,63}}"),
            Self::InvalidName => write!(f, "nome deve ter entre 1 e 200 bytes sem NUL"),
            Self::InvalidPreset => write!(f, "preset inválido"),
            Self::InvalidTransport(reason) => write!(f, "transporte inválido: {reason}"),
            Self::InvalidAuth(reason) => write!(f, "autenticação inválida: {reason}"),
            Self::InvalidProject => write!(f, "identificador de projeto inválido"),
            Self::InvalidAllowedTool => write!(f, "nome de ferramenta inválido"),
            Self::InvalidTimestamp => write!(f, "timestamp inválido"),
        }
    }
}

impl std::error::Error for ValidationError {}

pub(crate) fn valid_tool_name(name: &str) -> bool {
    valid_bounded_text(name, MAX_TOOL_NAME_BYTES)
}

fn default_enabled() -> bool {
    true
}

fn valid_bounded_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.contains('\0')
}

fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_HEADER_NAME_BYTES
        && name.is_ascii()
        && !name
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || ch == ':')
}

fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_HEADER_NAME_BYTES
        && !key.contains('=')
        && !key.chars().any(|ch| ch.is_whitespace() || ch.is_control())
}

fn valid_http_url(url: &str) -> bool {
    if url.is_empty()
        || url.len() > MAX_URL_BYTES
        || url.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return false;
    }
    let Some(rest) = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Impede userinfo embutida (`user:pass@host`) para evitar segredos na URL persistida.
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    if let Some(after_open) = authority.strip_prefix('[') {
        let Some((host, tail)) = after_open.split_once(']') else {
            return false;
        };
        if host.is_empty()
            || !host
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() || ch == ':' || ch == '.')
        {
            return false;
        }
        return valid_optional_port(tail);
    }
    match authority.split_once(':') {
        Some((host, port)) => valid_hostname(host) && valid_port(port),
        None => valid_hostname(authority),
    }
}

fn valid_hostname(host: &str) -> bool {
    !host.is_empty()
        && !host.starts_with('.')
        && !host.ends_with('.')
        && !host.contains("..")
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '.' || ch == '-')
}

fn valid_optional_port(tail: &str) -> bool {
    if tail.is_empty() {
        return true;
    }
    tail.strip_prefix(':').is_some_and(valid_port)
}

fn valid_port(port: &str) -> bool {
    !port.is_empty()
        && port.chars().all(|ch| ch.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|value| value > 0)
}
