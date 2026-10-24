//! Transferência de conversa vista pela lista (`registry._decorate_transfers`): só leitura dos
//! registros que o `conversation_transfer.py` grava em `~/.hangar/conversation-transfers`.
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use hangar_api::session::SessionRow;
use serde_json::{Map, Value};

use super::discover::Problems;
use super::discover_other::Dirs;

const TERMINAL: [&str; 3] = ["complete", "rejected", "rolled_back"];
const PHASES: [&str; 9] = ["preparing", "source_stopped", "imported", "publishing", "complete", "restoring",
    "restore_failed", "rejected", "rolled_back"];
const META_FIELDS: [&str; 29] = ["name", "provider", "cwd", "jsonl", "session_id", "key", "config_dir", "headless",
    "engine", "model", "effort", "context_window", "permission_mode", "previous_non_plan", "subagent_model", "jev",
    "thread_id", "rollout_path", "codex_home", "codex_account", "lifecycle_id", "endpoint", "app_pid", "tui_pid",
    "launcher_pid", "pane_id", "pane_pid", "tool_output_token_limit", "transfer_id"];

#[derive(Clone, Debug)]
pub struct Record {
    pub id: String,
    pub name: String,
    pub source_life: String,
    pub phase: String,
    source_path: Option<String>,
    origin: Map<String, Value>,
    destination: Option<Map<String, Value>>,
    boundary_thread: Option<String>,
    pub error_code: Option<String>,
}

impl Record {
    fn terminal(&self) -> bool { TERMINAL.contains(&self.phase.as_str()) }

    /// `_destination`: (codex_home, thread) do destino, pelo metadado ou pela fronteira importada.
    fn destination(&self) -> Option<(String, String)> {
        let meta = self.destination.as_ref();
        let home = meta.and_then(|m| m.get("codex_home")).and_then(text)?;
        let thread = meta.and_then(|m| m.get("thread_id")).and_then(text).map(str::to_owned).or_else(|| self.boundary_thread.clone())?;
        (!thread.is_empty()).then(|| (home.to_owned(), thread))
    }
}

fn text(v: &Value) -> Option<&str> { v.as_str().filter(|s| !s.is_empty()) }

/// `str(uuid.UUID(value))`: a forma canônica, em minúsculas e com hífens.
fn uuid_id(value: &str) -> Result<String, String> {
    let raw = value.trim().trim_start_matches("urn:uuid:").trim_matches(|c| c == '{' || c == '}').replace('-', "");
    if raw.len() != 32 || !raw.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("transfer_id_invalid".into());
    }
    let h = raw.to_ascii_lowercase();
    Ok(format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..]))
}

pub(crate) fn sha256_hex(text: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, text.as_bytes()).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// `_thread_key`: `json.dumps([home resolvido, thread], ensure_ascii=False)`, com o espaço do separador.
pub(crate) fn thread_key(codex_home: &str, thread: &str) -> String {
    let home = super::links::resolve_lenient(&expand_user(codex_home));
    format!("[{}, {}]", serde_json::to_string(&home.to_string_lossy()).unwrap_or_default(), serde_json::to_string(thread).unwrap_or_default())
}

fn expand_user(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') =>
            std::env::home_dir().unwrap_or_default().join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(path),
    }
}

fn exact(obj: &Map<String, Value>, fields: &[&str]) -> Result<(), String> {
    if obj.len() != fields.len() || fields.iter().any(|f| !obj.contains_key(*f)) {
        return Err("transfer_record_fields".into());
    }
    Ok(())
}

fn string(v: &Value) -> Result<String, String> { text(v).map(str::to_owned).ok_or_else(|| "transfer_record_text".into()) }

fn digest(v: &Value) -> Result<(), String> {
    let d = string(v)?;
    if d.len() != 64 || !d.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')) { return Err("transfer_record_digest".into()); }
    Ok(())
}

/// `_metadata`: só campos conhecidos, com o tipo de cada um.
fn metadata(v: &Value) -> Result<Map<String, Value>, String> {
    let obj = v.as_object().ok_or("transfer_record_meta")?;
    for (key, item) in obj {
        if !META_FIELDS.contains(&key.as_str()) { return Err("transfer_record_meta".into()); }
        if item.is_null() { continue; }
        let ok = match key.as_str() {
            "headless" | "jev" => item.is_boolean(),
            "context_window" | "app_pid" | "tui_pid" | "launcher_pid" | "pane_pid" | "tool_output_token_limit" => item.is_i64() || item.is_u64(),
            _ => item.is_string(),
        };
        if !ok { return Err("transfer_record_meta".into()); }
    }
    Ok(obj.clone())
}

/// `_decode`: o registro inteiro, recusado ao primeiro campo fora da forma.
fn decode(v: &Value) -> Result<Record, String> {
    let data = v.as_object().ok_or("transfer_record_fields")?;
    exact(data, &["id", "name", "source_life", "phase", "source", "origin_meta", "destination_meta", "boundary", "error_code"])?;
    let mut source_path = None;
    if !data["source"].is_null() {
        let s = data["source"].as_object().ok_or("transfer_record_fields")?;
        exact(s, &["path", "provider", "digest", "selected_uuids"])?;
        digest(&s["digest"])?;
        if s["provider"] != "claude" { return Err("transfer_record_source".into()); }
        let uuids = s["selected_uuids"].as_array().ok_or("transfer_record_list")?;
        for u in uuids { string(u)?; }
        source_path = Some(string(&s["path"])?);
    }
    let mut boundary_thread = None;
    if !data["boundary"].is_null() {
        let b = data["boundary"].as_object().ok_or("transfer_record_boundary")?;
        let mut b = b.clone();
        b.entry("prefix_digest").or_insert(Value::Null);
        exact(&b, &["thread_id", "rollout_path", "min_offset", "imported_item_ids", "prefix_digest"])?;
        if !b["prefix_digest"].is_null() { digest(&b["prefix_digest"])?; }
        if !(b["min_offset"].is_i64() || b["min_offset"].is_u64()) || b["min_offset"].as_i64().is_some_and(|n| n < 0) {
            return Err("transfer_record_boundary".into());
        }
        string(&b["rollout_path"])?;
        for u in b["imported_item_ids"].as_array().ok_or("transfer_record_list")? { string(u)?; }
        boundary_thread = Some(string(&b["thread_id"])?);
    }
    let error_code = if data["error_code"].is_null() { None } else { Some(string(&data["error_code"])?) };
    let destination = if data["destination_meta"].is_null() { None } else { Some(metadata(&data["destination_meta"])?) };
    if let (Some(thread), Some(dest)) = (&boundary_thread, &destination)
        && let Some(t) = dest.get("thread_id").filter(|t| !t.is_null()) && t.as_str() != Some(thread) {
        return Err("transfer_record_thread".into());
    }
    let phase = data["phase"].as_str().filter(|p| PHASES.contains(p)).ok_or("transfer_record_phase")?.to_owned();
    Ok(Record { id: uuid_id(&string(&data["id"])?)?, name: string(&data["name"])?, source_life: string(&data["source_life"])?,
        phase, source_path, origin: metadata(&data["origin_meta"])?, destination, boundary_thread, error_code })
}

/// Os registros e os índices de uma rodada: cada um lido uma vez.
pub struct Reader {
    base: PathBuf,
    records: HashMap<String, Option<Record>>,
}

impl Reader {
    pub fn new(home: &Path) -> Self {
        Self { base: home.join(".hangar").join("conversation-transfers"), records: HashMap::new() }
    }

    /// `load_transfer`: só a ausência do arquivo é ausência; o resto é corrupção.
    fn load(&mut self, id: &str) -> Result<Option<Record>, String> {
        let id = uuid_id(id)?;
        if let Some(r) = self.records.get(&id) { return Ok(r.clone()); }
        let record = match std::fs::read(self.base.join(format!("{id}.json"))) {
            Ok(raw) => {
                let v: Value = serde_json::from_slice(&raw).map_err(|_| "transfer_record_json")?;
                let r = decode(&v)?;
                if r.id != id { return Err("transfer_record_identity".into()); }
                Some(r)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err("transfer_record_unreadable".into()),
        };
        self.records.insert(id, record.clone());
        Ok(record)
    }

    fn index_ids(&self, kind: &str, key: &str) -> Result<Vec<String>, String> {
        let path = self.base.join("indexes").join(format!("{kind}-{}.json", sha256_hex(key)));
        let raw = match std::fs::read(path) {
            Ok(raw) => raw,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err("transfer_index_unreadable".into()),
        };
        let values: Value = serde_json::from_slice(&raw).map_err(|_| "transfer_index_json")?;
        values.as_array().ok_or("transfer_index_invalid")?.iter().map(|v| uuid_id(&string(v)?)).collect()
    }

    /// `list_incomplete`: registros fora das fases terminais, em ordem de nome de arquivo.
    fn incomplete(&mut self) -> Result<Vec<Record>, String> {
        let entries = match std::fs::read_dir(&self.base) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err("transfer_dir_unreadable".into()),
        };
        let mut stems: Vec<String> = entries.flatten().map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .filter_map(|p| p.file_stem().map(|s| s.to_string_lossy().into_owned())).collect();
        stems.sort();
        let mut out = Vec::new();
        for stem in stems {
            if let Some(r) = self.load(&stem)? && !r.terminal() { out.push(r); }
        }
        Ok(out)
    }
}

/// `cx_sessions.load(name) or hl_sessions.load(name)`.
fn session_meta(home: &Path, name: &str) -> Option<Map<String, Value>> {
    [crate::state::parked::codex_sidecar_path(name, home), crate::state::parked::sidecar_path(name, home)].into_iter()
        .find_map(|p| serde_json::from_slice::<Value>(&std::fs::read(p).ok()?).ok()?.as_object().cloned().filter(|m| !m.is_empty()))
}

fn same_file(a: &str, b: &str) -> bool {
    let real = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| super::links::resolve_lenient(Path::new(p)));
    real(a) == real(b)
}

/// `_belongs`: o registro é desta sessão (pela marca no sidecar, pela vida ou pelo transcript).
fn belongs(r: &Record, life: Option<&str>, meta: Option<&Map<String, Value>>, source_path: Option<&str>) -> bool {
    let marked = meta.and_then(|m| m.get("transfer_id")).and_then(Value::as_str) == Some(r.id.as_str());
    if r.phase == "complete" {
        let dest_life = r.destination.as_ref().and_then(|d| d.get("lifecycle_id")).and_then(Value::as_str);
        if !marked && (life.is_none_or(str::is_empty) || life != dest_life) { return false; }
        if let Some(m) = meta.filter(|m| m.get("provider").and_then(Value::as_str) == Some("codex")) {
            let Some((home, thread)) = r.destination() else { return false };
            if m.get("thread_id").and_then(Value::as_str) != Some(thread.as_str()) { return false; }
            return match m.get("codex_home").and_then(text) {
                None => true,
                Some(h) => thread_key(h, &thread) == thread_key(&home, &thread),
            };
        }
        return true;
    }
    if marked { return true; }
    if life.is_none() || life != Some(r.source_life.as_str()) { return false; }
    let origin_sid = r.origin.get("session_id").and_then(text);
    if let Some(sp) = source_path.filter(|s| !s.is_empty()) {
        let origin_path = r.origin.get("jsonl").and_then(text);
        if let Some(op) = origin_path && !same_file(sp, op) { return false; }
        if origin_path.is_none() && let Some(sid) = origin_sid
            && Path::new(sp).file_stem().map(|s| s.to_string_lossy()) != Some(sid.into()) { return false; }
    }
    if let (Some(m), Some(sid)) = (meta, origin_sid)
        && let Some(own) = m.get("session_id").and_then(text) && own != sid { return false; }
    true
}

/// `transfer_for_session` com a vida dada (a da linha).
fn for_session(reader: &mut Reader, home: &Path, row: &SessionRow) -> Result<Option<Record>, String> {
    let meta = session_meta(home, &row.name);
    let life = row.lifecycle_id.as_deref();
    let source = row.jsonl.as_deref();
    if let Some(id) = meta.as_ref().and_then(|m| m.get("transfer_id")).and_then(text) {
        let r = reader.load(id)?.ok_or("transfer_record_missing")?;
        if r.name == row.name && belongs(&r, life, meta.as_ref(), source) { return Ok(Some(r)); }
    }
    for id in reader.index_ids("session", &row.name)?.iter().rev() {
        if let Some(r) = reader.load(id)? && r.name == row.name && belongs(&r, life, meta.as_ref(), source) {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

/// `transfer_for_thread`.
fn for_thread(reader: &mut Reader, codex_home: &str, thread: &str) -> Result<Option<Record>, String> {
    let key = thread_key(codex_home, thread);
    for id in reader.index_ids("thread", &key)?.iter().rev() {
        if let Some(r) = reader.load(id)? && r.destination().is_some_and(|(h, t)| thread_key(&h, &t) == key) {
            return Ok(Some(r));
        }
    }
    Ok(None)
}

/// As linhas que a transferência marca, substitui ou cria, e as em curso (fora de classificação
/// e decoração), como `list_facts._transfers`.
pub fn transfers(rows: &[SessionRow], dirs: &Dirs) -> Result<(Vec<SessionRow>, HashSet<String>), String> {
    let mut reader = Reader::new(&dirs.home);
    let mut out: Vec<SessionRow> = rows.to_vec();
    for row in out.iter_mut() {
        let mut record = for_session(&mut reader, &dirs.home, row)?;
        if record.is_none() && row.provider == "codex" && let Some(home) = row.codex_home.clone().filter(|h| !h.is_empty())
            && let Some(thread) = session_meta(&dirs.home, &row.name).and_then(|m| m.get("thread_id").and_then(text).map(str::to_owned))
        {
            record = for_thread(&mut reader, &home, &thread)?.filter(|r| r.name == row.name && !r.terminal());
        }
        if let Some(r) = record {
            row.transfer_id = Some(r.id.clone());
            row.transfer_phase = Some(r.phase.clone());
        }
    }
    for r in reader.incomplete()? {
        let at = out.iter().position(|row| row.name == r.name);
        // Uma sessão recriada com o nome antigo não pertence à recuperação.
        if let Some(i) = at && out[i].transfer_id.as_deref() != Some(r.id.as_str()) { continue; }
        let i = at.unwrap_or_else(|| { out.push(super::links::blank_row(&r.name)); out.len() - 1 });
        let row = &mut out[i];
        let origin = &r.origin;
        row.cwd = origin.get("cwd").and_then(Value::as_str).map(str::to_owned);
        row.jsonl = r.source_path.clone().or_else(|| origin.get("jsonl").and_then(Value::as_str).map(str::to_owned));
        row.provider = "claude".into();
        row.headless = origin.get("headless").and_then(Value::as_bool).unwrap_or(false);
        row.engine = origin.get("engine").and_then(Value::as_str).map(str::to_owned);
        row.codex_home = None;
        let cdir = origin.get("config_dir").and_then(text).map(PathBuf::from).unwrap_or_else(|| dirs.home.join(".claude"));
        row.conta = Some(format!("claude:{}", super::links::resolve_lenient(&cdir).to_string_lossy()));
        row.lifecycle_id = Some(r.source_life.clone());
        row.transfer_id = Some(r.id.clone());
        row.transfer_phase = Some(r.phase.clone());
        row.problema = r.error_code.clone();
        super::links::fill_links(row, dirs, &mut Problems::default());
    }
    let touched: Vec<SessionRow> = out.into_iter().filter(|r| r.transfer_id.is_some()).collect();
    let frozen = touched.iter().filter(|r| r.transfer_phase.as_deref().is_some_and(|p| !TERMINAL.contains(&p)))
        .map(|r| r.name.clone()).collect();
    Ok((touched, frozen))
}
