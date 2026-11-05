//! Igualdade das preferências antes da validação, inclusive números Python.
use serde_json::Value;
use crate::orchestration::codec;

enum Number { Integer(String), Float(f64) }

fn number(value: &Value) -> Option<Number> {
    match value {
        Value::Bool(value) => Some(Number::Integer(u8::from(*value).to_string())),
        Value::Number(value) => {
            let text = value.to_string();
            if text.contains(['.', 'e', 'E']) {
                value.as_f64().map(Number::Float)
            } else { Some(Number::Integer(text)) }
        }
        Value::String(_) if codec::contains_nonfinite(value) => {
            match codec::python_string(value).as_str() {
                "nan" => Some(Number::Float(f64::NAN)),
                "inf" => Some(Number::Float(f64::INFINITY)),
                "-inf" => Some(Number::Float(f64::NEG_INFINITY)),
                _ => None,
            }
        }
        _ => None,
    }
}

fn numbers_equal(left: Number, right: Number, nested: bool) -> bool {
    match (left, right) {
        (Number::Integer(left), Number::Integer(right)) => left == right,
        (Number::Float(left), Number::Float(right)) => {
            // O decoder Python reutiliza o objeto NaN; containers conferem identidade primeiro.
            left == right || (nested && left.is_nan() && right.is_nan())
        }
        (Number::Integer(integer), Number::Float(float))
        | (Number::Float(float), Number::Integer(integer)) => {
            float.is_finite() && float.fract() == 0.0
                && integer == if float == 0.0 { "0".into() } else { format!("{float:.0}") }
        }
    }
}

pub(crate) fn equal(left: &Value, right: &Value) -> bool {
    let mut pending = vec![(left, right, false)];
    while let Some((left, right, nested)) = pending.pop() {
        if let (Some(left), Some(right)) = (number(left), number(right)) {
            if !numbers_equal(left, right, nested) { return false; }
            continue;
        }
        match (left, right) {
            (Value::Array(left), Value::Array(right)) if left.len() == right.len() => {
                pending.extend(left.iter().zip(right).map(|(left,right)|(left,right,true)));
            }
            (Value::Object(left), Value::Object(right)) if left.len() == right.len() => {
                for (key,left) in left {
                    let Some(right) = right.get(key) else { return false; };
                    pending.push((left,right,true));
                }
            }
            (Value::Null,Value::Null) => {},
            (Value::String(left),Value::String(right)) if left == right => {},
            _ => return false,
        }
    }
    true
}
