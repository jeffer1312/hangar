use std::{collections::BTreeMap, path::Path};

use serde_json::{Value, json};

/// A publicação viva vence o ambiente; arquivo ausente ou corrompido conserva o padrão.
pub fn configured(path: &Path, fallback: &str) -> String {
    hangar_core::fs::read_replaced(path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|value| {
            value
                .get("editor")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_else(|| fallback.into())
}

pub async fn open(
    binary: &str,
    cwd: &str,
    env: &BTreeMap<String, String>,
) -> Result<Value, crate::Error> {
    let mut command = tokio::process::Command::new(binary);
    command
        .arg(cwd)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let mut child = command.spawn().map_err(|error| crate::Error {
        status: 500,
        code: "erro_editor_falhou",
        msg: format!("editor '{binary}' falhou: {error}"),
        params: json!({"binario":binary,"erro":error.to_string()}),
    })?;
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    Ok(json!({"ok":true}))
}
