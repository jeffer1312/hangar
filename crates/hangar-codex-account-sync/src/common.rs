use serde_json::{Map, Value, json};

#[derive(Debug)]
pub(crate) enum Error {
    Io(std::io::Error),
    Invalid,
    Changed,
    Conflict,
    Native,
    Toml,
    Unicode,
}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl Error {
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Io(error) => match error.kind() {
                std::io::ErrorKind::NotFound => "FileNotFoundError",
                std::io::ErrorKind::PermissionDenied => "PermissionError",
                std::io::ErrorKind::AlreadyExists => "FileExistsError",
                _ => "OSError",
            },
            Self::Invalid | Self::Conflict => "ValueError",
            Self::Changed => "AlteradoExternamente",
            Self::Native => "RuntimeError",
            Self::Toml => "TOMLDecodeError",
            Self::Unicode => "UnicodeDecodeError",
        }
    }
}
pub(crate) fn object(value: &Value) -> Map<String, Value> {
    value.as_object().cloned().unwrap_or_default()
}
pub(crate) fn hash(raw: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, raw)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub(crate) fn json_bytes(value: &Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap_or_default();
    bytes.push(b'\n');
    bytes
}
pub(crate) fn issue(code: &str, params: Value) -> Value {
    let params = object(&params)
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                Value::String(hangar_core::pycompat::py::py_str(&value)),
            )
        })
        .collect::<Map<_, _>>();
    json!({"code":code,"params":params})
}
pub(crate) fn blocking(issues: &[Value]) -> bool {
    issues.iter().any(|value| {
        !matches!(
            value["code"].as_str(),
            Some("codex_account_mcp_runtime_excluded" | "codex_account_mcp_auth_excluded")
        )
    })
}
pub(crate) fn status(
    status: &str,
    issues: Vec<Value>,
    trust: bool,
    stage: Option<&str>,
    inherited: Option<Value>,
) -> Value {
    json!({"status":status,"trust_pending":trust,"issues":issues,"etapa":stage,"herdado":inherited})
}
pub(crate) fn public_state(state: &Value) -> Value {
    let public = state
        .get("public")
        .filter(|v| v.is_object())
        .unwrap_or(state);
    let raw = public["status"].as_str().unwrap_or("idle");
    let name = if ["idle", "running", "ready", "partial", "error"].contains(&raw) {
        raw
    } else {
        "error"
    };
    let mut issues = Vec::new();
    match public.get("issues") {
        Some(Value::Array(entries)) => {
            for entry in entries {
                if let Some(code) = entry["code"].as_str() {
                    issues.push(issue(code, entry["params"].clone()));
                } else {
                    issues.push(issue("codex_account_state_invalid", json!({})));
                }
            }
        }
        None => {}
        _ => issues.push(issue("codex_account_state_invalid", json!({}))),
    };
    let stage = public["etapa"]
        .as_str()
        .filter(|v| ["configuracoes", "recursos", "plugins"].contains(v));
    let inherited = public["herdado"].as_object().map(|values| {
        Value::Object(
            values
                .iter()
                .filter(|(_, v)| v.is_i64() || v.is_u64())
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
        )
    });
    status(
        name,
        issues,
        public["trust_pending"].as_bool().unwrap_or(false),
        stage,
        inherited,
    )
}
