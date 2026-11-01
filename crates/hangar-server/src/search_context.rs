//! `GET /api/search/context`: a mensagem achada pela busca e as vizinhas dela, só as do usuário e as do
//! assistente com texto. Port de `search_context` (api.py) com `archive_jsonl` e
//! `merged_history("__archive__", …)`; `provider` (e `codex_account`) leem Codex, Pi, omp e Kimi pela
//! leitura do histórico do Arquivo. Só o dono; o resto segue ao Python, que recusa.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use hangar_api::chat::ChatEvent;
use regex::Regex;
use serde_json::{Value, json};

use crate::routes::{AppState, cors, gate, pass, route_failed};
use crate::session_open::AgentProvider;
use crate::session_open::archive::{Where, locate};
use crate::session_write::{detail_body, json_response};
use crate::speech::history::transfer_history as transfer;

const DEFAULT_AROUND: usize = 3;
const MAX_AROUND: f64 = 10.0;
// O `int` do FastAPI aceita espaço em volta, `_` entre dígitos e `3.0`.
pub(crate) static PY_INT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[+-]?[0-9](?:_?[0-9])*(?:\.0*)?\z").unwrap());

struct Params { project: String, session_id: String, event_id: String, around: usize, provider: Option<String>, codex_account: Option<String>,
    config_dir: Option<String> }

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
        (Some(project), Some(session_id), Some(event_id), Some(around)) if errors.is_empty() => Ok(Params { project, session_id, event_id, around,
            provider: query.get("provider").cloned(), codex_account: query.get("codex_account").cloned(),
            config_dir: query.get("config_dir").cloned() }),
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
    // A conta do trecho, quando a mesma conversa existe em mais de uma.
    if p.config_dir.as_ref().is_some_and(|dir| !config_dirs.contains(dir)) {
        return refused(400, "erro_config_dir_invalido", "config_dir invalido", json!({}));
    }
    // Sem `provider` é Claude (CTX-08); os outros agentes leem pelo parser do histórico do Arquivo.
    let Some(provider) = AgentProvider::parse(p.provider.as_deref().unwrap_or("claude")) else {
        return refused(400, "erro_provider_invalido", "provider invalido", json!({}));
    };
    let server_projects = env.claude_base.join("projects");
    let w = Where { home: &env.home, env: &env.base, server_projects: &server_projects, config_dirs: &config_dirs, live: &[] };
    let (jsonl, codex_home) = match locate(&w, &p.project, &p.session_id, p.config_dir.as_deref(), provider, p.codex_account.as_deref()) {
        Ok(found) => found,
        Err(e) => return refused(e.status, e.code.unwrap_or("erro_path_invalido"), &e.msg, e.params),
    };
    // Rollout transferido do Claude: os ids do índice são os do histórico composto, como no Arquivo.
    let composed = codex_home.as_deref().map_or(Ok(None), |home| {
        transfer::archive_record(&env.home, home, &crate::locate::session_key(&jsonl.to_string_lossy()))
            .and_then(|record| record.map(|r| transfer::archived_history(&r, &jsonl, home)).transpose())
    });
    let read = match composed {
        Ok(Some(evs)) => Ok(evs),
        Ok(None) => crate::archive_history::read_history(st, &jsonl, provider, None),
        Err(e) => Err(e),
    };
    let msgs: Vec<ChatEvent> = match read {
        Ok(evs) => evs.into_iter().filter(crate::archive_history::is_message).collect(),
        // Sumiu depois de achado: o trecho não está mais nele.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        // Ilegível não é conversa vazia: o 404 esconderia a falha.
        Err(e) => {
            let at = jsonl.to_string_lossy();
            st.search.report_once("busca.contexto_ilegivel", &at, || format!("{at}: {e}"));
            return refused(503, "search_context_io", "o transcript não pôde ser lido", json!({}));
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
