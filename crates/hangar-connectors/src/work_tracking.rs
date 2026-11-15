//! Capacidade `work_tracking/v1`: leitura de tickets, independente do fornecedor.
//!
//! O host valida o que manda e o que recebe e devolve só os campos do contrato: campo extra do
//! conector não atravessa. Nenhum formato de chave (PM, issue, JQL) é imposto aqui.
use crate::protocol::ConnectorError;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

pub const CAPABILITY: &str = "work_tracking";
pub const VERSION: u32 = 1;
pub const LIST: &str = "work_tracking.list";
pub const GET: &str = "work_tracking.get";
pub const MAX_LIMIT: u32 = 100;
/// Tamanho de página quando o pedido não diz.
pub const DEFAULT_LIMIT: u32 = 20;
const MAX_TEXT: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusCategory { Todo, InProgress, Done, Canceled, Unknown }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status { pub id: String, pub name: String, pub category: StatusCategory }

/// Pessoa ou projeto: ID opaco do fornecedor e nome para mostrar.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ref { pub id: String, pub name: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketSummary {
    pub id: String,
    pub key: String,
    pub title: String,
    pub url: String,
    pub status: Status,
    pub assignee: Option<Ref>,
    pub project: Option<Ref>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct TicketDetail {
    #[serde(flatten)]
    pub summary: TicketSummary,
    pub description_markdown: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ListResult { pub items: Vec<TicketSummary>, pub next_cursor: Option<String>, pub total: Option<u64> }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// `me` ou o ID opaco de uma pessoa no fornecedor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    /// `open` (a fazer e em andamento) ou `done` (concluído e cancelado); ausente, todos.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<String>,
}

/// Valores aceitos em `filter.state`.
pub const STATES: [&str; 2] = ["open", "done"];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListParams {
    #[serde(default)]
    pub filter: Filter,
    pub limit: u32,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetParams { pub ticket_id: String }

fn bounded(field: &str, value: &str, empty_ok: bool) -> Result<(), String> {
    if !empty_ok && value.trim().is_empty() { return Err(format!("{field} vazio")); }
    if value.len() > MAX_TEXT { return Err(format!("{field} acima de {MAX_TEXT} bytes")); }
    if value.contains('\0') { return Err(format!("{field} com NUL")); }
    Ok(())
}

impl ListParams {
    /// Lê e confere o pedido; texto vazio nos filtros vale como ausente.
    pub fn from_value(value: Value) -> Result<Self, ConnectorError> {
        let mut params: ListParams = serde_json::from_value(value)
            .map_err(|error| ConnectorError::invalid_params(format!("pedido de listagem inválido: {error}")))?;
        params.normalize().map_err(ConnectorError::invalid_params)?;
        Ok(params)
    }

    fn normalize(&mut self) -> Result<(), String> {
        if !(1..=MAX_LIMIT).contains(&self.limit) { return Err(format!("limit fora de 1..={MAX_LIMIT}")); }
        if let Some(state) = self.filter.state.as_deref() && !STATES.contains(&state) {
            return Err(format!("state precisa ser {}", STATES.join(" ou ")));
        }
        for (field, value) in [("query", &mut self.filter.query), ("assignee", &mut self.filter.assignee), ("cursor", &mut self.cursor)] {
            if let Some(text) = value.as_deref() {
                bounded(field, text, true)?;
                if text.trim().is_empty() { *value = None; }
            }
        }
        Ok(())
    }
}

impl ListParams {
    /// Pares nome/valor da URL ou das opções do CLI: `query`, `assignee`, `state`, `limit` e `cursor`.
    /// Nome desconhecido é `invalid_params`, nunca ignorado.
    pub fn from_pairs<K: AsRef<str>, V: Into<String>>(pairs: impl IntoIterator<Item = (K, V)>) -> Result<Self, ConnectorError> {
        let mut params = ListParams { filter: Filter::default(), limit: DEFAULT_LIMIT, cursor: None };
        for (name, value) in pairs {
            let value = value.into();
            match name.as_ref() {
                "query" => params.filter.query = Some(value),
                "assignee" => params.filter.assignee = Some(value),
                "state" => params.filter.state = Some(value),
                "cursor" => params.cursor = Some(value),
                "limit" => params.limit = value.parse().map_err(|_| ConnectorError::invalid_params("limit precisa ser número"))?,
                other => return Err(ConnectorError::invalid_params(format!("filtro desconhecido: {other}"))),
            }
        }
        params.normalize().map_err(ConnectorError::invalid_params)?;
        Ok(params)
    }
}

impl GetParams {
    pub fn new(ticket_id: impl Into<String>) -> Result<Self, ConnectorError> {
        let params = GetParams { ticket_id: ticket_id.into() };
        bounded("ticket_id", &params.ticket_id, false).map_err(ConnectorError::invalid_params)?;
        Ok(params)
    }

    pub fn from_value(value: Value) -> Result<Self, ConnectorError> {
        let params: GetParams = serde_json::from_value(value)
            .map_err(|error| ConnectorError::invalid_params(format!("pedido de detalhe inválido: {error}")))?;
        Self::new(params.ticket_id)
    }
}

/// Instante da leitura pelo relógio do host, acrescentado à lista e ao detalhe.
pub fn with_fetched_at(mut value: Value) -> Value {
    value["fetched_at"] = serde_json::json!(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
    value
}

fn absolute_http(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https") && url.host().is_some())
}

impl TicketSummary {
    fn check(&self) -> Result<(), String> {
        bounded("id", &self.id, false)?;
        bounded("key", &self.key, false)?;
        if self.title.len() > 4096 { return Err("title acima de 4096 bytes".into()); }
        if !absolute_http(&self.url) { return Err("url não é HTTP/HTTPS absoluta".into()); }
        bounded("status.id", &self.status.id, false)?;
        bounded("status.name", &self.status.name, true)?;
        for (field, value) in [("assignee", &self.assignee), ("project", &self.project)] {
            if let Some(value) = value { bounded(&format!("{field}.id"), &value.id, false)?; bounded(&format!("{field}.name"), &value.name, true)?; }
        }
        if let Some(at) = &self.updated_at && chrono::DateTime::parse_from_rfc3339(at).is_err() {
            return Err("updated_at sem RFC 3339 com fuso".into());
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct RawList { items: Vec<TicketSummary>, next_cursor: Option<String>, total: Option<u64> }

impl ListResult {
    /// Confere a resposta do conector contra o pedido (`limit`) e o contrato.
    pub fn from_connector(value: Value, limit: u32) -> Result<Self, ConnectorError> {
        let raw: RawList = serde_json::from_value(value)
            .map_err(|error| ConnectorError::protocol(format!("listagem fora do contrato: {error}")))?;
        if raw.items.len() > limit as usize { return Err(ConnectorError::protocol("mais itens que o limite pedido")); }
        let mut seen = HashSet::new();
        for item in &raw.items {
            item.check().map_err(|reason| ConnectorError::protocol(format!("ticket fora do contrato: {reason}")))?;
            if !seen.insert(item.id.as_str()) { return Err(ConnectorError::protocol("ticket repetido na mesma página")); }
        }
        if let Some(cursor) = &raw.next_cursor {
            bounded("next_cursor", cursor, false).map_err(ConnectorError::protocol)?;
        }
        Ok(Self { items: raw.items, next_cursor: raw.next_cursor, total: raw.total })
    }
}

#[derive(Deserialize)]
struct RawDetail {
    #[serde(flatten)]
    summary: TicketSummary,
    description_markdown: String,
}

impl TicketDetail {
    pub fn from_connector(value: Value) -> Result<Self, ConnectorError> {
        let raw: RawDetail = serde_json::from_value(value)
            .map_err(|error| ConnectorError::protocol(format!("detalhe fora do contrato: {error}")))?;
        raw.summary.check().map_err(|reason| ConnectorError::protocol(format!("ticket fora do contrato: {reason}")))?;
        Ok(Self { summary: raw.summary, description_markdown: raw.description_markdown })
    }
}
