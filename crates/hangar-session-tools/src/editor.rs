use std::collections::BTreeMap;

use serde_json::{Value, json};

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
