//! Parâmetros de query com a semântica do FastAPI, para as rotas que o Rust assumiu do Python.
use axum::{http::StatusCode, response::Response};
use serde_json::json;
use std::collections::HashMap;

/// Os valores que o pydantic aceita como `bool`; qualquer outro é recusado.
pub fn fastapi_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "y" | "on" | "t" => Some(true),
        "false" | "0" | "no" | "n" | "off" | "f" => Some(false),
        _ => None,
    }
}

/// Ausente é `false`; inválido devolve o mesmo 422 `bool_parsing` do FastAPI.
pub fn bool_param(query: &HashMap<String, String>, name: &str) -> Result<bool, Response> {
    let Some(value) = query.get(name) else {
        return Ok(false);
    };
    fastapi_bool(value).ok_or_else(|| {
        crate::session_write::json_response(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"detail":[{"type":"bool_parsing","loc":["query",name],
                "msg":"Input should be a valid boolean, unable to interpret input","input":value}]}),
        )
    })
}
