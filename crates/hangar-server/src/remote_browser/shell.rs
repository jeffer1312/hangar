//! O controlador relê seu endereço e credencial a cada comando.
use super::{input, ws::Failure};
use serde_json::{Value, json};
use std::{error::Error, io, path::Path};
const NOT_OPEN: &str = "o app desktop não está aberto nesta máquina";

fn unavailable(error: reqwest::Error, before_headers: bool) -> Failure {
    let connecting = error.is_connect();
    let mut causes = vec![error.to_string()];
    let mut refused = None;
    let mut incomplete = false;
    let mut cause = error.source();
    while let Some(current) = cause {
        causes.push(current.to_string());
        if let Some(current) = current.downcast_ref::<io::Error>() {
            if connecting && current.kind() == io::ErrorKind::ConnectionRefused {
                if let Some(code) = current.raw_os_error() {
                    let message = current.to_string();
                    let suffix = format!(" (os error {code})");
                    let message = message.strip_suffix(&suffix).unwrap_or(&message);
                    let kind = if cfg!(windows) { "WinError" } else { "Errno" };
                    refused = Some(format!("<urlopen error [{kind} {code}] {message}>"));
                }
            }
        }
        if let Some(current) = current.downcast_ref::<hyper::Error>() {
            incomplete |= current.is_incomplete_message();
        }
        cause = current.source();
    }
    let reason = if error.is_timeout() {
        if connecting { "<urlopen error timed out>".to_owned() } else { "timed out".to_owned() }
    } else if let Some(reason) = refused {
        reason
    } else if before_headers && incomplete {
        // Um corpo interrompido não é a ausência de resposta HTTP.
        "Remote end closed connection without response".to_owned()
    } else {
        causes.join(": ")
    };
    Failure::plain(format!("o app desktop não respondeu: {reason}"))
}

pub(super) fn sidecar(dir: &Path, name: &str, target: bool) -> Option<Value> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "json")
            || entry.file_name().to_string_lossy().starts_with('_')
        {
            continue;
        }
        let value = match std::fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|raw| serde_json::from_str::<Value>(&raw).map_err(|e| e.to_string()))
        {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(target:"hangar.navsock","sidecar de navegador ilegivel {}: {}",path.display(),error);
                continue;
            }
        };
        if !value.is_object() {
            continue;
        }
        let key = string_field(&value, "chave");
        if (key == name || key.ends_with(&format!("::{name}")))
            && (!target || input::truth(&value["targetId"]))
        {
            return Some(value);
        }
    }
    None
}
pub(super) fn string_field(value: &Value, field: &str) -> String {
    value.get(field).map(input::string).unwrap_or_default()
}
pub(super) fn server(dir: &Path) -> Result<Value, Failure> {
    let value = std::fs::read(dir.join("_srv.json"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .filter(Value::is_object)
        .ok_or_else(|| Failure::plain(NOT_OPEN))?;
    if !input::truth(&value["porta"]) || !input::truth(&value["token"]) {
        return Err(Failure::plain(NOT_OPEN));
    }
    Ok(value)
}
pub(super) async fn verb(
    http: &reqwest::Client,
    dir: &Path,
    name: &str,
    args: &[String],
) -> Result<String, Failure> {
    let dir = dir.to_owned();
    let name = name.to_owned();
    let (srv, key) = tokio::task::spawn_blocking(move || {
        let srv = server(&dir)?;
        let key = sidecar(&dir, &name, false)
            .filter(|sc| input::truth(&sc["chave"]))
            .map(|sc| string_field(&sc, "chave"))
            .ok_or_else(|| Failure::plain("esta sessão não tem navegador aberto"))?;
        Ok::<_, Failure>((srv, key))
    })
    .await
    .map_err(|_| Failure::plain(NOT_OPEN))??;
    let port = input::integer(&srv["porta"])?;
    let response = http
        .post(format!("http://127.0.0.1:{port}/cmd"))
        .bearer_auth(string_field(&srv, "token"))
        .header("content-type", "application/json")
        .body(json!({"chave":key,"verbo":"layout","args":args}).to_string())
        .send()
        .await
        .map_err(|error| unavailable(error, true))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|error| unavailable(error, false))?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}
