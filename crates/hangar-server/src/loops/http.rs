//! Rotas do loop e do `then` do dono, com os corpos do Python: `PUT`/`DELETE /then`,
//! `POST`/`GET`/`DELETE /loop`, `POST /loop/refine` e `POST /loop/resolve`. O convidado segue ao
//! Python, como antes.
use super::store::{self, is_active, status, Loop, Store, LOCK};
use super::tick;
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use futures_util::future::BoxFuture;
use hangar_api::session::SessionRow;
use percent_encoding::percent_decode_str;
use serde_json::{json, Map, Value};
use std::net::SocketAddr;
use std::sync::Arc;

use crate::routes::AppState;
use crate::session_write::{detail_body, json_response};

const BODY_LIMIT: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Op { ThenSet, ThenClear, Create, Get, Stop, Refine, Resolve }

/// A rota pelo caminho (segmento do nome escapado) e o método.
pub fn op(method: &Method, path: &str) -> Option<(String, Op)> {
    let rest = path.strip_prefix("/api/sessions/")?;
    let (seg, tail) = rest.split_once('/')?;
    if seg.is_empty() { return None; }
    let op = match (method.as_str(), tail) {
        ("PUT", "then") => Op::ThenSet,
        ("DELETE", "then") => Op::ThenClear,
        ("POST", "loop") => Op::Create,
        ("GET", "loop") => Op::Get,
        ("DELETE", "loop") => Op::Stop,
        ("POST", "loop/refine") => Op::Refine,
        ("POST", "loop/resolve") => Op::Resolve,
        _ => return None,
    };
    Some((percent_decode_str(seg).decode_utf8().ok()?.into_owned(), op))
}

pub(crate) fn matches(method: &Method, path: &str) -> bool { op(method, path).is_some() }

/// O que as rotas pedem ao servidor; o teste troca tudo (inclusive o aviso no celular).
pub trait Backend: Send + Sync {
    fn store(&self) -> Store;
    fn now(&self) -> f64;
    fn automations(&self) -> bool;
    /// A linha da sessão; `Err` é a resposta da lista indisponível.
    fn row<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<SessionRow>, (StatusCode, Value)>>;
    fn exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, bool>;
    /// Branch da worktree onde o agente da sessão trabalha.
    fn branch(&self, row: &SessionRow) -> Option<String>;
    /// Na fila da sessão, esperando a vez; o id da entrada ou `(código, mensagem)`.
    fn append<'a>(&'a self, name: &'a str, text: &'a str) -> BoxFuture<'a, Result<String, (String, String)>>;
    fn drain<'a>(&'a self, name: &'a str) -> BoxFuture<'a, ()>;
    fn notify(&self, name: &str, status: &str, reason: &str);
    fn refine<'a>(&'a self, goal: &'a str, check_cmd: Option<&'a str>) -> BoxFuture<'a, Result<String, String>>;
}

/// O tique visto pelas rotas: só o relógio, o aviso e a entrega do re-prompt.
struct RouteEnv<'a> { b: &'a dyn Backend, name: &'a str }

impl tick::Env for RouteEnv<'_> {
    fn now(&self) -> f64 { self.b.now() }
    fn automations(&self) -> bool { self.b.automations() }
    fn branch(&self) -> Option<String> { None }
    fn entry_delivered(&self, _id: Option<String>) -> BoxFuture<'_, Option<bool>> { Box::pin(async { None }) }
    fn run_check(&self, _cmd: String) -> BoxFuture<'_, (i64, String)> { Box::pin(async { (super::check::UNRUNNABLE, String::new()) }) }
    fn last_assistant(&self) -> BoxFuture<'_, Option<String>> { Box::pin(async { None }) }
    fn enqueue(&self, text: String) -> BoxFuture<'_, Result<(), String>> {
        Box::pin(async move {
            self.b.append(self.name, &text).await.map_err(|(code, _)| code)?;
            self.b.drain(self.name).await;
            Ok(())
        })
    }
    fn notify(&self, status: &str, reason: &str) { self.b.notify(self.name, status, reason); }
}

type Reply = (StatusCode, Value);

fn refused(status: StatusCode, code: &str, msg: &str, params: Value) -> Reply { (status, detail_body(code, msg, params)) }

fn unprocessable(msg: &str) -> Reply {
    (StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"type": "value_error", "loc": ["body"], "msg": msg, "input": null}]}))
}

fn unwritable(e: std::io::Error) -> Reply {
    tracing::error!(code = "loop_store_unwritable", kind = ?e.kind(), "loop: sidecar não gravou");
    refused(StatusCode::INTERNAL_SERVER_ERROR, "loop_store_unwritable", "o estado do loop não pôde ser gravado", json!({}))
}

/// Corpo estrito (`_StrictBody`): objeto JSON só com as chaves conhecidas.
fn object(bytes: &[u8], allowed: &[&str]) -> Result<Map<String, Value>, Reply> {
    let Ok(Value::Object(m)) = serde_json::from_slice::<Value>(bytes) else { return Err(unprocessable("Input should be a valid dictionary")) };
    if let Some(extra) = m.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(unprocessable(&format!("Extra inputs are not permitted: {extra}")));
    }
    Ok(m)
}

fn text(m: &Map<String, Value>, key: &str, max: Option<usize>) -> Result<String, Reply> {
    let s = m.get(key).and_then(Value::as_str).ok_or_else(|| unprocessable(&format!("{key}: Input should be a valid string")))?;
    let n = s.chars().count();
    if n < 1 { return Err(unprocessable(&format!("{key}: String should have at least 1 character"))); }
    if max.is_some_and(|max| n > max) { return Err(unprocessable(&format!("{key}: String should have at most {} characters", max.unwrap()))); }
    Ok(s.to_owned())
}

fn optional_text(m: &Map<String, Value>, key: &str) -> Result<Option<String>, Reply> {
    match m.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some(s.clone())),
        _ => Err(unprocessable(&format!("{key}: Input should be a valid string"))),
    }
}

fn boolean(m: &Map<String, Value>, key: &str, default: Option<bool>) -> Result<bool, Reply> {
    match (m.get(key), default) {
        (None, Some(d)) => Ok(d),
        (Some(Value::Bool(b)), _) => Ok(*b),
        _ => Err(unprocessable(&format!("{key}: Input should be a valid boolean"))),
    }
}

fn loop_reply(d: Option<Loop>) -> Reply { (StatusCode::OK, json!({"loop": d})) }

/// Uma rota já autorizada e com a trava de transferência (menos o `GET`).
pub async fn handle(b: &dyn Backend, name: &str, op: Op, body: &[u8]) -> Reply {
    match op {
        Op::Get => get(b, name).await,
        Op::ThenClear => match b.store().clear_chain(name) {
            Ok(()) => (StatusCode::OK, json!({"ok": true})),
            Err(e) => unwritable(e),
        },
        Op::ThenSet => then_set(b, name, body).await.unwrap_or_else(|r| r),
        Op::Create => create(b, name, body).await.unwrap_or_else(|r| r),
        Op::Stop => stop(b, name).await.unwrap_or_else(|r| r),
        Op::Resolve => resolve(b, name, body).await.unwrap_or_else(|r| r),
        Op::Refine => refine(b, body).await.unwrap_or_else(|r| r),
    }
}

async fn get(b: &dyn Backend, name: &str) -> Reply {
    let row = match b.row(name).await { Ok(row) => row, Err(r) => return r };
    let suggestions = match row.and_then(|r| r.cwd).filter(|c| !c.is_empty()) {
        Some(cwd) => tokio::task::spawn_blocking(move || suggest_checks(std::path::Path::new(&cwd))).await.unwrap_or_default(),
        None => Vec::new(),
    };
    (StatusCode::OK, json!({"loop": b.store().get(name), "suggestions": suggestions}))
}

async fn then_set(b: &dyn Backend, name: &str, body: &[u8]) -> Result<Reply, Reply> {
    let m = object(body, &["target", "text"])?;
    let (target, text) = (text(&m, "target", None)?, text(&m, "text", None)?);
    if target == name {
        return Err(refused(StatusCode::BAD_REQUEST, "erro_encadeamento_proprio", "sessao nao pode encadear pra si mesma", json!({})));
    }
    if !b.exists(&target).await {
        return Err(refused(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessao alvo nao encontrada", json!({})));
    }
    b.store().set_chain(name, &target, &text).map_err(unwritable)?;
    Ok((StatusCode::OK, json!({"ok": true})))
}

async fn create(b: &dyn Backend, name: &str, body: &[u8]) -> Result<Reply, Reply> {
    let m = object(body, &["goal", "check_cmd", "max_iters", "require_branch"])?;
    let goal = text(&m, "goal", None)?;
    let check_cmd = optional_text(&m, "check_cmd")?;
    let max_iters = match m.get("max_iters") {
        None => 10,
        Some(v) => v.as_i64().ok_or_else(|| unprocessable("max_iters: Input should be a valid integer"))?,
    };
    if !(1..=100).contains(&max_iters) { return Err(unprocessable("max_iters: Input should be between 1 and 100")); }
    let require_branch = boolean(&m, "require_branch", Some(true))?;
    let Some(row) = b.row(name).await? else {
        return Err(refused(StatusCode::NOT_FOUND, "erro_sessao_inexistente", "sessão não encontrada", json!({})));
    };
    // Sem fim de turno o tique nunca dispara: o loop ficaria rodando mudo.
    if row.provider != "claude" {
        return Err(refused(StatusCode::CONFLICT, "erro_loop_provider_invalido", "loop runner só suporta sessões claude", json!({})));
    }
    if !b.automations() {
        return Err(refused(StatusCode::CONFLICT, "erro_automacoes_desligadas", "automações desligadas (kill-switch)", json!({})));
    }
    let store = b.store();
    {
        let _one = LOCK.lock().await;
        if store.get(name).is_some_and(|d| is_active(&d)) {
            return Err(refused(StatusCode::CONFLICT, "erro_loop_ja_ativo", "já existe um loop ativo nesta sessão", json!({})));
        }
        if let Some(br) = b.branch(&row).filter(|b| b == "main" || b == "master") && require_branch {
            return Err(refused(StatusCode::CONFLICT, "erro_loop_branch_invalida",
                &format!("sessão está na branch {br} — crie uma branch ou desligue 'exigir branch'"), json!({"br": br})));
        }
        let mut d = store::new_loop(&goal, check_cmd.as_deref(), max_iters, require_branch, b.now());
        let entry = b.append(name, &goal).await.map_err(|(code, msg)| refused(StatusCode::SERVICE_UNAVAILABLE, &code, &msg, json!({})))?;
        d.insert("goal_entry_id".into(), json!(entry));
        store.set(name, &d).map_err(unwritable)?;
    }
    b.drain(name).await;
    Ok(loop_reply(store.get(name)))
}

async fn stop(b: &dyn Backend, name: &str) -> Result<Reply, Reply> {
    let store = b.store();
    let _one = LOCK.lock().await;
    if store.get(name).is_none() {
        return Err(refused(StatusCode::NOT_FOUND, "erro_loop_inexistente", "nenhum loop nesta sessão", json!({})));
    }
    let d = tick::end(&store, name, "stopped", "parado pelo usuário", &RouteEnv { b, name }).map_err(unwritable)?;
    Ok(loop_reply(d))
}

async fn resolve(b: &dyn Backend, name: &str, body: &[u8]) -> Result<Reply, Reply> {
    let m = object(body, &["accept"])?;
    let accept = boolean(&m, "accept", None)?;
    let (store, env) = (b.store(), RouteEnv { b, name });
    let _one = LOCK.lock().await;
    let Some(mut d) = store.get(name).filter(|d| status(d) == "done_claimed") else {
        return Err(refused(StatusCode::CONFLICT, "erro_loop_estado_errado", "loop não está aguardando confirmação", json!({})));
    };
    if accept { return Ok(loop_reply(tick::end(&store, name, "done", "confirmado pronto", &env).map_err(unwritable)?)); }
    d.insert("status".into(), json!("running"));
    store.set(name, &d).map_err(unwritable)?;
    let max = store::int(&d, "max_iters");
    if store::int(&d, "iter") + 1 > max {
        return Ok(loop_reply(tick::end(&store, name, "exhausted", &format!("esgotou {max} iterações"), &env).map_err(unwritable)?));
    }
    if !b.row(name).await?.is_some_and(|r| r.jsonl.is_some()) {
        tick::end(&store, name, "failed", "sessão morta", &env).map_err(unwritable)?;
        return Ok(loop_reply(store.get(name)));
    }
    let after = tick::reprompt(&store, name, d, "conclusão rejeitada pelo usuário", None, &env).await.map_err(unwritable)?;
    Ok(loop_reply(after))
}

async fn refine(b: &dyn Backend, body: &[u8]) -> Result<Reply, Reply> {
    let m = object(body, &["goal", "check_cmd"])?;
    let goal = text(&m, "goal", Some(super::refine::GOAL_MAX))?;
    let check_cmd = optional_text(&m, "check_cmd")?;
    if !b.automations() {
        return Err(refused(StatusCode::CONFLICT, "erro_automacoes_desligadas", "automações desligadas (kill-switch)", json!({})));
    }
    match b.refine(&goal, check_cmd.as_deref()).await {
        Ok(goal) => Ok((StatusCode::OK, json!({"goal": goal}))),
        Err(msg) => {
            tracing::warn!(code = "loop_refine_failed", "loop/refine falhou: {msg}");
            Err((StatusCode::BAD_GATEWAY, json!({"detail": msg})))
        }
    }
}

/// Comandos de conferência que o projeto já tem, um argv cada (sem `&&`: não há shell).
pub fn suggest_checks(cwd: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let package = cwd.join("package.json");
    if package.is_file() {
        let Ok(Ok(v)) = std::fs::read(&package).map(|raw| serde_json::from_slice::<Value>(&raw)) else { return out };
        match v.get("scripts") {
            Some(Value::Object(scripts)) => out.extend(["check", "test", "lint"].iter().filter(|s| scripts.contains_key(**s)).map(|s| format!("npm run {s}"))),
            // `in` do Python numa lista procura o item.
            Some(Value::Array(items)) => out.extend(["check", "test", "lint"].iter().filter(|s| items.iter().any(|i| i == **s)).map(|s| format!("npm run {s}"))),
            Some(Value::String(text)) => out.extend(["check", "test", "lint"].iter().filter(|s| text.contains(**s)).map(|s| format!("npm run {s}"))),
            _ => {}
        }
    }
    if cwd.join("pyproject.toml").is_file() || cwd.join("pytest.ini").is_file() { out.push("uv run pytest -x -q".into()); }
    if cwd.join("Cargo.toml").is_file() { out.push("cargo test".into()); }
    if cwd.join("go.mod").is_file() { out.push("go test ./...".into()); }
    out.truncate(4);
    out
}

/// O servidor de verdade.
pub struct Live { pub st: Arc<AppState> }

impl Backend for Live {
    fn store(&self) -> Store { super::store(&self.st) }
    fn now(&self) -> f64 { crate::nav_requests::now() }
    fn automations(&self) -> bool { super::automations(&self.st) }
    fn row<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<SessionRow>, Reply>> {
        Box::pin(async move {
            match tokio::time::timeout(crate::locate::LOCATE_TIMEOUT, self.st.list.find_row(name)).await {
                Ok(Ok(row)) => Ok(row),
                Ok(Err(e)) => Err(refused(StatusCode::SERVICE_UNAVAILABLE, e.code, e.detail, json!({}))),
                Err(_) => Err(refused(StatusCode::SERVICE_UNAVAILABLE, "locate_timeout", "a lista de sessões não respondeu a tempo", json!({}))),
            }
        })
    }
    fn exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move {
            let home = self.st.list.env().dirs.as_ref().map(|d| d.home.clone()).or_else(std::env::home_dir).unwrap_or_default();
            crate::mcp::identity::exists(&crate::mcp::identity::Local { home: &home, mux: &self.st.list.env().mux }, name).await
        })
    }
    fn branch(&self, row: &SessionRow) -> Option<String> { super::branch(row) }
    fn append<'a>(&'a self, name: &'a str, text: &'a str) -> BoxFuture<'a, Result<String, (String, String)>> {
        Box::pin(async move { super::append(&self.st, name, text).await.map_err(|e| (e.code, e.message)) })
    }
    fn drain<'a>(&'a self, name: &'a str) -> BoxFuture<'a, ()> { Box::pin(super::drain(&self.st, name)) }
    fn notify(&self, name: &str, status: &str, reason: &str) { super::notify(&self.st, name, status, reason); }
    fn refine<'a>(&'a self, goal: &'a str, check_cmd: Option<&'a str>) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async move {
            let bridge = crate::accounts::http::bridge(&self.st).ok();
            let cli = super::refine::Cli { accounts: self.st.accounts.clone(), bridge };
            super::refine::refine(goal, check_cmd, &cli).await
        })
    }
}

/// Só o dono; o resto segue ao Python.
pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = crate::routes::gate(&st, peer, &req);
    if !owner { return crate::routes::pass(&st, req, &fwd).await; }
    let headers = req.headers().clone();
    let mut r = serve(&st, req).await;
    crate::routes::cors(&headers, r.headers_mut());
    r
}

async fn serve(st: &Arc<AppState>, req: Request) -> Response {
    let Some((name, op)) = op(req.method(), req.uri().path()) else {
        return json_response(StatusCode::NOT_FOUND, json!({"detail": "Not Found"}));
    };
    // A trava vem antes do corpo, como a dependência `_transfer_guard` antes da validação.
    let _pass = if op == Op::Get { None } else {
        match crate::plans::routes::enter(st, &name).await { Ok(p) => p, Err(r) => return r }
    };
    let Ok(bytes) = to_bytes(req.into_body(), BODY_LIMIT).await else { return crate::plans::routes::unprocessable_body("corpo grande demais") };
    let (status, body) = handle(&Live { st: st.clone() }, &name, op, &bytes).await;
    json_response(status, body)
}

#[cfg(test)]
mod tests;
