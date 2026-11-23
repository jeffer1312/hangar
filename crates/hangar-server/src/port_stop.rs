//! `POST /api/sessions/{name}/ports/{port}/stop`: para o servidor que escuta na porta da sessão, pelo
//! `hangar-port-stop`. Só em `public_routes` (dono e aparelho do Connect): a 8766 do convidado não
//! tem esta rota. O cliente manda só a porta; quem a segura é lido aqui, agora.
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use hangar_core::http::detail;
use hangar_port_stop::{Error, Real, Scope, Stopped};
use serde_json::json;

use crate::routes::{AppState, cors, owner_gate};

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
                   Path((name, port)): Path<(String, String)>, req: Request) -> Response {
    let mut response = match owner_gate(&st, peer, &req).await {
        Ok(_) => stop(&st, &name, &port).await,
        Err(response) => response,
    };
    cors(req.headers(), response.headers_mut());
    response
}

async fn stop(st: &AppState, name: &str, port: &str) -> Response {
    if !hangar_port_stop::SUPPORTED {
        return refusal(StatusCode::NOT_IMPLEMENTED, "port_stop_unsupported", "parar servidor só existe no Linux");
    }
    let Some(port) = port.parse::<u16>().ok().filter(|port| *port > 0) else { return not_found() };
    let facts = match st.list.port_stop_pids(name).await {
        Ok(Some(facts)) => facts,
        Ok(None) => return not_found(),
        Err(e) => return hangar_core::http::json_response(StatusCode::SERVICE_UNAVAILABLE,
            json!({"detail": crate::list::hub::unavailable_detail(&e)})),
    };
    let done = tokio::task::spawn_blocking(move || -> Result<Option<(Stopped, Option<Scope>)>, Error> {
        // A lista guarda os sockets por até 3 s: quem segura a porta é relido agora.
        let Some((pid, start)) = facts.listener(port).map_err(Error::Io)? else { return Ok(None) };
        hangar_port_stop::stop(&mut Real, pid, start, std::process::id(), facts.guarded()).map(Some)
    }).await;
    let (outcome, scope) = match done {
        // O pid de quem escutava já é de outro processo: a porta não é mais dele.
        Ok(Ok(None) | Err(Error::Reused)) => return not_found(),
        Ok(Ok(Some(stopped))) => stopped,
        Ok(Err(Error::SharedGroup)) => return refusal(StatusCode::CONFLICT, "port_stop_shared_group",
            "o processo que escuta é do agente, da sessão ou do Hangar"),
        Ok(Err(Error::Io(error))) => {
            tracing::warn!(code = "port_stop_failed", kind = ?error.kind(), "parar servidor da porta falhou");
            return refusal(StatusCode::INTERNAL_SERVER_ERROR, "port_stop_failed", "não foi possível parar o servidor");
        }
        Err(_) => return refusal(StatusCode::INTERNAL_SERVER_ERROR, "port_stop_failed", "a parada não terminou"),
    };
    // A leitura guardada dos sockets ainda tem a porta: sem ela, a próxima rodada da lista relê os
    // sockets e já sai sem a porta.
    crate::list::ports::forget_scan();
    st.list.invalidate();
    let result = match outcome { Stopped::Gone => "gone", Stopped::Terminated => "terminated", Stopped::Killed => "killed" };
    let mut body = json!({"ok": true, "result": result});
    // Sem escopo: o processo já tinha saído antes de qualquer sinal.
    if let Some(scope) = scope { body["scope"] = json!(match scope { Scope::Group => "group", Scope::Tree => "tree" }); }
    hangar_core::http::json_response(StatusCode::OK, body)
}

fn not_found() -> Response {
    refusal(StatusCode::NOT_FOUND, "port_stop_not_found", "a porta não é mais desta sessão")
}

fn refusal(status: StatusCode, code: &str, msg: &str) -> Response { detail(status, code, msg, json!({})) }
