//! Leitura de uma conversa do Arquivo: `GET /api/archive/{project}/{session_id}/history` e a imagem
//! colada nela (`.../transcript-image/{uuid}/{idx}`). Port de `archive_history`/`archive_image`
//! (api.py), `archive.tail_events` e dos parsers do Pi e do Kimi.
use std::{
    io::{self, Read, Seek, SeekFrom},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use axum::{
    body::Body,
    extract::{ConnectInfo, Path as RoutePath, Request, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use hangar_api::chat::{ChatEvent, ChatKind};
use serde_json::{Value, json};

use crate::{
    routes::{AppState, cors, gate, pass, route_failed},
    session_open::{AgentProvider, OpenError, archive},
    speech::history::transfer_history as transfer,
    session_write::{detail_body, json_response},
    transcript::{HistoryRequest, Provider, TAIL_WINDOW, history},
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

fn int_parsing(req: &axum::http::HeaderMap, loc: [&str; 2], input: &str) -> Response {
    let mut response = json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"type": "int_parsing", "loc": loc,
        "msg": "Input should be a valid integer, unable to parse string as an integer", "input": input}]}));
    cors(req, response.headers_mut());
    response
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
    let mut config_dir = crate::auth::query_param(query, "config_dir");
    if let Some(dir) = &config_dir {
        let catalog = st.accounts.claude_catalog().map_err(|e| OpenError::new(e.status, e.code, e.message))?;
        let known = crate::session_open::resolve::known_config_dir(
            catalog.as_array().into_iter().flatten().filter_map(|row| row["path"].as_str()), dir);
        let Some(known) = known else { return Err(OpenError::new(400, "erro_config_dir_invalido", "config_dir invalido")) };
        config_dir = Some(known.to_owned());
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
        Some(raw) => match raw.trim().parse::<i64>() { Ok(n) => n, Err(_) => return int_parsing(req.headers(), ["query", "tail"], &raw) },
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
            Ok(agent_lines(provider, raw.split(|b| *b == b'\n')))
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
        let mut msgs: Vec<ChatEvent> = agent_lines(provider, lines).into_iter().filter(is_message).collect();
        if start == 0 || msgs.len() >= n {
            msgs.drain(..msgs.len().saturating_sub(n));
            return Ok(msgs);
        }
        window = window.saturating_mul(8);
    }
}

fn transcript_provider(provider: AgentProvider) -> Provider {
    match provider {
        AgentProvider::Claude => Provider::Claude,
        AgentProvider::Codex => Provider::Codex,
        AgentProvider::Pi => Provider::Pi,
        AgentProvider::Omp => Provider::Omp,
        AgentProvider::Kimi => Provider::Kimi,
    }
}

fn agent_lines<'a>(provider: AgentProvider, lines: impl Iterator<Item = &'a [u8]>) -> Vec<ChatEvent> {
    let mut parser = crate::transcript::LineParser::new(transcript_provider(provider));
    let mut events = Vec::new();
    let mut offset = 0;
    for line in lines {
        events.extend(parser.feed(line, offset));
        offset += line.len() as u64 + 1;
    }
    events.extend(parser.flush());
    events
}

/// Leitura isolada para consumidores que não atravessam fronteiras de linha.
pub(crate) fn agent_line(provider: AgentProvider, raw: &[u8]) -> Vec<ChatEvent> {
    let Some(value) = crate::transcript::decode_line(raw) else { return Vec::new() };
    match provider {
        AgentProvider::Kimi => hangar_transcript::parse_kimi_object(&value),
        AgentProvider::Pi | AgentProvider::Omp => hangar_transcript::parse_pi_object(&value),
        _ => crate::transcript::LineParser::new(transcript_provider(provider)).feed(raw, 0),
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
    let Ok(idx) = idx.trim().parse::<i64>() else { return int_parsing(req.headers(), ["path", "idx"], &idx) };
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
