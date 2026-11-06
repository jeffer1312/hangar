use axum::{extract::Request, http::{HeaderMap, StatusCode}, response::Response};
use serde_json::{Value, json};
use std::{path::Path, sync::{Arc, LazyLock}, time::Duration};
use regex::Regex;
use unicode_normalization::UnicodeNormalization;
use crate::{http_cors::{internal_error, json_guarded}, orchestration::{body, codec}, routes::AppState};

fn issue(kind: &str, location: Value, input: &Value) -> Value {
    let message = match kind {
        "missing" => "Field required",
        "model_attributes_type" => "Input should be a valid dictionary or object to extract fields from",
        "string_type" => "Input should be a valid string",
        "string_unicode" => "Input should be a valid string, unable to parse raw data as a unicode string",
        "string_too_short" => "String should have at least 1 character",
        "extra_forbidden" => "Extra inputs are not permitted",
        _ => unreachable!(),
    };
    let mut value = json!({"type":kind,"loc":location,"msg":message});
    value.as_object_mut().unwrap().insert("input".into(),codec::clone_value(input));
    if kind == "string_too_short" { value["ctx"] = json!({"min_length":1}); }
    value
}

fn validate(bytes: &[u8], headers: &HeaderMap) -> Result<String, Response> {
    let value = codec::ValueGuard(if bytes.is_empty() { Value::Null }
        else if crate::http_body_input::is_json(headers) {
            body::parse(bytes).map_err(|(status,error)|json_guarded(status,error))?
        } else {
            Value::String(std::str::from_utf8(bytes).map_err(|_|internal_error())?.to_owned())
        });
    let Some(object) = value.as_object() else {
        let error = issue(if value.is_null(){"missing"}else{"model_attributes_type"},json!(["body"]),&value);
        let mut result = serde_json::Map::new();
        result.insert("detail".into(),Value::Array(vec![error]));
        return Err(json_guarded(StatusCode::UNPROCESSABLE_ENTITY,Value::Object(result)));
    };
    let mut errors = Vec::new();
    match object.get("repo") {
        None => errors.push(issue("missing",json!(["body","repo"]),&value)),
        Some(input) => {
            let kind = if !input.is_string() || codec::contains_nonfinite(input) { Some("string_type") }
                else if codec::contains_surrogate(input) { Some("string_unicode") }
                else if input.as_str() == Some("") { Some("string_too_short") } else { None };
            if let Some(kind) = kind { errors.push(issue(kind,json!(["body","repo"]),input)); }
        }
    }
    for (key,input) in object {
        if key != "repo" { errors.push(issue("extra_forbidden",json!(["body",key]),input)); }
    }
    if errors.is_empty() { Ok(object["repo"].as_str().unwrap().to_owned()) }
    else {
        let mut error = serde_json::Map::new();
        error.insert("detail".into(),Value::Array(errors));
        Err(json_guarded(StatusCode::UNPROCESSABLE_ENTITY,Value::Object(error)))
    }
}

fn valid_port(authority: &str) -> bool {
    let stripped = authority.replace(['@',':','#','?'],"");
    if stripped.nfkc().any(|ch| matches!(ch,'/'|'?'|'#'|'@'|':')) { return false; }
    let port = if let Some(rest) = authority.strip_prefix('[') {
        let Some((host,tail)) = rest.split_once(']') else { return false };
        let valid_host = host.split('%').next().is_some_and(|ip| ip.parse::<std::net::Ipv6Addr>().is_ok())
            || host.strip_prefix('v').and_then(|rest|rest.split_once('.')).is_some_and(|(version,name)|!version.is_empty() && version.chars().all(|c|c.is_ascii_hexdigit()) && !name.is_empty());
        if !valid_host { return false; }
        if tail.is_empty() { None } else if let Some(port) = tail.strip_prefix(':') { Some(port) } else { return false; }
    } else {
        if authority.contains(['[',']']) { return false; }
        authority.split_once(':').map(|(_,port)|port)
    };
    port.is_none_or(|port|port.is_empty() || port.bytes().all(|c|c.is_ascii_digit()) && port.parse::<u16>().is_ok())
}

fn scrub(text: &str) -> String {
    static NORMAL: LazyLock<Regex> = LazyLock::new(||Regex::new(r"(://)[^/?#\s]*@").unwrap());
    static BROKEN: LazyLock<Regex> = LazyLock::new(||Regex::new(r"(://)([^/?#\s]*:[^?#\s]*)@").unwrap());
    let text = NORMAL.replace_all(text,"${1}***@");
    BROKEN.replace_all(&text,|caps:&regex::Captures<'_>| {
        if caps[2].starts_with("***@") || valid_port(caps[2].split('/').next().unwrap_or("")) { caps[0].to_owned() }
        else { format!("{}***@",&caps[1]) }
    }).into_owned()
}

fn process_error(error: hangar_workspace::WorkspaceError) -> hangar_workspace::WorkspaceError {
    let message = error.detail.as_str().unwrap_or("");
    if error.status == 503 && message == "git não encontrado" { return hangar_workspace::error(500,"git nao encontrado"); }
    if error.status == 503 && let Some(cause) = message.strip_prefix("git não iniciou: ") {
        let rendered = if let Some((text,code)) = cause.rsplit_once(" (os error ").and_then(|(text,code)|code.strip_suffix(')').map(|code|(text,code))) {
            if cfg!(windows) { format!("[WinError {code}] {}",text.trim_end_matches('.')) }
            else { format!("[Errno {code}] {text}: 'git'") }
        } else { cause.to_owned() };
        return hangar_workspace::error(500,format!("git falhou: {rendered}"));
    }
    error
}

fn fetch(repo: &str, timeout: Duration) -> hangar_workspace::Result<()> {
    let output = hangar_workspace::process::run(Path::new(repo),&["fetch","--all","--prune"],timeout).map_err(process_error)?;
    if output.code != 0 { return Err(hangar_workspace::error(409,scrub(&output.reason("fetch falhou")))); }
    Ok(())
}

pub(crate) fn owns(path: &str) -> bool {
    path.trim_end_matches('/') == "/api/worktrees/fetch"
}

pub(crate) async fn route(st: Arc<AppState>, peer: std::net::SocketAddr, request: Request) -> Response {
    let headers = request.headers().clone();
    if let Some(response) = crate::http_cors::preflight(request.method(),&headers) { return response; }
    if let Some(mut response) = crate::http_body_limit::precheck(&headers) {
        crate::http_cors::apply(&headers,&mut response);
        return response;
    }
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy();
    let decision = crate::http_contract::resolve(&[("/api/worktrees/fetch","POST")],request.method().as_str(),&path);
    let (_,https) = st.cfg.trusted.resolve(peer.ip(),&headers);
    if let Some(mut response) = crate::http_route_response::response(&decision,request.method(),request.uri(),&headers,https,&st.cfg.listen.to_string()) {
        crate::http_cors::apply(&headers,&mut response);
        return response;
    }
    let mut response = match crate::routes::who(&st,peer,&request).await {
        Err(error) => crate::auth::unavailable(error),
        Ok((_,Some(crate::share::caller::Caller::Guest(_)))) => json_guarded(StatusCode::FORBIDDEN,json!({"detail":"worktrees pertencem ao dono"})),
        Ok(_) => match crate::routes::owner_gate(&st,peer,&request).await {
            Ok(_) => public(st,request).await,
            Err(response) => response,
        },
    };
    crate::http_cors::apply(&headers,&mut response);
    response
}

pub(crate) async fn public(st: Arc<AppState>, request: Request) -> Response {
    use axum::body::HttpBody;
    let headers = request.headers().clone();
    let mut response = execute(st,request).await;
    let size = response.body().size_hint().exact().unwrap_or(0);
    let success = response.status() == StatusCode::OK;
    if success || size >= 1024 {
        response.headers_mut().insert(axum::http::header::VARY,axum::http::HeaderValue::from_static("Accept-Encoding"));
        let wants = headers.get(axum::http::header::ACCEPT_ENCODING).and_then(|v|v.to_str().ok()).is_some_and(|v|v.contains("gzip"));
        if wants {
            let (mut parts,body) = response.into_parts();
            let bytes = match axum::body::to_bytes(body,usize::MAX).await { Ok(bytes)=>bytes,Err(_)=>return internal_error() };
            return tokio::task::spawn_blocking(move || {
                let encoded = crate::routes::gzip_with_minimum(&headers,&mut parts.headers,bytes.to_vec(),if success{0}else{1024});
                parts.headers.insert(axum::http::header::CONTENT_LENGTH,axum::http::HeaderValue::from_str(&encoded.len().to_string()).unwrap());
                Response::from_parts(parts,axum::body::Body::from(encoded))
            }).await.unwrap_or_else(|_|internal_error());
        }
    }
    response
}

async fn execute(st: Arc<AppState>, request: Request) -> Response {
    if let Some(response) = crate::http_body_limit::precheck(request.headers()) { return response; }
    let (parts,body) = request.into_parts();
    let bytes = match crate::http_body_limit::read(body).await {
        Ok(bytes) => bytes,
        Err(error) if crate::http_body_limit::is_limit(&error) => return crate::http_body_limit::rejected(),
        Err(_) => return json_guarded(StatusCode::BAD_REQUEST,json!({"detail":"There was an error parsing the body"})),
    };
    let repo = match validate(&bytes,&parts.headers) { Ok(repo)=>repo,Err(response)=>return response };
    let done = tokio::task::spawn_blocking(move || {
        let roots = crate::worktree_context::roots(&st).map_err(|_|hangar_workspace::error(503,"fatos de worktrees indisponíveis"))?;
        let repo = hangar_workspace::worktrees::allowed_fetch_repo(&repo,&roots)?;
        if repo.contains('\0') { return Err(hangar_workspace::error(500,"Internal Server Error")); }
        fetch(&repo,Duration::from_secs(120))
    }).await;
    match done {
        Ok(Ok(()))=>json_guarded(StatusCode::OK,json!({"ok":true})),
        Ok(Err(error)) if error.status == 500 && error.detail == "Internal Server Error"=>internal_error(),
        Ok(Err(error))=>json_guarded(StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),json!({"detail":error.detail})),
        Err(_)=>internal_error(),
    }
}

#[cfg(test)]
#[path = "worktree_fetch_tests.rs"]
mod tests;
