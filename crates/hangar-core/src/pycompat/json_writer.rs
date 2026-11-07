//! A profundidade do documento ocupa memória de trabalho, nunca a pilha do processo.
use serde_json::Value;
use std::cmp::Ordering;

enum Task<'a> {
    Value(&'a Value, usize),
    Key(&'a str),
    Text(&'static str),
    Line(usize),
}

pub(super) fn write(
    out: &mut String,
    value: &Value,
    ascii: bool,
    indent: Option<usize>,
    order: Option<&dyn Fn(&str, &str) -> Ordering>,
    compact: bool,
    legacy_surrogates: bool,
) {
    use super::pyjson::{number_repr, write_str};
    let mut pending = vec![Task::Value(value, 0)];
    while let Some(task) = pending.pop() {
        match task {
            Task::Text(text) => out.push_str(text),
            Task::Key(key) => write_str(out, key, ascii, legacy_surrogates),
            Task::Line(level) => {
                out.push('\n');
                out.extend(std::iter::repeat_n(' ', indent.unwrap_or(0) * level));
            }
            Task::Value(value, level) => match value {
                Value::Null => out.push_str("null"),
                Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
                Value::Number(value) => out.push_str(&number_repr(value)),
                Value::String(value) => write_str(out, value, ascii, legacy_surrogates),
                Value::Array(items) => {
                    out.push('[');
                    pending.push(Task::Text("]"));
                    if indent.is_some() && !items.is_empty() {
                        pending.push(Task::Line(level));
                    }
                    for (index, value) in items.iter().enumerate().rev() {
                        pending.push(Task::Value(value, level + 1));
                        if indent.is_some() {
                            pending.push(Task::Line(level + 1));
                        }
                        if index > 0 {
                            pending.push(Task::Text(if indent.is_some() || compact { "," } else { ", " }));
                        }
                    }
                }
                Value::Object(items) => {
                    let mut pairs: Vec<_> = items.iter().collect();
                    if let Some(compare) = order {
                        pairs.sort_by(|left, right| compare(left.0, right.0));
                    }
                    out.push('{');
                    pending.push(Task::Text("}"));
                    if indent.is_some() && !pairs.is_empty() {
                        pending.push(Task::Line(level));
                    }
                    for (index, (key, value)) in pairs.into_iter().enumerate().rev() {
                        pending.push(Task::Value(value, level + 1));
                        pending.push(Task::Text(if compact { ":" } else { ": " }));
                        pending.push(Task::Key(key));
                        if indent.is_some() {
                            pending.push(Task::Line(level + 1));
                        }
                        if index > 0 {
                            pending.push(Task::Text(if indent.is_some() || compact { "," } else { ", " }));
                        }
                    }
                }
            },
        }
    }
}
