//! Política do login próprio: o dono da sessão e a permissão vêm do cadastro local.
const GLOBAL: &[(&str, &str)] = &[
    ("GET", "/api/sessions"),
    ("GET", "/api/sessions/events"),
    ("GET", "/api/model-options"),
    ("GET", "/api/engines"),
    ("GET", "/api/harness/codex/opcoes"),
    ("POST", "/api/ditado/relimpar"),
    ("POST", "/api/pensamento/pt"),
    ("POST", "/api/tts"),
    ("POST", "/api/tts/narrar"),
    ("POST", "/api/sessions"),
    ("GET", "/api/sessions/creation-progress"),
    ("GET", "/api/fs/roots"),
    ("GET", "/api/fs/scan"),
    ("GET", "/api/fs/branches"),
    ("GET", "/api/providers"),
    ("GET", "/api/claude-configs"),
    ("GET", "/api/codex-contas"),
    ("GET", "/api/cotas"),
    ("GET", "/api/me"),
    ("POST", "/api/diag"),
];
const BLOCKED: &[&str] = &[
    "pair",
    "pair-remote",
    "unpair-remote",
    "group-message",
    "then",
    "orq",
    "bastao",
    "open-terminal",
    "open-editor",
    "nav",
    "share",
    "pair-invite",
    "pair-accept",
    "pair-access",
    "nav-remoto",
    "navegador",
];
pub fn allowed(method: &str, path: &str, visible: impl Fn(&str) -> bool) -> bool {
    if GLOBAL.contains(&(method, path)) || (method == "GET" && path.starts_with("/api/tts/audio/"))
    {
        return true;
    }
    let Some(tail) = path.strip_prefix("/api/sessions/") else {
        return false;
    };
    let (name, rest) = tail.split_once('/').unwrap_or((tail, ""));
    if let Some(owner) = name.strip_prefix("term-").filter(|_| rest == "term") {
        return visible(owner);
    }
    if BLOCKED.contains(&rest.split('/').next().unwrap_or("")) {
        return false;
    }
    visible(name)
}

use super::store::Guest;
use crate::{
    routes::AppState,
    share::{
        caller::{Caller, GuestGrant, SessionPath},
        life::Lives,
        store::Kind,
    },
};
use axum::{
    body::Body,
    extract::{ConnectInfo, Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use futures_util::StreamExt;
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

#[derive(Clone)]
pub struct Login {
    pub guest: Guest,
    pub token: String,
}
#[derive(Clone)]
pub struct Watcher {
    st: Arc<AppState>,
    token: String,
    method: String,
    path: String,
}
impl Watcher {
    pub async fn ended(watcher: Option<&Self>) {
        match watcher {
            Some(watcher) => watcher.until_ended().await,
            None => std::future::pending().await,
        }
    }
    pub async fn until_ended(&self) {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let service = self.st.login_guests.clone();
            let token = self.token.clone();
            let guest = tokio::task::spawn_blocking(move || service.store.lookup(token.as_bytes()))
                .await
                .ok()
                .flatten();
            let Some(guest) = guest else { return };
            if !check(&self.st, &guest, &self.method, &self.path).await {
                return;
            }
        }
    }
}
async fn session_life(st: &AppState, name: &str) -> Option<String> {
    if let Some(share) = &st.share {
        return share.lives.life(name, false).await;
    }
    st.login_guests
        .lives
        .get_or_init(|| {
            Arc::new(Lives::new(
                Arc::new(crate::share::life::MuxProbe::new(
                    st.list.env().capture_program.clone(),
                    st.accounts.env.home.clone(),
                )),
                Arc::default(),
            ))
        })
        .life(name, false)
        .await
}
async fn check(st: &AppState, guest: &Guest, method: &str, path: &str) -> bool {
    if !allowed(method, path, |_| true) {
        return false;
    }
    let session = std::cell::RefCell::new(None);
    let global = allowed(method, path, |name| {
        *session.borrow_mut() = Some(name.to_owned());
        false
    });
    if global {
        return true;
    }
    let Some(name) = session.into_inner() else {
        return false;
    };
    let life = session_life(st, &name).await;
    let service = st.login_guests.clone();
    let guest = guest.clone();
    tokio::task::spawn_blocking(move || service.store.visible(Some(&guest), &name, life.as_deref()))
        .await
        .unwrap_or(false)
}
fn deny(headers: &axum::http::HeaderMap) -> Response {
    let mut response=(StatusCode::FORBIDDEN,[(header::CONTENT_TYPE,"application/json")],serde_json::json!({"detail":{"code":"erro_fora_do_convidado","params":{},"msg":"fora do acesso do convidado"}}).to_string()).into_response();
    crate::routes::cors(headers, response.headers_mut());
    response
}
pub async fn guard(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = percent_encoding::percent_decode_str(request.uri().path())
        .decode_utf8_lossy()
        .into_owned();
    if !path.starts_with("/api/")
        || path.starts_with("/api/sync/")
        || crate::routes::on_guest_port(&request)
    {
        return next.run(request).await;
    }
    if let Some(response) = crate::share::gate::preflight(&request) {
        return response;
    }
    let (forward, owner) = match crate::routes::gate(&st, peer, &request).await { Ok(value)=>value, Err(error)=>return crate::auth::unavailable(error) };
    if owner {
        return next.run(request).await;
    }
    let Some(token) = crate::auth::presented_token(
        request.headers(),
        request.uri().query(),
        request.method(),
        forward.https,
    ) else {
        return next.run(request).await;
    };
    let service = st.login_guests.clone();
    let lookup = token.clone();
    let guest = tokio::task::spawn_blocking(move || service.store.lookup(&lookup))
        .await
        .ok()
        .flatten();
    let Some(guest) = guest else {
        return next.run(request).await;
    };
    let method = request.method().to_string();
    if !check(&st, &guest, &method, &path).await {
        return deny(request.headers());
    }
    let token = String::from_utf8_lossy(&token).into_owned();
    let watcher = Watcher {
        st: st.clone(),
        token: token.clone(),
        method,
        path: path.clone(),
    };
    let mut sessions = HashMap::new();
    if let Some(session) = SessionPath::parse(request.uri().path()) {
        sessions.insert(session.session.clone(), Kind::Share);
        request.extensions_mut().insert(session);
    }
    request
        .extensions_mut()
        .insert(Caller::Guest(Arc::new(GuestGrant {
            token: token.clone(),
            token_hash: super::store::hash(token.as_bytes()),
            sessions,
            pair_roots: vec![],
            invite: true,
        })));
    request.extensions_mut().insert(Login { guest, token });
    request.extensions_mut().insert(watcher.clone());
    let response = next.run(request).await;
    if path.ends_with("/events") {
        let (parts, body) = response.into_parts();
        let stream = body
            .into_data_stream()
            .take_until(async move { watcher.until_ended().await });
        Response::from_parts(parts, Body::from_stream(stream))
    } else {
        response
    }
}

/// Só o login próprio usa credencial ambiente; convites e token explícito do dono têm outro porteiro.
pub(crate) async fn terminal_origin_allowed(st: &Arc<AppState>, headers: &axum::http::HeaderMap) -> bool {
    let Some(origin) = headers.get("origin") else { return true };
    let Ok(origin) = origin.to_str() else { return false };
    let origin = origin.to_owned();
    let host = headers.get("host").and_then(|v|v.to_str().ok()).map(str::to_owned);
    let home = st.accounts.env.home.clone();
    let runtime = st.accounts.env.claude_base.join("runtime-config.json");
    tokio::task::spawn_blocking(move || {
        let backend = crate::session_open::cli::backend_dir();
        let public = crate::machine_peers::config_value(&home, backend.as_deref(), "CP_PUBLIC_URL").ok().flatten().unwrap_or_default();
        let configured = crate::machine_peers::config_value(&home, backend.as_deref(), "CP_TERM_ORIGINS").ok().flatten().unwrap_or_default();
        let runtime = super::store::read(&runtime).ok().flatten();
        let saved = runtime.as_ref().and_then(|v|v["term_origins"].as_str()).unwrap_or_default();
        let extras:Vec<String> = configured.split(',').chain(saved.split(',')).map(str::trim).filter(|v|!v.is_empty()).map(str::to_owned).collect();
        crate::machine_peers::origin_allowed(&home, backend.as_deref(), &origin, host.as_deref(), &public, &extras)
    }).await.unwrap_or(false)
}
