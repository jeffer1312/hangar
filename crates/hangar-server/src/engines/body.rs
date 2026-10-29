//! Validação da borda tipada original; erros mantêm ordem, localização e entrada recebida.
use crate::orchestration::{body, codec};
use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Map, Value, json};

#[derive(Clone, Copy)]
pub enum Schema {
    Model,
    Probe,
    Management,
    Sync,
    Alias,
}
#[derive(Clone, Copy)]
struct Field {
    name: &'static str,
    required: bool,
    nullable: bool,
    min: usize,
    max: Option<usize>,
    list: bool,
}
impl Field {
    const fn text(
        name: &'static str,
        required: bool,
        nullable: bool,
        min: usize,
        max: Option<usize>,
    ) -> Self {
        Self {
            name,
            required,
            nullable,
            min,
            max,
            list: false,
        }
    }
}
impl Schema {
    fn fields(self) -> Vec<Field> {
        match self {
            Self::Model => vec![
                Field::text("model", true, false, 0, None),
                Field::text("effort", false, true, 0, None),
            ],
            Self::Probe => ["nome", "base_url", "api_key"]
                .into_iter()
                .map(|name| Field::text(name, false, true, 0, None))
                .collect(),
            Self::Management => vec![Field::text("management_key", false, false, 0, Some(512))],
            Self::Sync => vec![
                Field::text("id", true, false, 1, Some(512)),
                Field {
                    name: "alvos",
                    required: false,
                    nullable: false,
                    min: 0,
                    max: None,
                    list: true,
                },
            ],
            Self::Alias => vec![
                Field::text("id", true, false, 1, Some(512)),
                Field::text("apelido", false, false, 0, Some(40)),
            ],
        }
    }
    fn strict(self) -> bool {
        matches!(self, Self::Model | Self::Probe | Self::Management)
    }
}

pub fn internal_error() -> Response {
    let mut response = (StatusCode::INTERNAL_SERVER_ERROR, "Internal Server Error").into_response();
    response
        .extensions_mut()
        .insert(crate::http_cors::UnhandledServerError);
    response
}
pub fn json_error(status: StatusCode, value: Value) -> Response {
    if codec::contains_surrogate(&value) || codec::contains_nonfinite(&value) {
        codec::discard(value);
        internal_error()
    } else {
        let text = crate::transcript::pyjson::dumps_compact_unicode(&value);
        codec::discard(value);
        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            text,
        )
            .into_response()
    }
}
fn detail(errors: Vec<Value>) -> Value {
    let mut object = Map::new();
    object.insert("detail".into(), Value::Array(errors));
    Value::Object(object)
}
fn issue(kind: &str, location: Vec<Value>, input: &Value, length: Option<usize>) -> Value {
    let message = match kind {
        "missing" => "Field required".to_owned(),
        "model_attributes_type" => {
            "Input should be a valid dictionary or object to extract fields from".to_owned()
        }
        "string_type" => "Input should be a valid string".to_owned(),
        "string_unicode" => {
            "Input should be a valid string, unable to parse raw data as a unicode string"
                .to_owned()
        }
        "list_type" => "Input should be a valid list".to_owned(),
        "extra_forbidden" => "Extra inputs are not permitted".to_owned(),
        "string_too_short" => format!(
            "String should have at least {} character{}",
            length.unwrap(),
            if length == Some(1) { "" } else { "s" }
        ),
        "string_too_long" => format!(
            "String should have at most {} character{}",
            length.unwrap(),
            if length == Some(1) { "" } else { "s" }
        ),
        _ => unreachable!(),
    };
    let mut result = json!({"type":kind,"loc":location,"msg":message});
    result
        .as_object_mut()
        .unwrap()
        .insert("input".into(), codec::clone_value(input));
    if let Some(length) = length {
        result["ctx"] = if kind == "string_too_short" {
            json!({"min_length":length})
        } else {
            json!({"max_length":length})
        };
    }
    result
}
fn text_error(field: Field, value: &Value, location: Vec<Value>) -> Option<Value> {
    if value.is_null() && field.nullable {
        return None;
    }
    let Some(text) = value.as_str().filter(|_| !codec::contains_nonfinite(value)) else {
        return Some(issue("string_type", location, value, None));
    };
    if (field.min > 0 || field.max.is_some()) && codec::contains_surrogate(value) {
        return Some(issue("string_unicode", location, value, None));
    }
    let length = text.chars().count();
    if length < field.min {
        Some(issue("string_too_short", location, value, Some(field.min)))
    } else if field.max.is_some_and(|max| length > max) {
        Some(issue("string_too_long", location, value, field.max))
    } else {
        None
    }
}

pub fn typed(schema: Schema, bytes: &[u8], headers: &HeaderMap) -> Result<Value, Response> {
    let value = if bytes.is_empty() {
        Value::Null
    } else if crate::http_body_input::is_json(headers) {
        body::parse(bytes).map_err(|(status, error)| json_error(status, error))?
    } else {
        Value::String(
            std::str::from_utf8(bytes)
                .map_err(|_| internal_error())?
                .to_owned(),
        )
    };
    let Some(object) = value.as_object() else {
        let error = issue(
            if value.is_null() {
                "missing"
            } else {
                "model_attributes_type"
            },
            vec![json!("body")],
            &value,
            None,
        );
        codec::discard(value);
        return Err(json_error(
            StatusCode::UNPROCESSABLE_ENTITY,
            detail(vec![error]),
        ));
    };
    let fields = schema.fields();
    let mut errors = Vec::new();
    let mut result = Map::new();
    for field in &fields {
        let location = vec![json!("body"), json!(field.name)];
        match object.get(field.name) {
            None if field.required => errors.push(issue("missing", location, &value, None)),
            None => {
                result.insert(
                    field.name.into(),
                    if field.list {
                        json!(["pi", "kimi", "codex"])
                    } else if field.nullable {
                        Value::Null
                    } else {
                        json!("")
                    },
                );
            }
            Some(value) if field.list => {
                let previous_errors = errors.len();
                if let Some(items) = value.as_array() {
                    for (index, item) in items.iter().enumerate() {
                        if let Some(error) = text_error(
                            Field::text(field.name, true, false, 0, None),
                            item,
                            vec![json!("body"), json!(field.name), json!(index)],
                        ) {
                            errors.push(error);
                        }
                    }
                } else {
                    errors.push(issue("list_type", location, value, None));
                }
                if previous_errors == errors.len() {
                    result.insert(field.name.into(), codec::clone_value(value));
                }
            }
            Some(value) => {
                if let Some(error) = text_error(*field, value, location) {
                    errors.push(error);
                } else {
                    result.insert(field.name.into(), codec::clone_value(value));
                }
            }
        }
    }
    if schema.strict() {
        for (field, value) in object {
            if !fields.iter().any(|known| known.name == field) {
                errors.push(issue(
                    "extra_forbidden",
                    vec![json!("body"), json!(field)],
                    value,
                    None,
                ));
            }
        }
    }
    codec::discard(value);
    if errors.is_empty() {
        Ok(Value::Object(result))
    } else {
        codec::discard(Value::Object(result));
        Err(json_error(StatusCode::UNPROCESSABLE_ENTITY, detail(errors)))
    }
}

/// Request.json() não usa o modelo tipado nem a seleção por Content-Type.
pub fn raw(bytes: &[u8]) -> Result<Value, Response> {
    if bytes.is_empty() {
        return Err(internal_error());
    }
    body::parse(bytes).map_err(|(_, error)| {
        codec::discard(error);
        internal_error()
    })
}
