//! A aplicação pertence à tarefa do servidor, mesmo após desconexão do cliente.
use super::{apply::Apply, bundle::{Bundle, Error}, http};
use crate::{routes::{AppState, owner_gate, cors}, session_write::json_response};
use axum::{body::{Body, to_bytes}, extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::{IntoResponse, Response}};
use serde_json::{Value, json};
use std::{convert::Infallible, net::SocketAddr, sync::Arc};

static APPLYING: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[cfg(test)]
#[path = "apply_http_tests.rs"]
mod tests;

fn busy() -> Error {
    Error::new("config_sync_busy", "outra aplicação está em andamento nesta máquina", json!({}))
}

pub async fn handle(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let headers = request.headers().clone();
    let mut response = handle_inner(state, peer, request).await;
    cors(&headers, response.headers_mut());
    response
}

async fn handle_inner(state: Arc<AppState>, peer: SocketAddr, request: Request) -> Response {
    if request.method() != Method::POST {
        let mut response = json_response(StatusCode::METHOD_NOT_ALLOWED, json!({"detail":"Method Not Allowed"}));
        response.headers_mut().insert("allow", "POST".parse().unwrap());
        return response;
    }
    if let Err(response) = owner_gate(&state, peer, &request).await { return response; }
    let stream = match crate::auth::query_param(request.uri().query(), "stream") {
        None => false,
        Some(value) => match value.to_ascii_lowercase().as_str() {
            "1" | "true" | "t" | "yes" | "y" | "on" => true,
            "0" | "false" | "f" | "no" | "n" | "off" => false,
            _ => return json_response(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail":[{
                "type":"bool_parsing", "loc":["query","stream"],
                "msg":"Input should be a valid boolean, unable to interpret input", "input":value
            }]})),
        }
    };
    let chosen = match http::items(&crate::auth::query_param(request.uri().query(), "items").unwrap_or_default()) {
        Ok(chosen) => chosen,
        Err(error) => return http::failure(StatusCode::BAD_REQUEST, error),
    };
    if APPLYING.try_lock().is_err() { return http::failure(StatusCode::CONFLICT, busy()); }
    let raw = match to_bytes(request.into_body(), usize::MAX).await {
        Ok(raw) => raw,
        Err(_) => return http::failure(StatusCode::BAD_REQUEST, Error::invalid("pacote ilegível")),
    };
    let bundle = match tokio::task::spawn_blocking(move || Bundle::unpack(&raw)).await {
        Ok(Ok(bundle)) => bundle,
        Ok(Err(error)) => return http::failure(StatusCode::BAD_REQUEST, error),
        Err(_) => return http::failure(StatusCode::INTERNAL_SERVER_ERROR, Error::invalid("falha ao ler o pacote")),
    };
    if !stream {
        let task = tokio::spawn(async move { work(state, bundle, chosen, None).await });
        return match task.await.unwrap_or_else(|_| Err((StatusCode::INTERNAL_SERVER_ERROR,
            Error::new("config_sync_failed", "falha ao aplicar a configuração", json!({}))))) {
            Ok(result) => json_response(StatusCode::OK, result),
            Err((status,error)) => http::failure(status,error),
        };
    }
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let _final_event = FinalEvent(sender.clone());
        let event = match work(state, bundle, chosen, Some(sender.clone())).await {
            Ok(result) => json!({"type":"done","result":result}),
            Err((status,error)) => json!({"type":"error","status":status.as_u16(),
                "detail":crate::session_write::detail_body(error.code,&error.message,error.params)["detail"]}),
        };
        let _ = sender.send(event);
    });
    let lines = futures_util::stream::unfold((receiver,false), |(mut receiver,done)| async move {
        if done { return None; }
        let mut event = receiver.recv().await?;
        let final_event = event.get("type").is_some();
        if !final_event {
            let mut progress = serde_json::Map::new();
            progress.insert("type".into(),json!("progress"));
            progress.extend(event.as_object_mut().map(std::mem::take).unwrap_or_default());
            event = Value::Object(progress);
        }
        let line = format!("{}\n",crate::transcript::pyjson::dumps_unicode(&event,false));
        Some((Ok::<_,Infallible>(line),(receiver,final_event)))
    });
    ([("content-type","application/x-ndjson"),("content-encoding","identity"),("cache-control","no-store")], Body::from_stream(lines)).into_response()
}

struct FinalEvent(tokio::sync::mpsc::UnboundedSender<Value>);
impl Drop for FinalEvent {
    fn drop(&mut self) { let _ = self.0.send(json!({"type":"error","status":499,"detail":"interrompido"})); }
}

async fn work(state: Arc<AppState>, bundle: Bundle, chosen: Vec<String>, progress: Option<tokio::sync::mpsc::UnboundedSender<Value>>) -> Result<Value,(StatusCode,Error)> {
    let _guard = APPLYING.try_lock().map_err(|_|(StatusCode::CONFLICT,busy()))?;
    let mut context = http::context(&state);
    context.progress = progress;
    let stamp = format!("{}-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), crate::mods::state::random_hex(3));
    let programs = ["claude","codex"].into_iter().filter_map(|name| {
        super::cli_probe::resolve_program(name,&state.accounts.env).map(|path|(name.into(),path.to_string_lossy().into_owned()))
    }).collect();
    let backups = std::path::Path::new(&context.roots.home).join(".hangar/config-sync/backups").join(stamp);
    let apply = Apply {
        context, bundle, backups, programs, report: Default::default(),
        accounts: state.accounts.clone(), runtime_config: super::runtime::RuntimeConfig::from_state(&state),
    };
    let result = super::apply_run::run_items(apply,&chosen).await
        .map_err(|error|(StatusCode::INTERNAL_SERVER_ERROR,error))?;
    Ok(result.report())
}
