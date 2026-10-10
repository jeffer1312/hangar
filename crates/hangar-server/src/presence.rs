//! "No PC" / "Fora": com o dono no PC o push do celular fica retido. A escolha (`mode`) é do dono e
//! sobrevive a reinício; estar no PC de fato exige o sinal de vida do app de desktop com a janela à vista.
use crate::routes::{AppState, gate, pass};
use crate::session_write::{detail_body, json_response};
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Dois sinais de 30 s perdidos: queda, PC desligado ou app fechado sem aviso viram "Fora".
const ALIVE_FOR: Duration = Duration::from_secs(75);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode { Pc, Away }

impl Mode {
    fn parse(text: &str) -> Option<Self> {
        match text { "pc" => Some(Self::Pc), "away" => Some(Self::Away), _ => None }
    }
    fn as_str(self) -> &'static str { if self == Self::Pc { "pc" } else { "away" } }
}

struct Presence { mode: Option<Mode>, seen: Option<Instant> }

static STATE: Mutex<Presence> = Mutex::new(Presence { mode: None, seen: None });

fn file(state: &AppState) -> PathBuf { state.accounts.env.home.join(".hangar").join("presenca.json") }

/// Sem arquivo ou arquivo torto: "No PC", o padrão que só retém push com o app aberto e à vista.
fn stored(path: &Path) -> Mode {
    std::fs::read(path).ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|v| v["mode"].as_str().and_then(Mode::parse))
        .unwrap_or(Mode::Pc)
}

fn view(state: &AppState, now: Instant) -> Value {
    let mut p = STATE.lock().unwrap_or_else(|e| e.into_inner());
    let mode = *p.mode.get_or_insert_with(|| stored(&file(state)));
    let alive = p.seen.is_some_and(|at| now.saturating_duration_since(at) < ALIVE_FOR);
    json!({"mode": mode.as_str(), "desktop_alive": alive, "present": mode == Mode::Pc && alive})
}

/// O que o push do Python consulta: só `true` retém o envio.
pub fn present(state: &AppState) -> bool { view(state, Instant::now())["present"] == true }

/// `GET /api/presence`, `POST /api/presence {"mode"}` e `POST /api/presence/heartbeat {"leaving"?}`; só o dono.
pub async fn route(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, request: Request) -> Response {
    let (forward, owner) = gate(&state, peer, &request);
    if !owner {
        return pass(&state, request, &forward).await;
    }
    let heartbeat = request.uri().path().ends_with("/heartbeat");
    if request.method() == Method::GET && !heartbeat {
        return json_response(StatusCode::OK, view(&state, Instant::now()));
    }
    if request.method() != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let body = to_bytes(request.into_body(), 4 * 1024).await.ok()
        .and_then(|bytes| if bytes.is_empty() { Some(json!({})) } else { serde_json::from_slice::<Value>(&bytes).ok() });
    let Some(body) = body.filter(Value::is_object) else { return invalid() };
    if heartbeat {
        let mut p = STATE.lock().unwrap_or_else(|e| e.into_inner());
        // A janela saiu de vista ou o app fechou: o celular volta a receber na hora, sem esperar o prazo.
        p.seen = if body["leaving"] == true { None } else { Some(Instant::now()) };
        drop(p);
        return json_response(StatusCode::OK, view(&state, Instant::now()));
    }
    let Some(mode) = body["mode"].as_str().and_then(Mode::parse) else { return invalid() };
    let path = file(&state);
    let bytes = json!({"mode": mode.as_str()}).to_string();
    let written = path.parent().map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| crate::runtime::queue::atomic_write(&path, bytes.as_bytes()));
    if written.is_err() {
        return json_response(StatusCode::INTERNAL_SERVER_ERROR,
            detail_body("presence_write_failed", "não consegui gravar o status de presença", json!({})));
    }
    STATE.lock().unwrap_or_else(|e| e.into_inner()).mode = Some(mode);
    json_response(StatusCode::OK, view(&state, Instant::now()))
}

fn invalid() -> Response {
    json_response(StatusCode::BAD_REQUEST, detail_body("presence_invalid", "status de presença inválido: use pc ou away", json!({})))
}

/// Ponte privada: o push do Python pergunta antes de cada envio.
pub async fn private(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<std::net::SocketAddr>, request: Request) -> Response {
    if !crate::workspace_routes::private_ok(&state, peer, request.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    json_response(StatusCode::OK, json!({"present": present(&state)}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stored_mode_defaults_to_pc_and_reads_away() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("presenca.json");
        assert_eq!(stored(&path), Mode::Pc);
        std::fs::write(&path, r#"{"mode":"away"}"#).unwrap();
        assert_eq!(stored(&path), Mode::Away);
        std::fs::write(&path, "torto").unwrap();
        assert_eq!(stored(&path), Mode::Pc);
    }

    #[test]
    fn only_known_modes_parse() {
        assert_eq!(Mode::parse("pc"), Some(Mode::Pc));
        assert_eq!(Mode::parse("away"), Some(Mode::Away));
        assert_eq!(Mode::parse("PC"), None);
    }
}
