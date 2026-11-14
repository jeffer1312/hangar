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
    fn send_bound(&self, session: String, _key: Option<String>, text: String) -> std::pin::Pin<Box<dyn std::future::Future<Output=()> + Send>> {self.send(session,text)}
    fn groups(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<crate::model::Group>, Error>> + Send>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn fetch_evidence(&self, _server: String, _path: String) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<(String,Vec<u8>),Error>> + Send>> {
        Box::pin(async {Err(Error::new(Kind::Unavailable,"ticket_remote_unavailable","a outra máquina não está disponível"))})
    }
    fn identity(&self, _session: String) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<Option<String>,Error>> + Send>> {
        Box::pin(async {Ok(None)})
    }
    fn adopt(&self, store: Arc<Store>, connector: String, ticket: String, link: Link, expected: Option<String>) -> std::pin::Pin<Box<dyn std::future::Future<Output=Result<(crate::model::Item,bool),Error>> + Send + '_>> {
        Box::pin(async move {
            let key=self.identity(link.session.clone()).await?;
            if expected.is_some() && expected!=key {return Err(Error::new(Kind::Gate,"ticket_session_changed","a sessão mudou desde a escolha; confira o ticket novamente"));}
            let groups=self.groups().await?;
            tokio::task::spawn_blocking(move||store.link_owner(&connector,&ticket,link,key.as_deref(),&groups)).await
                .unwrap_or_else(|_|Err(Error::new(Kind::Unavailable,"mission_unavailable","associação interrompida")))
        })
    }
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

fn notify(state: &Http, item: &crate::model::Item, text: String) {
    if let Some(session)=&item.session {tokio::spawn(state.notify.send_bound(session.clone(),item.session_key.clone().or_else(||item.session_keys.get(session).cloned()),text));}
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
    key: Option<String>,
    #[serde(default)]
    group_id: Option<String>,
    #[serde(default)]
    source_server: Option<String>,
    #[serde(default)]
    connector: Option<String>,
    #[serde(default)]
    ticket_id: Option<String>,
    #[serde(default)]
    history_revision: Option<usize>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OwnerLink { key: String, title: String, url: String, session: String, #[serde(default)] session_key: Option<String> }

/// Trabalho de disco (cópia de evidência, regravação) fora do executor.
async fn blocking<T: Send + 'static>(state: &Http, work: impl FnOnce(&Store) -> Result<T, Error> + Send + 'static) -> Result<T, Error> {
    let store = state.store.clone();
    tokio::task::spawn_blocking(move || work(&store)).await
        .unwrap_or_else(|_| Err(Error::new(Kind::Unavailable, "mission_unavailable", "tarefa interrompida")))
}

async fn current_snapshot(state: &Http) -> Result<crate::model::Snapshot, Error> {
    let groups = state.notify.groups().await?;
    blocking(state,move |store|store.sync_groups(&groups)).await
}

async fn session_item(state: &Http, session: String, key: Option<String>, group: Option<String>) -> Result<serde_json::Value,Error> {
    let snapshot=current_snapshot(state).await?;
    let weak=snapshot.items.iter().any(|item|item.sessions.contains(&session) && !item.session_keys.contains_key(&session));
    let matches=snapshot.items.into_iter().filter(|item| {
        let member=item.sessions.contains(&session) || group.as_ref().is_some_and(|group|item.group_ids.contains(group));
        let confirmed=item.session_keys.get(&session).map_or_else(||key.is_none() || group.as_ref().is_some_and(|group|item.group_ids.contains(group)),|expected|key.as_ref()==Some(expected));
        member && confirmed
    }).collect::<Vec<_>>();
    let active=matches.iter().filter(|item|item.delivered_at.is_none()).collect::<Vec<_>>();
    if active.len()>1 {return Err(Error::new(Kind::Gate,"ticket_group_conflict","o grupo está associado a tickets diferentes; remova a associação incorreta no board"));}
    let mut item=active.first().map(|item|(*item).clone()).or_else(||matches.last().cloned())
        .ok_or_else(||if weak && key.is_some() {Error::new(Kind::Gate,"ticket_session_identity_required","confirme a associação da sessão na tela do ticket")} else {Error::new(Kind::NotFound,"mission_session_not_linked","esta sessão não está ligada a um ticket")})?;
    if let Some(key)=key && !item.session_keys.contains_key(&session) {
        let (connector,ticket)=(item.connector.clone(),item.ticket_id.clone());
        item=blocking(state,move|store|store.bind_participant(&connector,&ticket,&session,&key)).await?;
    }
    Ok(json!({"item":item,"stages":snapshot.stages}))
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
        (Method::GET, ["api", "mission"]) => snapshot(current_snapshot(state).await),
        (Method::GET, ["api", "mission", "session"]) => {
            let query=form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes()).into_owned().collect::<std::collections::HashMap<_,_>>();
            match session_item(state,query.get("session").cloned().unwrap_or_default(),query.get("key").cloned(),query.get("group_id").cloned()).await {
                Ok(value)=>json_response(StatusCode::OK,value),Err(error)=>failure(&error),
            }
        }
        (Method::GET, ["api", "mission", "source"]) => {
            let path=form_urlencoded::parse(request.uri().query().unwrap_or_default().as_bytes()).find(|(key,_)|key=="path").map(|(_,value)|PathBuf::from(value.as_ref())).unwrap_or_default();
            match blocking(state,move|store|store.source_file(&path)).await {
                Ok((file,name,mime))=>hangar_http_file::serve_open_file(tokio::fs::File::from_std(file),std::path::Path::new(&name),request.headers(),false,Some(&mime)).await,
                Err(error)=>failure(&error),
            }
        }
        (Method::PUT, ["api", "mission", "stages"]) => {
            let body: StagesBody = match body(request).await { Ok(body) => body, Err(response) => return response };
            snapshot(blocking(state, move |store| store.set_stages(body.stages)).await)
        }
        (Method::POST, ["api", "mission", "report"]) => {
            let body: ReportBody = match body(request).await { Ok(body) => body, Err(response) => return response };
            let paths=body.evidence;
            let source=body.source_server;
            let report = Report { stage: body.stage, note: body.note, evidence: if source.is_some() {Vec::new()} else {paths.iter().map(|e| (e.path.clone(), e.caption.clone())).collect()} };
            let session = body.session;
            let reporter_key=body.key.clone();
            let linked=match session_item(state,session.clone(),body.key,body.group_id).await {
                Ok(value)=>serde_json::from_value::<crate::model::Item>(value["item"].clone()).expect("item serializado pelo mesmo módulo"),
                Err(error)=>return failure(&error),
            };
            let binding=crate::store::ReportBinding {session_key:reporter_key,history_revision:body.history_revision.unwrap_or(linked.history.len())};
            if body.connector.as_ref().is_some_and(|connector|connector!=&linked.connector) || body.ticket_id.as_ref().is_some_and(|ticket|ticket!=&linked.ticket_id) {
                return failure(&Error::new(Kind::Gate,"ticket_association_changed","a associação mudou desde a consulta; confira o ticket antes de relatar"));
            }
            let mut imported=Vec::new();
            if let Some(source)=source {
                if !session.starts_with(&format!("{source}::")) {return failure(&Error::invalid("máquina de origem não corresponde à sessão"));}
                if paths.len()+linked.evidence.len()>200 {return failure(&Error::invalid("no máximo 200 evidências por ticket"));}
                for evidence in paths {
                    let (name,bytes)=match state.notify.fetch_evidence(source.clone(),evidence.path.to_string_lossy().into_owned()).await {Ok(value)=>value,Err(error)=>return failure(&error)};
                    match blocking(state,move|store|store.import_evidence(&name,&bytes,&evidence.caption)).await {Ok(entry)=>imported.push(entry),Err(error)=>return failure(&error)}
                }
            }
            item(blocking(state, move |store| store.report_imported(&linked.connector,&linked.ticket_id,&session,report,imported,Some(binding))).await)
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
                    let body:OwnerLink=match body(request).await {Ok(body)=>body,Err(response)=>return response};
                    let link=Link {key:body.key,title:body.title,url:body.url,session:body.session};
                    let linked=state.notify.adopt(state.store.clone(),connector,ticket,link,body.session_key).await;
                    if let Ok((linked, true)) = &linked { notify(state, linked, linked_notice(&linked.key)); }
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
                        notify(state, changed, format!("Ajustes pedidos pelo dono no ticket {}, na etapa {refused}: {note}", changed.key));
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
