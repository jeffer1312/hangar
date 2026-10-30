//! Rotas do cofre: só o dono, nunca repassadas ao Python. Corpo malformado é recusado com código
//! fixo, sem ecoar nada: a recusa padrão do `Json` do axum e o erro do serde podem repetir a senha.
use axum::{body::to_bytes, extract::{ConnectInfo, Path, Request, State}, http::{HeaderMap, Method, StatusCode, header}, response::{IntoResponse, Response}};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};

use super::{EntryChange, KeyError, NewEntry, VaultError};
use crate::{routes::AppState, session_write::detail};

const MAX_BODY: usize = 64 * 1024;

/// As rotas do `routes::router` daqui, para a tabela da migração (`migration_status::rust_route`).
pub fn matches(method: &Method, path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/api/vault/") else { return false };
    match rest.split('/').collect::<Vec<_>>().as_slice() {
        ["status"] => method == Method::GET,
        ["config"] => method == Method::PUT,
        ["entries"] => matches!(*method, Method::GET | Method::POST),
        ["entries", id] if !id.is_empty() => matches!(*method, Method::PATCH | Method::DELETE),
        ["entries", id, "password"] if !id.is_empty() => method == Method::GET,
        _ => false,
    }
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response { detail(status, code, msg, Value::Null) }

fn invalid() -> Response { fail(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "corpo inválido") }

/// Convidado nunca; sem o token do dono, 401 e a falha conta para o bloqueio.
async fn owner(st: &AppState, peer: SocketAddr, req: &Request) -> Result<(), Response> {
    if crate::routes::on_guest_port(req) || req.headers().contains_key("x-hangar-guest") {
        return Err(fail(StatusCode::FORBIDDEN, "erro_cofre_so_dono", "só o dono usa o cofre"));
    }
    let (fwd, owner) = crate::routes::gate(st, peer, req).await.map_err(crate::auth::unavailable)?;
    if !owner {
        st.auth.record_failure(&fwd.client_ip).await.map_err(crate::auth::unavailable)?;
        return Err(fail(StatusCode::UNAUTHORIZED, "unauthorized", "token do dono ausente ou errado"));
    }
    Ok(())
}

/// O corpo lido é zerado depois do parse; `None` = malformado, sem dizer o quê.
async fn body<T: DeserializeOwned>(req: Request) -> Option<T> {
    let bytes = to_bytes(req.into_body(), MAX_BODY).await.ok()?;
    let mut raw = bytes.try_into_mut().unwrap_or_else(|b| b[..].into());
    let parsed = serde_json::from_slice::<T>(&raw).ok();
    zeroize::Zeroize::zeroize(raw.as_mut());
    parsed
}

fn failure(st: &AppState, error: VaultError) -> Response {
    let (status, code, msg) = match error {
        VaultError::Key(KeyError::Locked) => (StatusCode::CONFLICT, "erro_cofre_trancado", "o chaveiro está trancado"),
        VaultError::Key(KeyError::Lost | KeyError::Absent) =>
            (StatusCode::CONFLICT, "erro_cofre_chave_perdida", "o cofre existe e a chave dele sumiu do chaveiro"),
        VaultError::Key(KeyError::Unsupported) =>
            (StatusCode::NOT_IMPLEMENTED, "erro_cofre_sem_chaveiro", "esta máquina não tem chaveiro do sistema para o cofre"),
        VaultError::Key(KeyError::Bus) => (StatusCode::SERVICE_UNAVAILABLE, "erro_cofre_chaveiro_indisponivel", "o chaveiro não respondeu"),
        VaultError::Absent => (StatusCode::CONFLICT, "erro_cofre_ausente", "esta máquina não tem cofre"),
        VaultError::Unreadable => (StatusCode::INTERNAL_SERVER_ERROR, "erro_cofre_ilegivel", "o arquivo do cofre não abriu com a chave do chaveiro"),
        VaultError::Io => (StatusCode::SERVICE_UNAVAILABLE, "erro_cofre_indisponivel", "não foi possível ler ou gravar o cofre"),
        VaultError::UnknownEntry => (StatusCode::NOT_FOUND, "erro_cofre_entrada_desconhecida", "senha não encontrada no cofre"),
        VaultError::DuplicateName => (StatusCode::CONFLICT, "erro_cofre_nome_repetido", "já existe uma senha com esse nome"),
        VaultError::Invalid => return invalid(),
    };
    if matches!(error, VaultError::Key(KeyError::Bus) | VaultError::Unreadable | VaultError::Io) {
        st.diag.report("rust.vault_failed", "", code, "o cofre não abriu ou não gravou");
    }
    fail(status, code, msg)
}

fn done(headers: &HeaderMap, mut resp: Response) -> Response {
    crate::routes::cors(headers, resp.headers_mut());
    resp
}

fn ok(body: Value) -> Response { crate::session_write::json_response(StatusCode::OK, body) }

pub async fn status(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let resp = match owner(&st, peer, &req).await {
        Err(resp) => resp,
        Ok(()) => ok(json!(st.vault.status(&st.hangar_dir).await)),
    };
    done(&headers, resp)
}

/// Só `owner` por enquanto; os outros campos ficam para a configuração entre máquinas.
#[derive(Deserialize)]
struct Config { owner: Option<String> }

pub async fn config(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let resp = 'r: {
        if let Err(resp) = owner(&st, peer, &req).await { break 'r resp; }
        let Some(Config { owner: Some(chosen) }) = body::<Config>(req).await else { break 'r invalid() };
        if chosen != "self" { break 'r invalid(); }
        match st.vault.create(&st.hangar_dir).await {
            Ok(()) => ok(json!(st.vault.status(&st.hangar_dir).await)),
            Err(error) => failure(&st, error),
        }
    };
    done(&headers, resp)
}

pub async fn entries(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let resp = 'r: {
        if let Err(resp) = owner(&st, peer, &req).await { break 'r resp; }
        if req.method() == axum::http::Method::GET {
            break 'r match st.vault.list(&st.hangar_dir).await {
                Ok(list) => ok(json!(list)),
                Err(error) => failure(&st, error),
            };
        }
        let Some(new) = body::<NewEntry>(req).await else { break 'r invalid() };
        match st.vault.add(&st.hangar_dir, new).await {
            Ok(id) => ok(json!({"id": id})),
            Err(error) => failure(&st, error),
        }
    };
    done(&headers, resp)
}

pub async fn entry(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    let resp = 'r: {
        if let Err(resp) = owner(&st, peer, &req).await { break 'r resp; }
        if req.method() == axum::http::Method::DELETE {
            break 'r match st.vault.remove(&st.hangar_dir, &id).await {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(error) => failure(&st, error),
            };
        }
        let Some(change) = body::<EntryChange>(req).await else { break 'r invalid() };
        match st.vault.update(&st.hangar_dir, &id, change).await {
            Ok(()) => ok(json!({"id": id})),
            Err(error) => failure(&st, error),
        }
    };
    done(&headers, resp)
}

#[derive(serde::Serialize)]
struct Reveal<'a> { password: &'a str }

pub async fn password(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    let resp = 'r: {
        if let Err(resp) = owner(&st, peer, &req).await { break 'r resp; }
        match st.vault.password(&st.hangar_dir, &id).await {
            Ok(password) => {
                let Ok(body) = serde_json::to_vec(&Reveal { password: password.as_str() }) else { break 'r failure(&st, VaultError::Io) };
                (StatusCode::OK, [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")], body).into_response()
            }
            Err(error) => failure(&st, error),
        }
    };
    done(&headers, resp)
}
