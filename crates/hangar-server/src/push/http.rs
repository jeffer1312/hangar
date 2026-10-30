//! Portas de notificações: contrato FastAPI completo e persistência nativa.
use super::store;
use crate::{
    orchestration::{body, codec},
    routes::AppState,
};
use axum::{
    body::{Body, to_bytes},
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::json;
use crate::http_cors::{internal_error, json_guarded};
use std::{net::SocketAddr, sync::Arc};
const ROUTES: &[(&str, &str)] = &[
    ("/api/push/vapid", "GET"),
    ("/api/push/subscribe", "POST"),
    ("/api/push/settings", "GET"),
    ("/api/push/mute", "POST"),
    ("/api/push/quiet-hours", "POST"),
];
pub fn owns_path(raw: &str) -> bool {
    let path = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
    (path == "/api/push" || path.starts_with("/api/push/")) && !path.starts_with("/api/push/expo")
}
pub fn matches(method: &Method, path: &str) -> bool {
    ROUTES
        .iter()
        .any(|(p, m)| *p == path && *m == method.as_str())
}
pub async fn route(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
) -> Response {
    if let Some(response) = crate::http_cors::preflight(req.method(), req.headers()) {
        return response;
    }
    let headers = req.headers().clone();
    let method = req.method().clone();
    let mut response = admit(&st, peer, req).await;
    crate::http_cors::apply(&headers, &mut response);
    crate::http_route_response::suppress_head(&method, &mut response);
    response
}
async fn admit(st: &Arc<AppState>, peer: SocketAddr, mut req: Request) -> Response {
    if let Some(response) = crate::http_body_limit::precheck(req.headers()) {
        return response;
    }
    let path = percent_encoding::percent_decode_str(req.uri().path())
        .decode_utf8_lossy()
        .into_owned();
    let decision = crate::http_contract::resolve(ROUTES, req.method().as_str(), &path);
    let (_, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    if let Some(response) = crate::http_route_response::response(
        &decision,
        req.method(),
        req.uri(),
        req.headers(),
        https,
        &st.cfg.listen.to_string(),
    ) {
        return response;
    }
    let schema = match path.as_str() {
        "/api/push/subscribe" => Some("PushSubscribeBody"),
        "/api/push/mute" => Some("PushMuteBody"),
        "/api/push/quiet-hours" => Some("PushQuietHoursBody"),
        _ => None,
    };
    let is_json = crate::http_body_input::is_json(req.headers());
    if schema.is_some() && is_json {
        let (parts, body) = req.into_parts();
        let raw = match to_bytes(body, crate::http_body_limit::MAX_BYTES).await {
            Ok(raw) => raw,
            Err(_) => {
                return json_guarded(
                    StatusCode::BAD_REQUEST,
                    json!({"detail":"There was an error parsing the body"}),
                );
            }
        };
        match body::parse(&raw) {
            Ok(value) => codec::discard(value),
            Err((status, value)) => return json_guarded(status, value),
        }
        req = Request::from_parts(parts, Body::from(raw));
    }
    if let Err(response) = crate::routes::owner_gate(st, peer, &req).await {
        return response;
    }
    let config = match super::config::Frozen::from_state(st) {
        Ok(config) => config,
        Err(code) => {
            return json_guarded(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"detail":{"code":code}}),
            );
        }
    };
    let dir = config.dir;
    if path == "/api/push/vapid" {
        return json_guarded(StatusCode::OK, json!({"key":config.env["CP_VAPID_PUBLIC"]}));
    }
    if path == "/api/push/settings" {
        return match tokio::task::spawn_blocking(move || store::settings(&dir)).await {
            Ok(Ok(value)) => json_guarded(StatusCode::OK, value),
            _ => internal_error(),
        };
    }
    let raw = match to_bytes(req.into_body(), crate::http_body_limit::MAX_BYTES).await {
        Ok(raw) => raw,
        Err(_) => {
            return json_guarded(
                StatusCode::BAD_REQUEST,
                json!({"detail":"There was an error parsing the body"}),
            );
        }
    };
    let value = if !is_json && !raw.is_empty() {
        match std::str::from_utf8(&raw) {
            Ok(text) => json!(text),
            Err(_) => return internal_error(),
        }
    } else {
        match body::parse(&raw) {
            Ok(value) => value,
            Err((status, value)) => return json_guarded(status, value),
        }
    };
    let checked = crate::login_guests::schema::validate(schema.unwrap(), &value);
    codec::discard(value);
    let normalized = match checked {
        Ok(value) => value,
        Err(failure) => return json_guarded(StatusCode::from_u16(failure.status).unwrap(), failure.body),
    };
    if path == "/api/push/subscribe"
        && let Err(detail) = store::validate_subscription(&normalized["subscription"])
    {
        codec::discard(normalized);
        return json_guarded(StatusCode::BAD_REQUEST, json!({"detail":detail}));
    }
    let operation = path.clone();
    // A espera da trava não pode ocupar o pool usado pelo envio para ler e podar.
    let guard = store::delivery_lock().await;
    let result = tokio::task::spawn_blocking(move || {
        let result = match operation.as_str() {
            "/api/push/subscribe" => store::add_locked(
                &guard,
                &dir,
                codec::clone_value(&normalized["subscription"]),
                normalized["label"].as_str().unwrap(),
                normalized["serverId"].as_str().unwrap(),
                normalized["locale"].as_str().unwrap_or("en"),
            ),
            "/api/push/mute" => store::set_muted_locked(
                &guard,
                &dir,
                normalized["session"].as_str().unwrap(),
                normalized["muted"].as_bool().unwrap(),
            ),
            _ => store::set_quiet_hours_locked(
                &guard,
                &dir,
                normalized["start"].as_str(),
                normalized["end"].as_str(),
            ),
        };
        codec::discard(normalized);
        result
    })
    .await
    .unwrap_or(Err("push_store_unwritable"));
    match result {
        Ok(()) => json_guarded(StatusCode::OK, json!({"ok":true})),
        Err("push_settings_invalid") => {
            st.diag.report(
                "rust.push_settings_failed",
                "push",
                "push_settings_invalid",
                "preferências incompatíveis; arquivo preservado",
            );
            json_guarded(
                StatusCode::SERVICE_UNAVAILABLE,
                json!({"detail":{"code":"push_settings_invalid"}}),
            )
        }
        Err("push_store_encoding") if path == "/api/push/subscribe" => json_guarded(
            StatusCode::BAD_REQUEST,
            json!({"detail":"push_encoding_invalid"}),
        ),
        Err("subscription sem endpoint") => json_guarded(
            StatusCode::BAD_REQUEST,
            json!({"detail":"subscription sem endpoint"}),
        ),
        Err("horario invalido (use HH:MM)") => json_guarded(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"detail":"horario invalido (use HH:MM)"}),
        ),
        Err(_) => internal_error(),
    }
}
#[cfg(test)]
mod tests {
    use crate::{
        accounts::{AccountService, environment::AccountEnvironment},
        auth::TrustedHosts,
        config::Config,
        routes::AppState,
    };
    use serde_json::{Value, json};
    use std::sync::Arc;

    #[tokio::test]
    async fn owner_routes_answer_with_the_fastapi_bodies_and_write_where_the_python_reads() {
        let home = tempfile::tempdir().unwrap();
        let mut state = AppState::new(Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream: "127.0.0.1:1".parse().unwrap(),
            internal_secret: "s".into(),
            auth_token: "owner".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = AccountService::new(AccountEnvironment::from_map(
            [
                ("HOME".into(), home.path().to_string_lossy().into_owned()),
                (
                    "USERPROFILE".into(),
                    home.path().to_string_lossy().into_owned(),
                ),
            ]
            .into(),
        ));
        let claude = state.accounts.env.claude_base.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let app = crate::routes::router(Arc::new(state))
            .into_make_service_with_connect_info::<std::net::SocketAddr>();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = reqwest::Client::new();
        let post = |path: &str, body: Value| {
            http.post(format!("{base}{path}"))
                .bearer_auth("owner")
                .json(&body)
                .send()
        };
        let answer = |r: reqwest::Response| async move {
            (r.status().as_u16(), r.json::<Value>().await.unwrap())
        };

        let sub = json!({"subscription": {"endpoint": "https://push.example/e1", "keys": {"p256dh": "x", "auth": "y"}}, "label": "Casa", "serverId": "s1"});
        assert_eq!(
            answer(post("/api/push/subscribe", sub).await.unwrap()).await,
            (200, json!({"ok": true}))
        );
        let saved: Value =
            serde_json::from_slice(&std::fs::read(claude.join(".hangar-push/subs.json")).unwrap())
                .unwrap();
        assert_eq!(
            saved,
            json!([{"subscription": {"endpoint": "https://push.example/e1", "keys": {"p256dh": "x", "auth": "y"}},
            "label": "Casa", "serverId": "s1", "locale": "en"}])
        );
        let (status, _) = answer(
            post(
                "/api/push/subscribe",
                json!({"subscription": {}, "label": "Casa", "serverId": "s1"}),
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(status, 400);
        let (status, _) = answer(
            post(
                "/api/push/mute",
                json!({"session": "a", "muted": true, "extra": 1}),
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(
            status, 422,
            "campo desconhecido é recusado, como o _StrictBody"
        );
        assert_eq!(
            answer(
                post("/api/push/mute", json!({"session": "a", "muted": true}))
                    .await
                    .unwrap()
            )
            .await,
            (200, json!({"ok": true}))
        );
        assert_eq!(
            answer(
                post(
                    "/api/push/quiet-hours",
                    json!({"start": "22:00", "end": "25:00"})
                )
                .await
                .unwrap()
            )
            .await,
            (422, json!({"detail": "horario invalido (use HH:MM)"}))
        );
        assert_eq!(
            answer(
                post(
                    "/api/push/quiet-hours",
                    json!({"start": "22:00", "end": "07:00"})
                )
                .await
                .unwrap()
            )
            .await,
            (200, json!({"ok": true}))
        );
        let settings = http
            .get(format!("{base}/api/push/settings"))
            .bearer_auth("owner")
            .send()
            .await
            .unwrap();
        assert_eq!(
            answer(settings).await,
            (
                200,
                json!({"muted": ["a"], "quiet_hours": {"start": "22:00", "end": "07:00"}})
            )
        );
        let vapid = http
            .get(format!("{base}/api/push/vapid"))
            .bearer_auth("owner")
            .send()
            .await
            .unwrap();
        assert_eq!(vapid.status().as_u16(), 200);
        assert!(vapid.json::<Value>().await.unwrap()["key"].is_string());
        server.abort();
    }
}
