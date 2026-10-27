//! Consultas locais para consumidores que precisam de fatos antes de o servidor abrir a porta.
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::ExitCode;

pub fn wrong_method(method: &axum::http::Method, path: &str, query: Option<&str>) -> Option<axum::response::Response> {
    use axum::http::{Method, StatusCode, header};
    if crate::orchestration::matches(method, path) || crate::provider_models::matches(method, path)
        || crate::provider_models::catalog_request(method, path, query) || crate::omp_sync::matches(method, path) {
        return None;
    }
    let allowed = crate::provider_models::allowed_method(path, query).or_else(|| {
        [Method::GET, Method::PUT, Method::POST, Method::DELETE].into_iter()
            .find(|method| crate::orchestration::matches(method, path) || crate::omp_sync::matches(method, path))
    })?;
    let mut response = crate::session_write::json_response(StatusCode::METHOD_NOT_ALLOWED, json!({"detail":"Method Not Allowed"}));
    response.headers_mut().insert(header::ALLOW, allowed.as_str().parse().ok()?);
    Some(response)
}

fn input() -> Result<Value, String> {
    let mut bytes = Vec::new();
    std::io::stdin().read_to_end(&mut bytes).map_err(|_| "provider_input_unreadable")?;
    serde_json::from_slice(&bytes).map_err(|_| "provider_input_invalid".into())
}

fn dispatch(command: &str, args: &[String], value: &Value) -> Result<Value, String> {
    if command == "orq-info" {
        let operation = args.first().ok_or("orq_operation_missing")?;
        return crate::orchestration::command(operation, value);
    }
    match value["op"].as_str() {
        Some("read-only") => {
            let env: BTreeMap<String, String> = serde_json::from_value(value["env"].clone()).map_err(|_| "provider_environment_invalid")?;
            let home = value["home"].as_str().ok_or("provider_home_missing")?;
            let cwd = value["cwd"].as_str().ok_or("provider_cwd_missing")?;
            let config = value["config_dir"].as_str();
            let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().map_err(|_| "provider_runtime_failed")?;
            let git_env = env.iter().filter(|(key, _)| !key.starts_with("GIT_")).map(|(key, value)| (key.clone(), value.clone())).collect();
            let run = |argv: Vec<String>, timeout| -> futures_util::future::BoxFuture<'_, crate::session_open::terminal::MuxOutput> {
                let environment = if argv.first().is_some_and(|program| program == "git") { &git_env } else { &env };
                Box::pin(crate::session_open::server::run_program(environment, argv, timeout))
            };
            let prefix = runtime.block_on(crate::session_open::open::read_only_envelope(&env, Path::new(home), cwd, config, &run))
                .map_err(|error| error.msg)?;
            Ok(json!(prefix))
        }
        Some("omp-directory") => {
            let home = value["home"].as_str().filter(|home| !home.is_empty()).ok_or("provider_home_missing")?;
            let env: BTreeMap<String, String> = serde_json::from_value(value["env"].clone()).map_err(|_| "provider_environment_invalid")?;
            let strict = value.get("strict").map_or(Some(false), Value::as_bool).ok_or("provider_strict_invalid")?;
            let (directory, warning) = crate::omp_sync::agent_directory(Path::new(home), &env, strict)?;
            Ok(json!({"agent_dir": directory, "warning": warning}))
        }
        _ => Err("provider_operation_unknown".into()),
    }
}

pub fn cli(command: &str, args: &[String]) -> ExitCode {
    let result = input().and_then(|value| dispatch(command, args, &value));
    let (code, value) = match result {
        Ok(result) => (ExitCode::SUCCESS, json!({"ok": true, "result": result})),
        Err(message) => (ExitCode::from(2), json!({"ok": false, "error": message})),
    };
    if writeln!(std::io::stdout().lock(), "{value}").is_err() { return ExitCode::from(1); }
    code
}
