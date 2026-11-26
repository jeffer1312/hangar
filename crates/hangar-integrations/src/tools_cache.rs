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

    /// Consome a ferramenta descoberta convertendo-a em resumo público sem clonar os campos.
    pub fn into_summary(self, config: &IntegrationConfig) -> ToolSummary {
        let effective_access = classify_tool_access(
            config.policy,
            &config.allowed_tools,
            self.read_only_hint,
            &self.name,
        );
        ToolSummary {
            name: self.name,
            description: self.description,
            input_schema: self.input_schema,
            read_only_hint: self.read_only_hint,
            effective_access,
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

#[derive(Default)]
struct CacheState {
    entries: HashMap<String, Vec<DiscoveredTool>>,
    generations: HashMap<String, u64>,
    floor: u64,
    next_seq: u64,
}

/// Armazenamento thread-safe em memória das ferramentas descobertas por integração.
#[derive(Default)]
pub struct ToolsCache {
    state: RwLock<CacheState>,
}

impl ToolsCache {
    /// Cria um cache de ferramentas vazio.
    pub fn new() -> Self {
        Self::default()
    }

    /// Retorna a geração monotônica de invalidação atual para `integration_id`.
    pub fn generation(&self, integration_id: &str) -> u64 {
        let state = self.state.read().unwrap_or_else(PoisonError::into_inner);
        state
            .generations
            .get(integration_id)
            .copied()
            .unwrap_or(state.floor)
    }

    /// Armazena ou substitui a lista de ferramentas descobertas de uma integração.
    pub fn set(&self, integration_id: &str, tools: Vec<DiscoveredTool>) {
        self.state
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .insert(integration_id.to_owned(), tools);
    }

    /// Armazena a lista apenas se a geração de invalidação de `integration_id` ainda for `expected_gen`.
    pub fn set_if_generation(
        &self,
        integration_id: &str,
        expected_gen: u64,
        tools: Vec<DiscoveredTool>,
    ) -> bool {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        let current = state
            .generations
            .get(integration_id)
            .copied()
            .unwrap_or(state.floor);
        if current != expected_gen {
            return false;
        }
        state.entries.insert(integration_id.to_owned(), tools);
        true
    }

    /// Retorna as ferramentas descobertas de `integration_id`, se já consultadas.
    pub fn get(&self, integration_id: &str) -> Option<Vec<DiscoveredTool>> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .get(integration_id)
            .cloned()
    }

    /// Busca uma ferramenta específica no cache de `integration_id`.
    pub fn get_tool(&self, integration_id: &str, tool_name: &str) -> Option<DiscoveredTool> {
        self.state
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .entries
            .get(integration_id)?
            .iter()
            .find(|tool| tool.name == tool_name)
            .cloned()
    }

    /// Remove o cache de `integration_id` e avança sua geração para invalidar descobertas em voo.
    pub fn invalidate(&self, integration_id: &str) -> Option<Vec<DiscoveredTool>> {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.next_seq += 1;
        let next_gen = state.next_seq;
        state
            .generations
            .insert(integration_id.to_owned(), next_gen);
        state.entries.remove(integration_id)
    }

    /// Remove o cache de `integration_id` (sinônimo de [`Self::invalidate`]).
    pub fn remove(&self, integration_id: &str) -> Option<Vec<DiscoveredTool>> {
        self.invalidate(integration_id)
    }

    /// Limpa todo o cache de ferramentas em memória e invalida descobertas em andamento.
    pub fn clear(&self) {
        let mut state = self.state.write().unwrap_or_else(PoisonError::into_inner);
        state.entries.clear();
        state.next_seq += 1;
        state.floor = state.next_seq;
        state.generations.clear();
    }

    /// Converte as ferramentas cacheadas de `config.id` em `ToolSummary` com acesso efetivo.
    pub fn summaries(&self, config: &IntegrationConfig) -> Option<Vec<ToolSummary>> {
        self.get(&config.id)
            .map(|tools| Self::into_tool_summaries(config, tools))
    }

    /// Converte um conjunto de ferramentas descobertas em `ToolSummary` para `config`.
    pub fn to_tool_summaries(
        config: &IntegrationConfig,
        tools: &[DiscoveredTool],
    ) -> Vec<ToolSummary> {
        tools.iter().map(|tool| tool.to_summary(config)).collect()
    }

    /// Consome uma lista de ferramentas descobertas convertendo em `ToolSummary` sem clonar schemas.
    pub fn into_tool_summaries(
        config: &IntegrationConfig,
        tools: Vec<DiscoveredTool>,
    ) -> Vec<ToolSummary> {
        tools
            .into_iter()
            .map(|tool| tool.into_summary(config))
            .collect()
    }
}
