//! Feed do condutor, lido diretamente do cofre e das unidades do vigia.
use crate::transcript::{py, pyjson};
use chrono::{Datelike, FixedOffset, Local, NaiveDate, TimeZone};
use regex::Regex;
use serde_json::{Value, json};
use std::{path::Path, sync::LazyLock};
pub const JEV_VETO_P: f64 = 0.40;
const PREFIXES: &[(&str, &str, bool)] = &[
    ("notify → arbiter: ", "woke", true),
    ("(jev: no action) ", "dropped", true),
    ("(regex: no action) ", "dropped", true),
    ("aviso: ", "notice", true),
    ("alarm: ", "alarm", true),
    ("notify FAILED: ", "alarm", false),
    ("close failed: ", "alarm", false),
    ("join failed: ", "alarm", false),
    ("closed session: ", "notice", false),
    ("joined group: ", "notice", false),
];
const TYPES: &[&str] = &[
    "execucao_inicio",
    "task_inicio",
    "entrega",
    "veredito",
    "sessao_trocada",
    "execucao_fim",
];
const MARK: &str = "\u{10ffff}orq-nonfinite:";
fn strip(s: &str) -> &str {
    s.trim_matches(|c: char| c.is_whitespace() || ('\x1c'..='\x1f').contains(&c))
}
fn lines(path: &Path) -> Vec<String> {
    let raw = std::fs::read(path).unwrap_or_default();
    py::splitlines(&String::from_utf8_lossy(&raw))
        .into_iter()
        .map(str::to_owned)
        .collect()
}
fn repr_string(text: &str, marker: &str) -> String {
    static PRINTABLE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[\p{L}\p{M}\p{N}\p{P}\p{S} ]$").unwrap());
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::new();
    out.push(quote);
    let mut at = 0;
    let prefix = format!("{marker}surrogate:");
    while at < text.len() {
        if text[at..].starts_with(&prefix) {
            let end = at + prefix.len() + 5;
            out.push_str(&format!("\\u{}", &text[at + prefix.len()..end - 1]));
            at = end;
            continue;
        }
        let ch = text[at..].chars().next().unwrap();
        at += ch.len_utf8();
        match ch {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c if c == quote => {
                out.push('\\');
                out.push(c)
            }
            c if !PRINTABLE.is_match(c.encode_utf8(&mut [0; 4])) => {
                let n = c as u32;
                out.push_str(&if n < 0x100 {
                    format!("\\x{n:02x}")
                } else if n < 0x10000 {
                    format!("\\u{n:04x}")
                } else {
                    format!("\\U{n:08x}")
                });
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
fn py_repr(v: &Value, marker: &str) -> String {
    match v {
        Value::String(text)
            if ["NaN", "Infinity", "-Infinity"]
                .iter()
                .any(|token| text == &format!("{marker}{token}")) =>
        {
            text[marker.len()..]
                .to_lowercase()
                .replace("infinity", "inf")
        }
        Value::String(text) if integer_text(text, marker).is_some() => {
            integer_text(text, marker).unwrap().to_owned()
        }
        Value::String(text) => repr_string(text, marker),
        Value::Number(n) if !n.to_string().contains(['.', 'e', 'E']) => n.to_string(),
        Value::Number(n) if n.as_f64().is_none_or(|f| !f.is_finite()) => {
            if n.to_string().starts_with('-') {
                "-inf".into()
            } else {
                "inf".into()
            }
        }
        Value::Array(items) => format!(
            "[{}]",
            items
                .iter()
                .map(|v| py_repr(v, marker))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Object(items) => format!(
            "{{{}}}",
            items
                .iter()
                .map(|(k, v)| format!("{}: {}", repr_string(k, marker), py_repr(v, marker)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => py::py_repr(v),
    }
}
pub(crate) fn python_string(value:&Value)->String { py_str(value,MARK) }
fn py_str(v: &Value, marker: &str) -> String {
    match v {
        Value::String(s)
            if integer_text(s, marker).is_none()
                && !["NaN", "Infinity", "-Infinity"]
                    .iter()
                    .any(|token| s == &format!("{marker}{token}")) =>
        {
            s.clone()
        }
        _ => py_repr(v, marker),
    }
}
fn when(v: &Value, fixed: Option<FixedOffset>, marker: &str) -> Option<i64> {
    let mut text = py_str(v, marker);
    let prefix = format!("{marker}surrogate:");
    while let Some(at) = text.find(&prefix) {
        // fromisoformat aceita qualquer caractere único como separador de data e hora.
        let end = at + prefix.len() + 5;
        text.replace_range(at..end, "\u{fffd}");
    }
    let text = if text.get(4..6) == Some("-W") || text.get(4..5) == Some("W") {
        let separated = text.get(4..5) == Some("-");
        let week_at = if separated { 6 } else { 5 };
        let year = text.get(..4)?.parse().ok()?;
        let week = text.get(week_at..week_at + 2)?.parse().ok()?;
        let mut end = week_at + 2;
        let mut weekday = 1;
        if separated && text.get(end..end + 1) == Some("-") {
            weekday = text.get(end + 1..end + 2)?.parse().ok()?;
            end += 2
        } else if !separated && text.as_bytes().get(end).is_some_and(u8::is_ascii_digit) {
            weekday = text.get(end..end + 1)?.parse().ok()?;
            end += 1
        }
        let weekday = match weekday {
            1 => chrono::Weekday::Mon,
            2 => chrono::Weekday::Tue,
            3 => chrono::Weekday::Wed,
            4 => chrono::Weekday::Thu,
            5 => chrono::Weekday::Fri,
            6 => chrono::Weekday::Sat,
            7 => chrono::Weekday::Sun,
            _ => return None,
        };
        let date = NaiveDate::from_isoywd_opt(year, week, weekday)?;
        format!(
            "{}{}",
            date.format("%Y-%m-%d"),
            text.get(end..).unwrap_or("")
        )
    } else {
        text
    };
    let (micros, offset) = py::fromisoformat(&text)?;
    let naive = chrono::DateTime::from_timestamp_micros(micros)?.naive_utc();
    if let Some(offset) = offset {
        let year = match fixed {
            Some(zone) => zone.timestamp_micros(micros - offset).single()?.year(),
            None => Local.timestamp_micros(micros - offset).single()?.year(),
        };
        (1..=9999).contains(&year).then_some(micros - offset)
    } else {
        // astimezone() consulta os dois lados da borda ao resolver a hora local.
        if (naive.year() == 1 && naive.ordinal() == 1)
            || (naive.year() == 9999 && naive.ordinal() == 365)
        {
            return None;
        }
        match fixed {
            Some(zone) => zone
                .from_local_datetime(&naive)
                .earliest()
                .map(|d| d.timestamp_micros()),
            None => Local
                .from_local_datetime(&naive)
                .earliest()
                .map(|d| d.timestamp_micros())
                .or_else(|| {
                    // Em uma hora pulada pelo horário de verão, fold=0 usa o deslocamento anterior.
                    [-1, 1]
                        .into_iter()
                        .filter_map(|days| naive.checked_add_signed(chrono::Duration::days(days)))
                        .filter_map(|near| Local.from_local_datetime(&near).earliest())
                        .map(|near| micros - i64::from(near.offset().local_minus_utc()) * 1_000_000)
                        .max()
                }),
        }
    }
}
fn integer_text<'a>(value: &'a str, marker: &str) -> Option<&'a str> {
    value
        .strip_prefix(marker)?
        .strip_prefix("integer:")?
        .strip_suffix(';')
}
fn integer_value(text: &str, marker: &str) -> Option<Value> {
    let n = text.parse::<num_bigint::BigInt>().ok()?;
    let canonical = n.to_string();
    if let Ok(n) = canonical.parse::<i64>() {
        return Some(json!(n));
    }
    if let Ok(n) = canonical.parse::<u64>() {
        return Some(json!(n));
    }
    Some(Value::String(format!("{marker}integer:{canonical};")))
}
fn int(v: &Value, marker: &str) -> Value {
    match v {
        Value::Number(n) if !n.is_f64() => v.clone(),
        Value::String(s) if integer_text(s, marker).is_some() => v.clone(),
        _ => Value::Null,
    }
}
fn decimal(c: char) -> Option<u8> {
    let n = c as u32;
    const STARTS: &[u32] = &[
        0x30, 0x660, 0x6f0, 0x7c0, 0x966, 0x9e6, 0xa66, 0xae6, 0xb66, 0xbe6, 0xc66, 0xce6, 0xd66,
        0xde6, 0xe50, 0xed0, 0xf20, 0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0, 0xa9f0, 0xaa50, 0xabf0,
        0xff10, 0x104a0, 0x10d30, 0x10d40, 0x11066, 0x110f0, 0x11136, 0x111d0, 0x112f0, 0x11450,
        0x114d0, 0x11650, 0x116c0, 0x116d0, 0x116da, 0x11730, 0x118e0, 0x11950, 0x11bf0, 0x11c50,
        0x11d50, 0x11da0, 0x11f50, 0x16130, 0x16a60, 0x16ac0, 0x16b50, 0x16d70, 0x1ccf0, 0x1d7ce,
        0x1d7d8, 0x1d7e2, 0x1d7ec, 0x1d7f6, 0x1e140, 0x1e2f0, 0x1e4f0, 0x1e5f1, 0x1e950, 0x1fbf0,
    ];
    STARTS
        .iter()
        .find_map(|start| (n >= *start && n < *start + 10).then(|| (n - start) as u8))
}
fn task(text: &str, marker: &str) -> Result<Value, &'static str> {
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"T(\d+)|Task (\d+)").unwrap());
    static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}_]$").unwrap());
    let word = |c: char| WORD.is_match(c.encode_utf8(&mut [0; 4]));
    let Some(c) = RE.captures_iter(text).find(|capture| {
        let full = capture.get(0).unwrap();
        !text[..full.start()].chars().next_back().is_some_and(word)
            && !text[full.end()..].chars().next().is_some_and(word)
    }) else {
        return Ok(Value::Null);
    };
    let digits: String = c
        .get(1)
        .or_else(|| c.get(2))
        .unwrap()
        .as_str()
        .chars()
        .filter_map(decimal)
        .map(|n| char::from(b'0' + n))
        .collect();
    if digits.len() > 4300 {
        return Err("orq_conductor_python_task_integer_limit");
    }
    Ok(integer_value(&digits, marker).unwrap_or(Value::Null))
}

fn num(v: &Value, marker: &str) -> Result<Value, &'static str> {
    if let Some(text) = v.as_str().and_then(|s| integer_text(s, marker)) {
        let n = text
            .parse::<f64>()
            .map_err(|_| "orq_conductor_python_number_overflow")?;
        return if n.is_finite() {
            Ok(json!(n))
        } else {
            Err("orq_conductor_python_number_overflow")
        };
    }
    let Some(n) = v.as_number() else {
        return Ok(Value::Null);
    };
    let x = n.as_f64().unwrap_or(f64::INFINITY);
    if !x.is_finite() {
        if !n.to_string().contains(['.', 'e', 'E']) {
            return Err("orq_conductor_python_number_overflow");
        }
        return Ok(Value::Null);
    }
    Ok(json!(x))
}
// json.loads aceita os três tokens; o restante da gramática continua estrito.
fn valid_number(text: &str) -> bool {
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?$").unwrap()
    });
    NUMBER.is_match(text)
}
fn key_follows(bytes: &[u8], after: usize) -> bool {
    bytes[after..]
        .iter()
        .find(|byte| !matches!(byte, b' ' | b'\t' | b'\r' | b'\n'))
        == Some(&b':')
}
fn load(text: &str, marker: &str) -> Option<Value> {
    let bytes = text.as_bytes();
    let (mut i, mut inside, mut escape) = (0, false, false);
    let mut raw = String::new();
    while i < bytes.len() {
        if inside {
            let ch = text[i..].chars().next()?;
            raw.push(ch);
            i += ch.len_utf8();
            if escape {
                escape = false
            } else if ch == '\\' {
                escape = true
            } else if ch == '"' {
                inside = false
            }
        } else if bytes[i] == b'"' {
            raw.push('"');
            inside = true;
            i += 1
        } else if bytes[i].is_ascii_digit()
            || (bytes[i] == b'-' && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = i;
            i += 1;
            while i < bytes.len()
                && !matches!(bytes[i], b',' | b'}' | b']' | b' ' | b'\t' | b'\r' | b'\n')
            {
                i += 1
            }
            let token = &text[start..i];
            // A expansão só representa valores; uma chave sem aspas precisa continuar inválida.
            if key_follows(bytes, i) {
                raw.push_str(token);
                continue;
            }
            let digits = token.strip_prefix('-').unwrap_or(token);
            let integer = !digits.is_empty()
                && digits.bytes().all(|b| b.is_ascii_digit())
                && (digits.len() == 1 || !digits.starts_with('0'));
            if integer {
                if digits.len() > 4300 {
                    return None;
                }
                raw.push_str(&integer_value(token, marker)?.to_string());
            } else if valid_number(token)
                && let Ok(number) = token.parse::<f64>()
            {
                if !number.is_finite() {
                    raw.push_str(
                        &serde_json::to_string(&format!(
                            "{marker}{}",
                            if number.is_sign_negative() {
                                "-Infinity"
                            } else {
                                "Infinity"
                            }
                        ))
                        .ok()?,
                    )
                } else {
                    raw.push_str(token)
                }
            } else {
                raw.push_str(token)
            }
        } else if let Some(token) = ["-Infinity", "Infinity", "NaN"]
            .into_iter()
            .find(|token| text[i..].starts_with(token))
        {
            if key_follows(bytes, i + token.len()) {
                raw.push_str(token)
            } else {
                raw.push_str(&serde_json::to_string(&format!("{marker}{token}")).ok()?)
            }
            i += token.len()
        } else {
            let ch = text[i..].chars().next()?;
            raw.push(ch);
            i += ch.len_utf8()
        }
    }
    fn decode(raw: &str) -> Option<Value> {
        use serde::Deserialize;
        let mut decoder = serde_json::Deserializer::from_str(raw);
        decoder.disable_recursion_limit();
        let value = Value::deserialize(&mut decoder).ok()?;
        decoder.end().ok()?;
        Some(value)
    }
    decode(&raw).or_else(|| {
        raw.contains("\\u")
            .then(|| {
                decode(&pyjson::mark_lone_surrogates_using(&raw, |code| {
                    format!("{marker}surrogate:{code:04x};")
                }))
            })
            .flatten()
    })
}
fn jev(row: &Value, marker: &str) -> Result<Value, &'static str> {
    let mut veto = serde_json::Map::new();
    let mut held = Vec::new();
    if let Some(raw) = row["veto"].as_object() {
        for (key, value) in raw {
            let value = num(value, marker)?;
            if value.is_null() || value.as_f64().is_some_and(|x| x > JEV_VETO_P) {
                held.push(key)
            }
            veto.insert(key.clone(), value);
        }
    }
    Ok(
        json!({"mode":row["mode"],"choice":row["choice"],"p":num(&row["p"],marker)?,"veto":veto,"held":held,"would_drop":row["would_drop"],"error":row["error"]}),
    )
}
fn event_text(row: &Value, marker: &str) -> String {
    let mut parts = vec![row["tipo"].as_str().unwrap_or_default().to_owned()];
    for (field, prefix) in [("task", "T"), ("rodada", "r")] {
        let n = int(&row[field], marker);
        if !n.is_null() {
            parts.push(format!("{prefix}{}", py_str(&n, marker)))
        }
    }
    for key in [
        "resultado",
        "commit",
        "sessao",
        "executor",
        "par",
        "de",
        "para",
        "motivo",
    ] {
        let value = &row[key];
        if !value.is_null() && value != "" {
            parts.push(format!("{key}={}", py_str(value, marker)))
        }
    }
    if row["reincide"] == Value::Bool(true) {
        parts.push("reincide".into())
    }
    parts.join(" ")
}
/// Só leitura; bugs conhecidos do Python continuam recusas, sem fabricar um feed parcial.
pub fn feed(root: &Path) -> Result<Vec<u8>, &'static str> {
    on_stack(root, |sources| {
        let (value, marker) = feed_core(sources, None)?;
        serialize(&value, &marker)
    })
}

/// Fuso explícito para comparar o mesmo contrato sem alterar o ambiente do processo.
pub fn feed_json_with_timezone(
    root: &Path,
    timezone: FixedOffset,
) -> Result<Vec<u8>, &'static str> {
    on_stack(root, move |sources| {
        let (value, marker) = feed_core(sources, Some(timezone))?;
        serialize(&value, &marker)
    })
}

struct Sources {
    journal: Vec<String>,
    shadow: Vec<String>,
    events: Vec<String>,
}

fn nesting(text: &str) -> usize {
    let (mut depth, mut maximum, mut quoted, mut escaped) = (0usize, 0usize, false, false);
    for ch in text.chars() {
        if quoted {
            if escaped {
                escaped = false
            } else if ch == '\\' {
                escaped = true
            } else if ch == '"' {
                quoted = false
            }
        } else {
            match ch {
                '"' => quoted = true,
                '[' | '{' => {
                    depth = depth.saturating_add(1);
                    maximum = maximum.max(depth)
                }
                ']' | '}' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
    }
    maximum
}

fn on_stack<T: Send + 'static>(
    root: &Path,
    work: impl FnOnce(Sources) -> Result<T, &'static str> + Send + 'static,
) -> Result<T, &'static str> {
    let sources = Sources {
        journal: lines(&root.join("registro.md")),
        shadow: lines(&root.join("jev-shadow.jsonl")),
        events: lines(&root.join("eventos.jsonl")),
    };
    let depth = sources
        .shadow
        .iter()
        .skip(sources.shadow.len().saturating_sub(300))
        .chain(sources.events.iter())
        .map(|line| nesting(line))
        .max()
        .unwrap_or(0);
    run_with_stack(depth, move || work(sources))
}
fn run_with_stack<T: Send + 'static>(
    depth: usize,
    work: impl FnOnce() -> Result<T, &'static str> + Send + 'static,
) -> Result<T, &'static str> {
    if depth <= 64 {
        return work();
    }
    // O decoder, o repr e a serialização precisam da mesma pilha; o caminho comum não cria thread.
    let stack = depth
        .checked_mul(8192)
        .ok_or("orq_conductor_stack_unavailable")?
        .max(16 * 1024 * 1024);
    std::thread::Builder::new()
        .name("orq-conductor-json".into())
        .stack_size(stack)
        .spawn(move || work())
        .map_err(|_| "orq_conductor_stack_unavailable")?
        .join()
        .map_err(|_| "orq_conductor_feed_failed")?
}

fn serialize(value: &Value, marker: &str) -> Result<Vec<u8>, &'static str> {
    let raw = serde_json::to_string(value).map_err(|_| "orq_conductor_feed_failed")?;
    let bytes = raw.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'"' {
            out.push(bytes[i]);
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        let mut escaped = false;
        while i < bytes.len() {
            let byte = bytes[i];
            i += 1;
            if escaped {
                escaped = false
            } else if byte == b'\\' {
                escaped = true
            } else if byte == b'"' {
                break;
            }
        }
        let text: String =
            serde_json::from_str(&raw[start..i]).map_err(|_| "orq_conductor_feed_failed")?;
        if let Some(integer) = integer_text(&text, marker) {
            out.extend_from_slice(integer.as_bytes())
        } else {
            out.extend_from_slice(&bytes[start..i])
        }
    }
    Ok(out)
}
fn feed_bytes(root: &Path, watchdog: Vec<u8>) -> Result<Vec<u8>, &'static str> {
    on_stack(root, move |sources| {
        let (value, marker) = feed_core(sources, None)?;
        let feed = serialize(&value, &marker)?;
        let mut out = b"{\"watchdog\":".to_vec();
        out.extend(watchdog);
        out.push(b',');
        out.extend_from_slice(&feed[1..]);
        Ok(out)
    })
}

fn feed_core(
    sources: Sources,
    fixed: Option<FixedOffset>,
) -> Result<(Value, String), &'static str> {
    struct Item {
        when: i64,
        value: Value,
    }
    struct Shadow {
        when: i64,
        norm: String,
        cut: bool,
        row: Value,
        used: bool,
    }
    static LINE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^- ([^\s\x1c-\x1f]+) · (.*)$").unwrap());
    let (mut items, mut skipped, mut shadows) = (Vec::new(), 0, Vec::new());
    let Sources {
        journal,
        shadow,
        events,
    } = sources;
    // O marcador nunca coincide com texto legítimo, inclusive strings com escapes Unicode.
    let baseline = journal
        .iter()
        .skip(journal.len().saturating_sub(300))
        .cloned()
        .chain(
            shadow
                .iter()
                .skip(shadow.len().saturating_sub(300))
                .chain(events.iter())
                .map(|line| load(line, "").map(|v| v.to_string()).unwrap_or_default()),
        )
        .collect::<Vec<_>>()
        .join("\n");
    let mut marker = MARK.to_owned();
    while baseline.contains(&marker) {
        marker.push('#');
    }

    for (n, line) in journal
        .iter()
        .enumerate()
        .skip(journal.len().saturating_sub(300))
    {
        if strip(line).is_empty() {
            continue;
        }
        let Some(c) = LINE.captures(line) else {
            skipped += 1;
            continue;
        };
        let Some(when) = when(&json!(&c[1]), fixed, &marker) else {
            skipped += 1;
            continue;
        };
        for &(prefix, kind, remove) in PREFIXES {
            if c[2].starts_with(prefix) {
                let text = if remove { &c[2][prefix.len()..] } else { &c[2] };
                items.push(Item{when,value:json!({"id":format!("r{}",n+1),"ts":&c[1],"kind":kind,"text":text,"task":task(text,&marker)?,"jev":null})});
                break;
            }
        }
    }
    for line in shadow.iter().skip(shadow.len().saturating_sub(300)) {
        if strip(line).is_empty() {
            continue;
        }
        let Some(row) = load(line, &marker) else {
            skipped += 1;
            continue;
        };
        let Some((when, text)) = when(&row["ts"], fixed, &marker).zip(row["text"].as_str()) else {
            skipped += 1;
            continue;
        };
        shadows.push(Shadow {
            when,
            norm: py::splitlines(strip(text)).join(" ⏎ "),
            cut: text.chars().count() >= 500,
            row,
            used: false,
        });
    }
    for item in &mut items {
        if !matches!(item.value["kind"].as_str(), Some("woke" | "dropped")) {
            continue;
        }
        let text = item.value["text"].as_str().unwrap_or_default();
        if let Some(s) = shadows.iter_mut().find(|s| {
            !s.used
                && (s.when - item.when).abs() <= 10_000_000
                && (s.norm == text || (s.cut && text.starts_with(&s.norm)))
        }) {
            item.value["jev"] = jev(&s.row, &marker)?;
            s.used = true
        }
    }
    for (n, line) in events.iter().enumerate() {
        if strip(line).is_empty() {
            continue;
        }
        let Some(row) = load(line, &marker) else {
            skipped += 1;
            continue;
        };
        if row
            .get("tipo")
            .is_some_and(|v| v.is_array() || v.is_object())
        {
            return Err("orq_conductor_python_unhashable_type");
        }
        if !row["tipo"].as_str().is_some_and(|t| TYPES.contains(&t)) {
            skipped += 1;
            continue;
        }
        let Some(when) = when(&row["ts"], fixed, &marker) else {
            skipped += 1;
            continue;
        };
        items.push(Item{when,value:json!({"id":format!("e{}",n+1),"ts":py_str(&row["ts"],&marker),"kind":"event","text":event_text(&row,&marker),"task":int(&row["task"],&marker),"jev":null})});
    }
    items.sort_by(|a, b| b.when.cmp(&a.when));
    let truncated = items.len() > 500;
    let value = json!({"feed":items.into_iter().take(500).map(|item|item.value).collect::<Vec<_>>(),"truncated":truncated,"skipped":skipped});
    fn invalid(v: &Value, marker: &str) -> bool {
        match v {
            Value::String(text) => text.contains(marker) && integer_text(text, marker).is_none(),
            Value::Number(n) => {
                n.to_string().contains(['.', 'e', 'E']) && n.as_f64().is_none_or(|x| !x.is_finite())
            }
            Value::Array(items) => items.iter().any(|v| invalid(v, marker)),
            Value::Object(items) => items
                .iter()
                .any(|(k, v)| k.contains(marker) || invalid(v, marker)),
            _ => false,
        }
    }
    if invalid(&value, &marker) {
        return Err("orq_conductor_python_nonfinite_output");
    }
    Ok((value, marker))
}

mod http;
pub use http::{matches, public, serve};
