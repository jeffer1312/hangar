//! Aviso no celular pelo Expo Push: o app registra o token pelo dono, o Python decide quando avisar.
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::StatusCode, response::{IntoResponse, Response}};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::HashSet, io, net::SocketAddr, path::{Path, PathBuf}, sync::Arc, time::Duration};
use crate::{diag::DiagClient, routes::{AppState, gate}, session_write::{detail_body, json_response}};

#[derive(Default, Serialize, Deserialize)]
struct Registry { #[serde(default)] tokens: Vec<Entry>, #[serde(default)] pending: Vec<Pending> }

#[derive(Clone, Serialize, Deserialize)]
struct Entry { token: String, server_id: String, label: String, locale: String, at: String }

#[derive(Clone, Serialize, Deserialize)]
struct Pending { ticket_id: String, token: String }

#[derive(Deserialize)]
struct Subscribe { token: String, server_id: String, label: String, locale: String }

#[derive(Deserialize)]
struct Localized { pt: String, en: String }

impl Localized {
    fn pick(&self, locale: &str) -> &str { if locale == "en" { &self.en } else { &self.pt } }
}

#[derive(Deserialize)]
struct Notify { session: String, title: Localized, body: Localized, #[serde(default)] list: bool }

// O Expo guarda recibos por 24 h e lê até 1000 por chamada; ticket sem recibo não fica para sempre.
const MAX_PENDING: usize = 1000;
// Um `/send` do Expo aceita até 100 mensagens.
const MAX_MESSAGES: usize = 100;
const BODY_LIMIT: usize = 16 * 1024;

fn path(home: &Path) -> PathBuf { home.join(".hangar").join("expo-push-tokens.json") }

fn read(path: &Path) -> io::Result<Registry> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Registry::default()),
        Err(error) => Err(error),
    }
}

/// Relê, muda e grava sob a trava: o envio só aplica podas e tickets novos sobre o que estiver gravado,
/// e uma inscrição feita durante o envio fica.
fn update(path: &Path, change: impl FnOnce(&mut Registry)) -> io::Result<()> {
    std::fs::create_dir_all(path.parent().ok_or_else(|| io::Error::other("pasta ausente"))?)?;
    let lock = std::fs::OpenOptions::new().read(true).append(true).create(true).open(path.with_extension("lock"))?;
    lock.lock()?;
    let mut registry = read(path)?;
    change(&mut registry);
    crate::runtime::queue::atomic_write(path, &serde_json::to_vec(&registry)?)
}

fn failure(status: StatusCode, code: &str, message: &str) -> Response {
    json_response(status, detail_body(code, message, json!({})))
}

pub async fn subscribe(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !gate(&state, peer, &request).1 {
        return failure(StatusCode::FORBIDDEN, "push_expo_owner_only", "só o dono pode registrar o aviso no celular");
    }
    serve(&state, request).await
}

pub(crate) async fn serve(state: &Arc<AppState>, request: Request) -> Response {
    let parsed = to_bytes(request.into_body(), BODY_LIMIT).await.ok()
        .and_then(|raw| serde_json::from_slice::<Subscribe>(&raw).ok())
        .filter(|s| s.token.starts_with("ExponentPushToken[") && ["pt", "en"].contains(&s.locale.as_str())
            && !s.server_id.trim().is_empty() && !s.label.trim().is_empty());
    let Some(new) = parsed else {
        return failure(StatusCode::BAD_REQUEST, "push_expo_invalid", "token do Expo, servidor, nome ou idioma inválidos");
    };
    let entry = Entry { token: new.token, server_id: new.server_id, label: new.label, locale: new.locale, at: chrono::Utc::now().to_rfc3339() };
    let path = path(&state.accounts.env.home);
    let saved = tokio::task::spawn_blocking(move || update(&path, |registry| {
        registry.tokens.retain(|e| e.token != entry.token);
        registry.tokens.push(entry);
    })).await;
    match saved {
        Ok(Ok(())) => json_response(StatusCode::OK, json!({"ok": true})),
        _ => failure(StatusCode::SERVICE_UNAVAILABLE, "push_expo_unavailable", "não foi possível guardar o token do aviso"),
    }
}

/// `POST /__hangar_server/push/expo`: responde na hora; o envio segue sozinho e falha só vai ao diário.
pub async fn private(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if !crate::workspace_routes::private_ok(&state, peer, request.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Some(notify) = to_bytes(request.into_body(), BODY_LIMIT).await.ok()
        .and_then(|raw| serde_json::from_slice::<Notify>(&raw).ok()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let base = state.accounts.env.base.get("HANGAR_EXPO_PUSH_URL").filter(|v| !v.is_empty())
        .map_or("https://exp.host", String::as_str).trim_end_matches('/').to_owned();
    tokio::spawn(deliver(path(&state.accounts.env.home), base, state.diag.clone(), notify));
    StatusCode::ACCEPTED.into_response()
}

/// O aviso que o próprio Rust decidiu (`push::send`): mesmo envio do `private`, sem a volta HTTP.
pub(crate) fn send(state: &AppState, session: &str, title: (String, String), body: (String, String), list: bool) {
    let base = state.accounts.env.base.get("HANGAR_EXPO_PUSH_URL").filter(|v| !v.is_empty())
        .map_or("https://exp.host", String::as_str).trim_end_matches('/').to_owned();
    let notify = Notify { session: session.to_owned(), title: Localized { pt: title.0, en: title.1 },
        body: Localized { pt: body.0, en: body.1 }, list };
    tokio::spawn(deliver(path(&state.accounts.env.home), base, state.diag.clone(), notify));
}

async fn post(client: &reqwest::Client, url: String, body: Value) -> Result<Value, &'static str> {
    let response = client.post(url).header("content-type", "application/json").body(body.to_string())
        .send().await.map_err(|_| "push_expo_unreachable")?;
    if !response.status().is_success() { return Err("push_expo_status"); }
    let bytes = response.bytes().await.map_err(|_| "push_expo_unreachable")?;
    serde_json::from_slice(&bytes).map_err(|_| "push_expo_bad_reply")
}

fn unregistered(result: &Value) -> bool { result["details"]["error"] == "DeviceNotRegistered" }

async fn deliver(path: PathBuf, base: String, diag: DiagClient, notify: Notify) {
    let report = |code: &str| diag.report("rust.push_expo_failed", "push", code, "aviso Expo não saiu; não repete");
    let reading = path.clone();
    let Ok(Ok(registry)) = tokio::task::spawn_blocking(move || read(&reading)).await else { return report("push_expo_registry") };
    if registry.tokens.is_empty() { return; }
    static CLIENT: std::sync::OnceLock<Option<reqwest::Client>> = std::sync::OnceLock::new();
    let Some(client) = CLIENT.get_or_init(|| crate::http_client().timeout(Duration::from_secs(10)).build().ok()) else {
        return report("push_expo_client");
    };
    let (mut dead, mut read_ids) = (HashSet::new(), HashSet::new());
    if !registry.pending.is_empty() {
        let ids: Vec<&str> = registry.pending.iter().map(|p| p.ticket_id.as_str()).collect();
        match post(client, format!("{base}/--/api/v2/push/getReceipts"), json!({"ids": ids})).await {
            Ok(reply) => for (id, receipt) in reply["data"].as_object().into_iter().flatten() {
                if unregistered(receipt) {
                    if let Some(p) = registry.pending.iter().find(|p| &p.ticket_id == id) { dead.insert(p.token.clone()); }
                } else if receipt["status"] == "error" {
                    report(&format!("push_expo_receipt_error:{}", receipt["details"]["error"].as_str().unwrap_or("unknown")));
                }
                read_ids.insert(id.clone());
            },
            Err(code) => report(code),
        }
    }
    let all: Vec<&Entry> = registry.tokens.iter().filter(|e| !dead.contains(&e.token)).collect();
    let mut tickets = Vec::new();
    for targets in all.chunks(MAX_MESSAGES) {
        let messages: Vec<Value> = targets.iter().map(|e| {
            let data = if notify.list { json!({"server": e.server_id}) } else { json!({"server": e.server_id, "session": notify.session}) };
            json!({"to": e.token, "title": format!("{} · {}", e.label, notify.title.pick(&e.locale)),
                "body": notify.body.pick(&e.locale), "data": data, "channelId": "default", "priority": "high"})
        }).collect();
        match post(client, format!("{base}/--/api/v2/push/send"), Value::Array(messages)).await {
            // O Expo devolve os tickets na ordem das mensagens.
            Ok(reply) => {
                let answered = reply["data"].as_array().map_or(&[][..], Vec::as_slice);
                if answered.len() != targets.len() { report("push_expo_ticket_count"); }
                let mut failed = 0;
                for (ticket, entry) in answered.iter().zip(targets) {
                    match ticket["id"].as_str() {
                        Some(id) if ticket["status"] == "ok" => tickets.push(Pending { ticket_id: id.into(), token: entry.token.clone() }),
                        _ if unregistered(ticket) => { dead.insert(entry.token.clone()); }
                        _ => failed += 1,
                    }
                }
                if failed > 0 { report(&format!("push_expo_ticket_error:{failed}")); }
            }
            Err(code) => report(code),
        }
    }
    if dead.is_empty() && read_ids.is_empty() && tickets.is_empty() { return; }
    let saved = tokio::task::spawn_blocking(move || update(&path, |registry| {
        registry.tokens.retain(|e| !dead.contains(&e.token));
        registry.pending.retain(|p| !read_ids.contains(&p.ticket_id) && !dead.contains(&p.token));
        registry.pending.extend(tickets);
        let excess = registry.pending.len().saturating_sub(MAX_PENDING);
        registry.pending.drain(..excess);
    })).await;
    if !matches!(saved, Ok(Ok(()))) { report("push_expo_registry"); }
}
