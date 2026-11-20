//! Troca de agente pedida pelo seletor de modelos com a sessão trabalhando (`POST /conta` com `when_idle`):
//! a fila para, a troca espera o turno acabar e então transfere a conversa ao Codex; o que chegou à fila no
//! meio vai ao agente novo. `DELETE /conta` cancela enquanto ela só espera. O estado sai na linha da lista.
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;

use super::{detail_body, session_name};
use crate::mods::http::reply;
use crate::routes::{AppState, gate};
use crate::runtime::claude_reopen::transfer::{Request as TransferRequest, waits};
use crate::runtime::gateway::RuntimeRegistry;
use hangar_api::session::HandoffState;

/// Teto entre conferências da origem: o evento do ator acorda antes; sem ele, a conferência periódica vale.
const POLL: Duration = Duration::from_secs(2);
/// Intervalo mínimo entre conferências, mesmo com eventos seguidos.
const MIN_GAP: Duration = Duration::from_secs(1);
/// Teto da espera: sessão esquecida trabalhando não segura a fila para sempre.
const MAX_WAIT: Duration = Duration::from_secs(2 * 60 * 60);
/// Quanto a falha fica na linha antes de sair.
const FAILED_SHOWN: Duration = Duration::from_secs(30);

/// Registra a troca e devolve 202 na hora; a espera e a transferência correm à parte.
pub(super) async fn start(st: Arc<AppState>, runtime: Arc<RuntimeRegistry>, name: String, request: TransferRequest,
    headers: &axum::http::HeaderMap) -> Response {
    let view = hangar_api::session::Handoff { state: HandoffState::Waiting, provider: "codex".into(), model: request.model.clone(),
        effort: request.effort.clone(), account: request.credential_id.clone(), code: None };
    let Some(generation) = runtime.hold_handoff(&name, view.clone()) else {
        return super::account::transfer_refusal(headers, hangar_conversation_transfer::TransferError::new("session_transfer_busy"));
    };
    st.list.invalidate();
    tokio::spawn(run(st, runtime, name, request, generation));
    reply(Some(headers), StatusCode::ACCEPTED, json!({"pending": true, "handoff": view}))
}

/// Espera o próximo evento do ator desta sessão (fim de turno, troca de estado) ou o teto, o que vier antes.
async fn next_turn_signal(events: &mut tokio::sync::broadcast::Receiver<crate::runtime::protocol::RuntimeEvent>, key: Option<&str>) {
    let signal = async {
        loop {
            match events.recv().await {
                Ok(event) if key.is_none_or(|key| event.key == key) => return,
                Ok(_) => {}
                // Perdeu eventos ou o canal fechou: conferir agora é o seguro.
                Err(_) => return,
            }
        }
    };
    let started = tokio::time::Instant::now();
    let _ = tokio::time::timeout(POLL, signal).await;
    // Turno em fluxo publica vários eventos por segundo: no máximo uma conferência por segundo.
    tokio::time::sleep(MIN_GAP.saturating_sub(started.elapsed())).await;
}

async fn run(st: Arc<AppState>, runtime: Arc<RuntimeRegistry>, name: String, request: TransferRequest, generation: u64) {
    let started = tokio::time::Instant::now();
    let mut events = runtime.subscribe();
    let key = runtime.claude_spec(&name).await.and_then(Result::ok).map(|spec| spec.key);
    let outcome = loop {
        if !runtime.handoff_live(&name, generation) { break None; }
        if started.elapsed() > MAX_WAIT { break Some(Err("session_transfer_wait_expired".to_owned())); }
        match runtime.transfer_ready(&name).await {
            Err(error) if waits(&error.code) => { next_turn_signal(&mut events, key.as_deref()).await; continue; }
            Err(error) => break Some(Err(error.code)),
            Ok(()) => {}
        }
        // Cancelar só vale enquanto espera: daqui em diante a troca segue até o fim ou até restaurar a origem.
        if !runtime.handoff_mark(&name, generation, HandoffState::Running, None) { break None; }
        st.list.invalidate();
        match super::account::transfer_now(&st, &runtime, &name, request.clone()).await {
            Ok(_) => break Some(Ok(())),
            // Um turno começou entre a conferência e a troca (mensagem de outro lugar): volta a esperar.
            Err(error) if waits(&error.code) => {
                if !runtime.handoff_mark(&name, generation, HandoffState::Waiting, None) { break None; }
                st.list.invalidate();
                next_turn_signal(&mut events, key.as_deref()).await;
            }
            Err(error) => break Some(Err(error.code)),
        }
    };
    match outcome {
        Some(Err(code)) => {
            tracing::warn!(session=%name, code=%code, "troca de agente pedida pelo seletor falhou");
            runtime.fail_handoff(&name, generation, &code);
            st.list.invalidate();
            tokio::time::sleep(FAILED_SHOWN).await;
            // A fila já voltou ao Claude na falha: aqui só sai a linha.
            runtime.release_handoff(&name, generation, false);
        }
        // Concluída, cancelada ou substituída: a fila volta a entregar a quem ficou com a sessão.
        Some(Ok(())) | None => runtime.release_handoff(&name, generation, true),
    }
    st.list.invalidate();
}

/// `DELETE /api/sessions/{nome}/conta`: cancela a troca que ainda espera o turno acabar.
pub async fn cancel_route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (_, owner) = match gate(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let (true, Some(name)) = (owner, session_name(req.uri().path())) else { return super::not_owner(&st, peer, req).await };
    let Some(runtime) = st.state.runtime.get().cloned() else { return super::no_runtime(req.headers()) };
    if !runtime.cancel_handoff(&name) {
        return reply(Some(req.headers()), StatusCode::CONFLICT,
            detail_body("session_handoff_not_waiting", "não há troca de agente esperando nesta sessão", json!({})));
    }
    st.list.invalidate();
    reply(Some(req.headers()), StatusCode::OK, json!({"ok": true}))
}
