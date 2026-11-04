//! Validação dos modelos HTTP existentes, inclusive a ordem e o conteúdo dos erros FastAPI.
use serde_json::{Value, json};

#[derive(Clone, Copy)]
pub enum Schema {
    Synthesis,
    Narration,
    Thought,
    Translation,
    Suggestion,
}

#[derive(Clone, Copy)]
enum Kind {
    Text(usize, usize),
    Boolean,
    Texts(usize, usize, usize),
    Language,
}

type Field = (&'static str, Kind, bool);

impl Schema {
    fn fields(self) -> &'static [Field] {
        use Kind::*;
        match self {
            Self::Synthesis => &[
                ("text", Text(1, 200_000), true),
                ("voice", Text(0, usize::MAX), false),
                ("provider", Text(0, usize::MAX), false),
                ("confirm", Boolean, false),
                ("instruction", Text(0, usize::MAX), false),
            ],
            Self::Narration => &[
                ("text", Text(1, 200_000), true),
                ("code_blocks", Texts(0, usize::MAX, usize::MAX), false),
                ("instruction", Text(1, 2000), true),
            ],
            Self::Thought => &[("textos", Texts(1, 20, 200_000), true)],
            Self::Translation => &[
                ("lang", Language, true),
                ("texts", Texts(0, 2000, 2000), true),
            ],
            Self::Suggestion => &[("sessions", Texts(0, usize::MAX, usize::MAX), true)],
        }
    }
}

fn error(kind: &str, location: Value, input: &Value, context: Option<Value>) -> Value {
    let message = match kind {
        "missing" => "Field required".to_owned(),
        "model_attributes_type" => {
            "Input should be a valid dictionary or object to extract fields from".into()
        }
        "string_type" => "Input should be a valid string".into(),
        "list_type" => "Input should be a valid list".into(),
        "bool_type" => "Input should be a valid boolean".into(),
        "bool_parsing" => "Input should be a valid boolean, unable to interpret input".into(),
        "extra_forbidden" => "Extra inputs are not permitted".into(),
        "string_too_short" => format!(
            "String should have at least {} character{}",
            context.as_ref().unwrap()["min_length"],
            if context.as_ref().unwrap()["min_length"] == 1 {
                ""
            } else {
                "s"
            }
        ),
        "string_too_long" => format!(
            "String should have at most {} characters",
            context.as_ref().unwrap()["max_length"]
        ),
        "too_short" => format!(
            "List should have at least {} item{} after validation, not {}",
            context.as_ref().unwrap()["min_length"],
            if context.as_ref().unwrap()["min_length"] == 1 {
                ""
            } else {
                "s"
            },
            context.as_ref().unwrap()["actual_length"]
        ),
        "too_long" => format!(
            "List should have at most {} items after validation, not {}",
            context.as_ref().unwrap()["max_length"],
            context.as_ref().unwrap()["actual_length"]
        ),
        "literal_error" => "Input should be 'pt' or 'en'".into(),
        _ => unreachable!("tipo de erro de validação conhecido"),
    };
    let mut result = json!({"type":kind,"loc":location,"msg":message,"input":input});
    if let Some(context) = context {
        result["ctx"] = context;
    }
    result
}

fn text(input: &Value, min: usize, max: usize, location: Value, errors: &mut Vec<Value>) {
    match input.as_str() {
        None => errors.push(error("string_type", location, input, None)),
        Some(value) => {
            let size = value.chars().count();
            if size < min {
                errors.push(error(
                    "string_too_short",
                    location,
                    input,
                    Some(json!({"min_length":min})),
                ));
            } else if size > max {
                errors.push(error(
                    "string_too_long",
                    location,
                    input,
                    Some(json!({"max_length":max})),
                ));
            }
        }
    }
}

fn boolean(input: &Value) -> Result<bool, &'static str> {
    if let Some(value) = input.as_bool() {
        return Ok(value);
    }
    if let Some(value) = input.as_str() {
        return crate::query::fastapi_bool(value).ok_or("bool_parsing");
    }
    match input.as_f64() {
        Some(0.) => Ok(false),
        Some(1.) => Ok(true),
        Some(n) if n.fract() == 0. => Err("bool_parsing"),
        _ => Err("bool_type"),
    }
}

pub fn validate(schema: Schema, mut value: Value) -> Result<Value, Value> {
    let Some(object) = value.as_object_mut() else {
        return Err(
            json!({"detail":[error(if value.is_null() { "missing" } else { "model_attributes_type" }, json!(["body"]), &value, None)]}),
        );
    };
    let original = Value::Object(object.clone());
    let mut errors = Vec::new();
    for (name, kind, required) in schema.fields() {
        let Some(input) = object.get(*name).cloned() else {
            if *required {
                errors.push(error("missing", json!(["body", name]), &original, None));
            }
            continue;
        };
        let location = json!(["body", name]);
        match kind {
            Kind::Text(min, max) => text(&input, *min, *max, location, &mut errors),
            Kind::Boolean => match boolean(&input) {
                Ok(v) => {
                    object.insert((*name).into(), json!(v));
                }
                Err(kind) => errors.push(error(kind, location, &input, None)),
            },
            Kind::Language => {
                if !matches!(input.as_str(), Some("pt" | "en")) {
                    errors.push(error(
                        "literal_error",
                        location,
                        &input,
                        Some(json!({"expected":"'pt' or 'en'"})),
                    ));
                }
            }
            Kind::Texts(min, max, item_max) => match input.as_array() {
                None => errors.push(error("list_type", location, &input, None)),
                Some(items) => {
                    let before = errors.len();
                    for (index, item) in items.iter().enumerate() {
                        text(
                            item,
                            0,
                            *item_max,
                            json!(["body", name, index]),
                            &mut errors,
                        );
                    }
                    // O Pydantic aplica o limite do container depois de validar seus itens.
                    if errors.len() == before && items.len() < *min {
                        errors.push(error("too_short", location, &input,
                            Some(json!({"field_type":"List","min_length":min,"actual_length":items.len()}))));
                    } else if items.len() > *max {
                        errors.truncate(before);
                        errors.push(error("too_long", location, &input,
                            Some(json!({"field_type":"List","max_length":max,"actual_length":items.len()}))));
                    }
                }
            },
        }
    }
    for (name, input) in object.iter() {
        if !schema
            .fields()
            .iter()
            .any(|(field, ..)| *field == name.as_str())
        {
            errors.push(error("extra_forbidden", json!(["body", name]), input, None));
        }
    }
    if errors.is_empty() {
        Ok(value)
    } else {
        Err(json!({"detail":errors}))
    }
}
