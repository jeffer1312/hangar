//! Respostas HTTP do próprio Rust com o formato que os clientes já conhecem do Python.
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

/// Mesmo teto do middleware de corpo do Python (`uploads.MAX_BYTES`), que recusa antes da rota.
pub const BODY_LIMIT: usize = 100 * 1024 * 1024;
const BUSY_MSG: &str = "A sessão está trocando de agente; tente novamente quando terminar.";

/// Envelope do `HTTPException(detail=erro(...))` do Python (`mensagens.py`).
pub fn detail(status: StatusCode, code: &str, msg: &str, params: Value) -> Response {
    json_response(status, detail_body(code, msg, params))
}

pub fn detail_body(code: &str, msg: &str, params: Value) -> Value {
    json!({"detail": {"code": code, "params": params, "msg": msg}})
}

pub fn json_response(status: StatusCode, body: Value) -> Response {
    (status, [(header::CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

/// Igual ao `_BodySizeLimitMiddleware`: texto puro, e o CORS por fora dele.
pub fn too_large(headers: &HeaderMap) -> Response {
    let mut response = (StatusCode::PAYLOAD_TOO_LARGE, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], "request body too large").into_response();
    cors(headers, response.headers_mut());
    response
}

pub fn busy_body() -> Value { detail_body("session_transfer_busy", BUSY_MSG, json!({})) }

/// O 404 do FastAPI, que os clientes já tratam.
pub fn not_found(headers: &HeaderMap) -> Response {
    let mut resp = (StatusCode::NOT_FOUND, [(header::CONTENT_TYPE, "application/json")], r#"{"detail":"Not Found"}"#).into_response();
    cors(headers, resp.headers_mut());
    resp
}

/// CORS das respostas do próprio Rust, como o CORSMiddleware do Python: `*`, sem credenciais,
/// ETag legível pelo JS. Preflight não chega aqui: vai sem token e é repassado.
pub fn cors(req: &HeaderMap, resp: &mut HeaderMap) {
    if req.contains_key(header::ORIGIN) {
        resp.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        resp.insert(header::ACCESS_CONTROL_EXPOSE_HEADERS, HeaderValue::from_static("ETag"));
    }
}

/// Gzip só quando o cliente pede e o corpo passa de 1 KB. SSE nunca: o gzip seguraria os quadros.
pub fn maybe_gzip(req: &HeaderMap, resp: &mut HeaderMap, body: Vec<u8>) -> Vec<u8> {
    gzip_with_minimum(req, resp, body, 1024)
}

pub fn gzip_with_minimum(req: &HeaderMap, resp: &mut HeaderMap, body: Vec<u8>, minimum: usize) -> Vec<u8> {
    let wants = req
        .get(header::ACCEPT_ENCODING)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.contains("gzip"));
    let sse = resp
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));
    if !wants || sse || body.len() < minimum {
        return body;
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(5));
    std::io::Write::write_all(&mut enc, &body).expect("escrita em memória");
    resp.insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
    resp.insert(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    enc.finish().expect("escrita em memória")
}
