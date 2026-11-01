//! Leitura de uma conversa do Arquivo: `GET /api/archive/{project}/{session_id}/history` e a imagem
//! colada nela (`.../transcript-image/{uuid}/{idx}`). Port de `archive_history`/`archive_image`
//! (api.py), `archive.tail_events` e dos parsers do Pi e do Kimi.
use std::{
    io::{self, Read, Seek, SeekFrom},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, LazyLock},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, Path as RoutePath, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::{
    routes::{AppState, cors, gate, pass, route_failed},
    session_open::{AgentProvider, OpenError, archive},
    speech::history::transfer_history as transfer,
    session_write::{detail_body, json_response},
    transcript::{HistoryRequest, Provider, TAIL_WINDOW, event, history, py},
};

/// O teto do `tail` (api.py: `min(tail, 200)`).
const TAIL_MAX: i64 = 200;

/// As duas rotas, para o `migration_status`.
pub fn matches(method: &Method, path: &str) -> bool {
    if *method != Method::GET { return false; }
    let Some(rest) = path.strip_prefix("/api/archive/") else { return false };
    let parts: Vec<&str> = rest.split('/').collect();
    matches!(parts.as_slice(), [p, s, "history"] if !p.is_empty() && !s.is_empty())
        || matches!(parts.as_slice(), [p, s, "transcript-image", u, i] if ![p, s, u, i].iter().any(|x| x.is_empty()))
}

/// O que a leitura decidiu: a resposta pronta ou o transcript que não abriu.
enum Outcome { Done(Response), Unreadable }

fn int_parsing(loc: [&str; 2], input: &str) -> Response {
    json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"type": "int_parsing", "loc": loc,
        "msg": "Input should be a valid integer, unable to parse string as an integer", "input": input}]}))
}

pub(crate) fn reply(error: OpenError) -> Response {
    let message = match error.code {
        Some("codex_account_ambiguous_rollout") => Some("a conversa Codex existe em mais de uma conta; escolha a conta"),
        Some("codex_account_archive_mismatch") => Some("a conta escolhida não é a origem desta conversa Codex"),
        Some("codex_account_not_found") => Some("conta Codex não encontrada"),
        Some("codex_account_invalid_name") => Some("nome de conta Codex inválido"),
        Some("codex_account_invalid_marker") => Some("conta Codex inválida"),
        _ => None,
    };
    let error = match message { Some(m) => OpenError { msg: m.into(), ..error }, None => error };
    crate::session_open::server::reply(&error)
}

/// O que as duas rotas validam antes de procurar a conversa (api.py), na ordem do Python.
struct Asked { config_dir: Option<String>, provider: AgentProvider, codex_account: Option<String> }

fn asked(st: &AppState, query: Option<&str>) -> Result<Asked, OpenError> {
    let config_dir = crate::auth::query_param(query, "config_dir");
    if let Some(dir) = &config_dir {
        let catalog = st.accounts.claude_catalog().map_err(|e| OpenError::new(e.status, e.code, e.message))?;
        if !catalog.as_array().into_iter().flatten().any(|row| row["path"].as_str() == Some(dir)) {
            return Err(OpenError::new(400, "erro_config_dir_invalido", "config_dir invalido"));
        }
    }
    let provider = crate::auth::query_param(query, "provider").unwrap_or_else(|| "claude".into());
    let provider = AgentProvider::parse(&provider).ok_or_else(|| OpenError::new(400, "erro_provider_invalido", "provider invalido"))?;
    Ok(Asked { config_dir, provider, codex_account: crate::auth::query_param(query, "codex_account") })
}

/// A conversa pedida; no Codex vêm junto o registro da transferência Claude→Codex de onde ela veio e
/// a casa da conta dona.
fn find(st: &AppState, project: &str, sid: &str, a: &Asked) -> Result<(PathBuf, Option<(Value, PathBuf)>), OpenError> {
    let env = &st.accounts.env;
    let server_projects = env.claude_base.join("projects");
    let config_dirs: Vec<String> = st.accounts.claude_catalog().map_err(|e| OpenError::new(e.status, e.code, e.message))?
        .as_array().into_iter().flatten().filter_map(|row| row["path"].as_str().map(str::to_owned)).collect();
    let w = archive::Where { home: &env.home, env: &env.base, server_projects: &server_projects, config_dirs: &config_dirs, live: &[] };
    let (path, codex_home) = archive::locate(&w, project, sid, a.config_dir.as_deref(), a.provider, a.codex_account.as_deref())?;
    let Some(codex_home) = codex_home else { return Ok((path, None)) };
    let record = transfer::archive_record(&env.home, &codex_home, &crate::locate::session_key(&path.to_string_lossy()))
        .map_err(|e| transfer_failed(st, &path, e))?;
    Ok((path, record.map(|r| (r, codex_home))))
}

/// A causa real de uma falha cujo status a rota mantém, no diário, uma vez por `at`.
fn note(st: &AppState, code: &'static str, at: &str, e: impl std::fmt::Display) {
    st.search.report_once(code, at, || format!("{at}: {e}"));
}

/// `history_invalid` com a causa no diário.
fn transfer_failed(st: &AppState, at: &Path, e: impl std::fmt::Display) -> OpenError {
    note(st, "arquivo.transferencia_ilegivel", &at.to_string_lossy(), e);
    history_invalid()
}

/// Uma fonte inválida da transferência nunca vira conversa vazia nem o rollout cru.
fn history_invalid() -> OpenError {
    OpenError::new(409, "session_transfer_history_invalid", "histórico da transferência indisponível")
}

/// `archived_image`: a imagem `transfer:` pelo registro e pela thread exata, na conta registrada, sem
/// procurar noutras contas; o rollout movido para `archived_sessions` da mesma conta ainda vale.
fn archived_image(st: &AppState, sid: &str, event_id: &str, idx: i64, asked: Option<&str>) -> Result<Option<(Vec<u8>, String)>, OpenError> {
    let parts: Vec<&str> = event_id.splitn(4, ':').collect();
    if parts.len() != 4 || parts[2] != "claude" { return Ok(None); }
    let id = canonical_uuid(parts[1]).ok_or_else(not_found)?;
    let home = &st.accounts.env.home;
    let record = match transfer::load_record(home, &id) {
        Ok(Some(record)) => record,
        Ok(None) => return Ok(None),
        Err(e) => {
            note(st, "arquivo.transferencia_ilegivel", &id, e);
            return Err(not_found());
        }
    };
    let boundary = &record["boundary"];
    if record["phase"] != "complete" || !boundary.is_object() || !record["source"].is_object() { return Err(history_invalid()); }
    if boundary["thread_id"].as_str() != Some(sid) { return Ok(None); }
    if boundary["prefix_digest"].as_str().is_none_or(str::is_empty) { return Err(history_invalid()); }
    let rollout = PathBuf::from(boundary["rollout_path"].as_str().unwrap_or_default());
    let service = crate::accounts::AccountService::new(st.accounts.env.clone());
    let accounts = service.snapshot(crate::accounts::Provider::Codex).map_err(|e| OpenError::new(e.status, e.code, e.message))?;
    let owner = archive::codex_owner(&accounts, &rollout).map_err(OpenError::from)?;
    let same = |a: &Path, b: &Path| a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(a, b)| a == b);
    let registered = record["destination_meta"]["codex_home"].as_str().filter(|h| !h.is_empty());
    let Some(owner) = owner.filter(|o| registered.is_some_and(|h| same(&o.home, Path::new(h)))) else { return Err(history_invalid()) };
    if let Some(asked) = asked && asked != owner.id {
        return Err(OpenError::new(409, "codex_account_archive_mismatch", "conta")
            .with_params(json!({"account_id": asked, "origin_account": owner.id})));
    }
    let rollout = if rollout.is_file() { rollout } else {
        let name = rollout.file_name().map(|n| n.to_owned()).unwrap_or_default();
        let mut found = Vec::new();
        walk(st, &owner.home.join("archived_sessions"), &name, &mut found);
        found.retain(|c| archive::codex_owner(&accounts, c).ok().flatten().is_some_and(|o| same(&o.home, &owner.home)));
        found.sort_by_key(|c| c.canonicalize().unwrap_or_else(|_| c.clone()));
        found.dedup_by_key(|c| c.canonicalize().unwrap_or_else(|_| c.clone()));
        let [only] = found.as_slice() else { return Err(history_invalid()) };
        only.clone()
    };
    transfer::verify_boundary(&record, &rollout, &owner.home).map_err(|e| transfer_failed(st, &rollout, e))?;
    transfer::historical_image(&record, event_id, idx).map_err(|e| transfer_failed(st, &rollout, e))
}

/// Os arquivos de nome `name` sob `dir`, em qualquer profundidade; pasta ilegível vai ao diário.
fn walk(st: &AppState, dir: &Path, name: &std::ffi::OsStr, out: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return,
        Err(e) => return note(st, "arquivo.pasta_ilegivel", &dir.to_string_lossy(), e),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() { walk(st, &path, name, out); } else if path.file_name() == Some(name) && path.is_file() { out.push(path); }
    }
}

pub async fn history(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
                     RoutePath((project, sid)): RoutePath<(String, String)>, req: Request) -> Response {
    let (fwd, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || req.method() != Method::GET { return pass(&st, req, &fwd).await; }
    let query = req.uri().query().map(str::to_owned);
    let tail = match crate::auth::query_param(query.as_deref(), "tail") {
        None => 0,
        Some(raw) => match raw.trim().parse::<i64>() { Ok(n) => n, Err(_) => return int_parsing(["query", "tail"], &raw) },
    };
    let worker = st.clone();
    let done = tokio::task::spawn_blocking(move || -> Result<Outcome, OpenError> {
        let a = asked(&worker, query.as_deref())?;
        let (path, transfer) = find(&worker, &project, &sid, &a)?;
        let tail = (tail > 0).then(|| tail.min(TAIL_MAX) as usize);
        let read = match transfer {
            Some((record, owner_home)) => {
                let mut evs = transfer::archived_history(&record, &path, &owner_home).map_err(|e| transfer_failed(&worker, &path, e))?;
                if let Some(n) = tail {
                    evs.retain(is_message);
                    evs.drain(..evs.len().saturating_sub(n));
                }
                Ok(evs)
            }
            None => read_history(&worker, &path, a.provider, tail),
        };
        let at = path.to_string_lossy();
        Ok(match read {
            Ok(evs) => match serde_json::to_vec(&evs) {
                Ok(body) => Outcome::Done(([(header::CONTENT_TYPE, "application/json")], body).into_response()),
                Err(e) => { note(&worker, "arquivo.historico_json", &at, e); Outcome::Unreadable }
            },
            Err(e) => { note(&worker, "arquivo.transcript_ilegivel", &at, e); Outcome::Unreadable }
        })
    }).await;
    let mut response = match done {
        Ok(Ok(Outcome::Done(response))) => response,
        Ok(Ok(Outcome::Unreadable)) => route_failed(&st, req.headers(), "rust.archive_history_failed", "archive", "archive_history_io", "o transcript não pôde ser lido"),
        Ok(Err(error)) => reply(error),
        Err(_) => route_failed(&st, req.headers(), "rust.archive_history_failed", "archive", "archive_history_panic", "a leitura da conversa caiu no servidor"),
    };
    cors(req.headers(), response.headers_mut());
    response
}

/// O histórico inteiro ou as últimas `tail` mensagens com texto.
pub(crate) fn read_history(st: &AppState, path: &Path, provider: AgentProvider, tail: Option<usize>) -> io::Result<Vec<ChatEvent>> {
    if provider == AgentProvider::Claude {
        // Nome de fila que não existe: só o transcript, como o `merged_history("__archive__")`.
        let req = HistoryRequest { provider: Provider::Claude, jsonl: path.to_owned(),
            queue: Some(crate::locate::queue_path(&st.accounts.env.claude_base, "__archive__")), limit: None, tail_window: TAIL_WINDOW };
        return match tail { Some(n) => history::message_tail(&req, n), None => history::merged_history(&req) };
    }
    match tail {
        None => {
            let raw = std::fs::read(path)?;
            Ok(raw.split(|b| *b == b'\n').flat_map(|line| agent_line(provider, line)).collect())
        }
        Some(n) => agent_tail(path, provider, n),
    }
}

pub(crate) fn is_message(ev: &ChatEvent) -> bool {
    matches!(ev.kind, ChatKind::UserMsg | ChatKind::AssistantMsg) && ev.text.as_deref().is_some_and(|t| !t.is_empty())
}

/// `tail_events` fora do Claude: a janela de 256 KB cresce ×8 até juntar `n` mensagens.
fn agent_tail(path: &Path, provider: AgentProvider, n: usize) -> io::Result<Vec<ChatEvent>> {
    let mut window = TAIL_WINDOW;
    loop {
        let mut file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        let start = size.saturating_sub(window);
        file.seek(SeekFrom::Start(start))?;
        let mut raw = Vec::new();
        file.read_to_end(&mut raw)?;
        // O começo da janela pode cair no meio de uma linha.
        let lines = raw.split(|b| *b == b'\n').skip(usize::from(start > 0));
        let mut msgs: Vec<ChatEvent> = lines.flat_map(|line| agent_line(provider, line)).filter(is_message).collect();
        if start == 0 || msgs.len() >= n {
            msgs.drain(..msgs.len().saturating_sub(n));
            return Ok(msgs);
        }
        window = window.saturating_mul(8);
    }
}

pub(crate) fn agent_line(provider: AgentProvider, raw: &[u8]) -> Vec<ChatEvent> {
    if provider == AgentProvider::Codex {
        return crate::transcript::LineParser::new(Provider::Codex).feed(raw, 0);
    }
    let Some(Value::Object(obj)) = crate::transcript::decode_line(raw) else { return Vec::new() };
    match provider {
        AgentProvider::Kimi => kimi_obj(&obj),
        _ => pi_obj(&obj),
    }
}

// adapters/pi/transcript.py `_ANSI_RE`: o Pi grava o texto já colorido.
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:?]*[ -/]*[@-~]").unwrap());

fn clean(text: &str) -> String { ANSI.replace_all(text, "").into_owned() }

/// `bool(valor)` do Python.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// O texto do bloco que tem conteúdo depois do `strip()`.
fn block_text<'a>(block: &'a Value, field: &str) -> Option<&'a str> {
    block[field].as_str().filter(|t| !py::strip(t).is_empty())
}

fn sub_id(id: &str, k: usize) -> String { if k == 0 { id.to_owned() } else { format!("{id}:{k}") } }

fn args(v: &Value) -> Option<Map<String, Value>> { Some(v.as_object().cloned().unwrap_or_default()) }

/// `adapters/pi/transcript.parse_obj` (Pi e omp).
fn pi_obj(obj: &Map<String, Value>) -> Vec<ChatEvent> {
    if obj.get("type").and_then(Value::as_str) != Some("message") { return Vec::new(); }
    let Some(msg) = obj.get("message").and_then(Value::as_object) else { return Vec::new() };
    let node = obj.get("id").and_then(Value::as_str).unwrap_or("");
    let ts = msg.get("timestamp").and_then(Value::as_f64).map(|t| t / 1000.0);
    let Some(content) = msg.get("content").and_then(Value::as_array) else { return Vec::new() };
    let mut out = Vec::new();
    match msg.get("role").and_then(Value::as_str) {
        Some("toolResult") => {
            let parts: Vec<String> = content.iter().filter_map(|b| match b["type"].as_str() {
                Some("text") => Some(clean(b["text"].as_str().unwrap_or(""))),
                Some("image") => Some(format!("[imagem {}]", b["mimeType"].as_str().filter(|m| !m.is_empty()).unwrap_or("desconhecida"))),
                _ => None,
            }).filter(|p| !p.is_empty()).collect();
            out.push(ChatEvent { tool_use_id: msg.get("toolCallId").and_then(Value::as_str).map(str::to_owned), result: Some(parts.join("\n")),
                is_error: Some(truthy(msg.get("isError"))), ts, ..event(ChatKind::ToolResult, node.to_owned()) });
        }
        Some("user") => {
            for (k, b) in content.iter().enumerate() {
                if b["type"] == "text" && let Some(text) = block_text(b, "text") {
                    out.push(ChatEvent { text: Some(clean(text)), ts, ..event(ChatKind::UserMsg, sub_id(node, k)) });
                }
            }
        }
        Some("assistant") => {
            let cache_read = msg.get("usage").and_then(|u| u.get("cacheRead")).and_then(Value::as_u64);
            for (k, b) in content.iter().enumerate() {
                match b["type"].as_str() {
                    Some("text") => if let Some(text) = block_text(b, "text") {
                        out.push(ChatEvent { text: Some(clean(text)), cache_read, ts, ..event(ChatKind::AssistantMsg, sub_id(node, k)) });
                    },
                    Some("thinking") => if let Some(text) = block_text(b, "thinking") {
                        out.push(ChatEvent { text: Some(clean(text)), ts, ..event(ChatKind::Thinking, sub_id(node, k)) });
                    },
                    Some("toolCall") => out.push(ChatEvent { tool_name: b["name"].as_str().map(str::to_owned), tool_input: args(&b["arguments"]),
                        tool_use_id: b["id"].as_str().map(str::to_owned), ts, ..event(ChatKind::ToolUse, sub_id(node, k)) }),
                    _ => {}
                }
            }
        }
        _ => {}
    }
    out
}

/// `adapters/kimi/transcript.parse_obj` (o `wire.jsonl`).
fn kimi_obj(obj: &Map<String, Value>) -> Vec<ChatEvent> {
    let ts = obj.get("time").and_then(Value::as_f64).map(|t| t / 1000.0);
    let kind = obj.get("type").and_then(Value::as_str);
    if kind == Some("context.append_message") {
        let Some(msg) = obj.get("message").and_then(Value::as_object) else { return Vec::new() };
        // O `injection` do modo de permissão é contexto do sistema, não bolha.
        if msg.get("role").and_then(Value::as_str) != Some("user") || msg.get("origin").and_then(|o| o.get("kind")).and_then(Value::as_str) != Some("user") {
            return Vec::new();
        }
        let base = msg.get("id").and_then(Value::as_str).unwrap_or("");
        let Some(content) = msg.get("content").and_then(Value::as_array) else { return Vec::new() };
        return content.iter().enumerate().filter(|(_, b)| b["type"] == "text").filter_map(|(k, b)| block_text(b, "text").map(|t| (k, t)))
            .map(|(k, text)| ChatEvent { text: Some(text.to_owned()), ts, ..event(ChatKind::UserMsg, sub_id(base, k)) }).collect();
    }
    if kind != Some("context.append_loop_event") { return Vec::new(); }
    let Some(ev) = obj.get("event").and_then(Value::as_object) else { return Vec::new() };
    let id = ev.get("uuid").and_then(Value::as_str).unwrap_or("").to_owned();
    let call = ev.get("toolCallId").and_then(Value::as_str);
    match ev.get("type").and_then(Value::as_str) {
        Some("content.part") => {
            let Some(part) = ev.get("part") else { return Vec::new() };
            if part["type"] == "text" && let Some(text) = block_text(part, "text") {
                return vec![ChatEvent { text: Some(text.to_owned()), ts, ..event(ChatKind::AssistantMsg, id) }];
            }
            if part["type"] == "think" && let Some(text) = block_text(part, "think") {
                return vec![ChatEvent { text: Some(text.to_owned()), ts, ..event(ChatKind::Thinking, id) }];
            }
            Vec::new()
        }
        Some("tool.call") => vec![ChatEvent { tool_name: ev.get("name").and_then(Value::as_str).map(str::to_owned),
            tool_input: args(ev.get("args").unwrap_or(&Value::Null)), tool_use_id: call.map(str::to_owned), ts, ..event(ChatKind::ToolUse, id) }],
        Some("tool.result") => {
            let result = ev.get("result");
            let text = match result {
                Some(Value::Object(o)) => match o.get("output") {
                    Some(Value::String(s)) => s.clone(),
                    _ => crate::transcript::pyjson::dumps_unicode(result.unwrap(), false),
                },
                Some(Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            // O `tool.result` não tem `uuid`: sem o prefixo todos os resultados ocupariam o mesmo id.
            let cid = [call, ev.get("parentUuid").and_then(Value::as_str), Some(id.as_str())].into_iter().flatten().find(|s| !s.is_empty());
            if cid.is_none() { tracing::warn!(code = "kimi_tool_result_sem_id", "kimi: tool.result sem toolCallId/parentUuid/uuid"); }
            vec![ChatEvent { tool_use_id: call.map(str::to_owned), result: Some(text), is_error: Some(truthy(result.and_then(|r| r.get("isError")))),
                ts, ..event(ChatKind::ToolResult, cid.map(|c| format!("res:{c}")).unwrap_or_default()) }]
        }
        _ => Vec::new(),
    }
}

/// `uuid.UUID(texto)` do Python, na forma canônica.
fn canonical_uuid(text: &str) -> Option<String> {
    let hex: String = text.replace("urn:", "").replace("uuid:", "").trim_matches(['{', '}']).replace('-', "");
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) { return None; }
    let h = hex.to_ascii_lowercase();
    Some(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

fn not_found() -> OpenError { OpenError::new(404, "erro_nao_encontrado", "not found") }
fn no_image() -> Response { json_response(StatusCode::NOT_FOUND, detail_body("erro_imagem_nao_encontrada", "image not found", json!({}))) }

pub async fn image(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
                   RoutePath((project, sid, uuid, idx)): RoutePath<(String, String, String, String)>, req: Request) -> Response {
    let (fwd, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if !owner || req.method() != Method::GET { return pass(&st, req, &fwd).await; }
    let Ok(idx) = idx.trim().parse::<i64>() else { return int_parsing(["path", "idx"], &idx) };
    let query = req.uri().query().map(str::to_owned);
    let worker = st.clone();
    let done = tokio::task::spawn_blocking(move || -> Result<Outcome, OpenError> {
        let a = asked(&worker, query.as_deref())?;
        let found = if uuid.starts_with("transfer:") {
            archived_image(&worker, &sid, &uuid, idx, a.codex_account.as_deref())?
        } else {
            let (path, transfer) = find(&worker, &project, &sid, &a).map_err(|e| match e.code {
                Some("erro_path_invalido" | "erro_transcript_nao_encontrado") => not_found(),
                _ => e,
            })?;
            match transfer {
                Some((record, _)) => transfer::historical_image(&record, &uuid, idx).map_err(|e| transfer_failed(&worker, &path, e))?,
                None => match crate::session_media::find_image(&path, &uuid, idx) {
                    Ok(found) => found.map(|(raw, media)| (raw, media.to_owned())),
                    Err(e) => {
                        note(&worker, "arquivo.transcript_ilegivel", &path.to_string_lossy(), e);
                        return Ok(Outcome::Unreadable);
                    }
                },
            }
        };
        let Some((raw, media)) = found else { return Ok(Outcome::Done(no_image())) };
        let mut response = Response::new(Body::from(raw));
        let headers = response.headers_mut();
        headers.insert(header::CONTENT_TYPE, HeaderValue::from_str(&media).unwrap_or(HeaderValue::from_static("image/png")));
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("max-age=31536000, immutable"));
        headers.insert(header::X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
        Ok(Outcome::Done(response))
    }).await;
    let mut response = match done {
        Ok(Ok(Outcome::Done(response))) => response,
        Ok(Ok(Outcome::Unreadable)) => route_failed(&st, req.headers(), "rust.archive_image_failed", "archive", "archive_image_io", "o transcript não pôde ser lido"),
        Ok(Err(error)) => reply(error),
        Err(_) => route_failed(&st, req.headers(), "rust.archive_image_failed", "archive", "archive_image_panic", "a leitura da imagem caiu no servidor"),
    };
    cors(req.headers(), response.headers_mut());
    response
}
