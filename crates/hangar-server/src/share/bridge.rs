//! Ponte privada dos convites (`/__hangar_server/share`): o Rust é o único dono de `shares.json` e
//! `external_pairs.json`. O Python pede aqui o que só ele vê acontecer (fechar, renomear, trocar
//! de modo, transferência, sessão morta, recado do MCP), com o envelope das outras pontes
//! (`{op, args}` → `{ok, result}` | `{ok: false, error}`). As rotas do dono que chegam pelo
//! Connect têm caminho privado explícito, nunca um repasse genérico "como dono".
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};

use super::ShareState;
use super::owner_routes::sync_tunnel;
use super::pair_routes::{self, Direct};
use crate::routes::AppState;

const MAX_BODY: usize = 1 << 20;

#[derive(Deserialize)]
#[serde(tag = "op", content = "args", rename_all = "snake_case")]
enum Operation {
    RevokeSession { name: String },
    SetLife { name: String, life: Option<String> },
    ModeChangeBegin { name: String },
    ModeChangeEnd { name: String, #[serde(default)] source_life: Option<String> },
    FactsSnapshot { lives: HashMap<String, String>, changing: HashSet<String> },
    SessionDied { name: String },
    EndPair { local: String, address: String },
    ActiveSessions {},
}

struct Failed { code: String, detail: String }

fn io_failed(e: std::io::Error) -> Failed {
    Failed { code: "share_store_failed".into(), detail: e.to_string() }
}

async fn execute(st: Arc<AppState>, share: Arc<ShareState>, op: Operation) -> Result<Value, Failed> {
    let facts = share.lives.facts();
    match op {
        Operation::RevokeSession { name } => {
            // Fechar mata a sessão: convite e par dela saem juntos, como no `kill_session`.
            let revoked = share.store.revoke_session(&name, None).map_err(io_failed)?;
            if revoked > 0 { sync_tunnel(&share).await; }
            Ok(json!({"revoked": revoked}))
        }
        Operation::SetLife { name, life } => share.store.set_life(&name, life).map(|()| json!({})).map_err(io_failed),
        Operation::ModeChangeBegin { name } => { facts.begin_mode_change(&name); Ok(json!({})) }
        Operation::ModeChangeEnd { name, source_life } => {
            // A transferência que segue incompleta (restauração que falhou) continua valendo.
            facts.end_mode_change(&name);
            facts.set_life(&name, source_life);
            Ok(json!({}))
        }
        Operation::FactsSnapshot { lives, changing } => { facts.apply_snapshot(lives, changing); Ok(json!({})) }
        Operation::SessionDied { name } => {
            pair_routes::session_died(st, share, Arc::new(Direct), &name).await;
            Ok(json!({}))
        }
        Operation::EndPair { local, address } => {
            let found = pair_routes::end_pair(&st, &share, &Direct, &local, &address).await;
            Ok(json!({"found": found.is_some(), "errors": found.unwrap_or_default()}))
        }
        Operation::ActiveSessions {} => {
            let mut sessions: Vec<String> = share.store.active_sessions().into_iter().collect();
            sessions.sort();
            Ok(json!({"sessions": sessions}))
        }
    }
}

fn reply(value: Value) -> Response {
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json")], value.to_string()).into_response()
}

pub async fn private(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let refused = |code: &'static str| {
        if crate::warn_limit::allow(None, code) {
            tracing::warn!(code, "ponte de convites recusou o pedido");
        }
        StatusCode::BAD_REQUEST.into_response()
    };
    let Ok(Ok(bytes)) = tokio::time::timeout(Duration::from_secs(6), to_bytes(req.into_body(), MAX_BODY)).await else {
        return refused("share_bridge_body");
    };
    // Operação desconhecida aqui costuma ser Python e Rust de versões diferentes.
    let Ok(op) = serde_json::from_slice::<Operation>(&bytes) else { return refused("share_bridge_invalid_request") };
    let Some(share) = st.share.clone() else {
        return reply(json!({"ok": false, "error": {"code": "share_bridge_off", "detail": "sem HANGAR_SHARES_FILE"}}));
    };
    match execute(st, share, op).await {
        Ok(result) => reply(json!({"ok": true, "result": result})),
        Err(e) => {
            if crate::warn_limit::allow(None, "share_bridge_failed") {
                tracing::warn!(code = %e.code, "ponte de convites falhou");
            }
            reply(json!({"ok": false, "error": {"code": e.code, "detail": e.detail}}))
        }
    }
}

/// Porta das rotas privadas do dono (`/__hangar_server/share/owner/*`): só o Python com o segredo.
pub async fn private_only(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    next.run(req).await
}
