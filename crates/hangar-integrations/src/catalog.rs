//! Catálogo declarativo de integrações MCP prontas para conexão rápida.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

const RAW_CATALOG: &str = include_str!("catalog.json");

static CATALOG: OnceLock<Vec<CatalogPreset>> = OnceLock::new();

/// Modelo de autenticação esperado por um preset do catálogo.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PresetAuthKind {
    /// Servidor público: conecta sem token.
    #[serde(rename = "none")]
    None,
    /// Token pessoal enviado como `Authorization: Bearer` (servidor sem registro dinâmico de cliente).
    #[serde(rename = "bearer")]
    Bearer,
    #[serde(rename = "oauth_dcr")]
    OAuthDcr,
    #[serde(rename = "oauth_manual_client")]
    OAuthManualClient,
}

/// Guia curto de configuração em português para provedores com `Client ID` manual.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetupGuide {
    pub title: String,
    pub steps: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_hint: Option<String>,
    /// Página do provedor onde a configuração começa (botão "Abrir" nos clientes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
    /// Endereço de retorno desta máquina, preenchido por [`catalog_for_port`] para quem exige redirecionamento local.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
}

/// Marca do serviço: logo (`icon`, nome do arquivo que os clientes embutem) e a cor de fundo dele, `#RRGGBB`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brand {
    pub icon: String,
    pub color: String,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<Brand>,
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

/// Catálogo como a pessoa deve segui-lo nesta máquina: no guia de quem exige redirecionamento local, o `<porta>` do
/// texto vira a porta do servidor e `redirect_uri` traz o endereço pronto para copiar. Porta 0 (ainda não ligada)
/// devolve o catálogo como está.
pub fn catalog_for_port(port: u16) -> Vec<CatalogPreset> {
    catalog()
        .iter()
        .cloned()
        .map(|mut preset| {
            if port != 0 && preset.requires_loopback_redirect {
                let uri = crate::oauth::loopback_redirect_uri(Some(&preset.id), port);
                if let Some(guide) = preset.setup_guide.as_mut() {
                    for step in &mut guide.steps {
                        *step = step.replace("<porta>", &port.to_string());
                    }
                    guide.redirect_uri = Some(uri);
                }
            }
            preset
        })
        .collect()
}

/// Marca de uma integração: a do preset dela; sem preset conhecido, a do serviço cujo endereço é o mesmo host ou, se
/// só um preset usa aquele domínio, o mesmo domínio.
pub fn brand_for(preset: Option<&str>, url: Option<&str>) -> Option<Brand> {
    if let Some(brand) = preset.and_then(find_preset).and_then(|p| p.brand.clone()) {
        return Some(brand);
    }
    let host = url.and_then(host_of)?;
    if let Some(brand) = catalog()
        .iter()
        .filter(|p| host_of(&p.url).as_deref() == Some(host.as_str()))
        .find_map(|p| p.brand.clone())
    {
        return Some(brand);
    }
    let domain = registrable(&host);
    let mut same_domain = catalog()
        .iter()
        .filter(|p| host_of(&p.url).is_some_and(|h| registrable(&h) == domain));
    match (same_domain.next(), same_domain.next()) {
        (Some(only), None) => only.brand.clone(),
        _ => None,
    }
}

fn host_of(url: &str) -> Option<String> {
    reqwest::Url::parse(url).ok()?.host_str().map(str::to_ascii_lowercase)
}

// ponytail: últimos dois rótulos, sem lista de sufixos públicos; serve aos domínios do catálogo (`.com`, `.app`, `.dev`).
fn registrable(host: &str) -> String {
    let labels: Vec<&str> = host.rsplit('.').take(2).collect();
    labels.into_iter().rev().collect::<Vec<_>>().join(".")
}

/// Busca um preset do catálogo pelo slug `id`.
pub fn find_preset(id: &str) -> Option<&'static CatalogPreset> {
    catalog().iter().find(|preset| preset.id == id)
}
