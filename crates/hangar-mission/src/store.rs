//! `~/.hangar/mission/mission.json` e a pasta das evidências: trocado de forma atômica, sob trava.
use crate::model::{Evidence, Event, EventKind, Item, Snapshot, Stage, default_stages, now};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};

const FORMAT_VERSION: u32 = 1;
const MAX_STAGES: usize = 12;
const MAX_EVIDENCE: usize = 200;
/// Teto de um arquivo de evidência: cabe um vídeo curto de funcionamento.
pub const MAX_EVIDENCE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TEXT: usize = 4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind { Invalid, NotFound, Gate, Unavailable }

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error { pub kind: Kind, pub code: &'static str, pub message: String }

impl Error {
    fn new(kind: Kind, code: &'static str, message: impl Into<String>) -> Self { Self { kind, code, message: message.into() } }
    pub fn invalid(message: impl Into<String>) -> Self { Self::new(Kind::Invalid, "invalid_params", message) }
    fn io(action: &str, error: &io::Error) -> Self { Self::new(Kind::Unavailable, "mission_unavailable", format!("{action}: {:?}", error.kind())) }
    fn item_not_found() -> Self { Self::new(Kind::NotFound, "mission_item_not_found", "ticket sem registro no Mission Control") }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "{}: {}", self.code, self.message) }
}

#[derive(Serialize, Deserialize)]
struct File {
    version: u32,
    stages: Vec<Stage>,
    #[serde(default)]
    items: Vec<Item>,
}

impl Default for File {
    fn default() -> Self { Self { version: FORMAT_VERSION, stages: default_stages(), items: Vec::new() } }
}

/// Ticket que a sessão vai trabalhar, com o que mostrar dele sem perguntar ao conector.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Link {
    pub key: String,
    pub title: String,
    pub url: String,
    pub session: String,
}

/// Mudança feita pelo dono na tela.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum OwnerChange {
    /// Põe o ticket numa etapa, sem portão: o dono decide.
    Stage { stage: String },
    /// Aprova a etapa com portão em que o ticket está e o leva à seguinte.
    Approve,
    /// Recusa a etapa atual com o motivo: volta à anterior e desfaz a aprovação dela.
    Changes { note: String },
    /// Validação entregue: o ticket sai das etapas.
    Delivered {
        #[serde(default)]
        note: Option<String>,
    },
    /// Volta a deduzir a etapa (desfaz a escolha manual e a entrega).
    Reset,
}

/// O que a sessão informa pela tool `mission`.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub stage: Option<String>,
    pub note: Option<String>,
    pub evidence: Vec<(PathBuf, String)>,
}

pub struct Store { path: PathBuf, evidence_dir: PathBuf }

fn text(field: &str, value: &str, max: usize, empty_ok: bool) -> Result<(), Error> {
    if (!empty_ok && value.trim().is_empty()) || value.len() > max || value.contains('\0') {
        return Err(Error::invalid(format!("{field} vazio, com NUL ou acima de {max} bytes")));
    }
    Ok(())
}

fn valid_stage_id(id: &str) -> bool {
    let mut chars = id.chars();
    matches!(chars.next(), Some('a'..='z')) && id.len() <= 32 && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-'))
}

/// Mesmo formato dos nomes de sessão do Hangar.
pub fn valid_session(name: &str) -> bool {
    (1..=64).contains(&name.len()) && name.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

/// Tipo pelo nome: só imagem, vídeo, PDF e texto viram evidência.
pub fn evidence_mime(name: &str) -> Option<&'static str> {
    let ext = Path::new(name).extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain; charset=utf-8",
        _ => return None,
    })
}

fn event(kind: EventKind, by: &str, stage: Option<&str>, note: Option<&str>) -> Event {
    Event { at: now(), kind, by: by.to_owned(), stage: stage.map(str::to_owned), note: note.map(str::to_owned) }
}

/// Primeira etapa com portão antes de `target` ainda sem aprovação.
fn pending_gate<'a>(stages: &'a [Stage], item: &Item, target: usize) -> Option<&'a Stage> {
    stages[..target].iter().find(|stage| stage.gate && !item.approved.contains(&stage.id))
}

impl Store {
    /// `dir` guarda o `mission.json` e a pasta `evidence/`.
    pub fn new(dir: PathBuf) -> Self { Self { path: dir.join("mission.json"), evidence_dir: dir.join("evidence") } }

    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let file = self.read()?;
        Ok(Snapshot { stages: file.stages, items: file.items })
    }

    pub fn set_stages(&self, stages: Vec<Stage>) -> Result<Snapshot, Error> {
        if stages.is_empty() || stages.len() > MAX_STAGES { return Err(Error::invalid(format!("de 1 a {MAX_STAGES} etapas"))); }
        let mut seen = HashSet::new();
        for stage in &stages {
            if !valid_stage_id(&stage.id) { return Err(Error::invalid(format!("id de etapa fora do formato: {:?}", stage.id))); }
            if !seen.insert(stage.id.as_str()) { return Err(Error::invalid(format!("etapa repetida: {}", stage.id))); }
            text("name", &stage.name, 60, false)?;
            for status in &stage.statuses { text("statuses", status, 120, false)?; }
        }
        self.update(|file| {
            let known: HashSet<&str> = stages.iter().map(|s| s.id.as_str()).collect();
            // Etapa que saiu deixa de valer nos tickets: eles voltam a ser deduzidos.
            for item in &mut file.items {
                if item.stage.as_deref().is_some_and(|id| !known.contains(id)) { item.stage = None; }
                item.approved.retain(|id| known.contains(id.as_str()));
            }
            file.stages = stages;
            Ok((Snapshot { stages: file.stages.clone(), items: file.items.clone() }, true))
        })
    }

    /// Liga o ticket à sessão. A sessão sai de outro ticket em que estivesse: uma sessão, um ticket.
    pub fn link(&self, connector: &str, ticket_id: &str, link: Link) -> Result<Item, Error> {
        text("key", &link.key, 1024, false)?;
        text("title", &link.title, 4096, true)?;
        text("url", &link.url, 4096, false)?;
        if !valid_session(&link.session) { return Err(Error::invalid("nome de sessão fora do formato")); }
        self.update(|file| {
            for other in file.items.iter_mut().filter(|i| i.session.as_deref() == Some(link.session.as_str())) {
                if other.connector != connector || other.ticket_id != ticket_id { other.session = None; }
            }
            let index = match file.items.iter().position(|i| i.connector == connector && i.ticket_id == ticket_id) {
                Some(index) => index,
                None => {
                    file.items.push(Item {
                        connector: connector.to_owned(), ticket_id: ticket_id.to_owned(), key: String::new(), title: String::new(),
                        url: String::new(), session: None, stage: None, approved: Vec::new(), history: Vec::new(),
                        evidence: Vec::new(), delivered_at: None,
                    });
                    file.items.len() - 1
                }
            };
            let item = &mut file.items[index];
            (item.key, item.title, item.url) = (link.key, link.title, link.url);
            if item.session.as_deref() != Some(link.session.as_str()) {
                item.history.push(event(EventKind::Linked, "dono", None, Some(&link.session)));
                item.session = Some(link.session);
            }
            Ok((item.clone(), true))
        })
    }

    /// Esquece o ticket, com as evidências dele.
    pub fn remove(&self, connector: &str, ticket_id: &str) -> Result<Item, Error> {
        let item = self.update(|file| {
            let index = file.items.iter().position(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::item_not_found)?;
            Ok((file.items.remove(index), true))
        })?;
        for evidence in &item.evidence { let _ = std::fs::remove_file(self.evidence_path(&evidence.id)); }
        Ok(item)
    }

    pub fn owner_change(&self, connector: &str, ticket_id: &str, change: OwnerChange) -> Result<Item, Error> {
        self.update(|file| {
            let stages = file.stages.clone();
            let item = file.items.iter_mut().find(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::item_not_found)?;
            let current = item.stage.as_deref().and_then(|id| stages.iter().position(|s| s.id == id));
            match change {
                OwnerChange::Stage { stage } => {
                    if !stages.iter().any(|s| s.id == stage) { return Err(Error::invalid(format!("etapa desconhecida: {stage}"))); }
                    item.history.push(event(EventKind::Stage, "dono", Some(&stage), None));
                    (item.stage, item.delivered_at) = (Some(stage), None);
                }
                OwnerChange::Approve => {
                    let Some(index) = current.filter(|i| stages[*i].gate) else {
                        return Err(Error::invalid("o ticket não está numa etapa com portão"));
                    };
                    let stage = &stages[index];
                    if !item.approved.contains(&stage.id) { item.approved.push(stage.id.clone()); }
                    item.history.push(event(EventKind::Approved, "dono", Some(&stage.id), None));
                    if let Some(next) = stages.get(index + 1) {
                        item.history.push(event(EventKind::Stage, "dono", Some(&next.id), None));
                        item.stage = Some(next.id.clone());
                    }
                }
                OwnerChange::Changes { note } => {
                    text("note", &note, MAX_TEXT, false)?;
                    let Some(index) = current else { return Err(Error::invalid("o ticket não tem etapa para recusar")) };
                    let back = &stages[index.saturating_sub(1)];
                    item.approved.retain(|id| id != &stages[index].id);
                    item.history.push(event(EventKind::Changes, "dono", Some(&stages[index].id), Some(&note)));
                    item.stage = Some(back.id.clone());
                    item.delivered_at = None;
                }
                OwnerChange::Delivered { note } => {
                    if let Some(note) = &note { text("note", note, MAX_TEXT, true)?; }
                    if let Some(gate) = pending_gate(&stages, item, stages.len()) {
                        return Err(Error::new(Kind::Gate, "mission_gate_pending", format!("falta aprovar a etapa {}", gate.name)));
                    }
                    item.history.push(event(EventKind::Delivered, "dono", None, note.as_deref()));
                    item.delivered_at = Some(now());
                }
                OwnerChange::Reset => { (item.stage, item.delivered_at) = (None, None); }
            }
            Ok((item.clone(), true))
        })
    }

    /// Relato da sessão: etapa, nota e evidências. O agente não passa de etapa com portão sem aprovação.
    pub fn report(&self, session: &str, report: Report) -> Result<Item, Error> {
        if let Some(note) = &report.note { text("note", note, MAX_TEXT, true)?; }
        let mut staged = Vec::new();
        for (path, caption) in &report.evidence {
            text("caption", caption, 500, true)?;
            staged.push((self.stage_evidence(path)?, caption.clone()));
        }
        let outcome = self.update(|file| {
            let stages = file.stages.clone();
            let item = file.items.iter_mut().find(|i| i.session.as_deref() == Some(session)).ok_or_else(|| Error::new(Kind::NotFound,
                "mission_session_not_linked", "esta sessão não está ligada a um ticket do Mission Control"))?;
            if let Some(stage) = &report.stage {
                let target = stages.iter().position(|s| &s.id == stage).ok_or_else(|| Error::invalid(format!(
                    "etapa desconhecida: {stage}; as etapas são {}", stages.iter().map(|s| s.id.as_str()).collect::<Vec<_>>().join(", "))))?;
                if let Some(gate) = pending_gate(&stages, item, target) {
                    return Err(Error::new(Kind::Gate, "mission_gate_pending",
                        format!("a etapa {} espera a aprovação do dono no Mission Control; anexe as evidências e aguarde", gate.name)));
                }
                if item.stage.as_deref() != Some(stage.as_str()) {
                    item.history.push(event(EventKind::Stage, session, Some(stage), report.note.as_deref()));
                    item.stage = Some(stage.clone());
                } else if let Some(note) = report.note.as_deref().filter(|n| !n.trim().is_empty()) {
                    item.history.push(event(EventKind::Note, session, Some(stage), Some(note)));
                }
            } else if let Some(note) = report.note.as_deref().filter(|n| !n.trim().is_empty()) {
                item.history.push(event(EventKind::Note, session, item.stage.as_deref(), Some(note)));
            }
            if item.evidence.len() + staged.len() > MAX_EVIDENCE { return Err(Error::invalid(format!("no máximo {MAX_EVIDENCE} evidências por ticket"))); }
            for (evidence, caption) in &staged {
                let evidence = Evidence { caption: caption.clone(), by: session.to_owned(), ..evidence.clone() };
                item.history.push(event(EventKind::Evidence, session, item.stage.as_deref(), Some(&evidence.name)));
                item.evidence.push(evidence);
            }
            Ok((item.clone(), true))
        });
        // Recusado: as cópias feitas antes da trava não ficam órfãs.
        if outcome.is_err() { for (evidence, _) in &staged { let _ = std::fs::remove_file(self.evidence_path(&evidence.id)); } }
        outcome
    }

    /// Evidência enviada pelo dono (bytes já lidos), no ticket.
    pub fn add_owner_evidence(&self, connector: &str, ticket_id: &str, name: &str, bytes: &[u8], caption: &str) -> Result<Item, Error> {
        text("caption", caption, 500, true)?;
        let evidence = self.store_bytes(name, bytes)?;
        let outcome = self.update(|file| {
            let item = file.items.iter_mut().find(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::item_not_found)?;
            if item.evidence.len() >= MAX_EVIDENCE { return Err(Error::invalid(format!("no máximo {MAX_EVIDENCE} evidências por ticket"))); }
            let evidence = Evidence { caption: caption.to_owned(), by: "dono".into(), ..evidence.clone() };
            item.history.push(event(EventKind::Evidence, "dono", item.stage.as_deref(), Some(&evidence.name)));
            item.evidence.push(evidence);
            Ok((item.clone(), true))
        });
        if outcome.is_err() { let _ = std::fs::remove_file(self.evidence_path(&evidence.id)); }
        outcome
    }

    pub fn remove_evidence(&self, connector: &str, ticket_id: &str, id: &str) -> Result<Item, Error> {
        let item = self.update(|file| {
            let item = file.items.iter_mut().find(|i| i.connector == connector && i.ticket_id == ticket_id).ok_or_else(Error::item_not_found)?;
            let index = item.evidence.iter().position(|e| e.id == id).ok_or_else(|| Error::new(Kind::NotFound, "mission_evidence_not_found", "evidência não encontrada"))?;
            item.evidence.remove(index);
            Ok((item.clone(), true))
        })?;
        let _ = std::fs::remove_file(self.evidence_path(id));
        Ok(item)
    }

    /// Arquivo e tipo de uma evidência registrada.
    pub fn evidence_file(&self, id: &str) -> Result<(PathBuf, Evidence), Error> {
        let file = self.read()?;
        let evidence = file.items.iter().flat_map(|i| i.evidence.iter()).find(|e| e.id == id).cloned()
            .ok_or_else(|| Error::new(Kind::NotFound, "mission_evidence_not_found", "evidência não encontrada"))?;
        Ok((self.evidence_path(id), evidence))
    }

    fn evidence_path(&self, id: &str) -> PathBuf { self.evidence_dir.join(id) }

    fn new_evidence(&self, name: &str, size: u64) -> Result<Evidence, Error> {
        let mime = evidence_mime(name).ok_or_else(|| Error::invalid(format!("{name}: só imagem, vídeo, PDF ou texto")))?;
        if size > MAX_EVIDENCE_BYTES { return Err(Error::invalid(format!("{name}: acima de {} MiB", MAX_EVIDENCE_BYTES / 1024 / 1024))); }
        std::fs::create_dir_all(&self.evidence_dir).map_err(|e| Error::io("pasta das evidências", &e))?;
        let id = sha1_smol::Sha1::from(format!("{name}\0{}\0{}", now(), std::process::id())).digest().to_string()[..20].to_owned();
        Ok(Evidence { id, name: name.to_owned(), mime: mime.to_owned(), size, caption: String::new(), at: now(), by: String::new() })
    }

    /// Copia o arquivo da sessão para a pasta das evidências: apagar o original depois não tira a prova.
    fn stage_evidence(&self, path: &Path) -> Result<Evidence, Error> {
        if !path.is_absolute() { return Err(Error::invalid(format!("{}: caminho precisa ser absoluto", path.display()))); }
        let meta = std::fs::metadata(path).map_err(|e| Error::invalid(format!("{}: {:?}", path.display(), e.kind())))?;
        if !meta.is_file() { return Err(Error::invalid(format!("{}: não é arquivo", path.display()))); }
        let name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::invalid("nome de arquivo inválido"))?;
        let evidence = self.new_evidence(name, meta.len())?;
        std::fs::copy(path, self.evidence_path(&evidence.id)).map_err(|e| Error::io("cópia da evidência", &e))?;
        Ok(evidence)
    }

    fn store_bytes(&self, name: &str, bytes: &[u8]) -> Result<Evidence, Error> {
        let name = Path::new(name).file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::invalid("nome de arquivo inválido"))?;
        text("name", name, 240, false)?;
        let evidence = self.new_evidence(name, bytes.len() as u64)?;
        hangar_core::fs::atomic_write(&self.evidence_path(&evidence.id), bytes).map_err(|e| Error::io("gravação da evidência", &e))?;
        Ok(evidence)
    }

    fn read(&self) -> Result<File, Error> {
        let bytes = match hangar_core::fs::read_replaced(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(File::default()),
            Err(error) => return Err(Error::io("leitura do Mission Control", &error)),
        };
        let file: File = serde_json::from_slice(&bytes)
            .map_err(|_| Error::new(Kind::Unavailable, "mission_unreadable", "mission.json ilegível"))?;
        if file.version != FORMAT_VERSION {
            return Err(Error::new(Kind::Unavailable, "mission_unreadable", format!("mission.json na versão {} desconhecida", file.version)));
        }
        Ok(file)
    }

    fn update<T>(&self, change: impl FnOnce(&mut File) -> Result<(T, bool), Error>) -> Result<T, Error> {
        let parent = self.path.parent().ok_or_else(|| Error::new(Kind::Unavailable, "mission_unavailable", "sem pasta"))?;
        std::fs::create_dir_all(parent).map_err(|e| Error::io("pasta do Mission Control", &e))?;
        let _lock = hangar_core::fs::lock_file(&self.path.with_extension("lock")).map_err(|e| Error::io("trava do Mission Control", &e))?;
        let mut file = self.read()?;
        let (value, changed) = change(&mut file)?;
        if changed {
            let bytes = serde_json::to_vec_pretty(&file).map_err(|_| Error::new(Kind::Unavailable, "mission_unavailable", "não serializou"))?;
            hangar_core::fs::atomic_write(&self.path, &bytes).map_err(|e| Error::io("gravação do Mission Control", &e))?;
        }
        Ok(value)
    }
}
