//! `/api/push/vapid|subscribe|settings|mute|quiet-hours` do dono, com os corpos do FastAPI.
use super::{store, webpush};
use crate::routes::{gate, pass, AppState};
use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::Response};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{net::SocketAddr, sync::Arc};

const BODY_LIMIT: usize = 64 * 1024;

pub fn matches(method: &Method, path: &str) -> bool {
    (*method == Method::GET && matches!(path, "/api/push/vapid" | "/api/push/settings"))
        || (*method == Method::POST && matches!(path, "/api/push/subscribe" | "/api/push/mute" | "/api/push/quiet-hours"))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Subscribe { subscription: serde_json::Map<String, Value>, label: String, #[serde(rename = "serverId")] server_id: String,
    #[serde(default = "default_locale")] locale: String }

fn default_locale() -> String { "en".into() }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Mute { session: String, muted: bool }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuietHours { #[serde(default)] start: Option<String>, #[serde(default)] end: Option<String> }

fn reply(status: StatusCode, body: Value) -> Response { crate::session_write::json_response(status, body) }

fn invalid() -> Response { reply(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": [{"type": "value_error", "msg": "corpo inválido"}]})) }

fn unwritable() -> Response { reply(StatusCode::SERVICE_UNAVAILABLE, json!({"detail": "push_store_unwritable"})) }

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, owner) = gate(&st, peer, &req);
    if !owner { return pass(&st, req, &fwd).await; }
    let (method, path) = (req.method().clone(), req.uri().path().to_owned());
    let dir = store::dir(&st.accounts.env.claude_base);
    if method == Method::GET {
        return if path.ends_with("/vapid") {
            reply(StatusCode::OK, json!({"key": webpush::public_key(&super::server_env())}))
        } else {
            reply(StatusCode::OK, store::settings(&dir))
        };
    }
    let Ok(raw) = to_bytes(req.into_body(), BODY_LIMIT).await else { return invalid() };
    match path.as_str() {
        "/api/push/subscribe" => {
            let Some(b) = serde_json::from_slice::<Subscribe>(&raw).ok().filter(|b| !b.label.is_empty() && !b.server_id.is_empty()) else { return invalid() };
            match tokio::task::spawn_blocking(move || store::add(&dir, Value::Object(b.subscription), &b.label, &b.server_id, &b.locale)).await {
                Ok(Ok(())) => reply(StatusCode::OK, json!({"ok": true})),
                Ok(Err(msg)) if msg == "subscription sem endpoint" => reply(StatusCode::BAD_REQUEST, json!({"detail": msg})),
                _ => unwritable(),
            }
        }
        "/api/push/mute" => {
            let Some(b) = serde_json::from_slice::<Mute>(&raw).ok().filter(|b| !b.session.is_empty()) else { return invalid() };
            match tokio::task::spawn_blocking(move || store::set_muted(&dir, &b.session, b.muted)).await {
                Ok(Ok(())) => reply(StatusCode::OK, json!({"ok": true})),
                _ => unwritable(),
            }
        }
        _ => {
            let Ok(b) = serde_json::from_slice::<QuietHours>(&raw) else { return invalid() };
            match tokio::task::spawn_blocking(move || store::set_quiet_hours(&dir, b.start.as_deref(), b.end.as_deref())).await {
                Ok(Ok(())) => reply(StatusCode::OK, json!({"ok": true})),
                Ok(Err(msg)) if msg != "push_store_unwritable" => reply(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail": msg})),
                _ => unwritable(),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{accounts::{environment::AccountEnvironment, AccountService}, auth::TrustedHosts, config::Config, routes::AppState};
    use serde_json::{json, Value};
    use std::sync::Arc;

    #[tokio::test]
    async fn owner_routes_answer_with_the_fastapi_bodies_and_write_where_the_python_reads() {
        let home = tempfile::tempdir().unwrap();
        let mut state = AppState::new(Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:1".parse().unwrap(),
            internal_secret: "s".into(), auth_token: "owner".into(), log_path: None, trusted: TrustedHosts::parse("127.0.0.1") });
        state.accounts = AccountService::new(AccountEnvironment::from_map([
            ("HOME".into(), home.path().to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.path().to_string_lossy().into_owned())].into()));
        let claude = state.accounts.env.claude_base.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = crate::routes::router(Arc::new(state)).into_make_service_with_connect_info::<std::net::SocketAddr>();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = reqwest::Client::new();
        let post = |path: &str, body: Value| http.post(format!("{base}{path}")).bearer_auth("owner").json(&body).send();
        let answer = |r: reqwest::Response| async move { (r.status().as_u16(), r.json::<Value>().await.unwrap()) };

        let sub = json!({"subscription": {"endpoint": "https://push.example/e1", "keys": {"p256dh": "x", "auth": "y"}}, "label": "Casa", "serverId": "s1"});
        assert_eq!(answer(post("/api/push/subscribe", sub).await.unwrap()).await, (200, json!({"ok": true})));
        let saved: Value = serde_json::from_slice(&std::fs::read(claude.join(".hangar-push/subs.json")).unwrap()).unwrap();
        assert_eq!(saved, json!([{"subscription": {"endpoint": "https://push.example/e1", "keys": {"p256dh": "x", "auth": "y"}},
            "label": "Casa", "serverId": "s1", "locale": "en"}]));
        let (status, _) = answer(post("/api/push/subscribe", json!({"subscription": {}, "label": "Casa", "serverId": "s1"})).await.unwrap()).await;
        assert_eq!(status, 400);
        let (status, _) = answer(post("/api/push/mute", json!({"session": "a", "muted": true, "extra": 1})).await.unwrap()).await;
        assert_eq!(status, 422, "campo desconhecido é recusado, como o _StrictBody");
        assert_eq!(answer(post("/api/push/mute", json!({"session": "a", "muted": true})).await.unwrap()).await, (200, json!({"ok": true})));
        assert_eq!(answer(post("/api/push/quiet-hours", json!({"start": "22:00", "end": "25:00"})).await.unwrap()).await,
            (422, json!({"detail": "horario invalido (use HH:MM)"})));
        assert_eq!(answer(post("/api/push/quiet-hours", json!({"start": "22:00", "end": "07:00"})).await.unwrap()).await, (200, json!({"ok": true})));
        let settings = http.get(format!("{base}/api/push/settings")).bearer_auth("owner").send().await.unwrap();
        assert_eq!(answer(settings).await, (200, json!({"muted": ["a"], "quiet_hours": {"start": "22:00", "end": "07:00"}})));
        let vapid = http.get(format!("{base}/api/push/vapid")).bearer_auth("owner").send().await.unwrap();
        assert_eq!(vapid.status().as_u16(), 200);
        assert!(vapid.json::<Value>().await.unwrap()["key"].is_string());
        server.abort();
    }
}
