//! Validação dos formulários na ordem dos campos publicada pelo FastAPI.
use super::store::{Failure, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Kind {
    Text(usize, usize),
    Boolean(bool),
    Integer,
    Dictionary,
    Blob,
    Credential(usize),
    User(bool),
}
#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    kind: Kind,
    optional: bool,
}
fn field(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        kind,
        optional: false,
    }
}
fn optional(name: &'static str, kind: Kind) -> Field {
    Field {
        name,
        kind,
        optional: true,
    }
}
fn schema(name: &str) -> (Vec<Field>, bool) {
    use Kind::*;
    match name {
        "NewGuest" | "GuestSettings" => {
            let mut fields = vec![
                field("root", Text(1, 4096)),
                optional("sees_owner", Boolean(false)),
                optional("owner_sees", Boolean(true)),
            ];
            if name == "NewGuest" {
                fields.push(field("name", Text(1, 100)));
            }
            (fields, false)
        }
        "SetupBody" => (
            vec![
                optional("user", User(false)),
                optional("salt", Credential(16)),
                optional("auth_hash", Credential(32)),
                optional("enc_blob", Blob),
            ],
            true,
        ),
        "RegisterBody" => (
            ["user", "salt", "auth_hash", "bootstrap"]
                .into_iter()
                .map(|n| field(n, Text(0, usize::MAX)))
                .collect(),
            false,
        ),
        "LoginBody" => (
            ["user", "auth_hash"]
                .into_iter()
                .map(|n| field(n, Text(0, usize::MAX)))
                .collect(),
            false,
        ),
        "VaultPutBody" => (
            vec![field("enc_blob", Dictionary), field("base_rev", Integer)],
            false,
        ),
        "GuestAccountBody" => (
            vec![
                field("user", User(true)),
                field("salt", Credential(16)),
                field("auth_hash", Credential(32)),
                field("enc_blob", Blob),
                field("admin_blob", Blob),
            ],
            true,
        ),
        "SetupBlob" => (
            vec![
                field("iv", Text(16, 16)),
                field("data", Text(24, 104857600)),
            ],
            true,
        ),
        _ => (vec![], false),
    }
}
fn issue(loc: Vec<Value>, kind: &str, message: &str, input: &Value, ctx: Option<Value>) -> Value {
    let mut value = json!({"type":kind,"loc":loc,"msg":message,"input":input});
    if let Some(ctx) = ctx {
        value["ctx"] = ctx;
    }
    value
}
fn error(loc: Vec<Value>, message: &str, input: &Value) -> Value {
    issue(
        loc,
        "value_error",
        &format!("Value error, {message}"),
        input,
        Some(json!({"error":{}})),
    )
}
fn push_failure(errors: &mut Vec<Value>, failure: Failure, loc: &[Value]) {
    for mut e in failure.body["detail"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let mut at = loc.to_vec();
        at.extend(e["loc"].as_array().unwrap().iter().skip(2).cloned());
        e["loc"] = json!(at);
        errors.push(e);
    }
}
fn base64_bytes(text: &str) -> std::result::Result<Vec<u8>, String> {
    if !text.is_ascii() {
        return Err("string argument should contain only ASCII characters".into());
    }
    if text
        .bytes()
        .any(|b| !b.is_ascii_alphanumeric() && !b"+/=".contains(&b))
    {
        return Err("Only base64 data is allowed".into());
    }
    STANDARD.decode(text).map_err(|_|{
        let count=text.bytes().take_while(|b|*b!=b'=').count();
        if count%4==1{format!("Invalid base64-encoded string: number of data characters ({count}) cannot be 1 more than a multiple of 4")}
        else if text.starts_with('='){"Leading padding not allowed".into()}
        else if text.find('=').is_some_and(|i|text[i..].bytes().any(|b|b!=b'=')){"Discontinuous padding not allowed".into()}
        else{"Incorrect padding".into()}
    })
}
fn integer(value: &Value) -> Option<Value> {
    if let Some(b) = value.as_bool() {
        return Some(json!(u8::from(b)));
    }
    let raw = match value {
        Value::String(s) => {
            let s = s.trim();
            let bytes = s.as_bytes();
            if bytes.iter().enumerate().any(|(i, b)| {
                *b == b'_'
                    && (i == 0
                        || i + 1 == bytes.len()
                        || !bytes[i - 1].is_ascii_digit()
                        || !bytes[i + 1].is_ascii_digit())
            }) {
                return None;
            }
            s.replace('_', "")
        }
        Value::Number(n) => {
            let raw = n.to_string();
            if raw.contains(['e', 'E']) {
                let f = n.as_f64()?;
                if !f.is_finite() || f.fract() != 0.0 {
                    return None;
                }
                format!("{f:.0}")
            } else {
                raw
            }
        }
        _ => return None,
    };
    let raw = raw.strip_prefix('+').unwrap_or(&raw);
    let (negative, raw) = raw.strip_prefix('-').map_or((false, raw), |v| (true, v));
    let raw = if let Some((whole, fraction)) = raw.split_once('.') {
        if fraction.is_empty() || !fraction.bytes().all(|b| b == b'0') {
            return None;
        }
        whole
    } else {
        raw
    };
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let digits = raw.trim_start_matches('0');
    let digits = if digits.is_empty() { "0" } else { digits };
    serde_json::from_str(&format!(
        "{}{digits}",
        if negative && digits != "0" { "-" } else { "" }
    ))
    .ok()
}
fn collect(name: &str, body: &Value, loc: Vec<Value>, errors: &mut Vec<Value>) -> Value {
    let (fields, extras) = schema(name);
    let Some(object) = body.as_object() else {
        errors.push(issue(
            loc,
            "model_attributes_type",
            "Input should be a valid dictionary or object to extract fields from",
            body,
            None,
        ));
        return Value::Null;
    };
    let mut output = serde_json::Map::new();
    let before = errors.len();
    for f in &fields {
        let mut at = loc.clone();
        at.push(json!(f.name));
        let Some(value) = object.get(f.name) else {
            if !f.optional {
                errors.push(issue(at, "missing", "Field required", body, None));
            }
            continue;
        };
        if f.optional && value.is_null() && !matches!(f.kind, Kind::Boolean(_)) {
            output.insert(f.name.into(), Value::Null);
            continue;
        }
        let string_kind = match f.kind {
            Kind::Text(min, max) => Some((min, max)),
            Kind::Credential(size) => Some((0, if size == 16 { 24 } else { 44 })),
            Kind::User(_) => Some((1, 100)),
            _ => None,
        };
        let normalized = if let Some((min, max)) = string_kind {
            match super::validation::text(body, f.name, min, max) {
                Err(e) => {
                    push_failure(errors, e, &at);
                    continue;
                }
                Ok(text) => match f.kind {
                    Kind::User(guest) => {
                        let trimmed = text.trim();
                        if trimmed.is_empty() || (guest && trimmed.contains('/')) {
                            errors.push(error(
                                at,
                                if guest {
                                    "usuário inválido"
                                } else {
                                    "usuário obrigatório"
                                },
                                value,
                            ));
                            continue;
                        }
                        if guest { json!(trimmed) } else { json!(text) }
                    }
                    Kind::Credential(size) => {
                        match base64_bytes(&text) {
                            Ok(bytes) if bytes.len() == size => {}
                            Ok(_) => {
                                errors.push(error(at, "tamanho inválido da credencial", value));
                                continue;
                            }
                            Err(e) => {
                                errors.push(error(at, &e, value));
                                continue;
                            }
                        }
                        json!(text)
                    }
                    _ => {
                        if name == "SetupBlob" {
                            match base64_bytes(&text) {
                                Ok(bytes)
                                    if (f.name == "iv" && bytes.len() == 12)
                                        || (f.name == "data" && bytes.len() >= 16) => {}
                                Ok(_) => {
                                    errors.push(error(
                                        at,
                                        "tamanho inválido do conteúdo cifrado",
                                        value,
                                    ));
                                    continue;
                                }
                                Err(e) => {
                                    errors.push(error(at, &e, value));
                                    continue;
                                }
                            }
                        }
                        json!(text)
                    }
                },
            }
        } else {
            match f.kind {
                Kind::Boolean(default) => match super::validation::boolean(body, f.name, default) {
                    Ok(v) => json!(v),
                    Err(e) => {
                        push_failure(errors, e, &at);
                        continue;
                    }
                },
                Kind::Integer => match integer(value) {
                    Some(v) => v,
                    None => {
                        let (kind, msg) = if value.is_string() {
                            (
                                "int_parsing",
                                "Input should be a valid integer, unable to parse string as an integer",
                            )
                        } else if value.is_number() {
                            (
                                "int_from_float",
                                "Input should be a valid integer, got a number with a fractional part",
                            )
                        } else {
                            ("int_type", "Input should be a valid integer")
                        };
                        errors.push(issue(at, kind, msg, value, None));
                        continue;
                    }
                },
                Kind::Dictionary if value.is_object() || value.is_null() => value.clone(),
                Kind::Dictionary => {
                    errors.push(issue(
                        at,
                        "dict_type",
                        "Input should be a valid dictionary",
                        value,
                        None,
                    ));
                    continue;
                }
                Kind::Blob => collect("SetupBlob", value, at, errors),
                _ => unreachable!(),
            }
        };
        output.insert(f.name.into(), normalized);
    }
    if extras {
        for (key, value) in object {
            if !fields.iter().any(|f| f.name == key) {
                let mut at = loc.clone();
                at.push(json!(key));
                errors.push(issue(
                    at,
                    "extra_forbidden",
                    "Extra inputs are not permitted",
                    value,
                    None,
                ));
            }
        }
    }
    if name == "SetupBody"
        && errors.len() == before
        && !object.is_empty()
        && ["user", "salt", "auth_hash"]
            .iter()
            .any(|k| output.get(*k).is_none_or(Value::is_null))
    {
        errors.push(error(loc, "informe usuário, salt e auth_hash juntos", body));
    }
    Value::Object(output)
}
pub fn validate(name: &str, body: &Value) -> Result<Value> {
    let mut errors = vec![];
    if body.is_null() {
        return Err(Failure {
            status: 422,
            body: json!({"detail":[issue(vec![json!("body")],"missing","Field required",body,None)]}),
        });
    }
    let normalized = collect(name, body, vec![json!("body")], &mut errors);
    if errors.is_empty() {
        Ok(normalized)
    } else {
        Err(Failure {
            status: 422,
            body: json!({"detail":errors}),
        })
    }
}
