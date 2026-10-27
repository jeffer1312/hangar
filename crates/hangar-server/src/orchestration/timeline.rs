//! Uma projeção estruturada para o chat, o histórico e o painel.
use super::{runs, text, truthy};
use indexmap::IndexMap;
use regex::Regex;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::{LazyLock, Mutex},
    time::SystemTime,
};
static SHADOW: LazyLock<
    Mutex<IndexMap<std::path::PathBuf, (Option<(SystemTime, u64)>, Vec<Value>)>>,
> = LazyLock::new(Mutex::default);
fn regex(pattern: &str) -> Regex {
    static REGEXES: LazyLock<Mutex<std::collections::HashMap<String, Regex>>> =
        LazyLock::new(Mutex::default);
    REGEXES
        .lock()
        .unwrap()
        .entry(pattern.to_owned())
        .or_insert_with(|| Regex::new(pattern).expect("expressão fixa válida"))
        .clone()
}
fn task(text: &str) -> Value {
    regex(r"\bT(\d+)\b|\bTask (\d+)\b")
        .captures(text)
        .and_then(|m| m.get(1).or_else(|| m.get(2)))
        .and_then(|m| m.as_str().parse::<u64>().ok())
        .map(|n| json!(n))
        .unwrap_or_default()
}
fn number(v: &Value) -> Value {
    v.as_f64()
        .filter(|n| n.is_finite())
        .map(|n| json!(n))
        .unwrap_or_default()
}
fn integer(v: &Value) -> Value {
    if v.is_boolean() {
        return json!(u8::from(v == true));
    }
    v.as_i64()
        .map(|n| json!(n))
        .or_else(|| v.as_u64().map(|n| json!(n)))
        .or_else(|| {
            v.as_f64()
                .filter(|n| n.is_finite())
                .map(|n| json!(n.trunc() as i64))
        })
        .or_else(|| {
            v.as_str()
                .and_then(|s| s.trim().parse::<i64>().ok())
                .map(|n| json!(n))
        })
        .unwrap_or_default()
}
fn shadow(dir: &Path) -> Vec<Value> {
    let path = dir.join("jev-shadow.jsonl");
    let sig = fs::metadata(&path)
        .ok()
        .and_then(|m| Some((m.modified().ok()?, m.len())));
    let mut cache = SHADOW.lock().unwrap();
    if let Some((saved, rows)) = cache.get(&path) {
        if *saved == sig {
            return rows.clone();
        }
    }
    let rows = runs::lines(&path, false).unwrap_or_default();
    if cache.len() >= 16 {
        cache.shift_remove_index(0);
    }
    cache.insert(path, (sig, rows.clone()));
    rows
}
fn jev_for(dir: &Path, msg: &str, at: &Value) -> Value {
    let Some(at) = runs::when(at) else {
        return Value::Null;
    };
    let prefix: String = msg.chars().take(500).collect();
    let mut best = None;
    for row in shadow(dir) {
        if row["text"] != prefix {
            continue;
        }
        let Some(t) = runs::when(&row["ts"]) else {
            continue;
        };
        let elapsed = (at - t).num_microseconds().unwrap_or(i64::MAX);
        if (0..=45_000_000).contains(&elapsed) && best.as_ref().is_none_or(|(old, _)| t > *old) {
            best = Some((t, row))
        }
    }
    let Some((_, r)) = best else {
        return Value::Null;
    };
    let mut veto = serde_json::Map::new();
    let mut held = Vec::new();
    if let Some(values) = r["veto"].as_object() {
        for (k, v) in values {
            let n = number(v);
            if n.is_null() || n.as_f64().unwrap_or(0.) > 0.4 {
                held.push(k.clone())
            }
            veto.insert(k.clone(), n);
        }
    }
    let mut probs = serde_json::Map::new();
    if let Some(p) = r["probs"].as_object() {
        for (k, v) in p {
            let n = number(v);
            if !n.is_null() {
                probs.insert(k.clone(), n);
            }
        }
    }
    json!({"mode":r["mode"],"choice":r["choice"],"p":number(&r["p"]),"veto":veto,"held":held,"would_drop":r["would_drop"],"error":r["error"],"probs":probs})
}
fn advance(s: &str) -> (Value, Value) {
    if let Some(m) = regex(r"^T(\d+): abriu (.+) e entregou o kick-off$").captures(s) {
        let sessions: Vec<_> = regex(r"([^\s()]+) \(([^,()]+), ([^()]+)\)")
            .captures_iter(&m[2])
            .map(|m| json!({"name":&m[1],"provider":&m[2],"model":&m[3]}))
            .collect();
        return (
            json!({"code":"opened","sessions":sessions}),
            integer(&json!(&m[1])),
        );
    }
    if let Some(m) = regex(r"^T(\d+) fechada → (merge → )?integração verde$").captures(s) {
        return (
            json!({"code":"integrated","merge":m.get(2).is_some()}),
            integer(&json!(&m[1])),
        );
    }
    if let Some(m) = regex(r"^T(\d+) entregou a rodada (\d+)(?: · ([0-9a-f]+))?$").captures(s) {
        return (
            json!({"code":"delivered","round":integer(&json!(&m[2])),"commit":m.get(3).map(|m|m.as_str())}),
            integer(&json!(&m[1])),
        );
    }
    if let Some(m) =
        regex(r"^T(\d+): integração vermelha → devolvida ao executor (\S+)$").captures(s)
    {
        return (
            json!({"code":"red_back","executor":&m[2]}),
            integer(&json!(&m[1])),
        );
    }
    if let Some(m) = regex(r"^T(\d+): integração vermelha, rodando de novo uma vez$").captures(s)
    {
        return (json!({"code":"red_retry"}), integer(&json!(&m[1])));
    }
    (Value::Null, Value::Null)
}
fn sentence_bounds(s: &str) -> (Vec<usize>, Vec<usize>) {
    let chars: Vec<_> = s.char_indices().collect();
    let mut starts = vec![0];
    let mut ends = Vec::new();
    for (i, (at, c)) in chars.iter().enumerate() {
        if !c.is_whitespace() || i == 0 || chars[i - 1].1.is_whitespace() {
            continue;
        }
        let before = chars[i - 1].1;
        if !matches!(before, '.' | '!' | '?') || (before == '?' && i >= 2 && chars[i - 2].1 == '?')
        {
            continue;
        }
        let end = chars
            .iter()
            .skip(i)
            .find(|(_, c)| !c.is_whitespace())
            .map(|(a, _)| *a);
        if let Some(end) = end {
            let ch = s[end..].chars().next().unwrap();
            if ch.is_ascii_uppercase() || ('À'..='Ý').contains(&ch) || "[`'\"(".contains(ch) {
                ends.push(*at);
                starts.push(end)
            }
        }
    }
    ends.push(s.len());
    (starts, ends)
}
pub fn first_sentence(body: &str) -> String {
    let body = body.replace('`', "");
    let body = body.trim();
    let (_, ends) = sentence_bounds(body);
    let first = body[..ends[0]].trim();
    if first.chars().count() <= 160 {
        first.into()
    } else {
        format!(
            "{}…",
            first.chars().take(159).collect::<String>().trim_end()
        )
    }
}
fn split_question(s: &str) -> (String, Option<String>) {
    let (starts, ends) = sentence_bounds(s);
    for (start, end) in starts.iter().zip(ends).rev() {
        let part = &s[*start..end];
        if part.ends_with('?') && !part.ends_with("??") {
            return (
                format!("{} {}", s[..*start].trim_end(), s[end..].trim_start())
                    .trim()
                    .into(),
                Some(part.into()),
            );
        }
    }
    (s.into(), None)
}
fn code(s: &str) -> String {
    let rx = regex(
        r#"(?:~?/)?(?:[\w.-]+/)*[\w.-]+\.(?:tsx?|svelte|py|rs|md|jsonl?|jsx?|kt|swift|sh|ps1)(?::\d+)?|[A-Za-z_]\w*(?:\.[A-Za-z_]\w*)+(?:\([^()\s]*\))?|[a-z]+[A-Z]\w*(?:\([^()\s]*\))?"#,
    );
    let mut out = String::new();
    let mut last = 0;
    for m in rx.find_iter(s) {
        let before = s[..m.start()].chars().next_back();
        let after = s[m.end()..].chars().next();
        let good_before = before.is_none_or(|c| c.is_whitespace() || "([\"'".contains(c));
        let good_after = after.is_some_and(|c| c.is_whitespace() || ",;:.)]\"'(".contains(c));
        if good_before && good_after {
            out.push_str(&s[last..m.start()]);
            out.push('`');
            out.push_str(m.as_str());
            out.push('`');
            last = m.end();
        }
    }
    out.push_str(&s[last..]);
    out
}
pub fn event_id(obj: &Value) -> String {
    let dumped = crate::transcript::pyjson::dumps(obj, true);
    format!(
        "orq:{}",
        &sha1_smol::Sha1::from(dumped.as_bytes())
            .digest()
            .to_string()[..16]
    )
}
pub fn entry(obj: &Value, dir: Option<&Path>) -> Option<Value> {
    let raw = obj["text"].as_str().filter(|s| !s.trim().is_empty())?;
    let kind = if matches!(
        text(&obj["kind"]),
        "advance" | "woke" | "would_drop" | "dropped" | "failed" | "notice"
    ) {
        text(&obj["kind"])
    } else {
        "notice"
    };
    let obj_task = integer(&obj["task"]);
    let mut result = json!({"kind":kind,"task":obj_task,"line":null,"origin":null,"sender":null,"mark":null,"alarm":false,"rejected_round":null,"body":raw,"question":null,"parecer":null,"error":null,"decided_by":null});
    if matches!(kind, "advance" | "notice") {
        let (line, from) = advance(raw);
        result["line"] = line;
        if result["task"].is_null() {
            result["task"] = if from.is_null() { task(raw) } else { from }
        }
        return Some(result);
    }
    let mut why = None;
    let mut err = None;
    let mut msg = raw.to_owned();
    let mut woke_format = false;
    if let Some(m)=regex(r"(?s)^(?:teria descartado|recado registrado sem acordar o árbitro) \((jev|regex: [^)]*)\)(?:; acordou o árbitro)?: (.*)$").captures(raw){why=Some(m[1].to_owned());msg=m[2].to_owned()}else if let Some(m)=regex(r"(?s)^recado ao árbitro não entregue \(((?:[^()]|\([^()]*\))*)\): (.*)$").captures(raw){err=Some(m[1].to_owned());msg=m[2].to_owned()}else if let Some(rest)=raw.strip_prefix("acordou o árbitro: "){msg=rest.into();woke_format=true}
    let outside = (kind == "failed" && err.is_none()) || (kind == "woke" && !woke_format);
    let origin = if obj.get("from").is_some() {
        "notify"
    } else if (kind == "woke" && !obj_task.is_null())
        || msg.starts_with("lote de prova pronto")
        || msg.starts_with("todas as Tasks integradas")
        || outside
    {
        "orchestrator"
    } else {
        "notify"
    };
    let mut sender = obj["from"].as_str().map(str::to_owned);
    let mut body = msg.trim().to_owned();
    let mut mark = None;
    if let Some(m) = regex(r"(?i)^\s*\[(aviso|decis[aã]o)\]\s*").find(&body) {
        if body[m.start()..m.end()].to_lowercase().contains("decis") {
            mark = Some("decisao")
        }
        body = body[m.end()..].into()
    }
    let alarm = regex(r"(?i)^\s*\[vigia\]\s*").is_match(&body);
    if alarm {
        body = regex(r"(?i)^\s*\[vigia\]\s*")
            .replace(&body, "")
            .into_owned();
        if sender.is_none() {
            sender = Some("vigia".into())
        }
    }
    let mut n = obj_task;
    let mut rejected = Value::Null;
    if let Some(m) = regex(r"^T(\d+)(?:\s+R(\d+)\s+reprovad[ao])?\s*:\s*").captures(&body) {
        if n.is_null() {
            n = integer(&json!(&m[1]))
        }
        rejected = m
            .get(2)
            .map(|s| integer(&json!(s.as_str())))
            .unwrap_or_default();
        body = body[m.get(0).unwrap().end()..].into()
    }
    let mut report = None;
    if let Some(m) = regex(r"\s*Parecer:\s*(/\S+?)[.,;)]*(?:\s|$)(?:\s*\([^)]*\))?").captures(&body)
    {
        report = Some(m[1].to_owned());
        let m = m.get(0).unwrap();
        let mut end = m.end();
        if let Some(last) = body[..end]
            .chars()
            .next_back()
            .filter(|c| c.is_whitespace())
        {
            end -= last.len_utf8();
        }
        body = format!("{}{}", &body[..m.start()], &body[end..]);
    }
    let (body, question) = split_question(body.trim());
    if n.is_null() {
        n = task(raw)
    }
    let mut decided = Value::Null;
    if origin == "orchestrator" && kind != "failed" {
        decided = json!({"source":"rule","rule":"orchestrator","jev":null,"regex":null,"regex_agreed":null})
    } else if origin == "notify" && kind != "failed" {
        if mark.is_some() {
            decided =
                json!({"source":"rule","rule":"mark","jev":null,"regex":null,"regex_agreed":null})
        } else if alarm {
            decided =
                json!({"source":"alarm","rule":null,"jev":null,"regex":null,"regex_agreed":null})
        } else {
            let jev = dir
                .map(|d| jev_for(d, &msg, &obj["ts"]))
                .unwrap_or_default();
            let usable = !jev.is_null() && !truthy(&jev["error"]);
            let source = if why.as_deref().is_some_and(|s| s.starts_with("regex"))
                || (kind == "woke" && !usable)
            {
                "regex"
            } else {
                "jev"
            };
            let rx = dir.map(|_| {
                let (verdict, category) = super::triage::decide(&msg);
                json!({"verdict":verdict,"category":category})
            });
            let agreed = rx
                .as_ref()
                .filter(|_| usable)
                .map(|r| (r["verdict"] == "drop") == truthy(&jev["would_drop"]));
            decided =
                json!({"source":source,"rule":null,"jev":jev,"regex":rx,"regex_agreed":agreed})
        }
    }
    result["task"] = n;
    result["origin"] = json!(origin);
    result["sender"] = json!(sender);
    result["mark"] = json!(mark);
    result["alarm"] = json!(alarm);
    result["rejected_round"] = rejected;
    result["body"] = json!(code(&body));
    result["question"] = json!(question.map(|s| code(&s)));
    result["parecer"] = json!(report);
    result["error"] = json!(err);
    result["decided_by"] = decided;
    Some(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_timeline_contract() {
        let fixture: Value = serde_json::from_str(include_str!("fixtures/timeline.json")).unwrap();
        for case in array(&fixture["cases"]) {
            let tmp = tempfile::tempdir().unwrap();
            let run = tmp.path().join(text(&case["run"]));
            fs::create_dir(&run).unwrap();
            fs::write(run.join("jev-shadow.jsonl"), text(&case["shadow"])).unwrap();
            for e in array(&case["entries"]) {
                assert_eq!(
                    entry(&e["input"], Some(&run)),
                    Some(e["output"].clone()),
                    "{}",
                    e["input"]
                );
                assert_eq!(event_id(&e["input"]), e["id"]);
            }
        }
    }
}

pub fn parse_obj(obj: &Value, dir: Option<&Path>) -> Vec<hangar_api::chat::ChatEvent> {
    let Some(content) = obj["text"].as_str().filter(|s| !s.trim().is_empty()) else {
        return Vec::new();
    };
    let enriched = entry(obj, dir).and_then(|v| v.as_object().cloned());
    vec![hangar_api::chat::ChatEvent {
        kind: hangar_api::chat::ChatKind::Notice,
        id: event_id(obj),
        text: Some(content.into()),
        ts: runs::when(&obj["ts"]).map(|t| t.timestamp_micros() as f64 / 1e6),
        orq: enriched,
        ..Default::default()
    }]
}
pub use super::panel::panel;
