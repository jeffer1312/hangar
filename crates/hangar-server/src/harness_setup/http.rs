//! Borda HTTP da saúde e instalação; o roteador aplica autenticação e limites antes daqui.
use super::{
    installer::{Installer, StartError},
    service::Service,
};
use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use std::sync::Arc;
pub const TABLE: &[(&str, &str)] = &[
    ("/api/harness", "GET"),
    ("/api/harness/instalar", "GET"),
    ("/api/harness/instalar/{cli}", "POST"),
    ("/api/harness/conserto/{id_:path}", "POST"),
];
pub fn matches(path: &str) -> bool {
    path == "/api/harness"
        || path == "/api/harness/"
        || path == "/api/harness/instalar"
        || path.starts_with("/api/harness/instalar/")
        || path.starts_with("/api/harness/conserto/")
}
fn coded(status: u16, code: &str, message: &str, params: Value) -> Response {
    crate::provider_models::ModelError::coded(status, code, message, params).response()
}
pub async fn serve(service: Service, installer: Arc<Installer>, request: Request) -> Response {
    let method = request.method();
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy();
    if method == Method::GET && path == "/api/harness/instalar" {
        return crate::session_write::json_response(StatusCode::OK, installer.status());
    }
    if method == Method::POST {
        if let Some(cli) = path.strip_prefix("/api/harness/instalar/") {
            return match installer.start(service, cli) {
                Ok(value) => crate::session_write::json_response(StatusCode::ACCEPTED, value),
                Err(StartError::Unsupported) => coded(
                    400,
                    "erro_harness_sem_instalador",
                    &format!("nao ha comando conferido pra instalar {cli} aqui"),
                    json!({"cli":cli}),
                ),
                Err(StartError::Busy(current)) => coded(
                    409,
                    "erro_harness_instalando",
                    &format!("ja ha uma instalacao em curso ({current}) — espere ela terminar"),
                    json!({"harness":current}),
                ),
                Err(StartError::Closed) => coded(
                    503,
                    "erro_harness_instalando",
                    "o serviço de instalação está encerrando",
                    json!({}),
                ),
            };
        }
    }
    if method == Method::GET && path == "/api/harness" {
        return match service.diagnose().await {
            Ok(value) => crate::session_write::json_response(StatusCode::OK, value),
            Err(_) => crate::engines::body::internal_error(),
        };
    }
    if method == Method::POST {
        if let Some(id) = path.strip_prefix("/api/harness/conserto/") {
            return match service.repair(id).await {
                Ok(done) => match service.diagnose().await {
                    Ok(cards) => crate::session_write::json_response(
                        StatusCode::OK,
                        json!({"feito":done,"harnesses":cards}),
                    ),
                    Err(_) => crate::engines::body::internal_error(),
                },
                Err(error) => coded(
                    error.status,
                    "erro_harness_conserto",
                    &error.message,
                    json!({"motivo":error.message}),
                ),
            };
        }
    }
    crate::session_write::json_response(StatusCode::NOT_FOUND, json!({"detail":"Not Found"}))
}
