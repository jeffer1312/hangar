//! Catálogo declarativo de integrações MCP prontas para conexão rápida.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const RAW_CATALOG: &str = include_str!("catalog.json");

static CATALOG: OnceLock<Vec<CatalogPreset>> = OnceLock::new();

/// Modelo de autenticação esperado por um preset do catálogo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetAuthKind {
    #[serde(rename = "oauth_dcr")]
    OAuthDcr,
    #[serde(rename = "oauth_manual_client")]
    OAuthManualClient,
    #[serde(rename = "oauth_dcr_or_bearer")]
    OAuthDcrOrBearer,
}

/// Guia curto de configuração em português para provedores com `Client ID` manual.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupGuide {
    pub title: String,
    pub steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_hint: Option<String>,
}

/// Definição declarativa de um servidor MCP remoto conhecido no catálogo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogPreset {
    pub id: String,
    pub name: String,
    pub category: String,
    pub description: String,
    pub url: String,
    #[serde(default)]
    pub url_editable: bool,
    pub auth_kind: PresetAuthKind,
    #[serde(default)]
    pub default_scopes: Vec<String>,
    #[serde(default)]
    pub requires_loopback_redirect: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub setup_guide: Option<SetupGuide>,
}

/// Retorna todos os presets embutidos no catálogo pronto.
///
/// # Panics
///
/// Entra em pânico somente se o arquivo embutido `catalog.json` estiver corrompido em build.
pub fn catalog() -> &'static [CatalogPreset] {
    CATALOG.get_or_init(|| {
        serde_json::from_str(RAW_CATALOG)
            .expect("catalog.json embutido deve ser um JSON válido de CatalogPreset")
    })
}

/// Busca um preset do catálogo pelo slug `id`.
pub fn find_preset(id: &str) -> Option<&'static CatalogPreset> {
    catalog().iter().find(|preset| preset.id == id)
}
