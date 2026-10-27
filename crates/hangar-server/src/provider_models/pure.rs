//! Leitores e validação: a fonte da lista e a confirmação da sessão têm papéis diferentes.
use super::ModelError;
use num_bigint::BigInt;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::LazyLock,
};

pub const LEVELS: [&str; 7] = ["off", "minimal", "low", "medium", "high", "xhigh", "max"];

pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64() != Some(0.),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
fn fallback(value: Option<&Value>, default: Value) -> Value {
    value.filter(|v| truthy(v)).cloned().unwrap_or(default)
}
pub fn sidecar_path(jsonl: &str, config: &Path) -> PathBuf {
    config.join(".hangar-pi/models").join(format!(
        "{}.json",
        Path::new(jsonl)
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
    ))
}
pub fn pi_catalog(jsonl: &str, config: &Path) -> Option<Value> {
    let data: Value = serde_json::from_slice(
        &crate::runtime::queue::read_replaced(&sidecar_path(jsonl, config)).ok()?,
    )
    .ok()?;
    (data.is_object() && data.get("models").is_some_and(Value::is_array)).then_some(data)
}
pub fn pi_provider(jsonl: &str, config: &Path) -> Option<String> {
    pi_catalog(jsonl, config)?["current"]["provider"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}
pub fn pi_response(catalog: &Value) -> Value {
    json!({"models":catalog.get("models").cloned().unwrap_or(json!([])),"current":catalog.get("current"),"thinking":catalog.get("thinking"),"levels":catalog.get("levels").cloned().unwrap_or(json!([]))})
}
pub fn parse_pi(output: &str) -> Vec<Value> {
    output.lines().filter_map(|line| {
        let c:Vec<_>=line.split_whitespace().collect();
        (c.len()==6&&c[0]!="provider").then(||json!({"provider":c[0],"id":c[1],"context":c[2],"max_out":c[3],"thinking":c[4]=="yes","images":c[5]=="yes"}))
    }).collect()
}
fn integer(value: &Value) -> BigInt {
    let raw = match value {
        Value::Bool(v) => return BigInt::from(u8::from(*v)),
        Value::Number(v) => {
            if v.is_f64() {
                return v
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .and_then(|v| format!("{:.0}", v.trunc()).parse().ok())
                    .unwrap_or_default();
            }
            v.to_string()
        }
        Value::String(v) => v.trim().replace('_', ""),
        _ => return BigInt::default(),
    };
    raw.parse().unwrap_or_default()
}
fn label(value: &Value) -> String {
    let n = integer(value);
    let thousand = BigInt::from(1000);
    let million = BigInt::from(1_000_000);
    if n >= million && &n % &million == BigInt::default() {
        format!("{}M", n / million)
    } else if n >= thousand {
        format!("{}K", n / thousand)
    } else {
        n.to_string()
    }
}
pub fn parse_omp(output: &str) -> Vec<Value> {
    let Ok(value) = serde_json::from_str::<Value>(output) else {
        return vec![];
    };
    let entries: Vec<&Value> = match value.get("models") {
        Some(Value::Array(v)) => v.iter().collect(),
        _ => vec![],
    };
    entries.into_iter().filter(|m|m.is_object()&&truthy(&m["provider"])&&truthy(&m["id"])).map(|m| {
        let image=match m.get("input"){Some(Value::String(s))=>s.contains("image"),Some(Value::Array(v))=>v.contains(&json!("image")),Some(Value::Object(m))=>m.contains_key("image"),_=>false};
        json!({"provider":m["provider"],"id":m["id"],"context":label(&m["contextWindow"]),"max_out":label(&m["maxTokens"]),"thinking":truthy(&m["thinking"]),"images":image})
    }).collect()
}
pub fn kimi_catalog(home: &Path) -> Option<Value> {
    read_kimi(&home.join(".kimi-code/config.toml"))
}
pub fn read_kimi(path: &Path) -> Option<Value> {
    parse_kimi(&crate::runtime::queue::read_replaced_to_string(path).ok()?)
}
pub fn parse_kimi(text: &str) -> Option<Value> {
    let value: toml::Value = toml::from_str(text).ok()?;
    let table = value.get("models")?.as_table()?;
    if table.is_empty() {
        return None;
    }
    let mut sorted: Vec<_> = table.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));
    let models:Vec<Value>=sorted.into_iter().filter_map(|(alias,model)|{
        let model=model.as_table()?;
        let get=|key:&str|model.get(key).and_then(|v|serde_json::to_value(v).ok());
        let (provider,mid)=alias.split_once('/').unwrap_or((alias,""));
        Some(json!({"alias":alias,"provider":provider,"id":fallback(get("model").as_ref(),json!(mid)),"name":fallback(get("display_name").as_ref(),json!(mid)),"context_length":get("max_context_size"),"efforts":fallback(get("support_efforts").as_ref(),json!([])),"default_effort":get("default_effort")}))
    }).collect();
    Some(
        json!({"default":value.get("default_model").and_then(|v|serde_json::to_value(v).ok()),"models":models}),
    )
}
fn space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}
pub fn repr(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c)
            }
            c if c < ' ' || c == '\u{7f}' => out.push_str(&format!("\\x{:02x}", c as u32)),
            _ => out.push(c),
        }
    }
    out.push(quote);
    out
}
pub fn clean(value: &str, field: &str) -> Result<String, ModelError> {
    let value2 = value.trim_matches(space);
    if value2.is_empty() || value2.chars().any(|c| space(c) || (c as u32) < 32) {
        return Err(ModelError::text(
            422,
            format!("{field} invalido: {}", repr(value)),
        ));
    }
    Ok(value2.into())
}
pub fn model_command(provider: &str, model: &str) -> Result<String, ModelError> {
    Ok(format!(
        "/cp-model {} {}",
        clean(provider, "provider")?,
        clean(model, "model")?
    ))
}
pub fn think_command(level: &str) -> Result<String, ModelError> {
    let v = level.trim_matches(space).to_lowercase();
    if !LEVELS.contains(&v.as_str()) {
        return Err(ModelError::text(
            422,
            format!("nivel desconhecido: {}", repr(level)),
        ));
    }
    Ok(format!("/cp-think {v}"))
}
pub fn check_pi(catalog: &Value, provider: &str, model: &str) -> Result<(), ModelError> {
    if catalog["models"].as_array().is_some_and(|v| {
        v.iter()
            .any(|m| m.is_object() && m["provider"] == provider && m["id"] == model)
    }) {
        Ok(())
    } else {
        Err(ModelError::text(
            422,
            format!("modelo fora do catalogo: {provider}/{model}"),
        ))
    }
}
pub fn check_kimi(catalog: &Value, alias: &str) -> Result<Value, ModelError> {
    let a = clean(alias, "alias")?;
    catalog["models"]
        .as_array()
        .and_then(|v| v.iter().find(|m| m["alias"] == a))
        .cloned()
        .ok_or_else(|| ModelError::text(422, format!("modelo fora do catalogo: {a}")))
}
pub fn check_effort(entry: &Value, level: &str) -> Result<String, ModelError> {
    let normalized = level.trim_matches(space).to_lowercase();
    let levels: Vec<String> = entry["efforts"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| v.to_string())
                .to_lowercase()
        })
        .collect();
    if !levels.contains(&normalized) {
        let hint = if levels.is_empty() {
            "nenhum — modelo sem níveis".into()
        } else {
            levels.join(", ")
        };
        return Err(ModelError::text(
            422,
            format!(
                "nível fora do suporte de {}: {} (use um de {hint})",
                entry["alias"].as_str().unwrap_or("None"),
                repr(level)
            ),
        ));
    }
    Ok(normalized)
}
pub fn confirms(
    catalog: &Value,
    provider: Option<&str>,
    model: Option<&str>,
    level: Option<&str>,
) -> bool {
    if let Some(model) = model.filter(|v| !v.is_empty()) {
        if !catalog["current"].is_object()
            || catalog["current"]["provider"].as_str() != provider
            || catalog["current"]["id"] != model
        {
            return false;
        }
    }
    if let Some(level) = level.filter(|v| !v.is_empty()) {
        let v = level.trim_matches(space).to_lowercase();
        if catalog["levels"]
            .as_array()
            .is_some_and(|levels| levels.contains(&json!(v)))
            && catalog["thinking"] != v
        {
            return false;
        }
    }
    true
}
static SWITCHED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"Switched to (.+?) with thinking (\S+?)( for this session only)?\.\s*$").unwrap()
});
static THINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Thinking set to (\S+?) for this session only\.\s*$").unwrap());
static BRACKET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[\s*([A-Za-z]+)\s*\]").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[A-Za-z]+").unwrap());
pub fn parse_switched(pane: &str) -> Option<Value> {
    pane.lines().filter_map(|line|SWITCHED.captures(line).map(|m|json!({"name":m[1].to_owned(),"level":m[2].to_owned(),"session_only":m.get(3).is_some(),"raw":line.trim()}))).last()
}
pub fn parse_thinking_set(pane: &str) -> Option<Value> {
    pane.lines()
        .filter_map(|line| {
            THINK
                .captures(line)
                .map(|m| json!({"level":m[1].to_owned(),"raw":line.trim()}))
        })
        .last()
}
pub fn parse_thinking_row(pane: &str) -> Option<Value> {
    let lines: Vec<_> = pane.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("Thinking") || !line.contains("switch") {
            continue;
        }
        for next in lines.iter().skip(i + 1).take(2) {
            if let Some(m) = BRACKET.captures(next) {
                let levels: Vec<_> = WORD.find_iter(next).map(|m| m.as_str()).collect();
                if levels.contains(&&m[1]) {
                    return Some(json!({"levels":levels,"current":m[1].to_owned()}));
                }
            }
        }
    }
    None
}
