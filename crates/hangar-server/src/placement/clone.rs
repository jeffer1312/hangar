//! `POST /api/placement/clone`: o repositório que esta máquina não tem, clonado na pasta de clone antes de a
//! sessão abrir nele. Passo próprio, fora da criação: o clone passa de longe o prazo dela.
use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};

use axum::{body::to_bytes, extract::{ConnectInfo, Request, State}, http::StatusCode, response::Response};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::diag::DiagClient;
use crate::groups::peers::{PeerClient, PeerError};
use crate::routes::{AppState, owner_gate};
use crate::session_open::OpenError;
use crate::session_write::{detail_body, json_response};

/// Repositório grande em rede lenta; quem pede espera um pouco mais (`groups::peers::CLONE_TIMEOUT`).
const CLONE_WAIT: Duration = Duration::from_secs(600);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Asked {
    origin: String,
    dir: String,
}

fn refusal(status: u16, code: &'static str, msg: impl Into<String>, params: Value) -> OpenError {
    OpenError::new(status, code, msg).with_params(params)
}

pub async fn clone(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    if let Err(response) = owner_gate(&st, peer, &request).await { return response; }
    let parsed = to_bytes(request.into_body(), 64 * 1024).await.ok()
        .and_then(|raw| serde_json::from_slice::<Asked>(&raw).ok());
    let Some(asked) = parsed else {
        return json_response(StatusCode::UNPROCESSABLE_ENTITY, detail_body("erro_corpo_invalido", "pedido de clone inválido", json!({})));
    };
    let dir = crate::worktree_context::expanded(asked.dir.trim(), &st.accounts.env);
    let diag = st.diag.clone();
    let result = tokio::task::spawn_blocking(move || clone_into(&asked.origin, &dir, CLONE_WAIT, &diag)).await
        .unwrap_or_else(|_| Err(refusal(500, "placement_clone_failed", "o clone não terminou", json!({}))));
    match result {
        Ok(root) => json_response(StatusCode::OK, json!({"root": root})),
        Err(r) => {
            st.diag.report("rust.placement_clone_failed", "", r.code.unwrap_or_default(), "clone do balanceamento recusado");
            json_response(StatusCode::from_u16(r.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR), r.body())
        }
    }
}

/// O clone do crate, com a pasta temporária que ficou no disco registrada no diário.
fn clone_into(origin: &str, dir: &Path, wait: Duration, diag: &DiagClient) -> Result<String, OpenError> {
    let cloned = hangar_placement::clone::clone_into(origin, dir, wait);
    if let Some(kind) = cloned.leftover {
        diag.report_with("rust.placement_clone_leftover", "", "placement_clone_tmp_left", "pasta temporária do clone ficou no disco",
            json!({"io": format!("{kind:?}")}));
    }
    cloned.result.map_err(|r| refusal(r.status, r.code, r.msg, r.params))
}

/// Pede o clone a `server` e devolve a raiz de lá.
pub async fn remote(peers: &PeerClient, server: &str, origin: &str, dir: &str) -> Result<String, OpenError> {
    let answer = peers.call_cloning(server, Method::POST, "/api/placement/clone", Some(&json!({"origin": origin, "dir": dir}))).await;
    read_remote(server, answer)
}

fn read_remote(server: &str, answer: Result<Option<Value>, PeerError>) -> Result<String, OpenError> {
    let params = json!({"server": server});
    let uncertain = |cause: String| OpenError::new(504, "placement_clone_uncertain", format!("o clone em {server} não respondeu: pode ter \
        terminado lá. Abra de novo, que a pasta já clonada é reaproveitada")).with_params(json!({"server": server, "cause": cause}));
    match answer {
        Ok(Some(value)) => value["root"].as_str().filter(|r| !r.is_empty()).map(str::to_owned)
            .ok_or_else(|| uncertain(format!("{server}: resposta do clone sem a raiz"))),
        Ok(None) => Err(uncertain(format!("{server}: resposta do clone vazia"))),
        // A rota não existe lá: a máquina não devia ter sido candidata.
        Err(error) if error.missing_route() => Err(OpenError::new(409, "placement_peer_old",
            format!("{server} está numa versão sem o clone do balanceamento: escolha outra máquina")).with_params(params)),
        Err(PeerError::Refused { status, detail }) => {
            let code = match detail["code"].as_str() {
                Some("placement_clone_conflict") => "placement_clone_conflict",
                Some("placement_clone_timeout") => "placement_clone_timeout",
                Some("placement_clone_invalid") => "placement_clone_invalid",
                _ => "placement_clone_failed",
            };
            let msg = detail["msg"].as_str().map_or_else(|| format!("{server} recusou o clone (HTTP {status})"), |m| format!("{server}: {m}"));
            let mut params = detail.get("params").filter(|p| p.is_object()).cloned().unwrap_or_else(|| json!({}));
            params["server"] = json!(server);
            Err(OpenError::new(status, code, msg).with_params(params))
        }
        Err(PeerError::Unknown) => Err(super::forward::machine_unknown(server)),
        Err(error) => Err(uncertain(error.text(server))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_answer_from_the_other_machine_keeps_its_code_and_an_old_one_is_out() {
        assert_eq!(read_remote("vm", Ok(Some(json!({"root": "/h/Projetos/a"})))).unwrap(), "/h/Projetos/a");
        let old = read_remote("vm", Err(PeerError::Refused { status: 404, detail: json!("Not Found") })).unwrap_err();
        assert_eq!((old.status, old.code), (409, Some("placement_peer_old")));
        let conflict = read_remote("vm", Err(PeerError::Refused { status: 409,
            detail: json!({"code": "placement_clone_conflict", "msg": "/h/a já existe", "params": {"root": "/h/a"}}) })).unwrap_err();
        assert_eq!((conflict.status, conflict.code), (409, Some("placement_clone_conflict")));
        assert_eq!((&conflict.params["root"], &conflict.params["server"]), (&json!("/h/a"), &json!("vm")));
        assert!(conflict.msg.starts_with("vm: "), "{}", conflict.msg);
        // Sem resposta, lá pode ter clonado: nunca "falhou".
        let silent = read_remote("vm", Err(PeerError::Transport("prazo".into()))).unwrap_err();
        assert_eq!((silent.status, silent.code), (504, Some("placement_clone_uncertain")));
    }
}
