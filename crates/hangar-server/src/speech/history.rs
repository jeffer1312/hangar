//! Fim das conversas para sugerir tarefa: os parsers já migrados e os registros Pi/omp/Kimi.
use std::{
    collections::HashMap,
    io,
    path::Path,
};

use hangar_api::chat::{ChatEvent, ChatKind};
use serde_json::Value;

use crate::transcript::{InternalInfo, history, py, pyjson};

#[path = "transfer_history.rs"]
pub(crate) mod transfer_history;

type Item = (f64, u64, ChatEvent);

pub fn read(info: &InternalInfo, limit: usize) -> io::Result<Vec<ChatEvent>> {
    match info.history_request(Some(limit)) {
        Some(request) => history::merged_history_window(&request),
        None => Ok(Vec::new()),
    }
}

pub fn read_row(
    row: &hangar_api::session::SessionRow,
    home: &Path,
    claude: &Path,
    limit: usize,
) -> io::Result<Vec<ChatEvent>> {
    let info = crate::locate::info_of(row, claude);
    if let Some(events) = transfer_history::read(row, home, &info, limit)? {
        return Ok(events);
    }
    read(&info, limit)
}

fn merge_queue(
    info: &InternalInfo,
    items: &mut Vec<Item>,
    committed: &HashMap<String, f64>,
    previous: f64,
    start: f64,
) -> io::Result<()> {
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
    Ok(())
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
