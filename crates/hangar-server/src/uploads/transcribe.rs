//! `POST /api/sessions/{name}/transcribe`: grava o áudio no cofre (ou acha um já enviado),
//! transcreve e, com `limpar`, organiza o texto. Tudo dentro do servidor.
use std::{collections::HashMap, net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, Path as RoutePath, Request, State},
    http::StatusCode,
    response::Response,
};
use serde_json::{Value, json};

use super::{http, store::MAX_UPLOAD_BYTES};
use crate::{
    dictation::model::OrganizationRequest,
    routes::{AppState, pass, who},
    session_write::{detail_body, json_response},
    share::caller::Caller,
    transcription::{model::Profile, routes::transcribe_here},
};

/// Áudio anexado (arquivo de até 10 min) não paga a limpeza nem o prazo do ditado.
pub fn profile(limpar: bool) -> Profile {
    if limpar { Profile::Dictation } else { Profile::File }
}

/// `_with_provider`: quem transcreveu entra na resposta; o aviso da reserva vem antes do da
/// limpeza e nenhum dos dois some.
pub fn with_provider(mut result: Value, provider: &str, aviso: Option<&str>) -> Value {
    let aviso = aviso.filter(|a| !a.is_empty());
    let cleaning = result.get("aviso").and_then(Value::as_str).filter(|a| !a.is_empty()).map(str::to_owned);
    result["provider"] = json!(provider);
    if let Some(a) = aviso {
        result["aviso_transcricao"] = json!(a);
    }
    let avisos: Vec<&str> = aviso.into_iter().chain(cleaning.as_deref()).collect();
    if !avisos.is_empty() {
        result["aviso"] = json!(avisos.join(" · "));
    }
    result
}

fn unavailable(st: &AppState, name: &str, cause: &'static str) -> Response {
    st.diag.report("rust.uploads_failed", name, cause, "o cofre de anexos não respondeu ao transcrever");
    json_response(StatusCode::SERVICE_UNAVAILABLE, detail_body("upload_storage_unavailable", "não foi possível acessar o cofre de anexos", json!({})))
}

pub async fn route(
    State(st): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    RoutePath(name): RoutePath<String>,
    req: Request,
) -> Response {
    let (fwd, caller) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let Some(caller) = caller.filter(|c| c.can(&name, req.method())) else {
        return pass(&st, req, &fwd).await;
    };
    serve(&st, &name, req, matches!(caller, Caller::Guest(_))).await
}

/// O convidado não transcreve por caminho absoluto nem usa a conta do harness para organizar.
pub async fn serve(st: &AppState, name: &str, req: Request, guest: bool) -> Response {
    let query: HashMap<String, String> = form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes()).into_owned().collect();
    // A sessão (e a trava da troca de conversa) vem antes da validação da query, como no FastAPI.
    let facts = match http::facts(st, name, "", true).await {
        Ok(facts) => facts,
        Err(response) => return *response,
    };
    let limpar = match crate::query::bool_param(&query, "limpar") {
        Ok(v) => v,
        Err(response) => return *response,
    };
    let include_recent = match query.contains_key("include_recent_messages")
        .then(|| crate::query::bool_param(&query, "include_recent_messages")).transpose() {
        Ok(v) => v,
        Err(response) => return *response,
    };
    let (store, project) = match http::vault(st, &facts) {
        Ok(v) => v,
        Err(response) => return response,
    };
    let text = |key: &str| query.get(key).cloned();
    let (path, data, filename) = match text("arquivo").filter(|a| !a.is_empty()) {
        Some(arquivo) => {
            let path = match http::resolve_audio(&store, &project, &facts, &arquivo, !guest).await {
                Ok(path) => path,
                Err(response) => return response,
            };
            let data = match tokio::fs::read(&path).await {
                Ok(data) => bytes::Bytes::from(data),
                // A poda pode tirar o áudio entre a resolução e a leitura.
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return http::failure(404, "erro_upload_inexistente", "arquivo não encontrado");
                }
                Err(_) => return unavailable(st, name, "upload_audio_unreadable"),
            };
            let filename = path.file_name().map(|n| n.to_string_lossy().into_owned());
            (path, data, filename)
        }
        None => {
            let declared = req.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
            if declared.is_some_and(|n| n > MAX_UPLOAD_BYTES) {
                return http::failure(413, "erro_arquivo_grande", "arquivo maior que 100 MiB");
            }
            let filename = req.headers().get("x-filename").and_then(|v| v.to_str().ok()).map(str::to_owned).or_else(|| text("name"));
            let data = match axum::body::to_bytes(req.into_body(), MAX_UPLOAD_BYTES as usize).await {
                Ok(data) => data,
                Err(e) if std::error::Error::source(&e).is_some_and(|s| s.is::<http_body_util::LengthLimitError>()) => {
                    return http::failure(413, "erro_arquivo_grande", "arquivo maior que 100 MiB");
                }
                Err(_) => return http::failure(400, "erro_upload_interrompido", "o envio do áudio foi interrompido"),
            };
            let body = futures_util::stream::iter([Ok::<_, std::io::Error>(data.clone())]);
            let path = match http::publish(st, &store, &project, &facts, filename.as_deref().unwrap_or(""), Some(data.len() as u64), body).await {
                Ok(path) => path,
                Err(response) => return response,
            };
            (path, data, filename)
        }
    };
    let transcription = match transcribe_here(st, data, filename, profile(limpar)).await {
        Ok(t) => t,
        Err(e) => {
            let status = StatusCode::from_u16(e.status).unwrap_or(StatusCode::BAD_GATEWAY);
            return json_response(status, detail_body(&e.code, &e.detail, json!({})));
        }
    };
    let aviso = transcription.aviso.as_deref();
    if !limpar {
        return json_response(StatusCode::OK, with_provider(json!({"path": path, "text": transcription.text}), &transcription.provider, aviso));
    }
    let mode = text("organization_mode");
    let payload = json!({"raw": transcription.text, "style": text("estilo"), "mode": mode, "session": name,
        "generation": text("generation"), "model": text("organization_model"), "account": text("organization_account"),
        "harness_allowed": !guest, "include_recent_messages": include_recent, "recent_messages": null});
    let started = tokio::time::Instant::now();
    let result = match serde_json::from_value::<OrganizationRequest>(payload) {
        Ok(request) => match serde_json::to_value(crate::dictation::routes::organize_request(st, request, started).await) {
            Ok(value) => value,
            Err(_) => return unavailable(st, name, "dictation_result_unserializable"),
        },
        // Estilo ou modo desconhecido: o texto transcrito volta cru, com o motivo.
        Err(_) => json!({"text": transcription.text, "raw": transcription.text, "aviso": "Pedido de organização inválido.",
            "estilo_aplicado": "cru", "organization_mode": mode, "organization_code": "dictation_request_invalid"}),
    };
    let mut out = with_provider(result, &transcription.provider, aviso);
    out["path"] = json!(path);
    json_response(StatusCode::OK, out)
}
