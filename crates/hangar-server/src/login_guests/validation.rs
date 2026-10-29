//! Erros de formulário mantêm o formato publicado pelo Pydantic.
use super::store::{Failure, Result};
use serde_json::{Value, json};

pub fn invalid(
    field: &str,
    kind: &str,
    message: &str,
    input: Value,
    ctx: Option<Value>,
) -> Failure {
    let mut error = json!({"type":kind,"loc":["body",field],"msg":message});
    error["input"]=input;
    if let Some(ctx) = ctx {
        error["ctx"] = ctx;
    }
    Failure {
        os_error: None,
        status: 422,
        body: super::object([("detail",Value::Array(vec![error]))]),
    }
}
pub fn text(body: &Value, key: &str, min: usize, max: usize) -> Result<String> {
    let value = body
        .get(key)
        .ok_or_else(|| invalid(key, "missing", "Field required", crate::orchestration::codec::clone_value(body), None))?;
    let s = value.as_str().ok_or_else(|| {
        invalid(
            key,
            "string_type",
            "Input should be a valid string",
            crate::orchestration::codec::clone_value(value),
            None,
        )
    })?;
    if crate::orchestration::codec::contains_surrogate(value) {
        return Err(invalid(key, "string_unicode", "Input should be a valid string, unable to parse raw data as a unicode string", crate::orchestration::codec::clone_value(value), None));
    }
    if s.chars().count() < min {
        return Err(invalid(
            key,
            "string_too_short",
            &format!(
                "String should have at least {min} character{}",
                if min == 1 { "" } else { "s" }
            ),
            crate::orchestration::codec::clone_value(value),
            Some(json!({"min_length":min})),
        ));
    }
    if s.chars().count() > max {
        return Err(invalid(
            key,
            "string_too_long",
            &format!("String should have at most {max} characters"),
            crate::orchestration::codec::clone_value(value),
            Some(json!({"max_length":max})),
        ));
    }
    Ok(s.into())
}
pub fn boolean(body: &Value, key: &str, default: bool) -> Result<bool> {
    let Some(v) = body.get(key) else {
        return Ok(default);
    };
    if let Some(value)=crate::value_coercion::boolean(v) {
        return Ok(value);
    }
    let parseable=v.is_string() || crate::value_coercion::integer(v).is_some_and(|value|value.as_i64().is_some());
    Err(invalid(
        key,
        if parseable {
            "bool_parsing"
        } else {
            "bool_type"
        },
        if parseable {
            "Input should be a valid boolean, unable to interpret input"
        } else {
            "Input should be a valid boolean"
        },
        crate::orchestration::codec::clone_value(v),
        None,
    ))
}
pub fn value_error(key: &str, input: Value, message: &str) -> Failure {
    invalid(
        key,
        "value_error",
        &format!("Value error, {message}"),
        input,
        Some(json!({"error":{}})),
    )
}
pub fn guest_settings(body: &Value) -> Result<(String, bool, bool)> {
    Ok((
        text(body, "root", 1, 4096)?,
        boolean(body, "sees_owner", false)?,
        boolean(body, "owner_sees", true)?,
    ))
}
pub fn forbid_extra(body: &Value, keys: &[&str]) -> Result<()> {
    if let Some(obj) = body.as_object() {
        for (k, v) in obj {
            if !keys.contains(&k.as_str()) {
                return Err(invalid(
                    k,
                    "extra_forbidden",
                    "Extra inputs are not permitted",
                    crate::orchestration::codec::clone_value(v),
                    None,
                ));
            }
        }
    }
    Ok(())
}
