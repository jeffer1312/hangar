//! URL publicada pelo desktop; a consulta não depende da sessão nem da vida do processo.
use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Method, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::access::{self, AccessError};
use crate::routes::AppState;
use crate::session_write::{detail_body, json_response};
use crate::share::caller::{Caller, caller};

// A identidade própria separa escapes inválidos de caracteres Unicode reais.
static JSON_MARKER: LazyLock<String> = LazyLock::new(|| {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut bytes).expect("aleatoriedade do sistema");
    format!("browser:{}:",bytes.iter().map(|b|format!("{b:02x}")).collect::<String>())
});

pub async fn route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let response = if crate::routes::on_guest_port(&req) {
        match caller(&req) {
            Some(Caller::Guest(grant)) if crate::share::caller::guest_allowed(req.method(),req.uri().path(),grant) => serve(&st,req.uri().path()).await,
            _ => json_response(StatusCode::FORBIDDEN,detail_body("erro_fora_do_convite","fora da sessao compartilhada",json!({}))),
        }
    } else {
        let (ip,https) = st.cfg.trusted.resolve(peer.ip(),req.headers());
        match access::http(&st.auth,&ip,req.headers(),req.uri().query(),https,Instant::now()) {
            Ok(()) => serve(&st,req.uri().path()).await,
            Err(AccessError::Unauthorized) => json_response(StatusCode::UNAUTHORIZED,
                detail_body("erro_nao_autorizado","unauthorized",json!({}))),
            Err(AccessError::Blocked) => {
                let mut response = json_response(StatusCode::TOO_MANY_REQUESTS,
                    detail_body("erro_muitas_tentativas","muitas tentativas — aguarde",json!({})));
                response.headers_mut().insert("retry-after",axum::http::HeaderValue::from_static("30"));
                response
            },
        }
    };
    let mut response = response;
    crate::routes::cors(req.headers(),response.headers_mut());
    response
}

pub(crate) fn matches(method: &Method, path: &str) -> bool {
    *method == Method::GET && session(path).is_some()
}

fn session(path: &str) -> Option<String> {
    let name = path.strip_prefix("/api/sessions/")?.strip_suffix("/navegador")?;
    if name.is_empty() || name.contains('/') { return None; }
    percent_encoding::percent_decode_str(name).decode_utf8().ok().map(|s|s.into_owned())
}

/// A autenticação e o filtro da borda Python já aconteceram; ela nunca oferece motor de reserva.
pub(crate) async fn serve_forwarded(st: &Arc<AppState>, req: Request, guest: bool) -> Response {
    if guest {
        return json_response(StatusCode::FORBIDDEN,detail_body("erro_fora_do_convidado","fora do acesso do convidado",json!({})));
    }
    serve(st,req.uri().path()).await
}

async fn serve(st: &AppState, path: &str) -> Response {
    let Some(name) = session(path) else { return json_response(StatusCode::NOT_FOUND,json!({"detail":"Not Found"})) };
    match read(&st.hangar_dir.join("nav"),&name).await {
        Ok(value) => json_response(StatusCode::OK,json!({"url":value})),
        // Tipos que o JSONResponse do Python recusa conservam a falha HTTP, sem inventar URL nula.
        Err(()) => (StatusCode::INTERNAL_SERVER_ERROR,"Internal Server Error").into_response(),
    }
}

async fn read(dir: &Path, name: &str) -> Result<Value,()> {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else { return Ok(Value::Null) };
    while let Ok(Some(entry)) = entries.next_entry().await {
        // pathlib.glob inclui nomes ocultos e auxiliares; a ordem é a do diretório.
        if !entry.file_name().to_string_lossy().ends_with(".json") { continue; }
        let path = entry.path();
        let text = match tokio::fs::read_to_string(&path).await {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!(code="browser_sidecar_unreadable",path=%path.display(),%error,"sidecar de navegador ilegivel");
                continue;
            },
        };
        // O codec mantém inteiros largos, floats Python e surrogates, inclusive em campos ignorados.
        let Some(sidecar) = crate::orq_conductor::load(&text,JSON_MARKER.as_str()) else {
            tracing::warn!(code="browser_sidecar_unreadable",path=%path.display(),"sidecar de navegador ilegivel: JSON inválido");
            continue;
        };
        let Value::Object(map) = sidecar else { continue };
        let key = match map.get("chave") {
            None => "",
            Some(Value::String(key)) => {
                if ["NaN","Infinity","-Infinity"].iter().any(|token| key == &format!("{}{token}",JSON_MARKER.as_str())) {
                    return Err(());
                }
                key
            },
            Some(_) => return Err(()),
        };
        if key == name || key.ends_with(&format!("::{name}")) {
            return http_value(map.get("url").cloned().unwrap_or(Value::Null));
        }
    }
    Ok(Value::Null)
}

fn http_value(value: Value) -> Result<Value,()> {
    match value {
        Value::String(text) if text.contains(JSON_MARKER.as_str()) => Err(()),
        Value::Number(number) => serde_json::from_str(&crate::transcript::pyjson::dumps(&Value::Number(number),false)).map_err(|_|()),
        Value::Array(items) => items.into_iter().map(http_value).collect::<Result<Vec<_>,_>>().map(Value::Array),
        Value::Object(map) => {
            let mut output = serde_json::Map::new();
            for (key,value) in map {
                if key.contains(JSON_MARKER.as_str()) { return Err(()); }
                output.insert(key,http_value(value)?);
            }
            Ok(Value::Object(output))
        },
        value => Ok(value),
    }
}
