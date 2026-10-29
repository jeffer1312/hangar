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
    let mut error = json!({"type":kind,"loc":["body",field],"msg":message,"input":input});
    if let Some(ctx) = ctx {
        error["ctx"] = ctx;
    }
    Failure {
        status: 422,
        body: json!({"detail":[error]}),
    }
}
pub fn text(body: &Value, key: &str, min: usize, max: usize) -> Result<String> {
    let value = body
        .get(key)
        .ok_or_else(|| invalid(key, "missing", "Field required", body.clone(), None))?;
    let s = value.as_str().ok_or_else(|| {
        invalid(
            key,
            "string_type",
            "Input should be a valid string",
            value.clone(),
            None,
        )
    })?;
    if s.chars().count() < min {
        return Err(invalid(
            key,
            "string_too_short",
            &format!(
                "String should have at least {min} character{}",
                if min == 1 { "" } else { "s" }
            ),
            value.clone(),
            Some(json!({"min_length":min})),
        ));
    }
    if s.chars().count() > max {
        return Err(invalid(
            key,
            "string_too_long",
            &format!("String should have at most {max} characters"),
            value.clone(),
            Some(json!({"max_length":max})),
        ));
    }
    Ok(s.into())
}
pub fn boolean(body: &Value, key: &str, default: bool) -> Result<bool> {
    let Some(v) = body.get(key) else {
        return Ok(default);
    };
    if let Some(b) = v.as_bool() {
        return Ok(b);
    }
    if v == 0 || v == 1 {
        return Ok(v == 1);
    }
    if let Some(s) = v.as_str() {
        match s.to_ascii_lowercase().as_str() {
            "0" | "off" | "f" | "false" | "n" | "no" => return Ok(false),
            "1" | "on" | "t" | "true" | "y" | "yes" => return Ok(true),
            _ => {}
        }
    }
    Err(invalid(
        key,
        if v.is_string() || v.is_number() {
            "bool_parsing"
        } else {
            "bool_type"
        },
        if v.is_string() || v.is_number() {
            "Input should be a valid boolean, unable to interpret input"
        } else {
            "Input should be a valid boolean"
        },
        v.clone(),
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
                    v.clone(),
                    None,
                ));
            }
        }
    }
    Ok(())
}
