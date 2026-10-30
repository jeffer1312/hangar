//! Rotas do cofre: só o dono, nunca repassadas ao Python. Corpo malformado é recusado com código
//! fixo, sem ecoar nada: a recusa padrão do `Json` do axum e o erro do serde podem repetir a senha.
use axum::{extract::{ConnectInfo, Path, Request, State}, http::{HeaderMap, Method, StatusCode, header}, response::{IntoResponse, Response}};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
use zeroize::Zeroizing;

use super::{EntryChange, KeyError, NewEntry, Role, Status, VaultError, config::{self, ConfigError, Placement}, remote::{Remote, RemoteError, UseStart, WaitStatus},
    requests::{Answer, RequestError}, store, wire::{AskReq, UseReply, UseReq, WAIT_HOLD, WaitReply, WaitState, request_id_ok}};
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
        ["use"] | ["ask-entry"] => method == Method::POST,
        ["requests", id] if !id.is_empty() => matches!(*method, Method::POST | Method::DELETE),
        ["requests", id, "wait"] if !id.is_empty() => method == Method::GET,
        _ => false,
    }
}

fn fail(status: StatusCode, code: &str, msg: &str) -> Response { detail(status, code, msg, Value::Null) }

fn invalid() -> Response { fail(StatusCode::UNPROCESSABLE_ENTITY, "erro_corpo_invalido", "corpo inválido") }

/// Convidado nunca; sem o token do dono, 401 e a falha conta para o bloqueio. Síncrona até o
/// `async move`: o futuro não guarda `&Request` (o corpo não é `Sync`, e o handler deixaria de ser `Send`).
fn owner(st: &AppState, peer: SocketAddr, req: &Request) -> impl Future<Output = Result<(), Response>> + Send + use<> {
    let guest = crate::routes::on_guest_port(req) || req.headers().contains_key("x-hangar-guest");
    let gate = crate::routes::gate(st, peer, req);
    let auth = st.auth.clone();
    async move {
        if guest { return Err(fail(StatusCode::FORBIDDEN, "erro_cofre_so_dono", "só o dono usa o cofre")); }
        let (fwd, owner) = gate.await.map_err(crate::auth::unavailable)?;
        if !owner {
            auth.record_failure(&fwd.client_ip).await.map_err(crate::auth::unavailable)?;
            return Err(fail(StatusCode::UNAUTHORIZED, "unauthorized", "token do dono ausente ou errado"));
        }
        Ok(())
    }
}

/// `None` = malformado, sem dizer o quê. O buffer nasce com a capacidade do limite (crescer
/// deixaria cópias do corpo sem zerar), e cada pedaço recebido é zerado depois de copiado.
async fn body<T: DeserializeOwned>(req: Request) -> Option<T> {
    use http_body_util::BodyExt;
    let mut body = req.into_body();
    let mut raw = Zeroizing::new(Vec::with_capacity(MAX_BODY));
    while let Some(frame) = body.frame().await {
        let Ok(data) = frame.ok()?.into_data() else { continue };
        let fits = raw.len() + data.len() <= MAX_BODY;
        if fits { raw.extend_from_slice(&data); }
        if let Ok(mut owned) = data.try_into_mut() { zeroize::Zeroize::zeroize(owned.as_mut()); }
        if !fits { return None; }
    }
    serde_json::from_slice::<T>(&raw).ok()
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

fn config_failure(st: &AppState, error: ConfigError) -> Response {
    let (status, code, msg) = match error {
        ConfigError::Unreadable => (StatusCode::INTERNAL_SERVER_ERROR, "erro_cofre_config_ilegivel", "o vault.json desta máquina não lê"),
        ConfigError::Io => (StatusCode::SERVICE_UNAVAILABLE, "erro_cofre_indisponivel", "não foi possível ler ou gravar o vault.json"),
    };
    st.diag.report("rust.vault_failed", "", code, "a configuração do cofre não leu ou não gravou");
    fail(status, code, msg)
}

/// A recusa da dona passa adiante com o código dela; dona fora vira 502 com o motivo.
pub(crate) fn remote_failure(owner: &str, error: RemoteError) -> Response {
    match error {
        RemoteError::OwnerDown(reason) => detail(StatusCode::BAD_GATEWAY, "erro_cofre_dona_fora", "a máquina dona do cofre não respondeu",
            json!({"owner": owner, "reason": reason})),
        RemoteError::Lost => fail(StatusCode::NOT_FOUND, "erro_cofre_pedido_perdido", "a dona do cofre não conhece mais este pedido"),
        RemoteError::Refused { status, code, msg } => fail(StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_GATEWAY), &code, &msg),
    }
}

/// `?local=1`: o pedido veio de outra máquina e não volta a ser encaminhado.
fn local_only(req: &Request) -> bool { crate::auth::query_param(req.uri().query(), "local").is_some_and(|v| v == "1") }

fn done(headers: &HeaderMap, mut resp: Response) -> Response {
    crate::routes::cors(headers, resp.headers_mut());
    resp
}

fn ok(body: Value) -> Response { crate::session_write::json_response(StatusCode::OK, body) }

pub async fn status(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let (allowed, local) = (owner(&st, peer, &req), local_only(&req));
    drop(req);
    let resp = match allowed.await {
        Err(resp) => resp,
        Ok(()) => status_body(&st, local).await,
    };
    done(&headers, resp)
}

/// O estado daqui; na máquina que aponta para uma dona, o da dona (`offline` com o motivo quando
/// ela não responde). `local`: nunca encaminha.
async fn status_body(st: &AppState, local: bool) -> Response {
    let here = st.vault.status(&st.hangar_dir).await;
    // O estado daqui vai para o evento `vault` da lista; o de outra dona, não.
    let mine = |here: Status| { st.vault_requests.note_state(here.state); ok(json!(here)) };
    if local || here.role == Role::Owner { return mine(here); }
    let owner_id = match config::placement(&st.hangar_dir) {
        Ok(Placement::Remote(id)) => id,
        Ok(_) => return mine(here),
        Err(error) => return config_failure(st, error),
    };
    let body = match (Remote { peers: &st.peers, owner: &owner_id }).status().await {
        Ok(Status { role: Role::Owner, state }) => json!({"role": "remote", "owner": owner_id, "state": state}),
        Ok(_) => json!({"role": "remote", "owner": owner_id, "state": "absent", "detail": format!("{owner_id} não guarda cofre")}),
        Err(error) => json!({"role": "remote", "owner": owner_id, "state": "offline", "detail": error.reason()}),
    };
    ok(body)
}

/// `owner`: `"self"` (ou o id desta máquina) cria o cofre aqui; outro id grava a dona no
/// `vault.json`. `backup_peer` fica para a cópia diária. `null` apaga o campo.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigChange {
    #[serde(default, deserialize_with = "super::present")]
    owner: Option<Option<String>>,
    #[serde(default, deserialize_with = "super::present")]
    backup_peer: Option<Option<String>>,
}

/// Máquina que o `peers.json` alcança: sem isso a dona ou a cópia não teriam para onde ir.
fn known_peer(st: &AppState, id: &str) -> Result<(), Response> {
    if !config::valid_server_id(id) { return Err(invalid()); }
    if st.peers.base_url(id).is_none() {
        return Err(fail(StatusCode::CONFLICT, "erro_cofre_peer_desconhecido", "a máquina não está no peers.json (ou sem base_url/token)"));
    }
    Ok(())
}

pub async fn config(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let allowed = owner(&st, peer, &req);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        let Some(change) = body::<ConfigChange>(req).await else { break 'r invalid() };
        if change.owner.is_none() && change.backup_peer.is_none() { break 'r invalid(); }
        let before = match config::load(&st.hangar_dir) { Ok(cfg) => cfg, Err(error) => break 'r config_failure(&st, error) };
        let mut cfg = before.clone();
        match change.owner {
            Some(Some(id)) if id == "self" || id == st.server_id => {
                if let Err(error) = st.vault.create(&st.hangar_dir).await { break 'r failure(&st, error); }
                cfg.owner = None;
            }
            Some(Some(id)) => {
                // Apontar para outra dona deixaria as senhas daqui inalcançáveis pelas tools.
                if store::exists(&store::path(&st.hangar_dir)) {
                    break 'r fail(StatusCode::CONFLICT, "erro_cofre_ja_e_dona", "esta máquina já guarda um cofre; outra não pode ser a dona");
                }
                if let Err(resp) = known_peer(&st, &id) { break 'r resp; }
                cfg.owner = Some(id);
            }
            Some(None) => cfg.owner = None,
            None => {}
        }
        match change.backup_peer {
            Some(Some(id)) => {
                if let Err(resp) = known_peer(&st, &id) { break 'r resp; }
                cfg.backup_peer = Some(id);
            }
            Some(None) => cfg.backup_peer = None,
            None => {}
        }
        if cfg != before && let Err(error) = config::save(&st.hangar_dir, &cfg) { break 'r config_failure(&st, error); }
        status_body(&st, false).await
    };
    done(&headers, resp)
}

pub async fn entries(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let (allowed, local, reading) = (owner(&st, peer, &req), local_only(&req), req.method() == Method::GET);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        if reading {
            if !local {
                match config::placement(&st.hangar_dir) {
                    Ok(Placement::Remote(id)) => break 'r match (Remote { peers: &st.peers, owner: &id }).entries().await {
                        Ok(list) => ok(json!(list)),
                        Err(error) => remote_failure(&id, error),
                    },
                    Ok(_) => {}
                    Err(error) => break 'r config_failure(&st, error),
                }
            }
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
    let (allowed, deleting) = (owner(&st, peer, &req), req.method() == Method::DELETE);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        if deleting {
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

/// Corpo que leva senha. `secret_len` é o tamanho do valor: cada byte escapa em até 6 no JSON, e a
/// capacidade nasce com isso (crescer deixaria cópia sem zerar). O dono zerado faz o corpo sumir
/// zerado quando o hyper o solta.
fn secret_json<T: serde::Serialize>(st: &AppState, value: &T, secret_len: usize) -> Response {
    let mut body = Zeroizing::new(Vec::with_capacity(secret_len * 6 + 128));
    if serde_json::to_writer(&mut *body, value).is_err() { return failure(st, VaultError::Io); }
    (StatusCode::OK, [(header::CONTENT_TYPE, "application/json"), (header::CACHE_CONTROL, "no-store")],
        axum::body::Body::from(bytes::Bytes::from_owner(body))).into_response()
}

pub async fn password(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    let allowed = owner(&st, peer, &req);
    drop(req);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        match st.vault.password(&st.hangar_dir, &id).await {
            Ok(password) => secret_json(&st, &Reveal { password: password.as_str() }, password.len()),
            Err(error) => failure(&st, error),
        }
    };
    done(&headers, resp)
}

fn lost() -> Response { fail(StatusCode::NOT_FOUND, "erro_cofre_pedido_perdido", "a dona do cofre não conhece mais este pedido") }

fn request_failure(st: &AppState, error: RequestError) -> Response {
    match error {
        RequestError::Vault(error) => failure(st, error),
        RequestError::Lost => lost(),
        RequestError::Stale => fail(StatusCode::CONFLICT, "erro_interacao_desatualizada", "o pedido mudou; abra o cartão de novo"),
        RequestError::Answered => fail(StatusCode::CONFLICT, "erro_cofre_pedido_respondido", "este pedido já foi respondido"),
        RequestError::TooMany => fail(StatusCode::TOO_MANY_REQUESTS, "erro_cofre_pedidos_demais", "pedidos demais esperando resposta no cofre"),
    }
}

/// `POST /api/vault/use`: senha liberada (ou já autorizada para a sessão) volta o valor; o resto
/// abre um pedido e volta o id dele.
pub async fn use_entry(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let allowed = owner(&st, peer, &req);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        let Some(asked) = body::<UseReq>(req).await else { break 'r invalid() };
        match super::requests::use_entry(&st, asked).await {
            Ok(UseStart::Request(id)) => ok(json!({"request_id": id})),
            Ok(UseStart::Value(value)) => {
                let len = value.len();
                secret_json(&st, &UseReply { request_id: None, value: Some(value) }, len)
            }
            Err(error) => request_failure(&st, error),
        }
    };
    done(&headers, resp)
}

pub async fn ask_entry(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let allowed = owner(&st, peer, &req);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        let Some(asked) = body::<AskReq>(req).await else { break 'r invalid() };
        match super::requests::ask_entry(&st, asked) {
            Ok(id) => ok(json!({"request_id": id})),
            Err(error) => request_failure(&st, error),
        }
    };
    done(&headers, resp)
}

fn wait_reply(status: WaitStatus) -> WaitReply {
    let plain = |status| WaitReply { status, value: None, entry: None, temporary: None };
    match status {
        WaitStatus::Pending => plain(WaitState::Pending),
        WaitStatus::Allowed(value) => WaitReply { status: WaitState::Allowed, value: Some(value), entry: None, temporary: None },
        WaitStatus::Saved { entry, temporary } => WaitReply { status: WaitState::Saved, value: None, entry: Some(entry), temporary: Some(temporary) },
        WaitStatus::Denied => plain(WaitState::Denied),
        WaitStatus::Declined => plain(WaitState::Declined),
        WaitStatus::TimedOut => plain(WaitState::TimedOut),
        WaitStatus::Cancelled => plain(WaitState::Cancelled),
    }
}

/// `GET /api/vault/requests/{id}/wait`: segura até `WAIT_HOLD` e responde `pending` sem novidade.
pub async fn wait(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    let allowed = owner(&st, peer, &req);
    drop(req);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        if !request_id_ok(&id) { break 'r lost(); }
        match st.vault_requests.wait(&id, WAIT_HOLD).await {
            Ok(status) => {
                let len = match &status { WaitStatus::Allowed(value) => value.len(), _ => 0 };
                secret_json(&st, &wait_reply(status), len)
            }
            Err(error) => request_failure(&st, error),
        }
    };
    done(&headers, resp)
}

/// `POST` responde o pedido (cartão do dono); `DELETE` é quem pediu desistindo.
pub async fn request(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    let (allowed, deleting) = (owner(&st, peer, &req), req.method() == Method::DELETE);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        if !request_id_ok(&id) { break 'r lost(); }
        if deleting {
            break 'r match st.vault_requests.cancel(&id) {
                Ok(()) => StatusCode::NO_CONTENT.into_response(),
                Err(error) => request_failure(&st, error),
            };
        }
        let Some(answer) = body::<Answer>(req).await else { break 'r invalid() };
        match st.vault_requests.answer(&st.vault, &st.hangar_dir, &id, answer).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(error) => request_failure(&st, error),
        }
    };
    done(&headers, resp)
}
