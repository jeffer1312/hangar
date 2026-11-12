//! Borda HTTP dos atalhos; validação e efeitos não voltam ao Python.
use crate::{routes::AppState, orchestration::codec::{self, ValueGuard}};
use axum::{body::{Body, Bytes}, extract::Request, http::{Method, StatusCode}, response::{IntoResponse, Response}};
use hangar_api::session::SessionRow;
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};

const ROUTES: &[(&str, &str)] = &[
    ("/api/sessions/{name}/project-shortcuts", "GET"),
    ("/api/sessions/{name}/project-shortcuts", "PUT"),
    ("/api/sessions/{name}/shortcut-shell", "POST"),
    ("/api/sessions/{name}/run-code", "POST"),
    ("/api/hangar-terminals/{id}/restart", "POST"),
];
pub(crate) fn matches(path: &str) -> bool {
    let path = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
    path.strip_prefix("/api/sessions/").and_then(|v|v.split_once('/'))
        .is_some_and(|(_, tail)| matches!(tail.split('/').next(),Some("project-shortcuts"|"shortcut-shell"|"run-code")))
        || path.strip_prefix("/api/hangar-terminals/").and_then(|v|v.split_once('/'))
            .is_some_and(|(_,tail)|tail.split('/').next()==Some("restart"))
}
fn unavailable() -> Response {
    super::guest::error(503, "shortcut_context_unavailable", "contexto do terminal indisponível", json!({}))
}
fn busy() -> Response { crate::session_write::json_response(StatusCode::CONFLICT, crate::session_write::busy_body()) }

pub(crate) async fn rows(st: &Arc<AppState>) -> Result<Vec<SessionRow>, Response> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|_|unavailable())?.as_secs_f64();
    let rows = st.list.discover(Some(now)).await.map_err(|_|unavailable())?;
    let home = st.accounts.env.home.clone();
    tokio::task::spawn_blocking(move || {
        let (orq, failed) = crate::list::local_facts::orq_rows(&home, now);
        if failed.is_some() { return Err(unavailable()); }
        Ok(rows.iter().chain(orq.iter()).cloned().collect())
    }).await.map_err(|_|unavailable())?
}
pub(crate) async fn blocked(st: &AppState, rows: Vec<SessionRow>, name: &str) -> Result<bool, Response> {
    crate::list::local_transfers::transfer_active(st.list.env(), name, rows).await.map_err(|_| unavailable())
}
fn find(rows: &[SessionRow], name: &str) -> Option<SessionRow> { rows.iter().find(|row| row.name == name).cloned() }

pub async fn public(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let headers = req.headers().clone();
    let method = req.method().clone();
    let mut response = inner(st, peer, req).await;
    crate::http_cors::apply(&headers,&mut response);
    let body=std::mem::replace(response.body_mut(),Body::empty());
    let bytes=match axum::body::to_bytes(body,usize::MAX).await {
        Ok(bytes)=>bytes,Err(_)=>return crate::http_cors::internal_error(),
    };
    let mut body=bytes.to_vec();
    let sse=response.headers().get("content-type").and_then(|v|v.to_str().ok()).is_some_and(|v|v.starts_with("text/event-stream"));
    // Sucesso no legado vinha em fluxo, que não aplica o mínimo de compressão.
    let minimum=if response.status().is_success() && method!=Method::OPTIONS {0}else{1024};
    if !body.is_empty() && body.len()>=minimum && !sse && !response.headers().contains_key("content-encoding")
        && response.extensions().get::<crate::http_cors::UnhandledServerError>().is_none() {
        let previous=response.headers().get("vary").map(|v|v.as_bytes().to_vec()).unwrap_or_default();
        body=crate::routes::gzip_with_minimum(&headers,response.headers_mut(),body,minimum);
        let mut vary=previous;
        if !vary.is_empty() { vary.extend_from_slice(b", "); }
        vary.extend_from_slice(b"Accept-Encoding");
        response.headers_mut().insert("vary",axum::http::HeaderValue::from_bytes(&vary).unwrap());
        if response.headers().contains_key("content-encoding") {
            if minimum==0 { response.headers_mut().remove("content-length"); }
            else { response.headers_mut().insert("content-length",axum::http::HeaderValue::from_str(&body.len().to_string()).unwrap()); }
        }
    }
    let chunked=minimum==0 && response.headers().contains_key("content-encoding");
    *response.body_mut()=if chunked {
        Body::from_stream(futures_util::stream::once(async move { Ok::<_,std::convert::Infallible>(body) }))
    } else { Body::from(body) };
    crate::http_route_response::suppress_head(&method,&mut response);
    response
}
async fn inner(st: Arc<AppState>, peer: SocketAddr, req: Request) -> Response {
    let forward = crate::routes::forward_for(&st, peer, &req);
    let path = percent_encoding::percent_decode_str(req.uri().path()).decode_utf8_lossy().into_owned();
    let decision = crate::http_contract::resolve(ROUTES, req.method().as_str(), &path);
    if let Some(response) = crate::http_route_response::response(&decision, req.method(), req.uri(), req.headers(), forward.https, "") { return response; }
    if let Some(id)=path.strip_prefix("/api/hangar-terminals/").and_then(|v|v.strip_suffix("/restart")) {
        if let Err(response)=crate::routes::owner_gate(&st,peer,&req).await { return response; }
        return match super::guest::restart(&st,id.to_owned()).await { Ok(response)|Err(response)=>response };
    }
    let (name, operation) = path.strip_prefix("/api/sessions/").unwrap().split_once('/').unwrap();
    let name = name.to_owned();
    let operation = operation.to_owned();
    let (parts,body) = req.into_parts();
    let mutation = parts.method != Method::GET;
    let bytes = if mutation { match crate::http_body_limit::read(body).await {
        Ok(bytes) => bytes,
        Err(_) => return (StatusCode::BAD_REQUEST, "There was an error parsing the body").into_response(),
    }} else { Bytes::new() };
    if mutation && crate::http_body_input::is_json(&parts.headers) && !bytes.is_empty() {
        match crate::orchestration::body::parse(&bytes) {
            Ok(value) => codec::discard(value),
            Err((status,value)) => return crate::http_cors::json_guarded(status,value),
        }
    }
    let req = Request::from_parts(parts, Body::empty());
    let (_,caller) = match crate::routes::who(&st,peer,&req).await { Ok(v)=>v, Err(e)=>return crate::auth::unavailable(e) };
    if !caller.as_ref().is_some_and(|c|c.can(&name,req.method())) {
        return match crate::routes::owner_gate(&st,peer,&req).await {
            Err(response)=>response,
            Ok(_)=>super::guest::error(403,"erro_fora_do_convite","fora da sessao compartilhada",json!({})),
        };
    }
    let discovered = match rows(&st).await { Ok(rows)=>rows, Err(response)=>return response };
    let mut row = find(&discovered,&name);
    let ingress = if mutation {
        match blocked(&st,discovered,&name).await { Ok(true)=>return busy(), Ok(false)=>{}, Err(response)=>return response }
        let Some(runtime) = st.state.runtime.get() else { return unavailable(); };
        let pass = match runtime.ingress().enter(&name,st.write_gate_wait).await { Ok(pass)=>Arc::new(pass), Err(_)=>return busy() };
        let current = match rows(&st).await { Ok(rows)=>rows, Err(response)=>return response };
        let next = find(&current,&name);
        if !crate::list::local_transfers::same_life(row.as_ref(),next.as_ref()) { return busy(); }
        match blocked(&st,current,&name).await { Ok(true)=>return busy(), Ok(false)=>{}, Err(response)=>return response }
        row = next;
        Some(pass)
    } else { None };
    let schema = match operation.as_str() {
        "shortcut-shell"=>super::body::Schema::Shell,
        "run-code"=>super::body::Schema::RunCode,
        _=>super::body::Schema::Project,
    };
    let value = if mutation { match super::body::parse(schema,&bytes,req.headers()) {
        Ok(value)=>ValueGuard(value), Err(response)=>return response,
    }} else { ValueGuard(Value::Null) };
    let guest = req.extensions().get::<crate::login_guests::gate::Login>().map(|login|login.guest.clone());
    let is_guest = !matches!(caller,Some(crate::share::caller::Caller::Owner));
    // Cancelar a conexão não libera a transferência enquanto a escrita aceita ainda tem efeitos.
    tokio::spawn(async move {
        let _ingress = ingress;
        if operation!="project-shortcuts" {
            return match super::guest::execute(&st,guest.as_ref(),is_guest,&name,row,operation=="run-code",&value).await {
                Ok(response)|Err(response)=>response,
            };
        }
        let Some(row) = row.filter(|row| row.cwd.as_deref().is_some_and(|p|!p.is_empty())) else {
            return super::guest::error(404,"erro_sessao_inexistente","sessao nao encontrada",json!({}));
        };
        let project = match super::project::describe(&st,row.cwd.as_deref().unwrap()).await { Ok(v)=>v, Err(response)=>return response };
        let items = mutation.then(||codec::clone_value(&value["items"]));
        match tokio::task::spawn_blocking(move || super::project::items(&st,project,items)).await {
            Ok(Ok(value))=>crate::http_cors::json_guarded(StatusCode::OK,value),
            Ok(Err(response))=>response,
            Err(_)=>crate::http_cors::internal_error(),
        }
    }).await.unwrap_or_else(|_|unavailable())
}
