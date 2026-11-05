//! Catálogos e escolha de modelo Pi, omp e Kimi, com confirmação no agente.
pub mod driver;
pub mod inbox;
mod pure;
mod routes;
pub(crate) use routes::kimi_working;
pub use routes::{allowed_method, catalog_request, inbox_http_request, matches, serve};
pub mod catalog;
pub use catalog::{catalog, warm};
pub use pure::{kimi_catalog, pi_catalog, pi_provider};
use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct ModelError {
    pub status: u16,
    pub detail: Value,
}
impl ModelError {
    pub fn text(status: u16, message: impl Into<String>) -> Self {
        Self {
            status,
            detail: json!(message.into()),
        }
    }
    pub fn coded(status: u16, code: &str, message: &str, params: Value) -> Self {
        Self {
            status,
            detail: json!({"code":code,"msg":message,"params":params}),
        }
    }
    pub fn response(self) -> axum::response::Response {
        if self.status == 500 {
            use axum::response::IntoResponse;
            return (axum::http::StatusCode::INTERNAL_SERVER_ERROR,"Internal Server Error").into_response();
        }
        crate::session_write::json_response(
            axum::http::StatusCode::from_u16(self.status)
                .unwrap_or(axum::http::StatusCode::INTERNAL_SERVER_ERROR),
            json!({"detail":self.detail}),
        )
    }
}
impl std::fmt::Display for ModelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "modelo: HTTP {}", self.status)
    }
}
impl std::error::Error for ModelError {}

#[cfg(test)]
pub fn contract_case(op: &str, input: &Value) -> Value {
    let s = input.as_str().unwrap_or("");
    let result: Result<Value, ModelError> = match op {
        "parse_pi" => Ok(json!(pure::parse_pi(s))),
        "parse_omp" => pure::parse_omp(s).map(|v|json!(v)),
        "model_command" => {
            pure::model_command(input[0].as_str().unwrap(), input[1].as_str().unwrap())
                .map(Value::String)
        }
        "think_command" => pure::think_command(s).map(Value::String),
        "confirms" => Ok(json!(pure::confirms(
            &input[0],
            input[1].as_str(),
            input[2].as_str(),
            input[3].as_str()
        ))),
        "parse_switched" => Ok(pure::parse_switched(s).unwrap_or(Value::Null)),
        "parse_thinking_set" => Ok(pure::parse_thinking_set(s).unwrap_or(Value::Null)),
        "parse_thinking_row" => Ok(pure::parse_thinking_row(s).unwrap_or(Value::Null)),
        "clean_alias" => pure::clean(s, "alias").map(Value::String),
        "kimi_catalog" => Ok(pure::parse_kimi(s).unwrap_or(Value::Null)),
        _ => return Value::Null,
    };
    match result {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"error":{"status":error.status,"detail":error.detail}}),
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod tests_extra;
