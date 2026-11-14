//! O que Tickets guarda por servidor: as etapas do trabalho e, por ticket, a sessão ligada,
//! a etapa, o histórico, as aprovações e as evidências.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default)]
pub struct Group {
    pub id: String,
    pub members: Vec<String>,
    pub keys: BTreeMap<String,String>,
}

/// Etapa do trabalho. Com `gate`, o agente não passa dela sem a aprovação do dono.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stage {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub gate: bool,
    /// Nomes de status do sistema de origem que indicam esta etapa (ex.: "Revisão de Código").
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub statuses: Vec<String>,
    /// Instruções opcionais acrescentadas à ação enviada para a sessão.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,
    /// IDs das ações escolhidas, separados pelo conector que as oferece.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actions: Option<BTreeMap<String, Vec<String>>>,
}

impl Stage {
    fn new(id: &str, name: &str, gate: bool, statuses: &[&str]) -> Self {
        Self { id: id.into(), name: name.into(), gate, statuses: statuses.iter().map(|s| (*s).to_owned()).collect(), prompt: None, actions: None }
    }
}

/// O caminho de uma PM no plugin Promedico: plano aprovado, execução, revisão do MR, validação com
/// evidências (o dono confere) e entrega. Cada servidor pode trocar a lista.
pub fn default_stages() -> Vec<Stage> {
    vec![
        Stage::new("planejamento", "Planejamento", false, &[]),
        Stage::new("execucao", "Execução", false, &[]),
        Stage::new("revisao", "Revisão", false, &["Revisão de Código"]),
        Stage::new("validacao", "Validação", true, &[]),
        Stage::new("entrega", "Entrega", false, &[]),
    ]
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    pub id: String,
    /// Nome do arquivo de origem, para mostrar.
    pub name: String,
    pub mime: String,
    pub size: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caption: String,
    pub at: String,
    /// `dono` ou o nome da sessão que anexou.
    pub by: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind { Linked, Stage, Approved, Changes, Evidence, Delivered, Note }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    pub at: String,
    pub kind: EventKind,
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub connector: String,
    pub ticket_id: String,
    pub key: String,
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation: Option<String>,
    /// Sessões associadas ao trabalho; fechar a coordenadora não desfaz os vínculos dos participantes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub session_keys: BTreeMap<String,String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub group_ids: Vec<String>,
    #[serde(default,skip_serializing_if="BTreeMap::is_empty")]
    pub group_members: BTreeMap<String,Vec<String>>,
    /// Pasta escolhida no ticket, usada quando uma ação precisa abrir a sessão.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repository: Option<String>,
    /// Etapa informada pelo agente ou escolhida pelo dono; sem ela, o cliente deduz pelo plano da sessão.
    #[serde(default)]
    pub stage: Option<String>,
    /// Etapas com portão já aprovadas pelo dono.
    #[serde(default)]
    pub approved: Vec<String>,
    #[serde(default)]
    pub history: Vec<Event>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
    #[serde(default)]
    pub delivered_at: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Snapshot {
    pub stages: Vec<Stage>,
    pub items: Vec<Item>,
}

pub(crate) fn now() -> String { chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true) }
