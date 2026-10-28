//! Tradução das mensagens do espectador para o protocolo do navegador.
use super::{cdp::Cdp, ws::Failure};
use serde_json::{Value, json};

pub(super) fn truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().is_some_and(|n| n != 0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
pub(super) fn string(value: &Value) -> String {
    match value {
        Value::String(v) => v.clone(),
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        _ => value.to_string(),
    }
}

pub(super) fn normalize_numeric_separators(raw: &str) -> Option<String> {
    let mut normalized = String::with_capacity(raw.len());
    let mut characters = raw.chars().peekable();
    let mut previous_digit = false;
    while let Some(character) = characters.next() {
        if character == '_' {
            if !previous_digit || !characters.peek().is_some_and(char::is_ascii_digit) {
                return None;
            }
        } else {
            normalized.push(character);
        }
        previous_digit = character.is_ascii_digit();
    }
    Some(normalized)
}

pub(super) fn integer(value: &Value) -> Result<i64, Failure> {
    match value {
        Value::Bool(v) => Ok(i64::from(*v)),
        Value::Number(n) => n
            .as_i64()
            .or_else(|| {
                n.as_f64()
                    .filter(|v| v.is_finite() && *v >= i64::MIN as f64 && *v < i64::MAX as f64)
                    .map(|v| v.trunc() as i64)
            })
            .ok_or_else(|| Failure::value("int() value out of range".into())),
        Value::String(v) => normalize_numeric_separators(v)
            .and_then(|normalized| normalized.trim().parse().ok())
            .ok_or_else(|| {
                Failure::value(format!(
                    "invalid literal for int() with base 10: '{}'",
                    v.replace('\\', "\\\\").replace('\'', "\\'")
                ))
            }),
        _ => Err(Failure::value(
            "int() argument must be a string, a bytes-like object or a real number".into(),
        )),
    }
}
fn number(value: &Value) -> Result<f64, Failure> {
    match value {
        Value::Bool(v) => Ok(if *v { 1.0 } else { 0.0 }),
        Value::Number(n) => n
            .as_f64()
            .ok_or_else(|| Failure::value("float value out of range".into())),
        Value::String(v) => normalize_numeric_separators(v)
            .and_then(|normalized| normalized.trim().parse().ok())
            .ok_or_else(|| Failure::value(format!("could not convert string to float: '{v}'"))),
        _ => Err(Failure::value(
            "float() argument must be a string or a real number".into(),
        )),
    }
}
fn default<'a>(value: &'a Value, zero: &'a Value) -> &'a Value {
    if truth(value) { value } else { zero }
}
fn px(value: &Value, size: i64) -> i64 {
    let Ok(n) = number(value) else { return 0 };
    let clamped = if n.is_nan() { 1.0 } else { n.clamp(0.0, 1.0) };
    (clamped * size as f64) as i64
}
pub(super) fn apply(cdp: &Cdp, message: &Value, width: i64, height: i64) -> Result<(), Failure> {
    let zero = json!(0);
    let modifiers = || integer(default(&message["mod"], &zero));
    match message["t"].as_str() {
        Some("m") => {
            let action = match message["acao"].as_str() {
                Some("press") => "mousePressed",
                Some("release") => "mouseReleased",
                Some("move") => "mouseMoved",
                _ => return Ok(()),
            };
            let button = match message["b"].as_str() {
                Some("right") => "right",
                Some("middle") => "middle",
                Some("none") => "none",
                _ => "left",
            };
            cdp.send("Input.dispatchMouseEvent", json!({
                "type": action,
                "x": px(&message["x"], width), "y": px(&message["y"], height),
                "button": button,
                "clickCount": if action == "mouseMoved" { 0 } else { 1 },
                "modifiers": modifiers()?,
            }));
        }
        Some("w") => cdp.send("Input.dispatchMouseEvent", json!({
            "type": "mouseWheel",
            "x": px(&message["x"], width), "y": px(&message["y"], height),
            "deltaX": number(default(&message["dx"], &zero))?,
            "deltaY": number(default(&message["dy"], &zero))?,
            "button": "none", "clickCount": 0, "modifiers": 0,
        })),
        Some("txt") => {
            let text = if truth(&message["s"]) {
                string(&message["s"]).chars().take(4096).collect::<String>()
            } else {
                String::new()
            };
            if !text.is_empty() {
                cdp.send("Input.insertText", json!({"text": text}));
            }
        }
        Some("k") => {
            let key = if truth(&message["key"]) {
                string(&message["key"]).chars().take(32).collect::<String>()
            } else {
                String::new()
            };
            if key.is_empty() {
                return Ok(());
            }
            let mut event = json!({
                "key": key, "modifiers": modifiers()?,
                "windowsVirtualKeyCode": integer(default(&message["code"], &zero))?,
            });
            let vk = match key.as_str() {
                "Enter" => Some(13),
                "Tab" => Some(9),
                "Escape" => Some(27),
                "Backspace" => Some(8),
                "Delete" => Some(46),
                "ArrowUp" => Some(38),
                "ArrowDown" => Some(40),
                "ArrowLeft" => Some(37),
                "ArrowRight" => Some(39),
                "Home" => Some(36),
                "End" => Some(35),
                "PageUp" => Some(33),
                "PageDown" => Some(34),
                _ => None,
            };
            if let Some(vk) = vk {
                event["code"] = json!(key);
                event["windowsVirtualKeyCode"] = json!(vk);
            }
            for action in ["keyDown", "keyUp"] {
                let mut params = event.clone();
                params["type"] = json!(action);
                if key == "Enter" && action == "keyDown" {
                    params["text"] = json!("\r");
                    params["unmodifiedText"] = json!("\r");
                }
                cdp.send("Input.dispatchKeyEvent", params);
            }
        }
        _ => {}
    }
    Ok(())
}
