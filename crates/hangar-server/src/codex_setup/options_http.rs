//! Validação tipada das opções; o interruptor antigo da voz continua aceito e ignorado.
use crate::{config_sync::runtime_values::ValueGuard, orchestration::codec, routes::AppState};
use axum::{
    extract::Request,
    http::{Method, StatusCode},
    response::Response,
};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
fn invalid(errors: Vec<Value>) -> Response {
    crate::engines::body::json_error(StatusCode::UNPROCESSABLE_ENTITY, json!({"detail":errors}))
}
fn validate(value: &Value) -> Result<bool, Response> {
    let issue = |kind: &str, field: Option<&str>, input: &Value, message: &str| {
        let mut location = vec![json!("body")];
        if let Some(field) = field {
            location.push(json!(field));
        }
        json!({"type":kind,"loc":location,"msg":message,"input":codec::clone_value(input)})
    };
    let Some(object) = value.as_object() else {
        return Err(invalid(vec![issue(
            if value.is_null() {
                "missing"
            } else {
                "model_attributes_type"
            },
            None,
            value,
            if value.is_null() {
                "Field required"
            } else {
                "Input should be a valid dictionary or object to extract fields from"
            },
        )]));
    };
    let mut errors = Vec::new();
    match object.get("contexto_estendido") {
        None => errors.push(issue(
            "missing",
            Some("contexto_estendido"),
            value,
            "Field required",
        )),
        Some(value) if !value.is_boolean() => errors.push(issue(
            "bool_type",
            Some("contexto_estendido"),
            value,
            "Input should be a valid boolean",
        )),
        _ => {}
    }
    if let Some(value) = object
        .get("codex_voice_beta")
        .filter(|value| !value.is_null() && !value.is_boolean())
    {
        errors.push(issue(
            "bool_type",
            Some("codex_voice_beta"),
            value,
            "Input should be a valid boolean",
        ));
    }
    for (name, value) in object {
        if !matches!(name.as_str(), "contexto_estendido" | "codex_voice_beta") {
            errors.push(issue(
                "extra_forbidden",
                Some(name),
                value,
                "Extra inputs are not permitted",
            ));
        }
    }
    if errors.is_empty() {
        Ok(object["contexto_estendido"].as_bool().unwrap())
    } else {
        Err(invalid(errors))
    }
}
pub async fn handle(state: Arc<AppState>, peer: SocketAddr, mut request: Request) -> Response {
    let post = request.method() == Method::POST;
    let mut value = ValueGuard(Value::Null);
    if let Some(response) = crate::http_body_limit::precheck(request.headers()) {
        return response;
    }
    if post {
        let body = std::mem::replace(request.body_mut(), axum::body::Body::empty());
        let bytes = match crate::http_body_limit::read(body).await {
            Ok(bytes) => bytes,
            Err(error) if crate::http_body_limit::is_limit(&error) => {
                return crate::http_body_limit::rejected();
            }
            Err(_) => return crate::engines::body::internal_error(),
        };
        if !bytes.is_empty() {
            value = ValueGuard(if crate::http_body_input::is_json(request.headers()) {
                match crate::orchestration::body::parse(&bytes) {
                    Ok(value) => value,
                    Err((status, value)) => return crate::engines::body::json_error(status, value),
                }
            } else {
                match std::str::from_utf8(&bytes) {
                    Ok(value) => json!(value),
                    Err(_) => return crate::engines::body::internal_error(),
                }
            });
        }
    }
    let login_read = !post
        && request
            .extensions()
            .get::<crate::login_guests::gate::Login>()
            .is_some();
    if !login_read {
        if let Err(response) = crate::routes::owner_gate(&state, peer, &request).await {
            return response;
        }
    }
    let enabled = if post {
        match validate(&value) {
            Ok(enabled) => Some(enabled),
            Err(response) => return response,
        }
    } else {
        None
    };
    let config = match crate::config_sync::runtime::RuntimeConfig::from_state(&state) {
        Ok(config) => config,
        Err(_) => return crate::engines::body::internal_error(),
    };
    let result = if let Some(enabled) = enabled {
        super::options::save(config, enabled).await
    } else {
        match tokio::task::spawn_blocking(move || super::options::read(&config)).await {
            Ok(result) => result,
            Err(error) => Err(error.to_string()),
        }
    };
    match result {
        Ok(value) => codec::response(StatusCode::OK, value),
        Err(_) if post => crate::provider_models::ModelError::coded(
            409,
            "erro_codex_opcoes",
            "Não foi possível salvar as opções do Codex.",
            json!({}),
        )
        .response(),
        Err(_) => crate::engines::body::internal_error(),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_context_voice_and_extra_validation() {
        for voice in [Value::Null, json!(true), json!(false)] {
            assert_eq!(
                validate(&json!({"contexto_estendido":true,"codex_voice_beta":voice})).unwrap(),
                true
            );
        }
        for input in [
            json!({}),
            json!({"contexto_estendido":1}),
            json!({"contexto_estendido":"true"}),
            json!({"contexto_estendido":true,"codex_voice_beta":0}),
            json!({"contexto_estendido":true,"extra":true}),
        ] {
            assert_eq!(
                validate(&input).unwrap_err().status(),
                StatusCode::UNPROCESSABLE_ENTITY
            );
        }
    }
}

#[cfg(test)]
mod route_tests {
    use super::*;
    #[tokio::test]
    async fn options_are_native_and_preserve_syntax_auth_validation_order() {
        let home = tempfile::tempdir().unwrap();
        let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let mut state = AppState::new(crate::config::Config {
            listen: addr,
            upstream: addr,
            internal_secret: "fixture-internal".into(),
            auth_token: "fixture-owner".into(),
            log_path: None,
            trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
        });
        state.accounts = crate::accounts::AccountService::new(
            crate::accounts::environment::AccountEnvironment::from_map(
                [
                    ("HOME".into(), home.path().display().to_string()),
                    ("USERPROFILE".into(), home.path().display().to_string()),
                    ("PATH".into(), String::new()),
                ]
                .into(),
            ),
        );
        let state = Arc::new(state);
        for (method, auth, body, expected) in [
            ("GET", true, "", 200),
            ("GET", false, "", 401),
            ("POST", false, "{", 422),
            ("POST", false, "{}", 401),
            ("POST", true, "{}", 422),
            ("POST", true, r#"{"contexto_estendido":1}"#, 422),
            (
                "POST",
                true,
                r#"{"contexto_estendido":false,"codex_voice_beta":true}"#,
                200,
            ),
        ] {
            let mut request = Request::builder()
                .method(method)
                .uri("/api/harness/codex/opcoes")
                .header("content-type", "application/json");
            if auth {
                request = request.header("authorization", "Bearer fixture-owner");
            }
            let response = crate::harness_setup::routing::handle(
                state.clone(),
                addr,
                request.body(axum::body::Body::from(body)).unwrap(),
            )
            .await;
            assert_eq!(
                response.status().as_u16(),
                expected,
                "{method} auth={auth} body={body}"
            );
            if expected == 200 {
                let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
                    .await
                    .unwrap();
                let value: Value = serde_json::from_slice(&bytes).unwrap();
                assert_eq!(value["codex_voice_beta"], true);
            }
        }
    }
}
