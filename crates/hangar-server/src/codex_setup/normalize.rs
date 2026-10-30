//! Normalização dos comandos de hooks importados, sem executar expressões de shell.
use crate::{accounts::quotas::other::py_strip, orchestration::codec};
use serde_json::{Value, json};
#[path="hook_launch.rs"]
mod hook_launch;

fn executable_command(text: &str, windows: bool) -> std::borrow::Cow<'_, str> {
    if windows && let Some((path, args)) = hook_launch::parse(text) {
        return format!("& \"{path}\" {args}").into();
    }
    text.into()
}
fn basename(text: &str) -> String {
    text.rsplit(['/', '\\']).next().unwrap_or("").to_lowercase()
}
fn split(command: &str, windows: bool) -> Result<Vec<String>, String> {
    let normalized = executable_command(command, windows);
    let command = normalized.as_ref();
    if !windows {
        return crate::loops::check::split(command, true).map_err(str::to_owned);
    }
    let chars: Vec<_> = command.chars().collect();
    let (mut out, mut word, mut quoted, mut started, mut i) =
        (Vec::new(), String::new(), false, false, 0);
    while i < chars.len() {
        let c = chars[i];
        if matches!(c, ' ' | '\t') && !quoted {
            if started {
                out.push(std::mem::take(&mut word));
                started = false;
            }
            i += 1;
            continue;
        }
        started = true;
        if c == '\\' {
            let begin = i;
            while i < chars.len() && chars[i] == '\\' {
                i += 1;
            }
            let n = i - begin;
            if i < chars.len() && chars[i] == '"' {
                word.extend(std::iter::repeat_n('\\', n / 2));
                if n % 2 == 1 {
                    word.push('"');
                } else {
                    quoted = !quoted;
                }
                i += 1;
            } else {
                word.extend(std::iter::repeat_n('\\', n));
            }
        } else if c == '"' {
            quoted = !quoted;
            i += 1;
        } else {
            word.push(c);
            i += 1;
        }
    }
    if quoted {
        return Err("Aspas incompletas no comando Windows".into());
    }
    if started {
        out.push(word);
    }
    if out.first().is_some_and(|word| word == "&") {
        out.remove(0);
    }
    Ok(out)
}
fn parts(command: &str, windows: bool) -> Option<Vec<String>> {
    let normalized = executable_command(command, windows);
    let command = normalized.as_ref();
    let chars: Vec<_> = command.chars().collect();
    let (mut out, mut start, mut quote, mut i) = (Vec::new(), 0, None, 0);
    while i < chars.len() {
        let mut c = chars[i];
        if c == '\\' && !windows && quote != Some('\'') {
            i += 2;
            continue;
        }
        if windows && c == '\\' {
            let mut end = i;
            while end < chars.len() && chars[end] == '\\' {
                end += 1;
            }
            if end == chars.len() || chars[end] != '"' {
                i = end;
                continue;
            }
            if (end - i) % 2 == 1 {
                i = end + 1;
                continue;
            }
            i = end;
            c = chars[i];
        }
        if c == '"' || (!windows && c == '\'') {
            if quote == Some(c) {
                quote = None;
            } else if quote.is_none() {
                quote = Some(c);
            }
        } else if (windows && matches!(c, '%' | '!'))
            || (!windows && quote != Some('\'') && matches!(c, '$' | '`'))
        {
            return None;
        } else if quote.is_none() {
            if c == '|' {
                out.push(py_strip(&chars[start..i].iter().collect::<String>()).to_owned());
                start = i + 1;
            } else if ";&<>\r\n()".contains(c)
                || (windows && c == '^')
                || (!windows && "*?[]{}~#".contains(c))
            {
                return None;
            }
        }
        i += 1;
    }
    if quote.is_some() {
        return None;
    }
    out.push(py_strip(&chars[start..].iter().collect::<String>()).to_owned());
    Some(out)
}
fn rtk(command: &str, windows: bool, native: bool) -> Option<Vec<String>> {
    let parts = parts(command, windows)?;
    if parts.len() > 2 {
        return None;
    }
    let mut args = split(&parts[0], windows).ok()?;
    if native && args.len() >= 6 && args[2] == "--"
        && matches!(basename(&args[1]).as_str(), "codex-hook-allow.py" | "codex-hook-allow")
    { args.drain(..3); }
    if args.len() < 3
        || !matches!(basename(&args[0]).as_str(), "rtk" | "rtk.exe")
        || args[1] != "hook"
        || args[2] != "claude"
    {
        return None;
    }
    if parts.len() == 2 {
        let filter = split(&parts[1], windows).ok()?;
        let native_filter = native && filter.len() == 2 && filter[1] == "codex-hook-allow";
        if !native_filter && (filter.len() != 2
            || !regex::Regex::new(r"\Apython(?:\d+(?:\.\d+)*)?(?:\.exe)?\z")
                .unwrap()
                .is_match(&basename(&filter[0]))
            || basename(&filter[1]) != "codex-hook-allow.py") {
            return None;
        }
    }
    Some(args)
}
fn command(args: &[String], windows: bool) -> String {
    if !windows {
        return args
            .iter()
            .map(|arg| crate::runtime::claude_relaunch::quote(arg, false))
            .collect::<Vec<_>>()
            .join(" ");
    }
    let expression = args.first().and_then(|path| hook_launch::expression(path));
    let skip = usize::from(expression.is_some());
    let mut out = vec![expression.unwrap_or_else(|| "&".to_owned())];
    for arg in &args[skip..] {
        let mut token = String::from("\"");
        let mut slashes = 0;
        for c in arg.chars() {
            if c == '\\' {
                slashes += 1;
                continue;
            }
            token.extend(std::iter::repeat_n(
                '\\',
                if c == '"' { slashes * 2 + 1 } else { slashes },
            ));
            slashes = 0;
            token.push(c);
        }
        token.extend(std::iter::repeat_n('\\', slashes * 2));
        token.push('"');
        out.push(token);
    }
    out.join(" ")
}
pub fn hooks(config: &Value, executable: &str, wrapper: &str, windows: bool) -> Value {
    let mut result = codec::clone_value(config);
    let Some(events) = result.get_mut("hooks").and_then(Value::as_object_mut) else {
        return result;
    };
    for (event, groups) in events {
        for group in groups.as_array_mut().into_iter().flatten() {
            for hook in group
                .get_mut("hooks")
                .and_then(Value::as_array_mut)
                .into_iter()
                .flatten()
            {
                if hook.get("type") != Some(&json!("command")) {
                    continue;
                }
                if event == "SessionEnd"
                    && hook
                        .get("timeout")
                        .and_then(Value::as_f64)
                        .is_some_and(|n| n > 3.)
                {
                    hook["timeout"] = json!(3);
                }
                if event == "PreToolUse" {
                    if let Some(args) = hook
                        .get("command")
                        .and_then(Value::as_str)
                        .and_then(|text| rtk(text, windows, wrapper == "codex-hook-allow"))
                    {
                        let mut next = vec![executable.to_owned(), wrapper.to_owned(), "--".into()];
                        next.extend(args);
                        hook["command"] = json!(command(&next, windows));
                    }
                }
            }
        }
    }
    result
}
pub fn security(
    config: &Value,
    executable: &str,
    wrapper: &str,
    windows: bool,
) -> Result<Value, String> {
    let mut result = codec::clone_value(config);
    let Some(events) = result.get_mut("hooks") else {
        return Ok(result);
    };
    let events = events.as_object_mut().ok_or("Objeto de hooks esperado")?;
    for groups in events.values_mut() {
        let groups = groups.as_array_mut().ok_or("Lista de grupos esperada")?;
        for group in groups {
            let Some(hooks) = group.get_mut("hooks") else {
                continue;
            };
            for hook in hooks.as_array_mut().ok_or("Lista de hooks esperada")? {
                let Some(text) = hook.get("command").and_then(Value::as_str) else {
                    continue;
                };
                if hook.get("type") != Some(&json!("command")) {
                    continue;
                }
                let args = split(text, windows).unwrap_or_default();
                let previous_wrapper = args.len() == 4 && args[2] == "--";
                if wrapper != "codex-hook-json" && previous_wrapper && basename(&args[1]) == basename(wrapper) {
                    continue;
                }
                let source = if wrapper == "codex-hook-json" && previous_wrapper
                    && matches!(basename(&args[1]).as_str(), "codex-hook-json.py" | "codex-hook-json")
                { &args[3] } else { text };
                hook["command"] = json!(command(
                    &[executable.into(), wrapper.into(), "--".into(), source.into()],
                    windows
                ));
            }
        }
    }
    Ok(result)
}
pub fn installed(config: &Value, windows: bool) -> Option<(String, String)> {
    for group in config.get("hooks")?.get("PreToolUse")?.as_array()? {
        for hook in group
            .get("hooks")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(text) = hook.get("command").and_then(Value::as_str) else {
                continue;
            };
            let Ok(args) = split(text, windows) else {
                continue;
            };
            if args.len() >= 6
                && args[2..6] == ["--", "rtk", "hook", "claude"]
                && matches!(basename(&args[1]).as_str(), "codex-hook-allow.py" | "codex-hook-allow")
            {
                return Some((args[0].clone(), args[1].clone()));
            }
        }
    }
    None
}

#[cfg(test)]
#[path="normalize_native_tests.rs"]
mod native_tests;
