//! Prévia de uma porta local no slot HTTPS próprio do Tailscale Serve.
#![forbid(unsafe_code)]

use axum::{http::StatusCode, response::Response};
use serde_json::{Value, json};

use hangar_core::{
    http::json_response,
    process::{CommandError, CommandOutput, Commands},
    pycompat::py::strip,
};

const SLOT: u16 = 10000;

pub async fn start(commands: &dyn Commands, port: u16) -> Response {
    let output = match call(commands, &["status", "--json"]).await {
        Ok(output) => output,
        Err(response) => return response,
    };
    if !output.success {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            message(&output.stderr, "tailscale status falhou"),
        );
    }
    let name = hangar_core::orq_codec::codec::loads(&output.stdout)
        .and_then(|value| {
            value
                .get("Self")
                .and_then(|this| this.get("DNSName"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .map(|name| name.trim_end_matches('.').to_owned())
        .filter(|name| !name.is_empty());
    let Some(name) = name else {
        return failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "tailscale sem DNSName (logado?)",
        );
    };
    let destination = format!("localhost:{port}");
    let slot = format!("--https={SLOT}");
    match call(commands, &["serve", "--bg", &slot, &destination]).await {
        Ok(output) if output.success => {
            json_response(StatusCode::OK, json!({"url":url(&name), "port":port}))
        }
        Ok(output) => {
            let combined = output.stdout + &output.stderr;
            if combined.contains("Access denied") || combined.contains("operator") {
                failure(
                    StatusCode::FORBIDDEN,
                    "backend sem permissão pro tailscale serve (rode uma vez: sudo tailscale set --operator=$USER)",
                )
            } else {
                failure(
                    StatusCode::BAD_GATEWAY,
                    message(&output.stderr, "tailscale serve falhou"),
                )
            }
        }
        Err(response) => response,
    }
}

pub async fn status(commands: &dyn Commands) -> Response {
    let output = match call(commands, &["serve", "status", "--json"]).await {
        Ok(output) => output,
        Err(response) => return response,
    };
    if !output.success || output.stdout.is_empty() {
        return inactive();
    }
    let Some(value) = hangar_core::orq_codec::codec::loads(&output.stdout) else {
        return inactive();
    };
    let Some(root) = value.as_object() else {
        return hangar_core::http_cors::internal_error();
    };
    let web = match object_or_empty(root.get("Web")) {
        Ok(Some(web)) => web,
        Ok(None) => return inactive(),
        Err(()) => return hangar_core::http_cors::internal_error(),
    };
    for (key, config) in web {
        let Some(host) = key.strip_suffix(&format!(":{SLOT}")) else {
            continue;
        };
        let Some(config) = config.as_object() else {
            return hangar_core::http_cors::internal_error();
        };
        let handlers = match object_or_empty(config.get("Handlers")) {
            Ok(value) => value,
            Err(()) => return hangar_core::http_cors::internal_error(),
        };
        let handler = match object_or_empty(handlers.and_then(|handlers| handlers.get("/"))) {
            Ok(value) => value,
            Err(()) => return hangar_core::http_cors::internal_error(),
        };
        let port = handler
            .and_then(|handler| handler.get("Proxy"))
            .and_then(Value::as_str)
            .and_then(|proxy| proxy.rsplit(':').next()?.split('/').next())
            .and_then(|port| hangar_core::value_coercion::integer(&json!(port)))
            .unwrap_or(Value::Null);
        return json_response(
            StatusCode::OK,
            json!({"active":true, "port":port, "url":url(host)}),
        );
    }
    inactive()
}
pub async fn stop(commands: &dyn Commands) -> Response {
    let slot = format!("--https={SLOT}");
    match call(commands, &["serve", &slot, "off"]).await {
        Ok(output)
            if output.success
                || (output.stdout.clone() + &output.stderr).contains("does not exist") =>
        {
            inactive()
        }
        Ok(output) => failure(
            StatusCode::BAD_GATEWAY,
            message(&output.stderr, "falha ao desligar"),
        ),
        Err(response) => response,
    }
}

async fn call(commands: &dyn Commands, args: &[&str]) -> Result<CommandOutput, Response> {
    commands
        .run("tailscale", args, std::time::Duration::from_secs(20))
        .await
        .map_err(|error| match error {
            CommandError::Timeout => failure(StatusCode::GATEWAY_TIMEOUT, "tailscale timeout"),
            CommandError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "tailscale não encontrado",
            ),
            CommandError::Io(error) => failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("tailscale falhou: {error}"),
            ),
        })
}

fn inactive() -> Response {
    json_response(
        StatusCode::OK,
        json!({"active":false, "port":null, "url":null}),
    )
}
fn failure(status: StatusCode, message: impl AsRef<str>) -> Response {
    json_response(status, json!({"detail":message.as_ref()}))
}
fn message<'a>(text: &'a str, default: &'a str) -> &'a str {
    let text = strip(text);
    if text.is_empty() { default } else { text }
}
fn url(host: &str) -> String {
    format!("https://{host}:{SLOT}/")
}

fn object_or_empty(value: Option<&Value>) -> Result<Option<&serde_json::Map<String, Value>>, ()> {
    match value {
        None | Some(Value::Null | Value::Bool(false)) => Ok(None),
        Some(Value::String(text)) if text.is_empty() => Ok(None),
        Some(Value::Array(items)) if items.is_empty() => Ok(None),
        Some(Value::Number(number)) if number.as_f64() == Some(0.0) => Ok(None),
        Some(Value::Object(object)) => Ok(Some(object)),
        _ => Err(()),
    }
}
