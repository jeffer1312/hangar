// crates/hangar-server/src/subagents.rs
//! `GET /api/sessions/{name}/subagents[/{agent_id}]`: os subagentes da conversa que o Rust escolheu
//! para a sessão (a da lista, ou a que o hub confirmou), nunca "o transcript mais novo do cwd".
//! Portado de backend/app/subagents.py, com o mesmo JSON nos três layouts:
//!   Claude `<transcript sem .jsonl>/subagents/agent-<id>.jsonl` (+ `.meta.json`),
//!   Kimi   `<sessão>/agents/<id>/wire.jsonl` ao lado do `agents/main/wire.jsonl`,
//!   Pi     `<transcript sem .jsonl>/<taskId>/run-<n>/session.jsonl` (só o run mais alto).
//!   Codex  rollouts da mesma conta com `parent_thread_id` igual à conversa principal.
mod codex;
use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::{ConnectInfo, Path as UrlPath, Request, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Response};
use hangar_api::chat::{ChatEvent, ChatKind};
use hangar_api::session::SessionRow;
use regex::Regex;
use serde_json::{Map, Value, json};

use crate::interactions::InteractionBinding;
use crate::list::capped::{Capped, SESSION_CAP};
use crate::plugin::{AgentLife, AgentSend, AgentState};
use crate::routes::{AppState, cors, gate, pass, route_failed};
use crate::side::Binding;
use crate::transcript::{LineParser, Provider, decode_line};

const LIST_TAIL: usize = 12;
const DETAIL_TAIL: usize = 40;
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(10);

// ── rotas ─────────────────────────────────────────────────────────────────────

pub async fn public(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    // O convidado passa pelo porteiro do Python, que devolve o pedido pela ponte privada.
    let Some(target) = owner.then(|| target_of(req.uri().path())).flatten() else { return pass(&st, req, &fwd).await };
    serve(&st, target, req.uri().query(), req.headers()).await
}

/// Ponte das portas do Python (convidado 8766, Connect 8768): pedido já autenticado lá.
pub async fn private(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(target) = req.headers().get("x-hangar-path").and_then(|v| v.to_str().ok()).and_then(target_of) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    serve(&st, target, req.uri().query(), req.headers()).await
}

/// `(sessão, subagente)` de `/api/sessions/{name}/subagents[/{id}]`, já decodificado.
fn target_of(path: &str) -> Option<(String, Option<String>)> {
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8().ok()?;
    let (name, tail) = decoded.strip_prefix("/api/sessions/")?.split_once('/')?;
    if name.is_empty() {
        return None;
    }
    match tail.strip_prefix("subagents") {
        Some("") => Some((name.to_owned(), None)),
        Some(rest) => rest.strip_prefix('/').filter(|id| !id.is_empty() && !id.contains('/')).map(|id| (name.to_owned(), Some(id.to_owned()))),
        None => None,
    }
}

fn reply(status: StatusCode, body: Value, req: &HeaderMap) -> Response {
    let mut resp = (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response();
    cors(req, resp.headers_mut());
    resp
}

fn refusal(status: StatusCode, code: &str, msg: &str, req: &HeaderMap) -> Response {
    reply(status, json!({"detail": {"code": code, "params": {}, "msg": msg}}), req)
}

/// Ponte viva, mas o plugin carregado na abertura da sessão não declara `agent_message`.
const OLD_PLUGIN: &str = "erro_subagente_mensagem_plugin_antigo";
const NO_PLUGIN: &str = "erro_subagente_mensagem_sem_plugin";
/// O vínculo ou a memória do plugin não puderam ser lidos: o estado é desconhecido, não "sem sinal".
const STATE_UNAVAILABLE: &str = "erro_subagente_estado_indisponivel";
const STALE_BINDING: &str = "erro_subagente_mensagem_vinculo_desatualizado";
const CODEX_MESSAGE_UNAVAILABLE: &str = "erro_subagente_mensagem_codex_indisponivel";
const MESSAGE_BODY_MAX: usize = 64 * 1024;

/// O vínculo do plugin desta sessão, só se ele for da mesma conversa listada; `Ok(None)` = vínculo de outra conversa.
/// Erro = o vínculo não pôde ser lido (o código de quem falhou).
async fn plugin_binding(st: &AppState, name: &str, jsonl: &Path) -> Result<Option<InteractionBinding>, String> {
    let registry = st.state.runtime.get().ok_or_else(|| "runtime_unavailable".to_owned())?;
    let binding = registry.interaction_binding_readonly(name).await.map_err(|e| e.code)?;
    Ok((jsonl.file_stem().is_some_and(|stem| stem.to_string_lossy() == binding.conversation)).then_some(binding))
}

/// Um aviso por sessão e código, não um a cada consulta da lista.
fn warn_once(name: &str, code: &str) {
    static SEEN: LazyLock<std::sync::Mutex<std::collections::HashSet<(String, String)>>> = LazyLock::new(Default::default);
    // ponytail: conjunto sem teto; cresce com sessões × códigos distintos, poucos por processo.
    if SEEN.lock().is_ok_and(|mut seen| seen.insert((name.to_owned(), code.to_owned()))) {
        tracing::warn!(session = %name, code, "estado dos subagentes indisponível");
    }
}

/// O que a sessão diz sobre mandar texto a subagentes, antes de olhar o plugin. Claude sem terminal também carrega o
/// plugin (`--plugin-dir` em todo modo) e o `$.session.send` retoma o subagente no `claude -p`.
fn message_blocked(provider: &str) -> Option<&'static str> {
    (provider != "claude").then_some(NO_PLUGIN)
}

/// Por que o plugin desta conversa não entrega a subagentes: sem ponte viva, ou plugin antigo sem o modo.
fn plugin_block(st: &AppState, binding: &InteractionBinding) -> Option<&'static str> {
    if !st.plugin.alive(binding, std::time::Instant::now()).unwrap_or(false) { return Some(NO_PLUGIN); }
    if !st.plugin.can_message_agents(binding, std::time::Instant::now()) { return Some(OLD_PLUGIN); }
    None
}

/// `{"<agentId>": {"state": running|completed|failed|stopped, "end_time": ms|null}}` do runtime sem terminal.
fn headless_lives(map: &Value) -> HashMap<String, AgentLife> {
    map.as_object().into_iter().flatten().filter_map(|(id, v)| {
        let state = match v.get("state").and_then(Value::as_str)? {
            "running" => AgentState::Running,
            "completed" => AgentState::Finished,
            "failed" | "stopped" => AgentState::Failed,
            _ => return None,
        };
        let ended_at = v.get("end_time").and_then(Value::as_i64)
            .and_then(chrono::DateTime::from_timestamp_millis).map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Millis, true));
        Some((id.clone(), AgentLife { state, ended_at }))
    }).collect()
}

/// `state`, `endedAt`, `canMessage` e `messageBlocked` de um item. No Claude com terminal o estado é o
/// sinal do plugin (null sem sinal); Kimi e Pi dizem sozinhos que acabaram (`finished`).
fn decorate(a: &mut Map<String, Value>, life: Option<&AgentLife>, blocked: Option<&str>, problem: Option<&str>) {
    let (state, ended) = match (life, a.get("finished").and_then(Value::as_bool)) {
        (Some(life), _) => (json!(life.state), json!(life.ended_at)),
        (None, Some(true)) => (json!("finished"), a.get("updatedAt").filter(|v| v.as_str().is_some_and(|t| !t.is_empty())).cloned().unwrap_or(Value::Null)),
        (None, Some(false)) => (json!("running"), Value::Null),
        (None, None) => (a.get("state").cloned().unwrap_or(Value::Null), a.get("endedAt").cloned().unwrap_or(Value::Null)),
    };
    a.insert("state".into(), state);
    a.insert("endedAt".into(), ended);
    a.insert("canMessage".into(), json!(blocked.is_none()));
    a.insert("messageBlocked".into(), json!(blocked));
    if let Some(problem) = problem { a.insert("problema".into(), json!(problem)); }
}

async fn serve(st: &AppState, (name, agent): (String, Option<String>), query: Option<&str>, headers: &HeaderMap) -> Response {
    // `events: int = 0` do FastAPI: texto é 422; negativo pede a lista vazia.
    let events = match crate::auth::query_param(query, "events") {
        None => 0,
        Some(v) => match v.parse::<i64>() {
            Ok(n) => n,
            Err(_) => return reply(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"loc": ["query", "events"], "msg": "events must be an integer"}]}), headers),
        },
    };
    let row = match tokio::time::timeout(LOOKUP_TIMEOUT, row_of(st, &name)).await.unwrap_or(Err("locate_timeout")) {
        Ok(Some(row)) => row,
        Ok(None) => return refusal(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "session or transcript not found", headers),
        Err(code) => return route_failed(st, headers, "rust.subagents_failed", &name, code, crate::routes::INFO_REASON),
    };
    let Some((provider, jsonl)) = conversation(&row, st.side.hubs.bound(&name)) else {
        return refusal(StatusCode::CONFLICT, "erro_subagentes_conversa_desconhecida",
            "the session's conversation is not known yet; subagents of another conversation are not shown", headers);
    };
    let mut blocked = message_blocked(&provider);
    let mut lives = HashMap::new();
    let mut codex_status = Value::Null;
    let mut codex_detail = None;
    let mut problem = None;
    if blocked.is_none() {
        match plugin_binding(st, &name, &jsonl).await {
            Ok(Some(b)) => match st.plugin.agent_lives(&b) {
                Ok(l) => {
                    lives = l;
                    blocked = plugin_block(st, &b);
                }
                // A memória respondeu: o plugin não registrou esta conversa. É ausência lida, não falha.
                Err(e) if e.0 == "erro_interacao_desatualizada" => blocked = Some(NO_PLUGIN),
                Err(e) => problem = Some(e.0.to_owned()),
            },
            Ok(None) => blocked = Some(NO_PLUGIN),
            Err(code) => problem = Some(code),
        }
        if let Some(code) = &problem {
            warn_once(&name, code);
            blocked = Some(STATE_UNAVAILABLE);
        }
    }
    // Claude sem terminal: o stream-json do runtime diz o ciclo de vida e vence o sinal do plugin.
    if provider == "claude" && let Some(runtime) = st.state.runtime.get() {
        match runtime.claude_subagents(&name).await {
            Some(Ok(map)) => lives.extend(headless_lives(&map)),
            Some(Err(e)) => { warn_once(&name, "runtime_subagents_unavailable"); tracing::debug!(session = %name, ?e, "subagentes do runtime"); problem = Some("runtime_subagents_unavailable".into()); }
            None => {}
        }
    }
    if provider == "codex" {
        blocked = Some(CODEX_MESSAGE_UNAVAILABLE);
        let Some(parent) = codex::parent_thread_id(&jsonl) else {
            return route_failed(st, headers, "rust.subagents_failed", &name, "codex_subagents_source_unavailable", "a conversa Codex não pôde ser lida");
        };
        if let Some(id) = &agent {
            let (path, id) = (jsonl.clone(), id.clone());
            codex_detail = match tokio::task::spawn_blocking(move || codex::get_checked(&path, &id, events)).await {
                Ok(Ok(found)) => found,
                Ok(Err(code)) => return route_failed(st, headers, "rust.subagents_failed", &name, code, "a família do subagente Codex não pôde ser lida"),
                Err(error) => {
                    tracing::warn!(session = %name, panic = error.is_panic(), "leitura do subagente Codex caiu");
                    return route_failed(st, headers, "rust.subagents_failed", &name, "subagents_panic", "a leitura dos subagentes caiu no servidor");
                }
            };
        }
        if let Some(runtime) = st.state.runtime.get() {
            match runtime.codex_subagents(&name, &parent).await {
                Some(Ok(map)) => { lives.extend(headless_lives(&map)); codex_status = map; }
                Some(Err(error)) => {
                    warn_once(&name, "runtime_subagents_unavailable");
                    tracing::debug!(session = %name, ?error, "subagentes Codex do runtime");
                    problem = Some("runtime_subagents_unavailable".into());
                }
                None => {}
            }
            if let Some(id) = &agent {
                // Só consulta o filho depois de conferir a família no histórico desta conta.
                if codex_detail.is_some() {
                    match runtime.codex_subagent_status(&name, &parent, id).await {
                        Some(Ok(status)) => {
                            let map = Value::Object(Map::from_iter([(id.clone(), status)]));
                            lives.extend(headless_lives(&map));
                            if !codex_status.is_object() { codex_status = json!({}); }
                            codex_status[id] = map[id].clone();
                        }
                        Some(Err(error)) => { tracing::debug!(session = %name, ?error, "leitura do subagente Codex"); }
                        None => {}
                    }
                }
            }
        }
    }
    let problem = problem.map(|_| STATE_UNAVAILABLE);
    let done = tokio::task::spawn_blocking(move || {
        let mut fill = |mut a: Map<String, Value>| {
            let life = a.get("agentId").and_then(Value::as_str).and_then(|id| lives.get(id));
            decorate(&mut a, life, blocked, problem);
            if provider == "codex" {
                let status = a.get("agentId").and_then(Value::as_str).map(|id| &codex_status[id]);
                let can_message = status.is_some_and(|status| status["can_message"] == true);
                a.insert("canMessage".into(), json!(can_message));
                a.insert("messageBlocked".into(), if can_message { Value::Null } else {
                    status.and_then(|status| status.get("message_blocked")).filter(|v| v.is_string()).cloned().unwrap_or_else(|| json!(CODEX_MESSAGE_UNAVAILABLE))
                });
            }
            a
        };
        match agent {
            None => {
                let rows = if provider == "codex" { codex::list_checked(&jsonl).map_err(Some)? } else { list_subagents(&provider, &jsonl) };
                Ok(json!(rows.into_iter().map(&mut fill).collect::<Vec<_>>()))
            }
            Some(id) => {
                let found = if provider == "codex" { codex_detail } else { get_subagent(&provider, &jsonl, &id, events) };
                found.map(|a| Value::Object(fill(a))).ok_or(None)
            }
        }
    })
    .await;
    match done {
        Ok(Ok(v)) => reply(StatusCode::OK, v, headers),
        Ok(Err(None)) => refusal(StatusCode::NOT_FOUND, "erro_subagente_inexistente", "subagent not found", headers),
        Ok(Err(Some(code))) => route_failed(st, headers, "rust.subagents_failed", &name, code, "os arquivos dos subagentes Codex não puderam ser lidos"),
        Err(e) => {
            tracing::warn!(session = %name, panic = e.is_panic(), "subagentes no Rust caiu");
            route_failed(st, headers, "rust.subagents_failed", &name, "subagents_panic", "a leitura dos subagentes caiu no servidor")
        }
    }
}

/// `POST /api/sessions/{name}/subagents/{agent_id}/message` `{"text"}`: o plugin do Claude roda o
/// `SendMessage` direto (fila do agente vivo, ou retomada do parado), sem passar pelo modelo pai.
pub async fn message(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
    UrlPath((name, agent_id)): UrlPath<(String, String)>, req: Request) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    if !owner { return pass(&st, req, &fwd).await; }
    let headers = req.headers().clone();
    let invalid = || refusal(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "body must be {\"text\": non-empty string without control characters}", &headers);
    let Ok(bytes) = axum::body::to_bytes(req.into_body(), MESSAGE_BODY_MAX).await else { return invalid() };
    let Some(text) = serde_json::from_slice::<Value>(&bytes).ok().and_then(|v| v.get("text").and_then(Value::as_str).map(str::to_owned))
        .filter(|t| valid_text(t)) else { return invalid() };
    let row = match tokio::time::timeout(LOOKUP_TIMEOUT, row_of(&st, &name)).await.unwrap_or(Err("locate_timeout")) {
        Ok(Some(row)) => row,
        Ok(None) => return refusal(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "session or transcript not found", &headers),
        Err(code) => return route_failed(&st, &headers, "rust.subagents_failed", &name, code, crate::routes::INFO_REASON),
    };
    if row.provider != "codex" && let Some(code) = message_blocked(&row.provider) {
        return refusal(StatusCode::CONFLICT, code, "this session cannot message its subagents", &headers);
    }
    let Some((provider, jsonl)) = conversation(&row, st.side.hubs.bound(&name)) else {
        return refusal(StatusCode::CONFLICT, "erro_subagentes_conversa_desconhecida", "the session's conversation is not known yet", &headers);
    };
    let exists = { let (provider, jsonl, id) = (provider.clone(), jsonl.clone(), agent_id.clone());
        tokio::task::spawn_blocking(move || if provider == "codex" { codex::get_checked(&jsonl, &id, 0).map(|v| v.is_some()) }
            else { Ok(get_subagent(&provider, &jsonl, &id, 0).is_some()) }).await };
    match exists {
        Ok(Ok(true)) => {}
        Ok(Ok(false)) => return refusal(StatusCode::CONFLICT, "erro_subagente_inexistente", "subagent not found in this conversation", &headers),
        Ok(Err(code)) => return route_failed(&st, &headers, "rust.subagents_failed", &name, code, "a família do subagente Codex não pôde ser lida"),
        Err(e) => {
            tracing::warn!(session = %name, panic = e.is_panic(), "leitura do subagente caiu antes da mensagem");
            return route_failed(&st, &headers, "rust.subagents_failed", &name, "subagents_panic", "a leitura dos subagentes caiu no servidor");
        }
    }
    if provider == "codex" {
        let Some(parent) = codex::parent_thread_id(&jsonl) else {
            return refusal(StatusCode::CONFLICT, STALE_BINDING, "a conversa Codex não pôde ser conferida", &headers);
        };
        let Some(runtime) = st.state.runtime.get() else {
            return refusal(StatusCode::SERVICE_UNAVAILABLE, CODEX_MESSAGE_UNAVAILABLE, "o runtime Codex não está disponível", &headers);
        };
        return match runtime.codex_subagent_message(&name, &parent, &agent_id, &text).await {
            Some(Ok(body)) => reply(StatusCode::OK, body, &headers),
            Some(Err(error)) => {
                let status = if error.code == "erro_subagente_mensagem_sem_resposta" { StatusCode::GATEWAY_TIMEOUT }
                    else if error.code.starts_with("erro_subagente") { StatusCode::CONFLICT } else { StatusCode::SERVICE_UNAVAILABLE };
                refusal(status, &error.code, &error.message, &headers)
            }
            None => refusal(StatusCode::SERVICE_UNAVAILABLE, CODEX_MESSAGE_UNAVAILABLE, "o runtime Codex não está disponível", &headers),
        };
    }
    let binding = match plugin_binding(&st, &name, &jsonl).await {
        Ok(Some(binding)) => binding,
        Ok(None) => return refusal(StatusCode::CONFLICT, NO_PLUGIN, "no live plugin bridge for this session", &headers),
        Err(code) => {
            warn_once(&name, &code);
            return route_failed(&st, &headers, "rust.subagents_failed", &name, STATE_UNAVAILABLE, "o vínculo do plugin desta sessão não pôde ser lido");
        }
    };
    if plugin_block(&st, &binding) == Some(OLD_PLUGIN) {
        return refusal(StatusCode::CONFLICT, OLD_PLUGIN, "the session's plugin predates agent messages; reopen the session", &headers);
    }
    match st.plugin.send_agent(&binding, &agent_id, &text).await {
        Ok(sent) => sent_reply(&sent, &headers),
        // A conversa do plugin trocou entre o vínculo e o envio.
        Err(error) if error.0 == "erro_interacao_desatualizada" => refusal(StatusCode::CONFLICT, STALE_BINDING, error.0, &headers),
        Err(error) => {
            warn_once(&name, error.0);
            route_failed(&st, &headers, "rust.subagents_failed", &name, STATE_UNAVAILABLE, "a memória do plugin não pôde ser lida")
        }
    }
}

/// Texto não vazio; quebra de linha e tab passam, o resto dos controles não.
fn valid_text(text: &str) -> bool {
    !text.trim().is_empty() && !text.chars().any(|c| c.is_control() && c != '\n' && c != '\t')
}

fn sent_reply(sent: &AgentSend, headers: &HeaderMap) -> Response {
    match sent {
        AgentSend::Delivered => reply(StatusCode::OK, json!({"delivered": true}), headers),
        AgentSend::Refused(reason) => reply(StatusCode::CONFLICT, json!({"detail": {"code": "erro_subagente_mensagem_recusada",
            "params": {"reason": reason}, "msg": "Claude Code refused the message", "reason": reason}}), headers),
        AgentSend::NoPlugin => refusal(StatusCode::CONFLICT, NO_PLUGIN, "no live plugin bridge for this session", headers),
        AgentSend::NoAnswer => refusal(StatusCode::GATEWAY_TIMEOUT, "erro_subagente_mensagem_sem_resposta", "the plugin did not confirm the delivery", headers),
    }
}

/// A linha da sessão na lista do Rust (a mesma busca do `locate::ListLocate`).
async fn row_of(st: &AppState, name: &str) -> Result<Option<SessionRow>, &'static str> {
    let p = st.list.snapshot().await.map_err(|e| e.code)?;
    if let Some(row) = p.rows.iter().find(|r| r.name == name) {
        return Ok(Some(row.clone()));
    }
    let p = st.list.refresh().await.map_err(|e| e.code)?;
    match p.rows.iter().find(|r| r.name == name) {
        Some(row) => Ok(Some(row.clone())),
        None if p.facts.unknown => Err("list_facts_unknown"),
        None => Ok(None),
    }
}

/// A conversa da sessão: só a que a descoberta marcou como dela (`tracked`); o chute "mais novo do
/// cwd" pode ser de outra conversa e listaria os subagentes dela. Com o chat ao vivo aberto vale o
/// transcript que o hub confirmou, como no `/history`.
pub(crate) fn conversation(row: &SessionRow, bound: Option<Binding>) -> Option<(String, PathBuf)> {
    let jsonl = row.jsonl.as_deref().filter(|j| row.tracked && !j.is_empty())?;
    let fresh = Binding::from_info(&crate::locate::info_of(row, Path::new("")));
    let jsonl = match bound.filter(|b| fresh.as_ref().is_some_and(|f| !b.changes_at_once(f))) {
        Some(b) => b.jsonl,
        None => PathBuf::from(jsonl),
    };
    Some((row.provider.clone(), jsonl))
}

// ── leitura ───────────────────────────────────────────────────────────────────

/// Todos os subagentes da conversa, do mais recém-escrito pro mais antigo.
pub fn list_subagents(provider: &str, jsonl: &Path) -> Vec<Map<String, Value>> {
    if provider == "codex" { return codex::list(jsonl); }
    let mut out = Vec::new();
    if let Some(root) = pi_agents_dir(provider, jsonl) {
        for task in sorted_dirs(&root) {
            let Some(run) = pi_run_dir(&task) else { continue };
            let id = file_name(&task);
            out.push(read_agent_pi(&task, &run, LIST_TAIL).unwrap_or_else(|| unreadable(&id, mtime(&run.join("session.jsonl")))));
        }
    } else if let Some(root) = kimi_agents_dir(jsonl) {
        for d in sorted_dirs(&root) {
            let id = file_name(&d);
            if id == "main" {
                continue;
            }
            out.push(read_agent_kimi(&d, LIST_TAIL).unwrap_or_else(|| unreadable(&id, mtime(&d.join("wire.jsonl")))));
        }
    } else {
        let mut files: Vec<PathBuf> = std::fs::read_dir(jsonl.with_extension("").join("subagents")).into_iter().flatten().flatten()
            .map(|e| e.path())
            .filter(|p| { let n = file_name(p); n.starts_with("agent-") && n.ends_with(".jsonl") })
            .collect();
        files.sort();
        for f in files {
            let id = claude_id(&f);
            out.push(read_agent_claude(&f, LIST_TAIL).unwrap_or_else(|| unreadable(&id, mtime(&f))));
        }
    }
    out.sort_by(|a, b| num(b, "mtime").total_cmp(&num(a, "mtime")));
    out
}

/// Um subagente com a cauda maior e, com `events != 0`, o transcript dele nos `ChatEvent` do chat.
/// None = não existe (404); arquivo existente e ilegível volta marcado `ilegivel`.
pub fn get_subagent(provider: &str, jsonl: &Path, agent_id: &str, events: i64) -> Option<Map<String, Value>> {
    if provider == "codex" { return codex::get(jsonl, agent_id, events); }
    let limit = usize::try_from(events.max(0)).unwrap_or(0);
    let (mut a, file, kind) = if let Some(root) = pi_agents_dir(provider, jsonl) {
        let task = child(&root, agent_id)?;
        let run = pi_run_dir(&task)?;
        let f = run.join("session.jsonl");
        (read_agent_pi(&task, &run, DETAIL_TAIL).unwrap_or_else(|| unreadable(agent_id, mtime(&f))), f, Layout::Pi)
    } else if let Some(root) = kimi_agents_dir(jsonl) {
        if agent_id == "main" {
            return None;
        }
        let d = child(&root, agent_id)?;
        let f = d.join("wire.jsonl");
        if !f.is_file() {
            return None;
        }
        (read_agent_kimi(&d, DETAIL_TAIL).unwrap_or_else(|| unreadable(agent_id, mtime(&f))), f, Layout::Kimi)
    } else {
        if agent_id.contains(['/', '\\']) || agent_id.contains(':') {
            return None;
        }
        let f = jsonl.with_extension("").join("subagents").join(format!("agent-{agent_id}.jsonl"));
        if !f.is_file() {
            return None;
        }
        (read_agent_claude(&f, DETAIL_TAIL).unwrap_or_else(|| unreadable(agent_id, mtime(&f))), f, Layout::Claude)
    };
    if a.contains_key("ilegivel") || events == 0 {
        return Some(a);
    }
    // Falha de leitura deixa a chave ausente: lista vazia diria "nenhuma ferramenta chamada".
    if let Some(ev) = tail_events(&file, limit, kind) {
        a.insert("events".into(), serde_json::to_value(ev).unwrap_or(Value::Array(Vec::new())));
    }
    Some(a)
}

#[derive(Clone, Copy)]
enum Layout {
    Claude,
    Kimi,
    Pi,
}

/// Pasta filha de nome `id`, conferida pelo caminho resolvido: `..`, caminho absoluto, `D:x` e
/// symlink para fora da pasta não abrem nada.
fn child(base: &Path, id: &str) -> Option<PathBuf> {
    let d = base.join(id);
    let real = std::fs::canonicalize(&d).ok()?;
    (real.parent()? == std::fs::canonicalize(base).ok()?).then_some(d)
}

fn sorted_dirs(root: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    v.sort();
    v
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn claude_id(f: &Path) -> String {
    let stem = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    stem.strip_prefix("agent-").map(str::to_owned).unwrap_or(stem)
}

fn mtime(f: &Path) -> f64 {
    f.metadata().and_then(|m| m.modified()).ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0.0, |d| d.as_secs_f64())
}

fn num(a: &Map<String, Value>, k: &str) -> f64 {
    a.get(k).and_then(Value::as_f64).unwrap_or(0.0)
}

/// Linhas JSON-objeto do arquivo, uma por vez: o transcript inteiro em `Value` custava várias
/// vezes o tamanho dele. None se não abriu; item None = falha de leitura no meio (já registrada).
fn objects(f: &Path) -> Option<impl Iterator<Item = Option<Map<String, Value>>>> {
    let file = File::open(f).map_err(|e| tracing::warn!("subagents: não consegui ler {}: {e}", f.display())).ok()?;
    let shown = f.display().to_string();
    Some(BufReader::new(file).split(b'\n').filter_map(move |line| match line {
        Err(e) => {
            tracing::warn!("subagents: não consegui ler {shown}: {e}");
            Some(None)
        }
        Ok(line) => match decode_line(&line) {
            Some(Value::Object(o)) => Some(Some(o)),
            _ => None,
        },
    }))
}

fn s<'a>(o: &'a Map<String, Value>, k: &str) -> Option<&'a str> {
    o.get(k).and_then(Value::as_str)
}

fn chars(text: &str, n: usize) -> String {
    text.chars().take(n).collect()
}

/// Texto de um `content` (string ou lista de blocos `text`).
fn text_of(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(blocks)) => blocks.iter().filter_map(Value::as_object)
            .filter(|b| s(b, "type") == Some("text"))
            .map(|b| s(b, "text").unwrap_or("")).collect::<Vec<_>>().join(" ").trim().to_owned(),
        _ => String::new(),
    }
}

/// Uma linha curta do que a tool está tocando: caminho, comando ou padrão.
fn tool_target(name: &str, input: Option<&Value>) -> String {
    let Some(inp) = input.and_then(Value::as_object) else { return String::new() };
    for k in ["file_path", "path", "notebook_path"] {
        if let Some(v) = s(inp, k) {
            return v.to_owned();
        }
    }
    // Case-fold: o Pi chama a tool de `bash`.
    if name.eq_ignore_ascii_case("bash") {
        return s(inp, "command").unwrap_or("").to_owned();
    }
    if matches!(name, "exec_command" | "functions.exec_command") {
        return s(inp, "cmd").unwrap_or("").to_owned();
    }
    ["pattern", "query", "url", "prompt", "description"].iter().find_map(|k| s(inp, k)).unwrap_or("").to_owned()
}

/// O que as três leituras acumulam, no mesmo dicionário de saída.
#[derive(Clone, Default)]
struct Acc {
    prompt: Option<String>,
    agent_type: Option<String>,
    tools: Vec<(String, usize)>,
    calls: Vec<Value>,
    last_text: String,
    started: String,
    updated: String,
}

impl Acc {
    fn when(&mut self, ts: String) {
        if self.started.is_empty() {
            self.started = ts.clone();
        }
        self.updated = ts;
    }

    fn call(&mut self, name: &str, input: Option<&Value>) {
        match self.tools.iter_mut().find(|(n, _)| n == name) {
            Some((_, c)) => *c += 1,
            None => self.tools.push((name.to_owned(), 1)),
        }
        self.calls.push(json!({"name": name, "target": chars(&tool_target(name, input), 200)}));
    }

    fn into_map(mut self, id: &str, mtime: f64, tail: usize) -> Map<String, Value> {
        // Estável: empate fica na ordem em que a tool apareceu, como o `sorted` do Python.
        self.tools.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        let total: usize = self.tools.iter().map(|(_, c)| c).sum();
        let recent = self.calls.split_off(self.calls.len().saturating_sub(tail));
        let Value::Object(m) = json!({
            "agentId": id, "agentType": self.agent_type, "prompt": self.prompt,
            "startedAt": self.started, "updatedAt": self.updated, "mtime": mtime,
            "toolCalls": total,
            "tools": self.tools.iter().map(|(n, c)| json!({"name": n, "count": c})).collect::<Vec<_>>(),
            "recent": recent, "lastText": chars(&self.last_text, 2000),
        }) else { unreachable!() };
        m
    }
}

/// Subagente que existe mas não deu pra ler agora: aparece marcado em vez de sumir da lista.
fn unreadable(id: &str, mtime: f64) -> Map<String, Value> {
    let Value::Object(m) = json!({"agentId": id, "agentType": null, "prompt": null, "startedAt": "", "updatedAt": "",
        "mtime": mtime, "toolCalls": 0, "tools": [], "recent": [], "lastText": "", "ilegivel": true}) else { unreachable!() };
    m
}

// ── Claude ────────────────────────────────────────────────────────────────────

const FORK_DIRECTIVE: &str = "\nYour directive: ";
static TEAMMATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^\s*<teammate-message\b[^>]*>\n?(.*?)\n?</teammate-message>\s*$").unwrap());

type Stamp = (Option<std::time::SystemTime>, u64, Option<std::time::SystemTime>);

/// Resumo por (arquivo, cauda); a lista é pedida a cada 5 s enquanto a sessão trabalha e só o
/// arquivo que cresceu precisa ser relido.
static CLAUDE_AGENTS: LazyLock<Mutex<Capped<(PathBuf, usize), (Stamp, Map<String, Value>)>>> =
    LazyLock::new(|| Mutex::new(Capped::new(SESSION_CAP)));

fn read_agent_claude(f: &Path, tail: usize) -> Option<Map<String, Value>> {
    let modified = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
    let stamp: Stamp = (modified(f), f.metadata().map_or(0, |m| m.len()), modified(&f.with_extension("meta.json")));
    let key = (f.to_path_buf(), tail);
    if let Some((seen, agent)) = CLAUDE_AGENTS.lock().unwrap_or_else(PoisonError::into_inner).get(&key)
        && *seen == stamp
    {
        return Some(agent.clone());
    }
    let agent = parse_agent_claude(f, tail)?;
    CLAUDE_AGENTS.lock().unwrap_or_else(PoisonError::into_inner).insert(key, (stamp, agent.clone()));
    Some(agent)
}

fn parse_agent_claude(f: &Path, tail: usize) -> Option<Map<String, Value>> {
    let mut acc = Acc::default();
    let mut failed = false;
    let mut model = None;
    for r in objects(f)? {
        let r = r?;
        if let Some(ts) = s(&r, "timestamp") {
            acc.when(ts.to_owned());
        }
        let content = r.get("message").and_then(Value::as_object).and_then(|m| m.get("content"));
        let kind = s(&r, "type");
        if kind == Some("assistant") {
            // Erro de API gravado como última resposta (529, 429); resposta normal depois desfaz.
            failed = r.get("isApiErrorMessage") == Some(&Value::Bool(true));
            // Erro de API vem com modelo `<synthetic>`: não é quem respondeu.
            if let Some(m) = r.get("message").and_then(Value::as_object).and_then(|m| s(m, "model")).filter(|m| !m.is_empty() && *m != "<synthetic>") {
                model = Some(m.to_owned());
            }
        }
        if kind == Some("user") && acc.prompt.is_none() {
            let mut t = text_of(content);
            if !t.is_empty() {
                if t.trim_start().starts_with("<fork-boilerplate>") && t.contains(FORK_DIRECTIVE) {
                    t = t.split_once(FORK_DIRECTIVE).map(|(_, d)| d.trim().to_owned()).unwrap_or(t);
                } else if let Some(m) = TEAMMATE.captures(&t).and_then(|c| c.get(1)) {
                    t = m.as_str().trim().to_owned();
                }
                acc.prompt = Some(t);
            }
        } else if let (Some("assistant"), Some(Value::Array(blocks))) = (kind, content) {
            for b in blocks.iter().filter_map(Value::as_object) {
                match s(b, "type") {
                    Some("tool_use") => acc.call(s(b, "name").filter(|n| !n.is_empty()).unwrap_or("?"), b.get("input")),
                    Some("text") => {
                        let t = s(b, "text").unwrap_or("").trim();
                        if !t.is_empty() {
                            acc.last_text = t.to_owned();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let meta = std::fs::read(f.with_extension("meta.json")).ok().and_then(|b| decode_line(&b));
    acc.agent_type = meta.as_ref().and_then(|m| m.get("agentType")).and_then(Value::as_str).map(str::to_owned);
    let mut m = acc.into_map(&claude_id(f), mtime(f), tail);
    // Só a falha: quem diz que terminou é o tool_result no transcript do pai.
    m.insert("failed".into(), Value::Bool(failed));
    if let Some(model) = model { m.insert("model".into(), Value::String(model)); }
    Some(m)
}

// ── Kimi ──────────────────────────────────────────────────────────────────────

/// A pasta `agents/` quando o transcript é o `agents/main/wire.jsonl` do Kimi.
fn kimi_agents_dir(jsonl: &Path) -> Option<PathBuf> {
    let main = jsonl.parent()?;
    let agents = main.parent()?;
    (file_name(jsonl) == "wire.jsonl" && file_name(main) == "main" && file_name(agents) == "agents").then(|| agents.to_path_buf())
}

/// Epoch em milissegundos -> ISO, como o `datetime.isoformat()` do Python.
fn ms_iso(ms: Option<&Value>) -> String {
    let Some(ms) = ms.filter(|v| v.is_number()).and_then(Value::as_f64).filter(|m| *m > 0.0) else { return String::new() };
    let Some(dt) = chrono::DateTime::from_timestamp_micros((ms * 1000.0).round() as i64) else { return String::new() };
    if dt.timestamp_subsec_micros() == 0 { dt.format("%Y-%m-%dT%H:%M:%S+00:00").to_string() } else { dt.format("%Y-%m-%dT%H:%M:%S%.6f+00:00").to_string() }
}

fn truthy(v: Option<&Value>) -> Option<&Value> {
    v.filter(|v| !matches!(v, Value::Null | Value::Bool(false)) && v.as_f64() != Some(0.0) && v.as_str() != Some(""))
}

fn read_agent_kimi(d: &Path, tail: usize) -> Option<Map<String, Value>> {
    let f = d.join("wire.jsonl");
    let mut acc = Acc::default();
    let mut ended = false;
    for r in objects(&f)? {
        let r = r?;
        let when = ms_iso(truthy(r.get("time")).or(r.get("created_at")));
        if !when.is_empty() {
            acc.when(when);
        }
        match s(&r, "type") {
            Some("profile.bind") => {
                if let Some(v) = s(&r, "profileName") {
                    acc.agent_type = Some(v.to_owned());
                }
            }
            Some("turn.prompt") if acc.prompt.is_none() => {
                let parts: Vec<&str> = r.get("input").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object)
                    .filter(|b| s(b, "type") == Some("text")).map(|b| s(b, "text").unwrap_or("")).collect();
                // O bloco <git-context> é igual em todos os subagentes: viraria o título de todos.
                let joined = parts.join(" ");
                let text = joined.find("</git-context>").map_or(joined.as_str(), |i| &joined[i + "</git-context>".len()..]).trim();
                if !text.is_empty() {
                    acc.prompt = Some(text.to_owned());
                }
            }
            Some("turn.ended") => ended = true,
            _ => {}
        }
        let Some(ev) = r.get("event").and_then(Value::as_object) else { continue };
        match s(ev, "type") {
            Some("tool.call") => acc.call(s(ev, "name").filter(|n| !n.is_empty()).unwrap_or("?"), ev.get("args")),
            Some("content.part") => {
                if let Some(part) = ev.get("part").and_then(Value::as_object).filter(|p| s(p, "type") == Some("text")) {
                    let t = s(part, "text").unwrap_or("").trim();
                    if !t.is_empty() {
                        acc.last_text = t.to_owned();
                    }
                }
            }
            _ => {}
        }
    }
    let mut m = acc.into_map(&file_name(d), mtime(&f), tail);
    // O Kimi diz sozinho que o subagente acabou (`turn.ended` no próprio wire).
    m.insert("finished".into(), Value::Bool(ended));
    Some(m)
}

/// `adapters/kimi/transcript.parse_obj`: o wire nos `ChatEvent` do chat.
fn kimi_events(obj: &Map<String, Value>) -> Vec<ChatEvent> {
    let ts = obj.get("time").filter(|v| v.is_number()).and_then(Value::as_f64).map(|t| t / 1000.0);
    let ev_of = |kind, id: &str| ChatEvent { ts, ..crate::transcript::event(kind, id.to_owned()) };
    if s(obj, "type") == Some("context.append_message") {
        let Some(msg) = obj.get("message").and_then(Value::as_object) else { return Vec::new() };
        let from_user = msg.get("origin").and_then(Value::as_object).is_some_and(|o| s(o, "kind") == Some("user"));
        if s(msg, "role") != Some("user") || !from_user {
            return Vec::new();
        }
        let base = s(msg, "id").unwrap_or("");
        return msg.get("content").and_then(Value::as_array).into_iter().flatten().enumerate()
            .filter_map(|(k, b)| {
                let b = b.as_object().filter(|b| s(b, "type") == Some("text"))?;
                let t = s(b, "text").filter(|t| !t.trim().is_empty())?;
                let id = if k == 0 { base.to_owned() } else { format!("{base}:{k}") };
                Some(ChatEvent { text: Some(t.to_owned()), ..ev_of(ChatKind::UserMsg, &id) })
            })
            .collect();
    }
    if s(obj, "type") != Some("context.append_loop_event") {
        return Vec::new();
    }
    let Some(ev) = obj.get("event").and_then(Value::as_object) else { return Vec::new() };
    let eid = s(ev, "uuid").unwrap_or("");
    match s(ev, "type") {
        Some("content.part") => {
            let Some(part) = ev.get("part").and_then(Value::as_object) else { return Vec::new() };
            if s(part, "type") == Some("text") && s(part, "text").is_some_and(|t| !t.trim().is_empty()) {
                return vec![ChatEvent { text: s(part, "text").map(str::to_owned), ..ev_of(ChatKind::AssistantMsg, eid) }];
            }
            match s(part, "think") {
                Some(t) if s(part, "type") == Some("think") && !t.trim().is_empty() => vec![ChatEvent { text: Some(t.to_owned()), ..ev_of(ChatKind::Thinking, eid) }],
                _ => Vec::new(),
            }
        }
        Some("tool.call") => vec![ChatEvent {
            tool_name: s(ev, "name").map(str::to_owned),
            tool_input: Some(ev.get("args").and_then(Value::as_object).cloned().unwrap_or_default()),
            tool_use_id: s(ev, "toolCallId").map(str::to_owned),
            ..ev_of(ChatKind::ToolUse, eid)
        }],
        Some("tool.result") => {
            let result = ev.get("result");
            let is_error = result.and_then(Value::as_object).and_then(|r| r.get("isError")).is_some_and(|v| truthy(Some(v)).is_some());
            let text = match result {
                Some(Value::Object(r)) => match r.get("output") {
                    Some(Value::String(o)) => o.clone(),
                    _ => crate::transcript::pyjson::dumps_unicode(result.unwrap(), false),
                },
                Some(Value::String(t)) => t.clone(),
                _ => String::new(),
            };
            // O tool.result não tem `uuid`: o id sai do toolCallId, com prefixo para não colidir.
            let cid = [s(ev, "toolCallId"), s(ev, "parentUuid"), Some(eid)].into_iter().flatten().find(|c| !c.is_empty()).unwrap_or("");
            let id = if cid.is_empty() { String::new() } else { format!("res:{cid}") };
            vec![ChatEvent { tool_use_id: s(ev, "toolCallId").map(str::to_owned), result: Some(text), is_error: Some(is_error),
                ..ev_of(ChatKind::ToolResult, &id) }]
        }
        _ => Vec::new(),
    }
}

// ── Pi ────────────────────────────────────────────────────────────────────────

static RUN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^run-(\d+)$").unwrap());
static SUB_NAME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^subagent-(.+?)-[0-9a-f]{8}-[0-9a-f-]+-\d+$").unwrap());
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:?]*[ -/]*[@-~]").unwrap());
/// `stopReason` que encerra o run; `toolUse` é "vou chamar mais uma ferramenta".
const PI_ENDED: [&str; 3] = ["stop", "error", "length"];

/// A pasta dos subagentes de um transcript do Pi: `<transcript sem .jsonl>/`.
fn pi_agents_dir(provider: &str, jsonl: &Path) -> Option<PathBuf> {
    let d = jsonl.with_extension("");
    (provider == "pi" && jsonl.extension().is_some_and(|e| e == "jsonl") && d.is_dir()).then_some(d)
}

/// O `run-<n>` mais alto com `session.jsonl`: os outros são retentativas abandonadas.
fn pi_run_dir(task: &Path) -> Option<PathBuf> {
    std::fs::read_dir(task).ok()?.flatten()
        .filter_map(|e| {
            let n: u64 = RUN.captures(&e.file_name().to_string_lossy())?.get(1)?.as_str().parse().ok()?;
            e.path().join("session.jsonl").is_file().then(|| (n, e.path()))
        })
        .max_by_key(|(n, _)| *n)
        .map(|(_, p)| p)
}

fn read_agent_pi(task: &Path, run: &Path, tail: usize) -> Option<Map<String, Value>> {
    let f = run.join("session.jsonl");
    let mut acc = Acc::default();
    let mut last: (Option<String>, Option<String>) = (None, None);
    for r in objects(&f)? {
        let r = r?;
        if let Some(ts) = s(&r, "timestamp") {
            acc.when(ts.to_owned());
        }
        if s(&r, "type") == Some("session_info") && acc.agent_type.is_none() {
            acc.agent_type = SUB_NAME.captures(s(&r, "name").unwrap_or("")).and_then(|c| c.get(1)).map(|m| m.as_str().to_owned());
            continue;
        }
        if s(&r, "type") != Some("message") {
            continue;
        }
        let Some(msg) = r.get("message").and_then(Value::as_object) else { continue };
        let Some(blocks) = msg.get("content").and_then(Value::as_array) else { continue };
        let role = s(msg, "role");
        last = (role.map(str::to_owned), s(msg, "stopReason").map(str::to_owned));
        if role == Some("user") && acc.prompt.is_none() {
            let t = text_of(msg.get("content"));
            if !t.is_empty() {
                acc.prompt = Some(t);
            }
        } else if role == Some("assistant") {
            for b in blocks.iter().filter_map(Value::as_object) {
                match s(b, "type") {
                    Some("toolCall") => acc.call(s(b, "name").filter(|n| !n.is_empty()).unwrap_or("?"), b.get("arguments")),
                    Some("text") => {
                        let t = s(b, "text").unwrap_or("").trim();
                        if !t.is_empty() {
                            acc.last_text = t.to_owned();
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    let finished = last.0.as_deref() == Some("assistant") && last.1.as_deref().is_some_and(|r| PI_ENDED.contains(&r));
    let mut m = acc.into_map(&file_name(task), mtime(&f), tail);
    // O pai do Pi nunca cita o taskId: sem isto todo subagente ficaria "rodando" pra sempre.
    m.insert("finished".into(), Value::Bool(finished));
    Some(m)
}

/// `adapters/pi/transcript.parse_obj`: uma linha do Pi nos `ChatEvent` do chat.
fn pi_events(obj: &Map<String, Value>) -> Vec<ChatEvent> {
    let clean = |t: &str| {
        let mut out = ANSI.replace_all(t, "").into_owned();
        crate::transcript::py::scrub_str(&mut out);
        out
    };
    if s(obj, "type") != Some("message") {
        return Vec::new();
    }
    let Some(msg) = obj.get("message").and_then(Value::as_object) else { return Vec::new() };
    let Some(content) = msg.get("content").and_then(Value::as_array) else { return Vec::new() };
    let node = s(obj, "id").unwrap_or("");
    let ts = msg.get("timestamp").filter(|v| v.is_number()).and_then(Value::as_f64).map(|t| t / 1000.0);
    let ev_of = |kind, k: usize| ChatEvent { ts, ..crate::transcript::event(kind, if k == 0 { node.to_owned() } else { format!("{node}:{k}") }) };
    let blocks = content.iter().enumerate().filter_map(|(k, b)| b.as_object().map(|b| (k, b)));
    match s(msg, "role") {
        Some("toolResult") => {
            let text: Vec<String> = blocks.filter_map(|(_, b)| match s(b, "type") {
                Some("text") => Some(clean(s(b, "text").unwrap_or(""))),
                Some("image") => Some(format!("[imagem {}]", s(b, "mimeType").filter(|m| !m.is_empty()).unwrap_or("desconhecida"))),
                _ => None,
            }).filter(|p| !p.is_empty()).collect();
            vec![ChatEvent { tool_use_id: s(msg, "toolCallId").map(str::to_owned), result: Some(text.join("\n")),
                is_error: Some(truthy(msg.get("isError")).is_some()), ..ev_of(ChatKind::ToolResult, 0) }]
        }
        Some("user") => blocks.filter(|(_, b)| s(b, "type") == Some("text") && s(b, "text").is_some_and(|t| !t.trim().is_empty()))
            .map(|(k, b)| ChatEvent { text: Some(clean(s(b, "text").unwrap_or(""))), ..ev_of(ChatKind::UserMsg, k) })
            .collect(),
        Some("assistant") => {
            let cache_read = msg.get("usage").and_then(Value::as_object).and_then(|u| u.get("cacheRead")).and_then(Value::as_u64);
            blocks.filter_map(|(k, b)| match s(b, "type") {
                Some("text") if s(b, "text").is_some_and(|t| !t.trim().is_empty()) =>
                    Some(ChatEvent { text: Some(clean(s(b, "text").unwrap_or(""))), cache_read, ..ev_of(ChatKind::AssistantMsg, k) }),
                Some("thinking") if s(b, "thinking").is_some_and(|t| !t.trim().is_empty()) =>
                    Some(ChatEvent { text: Some(clean(s(b, "thinking").unwrap_or(""))), ..ev_of(ChatKind::Thinking, k) }),
                Some("toolCall") => Some(ChatEvent {
                    tool_name: s(b, "name").map(str::to_owned),
                    tool_input: Some(b.get("arguments").and_then(Value::as_object).cloned().unwrap_or_default()),
                    tool_use_id: s(b, "id").map(str::to_owned),
                    ..ev_of(ChatKind::ToolUse, k)
                }),
                _ => None,
            }).collect()
        }
        _ => Vec::new(),
    }
}

/// A cauda do transcript do subagente nos `ChatEvent` do chat; None se não deu pra ler.
fn tail_events(f: &Path, limit: usize, layout: Layout) -> Option<Vec<ChatEvent>> {
    let file = File::open(f).map_err(|e| tracing::warn!("subagents: não consegui ler os eventos de {}: {e}", f.display())).ok()?;
    let mut out = VecDeque::with_capacity(limit.min(1024));
    let mut claude = LineParser::new(Provider::Claude);
    let mut offset = 0u64;
    for line in BufReader::new(file).split(b'\n') {
        let line = line.map_err(|e| tracing::warn!("subagents: não consegui ler os eventos de {}: {e}", f.display())).ok()?;
        let evs = match layout {
            Layout::Claude => claude.feed(&line, offset),
            Layout::Kimi | Layout::Pi => match decode_line(&line) {
                Some(Value::Object(o)) => if matches!(layout, Layout::Kimi) { kimi_events(&o) } else { pi_events(&o) },
                _ => Vec::new(),
            },
        };
        offset += line.len() as u64 + 1;
        for mut ev in evs {
            if limit == 0 {
                continue;
            }
            crate::transcript::finish(&mut ev);
            if out.len() == limit {
                out.pop_front();
            }
            out.push_back(ev);
        }
    }
    Some(out.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, lines: &[Value]) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, lines.iter().map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    }

    fn claude_agent(dir: &Path, conv: &str, id: &str, prompt: &str) {
        write(&dir.join(conv).join("subagents").join(format!("agent-{id}.jsonl")), &[
            json!({"type": "user", "timestamp": "2026-10-13T10:00:00Z", "message": {"content": prompt}}),
            json!({"type": "assistant", "timestamp": "2026-10-13T10:00:05Z", "message": {"content": [
                {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}},
                {"type": "tool_use", "name": "Read", "input": {"file_path": "/a"}},
                {"type": "tool_use", "name": "Bash", "input": {"command": "pwd"}},
                {"type": "text", "text": " feito "}]}}),
        ]);
    }

    fn row(v: Value) -> SessionRow {
        serde_json::from_value(v).unwrap()
    }

    /// A conversa da sessão vale; o transcript mais novo do cwd (de outra conversa) não entra, e o
    /// chute sem vínculo não mostra nada de ninguém.
    #[test]
    fn lists_only_the_sessions_own_conversation_never_the_newest_in_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("own.jsonl");
        let foreign = dir.path().join("foreign.jsonl");
        write(&own, &[json!({"type": "user"})]);
        claude_agent(dir.path(), "own", "mine", "meu");
        std::thread::sleep(Duration::from_millis(20));
        write(&foreign, &[json!({"type": "user"})]);
        claude_agent(dir.path(), "foreign", "theirs", "deles");

        let tracked = row(json!({"name": "s", "provider": "claude", "jsonl": own, "tracked": true}));
        let (provider, jsonl) = conversation(&tracked, None).unwrap();
        let ids: Vec<_> = list_subagents(&provider, &jsonl).iter().map(|a| a["agentId"].clone()).collect();
        assert_eq!(ids, [json!("mine")]);

        let guessed = row(json!({"name": "s", "provider": "claude", "jsonl": foreign, "tracked": false}));
        assert!(conversation(&guessed, None).is_none(), "chute do cwd não decide a conversa");
        let fresh = row(json!({"name": "s", "provider": "claude", "tracked": true}));
        assert!(conversation(&fresh, None).is_none());
    }

    #[test]
    fn list_rereads_a_subagent_only_after_its_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("c.jsonl");
        claude_agent(dir.path(), "c", "a1", "faz X");
        assert_eq!(list_subagents("claude", &own)[0]["toolCalls"], 3);
        let f = dir.path().join("c/subagents/agent-a1.jsonl");
        let mut file = std::fs::OpenOptions::new().append(true).open(&f).unwrap();
        std::io::Write::write_all(&mut file, format!("{}\n", json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "Grep", "input": {"pattern": "y"}}]}})).as_bytes()).unwrap();
        drop(file);
        assert_eq!(list_subagents("claude", &own)[0]["toolCalls"], 4);
        assert_eq!(list_subagents("claude", &own)[0]["recent"].as_array().unwrap().last().unwrap()["name"], "Grep");
    }

    /// O modelo que respondeu, não o apelido do Agent do pai: num motor "opus" vira o modelo do motor.
    #[test]
    fn claude_item_carries_the_model_that_answered() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("c.jsonl");
        write(&dir.path().join("c/subagents/agent-a1.jsonl"), &[
            json!({"type": "user", "message": {"content": "faz X"}}),
            json!({"type": "assistant", "message": {"model": "gpt-6.1-sol", "content": [{"type": "text", "text": "ok"}]}}),
            json!({"type": "assistant", "isApiErrorMessage": true, "message": {"model": "<synthetic>", "content": [{"type": "text", "text": "529"}]}}),
        ]);
        assert_eq!(list_subagents("claude", &own)[0]["model"], "gpt-6.1-sol");
        claude_agent(dir.path(), "c", "a2", "sem resposta com modelo");
        let a2 = list_subagents("claude", &own).into_iter().find(|a| a["agentId"] == "a2").unwrap();
        assert!(!a2.contains_key("model"));
    }

    #[test]
    fn response_shape_matches_the_python_per_provider() {
        let dir = tempfile::tempdir().unwrap();
        let own = dir.path().join("c.jsonl");
        claude_agent(dir.path(), "c", "a1", "<teammate-message from=\"lead\">\nfaz X\n</teammate-message>");
        std::fs::write(dir.path().join("c/subagents/agent-a1.meta.json"), r#"{"agentType":"Explore"}"#).unwrap();
        let a = &list_subagents("claude", &own)[0];
        let keys: Vec<_> = a.keys().map(String::as_str).collect();
        assert_eq!(keys.len(), 11);
        for k in ["agentId", "agentType", "prompt", "startedAt", "updatedAt", "mtime", "toolCalls", "tools", "recent", "lastText", "failed"] {
            assert!(keys.contains(&k), "{k}");
        }
        assert_eq!((a["agentType"].clone(), a["prompt"].clone(), a["toolCalls"].clone(), a["lastText"].clone(), a["failed"].clone()),
                   (json!("Explore"), json!("faz X"), json!(3), json!("feito"), json!(false)));
        assert_eq!(a["tools"], json!([{"name": "Bash", "count": 2}, {"name": "Read", "count": 1}]));
        assert_eq!(a["recent"][1], json!({"name": "Read", "target": "/a"}));
        assert_eq!((a["startedAt"].clone(), a["updatedAt"].clone()), (json!("2026-10-13T10:00:00Z"), json!("2026-10-13T10:00:05Z")));
        let d = get_subagent("claude", &own, "a1", 200).unwrap();
        assert_eq!(d["events"].as_array().unwrap().len(), 5, "{:?}", d["events"]);
        assert!(get_subagent("claude", &own, "../x", 0).is_none() && get_subagent("claude", &own, "nada", 0).is_none());

        // Kimi: pasta por subagente ao lado de agents/main, `finished` do turn.ended.
        let wire = dir.path().join("k/agents/main/wire.jsonl");
        write(&wire, &[]);
        write(&dir.path().join("k/agents/agent-0/wire.jsonl"), &[
            json!({"type": "profile.bind", "profileName": "coder", "time": 1_760_000_000_123_i64}),
            json!({"type": "turn.prompt", "input": [{"type": "text", "text": "<git-context>x</git-context> olha"}]}),
            json!({"type": "context.append_loop_event", "event": {"type": "tool.call", "name": "Grep", "args": {"pattern": "p"}, "toolCallId": "t1", "uuid": "u1"}}),
            json!({"type": "context.append_loop_event", "event": {"type": "tool.result", "toolCallId": "t1", "result": {"output": "ok"}}}),
            json!({"type": "turn.ended", "time": 1_760_000_001_000_i64}),
        ]);
        let k = &list_subagents("kimi", &wire);
        assert_eq!(k.len(), 1, "main não é subagente");
        let k = &k[0];
        assert_eq!((k["agentId"].clone(), k["agentType"].clone(), k["prompt"].clone(), k["finished"].clone()),
                   (json!("agent-0"), json!("coder"), json!("olha"), json!(true)));
        assert_eq!((k["startedAt"].clone(), k["updatedAt"].clone()), (json!("2025-10-09T08:53:20.123000+00:00"), json!("2025-10-09T08:53:21+00:00")));
        assert!(!k.contains_key("failed"));
        let ev = get_subagent("kimi", &wire, "agent-0", 10).unwrap()["events"].clone();
        assert_eq!((ev[0]["kind"].clone(), ev[1]["id"].clone()), (json!("tool_use"), json!("res:t1")));
        assert!(get_subagent("kimi", &wire, "main", 0).is_none() && get_subagent("kimi", &wire, "..", 0).is_none());

        // Pi: só o run mais alto, `finished` do stopReason da última mensagem.
        let pi = dir.path().join("p.jsonl");
        write(&pi, &[]);
        write(&dir.path().join("p/task1/run-1/session.jsonl"), &[json!({"type": "message", "message": {"role": "user", "content": [{"type": "text", "text": "velho"}]}})]);
        write(&dir.path().join("p/task1/run-2/session.jsonl"), &[
            json!({"type": "session_info", "name": "subagent-scout-0199a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b-0"}),
            json!({"type": "message", "id": "m1", "message": {"role": "user", "content": [{"type": "text", "text": "novo"}]}}),
            json!({"type": "message", "id": "m2", "message": {"role": "assistant", "stopReason": "stop", "content": [
                {"type": "toolCall", "name": "bash", "arguments": {"command": "ls"}, "id": "c1"}, {"type": "text", "text": "\u{1b}[38;2;1;1;1mfim\u{1b}[0m"}]}}),
        ]);
        let p = &list_subagents("pi", &pi)[0];
        assert_eq!((p["agentId"].clone(), p["agentType"].clone(), p["prompt"].clone(), p["finished"].clone(), p["recent"][0]["target"].clone()),
                   (json!("task1"), json!("scout"), json!("novo"), json!(true), json!("ls")));
        let ev = get_subagent("pi", &pi, "task1", 10).unwrap()["events"].clone();
        assert_eq!((ev[2]["id"].clone(), ev[2]["text"].clone()), (json!("m2:1"), json!("fim")));
    }

    #[test]
    fn items_carry_lifecycle_and_message_capability() {
        use crate::plugin::AgentState;
        let item = |extra: Value| { let mut m = unreadable("a", 0.0); m.insert("updatedAt".into(), json!("2026-10-13T10:00:05Z"));
            if let Value::Object(e) = extra { m.extend(e); } m };
        let pick = |a: &Map<String, Value>| json!([a["state"], a["endedAt"], a["canMessage"], a["messageBlocked"]]);

        let mut a = item(json!({}));
        decorate(&mut a, None, message_blocked("claude"), None);
        assert_eq!(pick(&a), json!([null, null, true, null]), "Claude com terminal sem sinal ainda");
        let life = AgentLife { state: AgentState::Finished, ended_at: Some("2026-10-13T10:00:09.000Z".into()) };
        decorate(&mut a, Some(&life), Some(NO_PLUGIN), None);
        assert_eq!(pick(&a), json!(["finished", "2026-10-13T10:00:09.000Z", false, NO_PLUGIN]));
        decorate(&mut a, Some(&AgentLife { state: AgentState::Running, ended_at: None }), None, None);
        assert_eq!(pick(&a), json!(["running", null, true, null]));

        let mut h = item(json!({"failed": true}));
        decorate(&mut h, None, Some(OLD_PLUGIN), None);
        assert_eq!(pick(&h), json!([null, null, false, OLD_PLUGIN]), "plugin sem o modo agent_message");

        let mut k = item(json!({"finished": true}));
        decorate(&mut k, None, message_blocked("kimi"), None);
        assert_eq!(pick(&k), json!(["finished", "2026-10-13T10:00:05Z", false, NO_PLUGIN]));
        let mut p = item(json!({"finished": false}));
        decorate(&mut p, None, message_blocked("pi"), None);
        assert_eq!(pick(&p), json!(["running", null, false, NO_PLUGIN]));

        let mut u = item(json!({}));
        decorate(&mut u, None, Some(STATE_UNAVAILABLE), Some(STATE_UNAVAILABLE));
        assert_eq!((pick(&u), &u["problema"]), (json!([null, null, false, STATE_UNAVAILABLE]), &json!(STATE_UNAVAILABLE)), "falha de leitura aparece, estado fica nulo");
    }

    #[test]
    fn message_text_and_replies_follow_the_contract() {
        assert!(valid_text("oi\n\tsegue") && !valid_text("  \n") && !valid_text("a\u{1b}[31m") && !valid_text("a\rb"));
        let h = HeaderMap::new();
        let body = |r: Response| async move {
            let status = r.status().as_u16();
            let v: Value = serde_json::from_slice(&axum::body::to_bytes(r.into_body(), 4096).await.unwrap()).unwrap();
            (status, v)
        };
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        rt.block_on(async {
            assert_eq!(body(sent_reply(&AgentSend::Delivered, &h)).await, (200, json!({"delivered": true})));
            let (s, v) = body(sent_reply(&AgentSend::Refused("this session has no SendMessage tool".into()), &h)).await;
            assert_eq!((s, v["detail"]["code"].clone(), v["detail"]["params"]["reason"].clone()),
                (409, json!("erro_subagente_mensagem_recusada"), json!("this session has no SendMessage tool")));
            let (s, v) = body(sent_reply(&AgentSend::NoPlugin, &h)).await;
            assert_eq!((s, v["detail"]["code"].clone()), (409, json!(NO_PLUGIN)));
            let (s, v) = body(sent_reply(&AgentSend::NoAnswer, &h)).await;
            assert_eq!((s, v["detail"]["code"].clone()), (504, json!("erro_subagente_mensagem_sem_resposta")));
        });
    }

    #[test]
    fn route_paths_parse_only_the_subagent_routes() {
        assert_eq!(target_of("/api/sessions/a%20b/subagents"), Some(("a b".into(), None)));
        assert_eq!(target_of("/api/sessions/a/subagents/x1"), Some(("a".into(), Some("x1".into()))));
        assert_eq!(target_of("/api/sessions/a/subagentsX"), None);
        assert_eq!(target_of("/api/sessions/a/subagents/x/y"), None);
    }
}
