//! Painel do dono: `/panel` mostra a entrada ou o painel conforme a sessão; `/panel/data` é o resumo em JSON.
//! Contas por convite (`accounts`); sessão num cookie `__Host-` que o JavaScript não lê.
use std::time::Duration;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use serde::Deserialize;
use serde_json::json;
use sqlx::PgPool;

use crate::{accounts, store};

const DASHBOARD: &str = include_str!("panel/dashboard.html");
const LOGIN: &str = include_str!("panel/login.html");
const JOIN: &str = include_str!("panel/join.html");
const STYLE: &str = include_str!("panel/panel.css");
const COOKIE: &str = "__Host-panel";
const DEFAULT_DAYS: i32 = 30;
/// Espera de toda entrada recusada: tentar senha em série fica lento sem atrasar quem acerta.
const REFUSAL_DELAY: Duration = Duration::from_millis(400);

#[derive(Deserialize)]
struct Period {
    days: Option<i32>,
}

#[derive(Deserialize)]
struct Login {
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct Join {
    invite: String,
    email: String,
    password: String,
}

pub fn router(pool: PgPool) -> Router {
    Router::new()
        .route("/panel", get(home))
        .route("/panel/join", get(|| async { page(JOIN) }))
        .route("/panel/assets/panel.css", get(|| async { ([(header::CONTENT_TYPE, "text/css; charset=utf-8")], STYLE) }))
        .route("/panel/data", get(data))
        .route("/panel/api/me", get(me))
        .route("/panel/api/login", post(login))
        .route("/panel/api/logout", post(logout))
        .route("/panel/api/invites", post(invite))
        .route("/panel/api/join", post(join))
        .with_state(pool)
}

fn page(html: &'static str) -> Response {
    // A página não roda dentro de outro site (clickjacking) e não manda o caminho a links de fora.
    let mut response = Html(html).into_response();
    let headers = response.headers_mut();
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn error(status: StatusCode, code: &str) -> Response { (status, axum::Json(json!({"error": code}))).into_response() }

fn cookie_token(headers: &HeaderMap) -> Option<String> {
    headers.get_all(header::COOKIE).iter().filter_map(|v| v.to_str().ok()).flat_map(|v| v.split(';')).find_map(|pair| {
        let (name, value) = pair.trim().split_once('=')?;
        (name == COOKIE && value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())).then(|| value.to_owned())
    })
}

fn with_session(mut response: Response, token: &str) -> Response {
    let max_age = accounts::SESSION_DAYS * 24 * 60 * 60;
    let value = format!("{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age={max_age}");
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&value).expect("cookie em ASCII"));
    response
}

fn without_session(mut response: Response) -> Response {
    let value = format!("{COOKIE}=; Path=/; HttpOnly; Secure; SameSite=Strict; Max-Age=0");
    response.headers_mut().insert(header::SET_COOKIE, HeaderValue::from_str(&value).expect("cookie em ASCII"));
    response
}

/// Pedido que muda estado só vale vindo desta mesma origem (o `SameSite=Strict` já barra o cookie; isto barra o resto).
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else { return true };
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok()).unwrap_or_default();
    origin.split_once("://").is_some_and(|(_, rest)| rest == host)
}

async fn user(pool: &PgPool, headers: &HeaderMap) -> Result<Option<(i64, String)>, Response> {
    let Some(token) = cookie_token(headers) else { return Ok(None) };
    accounts::session_user(pool, &token).await.map_err(|e| unavailable("session", &e))
}

fn unavailable(what: &str, cause: &sqlx::Error) -> Response {
    eprintln!("telemetry-ingest: panel {what}: {cause}");
    error(StatusCode::SERVICE_UNAVAILABLE, "store")
}

async fn home(State(pool): State<PgPool>, headers: HeaderMap) -> Response {
    match user(&pool, &headers).await {
        Ok(Some(_)) => page(DASHBOARD),
        Ok(None) => page(LOGIN),
        Err(response) => response,
    }
}

async fn me(State(pool): State<PgPool>, headers: HeaderMap) -> Response {
    match user(&pool, &headers).await {
        Ok(Some((_, email))) => axum::Json(json!({"email": email})).into_response(),
        Ok(None) => error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => response,
    }
}

async fn data(State(pool): State<PgPool>, headers: HeaderMap, Query(period): Query<Period>) -> Response {
    match user(&pool, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => return response,
    }
    let days = period.days.unwrap_or(DEFAULT_DAYS).clamp(1, 400);
    match store::summary(&pool, days).await {
        Ok(summary) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(summary)).into_response(),
        Err(e) => unavailable("summary", &e),
    }
}

async fn login(State(pool): State<PgPool>, headers: HeaderMap, body: axum::Json<Login>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "origin");
    }
    let Some(email) = accounts::normalize_email(&body.email) else {
        tokio::time::sleep(REFUSAL_DELAY).await;
        return error(StatusCode::UNAUTHORIZED, "login");
    };
    match accounts::login(&pool, &email, &body.password).await {
        Ok(Some(id)) => match accounts::create_session(&pool, id).await {
            Ok(token) => with_session(StatusCode::NO_CONTENT.into_response(), &token),
            Err(e) => unavailable("session", &e),
        },
        Ok(None) => {
            tokio::time::sleep(REFUSAL_DELAY).await;
            error(StatusCode::UNAUTHORIZED, "login")
        }
        Err(e) => unavailable("login", &e),
    }
}

async fn logout(State(pool): State<PgPool>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "origin");
    }
    if let Some(token) = cookie_token(&headers)
        && let Err(e) = accounts::end_session(&pool, &token).await
    {
        return unavailable("logout", &e);
    }
    without_session(StatusCode::NO_CONTENT.into_response())
}

async fn invite(State(pool): State<PgPool>, headers: HeaderMap) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "origin");
    }
    let id = match user(&pool, &headers).await {
        Ok(Some((id, _))) => id,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "session"),
        Err(response) => return response,
    };
    match accounts::create_invite(&pool, id).await {
        Ok(token) => axum::Json(json!({"path": format!("/panel/join#{token}"), "days": accounts::INVITE_DAYS})).into_response(),
        Err(e) => unavailable("invite", &e),
    }
}

async fn join(State(pool): State<PgPool>, headers: HeaderMap, body: axum::Json<Join>) -> Response {
    if !same_origin(&headers) {
        return error(StatusCode::FORBIDDEN, "origin");
    }
    let Some(email) = accounts::normalize_email(&body.email) else { return error(StatusCode::BAD_REQUEST, "email") };
    if body.password.chars().count() < accounts::MIN_PASSWORD {
        return error(StatusCode::BAD_REQUEST, "password");
    }
    let hash = accounts::hash_off_thread(body.password.clone()).await;
    match accounts::join(&pool, body.invite.trim(), &email, &hash).await {
        Ok(accounts::Join::Created(id)) => match accounts::create_session(&pool, id).await {
            Ok(token) => with_session(StatusCode::NO_CONTENT.into_response(), &token),
            Err(e) => unavailable("session", &e),
        },
        Ok(accounts::Join::BadInvite) => {
            tokio::time::sleep(REFUSAL_DELAY).await;
            error(StatusCode::BAD_REQUEST, "invite")
        }
        Ok(accounts::Join::EmailTaken) => error(StatusCode::CONFLICT, "email_taken"),
        Err(e) => unavailable("join", &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (k, v) in pairs {
            map.append(axum::http::HeaderName::from_bytes(k.as_bytes()).unwrap(), HeaderValue::from_str(v).unwrap());
        }
        map
    }

    #[test]
    fn session_cookie_is_read_only_in_its_own_shape() {
        let token = "a".repeat(64);
        assert_eq!(cookie_token(&headers(&[("cookie", &format!("x=1; {COOKIE}={token}"))])), Some(token.clone()));
        assert_eq!(cookie_token(&headers(&[("cookie", &format!("{COOKIE}=curto"))])), None);
        assert_eq!(cookie_token(&headers(&[("cookie", &format!("outro={token}"))])), None);
        assert_eq!(cookie_token(&headers(&[])), None);
    }

    #[test]
    fn writes_only_from_the_same_origin() {
        assert!(same_origin(&headers(&[("host", "telemetry.hangar.dev.br")])));
        assert!(same_origin(&headers(&[("host", "telemetry.hangar.dev.br"), ("origin", "https://telemetry.hangar.dev.br")])));
        assert!(!same_origin(&headers(&[("host", "telemetry.hangar.dev.br"), ("origin", "https://evil.example")])));
    }
}
