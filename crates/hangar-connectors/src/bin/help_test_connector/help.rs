//! Consultas GET ao Help e tradução para `work_tracking/v1`. Nunca escreve no Help.
use crate::credential::Sources;
use hangar_connectors::protocol::{ConnectorError, ErrorCode};
use hangar_connectors::work_tracking::{GetParams, ListParams};
use serde_json::{Value, json};
use std::time::Duration;

pub const DEFAULT_URL: &str = "https://help.pmedico.com";

pub struct Help {
    base: reqwest::Url,
    client: reqwest::Client,
    credentials: Sources,
    /// Login de `me`, junto do token que o descobriu.
    me: Option<(String, String)>,
    /// Status do fluxo do Help (`id`, categoria), junto do token que os leu.
    statuses: Option<(String, Vec<(i64, String)>)>,
}

fn error(code: ErrorCode, message: impl Into<String>) -> ConnectorError { ConnectorError::new(code, message) }

impl Help {
    /// Produção só por HTTPS; HTTP só para um servidor de teste no loopback.
    pub fn new(base: &str, credentials: Sources) -> Result<Self, ConnectorError> {
        let base = reqwest::Url::parse(base.trim_end_matches('/'))
            .map_err(|_| error(ErrorCode::InvalidParams, "endereço do Help inválido"))?;
        let loopback = matches!(base.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
        if base.scheme() != "https" && !(base.scheme() == "http" && loopback) {
            return Err(error(ErrorCode::InvalidParams, "o Help precisa de HTTPS (HTTP só no loopback, para teste)"));
        }
        let client = hangar_core::net::http_client()
            // Nunca levar o Authorization para outro endereço.
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|_| error(ErrorCode::Unavailable, "cliente HTTP não iniciou"))?;
        Ok(Self { base, client, credentials, me: None, statuses: None })
    }

    fn token(&self) -> Result<String, ConnectorError> {
        self.credentials.token().ok_or_else(|| error(ErrorCode::AuthRequired,
            "sem token do Help nesta máquina: faça o login do plugin Promedico (credenciais) e tente de novo"))
    }

    async fn get(&self, token: &str, path: &str, query: &[(&str, String)]) -> Result<Value, ConnectorError> {
        let mut url = self.base.clone();
        url.set_path(path);
        if !query.is_empty() { url.query_pairs_mut().extend_pairs(query); }
        let response = self.client.get(url).bearer_auth(token).send().await.map_err(|failure| {
            if failure.is_timeout() { error(ErrorCode::Timeout, "o Help não respondeu no prazo") } else { error(ErrorCode::Unavailable, "o Help está fora do alcance") }
        })?;
        let status = response.status().as_u16();
        match status {
            200 => {}
            300..=399 => return Err(error(ErrorCode::Unavailable, "o Help respondeu com redirecionamento, recusado")),
            401 => return Err(error(ErrorCode::AuthRequired, "o Help recusou o token: refaça o login do plugin Promedico")),
            403 => return Err(error(ErrorCode::PermissionDenied, "o Help negou acesso a este recurso")),
            404 => return Err(error(ErrorCode::NotFound, "não encontrado no Help")),
            _ => return Err(error(ErrorCode::Unavailable, format!("o Help respondeu HTTP {status}"))),
        }
        let bytes = response.bytes().await.map_err(|_| error(ErrorCode::Unavailable, "resposta do Help interrompida"))?;
        serde_json::from_slice(&bytes).map_err(|_| error(ErrorCode::ProtocolError, "resposta do Help não é JSON"))
    }

    async fn me(&mut self, token: &str) -> Result<String, ConnectorError> {
        if let Some((known, login)) = &self.me && known == token { return Ok(login.clone()); }
        let value = self.get(token, "/api/eu", &[]).await?;
        let login = value.get("login").and_then(Value::as_str).filter(|login| !login.is_empty())
            .ok_or_else(|| error(ErrorCode::ProtocolError, "o Help não informou o login de quem consulta"))?.to_owned();
        self.me = Some((token.to_owned(), login.clone()));
        Ok(login)
    }

    /// IDs dos status das categorias do Help que valem para `state`: `open` é a fazer e em andamento.
    async fn state_ids(&mut self, token: &str, state: &str) -> Result<Vec<i64>, ConnectorError> {
        if self.statuses.as_ref().is_none_or(|(known, _)| known != token) {
            let flow = self.get(token, "/api/fluxo", &[]).await?;
            let mut statuses: Vec<(i64, String)> = flow.as_array().into_iter().flatten()
                .filter_map(|transition| {
                    let target = transition.get("destino")?;
                    Some((target.get("id")?.as_i64()?, target.get("categoria")?.as_str()?.to_owned()))
                }).collect();
            statuses.sort();
            statuses.dedup();
            if statuses.is_empty() { return Err(error(ErrorCode::ProtocolError, "o fluxo do Help veio sem status")); }
            self.statuses = Some((token.to_owned(), statuses));
        }
        let wanted: &[&str] = if state == "open" { &["a_fazer", "em_andamento"] } else { &["concluido"] };
        Ok(self.statuses.iter().flat_map(|(_, all)| all).filter(|(_, category)| wanted.contains(&category.as_str())).map(|(id, _)| *id).collect())
    }

    pub async fn list(&mut self, params: ListParams) -> Result<Value, ConnectorError> {
        let token = self.token()?;
        let filters = filter_hash(&params);
        let page = match &params.cursor {
            None => 1,
            Some(cursor) => parse_cursor(cursor, params.limit, &filters)?,
        };
        let mut query = vec![("pagina", page.to_string()), ("tamanho", params.limit.to_string())];
        if let Some(text) = &params.filter.query { query.push(("busca", text.clone())); }
        if let Some(assignee) = &params.filter.assignee {
            // O filtro do Help recebe login: `me` vira o login de quem consulta.
            let login = if assignee == "me" { self.me(&token).await? } else { assignee.clone() };
            query.push(("responsaveis", login));
        }
        if let Some(state) = &params.filter.state {
            for id in self.state_ids(&token, state).await? { query.push(("status_ids", id.to_string())); }
        }
        let value = self.get(&token, "/api/pms/pagina", &query).await?;
        let items = value.get("itens").and_then(Value::as_array).ok_or_else(|| error(ErrorCode::ProtocolError, "lista do Help sem itens"))?;
        let total = value.get("total").and_then(Value::as_f64).filter(|total| *total >= 0.0).map(|total| total as u64);
        let summaries = items.iter().map(|item| self.summary(item)).collect::<Result<Vec<_>, _>>()?;
        let shown = u64::from(page) * u64::from(params.limit);
        let next = match total {
            Some(total) => shown < total,
            None => summaries.len() as u32 == params.limit,
        };
        Ok(json!({
            "items": summaries,
            "next_cursor": next.then(|| page.checked_add(1).map(|next| cursor(next, params.limit, &filters))).flatten(),
            "total": total,
        }))
    }

    pub async fn get_ticket(&mut self, params: GetParams) -> Result<Value, ConnectorError> {
        let token = self.token()?;
        let key = params.ticket_id;
        // `.`/`..` virariam segmento de ponto na URL e sairiam de `/api/pms/`.
        if matches!(key.trim(), "." | "..") { return Err(error(ErrorCode::NotFound, "não encontrado no Help")); }
        let path = format!("/api/pms/{}", percent_encoding::utf8_percent_encode(&key, percent_encoding::NON_ALPHANUMERIC));
        let value = self.get(&token, &path, &[]).await?;
        let mut detail = self.summary(&value)?;
        detail["description_markdown"] = json!(value.get("descricao").and_then(Value::as_str).unwrap_or(""));
        Ok(detail)
    }

    fn summary(&self, pm: &Value) -> Result<Value, ConnectorError> {
        let key = pm.get("chave").and_then(Value::as_str).filter(|key| !key.is_empty())
            .ok_or_else(|| error(ErrorCode::ProtocolError, "PM sem chave"))?;
        let status = pm.get("status").ok_or_else(|| error(ErrorCode::ProtocolError, "PM sem status"))?;
        let category = match status.get("categoria").and_then(Value::as_str) {
            Some("a_fazer") => "todo",
            Some("em_andamento") => "in_progress",
            Some("concluido") => "done",
            _ => "unknown",
        };
        let status_id = match status.get("id") {
            Some(Value::String(id)) => id.clone(),
            Some(Value::Number(id)) => id.to_string(),
            _ => return Err(error(ErrorCode::ProtocolError, "status da PM sem id")),
        };
        // O login é o ID da pessoa: é ele que o filtro de responsável aceita de volta.
        let assignee = pm.get("responsavel").filter(|person| person.is_object()).and_then(|person| {
            let login = person.get("login").and_then(Value::as_str).filter(|login| !login.is_empty())?;
            Some(json!({"id": login, "name": person.get("nome").and_then(Value::as_str).unwrap_or(login)}))
        });
        let updated_at = pm.get("atualizada_em").and_then(Value::as_str)
            .filter(|at| chrono::DateTime::parse_from_rfc3339(at).is_ok());
        let mut url = self.base.clone();
        url.set_query(None);
        url.path_segments_mut().map_err(|_| error(ErrorCode::InvalidParams, "endereço do Help sem caminho"))?.clear().push("pm").push(key);
        Ok(json!({
            "id": key, "key": key,
            "title": pm.get("resumo").and_then(Value::as_str).unwrap_or(""),
            "url": url.as_str(),
            "status": {"id": status_id, "name": status.get("nome").and_then(Value::as_str).unwrap_or(""), "category": category},
            "assignee": assignee,
            // A PM não traz um projeto de repositório inequívoco.
            "project": null,
            "updated_at": updated_at,
        }))
    }
}

/// O cursor vale só para os mesmos filtros e o mesmo tamanho de página.
fn filter_hash(params: &ListParams) -> String {
    let source = json!([params.filter.query, params.filter.assignee, params.filter.state, params.limit]).to_string();
    sha1_smol::Sha1::from(source).digest().to_string()[..12].to_owned()
}

fn cursor(page: u32, limit: u32, filters: &str) -> String { format!("v1.{page}.{limit}.{filters}") }

fn parse_cursor(text: &str, limit: u32, filters: &str) -> Result<u32, ConnectorError> {
    let invalid = || error(ErrorCode::InvalidCursor, "cursor de outra consulta: refaça a listagem sem cursor");
    let parts: Vec<&str> = text.split('.').collect();
    let [version, page, size, hash] = parts[..] else { return Err(invalid()) };
    if version != "v1" || hash != filters || size.parse::<u32>().ok() != Some(limit) { return Err(invalid()); }
    page.parse::<u32>().ok().filter(|page| *page >= 1).ok_or_else(invalid)
}
