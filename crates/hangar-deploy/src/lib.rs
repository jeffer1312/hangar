//! Autenticação e disparo do webhook de deploy, independente do processo que será reiniciado.
#![forbid(unsafe_code)]

use axum::{http::StatusCode, response::Response};
use serde_json::json;
use subtle::ConstantTimeEq;

use hangar_core::{
    http::{detail, json_response},
    orq_codec::{body_encoding, codec},
    process::{CommandError, Commands},
};

pub async fn webhook(
    secret: &str,
    event: &str,
    signature: &str,
    body: &[u8],
    commands: &dyn Commands,
) -> Response {
    if secret.is_empty() {
        return detail(
            StatusCode::NOT_FOUND,
            "erro_nao_encontrado",
            "not found",
            json!({}),
        );
    }
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes());
    let expected = ring::hmac::sign(&key, body)
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    if !signature
        .strip_prefix("sha256=")
        .is_some_and(|digest| bool::from(digest.as_bytes().ct_eq(expected.as_bytes())))
    {
        return detail(
            StatusCode::UNAUTHORIZED,
            "erro_assinatura_invalida",
            "bad signature",
            json!({}),
        );
    }
    match event {
        "ping" => json_response(StatusCode::OK, json!({"status":"pong"})),
        "push" => {
            let value = body_encoding::decode(body)
                .ok()
                .and_then(|decoded| codec::loads(&decoded.text));
            let Some(serde_json::Value::Object(value)) = value else {
                return detail(
                    StatusCode::BAD_REQUEST,
                    "erro_payload_invalido",
                    "invalid payload",
                    json!({}),
                );
            };
            let reference = value.get("ref").cloned().unwrap_or_else(|| json!(""));
            if reference != "refs/heads/main" {
                return json_response(
                    StatusCode::OK,
                    json!({"status":"ignored", "reason":format!("ref={}", codec::python_string(&reference))}),
                );
            }
            match commands
                .run(
                    "systemctl",
                    &["--user", "start", "--no-block", "hangar-deploy.service"],
                    std::time::Duration::from_secs(10),
                )
                .await
            {
                Ok(output) if output.success => {
                    tracing::info!(code = "deploy_accepted", "serviço de deploy acionado");
                    json_response(StatusCode::OK, json!({"status":"accepted"}))
                }
                Ok(_) | Err(CommandError::Timeout) => detail(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "erro_deploy_falhou",
                    "failed to trigger deploy",
                    json!({}),
                ),
                Err(CommandError::Io(_)) => hangar_core::http_cors::internal_error(),
            }
        }
        _ => json_response(
            StatusCode::OK,
            json!({"status":"ignored", "reason":format!("event={event}")}),
        ),
    }
}
