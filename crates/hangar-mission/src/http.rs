//! Rotas dos tickets do dono. Autenticação antes de ler corpo ou arquivo.
use crate::store::{Error, Kind, Link, OwnerChange, Report, Store};
use axum::Router;
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::Response;
use hangar_core::edge::Edge;
use hangar_core::http::{detail, json_response};
use serde::Deserialize;
use serde_json::json;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

const MAX_BODY: usize = 256 * 1024;

/// Entrega de texto a uma sessão (a mesma entrada que o dono usa). O servidor a implementa; falha não desfaz a mudança.
pub trait Notify: Send + Sync + 'static {
    fn send(&self, session: String, text: String) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>>;
}

struct Http { edge: Edge, store: Arc<Store>, notify: Arc<dyn Notify> }

pub fn router<S: Clone + Send + Sync + 'static>(edge: Edge, store: Arc<Store>, notify: Arc<dyn Notify>) -> Router<S> {
    Router::new()
        .route("/api/mission", axum::routing::any(public))
        .route("/api/mission/{*rest}", axum::routing::any(public))
        .with_state(Arc::new(Http { edge, store, notify }))
}

/// O que a sessão ligada recebe: como informar etapa e evidências.
fn linked_notice(key: &str) -> String {
    format!("[painel: Tickets] Esta sessão trabalha o ticket {key}. Ao mudar de etapa, informe pela tool `mission` do MCP \
hangar (sem argumentos ela mostra o ticket e as etapas). Na validação, anexe as evidências de funcionamento (prints, vídeos, \
relatórios) pela mesma tool e aguarde a aprovação do dono antes de seguir.")
}

fn notify(state: &Http, session: Option<&str>, text: String) {
    if let Some(session) = session { tokio::spawn(state.notify.send(session.to_owned(), text)); }
}

async fn public(
    State(state): State<Arc<Http>>,
    peer: Result<ConnectInfo<SocketAddr>, axum::extract::rejection::ExtensionRejection>,
    request: Request,
) -> Response {
    if let Some(response) = hangar_core::http_cors::preflight(request.method(), request.headers()) {
        return response;
    }
    let headers = request.headers().clone();
    let mut response = reply(&state, peer.ok().map(|peer| peer.0), request).await;
    hangar_core::http_cors::apply(&headers, &mut response);
    response
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidencePath { path: PathBuf, #[serde(default)] caption: String }

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportBody {
    session: String,
    #[serde(default)]
    stage: Option<String>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    evidence: Vec<EvidencePath>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StagesBody { stages: Vec<crate::model::Stage> }

/// Trabalho de disco (cópia de evidência, regravação) fora do executor.
async fn blocking<T: Send + 'static>(state: &Http, work: impl FnOnce(&Store) -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || work(&store)).await
        .unwrap_or_else(|_| Err(Error::new(Kind::Unavailable, "mission_unavailable", "tarefa interrompida")))
}

async fn reply(state: &Http, peer: Option<SocketAddr>, request: Request) -> Response {
    let socket = peer.unwrap_or_else(|| "0.0.0.0:0".parse().expect("endereço estático válido"));
    if let Err(response) = state.edge.owner_only(socket, &request, ("mission_owner_only", "só o dono administra os tickets")).await {
        return response;
    }
    let segments: Vec<String> = request.uri().path().trim_start_matches('/').split('/')
        .map(|part| percent_encoding::percent_decode_str(part).decode_utf8_lossy().into_owned()).collect();
    let segments: Vec<&str> = segments.iter().map(String::as_str).collect();
    let method = request.method().clone();
    let item = |result: Result<crate::model::Item, Error>| match result {
        Ok(item) => json_response(StatusCode::OK, json!({"item": item})),
        Err(error) => failure(&error),
    };
    let snapshot = |result: Result<crate::model::Snapshot, Error>| match result {
        Ok(snapshot) => json_response(StatusCode::OK, json!(snapshot)),
        Err(error) => failure(&error),
    };
    match (method, &segments[..]) {
        (Method::GET, ["api", "mission"]) => snapshot(blocking(state, |store| store.snapshot()).await),
        (Method::PUT, ["api", "mission", "stages"]) => {
            let body: StagesBody = match body(request).await { Ok(body) => body, Err(response) => return response };
            snapshot(blocking(state, move |store| store.set_stages(body.stages)).await)
        }
        (Method::POST, ["api", "mission", "report"]) => {
            let body: ReportBody = match body(request).await { Ok(body) => body, Err(response) => return response };
            let report = Report { stage: body.stage, note: body.note, evidence: body.evidence.into_iter().map(|e| (e.path, e.caption)).collect() };
            let session = body.session;
            item(blocking(state, move |store| store.report(&session, report)).await)
        }
        (Method::GET, ["api", "mission", "evidence", id]) => {
            let id = (*id).to_owned();
            let found = blocking(state, move |store| store.evidence_file(&id)).await;
            match found {
                Ok((path, evidence)) => match tokio::fs::File::open(&path).await {
                    Ok(file) => hangar_http_file::serve_open_file(file, std::path::Path::new(&evidence.name), request.headers(), false, Some(&evidence.mime)).await,
                    Err(_) => detail(StatusCode::NOT_FOUND, "mission_evidence_not_found", "arquivo da evidência sumiu", json!({})),
                },
                Err(error) => failure(&error),
            }
        }
        (method, ["api", "mission", "items", connector, ticket, rest @ ..]) => {
            let (connector, ticket) = ((*connector).to_owned(), (*ticket).to_owned());
            match (method, rest) {
                (Method::PUT, []) => {
                    let link: Link = match body(request).await { Ok(body) => body, Err(response) => return response };
                    let linked = blocking(state, move |store| store.link(&connector, &ticket, link)).await;
                    if let Ok((linked, true)) = &linked { notify(state, linked.session.as_deref(), linked_notice(&linked.key)); }
                    item(linked.map(|(linked, _)| linked))
                }
                (Method::PATCH, []) => {
                    let change: OwnerChange = match body(request).await { Ok(body) => body, Err(response) => return response };
                    let note = match &change { OwnerChange::Changes { note, .. } => Some(note.clone()), _ => None };
                    let changed = blocking(state, move |store| {
                        let item = store.owner_change(&connector, &ticket, change)?;
                        let stages = store.snapshot()?.stages;
                        Ok((item, stages))
                    }).await;
                    // O pedido de ajustes vai à sessão só depois de gravado, com o nome da etapa recusada.
                    if let (Ok((changed, stages)), Some(note)) = (&changed, note) {
                        let refused = changed.history.last().and_then(|e| e.stage.as_deref())
                            .and_then(|id| stages.iter().find(|s| s.id == id)).map_or("", |s| s.name.as_str());
                        notify(state, changed.session.as_deref(), format!("Ajustes pedidos pelo dono no ticket {}, na etapa {refused}: {note}", changed.key));
                    }
                    item(changed.map(|(changed, _)| changed))
                }
                (Method::DELETE, []) => item(blocking(state, move |store| store.remove(&connector, &ticket)).await),
                (Method::POST, ["evidence"]) => {
                    if let Some(refused) = hangar_core::http_body_limit::precheck_with_limit(request.headers(), crate::store::MAX_UPLOAD_BYTES) {
                        return refused;
                    }
                    let mut name = String::new();
                    let mut caption = String::new();
                    for (key, value) in form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes()) {
                        match key.as_ref() { "name" => name = value.into_owned(), "caption" => caption = value.into_owned(), _ => {} }
                    }
                    let Ok(bytes) = to_bytes(request.into_body(), crate::store::MAX_UPLOAD_BYTES).await else {
                        return detail(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", "evidência grande demais", json!({}));
                    };
                    item(blocking(state, move |store| store.add_owner_evidence(&connector, &ticket, &name, &bytes, &caption)).await)
                }
                (Method::DELETE, ["evidence", id]) => {
                    let id = (*id).to_owned();
                    item(blocking(state, move |store| store.remove_evidence(&connector, &ticket, &id)).await)
                }
                _ => detail(StatusCode::NOT_FOUND, "not_found", "rota desconhecida", json!({})),
            }
        }
        _ => detail(StatusCode::NOT_FOUND, "not_found", "rota desconhecida", json!({})),
    }
}

async fn body<T: serde::de::DeserializeOwned>(request: Request) -> Result<T, Response> {
    let bytes = to_bytes(request.into_body(), MAX_BODY).await
        .map_err(|_| detail(StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large", "corpo acima de 256 KiB", json!({})))?;
    serde_json::from_slice::<T>(&bytes)
        .map_err(|error| detail(StatusCode::BAD_REQUEST, "invalid_params", &format!("corpo inválido: {error}"), json!({})))
}

fn failure(error: &Error) -> Response {
    let status = match error.kind {
        Kind::Invalid => StatusCode::BAD_REQUEST,
        Kind::NotFound => StatusCode::NOT_FOUND,
        Kind::Gate => StatusCode::CONFLICT,
        Kind::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    };
    detail(status, error.code, &error.message, json!({}))
}
