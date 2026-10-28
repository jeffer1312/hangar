//! Validação da revisão do ditado, preservando os erros da borda FastAPI.
use serde_json::{Value, json};
fn error(kind: &str, loc: Value, input: &Value) -> Value {
    let message = match kind {
        "missing" => "Field required",
        "model_attributes_type" => {
            "Input should be a valid dictionary or object to extract fields from"
        }
        "string_type" => "Input should be a valid string",
        "string_too_short" => "String should have at least 1 character",
        "extra_forbidden" => "Extra inputs are not permitted",
        "bool_type" => "Input should be a valid boolean",
        "bool_parsing" => "Input should be a valid boolean, unable to interpret input",
        "list_type" => "Input should be a valid list",
        "dict_type" => "Input should be a valid dictionary",
        _ => unreachable!(),
    };
    let mut value = json!({"type":kind,"loc":loc,"msg":message,"input":input});
    if kind == "string_too_short" {
        value["ctx"] = json!({"min_length":1});
    }
    value
}
pub fn validate(mut value: Value) -> Result<Value, Value> {
    let Some(object) = value.as_object_mut() else {
        return Err(
            json!({"detail":[error(if value.is_null(){"missing"}else{"model_attributes_type"},json!(["body"]),&value)]}),
        );
    };
    let original = Value::Object(object.clone());
    let mut errors = Vec::new();
    const FIELDS: &[&str] = &[
        "texto",
        "estilo",
        "session",
        "generation",
        "organization_mode",
        "organization_model",
        "organization_account",
        "include_recent_messages",
        "recent_messages",
    ];
    for field in FIELDS {
        let required = matches!(*field, "texto" | "estilo");
        let Some(input) = object.get(*field).cloned() else {
            if required {
                errors.push(error("missing", json!(["body", field]), &original));
            }
            continue;
        };
        if input.is_null() && !required {
            continue;
        }
        let kind = if *field == "include_recent_messages" {
            let boolean = input
                .as_bool()
                .or_else(|| input.as_str().and_then(crate::query::fastapi_bool))
                .or_else(|| match input.as_f64() {
                    Some(0.) => Some(false),
                    Some(1.) => Some(true),
                    _ => None,
                });
            if let Some(boolean) = boolean {
                object.insert((*field).into(), json!(boolean));
                None
            } else {
                Some(if input.is_string() || input.is_number() {
                    "bool_parsing"
                } else {
                    "bool_type"
                })
            }
        } else if *field == "recent_messages" {
            if let Some(rows) = input.as_array() {
                for (i, row) in rows.iter().enumerate() {
                    if !row.is_object() {
                        errors.push(error("dict_type", json!(["body", field, i]), row));
                    }
                }
                None
            } else {
                Some("list_type")
            }
        } else if !input.is_string() {
            Some("string_type")
        } else if *field == "texto" && input.as_str() == Some("") {
            Some("string_too_short")
        } else {
            None
        };
        if let Some(kind) = kind {
            errors.push(error(kind, json!(["body", field]), &input));
        }
    }
    for (field, input) in object.iter() {
        if !FIELDS.contains(&field.as_str()) {
            errors.push(error("extra_forbidden", json!(["body", field]), input));
        }
    }
    if errors.is_empty() {
        Ok(value)
    } else {
        Err(json!({"detail":errors}))
    }
}
