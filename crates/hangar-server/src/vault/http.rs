//! Rotas do cofre: só o dono, nunca repassadas ao Python. Corpo malformado é recusado com código
//! fixo, sem ecoar nada: a recusa padrão do `Json` do axum e o erro do serde podem repetir a senha.
use axum::{extract::{ConnectInfo, Path, Request, State}, http::{HeaderMap, Method, StatusCode, header}, response::{IntoResponse, Response}};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
use zeroize::Zeroizing;

use super::{EntryChange, KeyError, NewEntry, Role, Status, VaultError, backup::{self, RestoreError}, config::{self, ConfigError, Placement}, remote::{Remote, RemoteError, UseStart, WaitStatus},
    requests::{Answer, RequestError}, store, wire::{AskReq, UseReply, UseReq, WAIT_HOLD, WaitReply, WaitState, own_id, request_id_ok}};
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
        ["recovery"] | ["restore"] => method == Method::POST,
        ["backup", id] if !id.is_empty() => matches!(*method, Method::GET | Method::PUT),
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
/// deixaria cópias do corpo sem zerar). Zerar cada pedaço recebido é melhor esforço: o `Bytes`
/// compartilhado com outro dono não se zera daqui.
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
    // Corpo fora de forma nas rotas é o código fixo, sem dizer qual campo.
    if error == VaultError::Invalid { return invalid(); }
    let (status, code, msg) = error.code_msg();
    let status = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
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
    let mut here = st.vault.status(&st.hangar_dir).await;
    // A chave abre, mas o arquivo não: dizer "aberto" levaria a tela a oferecer o que vai falhar.
    let unreadable = here.state == super::State::Open
        && matches!(st.vault.read(&st.hangar_dir, |_| Ok(())).await, Err(VaultError::Unreadable));
    if unreadable { here.state = super::State::Unavailable; }
    // O estado daqui vai para o evento `vault` da lista; o de outra dona, não.
    let mine = |here: Status| {
        st.vault_requests.note_state(here.state);
        let mut body = json!(here);
        if let Some(id) = own_id(&st.server_id) { body["server_id"] = json!(id); }
        // Cliente que ainda não conhece o estado mostra o motivo.
        if here.state == super::State::Unavailable {
            let why = if unreadable { VaultError::Unreadable } else { VaultError::Key(KeyError::Bus) };
            body["detail"] = json!(why.code_msg().2);
            // Só este caso a cópia resolve: com o chaveiro fora, restaurar falharia também.
            if unreadable { body["unreadable"] = json!(true); }
        }
        if here.role == Role::Owner {
            match config::load(&st.hangar_dir) {
                Ok(config::Config { backup_peer: Some(peer), .. }) => body["backup"] = super::backup::status(&st.hangar_dir, &peer),
                Ok(_) => {}
                Err(_) => body["backup"] = super::backup::status_without_config(&st.hangar_dir),
            }
        }
        ok(body)
    };
    if local || here.role == Role::Owner { return mine(here); }
    let owner_id = match config::placement(&st.hangar_dir) {
        Ok(Placement::Remote(id)) => id,
        Ok(_) => return mine(here),
        Err(error) => return config_failure(st, error),
    };
    let mut body = match (Remote { peers: &st.peers, owner: &owner_id }).status().await {
        Ok(Status { role: Role::Owner, state }) => json!({"role": "remote", "owner": owner_id, "state": state}),
        Ok(_) => json!({"role": "remote", "owner": owner_id, "state": "absent", "detail": format!("{owner_id} não guarda cofre")}),
        Err(error) => json!({"role": "remote", "owner": owner_id, "state": "offline", "detail": error.reason()}),
    };
    // O cliente acha entre os servidores dele qual é a dona, para abrir o cofre lá.
    let (peers, id) = (st.peers.clone(), owner_id.clone());
    if let Ok(Some(url)) = tokio::task::spawn_blocking(move || peers.base_url(&id)).await { body["owner_url"] = json!(url); }
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
        let is_self = |id: &str| id == "self" || (!st.server_id.is_empty() && id == st.server_id);
        // Ler, mudar e gravar o `vault.json` um pedido por vez.
        let config_guard = st.vault.config_lock.lock().await;
        // `vault.json` torto só não impede virar dona nem tirar a dona: os dois reescrevem o arquivo.
        let rewrite = matches!(&change.owner, Some(None)) || matches!(&change.owner, Some(Some(id)) if is_self(id.as_str()));
        let before = match config::load(&st.hangar_dir) {
            Ok(cfg) => Some(cfg),
            Err(ConfigError::Unreadable) if rewrite => {
                tracing::warn!(code = "vault_config_rewritten", "o vault.json estava torto e vai ser reescrito: a máquina da cópia diária precisa ser escolhida de novo");
                None
            }
            Err(error) => break 'r config_failure(&st, error),
        };
        let mut cfg = before.clone().unwrap_or_default();
        let mut created = false;
        match change.owner {
            Some(Some(id)) if is_self(id.as_str()) => {
                if let Err(error) = st.vault.create(&st.hangar_dir).await { break 'r failure(&st, error); }
                cfg.owner = None;
                created = true;
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
        if before.as_ref() != Some(&cfg) {
            let dir = st.hangar_dir.clone();
            let saved = tokio::task::spawn_blocking(move || config::save(&dir, &cfg)).await
                .unwrap_or_else(|error| Err(super::join_cause("vault_config_interrupted", ConfigError::Io)(error)));
            match saved {
                Err(_) if created => {
                    st.diag.report("rust.vault_failed", "", "erro_cofre_indisponivel", "o cofre nasceu e o vault.json não gravou");
                    break 'r fail(StatusCode::SERVICE_UNAVAILABLE, "erro_cofre_indisponivel",
                        "o cofre já existe nesta máquina, mas o vault.json não gravou; ela segue como dona");
                }
                Err(error) => break 'r config_failure(&st, error),
                Ok(()) => {}
            }
        }
        drop(config_guard);
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
        RequestError::Cooldown => fail(StatusCode::TOO_MANY_REQUESTS, "erro_cofre_pedidos_demais", "o dono acabou de recusar um pedido desta sessão; espere 30 s"),
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
            Ok(UseStart::Request(id)) => ok(json!(UseReply { request_id: Some(id), value: None, server_id: own_id(&st.server_id) })),
            Ok(UseStart::Value(value)) => {
                let len = value.len();
                secret_json(&st, &UseReply { request_id: None, value: Some(value), server_id: own_id(&st.server_id) }, len)
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
            Ok(id) => ok(json!(super::wire::AskReply { request_id: id, server_id: own_id(&st.server_id) })),
            Err(error) => request_failure(&st, error),
        }
    };
    done(&headers, resp)
}

fn wait_reply(status: WaitStatus, server_id: &str) -> WaitReply {
    let reply = |status| WaitReply { status, value: None, entry: None, temporary: None, server_id: own_id(server_id) };
    match status {
        WaitStatus::Pending => reply(WaitState::Pending),
        WaitStatus::Allowed(value) => WaitReply { value: Some(value), ..reply(WaitState::Allowed) },
        WaitStatus::Saved { entry, temporary } => WaitReply { entry: Some(entry), temporary: Some(temporary), ..reply(WaitState::Saved) },
        WaitStatus::Denied => reply(WaitState::Denied),
        WaitStatus::Declined => reply(WaitState::Declined),
        WaitStatus::TimedOut => reply(WaitState::TimedOut),
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
                secret_json(&st, &wait_reply(status, &st.server_id), len)
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryReq { password: Zeroizing<String> }

/// A senha de recuperação protege a cópia que mora em outra máquina: curta cai por força bruta.
const MIN_RECOVERY: usize = 12;

/// `POST /api/vault/recovery`: define (ou troca) a senha de recuperação e manda uma cópia já.
pub async fn recovery(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let allowed = owner(&st, peer, &req);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        let Some(asked) = body::<RecoveryReq>(req).await else { break 'r invalid() };
        if !backup::password_ok(&asked.password) { break 'r invalid(); }
        if asked.password.chars().count() < MIN_RECOVERY {
            break 'r fail(StatusCode::UNPROCESSABLE_ENTITY, "erro_cofre_recuperacao_curta", "a senha de recuperação precisa de 12 caracteres ou mais");
        }
        if let Err(error) = backup::set_recovery(&st.vault, &st.hangar_dir, asked.password).await { break 'r failure(&st, error); }
        let later = st.clone();
        tokio::spawn(async move { backup::run(&later).await; });
        status_body(&st, true).await
    };
    done(&headers, resp)
}

/// `server_id`: de qual máquina é a cópia; sem ele, a desta (a dona perdida com o mesmo id).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RestoreReq {
    password: Zeroizing<String>,
    from_peer: String,
    #[serde(default)]
    server_id: Option<String>,
}

fn restore_failure(st: &AppState, from_peer: &str, error: RestoreError) -> Response {
    match error {
        RestoreError::RemoteOwner => fail(StatusCode::CONFLICT, "erro_cofre_dona_remota",
            "esta máquina usa o cofre de outra dona; a restauração só entra na máquina que vai ser a dona"),
        RestoreError::NotEmpty => fail(StatusCode::CONFLICT, "erro_cofre_nao_vazio", "o cofre desta máquina já tem senhas; a restauração só entra num cofre vazio"),
        // 401 o cliente lê como token do dono perdido.
        RestoreError::WrongPassword => fail(StatusCode::UNPROCESSABLE_ENTITY, "erro_cofre_recuperacao_errada", "senha de recuperação errada (ou cópia adulterada)"),
        RestoreError::NoCopy => fail(StatusCode::NOT_FOUND, "erro_cofre_backup_ausente", "a máquina não guarda cópia deste cofre"),
        RestoreError::PeerDown(reason) => detail(StatusCode::BAD_GATEWAY, "erro_cofre_backup_peer_fora", "a máquina da cópia não respondeu",
            json!({"peer": from_peer, "reason": reason})),
        RestoreError::Unreadable => fail(StatusCode::BAD_GATEWAY, "erro_cofre_backup_ilegivel", "a cópia recebida não está no formato do cofre"),
        RestoreError::Vault(error) => failure(st, error),
        RestoreError::RecoveryNotSaved(_) => fail(StatusCode::CONFLICT, "erro_cofre_recuperacao_nao_gravada",
            "o cofre voltou, mas a senha de recuperação não gravou no chaveiro: defina de novo"),
    }
}

pub async fn restore(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let allowed = owner(&st, peer, &req);
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        let Some(RestoreReq { password, from_peer, server_id }) = body::<RestoreReq>(req).await else { break 'r invalid() };
        if let Err(resp) = known_peer(&st, &from_peer) { break 'r resp; }
        let server_id = server_id.unwrap_or_else(|| st.server_id.clone());
        if !config::valid_server_id(&server_id) || !backup::password_ok(&password) { break 'r invalid(); }
        // Tarefa própria: conexão que cai entre as duas gravações deixaria o cofre novo com a chave de recuperação velha.
        let (worker, source) = (st.clone(), from_peer.clone());
        let restored = tokio::spawn(async move {
            backup::restore(&worker.vault, &worker.peers, &worker.hangar_dir, &source, &server_id, password).await
        }).await.unwrap_or_else(|error| Err(super::join_cause("vault_restore_interrupted", RestoreError::Vault(VaultError::Io))(error)));
        match restored {
            Ok(()) => status_body(&st, true).await,
            Err(error) => restore_failure(&st, &from_peer, error),
        }
    };
    done(&headers, resp)
}

/// `PUT /api/vault/backup/{server_id}?entries=N` guarda a cópia de outra máquina (corpo opaco, sem a
/// chave); `GET` devolve a mais nova. Qualquer máquina do dono pode guardar.
pub async fn backup_copy(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(server_id): Path<String>, req: Request) -> Response {
    use http_body_util::BodyExt;
    let headers = req.headers().clone();
    let (allowed, putting) = (owner(&st, peer, &req), req.method() == Method::PUT);
    let entries = crate::auth::query_param(req.uri().query(), "entries").and_then(|v| v.parse::<u64>().ok());
    let resp = 'r: {
        if let Err(resp) = allowed.await { break 'r resp; }
        if !config::valid_server_id(&server_id) { break 'r invalid(); }
        let dir = st.hangar_dir.clone();
        if !putting {
            break 'r match tokio::task::spawn_blocking(move || backup::newest_copy(&dir, &server_id)).await {
                Ok(Ok(Some(bytes))) => (StatusCode::OK, [(header::CONTENT_TYPE, "application/octet-stream"), (header::CACHE_CONTROL, "no-store")],
                    bytes).into_response(),
                Ok(Ok(None)) => fail(StatusCode::NOT_FOUND, "erro_cofre_backup_ausente", "esta máquina não guarda cópia deste cofre"),
                _ => failure(&st, VaultError::Io),
            };
        }
        let Some(entries) = entries else { break 'r invalid() };
        let bytes = match http_body_util::Limited::new(req.into_body(), backup::MAX_COPY).collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(error) if error.downcast_ref::<http_body_util::LengthLimitError>().is_some() =>
                break 'r fail(StatusCode::PAYLOAD_TOO_LARGE, "erro_cofre_backup_grande", "a cópia passa de 4 MiB"),
            Err(_) => break 'r invalid(),
        };
        if !backup::looks_like_copy(&bytes) { break 'r invalid(); }
        match tokio::task::spawn_blocking(move || backup::keep_copy(&dir, &server_id, &bytes, entries)).await {
            Ok(Ok(_)) => ok(json!({"server_id": own_id(&st.server_id)})),
            Ok(Err(error)) => failure(&st, super::io_cause("vault_backup_keep", VaultError::Io)(error)),
            Err(error) => failure(&st, super::join_cause("vault_backup_keep_interrupted", VaultError::Io)(error)),
        }
    };
    done(&headers, resp)
}
