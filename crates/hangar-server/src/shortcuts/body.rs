//! Modelos originais dos terminais; os campos ignorados nunca são copiados recursivamente.
use crate::http_cors::{internal_error, json_guarded};
use crate::orchestration::codec;
use axum::{
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy)]
pub enum Schema {
    Shell,
    RunCode,
    Answer,
}

fn issue(
    kind: &str,
    field: Option<&str>,
    message: &str,
    input: &Value,
    ctx: Option<Value>,
) -> Value {
    let mut value = json!({"type":kind,"loc":["body"],"msg":message});
    if let Some(field) = field {
        value["loc"].as_array_mut().unwrap().push(json!(field));
    }
    value["input"] = codec::clone_value(input);
    if let Some(ctx) = ctx {
        value["ctx"] = ctx;
    }
    value
}

pub fn parse(schema: Schema, bytes: &[u8], headers: &HeaderMap) -> Result<Value, Response> {
    let value = if bytes.is_empty() {
        Value::Null
    } else if crate::http_body_input::is_json(headers) {
        crate::orchestration::body::parse(bytes)
            .map_err(|(status, value)| json_guarded(status, value))?
    } else {
        Value::String(
            std::str::from_utf8(bytes)
                .map_err(|_| internal_error())?
                .into(),
        )
    };
    let Some(object) = value.as_object() else {
        let error = issue(
            if value.is_null() {
                "missing"
            } else {
                "model_attributes_type"
            },
            None,
            if value.is_null() {
                "Field required"
            } else {
                "Input should be a valid dictionary or object to extract fields from"
            },
            &value,
            None,
        );
        codec::discard(value);
        return Err(json_guarded(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"detail":[error]}),
        ));
    };
    let fields: &[(&str, usize, bool, bool)] = match schema {
        Schema::Shell => &[
            ("command", 200000, true, false),
            ("label", 200, false, false),
            ("pasta", 4096, false, true),
            ("runs_in", 0, false, false),
            ("key", 200, false, false),
            ("home", 0, false, false),
            ("ask", 0, false, false),
        ],
        Schema::RunCode => &[
            ("command", 200000, true, false),
            ("language", 32, false, true),
            ("key", 80, false, false),
        ],
        Schema::Answer => &[("text", 4096, false, false)],
    };
    let mut result = Map::new();
    let mut errors = Vec::new();
    for &(field, max, required, nullable) in fields {
        let Some(input) = object.get(field) else {
            if required {
                errors.push(issue(
                    "missing",
                    Some(field),
                    "Field required",
                    &value,
                    None,
                ));
            } else {
                result.insert(
                    field.into(),
                    match field {
                        "home" | "ask" => json!(true),
                        "runs_in" => json!("session"),
                        _ if nullable => Value::Null,
                        _ => json!(""),
                    },
                );
            }
            continue;
        };
        if matches!(field, "home" | "ask") {
            if let Some(boolean) = crate::value_coercion::boolean(input) {
                result.insert(field.into(), json!(boolean));
            } else {
                let parseable = input.is_string()
                    || crate::value_coercion::integer(input).is_some_and(|v| v.as_i64().is_some());
                errors.push(issue(
                    if parseable {
                        "bool_parsing"
                    } else {
                        "bool_type"
                    },
                    Some(field),
                    if parseable {
                        "Input should be a valid boolean, unable to interpret input"
                    } else {
                        "Input should be a valid boolean"
                    },
                    input,
                    None,
                ));
            }
        } else if field == "runs_in" {
            if matches!(input.as_str(), Some("session" | "hangar")) {
                result.insert(field.into(), codec::clone_value(input));
            } else {
                errors.push(issue(
                    "literal_error",
                    Some(field),
                    "Input should be 'session' or 'hangar'",
                    input,
                    Some(json!({"expected":"'session' or 'hangar'"})),
                ));
            }
        } else if input.is_null() && nullable {
            result.insert(field.into(), Value::Null);
        } else if let Some(text) = input.as_str() {
            if codec::contains_surrogate(input) {
                errors.push(issue(
                    "string_unicode",
                    Some(field),
                    "Input should be a valid string, unable to parse raw data as a unicode string",
                    input,
                    None,
                ));
            } else if text.chars().count() > max {
                errors.push(issue(
                    "string_too_long",
                    Some(field),
                    &format!("String should have at most {max} characters"),
                    input,
                    Some(json!({"max_length":max})),
                ));
            } else {
                result.insert(field.into(), json!(text));
            }
        } else {
            errors.push(issue(
                "string_type",
                Some(field),
                "Input should be a valid string",
                input,
                None,
            ));
        }
    }
    codec::discard(value);
    if errors.is_empty() {
        Ok(Value::Object(result))
    } else {
        codec::discard(Value::Object(result));
        Err(json_guarded(
            StatusCode::UNPROCESSABLE_ENTITY,
            json!({"detail":errors}),
        ))
    }
}
