//! Cache em memória de ferramentas descobertas e classificação de acesso por política.

use std::collections::{BTreeSet, HashMap};
use std::sync::{PoisonError, RwLock};

use serde::{Deserialize, Serialize};

use crate::model::{IntegrationConfig, PermissionPolicy, ToolAccess, ToolSummary};

/// Ferramenta descoberta via `tools/list` em um servidor MCP externo.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DiscoveredTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
}

impl DiscoveredTool {
    /// Converte a ferramenta descoberta em resumo público aplicando a política da integração.
    pub fn to_summary(&self, config: &IntegrationConfig) -> ToolSummary {
        ToolSummary {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            read_only_hint: self.read_only_hint,
            effective_access: classify_tool_access(
                config.policy,
                &config.allowed_tools,
                self.read_only_hint,
                &self.name,
            ),
        }
    }
}

/// Classifica o nível efetivo de acesso de uma ferramenta conforme política e liberações.
///
/// Fail-closed: apenas `read_only_hint == Some(true)` é tratado como leitura livre; ausência
/// da dica (`None`) ou `Some(false)` exige aprovação ou bloqueio conforme a política.
pub fn classify_tool_access(
    policy: PermissionPolicy,
    allowed_tools: &BTreeSet<String>,
    read_only_hint: Option<bool>,
    tool_name: &str,
) -> ToolAccess {
    if read_only_hint == Some(true) {
        return ToolAccess::Read;
    }
    match policy {
        PermissionPolicy::ReadOnly => ToolAccess::Blocked,
        PermissionPolicy::AllowAll => ToolAccess::AllowedAlways,
        PermissionPolicy::ReadFreeAskWrite => {
            if allowed_tools.contains(tool_name) {
                ToolAccess::AllowedAlways
            } else {
                ToolAccess::AskWrite
            }
        }
    }
}

/// Armazenamento thread-safe em memória das ferramentas descobertas por integração.
#[derive(Default)]
pub struct ToolsCache {
    entries: RwLock<HashMap<String, Vec<DiscoveredTool>>>,
}

impl ToolsCache {
    /// Cria um cache de ferramentas vazio.
    pub fn new() -> Self {
        Self::default()
    }

    /// Armazena ou substitui a lista de ferramentas descobertas de uma integração.
    pub fn set(&self, integration_id: &str, tools: Vec<DiscoveredTool>) {
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(integration_id.to_owned(), tools);
    }

    /// Retorna as ferramentas descobertas de `integration_id`, se já consultadas.
    pub fn get(&self, integration_id: &str) -> Option<Vec<DiscoveredTool>> {
        self.entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(integration_id)
            .cloned()
    }

    /// Busca uma ferramenta específica no cache de `integration_id`.
    pub fn get_tool(&self, integration_id: &str, tool_name: &str) -> Option<DiscoveredTool> {
        self.entries
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(integration_id)?
            .iter()
            .find(|tool| tool.name == tool_name)
            .cloned()
    }

    /// Remove o cache de `integration_id`.
    pub fn remove(&self, integration_id: &str) -> Option<Vec<DiscoveredTool>> {
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(integration_id)
    }

    /// Limpa todo o cache de ferramentas em memória.
    pub fn clear(&self) {
        self.entries
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Converte as ferramentas cacheadas de `config.id` em `ToolSummary` com acesso efetivo.
    pub fn summaries(&self, config: &IntegrationConfig) -> Option<Vec<ToolSummary>> {
        self.get(&config.id)
            .map(|tools| Self::to_tool_summaries(config, &tools))
    }

    /// Converte um conjunto de ferramentas descobertas em `ToolSummary` para `config`.
    pub fn to_tool_summaries(
        config: &IntegrationConfig,
        tools: &[DiscoveredTool],
    ) -> Vec<ToolSummary> {
        tools.iter().map(|tool| tool.to_summary(config)).collect()
    }
}
