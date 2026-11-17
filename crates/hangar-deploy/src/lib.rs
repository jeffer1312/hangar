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

/// A porta do backend como o servidor a lê: `CP_PORT` do ambiente, senão a do `.env` dos dados do
/// usuário (ou da `backend/` dada, na máquina não migrada), senão 8765. Valor que não é porta é erro.
pub fn port(home: &std::path::Path, backend: Option<&std::path::Path>, environment: Option<&str>) -> Result<u16, String> {
    let value = match environment {
        Some(value) => Some(value.to_owned()),
        None => hangar_core::user_data::env_value(home, backend, "CP_PORT")?,
    };
    let Some(value) = value else { return Ok(8765) };
    value.trim().parse::<u16>().ok().filter(|port| *port > 0).ok_or_else(|| format!("CP_PORT={value:?} não é uma porta"))
}

/// `hangar-server deploy-port [--backend <pasta>]`: a porta que o `deploy.sh` sonda depois do restart.
pub fn port_cli(args: &[String]) -> std::process::ExitCode {
    let backend = match args {
        [] => None,
        [flag, dir] if flag == "--backend" => Some(std::path::PathBuf::from(dir)),
        _ => {
            eprintln!("uso: hangar-server deploy-port [--backend <pasta>]");
            return std::process::ExitCode::from(2);
        }
    };
    let home = std::env::home_dir().unwrap_or_default();
    let environment = std::env::var("CP_PORT").ok();
    match port(&home, backend.as_deref(), environment.as_deref()) {
        Ok(port) => { println!("{port}"); std::process::ExitCode::SUCCESS }
        Err(error) => { eprintln!("deploy-port: {error}"); std::process::ExitCode::FAILURE }
    }
}
