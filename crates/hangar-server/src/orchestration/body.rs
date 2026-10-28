//! A borda conserva a validação dos modelos HTTP usados pelos clientes existentes.
use serde::{Deserialize, Deserializer, de::Error};
use serde_json::{Value, json};

#[derive(Clone, Copy)]
enum Kind {
    Text,
    NullableText,
    Bool,
    NullableBool,
    Number,
    Texts,
    Roles,
}
type Field = (&'static str, Kind, bool);
fn fields(name: &str) -> Vec<Field> {
    use Kind::*;
    match name.rsplit("::").next().unwrap_or(name) {
        "PolicyBody" => vec![
            ("provider", Text, true),
            ("apelido", Text, false),
            ("modelos", Texts, false),
            ("trocar", Bool, false),
            ("ligada", Bool, false),
            ("mtime", Number, true),
        ],
        "RoleBody" | "OneRoleBody" => {
            let item = name.ends_with("::RoleBody");
            let mut out = vec![
                ("papel", Text, true),
                ("sessao", Text, false),
                ("provider", Text, true),
                ("conta", Text, true),
                ("modelo", Text, false),
                ("esforco", Text, false),
            ];
            if item {
                out.push(("vez", Text, false));
            }
            out.extend([
                ("headless", NullableBool, false),
                ("permissao", Text, false),
                ("motor", Text, false),
                ("jev", Bool, false),
                ("subagente", Text, false),
                ("perfil", Text, false),
            ]);
            out.push(if item {
                ("janela", NullableText, false)
            } else {
                ("mtime", Number, true)
            });
            out
        }
        "RolesBody" => vec![
            ("papeis", Roles, true),
            ("mtime", Number, true),
            ("avisar", Bool, false),
        ],
        "RemoveBody" => vec![
            ("papel", Text, true),
            ("vez", Text, false),
            ("mtime", Number, true),
        ],
        "GroupBody" => vec![("gid", Text, true), ("mtime", Number, true)],
        "StartBody" => vec![("mtime", Number, false)],
        _ => unreachable!("modelo HTTP de orquestração conhecido"),
    }
}
fn error(kind: &str, loc: &[Value], input: &Value) -> Value {
    let message = match kind {
        "missing" => "Field required",
        "model_attributes_type" => {
            "Input should be a valid dictionary or object to extract fields from"
        }
        "extra_forbidden" => "Extra inputs are not permitted",
        "string_type" => "Input should be a valid string",
        "bool_type" => "Input should be a valid boolean",
        "bool_parsing" => "Input should be a valid boolean, unable to interpret input",
        "float_type" => "Input should be a valid number",
        "float_parsing" => "Input should be a valid number, unable to parse string as a number",
        "list_type" => "Input should be a valid list",
        _ => unreachable!("erro de validação conhecido"),
    };
    json!({"type":kind,"loc":loc,"msg":message,"input":input})
}
fn boolean(value: &Value) -> Result<bool, &'static str> {
    if let Some(value) = value.as_bool() {
        return Ok(value);
    }
    if let Some(value) = value.as_f64() {
        return match value {
            0. => Ok(false),
            1. => Ok(true),
            _ => Err("bool_parsing"),
        };
    }
    if let Some(value) = value.as_str() {
        return match value.to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => Ok(true),
            _ => Err("bool_parsing"),
        };
    }
    Err("bool_type")
}
fn number(value: &Value) -> Result<f64, &'static str> {
    if let Some(value) = value.as_f64() {
        return Ok(value);
    }
    if let Some(value) = value.as_bool() {
        return Ok(if value { 1. } else { 0. });
    }
    if let Some(value) = value.as_str() {
        let value = value.trim();
        let chars: Vec<_> = value.chars().collect();
        if chars.iter().enumerate().any(|(i, c)| {
            *c == '_'
                && (i == 0
                    || i + 1 == chars.len()
                    || !chars[i - 1].is_ascii_digit()
                    || !chars[i + 1].is_ascii_digit())
        }) {
            return Err("float_parsing");
        }
        return value.replace('_', "").parse().map_err(|_| "float_parsing");
    }
    Err("float_type")
}
pub fn deserialize_number<'de, D: Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    number(&Value::deserialize(d)?).map_err(D::Error::custom)
}
fn model(value: &Value, schema: &[Field], loc: &[Value], errors: &mut Vec<Value>) -> Value {
    let Some(input) = value.as_object() else {
        errors.push(error(
            if value.is_null() {
                "missing"
            } else {
                "model_attributes_type"
            },
            loc,
            value,
        ));
        return value.clone();
    };
    let mut output = input.clone();
    for (name, kind, required) in schema {
        let mut at = loc.to_vec();
        at.push(json!(name));
        let Some(v) = input.get(*name) else {
            if *required {
                errors.push(error("missing", &at, value));
            }
            continue;
        };
        let failure = match kind {
            Kind::Text | Kind::NullableText => {
                if v.is_string() || (matches!(kind, Kind::NullableText) && v.is_null()) {
                    None
                } else {
                    Some("string_type")
                }
            }
            Kind::Bool | Kind::NullableBool => {
                if matches!(kind, Kind::NullableBool) && v.is_null() {
                    None
                } else {
                    match boolean(v) {
                        Ok(b) => {
                            output.insert((*name).into(), json!(b));
                            None
                        }
                        Err(e) => Some(e),
                    }
                }
            }
            Kind::Number => match number(v) {
                Ok(n) => {
                    if n.is_finite() {
                        output.insert((*name).into(), json!(n));
                    }
                    None
                }
                Err(e) => Some(e),
            },
            Kind::Texts | Kind::Roles => {
                if let Some(values) = v.as_array() {
                    let converted = values
                        .iter()
                        .enumerate()
                        .map(|(index, item)| {
                            let mut at = at.clone();
                            at.push(json!(index));
                            if matches!(kind, Kind::Roles) {
                                model(item, &fields("::RoleBody"), &at, errors)
                            } else {
                                if !item.is_string() {
                                    errors.push(error("string_type", &at, item));
                                }
                                item.clone()
                            }
                        })
                        .collect();
                    output.insert((*name).into(), Value::Array(converted));
                    None
                } else {
                    Some("list_type")
                }
            }
        };
        if let Some(kind) = failure {
            errors.push(error(kind, &at, v));
        }
    }
    for (key, value) in input {
        if !schema.iter().any(|(name, _, _)| name == key) {
            let mut at = loc.to_vec();
            at.push(json!(key));
            errors.push(error("extra_forbidden", &at, value));
        }
    }
    Value::Object(output)
}
pub fn validate(name: &str, value: &Value) -> Result<Value, Value> {
    let mut errors = Vec::new();
    let output = model(value, &fields(name), &[json!("body")], &mut errors);
    if errors.is_empty() {
        Ok(output)
    } else {
        Err(json!({"detail":errors}))
    }
}
pub fn parse(bytes: &[u8]) -> Result<Value, (axum::http::StatusCode, Value)> {
    use axum::http::StatusCode;
    if bytes.is_empty() {
        return Ok(Value::Null);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            json!({"detail":"There was an error parsing the body"}),
        )
    })?;
    if let Some(value) = super::codec::loads(text) { return Ok(value); }
    serde_json::from_str(text).map_err(|e|{
        let raw=e.to_string();let eof=e.is_eof();
        let previous:usize=text.split_inclusive('\n').take(e.line().saturating_sub(1)).map(str::len).sum();
        let byte=(previous+e.column().saturating_sub(usize::from(!eof))).min(text.len());
        let position=text[..text.floor_char_boundary(byte)].chars().count();
        let last=text.trim_end().chars().next_back();
        let message=if raw.starts_with("trailing characters"){"Extra data"}
            else if raw.starts_with("key must be a string")||(eof&&matches!(last,Some('{'))){"Expecting property name enclosed in double quotes"}
            else if raw.starts_with("expected `:`"){"Expecting ':' delimiter"}
            else if raw.starts_with("expected `,`")||(eof&&!matches!(last,Some('['|':'|','))){"Expecting ',' delimiter"}
            else {"Expecting value"};
        (StatusCode::UNPROCESSABLE_ENTITY,json!({"detail":[{"type":"json_invalid","loc":["body",position],"msg":"JSON decode error","input":{},"ctx":{"error":message}}]}))
    })
}
