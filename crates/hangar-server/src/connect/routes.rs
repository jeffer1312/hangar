//! Resgate do bilhete e aparelhos do dono. Falha aqui é resposta com código, nunca repasse ao Python.
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Path, Request, State};
use axum::http::{HeaderMap, Method, StatusCode};
use axum::response::Response;
use axum::routing::{delete, get, post};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use super::ConnectError;
use super::config::valid_label;
use super::devices::{Poll, Refusal, Store};
use super::gate::DeviceCaller;
use super::peers::{self, PeerLinker};
use super::preview::LINK_PARAM;
use super::state::LinkRequest;
use crate::machine_peers::store::valid_id;
use crate::routes::{AppState, cors, on_connect_port, owner_gate};
use crate::session_write::json_response;
use crate::share_reach::{Failure, check_answer_until};
use crate::voice::routes::preflight;

const BODY_LIMIT: usize = 16 * 1024;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/api/connect", get(connect_state).delete(forget).options(preflight))
        .route("/api/connect/link", post(link).options(preflight))
        .route("/api/connect/redeem", post(redeem).options(preflight))
        .route("/api/connect/redeem/status", post(status).options(preflight))
        .route("/api/connect/devices", get(list).options(preflight))
        .route("/api/connect/devices/self", post(enroll).options(preflight))
        .route("/api/connect/devices/{id}", delete(remove).options(preflight))
        .route("/api/connect/devices/{id}/approve", post(approve).options(preflight))
        .route("/api/connect/preview", get(preview_state).post(open_preview).delete(close_preview).options(preflight))
        .route("/api/connect/peers", get(peer_links).options(preflight))
        .route("/api/connect/machines", get(account_list).options(preflight))
        .route("/api/connect/peers/{id}/link", post(link_peer).options(preflight))
}

pub fn matches(method: &Method, path: &str) -> bool {
    let id = path.strip_prefix("/api/connect/devices/");
    let peer = path.strip_prefix("/api/connect/peers/").and_then(|rest| rest.strip_suffix("/link"));
    match method.as_str() {
        "GET" => matches!(path, "/api/connect" | "/api/connect/devices" | "/api/connect/peers" | "/api/connect/machines" | "/api/connect/preview"),
        "POST" => matches!(path, "/api/connect/link" | "/api/connect/redeem" | "/api/connect/redeem/status" | "/api/connect/devices/self" | "/api/connect/preview")
            || id.and_then(|rest| rest.strip_suffix("/approve")).is_some_and(|id| !id.is_empty() && !id.contains('/'))
            || peer.is_some_and(|id| !id.is_empty() && !id.contains('/')),
        "DELETE" => matches!(path, "/api/connect" | "/api/connect/preview") || id.is_some_and(|id| !id.is_empty() && !id.contains('/')),
        _ => false,
    }
}

fn with_cors(headers: &HeaderMap, mut response: Response) -> Response {
    cors(headers, response.headers_mut());
    response
}

fn reply(headers: &HeaderMap, status: StatusCode, body: Value) -> Response {
    with_cors(headers, json_response(status, body))
}

fn error(headers: &HeaderMap, status: StatusCode, code: &str, msg: &str) -> Response {
    reply(headers, status, json!({"detail": {"code": code, "params": {}, "msg": msg}}))
}

fn refused(headers: &HeaderMap, refusal: Refusal) -> Response {
    match refusal {
        Refusal::NotLinked => error(headers, StatusCode::CONFLICT, "connect_not_linked", "esta máquina não está ligada ao Connect"),
        Refusal::Ticket => error(headers, StatusCode::UNAUTHORIZED, "connect_ticket_refused", "bilhete recusado"),
        Refusal::TooManyPending => error(headers, StatusCode::TOO_MANY_REQUESTS, "connect_too_many_pending", "muitos aparelhos esperando aprovação"),
        Refusal::Credential => error(headers, StatusCode::UNAUTHORIZED, "connect_device_refused", "aparelho ou segredo recusado"),
        Refusal::Unknown => error(headers, StatusCode::NOT_FOUND, "connect_device_unknown", "aparelho desconhecido"),
        Refusal::NotYours => error(headers, StatusCode::FORBIDDEN, "connect_device_not_yours", "só a senha do dono revoga um aparelho que este não aprovou"),
        Refusal::Io(cause) => {
            tracing::warn!(code = "connect_store_failed", "os aparelhos do Connect não foram gravados: {cause}");
            error(headers, StatusCode::SERVICE_UNAVAILABLE, "connect_store_failed", "os aparelhos do Connect não foram gravados")
        }
    }
}

async fn parse<T: DeserializeOwned>(req: Request) -> Result<T, Response> {
    let headers = req.headers().clone();
    let invalid = || error(&headers, StatusCode::BAD_REQUEST, "connect_body_invalid", "corpo inválido");
    let bytes = to_bytes(req.into_body(), BODY_LIMIT).await.map_err(|_| invalid())?;
    serde_json::from_slice(&bytes).map_err(|_| invalid())
}

/// A loja grava com fsync: fora do executor assíncrono.
async fn blocking<T: Send + 'static>(st: &AppState, op: impl FnOnce(&Store) -> Result<T, Refusal> + Send + 'static) -> Result<T, Refusal> {
    let store = st.connect_devices.clone();
    tokio::task::spawn_blocking(move || op(&*store)).await.unwrap_or_else(|cause| Err(Refusal::Io(io::Error::other(cause))))
}

// `device_name` do corpo fica de fora de propósito: quem aparece na aprovação é o nome assinado no bilhete.
#[derive(Deserialize)]
struct RedeemBody { ticket: String, signature: String, secret: String }

async fn redeem(State(st): State<Arc<AppState>>, req: Request) -> Response {
    let headers = req.headers().clone();
    let body: RedeemBody = match parse(req).await { Ok(body) => body, Err(response) => return response };
    match blocking(&st, move |store| store.redeem(&body.ticket, &body.signature, &body.secret)).await {
        Ok(device_id) => reply(&headers, StatusCode::ACCEPTED, json!({"device_id": device_id, "status": "pending"})),
        Err(refusal) => refused(&headers, refusal),
    }
}

#[derive(Deserialize)]
struct StatusBody { device_id: String, secret: String }

async fn status(State(st): State<Arc<AppState>>, req: Request) -> Response {
    let headers = req.headers().clone();
    let body: StatusBody = match parse(req).await { Ok(body) => body, Err(response) => return response };
    let body = match blocking(&st, move |store| store.status(&body.device_id, &body.secret)).await {
        Ok(Poll::Pending) => json!({"status": "pending"}),
        Ok(Poll::Denied) => json!({"status": "denied"}),
        Ok(Poll::Approved(Some(token))) => json!({"status": "approved", "token": token}),
        Ok(Poll::Approved(None)) => json!({"status": "approved"}),
        Err(refusal) => return refused(&headers, refusal),
    };
    reply(&headers, StatusCode::OK, body)
}

async fn list(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    reply(&headers, StatusCode::OK, json!(st.connect_devices.list()))
}

async fn approve(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    let by = req.extensions().get::<DeviceCaller>().map(|device| device.id.clone());
    match blocking(&st, move |store| store.approve(&id, by.as_deref())).await {
        Ok(()) => reply(&headers, StatusCode::OK, json!({"ok": true})),
        Err(refusal) => refused(&headers, refusal),
    }
}

async fn remove(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    let by = req.extensions().get::<DeviceCaller>().map(|device| device.id.clone());
    match blocking(&st, move |store| by.map_or_else(|| store.remove(&id), |by| store.remove_by(&id, &by))).await {
        Ok(()) => reply(&headers, StatusCode::OK, json!({"ok": true})),
        Err(refusal) => refused(&headers, refusal),
    }
}

#[derive(Deserialize)]
struct EnrollBody { device_name: String }

/// Só a senha do dono, de fora da entrada do Connect. Aparelho não passa: quem for revogado sairia
/// com outra credencial, ou com a máquina ligada a outro serviço.
fn owner_secret_refusal(req: &Request) -> Option<Response> {
    if on_connect_port(req) {
        return Some(error(req.headers(), StatusCode::UNAUTHORIZED, "erro_nao_autorizado", "unauthorized"));
    }
    req.extensions().get::<DeviceCaller>()
        .map(|_| error(req.headers(), StatusCode::FORBIDDEN, "connect_owner_secret_refused", "rota só do dono"))
}

/// Sem aprovação porque quem pede apresentou a senha.
async fn enroll(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Some(response) = owner_secret_refusal(&req) {
        return response;
    }
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    let body: EnrollBody = match parse(req).await { Ok(body) => body, Err(response) => return response };
    match blocking(&st, move |store| store.enroll(&body.device_name)).await {
        Ok((device_id, token)) => reply(&headers, StatusCode::OK, json!({"device_id": device_id, "token": token})),
        Err(refusal) => refused(&headers, refusal),
    }
}

async fn connect_state(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    reply(&headers, StatusCode::OK, json!(st.connect_supervisor.status()))
}

fn finished(st: &AppState, headers: &HeaderMap, outcome: Result<Result<(), ConnectError>, tokio::task::JoinError>) -> Response {
    match outcome {
        Ok(Ok(())) => reply(headers, StatusCode::OK, json!(st.connect_supervisor.status())),
        Ok(Err(failure)) => error(headers, failure.status, failure.code, &failure.msg),
        Err(_) => error(headers, StatusCode::INTERNAL_SERVER_ERROR, "connect_start_failed", "a operação do Connect foi interrompida"),
    }
}

/// Em tarefa própria: o serviço troca a chave ao registrar, e cliente que desiste no meio a perderia.
async fn link(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Some(response) = owner_secret_refusal(&req) {
        return response;
    }
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    let body: LinkRequest = match parse(req).await { Ok(body) => body, Err(response) => return response };
    let supervisor = st.connect_supervisor.clone();
    let outcome = tokio::spawn(async move { supervisor.link(body).await }).await;
    finished(&st, &headers, outcome)
}

#[derive(Deserialize)]
struct PreviewBody { port: u16 }

fn preview_port_refused(headers: &HeaderMap) -> Response {
    error(headers, StatusCode::UNPROCESSABLE_ENTITY, "connect_preview_port_refused", "essa porta é do próprio Hangar")
}

/// Aparelho vale como dono: a prévia não dá acesso a mais nada e acaba com um `DELETE`.
async fn open_preview(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    let body: PreviewBody = match parse(req).await { Ok(body) => body, Err(response) => return response };
    if st.connect_previews.refused(body.port) {
        return preview_port_refused(&headers);
    }
    // Tarefa própria: quem vem pelo Connect perde a conexão na troca dos filhos, e o pedido cancelado os deixaria parados.
    let supervisor = st.connect_supervisor.clone();
    let (host, unconfirmed) = match tokio::spawn(async move { supervisor.publish_preview().await }).await {
        Ok(Ok(published)) => published,
        Ok(Err(failure)) => return error(&headers, failure.status, failure.code, &failure.msg),
        Err(_) => return error(&headers, StatusCode::INTERNAL_SERVER_ERROR, "connect_start_failed", "a operação do Connect foi interrompida"),
    };
    // O site recém-nascido ainda espera DNS e certificado. O 401 é a resposta do listener a quem não tem o link.
    if unconfirmed {
        let reach = &st.connect_preview_reach;
        if let Err(failure) = check_answer_until(&reach.probe, &host, 401, reach.every, reach.deadline).await {
            let (Failure::Dns { detail } | Failure::Funnel { detail }) = failure;
            return error(&headers, StatusCode::CONFLICT, "connect_preview_reach_failed", &format!("o site da prévia não abriu de fora: {detail}"));
        }
        st.connect_supervisor.preview_answered();
    }
    match st.connect_previews.open(body.port, &host) {
        Some(link) => reply(&headers, StatusCode::OK, json!({"url": format!("https://{host}/?{LINK_PARAM}={link}")})),
        None => preview_port_refused(&headers),
    }
}

/// Prévia esquecida precisa aparecer: o link some da tela, e ela segue aberta até o prazo.
async fn preview_state(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    reply(&headers, StatusCode::OK, json!({"preview": st.connect_previews.state()}))
}

async fn close_preview(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    st.connect_previews.close();
    reply(&headers, StatusCode::OK, json!({"ok": true}))
}

/// O andamento de cada ligação com outra máquina da conta: é o que a tela acompanha enquanto o cartão espera.
async fn peer_links(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    reply(&headers, StatusCode::OK, json!(st.connect_supervisor.peer_links().list()))
}

/// As outras máquinas da conta, marcando as que já estão no `peers.json` desta (pelo endereço ou pelo nome).
async fn account_list(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    let linked = match st.connect_supervisor.read_linked().await {
        Ok(Some(linked)) => linked,
        Ok(None) => return error(&headers, StatusCode::CONFLICT, "connect_not_linked", "esta máquina não está ligada ao Connect"),
        Err(failure) => return error(&headers, failure.status, failure.code, &failure.msg),
    };
    let machines = match peers::account_machines(&linked).await {
        Ok(machines) => machines,
        Err(failure) => return error(&headers, failure.status, failure.code, &failure.msg),
    };
    let book = match crate::machine_peers::Service::from_state(&st) {
        Ok(service) => service.store.tolerant(),
        Err(failure) => return error(&headers, StatusCode::SERVICE_UNAVAILABLE, failure.code, &failure.msg),
    };
    let rows: Vec<Value> = machines.into_iter().map(|machine| {
        let address = format!("https://{}", machine.host);
        let added = (&book).into_iter().any(|(key, entry)| *key == machine.machine_id
            || entry["base_url"].as_str().is_some_and(|url| url.trim_end_matches('/') == address));
        json!({"machine_id": machine.machine_id, "name": machine.name, "host": machine.host, "online": machine.online, "added": added})
    }).collect();
    reply(&headers, StatusCode::OK, json!({"this": {"machine_id": linked.machine_id, "host": linked.host}, "machines": rows}))
}

/// Volta na hora com 202. Só a senha, como `link`: esta máquina emite uma credencial sem cartão dela, e
/// aparelho que pedisse isso deixaria credencial fora da cascata de quem o aprovou.
async fn link_peer(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, Path(id): Path<String>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Some(response) = owner_secret_refusal(&req) {
        return response;
    }
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    let linked = match st.connect_supervisor.read_linked().await {
        Ok(Some(linked)) => linked,
        Ok(None) => return error(&headers, StatusCode::CONFLICT, "connect_not_linked", "esta máquina não está ligada ao Connect"),
        Err(failure) => return error(&headers, failure.status, failure.code, &failure.msg),
    };
    let server_id = st.current_server_id();
    if server_id.is_empty() {
        return error(&headers, StatusCode::CONFLICT, "connect_peer_no_identity", "esta máquina não tem identificador");
    }
    // Antes do bilhete: recusa depois do cartão gastaria a aprovação do dono à toa.
    if !valid_label(&id) || valid_id(&json!(id)).is_err() || id == linked.machine_id || id == server_id {
        return error(&headers, StatusCode::BAD_REQUEST, "connect_peer_invalid", "máquina inválida");
    }
    let store = match crate::machine_peers::Service::from_state(&st) {
        Ok(service) => service.store,
        Err(failure) => return error(&headers, StatusCode::SERVICE_UNAVAILABLE, failure.code, &failure.msg),
    };
    let expected = peers::expected_host(&linked, &id).map(|host| format!("https://{host}"));
    let book = store.tolerant();
    if book.get(&id).and_then(|entry| entry["base_url"].as_str()).is_some_and(|known| Some(known) != expected.as_deref()) {
        return error(&headers, StatusCode::CONFLICT, "peers_other_address", "essa máquina já está em outro endereço");
    }
    drop(book);
    let linker = PeerLinker { linked, devices: st.connect_devices.clone(), server_id, links: st.connect_supervisor.peer_links().clone(),
        save: Arc::new(move |entry: &Value| store.put_from(entry, entry["connect_return_device"].as_str()).map(|_| ()).map_err(|failure| {
            ConnectError::new(StatusCode::from_u16(failure.status).unwrap_or(StatusCode::SERVICE_UNAVAILABLE), failure.code, failure.msg)
        })) };
    if !linker.start(id.clone()) {
        return error(&headers, StatusCode::CONFLICT, "connect_peer_linking", "esta ligação já está em curso");
    }
    reply(&headers, StatusCode::ACCEPTED, json!({"machine_id": id, "stage": "requesting"}))
}

async fn forget(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    if let Some(response) = owner_secret_refusal(&req) {
        return response;
    }
    if let Err(response) = owner_gate(&st, peer, &req).await {
        return with_cors(&headers, response);
    }
    st.connect_previews.close();
    let supervisor = st.connect_supervisor.clone();
    let outcome = tokio::spawn(async move { supervisor.forget().await }).await;
    finished(&st, &headers, outcome)
}
