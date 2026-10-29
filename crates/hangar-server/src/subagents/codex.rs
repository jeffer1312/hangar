//! Conversas filhas do Codex, ligadas pelo cabeçalho original do rollout da mesma conta.
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, PoisonError};
use std::time::SystemTime;

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate};
use serde_json::{Map, Value, json};

use super::{Acc, DETAIL_TAIL, LIST_TAIL, mtime, unreadable};
use crate::list::capped::{Capped, SESSION_CAP};
use crate::transcript::{LineParser, Provider, decode_line};
use hangar_api::chat::{ChatEvent, ChatKind};

type Stamp = (Option<SystemTime>, u64, Option<SystemTime>);
const CACHED_EVENTS: usize = 200;

#[derive(Clone)]
struct Identity {
    id: String,
    parent: Option<String>,
    born: DateTime<FixedOffset>,
    path: Option<String>,
    nickname: Option<String>,
    role: Option<String>,
    forked: bool,
}

fn checked_stamp(path: &Path) -> std::io::Result<Stamp> {
    let metadata = path.metadata()?;
    Ok((metadata.modified().ok(), metadata.len(), metadata.created().ok()))
}

fn stamp(path: &Path) -> Option<Stamp> { checked_stamp(path).ok() }

static HEADERS: LazyLock<Mutex<Capped<PathBuf, (Stamp, Option<Identity>)>>> =
    LazyLock::new(|| Mutex::new(Capped::new(SESSION_CAP * 4)));
static DIRECTORIES: LazyLock<Mutex<Capped<PathBuf, Arc<Directory>>>> =
    LazyLock::new(|| Mutex::new(Capped::new(SESSION_CAP)));

#[derive(Clone)]
struct Directory {
    stamp: Stamp,
    subdirs: Vec<PathBuf>,
    families: HashMap<String, Vec<(PathBuf, Identity)>>,
    pending: Vec<PathBuf>,
}

impl Directory {
    fn record(&mut self, path: PathBuf, identity: Identity) {
        if path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.ends_with(&format!("-{}.jsonl", identity.id)))
            && let Some(parent) = identity.parent.clone() {
            self.families.entry(parent).or_default().push((path, identity));
        }
    }
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
}

fn header(path: &Path) -> std::io::Result<Option<Identity>> {
    let seen = checked_stamp(path)?;
    if let Some((old, identity)) = HEADERS.lock().unwrap_or_else(PoisonError::into_inner).get(path)
        && *old == seen { return Ok(identity.clone()); }
    // O cabeçalho é pequeno; um arquivo que não termina a primeira linha não prende a consulta.
    let mut first = Vec::new();
    let file = std::fs::File::open(path)?;
    BufReader::new(file.take(1024 * 1024)).read_until(b'\n', &mut first)?;
    let record = decode_line(&first).unwrap_or(Value::Null);
    let value = &record["payload"];
    let identity = (|| {
        if record["type"] != "session_meta" { return None; }
        let id = value["id"].as_str().filter(|id| valid_id(id))?;
        let born = DateTime::parse_from_rfc3339(value["timestamp"].as_str()?).ok()?;
        let source = &value["source"]["subagent"]["thread_spawn"];
        let parent = source["parent_thread_id"].as_str().or_else(|| value["parent_thread_id"].as_str());
        if parent.is_some_and(|id| !valid_id(id)) { return None; }
        if source["parent_thread_id"].as_str().zip(value["parent_thread_id"].as_str()).is_some_and(|(a, b)| a != b) { return None; }
        Some(Identity {
            id: id.to_owned(), parent: parent.map(str::to_owned), born,
            path: source["agent_path"].as_str().or_else(|| value["agent_path"].as_str()).map(str::to_owned),
            nickname: source["agent_nickname"].as_str().or_else(|| value["agent_nickname"].as_str()).map(str::to_owned),
            role: source["agent_role"].as_str().or_else(|| value["agent_role"].as_str()).map(str::to_owned),
            forked: value["forked_from_id"].as_str().is_some_and(|id| !id.is_empty()),
        })
    })();
    HEADERS.lock().unwrap_or_else(PoisonError::into_inner).insert(path.to_path_buf(), (seen, identity.clone()));
    Ok(identity)
}

pub(super) fn parent_thread_id(path: &Path) -> Option<String> { header(path).ok().flatten().map(|identity| identity.id) }

fn sessions_root(path: &Path) -> Option<PathBuf> {
    let real = path.canonicalize().ok()?;
    let root = real.ancestors().find(|p| p.file_name().is_some_and(|n| n == "sessions"))?;
    let components: Vec<_> = real.strip_prefix(root).ok()?.components().collect();
    (components.len() == 4).then(|| root.to_path_buf())
}

fn date_dirs(root: &Path, since: NaiveDate) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for year in &directory(root)?.subdirs {
        let Some(y) = year.file_name().and_then(|s| s.to_str()).and_then(|s| s.parse::<i32>().ok()) else { continue };
        if y < since.year() { continue; }
        for month in &directory(year)?.subdirs {
            let Some(m) = month.file_name().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
            if y == since.year() && m < since.month() { continue; }
            for day in &directory(month)?.subdirs {
                let Some(d) = day.file_name().and_then(|s| s.to_str()).and_then(|s| s.parse::<u32>().ok()) else { continue };
                if NaiveDate::from_ymd_opt(y, m, d).is_some_and(|date| date >= since) { out.push(day.clone()); }
            }
        }
    }
    Ok(out)
}

fn directory(dir: &Path) -> std::io::Result<Arc<Directory>> { directory_snapshot(dir, false) }

fn directory_snapshot(dir: &Path, rescan: bool) -> std::io::Result<Arc<Directory>> {
    let seen = checked_stamp(dir)?;
    let cached = DIRECTORIES.lock().unwrap_or_else(PoisonError::into_inner).get(dir).cloned();
    if let Some(cached) = cached.filter(|old| !rescan && old.stamp == seen) {
        // O arquivo pode aparecer antes de a primeira linha ser escrita, sem mudar a pasta depois.
        let mut ready = Vec::new();
        for path in &cached.pending {
            match header(path) {
                Ok(Some(identity)) => ready.push((path.clone(), identity)),
                Ok(None) => {},
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return directory_snapshot(dir, true),
                Err(error) => return Err(error),
            }
        }
        if ready.is_empty() { return Ok(cached); }
        let mut next = (*cached).clone();
        next.pending.retain(|path| !ready.iter().any(|(ready, _)| ready == path));
        for (path, identity) in ready { next.record(path, identity); }
        let next = Arc::new(next);
        DIRECTORIES.lock().unwrap_or_else(PoisonError::into_inner).insert(dir.to_path_buf(), next.clone());
        return Ok(next);
    }
    let mut next = Directory { stamp: seen, subdirs: Vec::new(), families: HashMap::new(), pending: Vec::new() };
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_dir() { next.subdirs.push(entry.path()); }
        else if kind.is_file() && entry.file_name().to_str().is_some_and(|n| n.starts_with("rollout-") && n.ends_with(".jsonl")) {
            match header(&entry.path())? {
                Some(identity) => next.record(entry.path(), identity),
                None => next.pending.push(entry.path()),
            }
        }
    }
    let next = Arc::new(next);
    DIRECTORIES.lock().unwrap_or_else(PoisonError::into_inner).insert(dir.to_path_buf(), next.clone());
    Ok(next)
}

fn children(parent: &Path) -> Result<Vec<(PathBuf, Identity)>, &'static str> {
    let root = sessions_root(parent).ok_or("codex_subagents_source_unavailable")?;
    let own = header(parent).map_err(|_| "codex_subagents_source_unavailable")?.ok_or("codex_subagents_source_unavailable")?;
    let real = parent.canonicalize().map_err(|_| "codex_subagents_source_unavailable")?;
    let relative = real.strip_prefix(&root).map_err(|_| "codex_subagents_source_unavailable")?;
    let parts = relative.components().take(3).map(|part| part.as_os_str().to_str()).collect::<Option<Vec<_>>>()
        .ok_or("codex_subagents_source_unavailable")?;
    let since = NaiveDate::parse_from_str(&parts.join("-"), "%Y-%m-%d").map_err(|_| "codex_subagents_source_unavailable")?;
    let mut out = Vec::new();
    // Um filho não pode nascer antes do pai; arquivos antigos da conta nem têm o cabeçalho aberto.
    for dir in date_dirs(&root, since).map_err(|_| "codex_subagents_source_unavailable")? {
        let snapshot = directory(&dir).map_err(|_| "codex_subagents_source_unavailable")?;
        let family = match child_family(&snapshot, &root, &own) {
            Ok(family) => family,
            // A exclusão pode conservar o carimbo da pasta; a releitura é única e só para ela.
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let refreshed = directory_snapshot(&dir, true).map_err(|_| "codex_subagents_source_unavailable")?;
                child_family(&refreshed, &root, &own).map_err(|_| "codex_subagents_source_unavailable")?
            }
            Err(_) => return Err("codex_subagents_source_unavailable"),
        };
        out.extend(family);
    }
    Ok(out)
}

fn child_family(snapshot: &Directory, root: &Path, own: &Identity) -> std::io::Result<Vec<(PathBuf, Identity)>> {
    let mut out = Vec::new();
    for (path, _) in snapshot.families.get(&own.id).into_iter().flatten() {
        let real = path.canonicalize()?;
        if !real.starts_with(root) { continue; }
        let identity = header(&real)?.ok_or_else(|| std::io::Error::from(std::io::ErrorKind::InvalidData))?;
        if identity.parent.as_deref() == Some(&own.id) && identity.born >= own.born
            && real.file_name().and_then(|n| n.to_str()).is_some_and(|name| name.ends_with(&format!("-{}.jsonl", identity.id))) {
            out.push((real, identity));
        }
    }
    Ok(out)
}

pub(super) fn list(parent: &Path) -> Vec<Map<String, Value>> {
    list_checked(parent).unwrap_or_default()
}

pub(super) fn list_checked(parent: &Path) -> Result<Vec<Map<String, Value>>, &'static str> {
    let mut out = children(parent)?.into_iter().map(|(path, identity)| {
        read(&path, &identity, LIST_TAIL, 0).unwrap_or_else(|| unreadable(&identity.id, mtime(&path)))
    }).collect::<Vec<_>>();
    out.sort_by(|a, b| super::num(b, "mtime").total_cmp(&super::num(a, "mtime")));
    Ok(out)
}

pub(super) fn get(parent: &Path, id: &str, events: i64) -> Option<Map<String, Value>> {
    get_checked(parent, id, events).ok().flatten()
}

pub(super) fn get_checked(parent: &Path, id: &str, events: i64) -> Result<Option<Map<String, Value>>, &'static str> {
    if !valid_id(id) { return Ok(None); }
    let Some((path, identity)) = children(parent)?.into_iter().find(|(_, identity)| identity.id == id) else { return Ok(None) };
    Ok(Some(read(&path, &identity, DETAIL_TAIL, usize::try_from(events.max(0)).unwrap_or(0))
        .unwrap_or_else(|| unreadable(id, mtime(&path)))))
}

#[derive(Clone)]
struct Parsed {
    stamp: Stamp,
    offset: u64,
    acc: Acc,
    model: Option<String>,
    state: Option<&'static str>,
    ended: Option<String>,
    events: VecDeque<ChatEvent>,
    capacity: usize,
    own_started: bool,
}

static AGENTS: LazyLock<Mutex<Capped<PathBuf, Parsed>>> = LazyLock::new(|| Mutex::new(Capped::new(SESSION_CAP)));

fn read(path: &Path, identity: &Identity, tail: usize, events: usize) -> Option<Map<String, Value>> {
    let seen = stamp(path)?;
    let cached = AGENTS.lock().unwrap_or_else(PoisonError::into_inner).get(path).cloned();
    let capacity = events.max(CACHED_EVENTS);
    let mut parsed = cached.filter(|p| p.capacity >= capacity && p.offset <= seen.1 && p.stamp.2 == seen.2
        && (p.stamp == seen || seen.1 > p.stamp.1)).unwrap_or_else(|| Parsed {
            stamp: (None, 0, seen.2), offset: 0, acc: Acc { started: identity.born.to_rfc3339(), agent_type: identity.role.clone(), ..Acc::default() },
            model: None, state: None, ended: None, events: VecDeque::new(), capacity, own_started: !identity.forked,
        });
    if parsed.stamp != seen {
        let mut file = std::fs::File::open(path).ok()?;
        file.seek(SeekFrom::Start(parsed.offset)).ok()?;
        let mut reader = BufReader::new(file);
        let mut parser = LineParser::new(Provider::Codex);
        loop {
            let mut line = Vec::new();
            let bytes = reader.read_until(b'\n', &mut line).ok()?;
            if bytes == 0 || line.last() != Some(&b'\n') { break; }
            let offset = parsed.offset;
            parsed.offset += bytes as u64;
            let Some(record) = decode_line(&line) else { continue };
            if record["type"] == "session_meta" { continue; }
            let explicit_thread = record["payload"]["thread_id"].as_str();
            if explicit_thread.is_some_and(|thread| thread != identity.id) { continue; }
            let when = record["timestamp"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok());
            let fresh_directive = record["type"] == "response_item" && record["payload"]["role"] == "developer"
                && when.is_some_and(|when| when >= identity.born)
                && visible_content(&record["payload"]["content"]).starts_with("You are now a newly spawned child agent.");
            if explicit_thread == Some(identity.id.as_str()) || fresh_directive { parsed.own_started = true; }
            // O fork contém a conversa do pai; o marcador nativo evita atribuir até eventos com o mesmo horário.
            if !parsed.own_started { continue; }
            if when.is_some_and(|when| when < identity.born) { continue; }
            if let Some(when) = when { parsed.acc.when(when.to_rfc3339()); }
            if record["type"] == "turn_context" {
                parsed.model = record["payload"]["model"].as_str().map(str::to_owned).or(parsed.model);
            }
            if record["type"] == "event_msg" {
                match record["payload"]["type"].as_str() {
                    Some("task_started") => { parsed.state = Some("running"); parsed.ended = None; }
                    Some("task_complete") => { parsed.state = Some("finished"); parsed.ended = when.map(|when| when.to_rfc3339()); }
                    Some("turn_aborted" | "task_failed") => { parsed.state = Some("failed"); parsed.ended = when.map(|when| when.to_rfc3339()); }
                    _ => {}
                }
            }
            if parsed.acc.prompt.is_none() && record["type"] == "response_item" && record["payload"]["type"] == "agent_message" {
                let text = visible_content(&record["payload"]["content"]);
                if text.starts_with("Message Type: NEW_TASK") && let Some((_, task)) = text.split_once("Payload:\n")
                    && !task.trim().is_empty() { parsed.acc.prompt = Some(task.trim().to_owned()); }
            }
            for event in parser.feed(&line, offset) {
                match &event.kind {
                    ChatKind::ToolUse => parsed.acc.call(event.tool_name.as_deref().unwrap_or("?"), event.tool_input.as_ref().map(|m| Value::Object(m.clone())).as_ref()),
                    ChatKind::UserMsg if parsed.acc.prompt.is_none() => parsed.acc.prompt = event.text.clone().filter(|text| !text.trim().is_empty()),
                    ChatKind::AssistantMsg => if let Some(text) = event.text.as_ref().filter(|text| !text.trim().is_empty()) { parsed.acc.last_text = text.clone(); },
                    _ => {}
                }
                if parsed.acc.calls.len() > DETAIL_TAIL { parsed.acc.calls.remove(0); }
                if parsed.events.len() == parsed.capacity { parsed.events.pop_front(); }
                parsed.events.push_back(event);
            }
        }
        parsed.stamp = seen;
        // Páginas maiores servem à leitura do histórico, sem ficar retidas no cache da lista.
        if capacity == CACHED_EVENTS {
            AGENTS.lock().unwrap_or_else(PoisonError::into_inner).insert(path.to_path_buf(), parsed.clone());
        }
    }
    let mut out = parsed.acc.clone().into_map(&identity.id, mtime(path), tail);
    out.insert("agentPath".into(), json!(identity.path));
    out.insert("nickname".into(), json!(identity.nickname));
    out.insert("model".into(), json!(parsed.model));
    out.insert("state".into(), json!(parsed.state));
    out.insert("endedAt".into(), json!(parsed.ended));
    if events > 0 {
        out.insert("events".into(), json!(parsed.events.iter().skip(parsed.events.len().saturating_sub(events)).collect::<Vec<_>>()));
    }
    Some(out)
}

fn visible_content(value: &Value) -> String {
    value.as_array().into_iter().flatten().filter(|part| part["type"] == "input_text" || part["type"] == "text")
        .filter_map(|part| part["text"].as_str()).collect::<Vec<_>>().join("")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(root: &Path, id: &str, parent: Option<&str>, records: &[Value]) -> PathBuf {
        let path = root.join("sessions/2026/10/28").join(format!("rollout-2026-10-28T09-00-00-{id}.jsonl"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let born = if parent.is_some() { "2026-10-28T09:00:00Z" } else { "2026-10-28T08:00:00Z" };
        let mut lines = vec![json!({"type":"session_meta","payload":{"id":id,"timestamp":born,"source":parent.map(|id| json!({"subagent":{"thread_spawn":{"parent_thread_id":id,"agent_path":format!("/root/{id}"),"agent_role":"worker","agent_nickname":"Kepler"}}}))}})];
        lines.extend_from_slice(records);
        std::fs::write(&path, lines.iter().map(|r|format!("{r}\n")).collect::<String>()).unwrap();
        path
    }

    fn event(when: &str, kind: &str) -> Value { json!({"timestamp":when,"type":"event_msg","payload":{"type":kind}}) }

    #[test]
    fn only_direct_children_in_the_same_account_are_listed_and_readable() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        write(root.path(), "mine", Some("parent"), &[event("2026-10-28T09:00:01Z", "task_started")]);
        write(root.path(), "other", Some("foreign"), &[]);
        write(root.path(), "grandchild", Some("mine"), &[]);
        let other_account = tempfile::tempdir().unwrap();
        write(other_account.path(), "outside", Some("parent"), &[]);
        let rows = list(&parent);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["agentId"], "mine");
        assert_eq!(rows[0]["agentPath"], "/root/parent");
        assert!(get(&parent, "other", 200).is_none());
        assert!(get(&parent, "outside", 200).is_none());
        assert!(get(&parent, "../mine", 200).is_none());
        assert!(get(&parent, "C:mine", 200).is_none());
    }

    #[test]
    fn discovery_reuses_unchanged_directories_and_detects_children_on_new_days_and_removal() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let child = write(root.path(), "mine", Some("parent"), &[event("2026-10-28T09:00:01Z", "task_started")]);
        assert_eq!(list_checked(&parent).unwrap().len(), 1);
        let cached = directory(child.parent().unwrap()).unwrap();
        let mut file = std::fs::OpenOptions::new().append(true).open(&child).unwrap();
        writeln!(file, "{}", event("2026-10-28T09:00:02Z", "task_complete")).unwrap();
        drop(file);
        assert_eq!(get_checked(&parent, "mine", 200).unwrap().unwrap()["state"], "finished");
        assert!(Arc::ptr_eq(&cached, &directory(child.parent().unwrap()).unwrap()));

        write(root.path(), "same-day", Some("parent"), &[]);
        assert_eq!(list_checked(&parent).unwrap().len(), 2);
        let later = write(root.path(), "next-day", Some("parent"), &[]);
        let next_day = root.path().join("sessions/2026/10/29");
        std::fs::create_dir_all(&next_day).unwrap();
        std::fs::rename(&later, next_day.join(later.file_name().unwrap())).unwrap();
        assert!(get_checked(&parent, "next-day", 0).unwrap().is_some());
        std::fs::remove_file(&child).unwrap();
        assert!(get_checked(&parent, "mine", 200).unwrap().is_none());
        assert_eq!(list_checked(&parent).unwrap().len(), 2);
    }

    fn keep_cached_directory_stamp(dir: &Path, cached: &Directory) -> Arc<Directory> {
        let mut stale = cached.clone();
        stale.stamp = stamp(dir).unwrap();
        let stale = Arc::new(stale);
        DIRECTORIES.lock().unwrap_or_else(PoisonError::into_inner).insert(dir.to_path_buf(), stale.clone());
        stale
    }

    #[test]
    fn a_removed_child_rescans_only_its_cached_directory_even_when_the_stamp_is_unchanged() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let child = write(root.path(), "mine", Some("parent"), &[]);
        write(root.path(), "sibling", Some("parent"), &[]);
        write(root.path(), "foreign", Some("another-parent"), &[]);
        assert_eq!(list_checked(&parent).unwrap().len(), 2);
        let dir = child.parent().unwrap();
        let cached = directory(dir).unwrap();
        let root_cache = directory(&sessions_root(&parent).unwrap()).unwrap();
        std::fs::remove_file(&child).unwrap();
        let stale = keep_cached_directory_stamp(dir, &cached);
        assert_eq!(stale.stamp, stamp(dir).unwrap());
        assert!(Arc::ptr_eq(&stale, &directory(dir).unwrap()));
        assert!(get_checked(&parent, "mine", 0).unwrap().is_none());
        let refreshed = directory(dir).unwrap();
        assert!(!Arc::ptr_eq(&stale, &refreshed));
        assert_eq!(list_checked(&parent).unwrap()[0]["agentId"], "sibling");
        assert!(get_checked(&parent, "sibling", 0).unwrap().is_some());
        assert_eq!(list_checked(&parent).unwrap().len(), 1);
        assert!(Arc::ptr_eq(&refreshed, &directory(dir).unwrap()));
        assert!(Arc::ptr_eq(&root_cache, &directory(&sessions_root(&parent).unwrap()).unwrap()));
    }

    #[test]
    fn a_removed_pending_header_refreshes_the_unchanged_directory_once() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let child = write(root.path(), "pending", Some("parent"), &[]);
        std::fs::write(&child, "").unwrap();
        assert!(list_checked(&parent).unwrap().is_empty());
        let dir = child.parent().unwrap();
        let cached = directory(dir).unwrap();
        assert_eq!(cached.pending, vec![child.clone()]);
        std::fs::remove_file(&child).unwrap();
        let stale = keep_cached_directory_stamp(dir, &cached);
        assert!(list_checked(&parent).unwrap().is_empty());
        let refreshed = directory(dir).unwrap();
        assert!(!Arc::ptr_eq(&stale, &refreshed));
        assert!(refreshed.pending.is_empty());
        assert!(Arc::ptr_eq(&refreshed, &directory(dir).unwrap()));
    }

    #[test]
    fn a_child_read_error_is_visible_without_invalidating_a_healthy_directory() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let child = write(root.path(), "mine", Some("parent"), &[]);
        assert!(get_checked(&parent, "mine", 0).unwrap().is_some());
        let dir = child.parent().unwrap();
        let cached = directory(dir).unwrap();
        std::fs::remove_file(&child).unwrap();
        std::fs::create_dir(&child).unwrap();
        let stale = keep_cached_directory_stamp(dir, &cached);
        assert_eq!(get_checked(&parent, "mine", 0).unwrap_err(), "codex_subagents_source_unavailable");
        assert!(Arc::ptr_eq(&stale, &directory(dir).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn permission_denied_is_not_reclassified_as_a_missing_child() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let child = write(root.path(), "mine", Some("parent"), &[]);
        assert!(get_checked(&parent, "mine", 0).unwrap().is_some());
        let dir = child.parent().unwrap();
        let cached = directory(dir).unwrap();
        let permissions = std::fs::metadata(dir).unwrap().permissions();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0)).unwrap();
        let header_error = header(&child).err();
        let discovery = get_checked(&parent, "mine", 0);
        std::fs::set_permissions(dir, permissions).unwrap();
        assert_eq!(header_error.unwrap().kind(), std::io::ErrorKind::PermissionDenied);
        assert_eq!(discovery.unwrap_err(), "codex_subagents_source_unavailable");
        assert!(Arc::ptr_eq(&cached, &directory(dir).unwrap()));
    }

    #[test]
    fn a_late_header_is_discovered_without_another_directory_change() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let child = write(root.path(), "late", Some("parent"), &[]);
        let contents = std::fs::read(&child).unwrap();
        std::fs::write(&child, "").unwrap();
        assert!(list_checked(&parent).unwrap().is_empty());
        let before = stamp(child.parent().unwrap()).unwrap();
        std::fs::write(&child, contents).unwrap();
        assert_eq!(stamp(child.parent().unwrap()).unwrap(), before);
        assert!(get_checked(&parent, "late", 0).unwrap().is_some());
    }

    /// O NTFS só avança a data de modificação no tique do relógio: duas escritas seguidas do mesmo tamanho saíam com o
    /// mesmo carimbo. Na vida real a troca do arquivo não cai no mesmo tique da leitura anterior.
    fn rewrite(path: &Path, contents: String) {
        let before = path.metadata().unwrap().modified().unwrap();
        std::fs::write(path, contents).unwrap();
        std::fs::File::options().write(true).open(path).unwrap().set_modified(before + std::time::Duration::from_secs(1)).unwrap();
    }

    #[test]
    fn replacing_the_parent_or_child_does_not_reuse_the_old_family() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let child = write(root.path(), "mine", Some("parent"), &[]);
        write(root.path(), "new-family", Some("new-parent"), &[]);
        assert!(get_checked(&parent, "mine", 0).unwrap().is_some());
        let mut meta: Value = serde_json::from_str(std::fs::read_to_string(&parent).unwrap().trim()).unwrap();
        meta["payload"]["id"] = json!("new-parent");
        rewrite(&parent, format!("{meta}\n"));
        assert!(get_checked(&parent, "mine", 0).unwrap().is_none());
        assert!(get_checked(&parent, "new-family", 0).unwrap().is_some());
        meta["payload"]["timestamp"] = json!("2026-10-28T10:00:00Z");
        rewrite(&parent, format!("{meta}\n"));
        assert!(get_checked(&parent, "new-family", 0).unwrap().is_none());
        let replacement = write(root.path(), "replacement", Some("new-parent"), &[]);
        let mut child_meta: Value = serde_json::from_str(std::fs::read_to_string(&replacement).unwrap().trim()).unwrap();
        child_meta["payload"]["id"] = json!("mine");
        std::fs::write(&replacement, format!("{child_meta}\n")).unwrap();
        meta["payload"]["id"] = json!("parent");
        meta["payload"]["timestamp"] = json!("2026-10-28T08:00:00Z");
        rewrite(&parent, format!("{meta}\n"));
        std::fs::rename(replacement, &child).unwrap();
        assert!(get_checked(&parent, "mine", 0).unwrap().is_none());
    }

    #[test]
    fn a_failed_directory_read_never_returns_the_cached_snapshot() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("day");
        std::fs::create_dir(&path).unwrap();
        directory(&path).unwrap();
        std::fs::remove_dir(&path).unwrap();
        std::fs::write(&path, "not a directory").unwrap();
        assert!(directory(&path).is_err());
    }

    #[test]
    fn inherited_parent_history_is_excluded_and_native_lifecycle_can_resume() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let child = write(root.path(), "mine", Some("parent"), &[
            json!({"timestamp":"2026-10-28T08:30:00Z","type":"response_item","payload":{"type":"function_call","name":"inherited","call_id":"old","arguments":"{}"}}),
            event("2026-10-28T08:31:00Z", "task_complete"),
            event("2026-10-28T09:00:01Z", "task_started"),
            json!({"timestamp":"2026-10-28T09:00:02Z","type":"turn_context","payload":{"model":"codex-model"}}),
            json!({"timestamp":"2026-10-28T09:00:03Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"own","arguments":"{\"cmd\":\"pwd\"}"}}),
            event("2026-10-28T09:00:04Z", "task_complete"),
        ]);
        let detail = get(&parent, "mine", 200).unwrap();
        assert_eq!(detail["state"], "finished");
        assert_eq!(detail["toolCalls"], 1);
        assert_eq!(detail["recent"][0]["target"], "pwd");
        assert_eq!(detail["model"], "codex-model");
        assert!(detail["events"].as_array().unwrap().iter().all(|e| e["tool_name"] != "inherited"));
        let mut file = std::fs::OpenOptions::new().append(true).open(child).unwrap();
        writeln!(file, "{}", event("2026-10-28T09:01:00Z", "task_started")).unwrap();
        let resumed = get(&parent, "mine", 200).unwrap();
        assert_eq!(resumed["state"], "running");
        assert!(resumed["endedAt"].is_null());
        assert_eq!(resumed["toolCalls"], 1);
    }

    #[test]
    fn older_history_is_returned_without_expanding_the_persistent_cache() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let records = (0..600).map(|index| json!({"timestamp":"2026-10-28T09:00:01Z","type":"response_item",
            "payload":{"type":"function_call","name":"exec_command","call_id":format!("call-{index}"),"arguments":"{\"cmd\":\"pwd\"}"}})).collect::<Vec<_>>();
        let child = write(root.path(), "mine", Some("parent"), &records);
        assert_eq!(get_checked(&parent, "mine", 200).unwrap().unwrap()["events"].as_array().unwrap().len(), 200);
        let page = get_checked(&parent, "mine", 400).unwrap().unwrap();
        assert_eq!(page["events"].as_array().unwrap().len(), 400);
        assert_eq!(page["events"][0]["tool_use_id"], "call-200");
        assert_eq!(get_checked(&parent, "mine", i64::MAX).unwrap().unwrap()["events"].as_array().unwrap().len(), 600);
        let cache = AGENTS.lock().unwrap_or_else(PoisonError::into_inner);
        let parsed = cache.peek(&child.canonicalize().unwrap()).unwrap();
        assert_eq!(parsed.capacity, CACHED_EVENTS);
        assert_eq!(parsed.events.len(), CACHED_EVENTS);
    }

    #[test]
    fn encrypted_task_body_is_not_a_visible_prompt_and_missing_state_is_unknown() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        write(root.path(), "mine", Some("parent"), &[
            json!({"timestamp":"2026-10-28T09:00:01Z","type":"response_item","payload":{"type":"agent_message","content":[{"type":"input_text","text":"Message Type: NEW_TASK\nPayload:\n"},{"type":"encrypted_content","encrypted_content":"secret"}]}}),
        ]);
        let detail = get(&parent, "mine", 200).unwrap();
        assert!(detail["state"].is_null());
        assert!(detail["prompt"].is_null());
        assert!(!serde_json::to_string(&detail).unwrap().contains("secret"));
    }

    #[test]
    fn fork_boundary_excludes_equal_timestamp_parent_events_and_keeps_own_unstamped_lines() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let path = write(root.path(), "mine", Some("parent"), &[]);
        let mut records = std::fs::read_to_string(&path).unwrap().lines().map(|line| serde_json::from_str::<Value>(line).unwrap()).collect::<Vec<_>>();
        records[0]["payload"]["forked_from_id"] = json!("parent");
        records.extend([
            json!({"timestamp":"2026-10-28T08:00:00Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"You are now a newly spawned child agent."}]}}),
            event("2026-10-28T09:00:00Z", "task_complete"),
        ]);
        std::fs::write(&path, records.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
        assert!(get(&parent, "mine", 200).unwrap()["state"].is_null());
        let mut file = std::fs::OpenOptions::new().append(true).open(path).unwrap();
        writeln!(file, "{}", json!({"type":"event_msg","payload":{"type":"thread_settings_applied","thread_id":"mine"}})).unwrap();
        writeln!(file, "{}", json!({"type":"event_msg","payload":{"type":"task_started"}})).unwrap();
        writeln!(file, "{}", json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","call_id":"own","arguments":"{\"cmd\":\"pwd\"}"}})).unwrap();
        let own = get(&parent, "mine", 200).unwrap();
        assert_eq!(own["state"], "running");
        assert_eq!(own["toolCalls"], 1);
        assert_eq!(own["events"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn unreadable_source_is_an_error_instead_of_an_empty_family() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        assert!(list_checked(&parent).unwrap().is_empty());
        std::fs::write(&parent, "not-json\n").unwrap();
        assert_eq!(list_checked(&parent).unwrap_err(), "codex_subagents_source_unavailable");
        assert_eq!(get_checked(&parent, "mine", 200).unwrap_err(), "codex_subagents_source_unavailable");
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_rollout_from_another_account_is_not_opened() {
        let root = tempfile::tempdir().unwrap();
        let parent = write(root.path(), "parent", None, &[]);
        let foreign = tempfile::tempdir().unwrap();
        let child = write(foreign.path(), "mine", Some("parent"), &[]);
        std::os::unix::fs::symlink(&child, parent.parent().unwrap().join(child.file_name().unwrap())).unwrap();
        assert!(list_checked(&parent).unwrap().is_empty());
        assert!(get(&parent, "mine", 200).is_none());
    }
}
