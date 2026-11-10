//! Tipos do contrato HTTP (JSON snake_case) e a validação do que chega do cliente.
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginsState {
    pub plugins: Vec<InstalledPlugin>,
    pub marketplaces: Vec<Marketplace>,
    /// Último `check` bem-sucedido; quem guarda é o servidor, a leitura do disco devolve `None`.
    pub checked_at: Option<String>,
    /// Arquivo presente que não pôde ser lido: a lista sai sem ele e quem chama registra o motivo.
    #[serde(skip)]
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub marketplace: String,
    pub version: String,
    pub last_updated: String,
    pub enabled: bool,
    pub available_version: Option<String>,
    pub update_known: bool,
    pub description: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marketplace {
    pub name: String,
    pub source_label: String,
    pub last_updated: Option<String>,
    pub auto_update: bool,
    pub auto_update_editable: bool,
    pub builtin: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailablePlugin {
    pub id: String,
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PluginDetails {
    pub id: String,
    pub inventory: Inventory,
    pub always_on_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Inventory {
    pub skills: Option<u32>,
    pub agents: Option<u32>,
    pub hooks: Option<u32>,
    pub mcp: Option<u32>,
    pub lsp: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OperationResult {
    pub outcome: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub new_version: Option<String>,
}

/// `nome@marketplace` já validado: vai à CLI como argumento, depois de `--`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceName(String);

/// `dono/repo`, URL `https://` ou caminho absoluto existente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceSource(String);

/// Entrada fora do formato; cada variante é um código do contrato.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputError {
    InvalidId,
    InvalidName,
    InvalidSource,
}

impl PluginId {
    /// # Errors
    ///
    /// [`InputError::InvalidId`] quando não é `nome@marketplace` com as duas partes válidas.
    pub fn parse(raw: &str) -> Result<Self, InputError> {
        let valid = raw.len() <= 256
            && raw.split_once('@').is_some_and(|(name, origin)| valid_part(name) && valid_part(origin));
        if valid { Ok(Self(raw.to_owned())) } else { Err(InputError::InvalidId) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::str::FromStr for PluginId {
    type Err = InputError;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl std::str::FromStr for MarketplaceName {
    type Err = InputError;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl std::str::FromStr for MarketplaceSource {
    type Err = InputError;
    fn from_str(raw: &str) -> Result<Self, Self::Err> {
        Self::parse(raw)
    }
}

impl MarketplaceName {
    /// # Errors
    ///
    /// [`InputError::InvalidName`] quando o nome foge da regra de [`PluginId`].
    pub fn parse(raw: &str) -> Result<Self, InputError> {
        if valid_part(raw) { Ok(Self(raw.to_owned())) } else { Err(InputError::InvalidName) }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl MarketplaceSource {
    /// # Errors
    ///
    /// [`InputError::InvalidSource`] fora das três formas aceitas.
    pub fn parse(raw: &str) -> Result<Self, InputError> {
        let github = raw
            .split_once('/')
            .is_some_and(|(owner, repo)| valid_part(owner) && valid_part(repo));
        let url = raw.strip_prefix("https://").is_some_and(|rest| {
            !rest.is_empty() && !rest.starts_with('/') && rest.chars().all(|c| !c.is_whitespace() && !c.is_control())
        });
        let path = Path::new(raw).is_absolute() && Path::new(raw).exists();
        if !raw.starts_with('-') && (github || url || path) {
            Ok(Self(raw.to_owned()))
        } else {
            Err(InputError::InvalidSource)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl InputError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidId => "plugins_invalid_id",
            Self::InvalidName => "plugins_invalid_name",
            Self::InvalidSource => "plugins_invalid_source",
        }
    }
}

impl fmt::Display for InputError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for InputError {}

/// Regra de `valid_part` em `hangar-server/src/claude_customizations.rs`, com o primeiro caractere
/// alfanumérico: assim nada vira opção da CLI nem caminho relativo.
fn valid_part(name: &str) -> bool {
    name.len() <= 128
        && name.bytes().next().is_some_and(|b| b.is_ascii_alphanumeric())
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_id_rejects_option_and_dot_names() {
        for raw in ["-x@y", "..@..", ".@.", "--x", "x@-y", "x@", "@y", "x@y@z", "a b@c"] {
            assert_eq!(PluginId::parse(raw), Err(InputError::InvalidId), "{raw}");
        }
    }

    #[test]
    fn plugin_id_accepts_real_ids() {
        for raw in ["caveman@caveman", "ui-ux-pro-max@ui-ux-pro-max-skill", "mattpocock-skills@claude-plugins-official"] {
            let id = PluginId::parse(raw).expect(raw);
            assert_eq!(id.as_str(), raw);
        }
    }

    #[test]
    fn marketplace_name_follows_the_same_rule() {
        for raw in ["--x", "-x", ".", "..", "", "a/b"] {
            assert_eq!(MarketplaceName::parse(raw), Err(InputError::InvalidName), "{raw}");
        }
        assert_eq!(MarketplaceName::parse("claude-plugins-official").expect("nome real").as_str(), "claude-plugins-official");
    }

    #[test]
    fn marketplace_source_accepts_only_three_forms() {
        let dir = tempfile::tempdir().expect("pasta temporária");
        let existing = dir.path().to_str().expect("caminho utf-8").to_owned();
        for raw in ["JuliusBrussee/caveman", "https://git.example.com/team/acme.git", existing.as_str()] {
            assert!(MarketplaceSource::parse(raw).is_ok(), "{raw}");
        }
        let missing = dir.path().join("nao-existe");
        for raw in [
            "--help",
            "-x/y",
            "http://example.com/x.git",
            "https://",
            "https://a b",
            "git@github.com:a/b.git",
            "a/b/c",
            "relativo",
            missing.to_str().expect("caminho utf-8"),
        ] {
            assert_eq!(MarketplaceSource::parse(raw), Err(InputError::InvalidSource), "{raw}");
        }
    }

    #[test]
    fn input_errors_map_to_contract_codes() {
        assert_eq!(InputError::InvalidId.code(), "plugins_invalid_id");
        assert_eq!(InputError::InvalidName.code(), "plugins_invalid_name");
        assert_eq!(InputError::InvalidSource.code(), "plugins_invalid_source");
    }

    #[test]
    fn state_serializes_with_contract_field_names() {
        let result = OperationResult { outcome: "ok".into(), message: "m".into(), old_version: None, new_version: Some("2".into()) };
        assert_eq!(
            serde_json::to_value(&result).expect("json"),
            serde_json::json!({"outcome": "ok", "message": "m", "new_version": "2"})
        );
    }
}
