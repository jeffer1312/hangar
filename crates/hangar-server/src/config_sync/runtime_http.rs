//! Preferências públicas do dono; configuração e gravação compartilham o serviço nativo.
use super::{runtime::RuntimeConfig, runtime_values::{ValueGuard, Values}};
use crate::{orchestration::codec, routes::AppState};
use axum::{extract::{ConnectInfo, Request, State}, http::{Method, StatusCode}, response::Response};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};

pub fn owns_path(raw: &str) -> bool {
    let path = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
    path == "/api/config" || path.starts_with("/api/config/")
}

pub async fn cors_layer(request: Request, next: axum::middleware::Next) -> Response {
    if !owns_path(request.uri().path()) {return next.run(request).await;}
    if let Some(response) = crate::http_cors::preflight(request.method(), request.headers()) {return response;}
    let headers = request.headers().clone();
    let method = request.method().clone();
    let mut response = next.run(request).await;
    crate::http_cors::apply(&headers, &mut response);
    crate::http_route_response::suppress_head(&method, &mut response);
    response
}

pub async fn handle(State(state): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, request: Request) -> Response {
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy();
    let decision = crate::http_contract::resolve(&[("/api/config","GET"),("/api/config","POST"),("/api/config","PATCH")], request.method().as_str(), &path);
    let (_, https) = state.cfg.trusted.resolve(peer.ip(),request.headers());
    if let Some(response) = crate::http_route_response::response(&decision,request.method(),request.uri(),request.headers(),https,&state.cfg.listen.to_string()) {return response;}
    if let Err(response) = crate::routes::owner_gate(&state,peer,&request).await {return response;}
    let service = match RuntimeConfig::from_state(&state) {
        Ok(service) => service,
        Err(_) => return crate::http_cors::json_guarded(StatusCode::SERVICE_UNAVAILABLE,json!({"detail":{"code":"config_unavailable","params":{},"msg":"configuração indisponível"}})),
    };
    let root = crate::install_root::root(&service.env.home).unwrap_or_else(||service.env.home.join(".hangar/current"));
    let version = match crate::boot_version::ensure(root.clone(),service.env.base.clone()).await {
        Ok(version)=>version,
        Err(_)=>return crate::http_cors::json_guarded(StatusCode::SERVICE_UNAVAILABLE,json!({"detail":{"code":"config_unavailable","params":{},"msg":"configuração indisponível"}})),
    };
    let writing = request.method() != Method::GET;
    let body = if writing {
        let bytes = match crate::http_body_limit::read(request.into_body()).await {
            Ok(bytes) => bytes,
            Err(error) if crate::http_body_limit::is_limit(&error) => return crate::http_body_limit::rejected(),
            Err(_) => return crate::http_cors::internal_error(),
        };
        match crate::engines::body::raw(&bytes) {
            Ok(value) => ValueGuard(value),
            Err(response) => return response,
        }
    } else {ValueGuard(Value::Null)};
    let result = tokio::task::spawn_blocking(move || {
        if writing {
            let Some(values) = body.as_object() else {
                return crate::http_cors::json_guarded(StatusCode::BAD_REQUEST,json!({"detail":{"code":"erro_corpo_deve_ser_objeto","params":{},"msg":"corpo deve ser um objeto"}}));
            };
            let changes = Values(values.iter().filter(|(_,value)|!value.is_null()).map(|(key,value)|(key.clone(),codec::clone_value(value))).collect());
            let remove = values.iter().filter(|(_,value)|value.is_null()).map(|(key,_)|key.clone()).collect();
            match service.apply_checked(&changes,&remove) {
                Ok(value) => codec::discard(Value::Object(value)),
                Err(error) if error.is_validation() => return crate::http_cors::json_guarded(StatusCode::BAD_REQUEST,json!({"detail":error.message})),
                Err(_) => return crate::http_cors::internal_error(),
            }
            if values.contains_key("claude_function_hooks") {
                let enabled = ValueGuard(service.get("claude_function_hooks"));
                if let Err(error) = super::plugin_publish::publish(&service.env.home,&root,state.cfg.listen.port(),&state.cfg.auth_token,crate::orchestration::truthy(&enabled)) {
                    tracing::warn!(code=error.code,"caminho do plugin não foi republicado");
                }
            }
        }
        let fields = match service.state() {Ok(value)=>ValueGuard(value),Err(_)=>return crate::http_cors::internal_error()};
        let metadata = match service.metadata() {Ok(value)=>ValueGuard(value),Err(_)=>return crate::http_cors::internal_error()};
        let setting = |name:&str| metadata.as_array().unwrap().iter().find(|row|row["nome"]==name).map(|row|row["valor"].clone()).unwrap_or(Value::Null);
        let readonly = json!({"port":setting("CP_PORT"),"lan_bind_ip":setting("CP_LAN_BIND_IP"),
            "server_id":state.current_server_id(),"public_url":setting("CP_PUBLIC_URL"),
            "terminal_panel":true,"terminal_origem_ok":true,"traducao_pensamento":translation_available(&service),"versao":version.describe});
        let mut result = Values(serde_json::Map::new());
        result.insert("campos".into(),codec::clone_value(&fields));
        result.insert("somente_leitura".into(),readonly);
        if !writing {result.insert("variaveis_env".into(),codec::clone_value(&metadata));}
        crate::http_cors::json_guarded(StatusCode::OK,Value::Object(result.into_map()))
    }).await;
    result.unwrap_or_else(|_|crate::http_cors::internal_error())
}

fn translation_available(service:&RuntimeConfig)->bool {
    let enabled=ValueGuard(service.get("traduzir_pensamento"));
    if !crate::orchestration::truthy(&enabled) {return false;}
    let text=|key| {
        let value=ValueGuard(service.get(key));
        if !crate::orchestration::truthy(&value) {return Some(String::new());}
        value.as_str().filter(|_|!codec::contains_nonfinite(&value)).map(|s|crate::transcript::py::strip(s).to_owned())
    };
    let Some(base)=text("llm_base_url") else {return false;};
    let base=base.trim_end_matches('/');
    let key=if base.is_empty()||base=="https://api.groq.com/openai/v1" {
        if text("transcription_base_url").is_none_or(|s|!s.is_empty()) {return false;}
        "groq_api_key"
    } else {"llm_api_key"};
    text(key).is_some_and(|key|!key.is_empty())
}
