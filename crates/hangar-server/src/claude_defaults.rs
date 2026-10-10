//! Padrão do Claude marcado no app: vai ao settings.json principal, que o espelho leva às contas.
use crate::routes::{AppState, gate, pass};
use crate::session_write::{detail_body, json_response};
use axum::body::to_bytes;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Map, Value, json};
use std::{net::SocketAddr, path::Path, sync::Arc};

const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const ALIASES: &[&str] = &["best", "opus", "opusplan", "sonnet", "haiku", "fable"];

/// Só id que a API da Anthropic aceita: modelo de motor no `model` global quebraria as contas Anthropic.
fn anthropic(model: &str) -> bool {
    let base = model.strip_suffix("[1m]").unwrap_or(model);
    ALIASES.contains(&base) || (base.starts_with("claude-") && !base.contains('/'))
}

/// `manual` é o nome da flag; no settings.json o mesmo modo se chama `default`.
fn settings_mode(permission: &str) -> Option<&'static str> {
    Some(match permission {
        "manual" => "default",
        "acceptEdits" => "acceptEdits",
        "auto" => "auto",
        "bypassPermissions" => "bypassPermissions",
        "dontAsk" => "dontAsk",
        "plan" => "plan",
        _ => return None,
    })
}

pub async fn save(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
) -> Response {
    let (forward, owner) = gate(&state, peer, &request);
    if !owner {
        return pass(&state, request, &forward).await;
    }
    let body = to_bytes(request.into_body(), 16 * 1024)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let Some(body) = body else {
        return failure(StatusCode::BAD_REQUEST, "claude_defaults_invalid");
    };
    let path = state.accounts.env.home.join(".claude").join("settings.json");
    match tokio::task::spawn_blocking(move || apply(&path, &body)).await {
        Ok(Ok(written)) => json_response(StatusCode::OK, json!({"ok": true, "written": written})),
        Ok(Err((status, code))) => failure(status, code),
        Err(_) => failure(StatusCode::INTERNAL_SERVER_ERROR, "claude_settings_write_failed"),
    }
}

fn failure(status: StatusCode, code: &str) -> Response {
    let msg = match code {
        "claude_defaults_invalid" => "valor de modelo, esforço ou permissão inválido",
        "claude_settings_unreadable" => "o settings.json principal não é um JSON legível; nada foi mexido",
        _ => "não consegui gravar o settings.json principal",
    };
    json_response(status, detail_body(code, msg, json!({})))
}

type Failure = (StatusCode, &'static str);

fn text<'a>(body: &'a Value, key: &str) -> Result<&'a str, Failure> {
    match &body[key] {
        Value::Null => Ok(""),
        Value::String(s) => Ok(s),
        _ => Err((StatusCode::BAD_REQUEST, "claude_defaults_invalid")),
    }
}

/// Devolve as chaves gravadas. Arquivo que não é objeto JSON não é tocado: perder a config inteira é pior.
fn apply(path: &Path, body: &Value) -> Result<Vec<&'static str>, Failure> {
    let invalid = (StatusCode::BAD_REQUEST, "claude_defaults_invalid");
    let (model, effort, permission) = (text(body, "model")?, text(body, "effort")?, text(body, "permission")?);
    if !effort.is_empty() && !EFFORTS.contains(&effort) {
        return Err(invalid);
    }
    let mode = match permission {
        "" => None,
        other => Some(settings_mode(other).ok_or(invalid)?),
    };
    // "Padrão" e modelo de motor não vão ao arquivo: o primeiro é o próprio settings, o segundo é de outra API.
    let model = (!model.is_empty() && model != "default" && anthropic(model)).then_some(model);
    let unreadable = (StatusCode::CONFLICT, "claude_settings_unreadable");
    let mut settings: Map<String, Value> = match std::fs::read(path) {
        Ok(bytes) if bytes.iter().all(u8::is_ascii_whitespace) => Map::new(),
        Ok(bytes) => match serde_json::from_slice(&bytes) {
            Ok(Value::Object(map)) => map,
            _ => return Err(unreadable),
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(_) => return Err(unreadable),
    };
    let mut written = Vec::new();
    if let Some(model) = model {
        settings.insert("model".into(), json!(model));
        written.push("model");
    }
    if !effort.is_empty() {
        settings.insert("effortLevel".into(), json!(effort));
        written.push("effortLevel");
    }
    if let Some(mode) = mode {
        let permissions = settings.entry("permissions").or_insert_with(|| json!({}));
        let Some(permissions) = permissions.as_object_mut() else { return Err(unreadable) };
        permissions.insert("defaultMode".into(), json!(mode));
        written.push("permissions.defaultMode");
    }
    if written.is_empty() {
        return Ok(written);
    }
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(settings))
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "claude_settings_write_failed"))?;
    bytes.push(b'\n');
    crate::runtime::queue::atomic_write(path, &bytes)
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "claude_settings_write_failed"))?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(start: Option<&str>, body: Value) -> (Result<Vec<&'static str>, Failure>, Option<Value>) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        if let Some(start) = start {
            std::fs::write(&path, start).unwrap();
        }
        let result = apply(&path, &body);
        let after = std::fs::read(&path).ok().and_then(|b| serde_json::from_slice(&b).ok());
        (result, after)
    }

    #[test]
    fn writes_the_three_keys_and_keeps_the_rest() {
        let (result, after) = run(
            Some(r#"{"hooks":{"x":1},"permissions":{"allow":["Bash"]}}"#),
            json!({"model":"opus[1m]","effort":"high","permission":"bypassPermissions"}),
        );
        assert_eq!(result.unwrap(), vec!["model", "effortLevel", "permissions.defaultMode"]);
        let after = after.unwrap();
        assert_eq!(after["model"], "opus[1m]");
        assert_eq!(after["effortLevel"], "high");
        assert_eq!(after["permissions"], json!({"allow":["Bash"],"defaultMode":"bypassPermissions"}));
        assert_eq!(after["hooks"], json!({"x":1}));
    }

    #[test]
    fn default_and_engine_models_never_reach_the_file() {
        for model in ["default", "", "claude-200-2/gpt-6.1-sol", "kimi-for-coding"] {
            let (result, after) = run(Some(r#"{"model":"opus"}"#), json!({"model":model,"effort":"","permission":""}));
            assert!(result.unwrap().is_empty());
            assert_eq!(after.unwrap()["model"], "opus");
        }
    }

    #[test]
    fn manual_becomes_default_mode() {
        let (_, after) = run(None, json!({"model":null,"effort":null,"permission":"manual"}));
        assert_eq!(after.unwrap()["permissions"]["defaultMode"], "default");
    }

    #[test]
    fn broken_file_and_unknown_values_are_refused_untouched() {
        let (result, _) = run(Some("{not json"), json!({"effort":"high"}));
        assert_eq!(result.unwrap_err().1, "claude_settings_unreadable");
        let (result, after) = run(Some(r#"{"effortLevel":"low"}"#), json!({"effort":"turbo"}));
        assert_eq!(result.unwrap_err().1, "claude_defaults_invalid");
        assert_eq!(after.unwrap()["effortLevel"], "low");
    }
}
