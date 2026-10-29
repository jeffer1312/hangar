//! Fim das conversas para sugerir tarefa: os parsers já migrados e os registros Pi/omp/Kimi.
use std::{
    collections::HashMap,
    fs::File,
    io::{self, BufRead, BufReader, Seek, SeekFrom},
    path::Path,
    sync::LazyLock,
};

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::Value;

use crate::transcript::{InternalInfo, TAIL_WINDOW, history, py, pyjson};

type Item = (f64, u64, ChatEvent);
static ANSI: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;:?]*[ -/]*[@-~]").unwrap());

pub fn read(info: &InternalInfo, limit: usize) -> io::Result<Vec<ChatEvent>> {
    if let Some(request) = info.history_request(Some(limit)) {
        return crate::transcript::merged_history(&request);
    }
    if !matches!(info.provider.as_str(), "pi" | "omp" | "kimi") {
        return Ok(Vec::new());
    }
    let Some(path) = info.jsonl.as_deref() else {
        return Ok(Vec::new());
    };
    let mut window = TAIL_WINDOW;
    let (mut items, committed, previous, start) = loop {
        let offset = tail_offset(path, window)?;
        let parsed = parse(path, &info.provider, offset)?;
        if offset == 0 || parsed.0.len() >= limit {
            break parsed;
        }
        window = window.saturating_mul(4);
    };
    if let Some(queue) = info.history.get("queue").and_then(Value::as_str) {
        for entry in load_queue(Path::new(queue))? {
            let Some(text) = entry
                .get("text")
                .and_then(Value::as_str)
                .map(py::strip)
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let ts = match entry.get("ts") {
                Some(Value::String(value)) if !value.is_empty() => {
                    py::strip(value).parse().unwrap_or(previous)
                }
                value if py::truthy(value) => value.and_then(py::number).unwrap_or(previous),
                _ => previous,
            };
            if history::is_local_output(&entry) {
                if start == 0. || history::from_current_session(&entry, start, ts) {
                    items.push((ts, 1_000_000_000, history::entry_event(&entry)));
                }
                continue;
            }
            let caption = history::strip_attach(text);
            let committed_at = committed
                .get(text)
                .copied()
                .unwrap_or(-1.)
                .max(committed.get(py::strip(&caption)).copied().unwrap_or(-1.));
            if py::truthy(entry.get("confirmed"))
                || committed_at >= ts
                || start != 0. && !history::from_current_session(&entry, start, ts)
            {
                continue;
            }
            items.push((ts, 1_000_000_000, history::entry_event(&entry)));
        }
    }
    items.sort_by(|a, b| {
        a.0.partial_cmp(&b.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.1.cmp(&b.1))
    });
    let skip = items.len().saturating_sub(limit);
    Ok(items
        .into_iter()
        .skip(skip)
        .map(|(_, _, event)| event)
        .collect())
}

fn load_queue(path: &Path) -> io::Result<Vec<serde_json::Map<String, Value>>> {
    let bytes = match std::fs::read(path) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(py::splitlines(&String::from_utf8_lossy(&bytes))
        .into_iter()
        .filter_map(|line| pyjson::loads_lossless(py::strip(line)))
        .filter_map(|value| value.as_object().cloned())
        .collect())
}

fn tail_offset(path: &Path, window: u64) -> io::Result<u64> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e),
    };
    let size = file.metadata()?.len();
    if size <= window {
        return Ok(0);
    }
    let offset = size - window;
    file.seek(SeekFrom::Start(offset))?;
    let n = BufReader::new(file).read_until(b'\n', &mut Vec::new())?;
    Ok(offset + n as u64)
}

fn timestamp(value: &Value) -> f64 {
    if let Some(timestamp) = value["timestamp"].as_str() {
        return py::ts_of_iso(timestamp).unwrap_or(0.);
    }
    [
        &value["message"]["timestamp"],
        &value["time"],
        &value["created_at"],
    ]
    .into_iter()
    .find_map(py::number)
    .unwrap_or(0.)
        / 1000.
}

fn first_timestamp(path: &Path) -> io::Result<f64> {
    for line in BufReader::new(File::open(path)?).split(b'\n') {
        let line = line?;
        if let Some(value) = pyjson::loads_lossless(&String::from_utf8_lossy(&line)) {
            let timestamp = timestamp(&value);
            if timestamp > 0. {
                return Ok(timestamp);
            }
        }
    }
    Ok(0.)
}

fn parse(
    path: &Path,
    provider: &str,
    offset: u64,
) -> io::Result<(Vec<Item>, HashMap<String, f64>, f64, f64)> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok((Vec::new(), HashMap::new(), 0., 0.));
        }
        Err(e) => return Err(e),
    };
    file.seek(SeekFrom::Start(offset))?;
    let (mut items, mut committed, mut previous, mut start) = (
        Vec::new(),
        HashMap::<String, f64>::new(),
        0.,
        if offset > 0 {
            first_timestamp(path)?
        } else {
            0.
        },
    );
    let mut pi = PiStream::default();
    let mut index = 0;
    for line in BufReader::new(file).split(b'\n') {
        index += 1;
        let line = line?;
        let Some(value) =
            pyjson::loads_lossless(&String::from_utf8_lossy(&line)).filter(Value::is_object)
        else {
            continue;
        };
        let clock = timestamp(&value);
        if clock > 0. {
            if start == 0. {
                start = clock;
            }
            previous = clock;
        }
        let events = if provider == "kimi" {
            kimi(&value)
        } else {
            pi.feed(&value)
        };
        absorb(events, previous, index, &mut items, &mut committed);
    }
    absorb(pi.held, previous, index + 1, &mut items, &mut committed);
    Ok((items, committed, previous, start))
}

fn absorb(
    events: Vec<ChatEvent>,
    clock: f64,
    index: u64,
    items: &mut Vec<Item>,
    committed: &mut HashMap<String, f64>,
) {
    for event in events {
        let ts = event.ts.filter(|t| *t != 0.).unwrap_or(clock);
        if event.kind == ChatKind::UserMsg
            && let Some(text) = &event.text
        {
            for key in history::chaves_de_commit(text) {
                let value = committed.entry(key).or_default();
                if ts > *value {
                    *value = ts;
                }
            }
        }
        items.push((ts, index, event));
    }
}

fn clean(text: &str) -> String {
    let mut text = ANSI.replace_all(text, "").into_owned();
    py::scrub_str(&mut text);
    text
}

#[derive(Default)]
struct PiStream {
    held: Vec<ChatEvent>,
    id: String,
}

impl PiStream {
    fn feed(&mut self, value: &Value) -> Vec<ChatEvent> {
        if value["type"] == "custom_message"
            && value["customType"] == "claude-hook-context"
            && !self.held.is_empty()
            && value["parentId"].as_str() == Some(&self.id)
        {
            let context = clean(value["content"].as_str().unwrap_or(""));
            let context = py::strip(&context);
            if !context.is_empty() {
                for event in &mut self.held {
                    if let Some(text) = &mut event.text {
                        let stripped = py::strip(text);
                        if let Some(rest) = stripped.strip_prefix(context) {
                            *text = py::strip(rest).to_owned();
                        }
                    }
                }
                self.held
                    .retain(|event| event.text.as_deref().is_some_and(|text| !text.is_empty()));
            }
            return std::mem::take(&mut self.held);
        }
        let mut events = std::mem::take(&mut self.held);
        let parsed = pi(value);
        self.id = value["id"].as_str().unwrap_or("").to_owned();
        if !self.id.is_empty()
            && !parsed.is_empty()
            && parsed.iter().all(|event| event.kind == ChatKind::UserMsg)
        {
            self.held = parsed;
        } else {
            events.extend(parsed);
        }
        events
    }
}

fn pi(value: &Value) -> Vec<ChatEvent> {
    if value["type"] != "message" {
        return Vec::new();
    }
    let message = &value["message"];
    let Some(content) = message["content"].as_array() else {
        return Vec::new();
    };
    let ts = message["timestamp"].as_f64().map(|n| n / 1000.);
    if message["role"] == "toolResult" {
        return vec![ChatEvent {
            kind: ChatKind::ToolResult,
            ts,
            ..Default::default()
        }];
    }
    content
        .iter()
        .filter_map(|block| {
            let role = message["role"].as_str()?;
            let (kind, text) = match (role, block["type"].as_str()?) {
                ("user", "text") => (
                    ChatKind::UserMsg,
                    block["text"]
                        .as_str()
                        .filter(|text| !py::strip(text).is_empty())
                        .map(clean),
                ),
                ("assistant", "text") => (
                    ChatKind::AssistantMsg,
                    block["text"]
                        .as_str()
                        .filter(|text| !py::strip(text).is_empty())
                        .map(clean),
                ),
                ("assistant", "thinking") => (
                    ChatKind::Thinking,
                    block["thinking"]
                        .as_str()
                        .filter(|text| !py::strip(text).is_empty())
                        .map(clean),
                ),
                ("assistant", "toolCall") => {
                    return Some(ChatEvent {
                        kind: ChatKind::ToolUse,
                        ts,
                        ..Default::default()
                    });
                }
                _ => return None,
            };
            text.map(|text| ChatEvent {
                kind,
                text: Some(text),
                ts,
                ..Default::default()
            })
        })
        .collect()
}

fn kimi(value: &Value) -> Vec<ChatEvent> {
    let ts = value["time"].as_f64().map(|n| n / 1000.);
    if value["type"] == "context.append_message" {
        let message = &value["message"];
        if message["role"] != "user" || message["origin"]["kind"] != "user" {
            return Vec::new();
        }
        return message["content"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|block| {
                if block["type"] != "text" {
                    return None;
                }
                block["text"]
                    .as_str()
                    .filter(|text| !py::strip(text).is_empty())
                    .map(|text| ChatEvent {
                        kind: ChatKind::UserMsg,
                        text: Some(text.to_owned()),
                        ts,
                        ..Default::default()
                    })
            })
            .collect();
    }
    if value["type"] != "context.append_loop_event" {
        return Vec::new();
    }
    let event = &value["event"];
    let (kind, text) = match event["type"].as_str() {
        Some("content.part") => match event["part"]["type"].as_str() {
            Some("text") => (ChatKind::AssistantMsg, event["part"]["text"].as_str()),
            Some("think") => (ChatKind::Thinking, event["part"]["think"].as_str()),
            _ => return Vec::new(),
        },
        Some("tool.call") => {
            return vec![ChatEvent {
                kind: ChatKind::ToolUse,
                ts,
                ..Default::default()
            }];
        }
        Some("tool.result") => {
            return vec![ChatEvent {
                kind: ChatKind::ToolResult,
                ts,
                ..Default::default()
            }];
        }
        _ => return Vec::new(),
    };
    text.filter(|text| !py::strip(text).is_empty())
        .map(|text| {
            vec![ChatEvent {
                kind,
                text: Some(text.to_owned()),
                ts,
                ..Default::default()
            }]
        })
        .unwrap_or_default()
}
