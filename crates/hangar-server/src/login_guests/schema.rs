//! Validação dos formulários na ordem dos campos publicada pelo FastAPI.
use super::store::{Failure, Result};
use base64::Engine;
use serde_json::{Value, json};
use crate::value_coercion::integer;

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
    let mut value = json!({"type":kind,"loc":loc,"msg":message});
    value["input"]=crate::orchestration::codec::clone_value(input);
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
fn push_failure(errors: &mut Vec<Value>, mut failure: Failure, loc: &[Value]) {
    let detail=std::mem::take(&mut failure.body["detail"]);
    for mut e in match detail {Value::Array(values)=>values,_=>Vec::new()}
    {
        let mut at = loc.to_vec();
        at.extend(e["loc"].as_array().unwrap().iter().skip(2).cloned());
        e["loc"] = json!(at);
        errors.push(e);
    }
}
pub(super) fn base64_bytes(text: &str) -> std::result::Result<Vec<u8>, String> {
    if !text.is_ascii() { return Err("string argument should contain only ASCII characters".into()); }
    let (mut position, mut padding, mut count) = (0usize, 0usize, 0usize);
    for (index, byte) in text.bytes().enumerate() {
        if byte == b'=' {
            if index == 0 { return Err("Leading padding not allowed".into()); }
            if position == 0 { return Err("Excess padding not allowed".into()); }
            padding += 1;
            if position >= 2 && position + padding >= 4 {
                if index + 1 != text.len() { return Err("Excess data after padding".into()); }
                position = 0;
                break;
            }
        } else if byte.is_ascii_alphanumeric() || b"+/".contains(&byte) {
            if padding > 0 { return Err("Discontinuous padding not allowed".into()); }
            count += 1;
            position = (position + 1) % 4;
        } else { return Err("Only base64 data is allowed".into()); }
    }
    if position == 1 {
        return Err(format!("Invalid base64-encoded string: number of data characters ({count}) cannot be 1 more than a multiple of 4"));
    }
    if position != 0 { return Err("Incorrect padding".into()); }
    let engine = base64::engine::GeneralPurpose::new(&base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true));
    engine.decode(text).map_err(|_| "Incorrect padding".into())
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
                        let size_error=value.as_str().is_some_and(|text|text.trim().replace('_', "").len()>4300)
                            || value.is_number() && value.as_f64().is_some_and(|number|number.is_finite() && number.fract()==0.0 && (number<=i64::MIN as f64 || number>=-(i64::MIN as f64)));
                        let (kind, msg) = if size_error {
                            ("int_parsing_size","Unable to parse input string as an integer, exceeded maximum size")
                        } else if value.is_string() {
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
                Kind::Dictionary if value.is_object() || value.is_null() => crate::orchestration::codec::clone_value(value),
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
            os_error: None,
            status: 422,
            body: json!({"detail":[issue(vec![json!("body")],"missing","Field required",body,None)]}),
        });
    }
    let normalized = collect(name, body, vec![json!("body")], &mut errors);
    if errors.is_empty() {
        Ok(normalized)
    } else {
        crate::orchestration::codec::discard(normalized);
        Err(Failure {
            os_error: None,
            status: 422,
            body: super::object([("detail", Value::Array(errors))]),
        })
    }
}
