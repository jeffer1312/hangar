//! Árvores JSON não usam a profundidade dos dados como profundidade da pilha.
use serde::Deserialize;
use serde_json::{Map, Number, Value};

fn typed<'de, T: Deserialize<'de>>(raw: &'de str) -> Option<T> {
    let mut decoder = serde_json::Deserializer::from_str(raw);
    decoder.disable_recursion_limit();
    let value = T::deserialize(&mut decoder).ok()?;
    decoder.end().ok()?;
    Some(value)
}

pub fn discard(value: Value) {
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        match value {
            Value::Array(values) => pending.extend(values),
            Value::Object(values) => pending.extend(values.into_values()),
            _ => {}
        }
    }
}

enum Frame {
    Array(Vec<Value>),
    Object(Map<String, Value>, Option<String>),
}
#[derive(Default)]
struct Frames(Vec<Frame>);
impl Drop for Frames {
    fn drop(&mut self) {
        for frame in self.0.drain(..) {
            discard(match frame {
                Frame::Array(values) => Value::Array(values),
                Frame::Object(values, _) => Value::Object(values),
            });
        }
    }
}

pub fn decode(raw: &str) -> Option<Value> {
    // A gramática é conferida uma vez; reler cada subárvore seria quadrático.
    let _: &serde_json::value::RawValue = typed(raw)?;
    let bytes = raw.as_bytes();
    let mut at = 0;
    let mut frames = Frames::default();
    while at < bytes.len() {
        let value = match bytes[at] {
            b' ' | b'\t' | b'\r' | b'\n' | b',' | b':' => { at += 1; continue; }
            b'[' => { frames.0.push(Frame::Array(Vec::new())); at += 1; continue; }
            b'{' => { frames.0.push(Frame::Object(Map::new(), None)); at += 1; continue; }
            b']' | b'}' => {
                at += 1;
                match frames.0.pop()? {
                    Frame::Array(values) => Value::Array(values),
                    Frame::Object(values, _) => Value::Object(values),
                }
            }
            b'"' => {
                let start = at;
                at += 1;
                while bytes[at] != b'"' {
                    at += if bytes[at] == b'\\' { 2 } else { 1 };
                }
                at += 1;
                let text: String = typed(&raw[start..at])?;
                if let Some(Frame::Object(_, key @ None)) = frames.0.last_mut() {
                    *key = Some(text);
                    continue;
                }
                Value::String(text)
            }
            _ => {
                let start = at;
                while at < bytes.len() && !b" \t\r\n,]}".contains(&bytes[at]) { at += 1; }
                let token = &raw[start..at];
                match token {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "null" => Value::Null,
                    _ if token == "-0" || token.contains(['.', 'e', 'E']) => {
                        Value::Number(Number::from_f64(typed(token)?)?)
                    }
                    _ => {
                        let canonical = token.parse::<num_bigint::BigInt>().ok()?.to_string();
                        Value::Number(canonical.parse().ok()?)
                    }
                }
            }
        };
        match frames.0.last_mut() {
            Some(Frame::Array(values)) => values.push(value),
            Some(Frame::Object(values, key)) => {
                if let Some(previous) = values.insert(key.take()?, value) { discard(previous); }
            }
            None => return Some(value),
        }
    }
    None
}

/// Copia a estrutura sem recursão e transforma somente folhas escalares.
pub fn map_leaves(value: &Value, mut leaf: impl FnMut(&Value) -> Value) -> Value {
    enum Task<'a> { Visit(&'a Value), Array(usize), Object(Vec<String>) }
    let mut tasks = vec![Task::Visit(value)];
    let mut output = Vec::new();
    while let Some(task) = tasks.pop() {
        match task {
            Task::Visit(Value::Array(values)) => {
                tasks.push(Task::Array(values.len()));
                tasks.extend(values.iter().rev().map(Task::Visit));
            }
            Task::Visit(Value::Object(values)) => {
                tasks.push(Task::Object(values.keys().cloned().collect()));
                tasks.extend(values.values().rev().map(Task::Visit));
            }
            Task::Visit(value) => output.push(leaf(value)),
            Task::Array(length) => {
                let values = output.split_off(output.len() - length);
                output.push(Value::Array(values));
            }
            Task::Object(keys) => {
                let values = output.split_off(output.len() - keys.len());
                output.push(Value::Object(keys.into_iter().zip(values).collect()));
            }
        }
    }
    output.pop().expect("a visita produz exatamente uma raiz")
}
