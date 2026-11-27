//! Painel do dono: `/panel` (a página) e `/panel/data` (o resumo em JSON), atrás da senha `PANEL_PASSWORD`.
//! Sem a senha no ambiente o painel não existe (404): nada fica aberto por esquecimento.
use axum::Router;
use axum::extract::{Query, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::get;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use sqlx::PgPool;

use crate::store;

const PAGE: &str = include_str!("panel.html");
const DEFAULT_DAYS: i32 = 30;

#[derive(Clone)]
struct Panel {
    pool: PgPool,
    password: String,
}

#[derive(serde::Deserialize)]
struct Period {
    days: Option<i32>,
}

pub fn router(pool: PgPool, password: Option<String>) -> Router {
    let Some(password) = password.filter(|p| !p.is_empty()) else { return Router::new() };
    let panel = Panel { pool, password };
    Router::new()
        .route("/panel", get(|| async { Html(PAGE) }))
        .route("/panel/data", get(data))
        .layer(middleware::from_fn_with_state(panel.clone(), guard))
        .with_state(panel)
}

async fn guard(State(panel): State<Panel>, request: Request, next: Next) -> Response {
    if allowed(request.headers().get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()), &panel.password) {
        return next.run(request).await;
    }
    (StatusCode::UNAUTHORIZED, [(header::WWW_AUTHENTICATE, r#"Basic realm="telemetria", charset="UTF-8""#)]).into_response()
}

/// Basic com qualquer usuário: só a senha conta.
fn allowed(authorization: Option<&str>, password: &str) -> bool {
    let Some((scheme, encoded)) = authorization.and_then(|v| v.split_once(' ')) else { return false };
    if !scheme.eq_ignore_ascii_case("basic") {
        return false;
    }
    let Ok(decoded) = STANDARD.decode(encoded.trim()) else { return false };
    let Some(given) = decoded.splitn(2, |b| *b == b':').nth(1) else { return false };
    same(given, password.as_bytes())
}

/// Comparação sem saída antecipada no conteúdo, para o tempo não dizer quantos bytes acertaram.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

async fn data(State(panel): State<Panel>, Query(period): Query<Period>) -> Response {
    let days = period.days.unwrap_or(DEFAULT_DAYS).clamp(1, 400);
    match store::summary(&panel.pool, days).await {
        Ok(summary) => ([(header::CACHE_CONTROL, "no-store")], axum::Json(summary)).into_response(),
        Err(error) => {
            eprintln!("telemetry-ingest: summary: {error}");
            (StatusCode::SERVICE_UNAVAILABLE, "store").into_response()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn basic(user_pass: &str) -> String { format!("Basic {}", STANDARD.encode(user_pass)) }

    #[test]
    fn only_the_right_password_passes() {
        assert!(allowed(Some(&basic("qualquer:segredo")), "segredo"));
        assert!(allowed(Some(&basic(":segredo")), "segredo"));
        assert!(!allowed(Some(&basic("segredo:errada")), "segredo"));
        assert!(!allowed(Some(&basic("sem-dois-pontos")), "segredo"));
        assert!(allowed(Some(&basic("x:segredo").replace("Basic", "basic")), "segredo"), "o esquema não tem caixa");
        assert!(!allowed(Some("Bearer segredo"), "segredo"));
        assert!(!allowed(Some("Basic !!!"), "segredo"));
        assert!(!allowed(None, "segredo"));
    }
}
