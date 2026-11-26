//! Modelos de domínio e regras de validação das integrações MCP.

use std::collections::BTreeSet;
use std::fmt;
use std::net::Ipv6Addr;
use std::path::PathBuf;

use ring::digest::{Context as DigestContext, SHA256};
use serde::{Deserialize, Serialize};

use hangar_core::ids::valid_slug;

use crate::secrets::IntegrationSecrets;

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
    /// Normaliza campos normalizáveis da configuração antes de validar ou persistir.
    pub fn normalize(&mut self) {
        self.auth.normalize();
    }

    /// Consome a configuração e retorna a cópia normalizada.
    pub fn normalized(mut self) -> Self {
        self.normalize();
        self
    }

    /// Valida todos os campos de configuração antes de persistir em `config.json`.
    ///
    /// # Errors
    ///
    /// Retorna [`ValidationError`] quando o `id`, nome, preset, transporte, autenticação,
    /// projetos, ferramentas liberadas ou timestamps violarem os limites do domínio.
    pub fn validate(&self) -> Result<(), ValidationError> {
        if !valid_slug(&self.id, MAX_SLUG_BYTES)
            || matches!(self.id.as_str(), "catalog" | "oauth" | "approvals")
        {
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
        if let Transport::RemoteHttp { url, header_names } = &self.transport
            && is_plaintext_non_loopback_http(url)
            && (!matches!(self.auth, AuthConfig::None) || !header_names.is_empty())
        {
            return Err(ValidationError::InvalidTransport(
                "URL http:// fora de loopback (localhost, 127.0.0.1, ::1) não permite autenticação nem cabeçalhos extras; use https://"
                    .into(),
            ));
        }
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
    /// Normaliza campos textuais da configuração de autenticação (remove espaços nas bordas de `client_id`).
    pub fn normalize(&mut self) {
        if let Self::OAuthManualClient { client_id, .. } = self {
            let trimmed = client_id.trim();
            if trimmed.len() != client_id.len() {
                *client_id = trimmed.to_owned();
            }
        }
    }

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

/// Valida em conjunto a configuração pública e os segredos da integração.
///
/// Rejeita qualquer configuração inválida ou URL `http://` não-loopback que carregue
/// credenciais (`bearer_token`, `oauth`) ou cabeçalhos secretos (`secrets.headers`),
/// garantindo regra fail-closed única entre a API HTTP e o cliente MCP.
///
/// # Errors
///
/// Retorna mensagem legível em português quando a validação da configuração ou dos segredos falha.
pub fn validate_config_and_secrets(
    config: &IntegrationConfig,
    secrets: &IntegrationSecrets,
) -> Result<(), String> {
    config.validate().map_err(|err| err.to_string())?;
    if let Transport::RemoteHttp { url, .. } = &config.transport
        && is_plaintext_non_loopback_http(url)
        && (!secrets.headers.is_empty()
            || secrets
                .bearer_token
                .as_ref()
                .is_some_and(|tok| !tok.trim().is_empty())
            || secrets.oauth.as_ref().is_some_and(|oauth| {
                !oauth.access_token.trim().is_empty()
                    || oauth
                        .refresh_token
                        .as_ref()
                        .is_some_and(|rt| !rt.trim().is_empty())
                    || oauth.registered_client_secret.is_some()
            }))
    {
        return Err(
            "URL http:// fora de loopback (localhost, 127.0.0.1, ::1) não permite autenticação nem cabeçalhos extras; use https://"
                .into(),
        );
    }
    Ok(())
}

/// Calcula a impressão digital SHA-256 de destino e autenticação da integração.
///
/// Cobre transporte (`RemoteHttp.url` canônica ou `LocalStdio.command`/`cwd`) e `auth`
/// (normalizando `client_id` com `.trim()`), permitindo detectar edição de endpoint/credencial
/// durante fluxos OAuth ou refreshes concorrentes.
pub fn config_fingerprint(config: &IntegrationConfig) -> [u8; 32] {
    let mut ctx = DigestContext::new(&SHA256);
    match &config.transport {
        Transport::RemoteHttp { url, .. } => {
            update_len_prefixed(&mut ctx, b"remote_http");
            let canonical = canonicalize_url_for_fingerprint(url);
            update_len_prefixed(&mut ctx, canonical.as_bytes());
        }
        Transport::LocalStdio { command, cwd, .. } => {
            update_len_prefixed(&mut ctx, b"local_stdio");
            ctx.update(&(command.len() as u64).to_le_bytes());
            for part in command {
                update_len_prefixed(&mut ctx, part.as_bytes());
            }
            match cwd {
                Some(dir) => {
                    ctx.update(&[1]);
                    update_len_prefixed(&mut ctx, dir.as_os_str().as_encoded_bytes());
                }
                None => ctx.update(&[0]),
            }
        }
    }
    match &config.auth {
        AuthConfig::None => update_len_prefixed(&mut ctx, b"none"),
        AuthConfig::Bearer => update_len_prefixed(&mut ctx, b"bearer"),
        AuthConfig::OAuthDcr => update_len_prefixed(&mut ctx, b"oauth_dcr"),
        AuthConfig::OAuthManualClient { client_id, scopes } => {
            update_len_prefixed(&mut ctx, b"oauth_manual_client");
            update_len_prefixed(&mut ctx, client_id.trim().as_bytes());
            ctx.update(&(scopes.len() as u64).to_le_bytes());
            for scope in scopes {
                update_len_prefixed(&mut ctx, scope.as_bytes());
            }
        }
    }
    let digest = ctx.finish();
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

pub(crate) fn valid_tool_name(name: &str) -> bool {
    valid_bounded_text(name, MAX_TOOL_NAME_BYTES)
}

fn update_len_prefixed(ctx: &mut DigestContext, bytes: &[u8]) {
    ctx.update(&(bytes.len() as u64).to_le_bytes());
    ctx.update(bytes);
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
        && name.bytes().all(is_rfc9110_tchar)
}

fn is_rfc9110_tchar(byte: u8) -> bool {
    byte.is_ascii_alphanumeric()
        || matches!(
            byte,
            b'!' | b'#'
                | b'$'
                | b'%'
                | b'&'
                | b'\''
                | b'*'
                | b'+'
                | b'-'
                | b'.'
                | b'^'
                | b'_'
                | b'`'
                | b'|'
                | b'~'
        )
}

fn valid_env_key(key: &str) -> bool {
    !key.is_empty()
        && key.len() <= MAX_HEADER_NAME_BYTES
        && !key.contains('=')
        && !key.chars().any(|ch| ch.is_whitespace() || ch.is_control())
}

fn valid_http_url(url: &str) -> bool {
    parse_http_parts(url).is_some()
}

fn is_plaintext_non_loopback_http(url: &str) -> bool {
    let Some((scheme, host, is_ipv6)) = parse_http_parts(url) else {
        return false;
    };
    if scheme != "http" {
        return false;
    }
    let is_loopback = if is_ipv6 {
        host.parse::<Ipv6Addr>()
            .is_ok_and(|ip| ip == Ipv6Addr::LOCALHOST)
    } else {
        host == "localhost" || host == "127.0.0.1"
    };
    !is_loopback
}

fn parse_http_parts(url: &str) -> Option<(String, String, bool)> {
    if url.is_empty()
        || url.len() > MAX_URL_BYTES
        || url.chars().any(|ch| ch.is_whitespace() || ch.is_control())
    {
        return None;
    }
    let (scheme, rest) = if url
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("https://"))
    {
        ("https".to_owned(), url.get(8..)?)
    } else if url
        .get(..7)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("http://"))
    {
        ("http".to_owned(), url.get(7..)?)
    } else {
        return None;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    // Impede userinfo embutida (`user:pass@host`) para evitar segredos na URL persistida.
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    if let Some(after_open) = authority.strip_prefix('[') {
        let (host, tail) = after_open.split_once(']')?;
        if host.is_empty()
            || !host
                .chars()
                .all(|ch| ch.is_ascii_hexdigit() || ch == ':' || ch == '.')
            || host.parse::<Ipv6Addr>().is_err()
        {
            return None;
        }
        if !valid_optional_port(tail) {
            return None;
        }
        return Some((scheme, host.to_ascii_lowercase(), true));
    }
    let host = match authority.split_once(':') {
        Some((host, port)) => {
            if !valid_hostname(host) || !valid_port(port) {
                return None;
            }
            host
        }
        None => {
            if !valid_hostname(authority) {
                return None;
            }
            authority
        }
    };
    Some((scheme, host.to_ascii_lowercase(), false))
}

fn canonicalize_url_for_fingerprint(raw: &str) -> String {
    let without_fragment = raw.trim().split('#').next().unwrap_or("");
    let Ok(uri) = without_fragment.parse::<http::Uri>() else {
        return without_fragment.to_owned();
    };
    let Some(scheme) = uri.scheme_str().map(str::to_ascii_lowercase) else {
        return without_fragment.to_owned();
    };
    let Some(authority) = uri.authority() else {
        return without_fragment.to_owned();
    };
    let default_port = if scheme == "https" { "443" } else { "80" };
    let lower_auth = authority.as_str().to_ascii_lowercase();
    let norm_auth = if let Some(after_bracket) = lower_auth.strip_prefix('[')
        && let Some((ipv6, tail)) = after_bracket.split_once(']')
    {
        if tail
            .strip_prefix(':')
            .is_some_and(|port| port == default_port)
        {
            format!("[{ipv6}]")
        } else {
            lower_auth
        }
    } else if let Some((host, port)) = lower_auth.rsplit_once(':')
        && port == default_port
    {
        host.to_owned()
    } else {
        lower_auth
    };
    let path = match uri.path() {
        "" | "/" => "",
        other => other.trim_end_matches('/'),
    };
    match uri.query() {
        Some(q) if !q.is_empty() => format!("{scheme}://{norm_auth}{path}?{q}"),
        _ => format!("{scheme}://{norm_auth}{path}"),
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
