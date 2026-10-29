//! Tradução das mensagens do espectador para o protocolo do navegador.
use super::{cdp::Cdp, ws::Failure};
use serde_json::{Value, json};

pub(super) fn truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(v) => *v,
        Value::Number(v) => v.as_f64().map_or_else(|| v.to_string() != "0", |n| n != 0.0),
        Value::String(v) => !v.is_empty(),
        Value::Array(v) => !v.is_empty(),
        Value::Object(v) => !v.is_empty(),
    }
}
pub(super) fn string(value: &Value) -> String {
    super::value::python_string(value)
}

pub(super) fn normalize_numeric_separators(raw: &str) -> Option<String> {
    let mut normalized = String::with_capacity(raw.len());
    let mut characters = raw.chars().peekable();
    let mut previous_digit = false;
    while let Some(character) = characters.next() {
        if character == '_' {
            if !previous_digit || !characters.peek().is_some_and(|c| crate::orq_conductor::decimal(*c).is_some()) {
                return None;
            }
        } else {
            normalized.push(character);
        }
        previous_digit = crate::orq_conductor::decimal(character).is_some();
    }
    Some(normalized)
}

pub(super) fn integer(value: &Value) -> Result<i64, Failure> {
    super::value::integer(value)?.as_i64()
        .ok_or_else(|| Failure::value("int() value out of range".into()))
}
fn default<'a>(value: &'a Value, zero: &'a Value) -> &'a Value {
    if truth(value) { value } else { zero }
}
pub(super) fn apply(cdp: &Cdp, message: &Value, width: &Value, height: &Value) -> Result<(), Failure> {
    let zero = json!(0);
    let modifiers = || super::value::integer(default(&message["mod"], &zero));
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
                "x": super::value::pixel(&message["x"], width)?, "y": super::value::pixel(&message["y"], height)?,
                "button": button,
                "clickCount": if action == "mouseMoved" { 0 } else { 1 },
                "modifiers": modifiers()?,
            }));
        }
        Some("w") => cdp.send("Input.dispatchMouseEvent", json!({
            "type": "mouseWheel",
            "x": super::value::pixel(&message["x"], width)?, "y": super::value::pixel(&message["y"], height)?,
            "deltaX": super::value::float_value(super::value::number(default(&message["dx"], &zero))?),
            "deltaY": super::value::float_value(super::value::number(default(&message["dy"], &zero))?),
            "button": "none", "clickCount": 0, "modifiers": 0,
        })),
        Some("txt") => {
            let text = if truth(&message["s"]) {
                super::value::truncate(&string(&message["s"]), 4096)
            } else {
                String::new()
            };
            if !text.is_empty() {
                cdp.send("Input.insertText", json!({"text": text}));
            }
        }
        Some("k") => {
            let key = if truth(&message["key"]) {
                super::value::truncate(&string(&message["key"]), 32)
            } else {
                String::new()
            };
            if key.is_empty() {
                return Ok(());
            }
            let mut event = json!({
                "key": key, "modifiers": modifiers()?,
                "windowsVirtualKeyCode": super::value::integer(default(&message["code"], &zero))?,
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
