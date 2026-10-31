//! `GET /api/search/context`: a mensagem achada pela busca e as vizinhas dela, só as do usuário e as do
//! assistente com texto. Port de `search_context` (api.py) com `archive_jsonl` e
//! `merged_history("__archive__", …)`. Só o dono; o resto segue ao Python, que recusa.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Value, json};

use crate::routes::{AppState, cors, gate, pass, route_failed};
use crate::session_open::OpenError;
use crate::session_open::archive::{Where, claude_jsonl};
use crate::session_write::{detail_body, json_response};
use crate::transcript::{HistoryRequest, Provider, TAIL_WINDOW, merged_history};

const DEFAULT_AROUND: usize = 3;
const MAX_AROUND: f64 = 10.0;
// O `int` do FastAPI aceita espaço em volta, `_` entre dígitos e `3.0`.
static PY_INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[+-]?[0-9](?:_?[0-9])*(?:\.0*)?\z").unwrap());

struct Params { project: String, session_id: String, event_id: String, around: usize }

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (forward, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || req.method() != Method::GET {
        return pass(&st, req, &forward).await;
    }
    let query: HashMap<String, String> = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
    let mut resp = match parse(&query) {
        Err(body) => json_response(StatusCode::UNPROCESSABLE_ENTITY, body),
        Ok(params) => {
            let worker = st.clone();
            match tokio::task::spawn_blocking(move || excerpt(&worker, &params)).await {
                Ok(resp) => resp,
                Err(_) => return route_failed(&st, req.headers(), "rust.search_context_failed", "", "search_context_panic",
                    "a leitura do trecho caiu no servidor"),
            }
        }
    };
    cors(req.headers(), resp.headers_mut());
    resp
}

/// A validação do FastAPI: todos os erros juntos, na ordem dos parâmetros.
fn parse(query: &HashMap<String, String>) -> Result<Params, Value> {
    let mut errors = Vec::new();
    let mut text = |name: &str| query.get(name).cloned().or_else(|| {
        errors.push(json!({"type": "missing", "loc": ["query", name], "msg": "Field required", "input": null}));
        None
    });
    let (project, session_id, event_id) = (text("project"), text("session_id"), text("event_id"));
    let around = match query.get("around") {
        None => Some(DEFAULT_AROUND),
        Some(raw) => {
            let raw_t = raw.trim();
            let clamped = PY_INT.is_match(raw_t).then(|| raw_t.replace('_', "").parse::<f64>().ok()).flatten()
                .map(|n| n.clamp(0.0, MAX_AROUND) as usize);
            if clamped.is_none() {
                errors.push(json!({"type": "int_parsing", "loc": ["query", "around"],
                    "msg": "Input should be a valid integer, unable to parse string as an integer", "input": raw}));
            }
            clamped
        }
    };
    match (project, session_id, event_id, around) {
        (Some(project), Some(session_id), Some(event_id), Some(around)) if errors.is_empty() => Ok(Params { project, session_id, event_id, around }),
        _ => Err(json!({"detail": errors})),
    }
}

fn refused(status: u16, code: &str, msg: &str, params: Value) -> Response {
    json_response(StatusCode::from_u16(status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE), detail_body(code, msg, params))
}

fn excerpt(st: &AppState, p: &Params) -> Response {
    let env = &st.accounts.env;
    let config_dirs: Vec<String> = match st.accounts.claude_catalog() {
        Ok(catalog) => catalog.as_array().into_iter().flatten().filter_map(|row| row["path"].as_str().map(str::to_owned)).collect(),
        Err(e) => return refused(e.status, e.code, &e.message, e.params),
    };
    let server_projects = env.claude_base.join("projects");
    let w = Where { home: &env.home, env: &env.base, server_projects: &server_projects, config_dirs: &config_dirs, live: &[] };
    let jsonl = match claude_jsonl(&w, &p.project, &p.session_id, None) {
        Ok(path) => path,
        Err(miss) => {
            let e = OpenError::from(miss);
            return refused(e.status, e.code.unwrap_or("erro_path_invalido"), &e.msg, e.params);
        }
    };
    let req = HistoryRequest { provider: Provider::Claude, jsonl, limit: None, tail_window: TAIL_WINDOW,
        queue: Some(crate::locate::queue_path(&env.claude_base, "__archive__")) };
    let msgs: Vec<ChatEvent> = match merged_history(&req) {
        Ok(evs) => evs.into_iter()
            .filter(|ev| matches!(ev.kind, ChatKind::UserMsg | ChatKind::AssistantMsg) && ev.text.as_deref().is_some_and(|t| !t.is_empty()))
            .collect(),
        // Como o Python: transcript que não abre é conversa vazia, e o trecho não está mais nela.
        Err(_) => {
            st.diag.report("rust.search_context_unreadable", &p.session_id, "search_context_io", "o transcript não pôde ser lido");
            Vec::new()
        }
    };
    let Some(i) = msgs.iter().position(|ev| ev.id == p.event_id) else {
        return refused(404, "erro_trecho_nao_encontrado", "a mensagem não está mais nessa conversa", json!({}));
    };
    let window = &msgs[i.saturating_sub(p.around)..(i + p.around + 1).min(msgs.len())];
    match serde_json::to_value(window) {
        Ok(body) => json_response(StatusCode::OK, body),
        Err(_) => refused(500, "search_context_json", "o trecho não pôde ser montado", json!({})),
    }
}
