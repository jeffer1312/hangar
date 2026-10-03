// crates/hangar-server/src/transcript/claude.rs
//! Porte de `parse_obj`, `RewriteFilter` e `silent_attachment_timestamp` (backend/app/transcript.py).
//! Campo com tipo errado, que no Python levanta exceção e derruba a leitura, aqui conta como ausente.

use std::collections::HashMap;
use std::sync::LazyLock;

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value};

use super::py::{self, lstrip, py_re, strip};
use super::{event, finish, pyjson};

// transcript.py:40-45
static IMAGE_SOURCE: LazyLock<Regex> = LazyLock::new(|| py_re(r"\A\[Image(?:\]|: [^\]]*\])\z"));
static IMAGE_MARKER: LazyLock<Regex> = LazyLock::new(|| py_re(r"\[Image #\d+\]\s*"));
static INTERRUPTED: LazyLock<Regex> = LazyLock::new(|| py_re(r"\A\[Request interrupted by user[^\]]*\]\z"));
// transcript.py:76, 89, 95, 102-103
static META_BLOCK: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)<system-reminder>.*?</system-reminder>"));
static TASK_NOTIF: LazyLock<Regex> = LazyLock::new(|| py_re(r"<task-id>([^<]+)</task-id>"));
static AGENT_MSG: LazyLock<Regex> =
    LazyLock::new(|| py_re(r#"(?s)\A<agent-message from="([^"]+)"[^>]*>(.*)</agent-message>\z"#));
static TEAMMATE_START: LazyLock<Regex> =
    LazyLock::new(|| py_re(r"\A(?:Another Claude session sent a message:\s*)?<teammate-message\b"));
static TEAMMATE_BLOCK: LazyLock<Regex> =
    LazyLock::new(|| py_re(r"(?s)<teammate-message\b([^>]*)>\n?(.*?)\n?</teammate-message>"));
// transcript.py:163-166
static PEER_WRAP: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"(?s)\A<cross-session-message\b([^>]*)>\n?(.*?)\n?</cross-session-message>\z")
});
static PEER_ATTR: LazyLock<Regex> = LazyLock::new(|| py_re(r#"([\w-]+)="([^"]*)""#));
static SOCK_PID: LazyLock<Regex> = LazyLock::new(|| py_re(r"/(\d+)\.sock"));
static PEER_PREFIX: LazyLock<Regex> = LazyLock::new(|| py_re(r"\A\[(de|grupo|painel):\s*[^\]]+\]"));
// transcript.py:263
static ORIGINAL_PROMPT: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r"(?s)UserPromptSubmit operation blocked by hook:\s*(.*?)\s*Original prompt: (.+)$")
});
// transcript.py:375-378
static ATTACHMENT_HEAD: LazyLock<Regex> = LazyLock::new(|| {
    py_re(r#"\A\{"parentUuid":(?:null|"[^"\\]*"),"isSidechain":(?:true|false),"attachment":\{"type":"([^"\\]*)""#)
});
const ATTACHMENT_TAIL: &str = r#"},"type":"attachment","uuid":""#;
static ATTACHMENT_TAIL_RE: LazyLock<Regex> =
    LazyLock::new(|| py_re(r#"\A\},"type":"attachment","uuid":"[^"\\]*","timestamp":"([^"\\]*)""#));
// transcript.py:57-72
const COMMAND_META_PREFIXES: [&str; 12] = [
    "<command-name>", "<command-message>", "<command-args>", "<local-command-caveat>",
    "<local-command-stdout>", "<local-command-stderr>", "<bash-input>", "<bash-stdout>", "<bash-stderr>",
    "Base directory for this skill:", "<task-notification>", "<system-reminder>",
];

const WINDOW_S: f64 = 60.0;

/// `RewriteFilter` (transcript.py:315): descarta o que o `claude --resume` regravou.
#[derive(Default)]
pub(crate) struct RewriteFilter {
    max: f64,
    recent: HashMap<(String, String), f64>,
}

impl RewriteFilter {
    pub(crate) fn keep(&mut self, obj: &Map<String, Value>) -> bool {
        if !matches!(obj.get("type").and_then(Value::as_str), Some("user" | "assistant")) {
            return true;
        }
        let Some(t) = obj.get("timestamp").and_then(Value::as_str) else { return true };
        let Some(ts) = py::iso_timestamp(&t.replace('Z', "+00:00")) else { return true };
        if ts < self.max - WINDOW_S {
            return false;
        }
        let content = obj.get("message").and_then(Value::as_object).and_then(|m| m.get("content"));
        // ensure_ascii=True no lugar do False do Python: a igualdade entre duas linhas é a mesma, e o
        // surrogate solto que derruba o md5 do Python aqui passa.
        let fp = (t.to_string(), py::md5_hex(&pyjson::dumps(content.unwrap_or(&Value::Null), true)));
        if self.recent.contains_key(&fp) {
            return false;
        }
        self.recent.insert(fp, ts);
        if ts > self.max {
            self.max = ts;
            if self.recent.len() > 256 {
                self.recent.retain(|_, v| *v >= ts - WINDOW_S);
            }
        }
        true
    }
}

/// `silent_attachment_timestamp` (transcript.py:381): relógio de um anexo que nunca vira bolha,
/// lido sem json.
pub(crate) fn silent_attachment_timestamp(line: &str) -> Option<&str> {
    if !line.ends_with("}\n") {
        return None;
    }
    let head = ATTACHMENT_HEAD.captures(line)?;
    if matches!(&head[1], "queued_command" | "hook_additional_context") {
        return None;
    }
    let i = line.rfind(ATTACHMENT_TAIL)?;
    Some(ATTACHMENT_TAIL_RE.captures(&line[i..])?.get(1)?.as_str())
}

/// `parse_obj` (transcript.py:397), já com o `scrub_surrogates` do `ChatEvent`.
pub(crate) fn parse_obj(obj: &Map<String, Value>, resolve: PeerResolver) -> Vec<ChatEvent> {
    let mut out = parse_raw(obj, resolve);
    out.iter_mut().for_each(finish);
    out
}

fn parse_raw(obj: &Map<String, Value>, resolve: PeerResolver) -> Vec<ChatEvent> {
    let etype = obj.get("type").and_then(Value::as_str);
    let uid = obj.get("uuid").and_then(Value::as_str).unwrap_or("");
    match etype {
        Some("system") => return system(obj, resolve),
        Some("queue-operation") => return queue_operation(obj, resolve),
        Some("attachment") => return attachment(obj, uid),
        _ => {}
    }
    let Some(msg) = obj.get("message").and_then(Value::as_object) else { return Vec::new() };
    let content = msg.get("content");
    match (etype, content) {
        (Some("user"), _) => user(obj, uid, content, resolve),
        (Some("assistant"), Some(Value::Array(items))) => assistant(obj, msg, uid, items),
        _ => Vec::new(),
    }
}

fn ts(obj: &Map<String, Value>) -> Option<f64> {
    py::ts_of_iso(obj.get("timestamp")?.as_str()?)
}

fn md5_8(s: &str) -> String {
    let mut h = py::md5_hex(s);
    h.truncate(8);
    h
}

fn sub_id(uid: &str, k: usize) -> String {
    if k == 0 { uid.to_string() } else { format!("{uid}:{k}") }
}

fn text_event(kind: ChatKind, id: String, text: String) -> ChatEvent {
    ChatEvent { text: Some(text), ..event(kind, id) }
}

fn task_result(id: String, task: &str) -> ChatEvent {
    ChatEvent {
        tool_use_id: Some(format!("task:{task}")),
        result: Some("task-notification".into()),
        ..event(ChatKind::ToolResult, id)
    }
}

fn first<'a>(items: &'a [Value], type_name: &str) -> Option<&'a Map<String, Value>> {
    items
        .iter()
        .filter_map(Value::as_object)
        .find(|it| it.get("type").and_then(Value::as_str) == Some(type_name))
}

fn is_command_meta(text: &str) -> bool {
    let t = lstrip(text);
    COMMAND_META_PREFIXES.iter().any(|p| t.starts_with(p))
}

/// `_strip_meta_blocks` (transcript.py:300).
fn strip_meta_blocks(text: &str) -> String {
    strip(&strip_pasted(&META_BLOCK.replace_all(text, ""))).to_string()
}

/// `_PASTED_RE.sub(r"\2", …)` (transcript.py:83). O `regex` não tem retrorreferência, então o
/// fechamento com o mesmo id é procurado à mão: o primeiro depois da abertura, sem um "\n" de cada
/// lado do conteúdo.
fn strip_pasted(text: &str) -> String {
    const OPEN: &str = "<pasted_content id=\"";
    let mut out = String::with_capacity(text.len());
    let (mut copied, mut search) = (0, 0);
    while let Some(rel) = text[search..].find(OPEN) {
        let at = search + rel;
        let id_start = at + OPEN.len();
        let Some(id_len) = text[id_start..].find('"') else { break };
        let after = id_start + id_len + 1;
        if !text[after..].starts_with('>') {
            search = at + 1;
            continue;
        }
        let id = &text[id_start..id_start + id_len];
        let mut body = after + 1;
        if text[body..].starts_with('\n') {
            body += 1;
        }
        let close = format!("</pasted_content id=\"{id}\">");
        let Some(rel_close) = text[body..].find(&close) else {
            search = at + 1;
            continue;
        };
        let end = body + rel_close + close.len();
        let mut body_end = body + rel_close;
        if body_end > body && text[..body_end].ends_with('\n') {
            body_end -= 1;
        }
        out.push_str(&text[copied..at]);
        out.push_str(&text[body..body_end]);
        copied = end;
        search = end;
    }
    out.push_str(&text[copied..]);
    out
}

fn attrs(raw: &str) -> HashMap<&str, &str> {
    PEER_ATTR.captures_iter(raw).map(|c| (c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str())).collect()
}

/// `_teammate_textos` (transcript.py:106).
fn teammate_texts(text: Option<&Value>) -> Option<Vec<String>> {
    let t = lstrip(text?.as_str()?);
    if !TEAMMATE_START.is_match(t) {
        return None;
    }
    let blocks: Vec<_> = TEAMMATE_BLOCK.captures_iter(t).collect();
    if blocks.is_empty() {
        return None;
    }
    let mut out = Vec::new();
    for b in blocks {
        let body = strip(&b[2]);
        if body.starts_with('{') && matches!(pyjson::loads_lossless(body), Some(Value::Object(_))) {
            continue;
        }
        let name = attrs(&b[1]).get("teammate_id").copied().filter(|n| !n.is_empty()).unwrap_or("colega");
        if !body.is_empty() {
            out.push(format!("[de: {name}] {body}"));
        }
    }
    Some(out)
}

fn teammate_events(text: Option<&Value>, id: &str) -> Option<Vec<ChatEvent>> {
    let texts = teammate_texts(text)?;
    Some(texts.into_iter().enumerate().map(|(k, t)| text_event(ChatKind::UserMsg, sub_id(id, k), t)).collect())
}

/// `_agent_msg` (transcript.py:138).
fn agent_msg(text: Option<&Value>, id: &str) -> Option<Vec<ChatEvent>> {
    let m = AGENT_MSG.captures(strip(text?.as_str()?))?;
    let body = &m[2];
    if body.contains("<agent-message") || body.contains("</agent-message>") {
        return None;
    }
    if !body.contains("[Subagent hand-back]") {
        return Some(Vec::new());
    }
    Some(vec![task_result(id.to_string(), &m[1])])
}

/// Resolve o pid do remetente no nome da sessão tmux (`peer::name_of_pid`; nos testes, um fake).
pub(crate) type PeerResolver = fn(i64) -> Option<String>;

/// `_peer_nome` (transcript.py:170): o nome tmux do pid; sem ele, o título que veio no recado.
fn peer_name(pid: Option<i64>, fallback: Option<&str>, resolve: PeerResolver) -> String {
    if let Some(name) = pid.and_then(resolve) {
        return name;
    }
    match fallback.map(strip) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => "sessão".to_string(),
    }
}

/// `_peer_msg` (transcript.py:194).
fn peer_msg(obj: &Map<String, Value>, resolve: PeerResolver) -> Option<String> {
    let origin = obj.get("origin")?.as_object()?;
    if origin.get("kind").and_then(Value::as_str) != Some("peer") {
        return None;
    }
    let body = origin.get("body")?.as_str()?;
    if strip(body).is_empty() {
        return None;
    }
    if PEER_PREFIX.is_match(lstrip(body)) {
        return Some(strip(body).to_string());
    }
    let pid = origin.get("verifiedPeerPid").and_then(py::int_of).and_then(|p| i64::try_from(p).ok());
    let name = peer_name(pid, origin.get("name").and_then(Value::as_str), resolve);
    Some(format!("[de: {name}] {}", strip(body)))
}

/// `_peer_msg_embrulhado` (transcript.py:218).
fn wrapped_peer_msg(text: &str, resolve: PeerResolver) -> Option<String> {
    let t = strip(text);
    if !t.starts_with("<cross-session-message") || !t.ends_with("</cross-session-message>") {
        return None;
    }
    let m = PEER_WRAP.captures(t)?;
    let body = strip(&m[2]);
    if body.is_empty() || body.contains("<cross-session-message") || body.contains("</cross-session-message>") {
        return None;
    }
    if PEER_PREFIX.is_match(body) {
        return Some(body.to_string());
    }
    let attrs = attrs(&m[1]);
    let pid = SOCK_PID.captures(attrs.get("from").copied().unwrap_or("")).and_then(|c| c[1].parse().ok());
    Some(format!("[de: {}] {body}", peer_name(pid, attrs.get("from-name").copied(), resolve)))
}

/// `_blocked_prompt` (transcript.py:267). O aviso de "parece recado" do Python fica de fora: ele
/// levaria texto da conversa ao log.
fn blocked_prompt(content: Option<&Value>, resolve: PeerResolver) -> Option<(String, String)> {
    let m = ORIGINAL_PROMPT.captures(content?.as_str()?)?;
    let text = strip(&m[2]);
    if text.is_empty() {
        return None;
    }
    Some((wrapped_peer_msg(text, resolve).unwrap_or_else(|| text.to_string()), m[1].to_string()))
}

fn system(obj: &Map<String, Value>, resolve: PeerResolver) -> Vec<ChatEvent> {
    let Some((text, error)) = blocked_prompt(obj.get("content"), resolve) else { return Vec::new() };
    vec![ChatEvent {
        ts: ts(obj),
        desistiu: Some(true),
        hook_error: (!error.is_empty()).then_some(error),
        ..text_event(ChatKind::UserMsg, format!("held:{}", md5_8(&text)), text)
    }]
}

fn queue_operation(obj: &Map<String, Value>, resolve: PeerResolver) -> Vec<ChatEvent> {
    let Some(q) = obj.get("content").and_then(Value::as_str) else { return Vec::new() };
    let queued = obj.get("content");
    let h = md5_8(q);
    if let Some(a) = agent_msg(queued, &format!("queued-agent:{h}")) {
        return a;
    }
    if let Some(c) = teammate_events(queued, &format!("queued-teammate:{h}")) {
        return c;
    }
    if lstrip(q).starts_with("<task-notification>") {
        return match TASK_NOTIF.captures(q) {
            Some(m) => {
                let tid = strip(&m[1]);
                vec![task_result(format!("queued-task:{tid}"), tid)]
            }
            None => Vec::new(),
        };
    }
    if obj.get("operation").and_then(Value::as_str) != Some("remove") {
        return Vec::new();
    }
    let id = format!("queued:{}:{h}", obj.get("timestamp").map_or_else(String::new, py::py_str));
    if let Some(peer) = wrapped_peer_msg(q, resolve) {
        return vec![text_event(ChatKind::UserMsg, id, peer)];
    }
    if is_command_meta(q) {
        return Vec::new();
    }
    let cleaned = strip_meta_blocks(q);
    if cleaned.is_empty() || IMAGE_SOURCE.is_match(&cleaned) {
        return Vec::new();
    }
    let cleaned = strip(&IMAGE_MARKER.replace_all(&cleaned, "")).to_string();
    if cleaned.is_empty() {
        return Vec::new();
    }
    vec![text_event(ChatKind::UserMsg, id, cleaned)]
}

fn attachment(obj: &Map<String, Value>, uid: &str) -> Vec<ChatEvent> {
    let Some(att) = obj.get("attachment").and_then(Value::as_object) else { return Vec::new() };
    let atype = att.get("type").and_then(Value::as_str);
    if atype == Some("queued_command") {
        let parts: Vec<&str> = match att.get("prompt") {
            Some(Value::Array(blocks)) => blocks
                .iter()
                .filter_map(Value::as_object)
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .map(|b| b.get("text").and_then(Value::as_str).unwrap_or(""))
                .collect(),
            _ => Vec::new(),
        };
        let joined = parts.join("\n");
        let text = strip(&joined);
        if text.is_empty() || is_command_meta(text) {
            return Vec::new();
        }
        let text = strip_meta_blocks(text);
        if !text.is_empty() {
            return vec![ChatEvent { ts: ts(obj), ..text_event(ChatKind::UserMsg, uid.into(), text) }];
        }
    }
    if atype == Some("hook_additional_context") && att.get("hookEvent").and_then(Value::as_str) == Some("Stop") {
        let content = att.get("content");
        let text = match content {
            Some(Value::Array(items)) => items.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("\n"),
            Some(v) if py::truthy(content) => py::py_str(v),
            _ => String::new(),
        };
        let t = strip(&text);
        if !t.is_empty() {
            return vec![ChatEvent {
                hook_error: Some(t.to_string()),
                ts: ts(obj),
                ..text_event(ChatKind::Notice, uid.into(), "hook_prompt".into())
            }];
        }
    }
    Vec::new()
}

fn user(obj: &Map<String, Value>, uid: &str, content: Option<&Value>, resolve: PeerResolver) -> Vec<ChatEvent> {
    if let Some(origin) = obj.get("origin").and_then(Value::as_object) {
        if let Some(c) = teammate_events(origin.get("body"), uid) {
            return c;
        }
    }
    if let Some(peer) = peer_msg(obj, resolve) {
        return vec![text_event(ChatKind::UserMsg, uid.into(), peer)];
    }
    if obj.get("isCompactSummary") == Some(&Value::Bool(true)) {
        return vec![text_event(ChatKind::Notice, uid.into(), "compacted".into())];
    }
    if obj.get("isMeta") == Some(&Value::Bool(true)) {
        return Vec::new();
    }
    let first_text = match content {
        Some(Value::String(_)) => content,
        Some(Value::Array(items)) => first(items, "text").and_then(|b| b.get("text")),
        _ => None,
    };
    if let Some(a) = agent_msg(first_text, uid) {
        return a;
    }
    if let Some(c) = teammate_events(first_text, uid) {
        return c;
    }
    match content {
        Some(Value::String(s)) => user_text(uid, s),
        Some(Value::Array(items)) => user_blocks(obj, uid, items),
        _ => Vec::new(),
    }
}

fn user_text(uid: &str, s: &str) -> Vec<ChatEvent> {
    if lstrip(s).starts_with("<task-notification>") {
        return match TASK_NOTIF.captures(s) {
            Some(m) => vec![task_result(uid.into(), strip(&m[1]))],
            None => Vec::new(),
        };
    }
    if is_command_meta(s) {
        return Vec::new();
    }
    if INTERRUPTED.is_match(strip(s)) {
        return vec![text_event(ChatKind::Notice, uid.into(), "interrupted".into())];
    }
    let cleaned = strip_meta_blocks(s);
    if cleaned.is_empty() || IMAGE_SOURCE.is_match(&cleaned) {
        return Vec::new();
    }
    vec![text_event(ChatKind::UserMsg, uid.into(), cleaned)]
}

fn user_blocks(obj: &Map<String, Value>, uid: &str, items: &[Value]) -> Vec<ChatEvent> {
    let is_type = |it: &&Map<String, Value>, t: &str| it.get("type").and_then(Value::as_str) == Some(t);
    let trs: Vec<_> = items.iter().filter_map(Value::as_object).filter(|it| is_type(it, "tool_result")).collect();
    if !trs.is_empty() {
        let ts = ts(obj);
        return trs
            .into_iter()
            .enumerate()
            .map(|(k, tr)| {
                let result = match tr.get("content") {
                    Some(Value::Array(blocks)) => Some(
                        blocks
                            .iter()
                            .filter_map(Value::as_object)
                            .map(|b| b.get("text").map_or_else(String::new, py::py_str))
                            .collect::<Vec<_>>()
                            .join(" "),
                    ),
                    None | Some(Value::Null) => None,
                    Some(other) => Some(py::py_str(other)),
                };
                ChatEvent {
                    tool_use_id: tr.get("tool_use_id").and_then(Value::as_str).map(str::to_string),
                    result,
                    is_error: Some(py::truthy(tr.get("is_error"))),
                    ts,
                    ..event(ChatKind::ToolResult, sub_id(uid, k))
                }
            })
            .collect();
    }
    let img_count = items.iter().filter_map(Value::as_object).filter(|it| is_type(it, "image")).count();
    let t = first(items, "text").and_then(|b| b.get("text")).and_then(Value::as_str).unwrap_or("");
    if is_command_meta(t) {
        return Vec::new();
    }
    if INTERRUPTED.is_match(strip(t)) {
        return vec![text_event(ChatKind::Notice, uid.into(), "interrupted".into())];
    }
    let cleaned = strip_meta_blocks(t);
    if IMAGE_SOURCE.is_match(&cleaned) {
        return Vec::new();
    }
    let cleaned = strip(&IMAGE_MARKER.replace_all(&cleaned, "")).to_string();
    if cleaned.is_empty() && img_count == 0 {
        return Vec::new();
    }
    vec![ChatEvent {
        image_count: (img_count > 0).then(|| img_count.try_into().ok()).flatten(),
        ..text_event(ChatKind::UserMsg, uid.into(), cleaned)
    }]
}

/// `_cache_info` (transcript.py:633).
fn cache_info(msg: &Map<String, Value>) -> (Option<u64>, Option<u64>) {
    let Some(usage) = msg.get("usage").and_then(Value::as_object) else { return (None, None) };
    let read = usage.get("cache_read_input_tokens").and_then(py::int_trunc).and_then(|n| u64::try_from(n).ok());
    let creation = usage.get("cache_creation").and_then(Value::as_object);
    // `_tok`: bool não conta como número aqui.
    let tok = |k: &str| creation.and_then(|c| c.get(k)).filter(|v| !v.is_boolean()).and_then(py::int_trunc).unwrap_or(0);
    let ttl = if tok("ephemeral_1h_input_tokens") > 0 {
        Some(3600)
    } else if tok("ephemeral_5m_input_tokens") > 0 {
        Some(300)
    } else {
        None
    };
    (read, ttl)
}

fn assistant(obj: &Map<String, Value>, msg: &Map<String, Value>, uid: &str, items: &[Value]) -> Vec<ChatEvent> {
    let (cache_read, cache_ttl_s) = cache_info(msg);
    let ts = ts(obj);
    let mut out = Vec::new();
    for it in items.iter().filter_map(Value::as_object) {
        let id = sub_id(uid, out.len());
        match it.get("type").and_then(Value::as_str) {
            Some("tool_use") => out.push(ChatEvent {
                tool_name: it.get("name").and_then(Value::as_str).map(str::to_string),
                tool_use_id: it.get("id").and_then(Value::as_str).map(str::to_string),
                tool_input: Some(match it.get("input") {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Map::new(),
                }),
                ts,
                ..event(ChatKind::ToolUse, id)
            }),
            Some("text") => out.push(ChatEvent {
                // `it.get("text", "")`: ausente vira "", null fica null.
                text: match it.get("text") {
                    None => Some(String::new()),
                    Some(v) => v.as_str().map(str::to_string),
                },
                ts,
                cache_read,
                cache_ttl_s,
                ..event(ChatKind::AssistantMsg, id)
            }),
            Some("thinking") => {
                if let Some(p) = it.get("thinking").and_then(Value::as_str).filter(|p| !strip(p).is_empty()) {
                    out.push(ChatEvent { ts, ..text_event(ChatKind::Thinking, id, p.to_string()) });
                }
            }
            _ => {}
        }
    }
    out
}
