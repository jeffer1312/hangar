//! Liga as integrações locais aos porteiros e ao estado do servidor.
use std::{net::SocketAddr, sync::Arc};

use axum::{
    extract::{ConnectInfo, Request, State},
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::json;

use hangar_core::process::{CommandError, CommandOutput, Commands};

use crate::{config_sync::runtime_values::ValueGuard, routes::AppState};

pub(crate) const ROUTES: &[(&str, &str)] = &[
    ("/api/credenciais/cookie", "PUT"),
    ("/api/deploy/github-webhook", "POST"),
    ("/api/preview", "GET"),
    ("/api/preview", "POST"),
    ("/api/preview", "DELETE"),
];

pub(crate) fn owns_path(raw: &str) -> bool {
    let path = percent_encoding::percent_decode_str(raw).decode_utf8_lossy();
    ROUTES.iter().any(|(base, _)| {
        path == *base
            || path
                .strip_prefix(base)
                .is_some_and(|tail| tail.starts_with('/'))
    })
}

pub(crate) struct NativeCommands(pub(crate) crate::accounts::environment::AccountEnvironment);

impl Commands for NativeCommands {
    fn run<'a>(
        &'a self,
        program: &'a str,
        args: &'a [&'a str],
        deadline: std::time::Duration,
    ) -> futures_util::future::BoxFuture<'a, Result<CommandOutput, CommandError>> {
        Box::pin(async move {
            let args = std::iter::once(program)
                .chain(args.iter().copied())
                .map(str::to_owned)
                .collect();
            let result = crate::command_capture::capture_bytes(
                args,
                self.0.clone(),
                deadline,
                crate::command_capture::CaptureOptions {
                    preserve_account: true,
                    ..Default::default()
                },
            )
            .await;
            if program == "systemctl" {
                let scrubber = crate::diag_report::Scrubber::for_env(&self.0);
                match &result {
                    Ok(output) if !output.success => {
                        let cause = String::from_utf8_lossy(&output.stderr);
                        tracing::warn!(code="deploy_service_failed", stage="service_start",
                            cause=%scrubber.scrub(&cause), "systemctl recusou o início do deploy");
                    }
                    Err(error) => {
                        let cause = error
                            .io_error
                            .as_ref()
                            .map(|error| error.to_string())
                            .unwrap_or_else(|| error.kind.clone());
                        tracing::warn!(code="deploy_command_failed", stage="service_start", kind=%error.kind,
                            cause=%scrubber.scrub(&cause), "não foi possível executar o comando do deploy");
                    }
                    _ => {}
                }
            }
            match result {
                Ok(output) => {
                    let normalize = |bytes: &[u8]| {
                        String::from_utf8_lossy(bytes)
                            .replace("\r\n", "\n")
                            .replace('\r', "\n")
                    };
                    Ok(CommandOutput {
                        success: output.success,
                        stdout: normalize(&output.stdout),
                        stderr: normalize(&output.stderr),
                    })
                }
                Err(error) if error.kind == "timeout" => Err(CommandError::Timeout),
                Err(error) => {
                    if let Some(cause) = error.io_error {
                        return Err(CommandError::Io(match cause.raw_os_error() {
                            Some(code) => std::io::Error::from_raw_os_error(code),
                            None => std::io::Error::new(cause.kind(), cause.to_string()),
                        }));
                    }
                    let kind = match error.kind.as_str() {
                        "FileNotFoundError" => std::io::ErrorKind::NotFound,
                        "PermissionError" => std::io::ErrorKind::PermissionDenied,
                        _ => std::io::ErrorKind::Other,
                    };
                    Err(CommandError::Io(std::io::Error::new(kind, error.kind)))
                }
            }
        })
    }
}

pub(crate) async fn handle(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    if let Some(response) = crate::http_cors::preflight(request.method(), request.headers()) {
        return response;
    }
    let (headers, method) = (request.headers().clone(), request.method().clone());
    let mut response = reply(state, peer, request).await;
    crate::http_cors::apply(&headers, &mut response);
    crate::http_route_response::suppress_head(&method, &mut response);
    response
}

async fn reply(state: Arc<AppState>, peer: SocketAddr, request: Request) -> Response {
    let path = percent_encoding::percent_decode_str(request.uri().path()).decode_utf8_lossy();
    let decision = crate::http_contract::resolve(ROUTES, request.method().as_str(), &path);
    let (client_ip, https) = state.cfg.trusted.resolve(peer.ip(), request.headers());
    if let Some(response) = crate::http_route_response::response(
        &decision,
        request.method(),
        request.uri(),
        request.headers(),
        https,
        "",
    ) {
        return response;
    }
    if path == "/api/deploy/github-webhook" {
        let secret = match state
            .settings_snapshot()
            .and_then(|snapshot| snapshot.value("CP_DEPLOY_SECRET"))
        {
            Ok(secret) => secret.unwrap_or_default(),
            Err(_) => return crate::http_cors::internal_error(),
        };
        if secret.is_empty() {
            return hangar_deploy::webhook("", "", "", &[], state.local_commands.as_ref()).await;
        }
        let event = request
            .headers()
            .get("x-github-event")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let signature = request
            .headers()
            .get("x-hub-signature-256")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_owned();
        let bytes = match read_body(request).await {
            Ok(bytes) => bytes,
            Err(response) => return response,
        };
        let response = hangar_deploy::webhook(
            &secret,
            &event,
            &signature,
            &bytes,
            state.local_commands.as_ref(),
        )
        .await;
        if response.status() == StatusCode::UNAUTHORIZED {
            tracing::warn!(code="deploy_bad_signature", ip=%client_ip, "webhook de deploy com assinatura inválida");
        }
        return response;
    }
    if let Err(response) = crate::routes::owner_gate(&state, peer, &request).await {
        return response;
    }
    let preview = path == "/api/preview";
    if preview && request.method() == Method::GET {
        return hangar_port_preview::status(state.local_commands.as_ref()).await;
    }
    if preview && request.method() == Method::DELETE {
        return hangar_port_preview::stop(state.local_commands.as_ref()).await;
    }
    let headers = request.headers().clone();
    let bytes = match read_body(request).await {
        Ok(bytes) => bytes,
        Err(response) => return response,
    };
    if preview {
        let parsed = if bytes.is_empty() {
            serde_json::Value::Null
        } else if crate::http_body_input::is_json(&headers) {
            match crate::orchestration::body::parse(&bytes) {
                Ok(value) => value,
                Err((status, value)) => return crate::http_cors::json_guarded(status, value),
            }
        } else {
            match std::str::from_utf8(&bytes) {
                Ok(text) => json!(text),
                Err(_) => return crate::http_cors::internal_error(),
            }
        };
        let body = ValueGuard(parsed);
        return match crate::login_guests::schema::validate("PreviewBody", &body) {
            Ok(value) => {
                hangar_port_preview::start(
                    state.local_commands.as_ref(),
                    value["port"].as_u64().unwrap() as u16,
                )
                .await
            }
            Err(error) => {
                crate::http_cors::json_guarded(StatusCode::UNPROCESSABLE_ENTITY, error.body)
            }
        };
    }
    let body =
        match crate::engines::body::typed(crate::engines::body::Schema::Cookie, &bytes, &headers) {
            Ok(body) => ValueGuard(body),
            Err(response) => return response,
        };
    let id = body["id"].as_str().unwrap().to_owned();
    let home = state.accounts.env.home.clone();
    // O pedido pode sumir enquanto o arquivo é gravado; a operação termina a invalidação mesmo assim.
    tokio::spawn(async move {
        let saved = tokio::task::spawn_blocking(move || {
            hangar_opencode_cookie::write(
                &home,
                body["id"].as_str().unwrap(),
                body["workspace_id"].as_str().unwrap(),
                body["auth_cookie"].as_str().unwrap(),
            )
        })
        .await;
        match saved {
            Ok(Ok(defined)) => {
                if state.accounts.invalidate_quota(&id).await.is_err() {
                    state.diag.report(
                        "rust.opencode_cookie",
                        "",
                        "quota_invalidation_failed",
                        "cookie gravado; a cota em disco não foi invalidada",
                    );
                }
                crate::http_cors::json_guarded(
                    StatusCode::OK,
                    json!({"id":id,"cookie_definido":defined}),
                )
            }
            Ok(Err(error)) => {
                tracing::warn!(code = "opencode_cookie_write_failed", kind = ?error.kind(), "cookie do OpenCode não foi gravado");
                state.diag.report("rust.opencode_cookie", "", "cookie_write_failed", "cookie do OpenCode não foi gravado");
                crate::http_cors::internal_error()
            }
            Err(_) => {
                state.diag.report("rust.opencode_cookie", "", "cookie_write_interrupted", "gravação do cookie do OpenCode interrompida");
                crate::http_cors::internal_error()
            }
        }
    })
    .await
    .unwrap_or_else(|_| crate::http_cors::internal_error())
}

async fn read_body(request: Request) -> Result<axum::body::Bytes, Response> {
    crate::http_body_limit::read(request.into_body())
        .await
        .map_err(|error| {
            if crate::http_body_limit::is_limit(&error) {
                crate::http_body_limit::rejected()
            } else {
                crate::http_cors::internal_error()
            }
        })
}
