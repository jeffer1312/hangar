//! Rotas do sonho: as das telas e as que a máquina que sonha chama nos peers. Todas só do dono; o
//! convidado recebe 403. Uma rota que grava memória só aceita a máquina registrada como a que sonha.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::Request;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use chrono::{DateTime, Utc};
use hangar_dream::config::Config;
use hangar_dream::memory::{self, WriteError};
use hangar_dream::project::memory_dir;
use hangar_dream::render::{self, Header};
use hangar_dream::state::Mode;
use serde::Deserialize;
use serde_json::{json, Value};

use super::round::{self, Options, USER_MEMORY_PATH};
use super::{inventory, load_config, load_state, peers, this_machine, Paths, RUNNING, WAKE};
use crate::routes::AppState;

const ROUTES: &[(&str, &str)] = &[
    ("/api/dream/config", "GET"),
    ("/api/dream/config", "PUT"),
    ("/api/dream/status", "GET"),
    ("/api/dream/run", "POST"),
    ("/api/dream/runs/{id}", "GET"),
    ("/api/dream/runs/{id}/undo", "POST"),
    ("/api/dream/project", "PUT"),
    ("/api/dream/suggestions/dismiss", "POST"),
    ("/api/dream/inventory", "GET"),
    ("/api/dream/transcript", "GET"),
    ("/api/dream/memory", "GET"),
    ("/api/dream/memory", "PUT"),
    ("/api/dream/user-memory", "PUT"),
];

const BODY_LIMIT: usize = 2 * 1024 * 1024;
const USER_MEMORY_LIMIT: usize = 64 * 1024;

pub fn matches(_method: &Method, path: &str) -> bool {
    let path = path.trim_end_matches('/');
    path == "/api/dream" || path.starts_with("/api/dream/")
}

pub async fn route(st: Arc<AppState>, peer: std::net::SocketAddr, req: Request) -> Response {
    let headers = req.headers().clone();
    let method = req.method().clone();
    let mut response = route_inner(st, peer, req).await;
    crate::routes::cors(&headers, response.headers_mut());
    crate::http_route_response::suppress_head(&method, &mut response);
    response
}

async fn route_inner(st: Arc<AppState>, peer: std::net::SocketAddr, req: Request) -> Response {
    let path = req.uri().path().trim_end_matches('/').to_string();
    if let Some(response) = crate::http_cors::preflight(req.method(), req.headers()) {
        return response;
    }
    let decision = crate::http_contract::resolve(ROUTES, req.method().as_str(), &path);
    let forward = crate::routes::forward_for(&st, peer, &req);
    if let Some(response) = crate::http_route_response::response(&decision, req.method(), req.uri(), req.headers(), forward.https, "") {
        return response;
    }
    match crate::routes::who(&st, peer, &req).await {
        Err(e) => return crate::auth::unavailable(e),
        Ok((_, Some(crate::share::caller::Caller::Guest(_)))) => return error(403, "guest_forbidden", "convidado não acessa o sonho da memória"),
        _ => {}
    }
    if let Err(response) = crate::routes::owner_gate(&st, peer, &req).await {
        return response;
    }
    let query: HashMap<String, String> = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
    let method = req.method().clone();
    match (method.as_str(), path.as_str()) {
        ("GET", "/api/dream/config") => get_config(&st).await,
        ("PUT", "/api/dream/config") => put_config(&st, req).await,
        ("GET", "/api/dream/status") => status(&st).await,
        ("POST", "/api/dream/run") => start(&st, req).await,
        ("PUT", "/api/dream/project") => put_project(&st, &query, req).await,
        ("POST", "/api/dream/suggestions/dismiss") => dismiss(&st, req).await,
        ("GET", "/api/dream/inventory") => serve_inventory(&st, &query).await,
        ("GET", "/api/dream/transcript") => serve_transcript(&st, &query).await,
        ("GET", "/api/dream/memory") => serve_memory(&st, &query).await,
        ("PUT", "/api/dream/memory") => receive_memory(&st, req).await,
        ("PUT", "/api/dream/user-memory") => receive_user_memory(&st, req).await,
        ("GET", p) if p.starts_with("/api/dream/runs/") => run_detail(&st, &p["/api/dream/runs/".len()..]),
        ("POST", p) if p.starts_with("/api/dream/runs/") && p.ends_with("/undo") => {
            undo(&st, &p["/api/dream/runs/".len()..p.len() - "/undo".len()], query.get("project").map(String::as_str)).await
        }
        _ => error(404, "not_found", "rota do sonho desconhecida"),
    }
}

fn ok(value: Value) -> Response {
    hangar_core::http::json_response(StatusCode::OK, value)
}

fn error(status: u16, code: &str, msg: &str) -> Response {
    hangar_core::http::detail(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), code, msg, json!({}))
}

/// Erro interno com código no começo (`dream_xxx: detalhe`): 409 para estado, 500 para o resto.
fn failure(message: &str) -> Response {
    let code = message.split(':').next().unwrap_or("dream_failed");
    let status = match code {
        "dream_running" | "dream_not_dreamer" | "dream_memory_changed" | "dream_locked" => 409,
        "dream_run_not_found" => 404,
        _ => 500,
    };
    error(status, code, message)
}

async fn body<T: for<'de> Deserialize<'de>>(req: Request) -> Result<T, Response> {
    let bytes = match tokio::time::timeout(Duration::from_secs(15), axum::body::to_bytes(req.into_body(), BODY_LIMIT)).await {
        Ok(Ok(bytes)) => bytes,
        _ => return Err(error(422, "erro_corpo_invalido", "corpo inválido")),
    };
    serde_json::from_slice(&bytes).map_err(|_| error(422, "erro_corpo_invalido", "corpo inválido"))
}

/// A chave do modelo auxiliar, quando o endereço dele é a Anthropic.
fn aux_key(st: &AppState) -> Option<String> {
    let values = crate::config_sync::runtime::RuntimeConfig::from_state(st).ok()?.read();
    let url = values.get("jev_aux_base_url")?.as_str()?;
    let key = values.get("jev_aux_api_key")?.as_str()?;
    (url.contains("api.anthropic.com") && !key.is_empty()).then(|| key.to_string())
}

/// Primeira leitura com a chave vazia: vem a do modelo auxiliar, gravada no campo próprio (depois
/// trocar o auxiliar não tira a chave do sonho).
fn config_with_key(st: &AppState) -> Result<Config, String> {
    let mut config = load_config(st)?;
    if config.api_key.is_empty() {
        if let Some(key) = aux_key(st) {
            config.api_key = key;
            config.save(&Paths::of(st).config()).map_err(|e| format!("dream_io: {e}"))?;
        }
    }
    Ok(config)
}

fn config_json(st: &AppState, config: &Config) -> Value {
    json!({
        "enabled": config.enabled,
        "api_key": crate::engines::store::mask(&config.api_key),
        "hour": config.hour,
        "project_cap_usd": config.project_cap_usd,
        "month_cap_usd": config.month_cap_usd,
        "machine": config.machine,
        "this_machine": this_machine(st),
    })
}

async fn get_config(st: &Arc<AppState>) -> Response {
    match config_with_key(st) {
        Ok(config) => ok(config_json(st, &config)),
        Err(e) => failure(&e),
    }
}

#[derive(Deserialize)]
struct ConfigPatch {
    enabled: Option<bool>,
    api_key: Option<String>,
    hour: Option<String>,
    project_cap_usd: Option<f64>,
    #[serde(default, with = "double_option")]
    month_cap_usd: Option<Option<f64>>,
    machine: Option<String>,
}

mod double_option {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<f64>>, D::Error> {
        Option::<f64>::deserialize(d).map(Some)
    }
}

async fn put_config(st: &Arc<AppState>, req: Request) -> Response {
    let patch: ConfigPatch = match body(req).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let mut config = match config_with_key(st) {
        Ok(c) => c,
        Err(e) => return failure(&e),
    };
    if let Some(v) = patch.enabled {
        config.enabled = v;
    }
    // A tela devolve a máscara quando não mexeu no campo.
    if let Some(key) = patch.api_key.filter(|k| *k != crate::engines::store::mask(&config.api_key)) {
        config.api_key = key.trim().to_string();
    }
    if let Some(v) = patch.hour {
        config.hour = v;
    }
    if let Some(v) = patch.project_cap_usd {
        config.project_cap_usd = v;
    }
    if let Some(v) = patch.month_cap_usd {
        config.month_cap_usd = v;
    }
    if let Some(v) = patch.machine {
        config.machine = v;
    }
    if let Err(field) = config.validate() {
        return hangar_core::http::detail(StatusCode::UNPROCESSABLE_ENTITY, "dream_invalid_config", "valor inválido", json!({"field": field}));
    }
    if let Err(e) = config.save(&Paths::of(st).config()) {
        return failure(&format!("dream_io: {e}"));
    }
    WAKE.notify_one();
    ok(config_json(st, &config))
}

async fn status(st: &Arc<AppState>) -> Response {
    let state = match load_state(st) {
        Ok(s) => s,
        Err(e) => return failure(&e),
    };
    let me = this_machine(st);
    let index = st.search.path().to_owned();
    let since = state.last_round_at;
    let local = tokio::task::spawn_blocking(move || inventory::local_sessions(&index, since)).await.ok().and_then(Result::ok).unwrap_or_default();
    let mut new_sessions: BTreeMap<String, usize> = BTreeMap::new();
    for s in &local {
        if let Some(project) = &s.project {
            *new_sessions.entry(round::project_key(&state, project, &me)).or_default() += 1;
        }
    }
    let mut machines: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (key, entry) in &state.roots {
        let machine = key.split('/').next().unwrap_or_default().to_string();
        let list = machines.entry(round::project_key(&state, &entry.project, &machine)).or_default();
        if !list.contains(&machine) {
            list.push(machine);
        }
    }
    let ids: std::collections::BTreeSet<&String> = state.projects.keys().chain(machines.keys()).collect();
    let projects: Vec<Value> = ids
        .into_iter()
        .map(|id| {
            let p = state.projects.get(id).cloned().unwrap_or_default();
            json!({
                "id": id,
                "name": id.rsplit(['/', ':']).next().unwrap_or(id),
                "machines": machines.get(id).cloned().unwrap_or_default(),
                "new_sessions": new_sessions.get(id).copied().unwrap_or(0),
                "enabled": p.enabled, "mode": p.mode, "last_dream_at": p.last_dream_at,
                "last_cost_usd": p.last_cost_usd, "last_run_id": p.last_run_id, "last_error": p.last_error,
                "needs_confirm": id.starts_with("dir:"),
            })
        })
        .collect();
    let running = RUNNING.lock().unwrap_or_else(|e| e.into_inner()).clone();
    ok(json!({"projects": projects, "month": state.month, "running": running, "suggestions": state.suggestions, "last_round_at": state.last_round_at}))
}

#[derive(Deserialize)]
struct RunBody {
    project: Option<String>,
}

async fn start(st: &Arc<AppState>, req: Request) -> Response {
    let run: RunBody = match body(req).await {
        Ok(b) => b,
        Err(r) => return r,
    };
    match load_config(st) {
        Ok(c) if c.machine != this_machine(st) => return failure("dream_not_dreamer: esta máquina não é a que sonha"),
        Err(e) => return failure(&e),
        _ => {}
    }
    match round::start(st, Options { project: run.project, forced: true }) {
        Ok(run_id) => hangar_core::http::json_response(StatusCode::ACCEPTED, json!({"run_id": run_id})),
        Err(e) => failure(&e),
    }
}

#[derive(Deserialize)]
struct ProjectPatch {
    enabled: Option<bool>,
    mode: Option<Mode>,
    alias_of: Option<String>,
}

async fn put_project(st: &Arc<AppState>, query: &HashMap<String, String>, req: Request) -> Response {
    let Some(id) = query.get("id").cloned() else { return error(422, "erro_corpo_invalido", "falta o id") };
    let patch: ProjectPatch = match body(req).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    let path = Paths::of(st).state();
    let mut state = match load_state(st) {
        Ok(s) => s,
        Err(e) => return failure(&e),
    };
    let entry = state.projects.entry(id.clone()).or_default();
    if let Some(v) = patch.enabled {
        entry.enabled = v;
    }
    if let Some(v) = patch.mode {
        entry.mode = v;
    }
    if let Some(target) = patch.alias_of {
        if target.is_empty() || target == id {
            state.aliases.remove(&id);
        } else {
            state.aliases.insert(id, target);
        }
    }
    match state.save(&path) {
        Ok(()) => ok(json!({"ok": true})),
        Err(e) => failure(&format!("dream_io: {e}")),
    }
}

#[derive(Deserialize)]
struct Dismiss {
    index: usize,
}

async fn dismiss(st: &Arc<AppState>, req: Request) -> Response {
    let d: Dismiss = match body(req).await {
        Ok(d) => d,
        Err(r) => return r,
    };
    let mut state = match load_state(st) {
        Ok(s) => s,
        Err(e) => return failure(&e),
    };
    if d.index < state.suggestions.len() {
        state.suggestions.remove(d.index);
    }
    match state.save(&Paths::of(st).state()) {
        Ok(()) => ok(json!({"ok": true})),
        Err(e) => failure(&format!("dream_io: {e}")),
    }
}

fn run_detail(st: &AppState, run_id: &str) -> Response {
    match round::load_records(&Paths::of(st), run_id) {
        Ok(records) => ok(json!({"run_id": run_id, "projects": records})),
        Err(e) => failure(&e),
    }
}

/// Grava a memória de antes onde a rodada gravou, só se a pasta ainda for a que a rodada deixou.
/// Com `project`, só aquele projeto volta: a rodada da noite cobre vários.
async fn undo(st: &Arc<AppState>, run_id: &str, project: Option<&str>) -> Response {
    let records = match round::load_records(&Paths::of(st), run_id) {
        Ok(r) => r,
        Err(e) => return failure(&e),
    };
    let me = this_machine(st);
    let mut results = Vec::new();
    for record in records.iter().filter(|r| project.map_or(true, |p| p == r.project)) {
        for place in record.places.iter().filter(|p| p.written) {
            let expect = memory::digest(&place.after);
            let result = match &place.dir {
                Some(dir) => {
                    let (dir, before) = (PathBuf::from(dir), place.before.clone());
                    tokio::task::spawn_blocking(move || memory::write(&dir, &before, &expect))
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|r| r.map_err(|e| format!("{e:?}")))
                }
                None => peers::memory_put(st, &place.machine, &place.root, &place.before, &expect, &me).await,
            };
            results.push(json!({"project": record.project, "machine": place.machine, "root": place.root, "ok": result.is_ok(), "error": result.err()}));
        }
    }
    ok(json!({"results": results}))
}

/// Peer: só a máquina registrada aqui como a que sonha grava memória nesta.
fn from_dreamer(st: &AppState, from: &str) -> Result<(), Response> {
    match load_config(st) {
        Ok(c) if !c.machine.is_empty() && c.machine == from => Ok(()),
        Ok(_) => Err(error(403, "dream_not_dreamer", "a máquina que pediu não é a que sonha")),
        Err(e) => Err(failure(&e)),
    }
}

async fn serve_inventory(st: &Arc<AppState>, query: &HashMap<String, String>) -> Response {
    let since = match query.get("since").filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => match DateTime::parse_from_rfc3339(s) {
            Ok(t) => Some(t.with_timezone(&Utc)),
            Err(_) => return error(422, "erro_corpo_invalido", "since inválido"),
        },
    };
    let index = st.search.path().to_owned();
    match tokio::task::spawn_blocking(move || inventory::local_sessions(&index, since)).await {
        Ok(Ok(sessions)) => ok(json!({"machine": this_machine(st), "sessions": sessions})),
        Ok(Err(e)) => failure(&e),
        Err(e) => failure(&format!("dream_index_unavailable: {e}")),
    }
}

/// Uma parte do texto novo de uma sessão; só serve arquivo que está no índice.
async fn serve_transcript(st: &Arc<AppState>, query: &HashMap<String, String>) -> Response {
    let (Some(path), Some(from), Some(part)) = (
        query.get("path").cloned(),
        query.get("from").and_then(|v| v.parse::<usize>().ok()),
        query.get("part").and_then(|v| v.parse::<usize>().ok()),
    ) else {
        return error(422, "erro_corpo_invalido", "path, from e part obrigatórios");
    };
    let until = query.get("to").and_then(|v| v.parse::<usize>().ok());
    let index = st.search.path().to_owned();
    let machine = this_machine(st);
    let result = tokio::task::spawn_blocking(move || -> Result<Option<(Vec<String>, usize)>, String> {
        let conn = rusqlite::Connection::open_with_flags(&index, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|e| format!("dream_index_unavailable: {e}"))?;
        let provider: Option<String> = conn.query_row("SELECT provider FROM files WHERE path=?1", [&path], |r| r.get(0)).ok();
        let Some(provider) = provider.as_deref().and_then(hangar_transcript::Provider::parse) else { return Ok(None) };
        let session = Path::new(&path).file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let r = render::render(provider, Path::new(&path), from, until, &Header { session: &session, machine: &machine }, &|t| hangar_dream::secrets::scrub(t).into_owned())
            .map_err(|e| format!("dream_io: {e}"))?;
        Ok(Some((r.parts, r.end_event)))
    })
    .await;
    match result {
        Ok(Ok(Some((parts, end)))) => ok(json!({"parts": parts.len(), "text": parts.get(part), "end_event": end})),
        Ok(Ok(None)) => error(404, "dream_unknown_transcript", "transcrição fora do índice"),
        Ok(Err(e)) => failure(&e),
        Err(e) => failure(&format!("dream_io: {e}")),
    }
}

async fn serve_memory(st: &Arc<AppState>, query: &HashMap<String, String>) -> Response {
    let Some(root) = query.get("root").filter(|r| Path::new(r).is_absolute()) else {
        return error(422, "erro_corpo_invalido", "raiz absoluta obrigatória");
    };
    match memory::read(&memory_dir(&st.accounts.env.claude_base, Path::new(root))) {
        Ok(snap) => ok(json!({"files": snap.files, "digest": snap.digest})),
        Err(e) => failure(&format!("dream_io: {e}")),
    }
}

#[derive(Deserialize)]
struct MemoryPut {
    root: String,
    files: BTreeMap<String, String>,
    expect_digest: String,
    from_machine: String,
}

async fn receive_memory(st: &Arc<AppState>, req: Request) -> Response {
    let put: MemoryPut = match body(req).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(r) = from_dreamer(st, &put.from_machine) {
        return r;
    }
    if !Path::new(&put.root).is_absolute() {
        return error(422, "erro_corpo_invalido", "raiz absoluta obrigatória");
    }
    let dir = memory_dir(&st.accounts.env.claude_base, Path::new(&put.root));
    match tokio::task::spawn_blocking(move || memory::write(&dir, &put.files, &put.expect_digest)).await {
        Ok(Ok(())) => ok(json!({"ok": true})),
        Ok(Err(WriteError::Changed)) => error(409, "dream_memory_changed", "a memória mudou depois da leitura"),
        Ok(Err(WriteError::Locked)) => error(409, "dream_locked", "outra consolidação em andamento"),
        Ok(Err(WriteError::InvalidName(n))) => error(422, "dream_invalid_name", &n),
        Ok(Err(WriteError::Io(e))) => failure(&format!("dream_io: {e}")),
        Err(e) => failure(&format!("dream_io: {e}")),
    }
}

#[derive(Deserialize)]
struct UserMemoryPut {
    content: String,
    from_machine: String,
}

async fn receive_user_memory(st: &Arc<AppState>, req: Request) -> Response {
    let put: UserMemoryPut = match body(req).await {
        Ok(p) => p,
        Err(r) => return r,
    };
    if let Err(r) = from_dreamer(st, &put.from_machine) {
        return r;
    }
    if put.content.len() > USER_MEMORY_LIMIT {
        return error(422, "erro_corpo_invalido", "arquivo grande demais");
    }
    let target = st.accounts.env.claude_base.join(USER_MEMORY_PATH);
    let write = || -> std::io::Result<()> {
        if let Some(dir) = target.parent() {
            std::fs::create_dir_all(dir)?;
        }
        hangar_core::fs::atomic_write(&target, put.content.as_bytes())
    };
    match write() {
        Ok(()) => ok(json!({"ok": true})),
        Err(e) => failure(&format!("dream_io: {e}")),
    }
}
