//! Ocultação automática da sessão parada (`~/.hangar/session-auto-hide.json`): nasce desligada,
//! e nunca fecha nem interrompe nada, só tira a linha da lista principal.
use std::io;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{blocking, home, refuse};
use crate::routes::{AppState, cors, gate};
use crate::session_write::json_response;

/// A rota aceita de 1 min (verificação manual) a 7 dias; a tela só oferece horas inteiras.
const MAX_MINUTES: u32 = 7 * 24 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AutoHide { pub enabled: bool, pub after_minutes: u32 }

impl Default for AutoHide {
    fn default() -> Self { Self { enabled: false, after_minutes: 180 } }
}

fn path(home: &Path) -> PathBuf { home.join(".hangar").join("session-auto-hide.json") }

/// Ausente = o padrão; ilegível é erro, para um "desligado" nunca aparecer no lugar do que foi gravado.
pub fn load(home: &Path) -> io::Result<AutoHide> {
    let file = path(home);
    // Lido a cada rodada da lista e quase sempre ausente: a espera do Windows fica para o arquivo que existe.
    if !file.exists() { return Ok(AutoHide::default()); }
    match crate::runtime::queue::read_replaced(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(AutoHide::default()),
        Err(error) => Err(error),
    }
}

pub fn save(home: &Path, value: &AutoHide) -> io::Result<()> {
    let file = path(home);
    std::fs::create_dir_all(file.parent().ok_or_else(|| io::Error::other("pasta ausente"))?)?;
    crate::runtime::queue::atomic_write(&file, &serde_json::to_vec(value)?)
}

/// `GET`/`PUT /api/session-auto-hide`.
pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let mut response = match gate(&st, peer, &req).await {
        Err(e) => crate::auth::unavailable(e),
        Ok((_, false)) => refuse(StatusCode::FORBIDDEN, "session_auto_hide_owner_only", "só o dono lê ou altera a ocultação automática"),
        Ok((_, true)) => serve(&st, req).await,
    };
    cors(&headers, response.headers_mut());
    response
}

fn store_failed(st: &AppState, error: &io::Error) -> Response {
    tracing::warn!(%error, "configuração da ocultação automática ilegível ou não gravada");
    st.diag.report("rust.session_visibility", "", "session_auto_hide_store_failed", "ocultacao automatica ilegivel ou nao gravada");
    refuse(StatusCode::INTERNAL_SERVER_ERROR, "session_auto_hide_store_failed", "não foi possível ler ou guardar a ocultação automática")
}

async fn serve(st: &AppState, req: Request) -> Response {
    let Some(home) = home(st) else {
        return store_failed(st, &io::Error::other("pastas das sessões não configuradas"));
    };
    if req.method() != Method::PUT {
        return match blocking(move || load(&home)).await {
            Ok(value) => json_response(StatusCode::OK, json!(value)),
            Err(error) => store_failed(st, &error),
        };
    }
    let parsed = to_bytes(req.into_body(), 4096).await.ok()
        .and_then(|raw| serde_json::from_slice::<AutoHide>(&raw).ok())
        .filter(|value| (1..=MAX_MINUTES).contains(&value.after_minutes));
    let Some(value) = parsed else {
        return refuse(StatusCode::UNPROCESSABLE_ENTITY, "session_auto_hide_invalid", "prazo fora de 1 minuto a 7 dias, ou campo desconhecido");
    };
    match blocking(move || save(&home, &value)).await {
        Ok(()) => {
            st.list.invalidate();
            json_response(StatusCode::OK, json!(value))
        }
        Err(error) => store_failed(st, &error),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::super::fixture::{call, state};
    use super::*;

    #[tokio::test]
    async fn absent_is_off_at_three_hours_and_a_put_is_read_back() {
        let root = tempfile::tempdir().unwrap();
        let st = state(root.path());
        let (status, body) = call(&st, route, Method::GET, "/api/session-auto-hide", "fixture-owner", "").await;
        assert_eq!((status, body), (StatusCode::OK, json!({"enabled": false, "after_minutes": 180})));
        let sent = r#"{"enabled": true, "after_minutes": 60}"#;
        let (status, body) = call(&st, route, Method::PUT, "/api/session-auto-hide", "fixture-owner", sent).await;
        assert_eq!((status, body), (StatusCode::OK, json!({"enabled": true, "after_minutes": 60})));
        let (_, body) = call(&st, route, Method::GET, "/api/session-auto-hide", "fixture-owner", "").await;
        assert_eq!(body, json!({"enabled": true, "after_minutes": 60}));
        assert_eq!(load(root.path()).unwrap(), AutoHide { enabled: true, after_minutes: 60 });
    }

    #[tokio::test]
    async fn out_of_range_unknown_field_and_guest_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let st = state(root.path());
        for body in [r#"{"enabled": true, "after_minutes": 0}"#, r#"{"enabled": true, "after_minutes": 10081}"#,
            r#"{"enabled": true, "after_minutes": 60, "extra": 1}"#, r#"{"enabled": true}"#] {
            let (status, got) = call(&st, route, Method::PUT, "/api/session-auto-hide", "fixture-owner", body).await;
            assert_eq!((status, got["detail"]["code"].as_str()), (StatusCode::UNPROCESSABLE_ENTITY, Some("session_auto_hide_invalid")), "{body}");
        }
        let (status, got) = call(&st, route, Method::GET, "/api/session-auto-hide", "errado", "").await;
        assert_eq!((status, got["detail"]["code"].as_str()), (StatusCode::FORBIDDEN, Some("session_auto_hide_owner_only")));
        assert!(!path(root.path()).exists(), "nada gravado");
    }

    #[test]
    fn a_corrupt_file_is_an_error() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".hangar")).unwrap();
        std::fs::write(path(root.path()), "{").unwrap();
        assert!(load(root.path()).is_err());
    }
}
