//! Consumidores locais do painel: projeção curta das sessões e resolução de programas.
use serde_json::Value;
use std::path::Path;

pub(super) const KEEP: &[&str] = &[
    "name",
    "cwd",
    "state",
    "branch",
    "status_line",
    "question",
    "options",
    "stalled",
    "limited",
    "pair_peers",
    "provider",
    "last_activity",
    "git_dirty",
    "git_ahead",
    "git_behind",
];
pub fn fields(value: &Value) -> (String, String, String) {
    let field = |key| {
        crate::transcript::py::strip(value.get(key).filter(|value|!crate::orchestration::codec::contains_nonfinite(value)).and_then(Value::as_str).unwrap_or(""))
            .to_owned()
    };
    (field("base_url"), field("token"), field("web_url"))
}
pub fn enabled(value: &Value) -> bool {
    value.get("enabled") != Some(&Value::Bool(false))
}
pub fn attached(output: &str) -> std::collections::BTreeSet<String> {
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("0"), Some(name), None) if !name.is_empty() => Some(name.into()),
                _ => None,
            }
        })
        .collect()
}
pub fn detect_command(path: &Path) -> String {
    let Some(package) = std::fs::read(path.join("package.json"))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
    else {
        return String::new();
    };
    let Some(scripts) = package.get("scripts").and_then(Value::as_object) else {
        return String::new();
    };
    let script = if scripts.contains_key("dev") {
        "dev"
    } else if scripts.contains_key("start") {
        "start"
    } else {
        return String::new();
    };
    let manager = if path.join("pnpm-lock.yaml").exists() {
        "pnpm"
    } else if path.join("yarn.lock").exists() {
        "yarn"
    } else if path.join("bun.lockb").exists() {
        "bun"
    } else if path.join("package-lock.json").exists() {
        "npm"
    } else {
        match package
            .get("packageManager")
            .and_then(Value::as_str)
            .unwrap_or("")
            .split('@')
            .next()
            .unwrap_or("")
        {
            "pnpm" => "pnpm",
            "yarn" => "yarn",
            "bun" => "bun",
            _ => "npm",
        }
    };
    format!(
        "{manager}{}{script}",
        if matches!(manager, "pnpm" | "yarn") {
            " "
        } else {
            " run "
        }
    )
}
