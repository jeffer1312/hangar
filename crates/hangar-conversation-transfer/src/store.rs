use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};

use hangar_core::pycompat::pyjson;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::TransferError;

type Result<T> = std::result::Result<T, TransferError>;
const META_FIELDS: &[&str] = &[
    "name",
    "provider",
    "cwd",
    "jsonl",
    "session_id",
    "key",
    "config_dir",
    "headless",
    "engine",
    "model",
    "effort",
    "context_window",
    "permission_mode",
    "previous_non_plan",
    "subagent_model",
    "jev",
    "thread_id",
    "rollout_path",
    "codex_home",
    "codex_account",
    "lifecycle_id",
    "endpoint",
    "app_pid",
    "tui_pid",
    "launcher_pid",
    "pane_id",
    "pane_pid",
    "tool_output_token_limit",
    "transfer_id",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferPhase {
    Preparing,
    SourceStopped,
    Imported,
    Publishing,
    Complete,
    Restoring,
    RestoreFailed,
    Rejected,
    RolledBack,
}
impl TransferPhase {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Complete | Self::Rejected | Self::RolledBack)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConversationSource {
    pub path: String,
    pub provider: String,
    pub digest: String,
    pub selected_uuids: Vec<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportBoundary {
    pub thread_id: String,
    pub rollout_path: String,
    pub min_offset: u64,
    pub imported_item_ids: Vec<String>,
    #[serde(default)]
    pub prefix_digest: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TransferRecord {
    pub id: String,
    pub name: String,
    pub source_life: String,
    pub phase: TransferPhase,
    pub source: Option<ConversationSource>,
    pub origin_meta: Value,
    pub destination_meta: Option<Value>,
    pub boundary: Option<ImportBoundary>,
    pub error_code: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Store {
    base: PathBuf,
}

#[derive(Debug, Default)]
pub struct IncompleteScan {
    pub records: Vec<TransferRecord>,
    pub invalid_ids: Vec<String>,
}
impl Store {
    pub fn new(base: PathBuf) -> Self {
        Self { base }
    }
    pub fn base(&self) -> &Path {
        &self.base
    }
    pub fn load(&self, id: &str) -> Result<Option<TransferRecord>> {
        let id = canonical_id(id)?;
        let raw = match hangar_core::fs::read_replaced(&self.base.join(format!("{id}.json"))) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let record = decode_record(&raw, &id)?;
        Ok(Some(record))
    }
    pub fn save(&self, record: &TransferRecord) -> Result<()> {
        let mut record = record.clone();
        record.validate()?;
        let _lock = self.lock()?;
        let previous = self.load(&record.id)?;
        if !record.origin_meta["key"]
            .as_str()
            .is_some_and(|s| !s.is_empty())
        {
            record.origin_meta["key"] = previous
                .as_ref()
                .map(|r| r.origin_meta["key"].clone())
                .filter(|v| v.as_str().is_some_and(|s| !s.is_empty()))
                .unwrap_or_else(|| Value::String(hangar_core::ids::random_hex(16)));
        } else if previous
            .as_ref()
            .is_some_and(|r| r.origin_meta["key"] != record.origin_meta["key"])
        {
            return Err(invalid());
        }
        if previous
            .as_ref()
            .is_some_and(|r| r.source_life != record.source_life)
        {
            return Err(invalid());
        }
        self.save_locked(&record)
    }
    /// Inicia a restauração; E/S indisponível permite tentar salvar a origem e volta para diagnóstico.
    /// Identidade divergente continua sendo uma recusa, nunca uma autorização de sobrescrita.
    pub fn begin_restore(&self, record: &mut TransferRecord) -> Result<Option<TransferError>> {
        record.phase = TransferPhase::Restoring;
        match self.save_recovery(record) {
            Ok(()) => Ok(None),
            Err(failure) if failure.code == "session_transfer_history_invalid" => Ok(Some(failure)),
            Err(failure) => Err(failure),
        }
    }

    /// Confirma a fase durável depois do efeito de restauração, preservando sua causa pública.
    pub fn finish_restore(&self, record: &mut TransferRecord, restored: Result<()>) -> Result<()> {
        match restored {
            Ok(()) => {
                record.phase = TransferPhase::RolledBack;
                self.save_recovery(record)
            }
            Err(cause) => {
                record.phase = TransferPhase::RestoreFailed;
                record.error_code = Some(cause.code.clone());
                let mut params = json!({"phase": cause.code});
                if let Err(failure) = self.save_recovery(record) {
                    params["save"] = json!(failure.code);
                }
                Err(TransferError::new("session_transfer_restore_failed").with_params(params))
            }
        }
    }

    /// Recupera apenas a operação já conhecida pelo coordenador, preservando o arquivo inválido.
    /// Erro de leitura ou identidade divergente nunca autoriza substituir o registro.
    pub fn save_recovery(&self, record: &TransferRecord) -> Result<()> {
        let mut record = record.clone();
        record.validate()?;
        if !matches!(
            record.phase,
            TransferPhase::Restoring | TransferPhase::RestoreFailed | TransferPhase::RolledBack
        ) || !record.origin_meta["key"]
            .as_str()
            .is_some_and(|key| !key.is_empty())
        {
            return Err(invalid());
        }
        let _lock = self.lock()?;
        let path = self.base.join(format!("{}.json", record.id));
        let raw = match hangar_core::fs::read_replaced(&path) {
            Ok(raw) => Some(raw),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if let Some(raw) = raw {
            // Mesmo um esquema incompleto pode provar que o arquivo já pertence a outra operação.
            if let Ok(value) = serde_json::from_slice::<Value>(&raw) {
                for (actual, expected) in [
                    (&value["id"], json!(record.id)),
                    (&value["name"], json!(record.name)),
                    (&value["source_life"], json!(record.source_life)),
                    (
                        &value["origin_meta"]["key"],
                        record.origin_meta["key"].clone(),
                    ),
                ] {
                    if !actual.is_null() && actual != &expected {
                        return Err(invalid());
                    }
                }
            }
            if decode_record(&raw, &record.id).is_err() {
                let quarantine =
                    self.base
                        .join("invalid")
                        .join(format!("{}-{}.json", record.id, digest(&raw)));
                write_bytes(&quarantine, &raw)?;
            }
        }
        self.save_locked(&record)
    }

    pub fn capture_snapshot(
        &self,
        record: &TransferRecord,
        source_path: &Path,
        selected_uuids: Vec<String>,
    ) -> Result<TransferRecord> {
        if record.phase != TransferPhase::SourceStopped
            || selected_uuids.iter().any(String::is_empty)
        {
            return Err(invalid());
        }
        let raw = std::fs::read(source_path)?;
        let hash = digest(&raw);
        let path = self
            .base
            .join("snapshots")
            .join(format!("{}-{hash}.jsonl", canonical_id(&record.id)?));
        let _lock = self.lock()?;
        let previous = self.load(&record.id)?;
        if previous.as_ref().is_some_and(|p| {
            p.phase != TransferPhase::SourceStopped
                || p.source_life != record.source_life
                || p.name != record.name
        }) {
            return Err(invalid());
        }
        let mut captured = record.clone();
        captured.origin_meta = prepare_origin(
            previous
                .as_ref()
                .map(|p| &p.origin_meta)
                .unwrap_or(&record.origin_meta),
        )?;
        write_bytes(&path, &raw)?;
        if digest(&hangar_core::fs::read_replaced(&path)?) != hash {
            return Err(invalid());
        }
        captured.source = Some(ConversationSource {
            path: path.to_string_lossy().into_owned(),
            provider: "claude".into(),
            digest: hash,
            selected_uuids,
        });
        captured.validate()?;
        self.save_locked(&captured)?;
        Ok(captured)
    }
    pub fn for_thread(&self, home: &str, thread: &str) -> Result<Option<TransferRecord>> {
        let key = thread_key(home, thread);
        for id in self.index_ids("thread", &key)?.into_iter().rev() {
            if let Some(record) = self.load(&id)? {
                if record
                    .destination()
                    .is_some_and(|(h, t)| thread_key(h, t) == key)
                {
                    return Ok(Some(record));
                }
            }
        }
        Ok(None)
    }
    pub fn for_session(
        &self,
        name: &str,
        life: Option<&str>,
        meta: Option<&Value>,
        source_path: Option<&Path>,
        parked: bool,
    ) -> Result<Option<TransferRecord>> {
        if let Some(id) = meta
            .and_then(|m| m["transfer_id"].as_str())
            .filter(|s| !s.is_empty())
        {
            let record = self.load(id)?.ok_or_else(invalid)?;
            if record.name == name && record.belongs(life, meta, source_path) {
                return Ok(Some(record));
            }
        }
        for id in self.index_ids("session", name)?.into_iter().rev() {
            if let Some(record) = self.load(&id)? {
                if record.name == name {
                    let association =
                        if parked && life.is_none() && meta.is_none() && !record.phase.terminal() {
                            Some(record.source_life.as_str())
                        } else {
                            life
                        };
                    if record.belongs(association, meta, source_path) {
                        return Ok(Some(record));
                    }
                }
            }
        }
        Ok(None)
    }
    /// Consulta só o índice da sessão; corrupção de outra conversa não bloqueia esta.
    pub fn incomplete_for_session(&self, name: &str) -> Result<Vec<TransferRecord>> {
        let mut records = Vec::new();
        for id in self.index_ids("session", name)? {
            if let Some(record) = self.load(&id)? {
                if record.name == name && !record.phase.terminal() {
                    records.push(record);
                }
            }
        }
        Ok(records)
    }

    pub fn list_incomplete(&self) -> Result<Vec<TransferRecord>> {
        let scan = self.scan_incomplete()?;
        if !scan.invalid_ids.is_empty() {
            return Err(invalid());
        }
        Ok(scan.records)
    }

    /// A recuperação recebe os registros válidos e a identidade de cada arquivo recusado.
    pub fn scan_incomplete(&self) -> Result<IncompleteScan> {
        let entries = match std::fs::read_dir(&self.base) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(IncompleteScan::default());
            }
            Err(e) => return Err(e.into()),
        };
        let mut paths = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.extension().is_some_and(|e| e == "json") {
                paths.push(path);
            }
        }
        paths.sort();
        let mut scan = IncompleteScan::default();
        for path in paths {
            let id = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .ok_or_else(invalid)?;
            match self.load(&id) {
                Ok(Some(record)) if !record.phase.terminal() => scan.records.push(record),
                Ok(_) => {}
                Err(_) => scan.invalid_ids.push(id),
            }
        }
        Ok(scan)
    }
    pub fn runtime(&self, id: &str) -> Result<Value> {
        Ok(serde_json::from_slice(&hangar_core::fs::read_replaced(
            &self.runtime_path(id)?,
        )?)?)
    }
    pub fn write_runtime(&self, id: &str, value: &Value) -> Result<()> {
        write_json(&self.runtime_path(id)?, value)
    }
    pub fn prepare_runtime(&self, id: &str, original: Value, processes: Value) -> Result<()> {
        let _lock = self.lock()?;
        let path = self.runtime_path(id)?;
        if path.try_exists()? {
            return Err(TransferError::new("session_transfer_runtime_exists"));
        }
        write_json(&path, &json!({"original":original,"processes":processes}))
    }
    fn runtime_path(&self, id: &str) -> Result<PathBuf> {
        Ok(self
            .base
            .join("runtime")
            .join(format!("{}.json", canonical_id(id)?)))
    }
    fn lock(&self) -> Result<File> {
        private_dir(&self.base)?;
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(self.base.join(".lock"))?;
        file.lock()?;
        Ok(file)
    }
    fn index_path(&self, kind: &str, key: &str) -> PathBuf {
        self.base
            .join("indexes")
            .join(format!("{kind}-{}.json", digest(key.as_bytes())))
    }
    fn index_ids(&self, kind: &str, key: &str) -> Result<Vec<String>> {
        let raw = match hangar_core::fs::read_replaced(&self.index_path(kind, key)) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let values: Vec<String> = serde_json::from_slice(&raw)?;
        values.iter().map(|v| canonical_id(v)).collect()
    }
    fn append_index(&self, kind: &str, key: &str, id: &str) -> Result<()> {
        let mut ids = self.index_ids(kind, key)?;
        if !ids.iter().any(|v| v == id) {
            ids.push(id.to_owned());
            write_json(&self.index_path(kind, key), &json!(ids))?;
        }
        Ok(())
    }
    fn save_locked(&self, record: &TransferRecord) -> Result<()> {
        // O registro confirma a associação; um índice adiantado nunca publica a etapa seguinte.
        self.append_index("session", &record.name, &record.id)?;
        self.append_index("life", &record.source_life, &record.id)?;
        if let Some(life) = record
            .destination_meta
            .as_ref()
            .and_then(|v| v["lifecycle_id"].as_str())
            .filter(|s| !s.is_empty())
        {
            self.append_index("life", life, &record.id)?;
        }
        if let Some((home, thread)) = record.destination() {
            self.append_index("thread", &thread_key(home, thread), &record.id)?;
        }
        write_json(
            &self.base.join(format!("{}.json", record.id)),
            &serde_json::to_value(record)?,
        )
    }
}
impl TransferRecord {
    pub fn validate(&mut self) -> Result<()> {
        self.id = canonical_id(&self.id)?;
        if self.name.is_empty()
            || self.source_life.is_empty()
            || self.error_code.as_ref().is_some_and(String::is_empty)
        {
            return Err(invalid());
        }
        metadata(&self.origin_meta)?;
        if let Some(d) = &self.destination_meta {
            metadata(d)?;
        }
        if let Some(s) = &self.source {
            if s.path.is_empty()
                || s.provider != "claude"
                || !valid_digest(&s.digest)
                || s.selected_uuids.iter().any(String::is_empty)
            {
                return Err(invalid());
            }
        }
        if let Some(b) = &self.boundary {
            if b.thread_id.is_empty()
                || b.rollout_path.is_empty()
                || b.imported_item_ids.iter().any(String::is_empty)
                || b.prefix_digest.as_ref().is_some_and(|d| !valid_digest(d))
            {
                return Err(invalid());
            }
            if self
                .destination_meta
                .as_ref()
                .is_some_and(|d| !d["thread_id"].is_null() && d["thread_id"] != b.thread_id)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
    pub fn destination(&self) -> Option<(&str, &str)> {
        let meta = self.destination_meta.as_ref()?;
        let home = meta["codex_home"].as_str().filter(|s| !s.is_empty())?;
        let thread = meta["thread_id"]
            .as_str()
            .filter(|s| !s.is_empty())
            .or_else(|| self.boundary.as_ref().map(|b| b.thread_id.as_str()))?;
        Some((home, thread))
    }
    pub fn belongs(
        &self,
        life: Option<&str>,
        meta: Option<&Value>,
        source_path: Option<&Path>,
    ) -> bool {
        let marked = meta.is_some_and(|m| m["transfer_id"] == self.id);
        if self.phase == TransferPhase::Complete {
            if !marked
                && (life.is_none()
                    || life
                        != self
                            .destination_meta
                            .as_ref()
                            .and_then(|d| d["lifecycle_id"].as_str()))
            {
                return false;
            }
            if let Some(m) = meta.filter(|m| m["provider"] == "codex") {
                let Some((home, thread)) = self.destination() else {
                    return false;
                };
                if m["thread_id"] != thread {
                    return false;
                }
                return m["codex_home"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .is_none_or(|h| thread_key(h, thread) == thread_key(home, thread));
            }
            return true;
        }
        if marked {
            return true;
        }
        if life != Some(self.source_life.as_str()) {
            return false;
        }
        let sid = self.origin_meta["session_id"]
            .as_str()
            .filter(|s| !s.is_empty());
        if let Some(path) = source_path {
            if let Some(origin) = self.origin_meta["jsonl"].as_str().filter(|s| !s.is_empty()) {
                if resolve(path) != resolve(Path::new(origin)) {
                    return false;
                }
            } else if sid.is_some() && path.file_stem().and_then(|s| s.to_str()) != sid {
                return false;
            }
        }
        !meta.is_some_and(|m| {
            sid.is_some()
                && m["session_id"]
                    .as_str()
                    .is_some_and(|s| !s.is_empty() && Some(s) != sid)
        })
    }
}
pub fn prepare_origin(value: &Value) -> Result<Value> {
    metadata(value)?;
    let mut result = value.clone();
    if !result["key"].as_str().is_some_and(|s| !s.is_empty()) {
        result["key"] = Value::String(hangar_core::ids::random_hex(16));
    }
    Ok(result)
}
pub fn verify_source(source: &ConversationSource) -> Result<()> {
    if digest(&hangar_core::fs::read_replaced(Path::new(&source.path))?) != source.digest {
        return Err(TransferError::new("session_transfer_history_invalid"));
    }
    Ok(())
}
pub fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
pub fn canonical_id(value: &str) -> Result<String> {
    let raw = value
        .replace("urn:", "")
        .replace("uuid:", "")
        .trim_matches(['{', '}'])
        .replace('-', "");
    if raw.len() != 32 || !raw.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid());
    }
    let h = raw.to_ascii_lowercase();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    ))
}
pub fn map_permission(meta: &Value) -> Result<(&'static str, &'static str)> {
    let plan = meta["permission_mode"] == "plan";
    let mode = &meta[if plan {
        "previous_non_plan"
    } else {
        "permission_mode"
    }];
    let permission = match mode.as_str() {
        Some("bypassPermissions") => "Full Access",
        Some("manual" | "default") => "Ask for approval",
        Some("acceptEdits") => "Approve for me",
        _ => {
            return Err(TransferError::new(
                "session_transfer_invalid_permission_mode",
            ));
        }
    };
    Ok((permission, if plan { "plan" } else { "default" }))
}
fn invalid() -> TransferError {
    TransferError::new("session_transfer_invalid_record")
}
fn valid_digest(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|c| c.is_ascii_digit() || matches!(c, b'a'..=b'f'))
}
fn metadata(value: &Value) -> Result<()> {
    let map = value.as_object().ok_or_else(invalid)?;
    for (key, item) in map {
        if !META_FIELDS.contains(&key.as_str()) {
            return Err(invalid());
        }
        if item.is_null() {
            continue;
        }
        let valid = match key.as_str() {
            "headless" | "jev" => item.is_boolean(),
            "context_window"
            | "app_pid"
            | "tui_pid"
            | "launcher_pid"
            | "pane_pid"
            | "tool_output_token_limit" => item
                .as_number()
                .is_some_and(|n| !n.to_string().contains(['.', 'e', 'E'])),
            _ => item.is_string(),
        };
        if !valid {
            return Err(invalid());
        }
    }
    Ok(())
}
fn private_dir(path: &Path) -> Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path)?;
    Ok(())
}
fn write_bytes(path: &Path, raw: &[u8]) -> Result<()> {
    private_dir(path.parent().ok_or_else(invalid)?)?;
    hangar_core::fs::atomic_write(path, raw)?;
    Ok(())
}
fn write_json(path: &Path, value: &Value) -> Result<()> {
    write_bytes(path, pyjson::dumps_unicode(value, false).as_bytes())
}
fn thread_key(home: &str, thread: &str) -> String {
    let expanded = if home == "~" || home.starts_with("~/") || home.starts_with("~\\") {
        std::env::home_dir()
            .unwrap_or_default()
            .join(home[1..].trim_start_matches(['/', '\\']))
    } else {
        PathBuf::from(home)
    };
    pyjson::dumps_unicode(
        &json!([resolve(&expanded).to_string_lossy(), thread]),
        false,
    )
}
fn resolve(path: &Path) -> PathBuf {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    if let Ok(p) = std::fs::canonicalize(&absolute) {
        return hangar_core::fs::without_verbatim(p);
    }
    let mut result = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::ParentDir => {
                result.pop();
            }
            Component::CurDir => {}
            _ => {
                result.push(part.as_os_str());
                if let Ok(resolved) = std::fs::canonicalize(&result) {
                    result = hangar_core::fs::without_verbatim(resolved);
                }
            }
        }
    }
    result
}

fn decode_record(raw: &[u8], id: &str) -> Result<TransferRecord> {
    let value: Value = serde_json::from_slice(raw)?;
    let fields = [
        "id",
        "name",
        "source_life",
        "phase",
        "source",
        "origin_meta",
        "destination_meta",
        "boundary",
        "error_code",
    ];
    let object = value.as_object().ok_or_else(invalid)?;
    if object.len() != fields.len() || fields.iter().any(|field| !object.contains_key(*field)) {
        return Err(invalid());
    }
    let mut record: TransferRecord = serde_json::from_value(value)?;
    record.validate()?;
    if record.id != id {
        return Err(invalid());
    }
    Ok(record)
}
