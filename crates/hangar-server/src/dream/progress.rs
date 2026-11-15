//! O andamento da rodada para a tela: um retrato em memória que o `events` manda a cada mudança, e os
//! passos de cada chamada gravados na pasta da rodada para o "Como sonhou".

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{LazyLock, Mutex};

use chrono::{DateTime, Utc};
use hangar_dream::run::{CallKind, Observer};
use hangar_dream::steps::{self, Step};
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

/// `None` = nenhuma rodada nesta máquina.
pub(crate) static PROGRESS: LazyLock<watch::Sender<Option<Progress>>> = LazyLock::new(|| watch::channel(None).0);
/// O Parar. Fica ligado até a rodada acabar: chamada que ainda esperava na fila também o vê.
pub(crate) static STOP: LazyLock<watch::Sender<bool>> = LazyLock::new(|| watch::channel(false).0);

pub const USER_PROJECT: &str = "user";

#[derive(Clone, Debug, Serialize)]
pub struct Progress {
    pub run_id: String,
    pub started_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub spent_usd: f64,
    pub stopping: bool,
    pub current: Option<usize>,
    pub projects: Vec<ProjectProgress>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Queued,
    Reading,
    Analyzing,
    Organizing,
    Writing,
    Done,
    Failed,
    Skipped,
    Stopped,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectProgress {
    pub id: String,
    pub name: String,
    pub status: Status,
    pub calls_total: usize,
    pub calls_done: usize,
    pub cost_usd: f64,
    pub error: Option<String>,
    pub notes: Option<NoteCounts>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct NoteCounts {
    pub new: usize,
    pub changed: usize,
    pub removed: usize,
}

impl NoteCounts {
    pub fn between(before: &std::collections::BTreeMap<String, String>, after: &std::collections::BTreeMap<String, String>) -> Self {
        Self {
            new: after.keys().filter(|k| !before.contains_key(*k)).count(),
            changed: after.iter().filter(|(k, v)| before.get(*k).is_some_and(|b| b != *v)).count(),
            removed: before.keys().filter(|k| !after.contains_key(*k)).count(),
        }
    }
}

pub fn begin(run_id: &str, projects: Vec<(String, String)>) {
    let now = Utc::now();
    let projects = projects
        .into_iter()
        .map(|(id, name)| ProjectProgress { id, name, status: Status::Queued, calls_total: 0, calls_done: 0, cost_usd: 0.0, error: None, notes: None })
        .collect();
    PROGRESS.send_replace(Some(Progress { run_id: run_id.into(), started_at: now, last_activity_at: now, spent_usd: 0.0, stopping: false, current: None, projects }));
}

pub fn end() {
    PROGRESS.send_replace(None);
}

pub fn stopping() -> bool {
    *STOP.borrow()
}

pub fn stop() {
    STOP.send_replace(true);
    PROGRESS.send_modify(|p| {
        if let Some(p) = p {
            p.stopping = true;
        }
    });
}

/// Mexe no projeto `idx` e marca a atividade; o projeto em andamento vira o `current`.
pub fn project(idx: usize, f: impl FnOnce(&mut ProjectProgress)) {
    PROGRESS.send_modify(|p| {
        let Some(p) = p else { return };
        if let Some(project) = p.projects.get_mut(idx) {
            let before = project.cost_usd;
            f(project);
            p.spent_usd += project.cost_usd - before;
            if matches!(project.status, Status::Reading | Status::Analyzing | Status::Organizing | Status::Writing) {
                p.current = Some(idx);
            }
        }
        p.last_activity_at = Utc::now();
    });
}

fn touch() {
    PROGRESS.send_modify(|p| {
        if let Some(p) = p {
            p.last_activity_at = Utc::now();
        }
    });
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CallMeta {
    pub n: usize,
    pub kind: String,
    pub label: String,
    pub model: String,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub cost_usd: Option<f64>,
    pub error: Option<String>,
    pub steps: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub prompt: String,
}

/// Grava as chamadas de um projeto: `<n>.json` (CallMeta com o pedido) e `<n>.steps.jsonl`.
pub struct Recorder {
    dir: PathBuf,
    workdir: PathBuf,
    project: usize,
    next: AtomicUsize,
    calls: Mutex<Vec<CallMeta>>,
}

impl Recorder {
    pub fn new(dir: PathBuf, workdir: &Path, project: usize) -> Self {
        Self { dir, workdir: workdir.to_path_buf(), project, next: AtomicUsize::new(0), calls: Mutex::new(Vec::new()) }
    }

    fn save(&self, meta: &CallMeta) {
        // Falhar aqui não pode derrubar o sonho: a tela só perde o detalhe desta chamada.
        let saved = std::fs::create_dir_all(&self.dir)
            .and_then(|()| serde_json::to_vec(meta).map_err(std::io::Error::other))
            .and_then(|bytes| hangar_core::fs::atomic_write(&self.dir.join(format!("{}.json", meta.n)), &bytes));
        if saved.is_err() {
            tracing::warn!(target: "dream", codigo = "dream_call_unrecorded", "passo do sonho não gravado");
        }
    }

    fn append(&self, n: usize, steps: &[Step]) -> std::io::Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(self.dir.join(format!("{n}.steps.jsonl")))?;
        for step in steps {
            let mut line = serde_json::to_vec(step).map_err(std::io::Error::other)?;
            line.push(b'\n');
            file.write_all(&line)?;
        }
        Ok(())
    }
}

fn scrub(step: Step) -> Step {
    let clean = |t: String| hangar_dream::secrets::scrub(&t).into_owned();
    match step {
        Step::Text { text } => Step::Text { text: clean(text) },
        Step::Tool { tool, target, detail } => Step::Tool { tool, target, detail: detail.map(clean) },
    }
}

impl Observer for Recorder {
    fn started(&self, kind: CallKind, label: &str, model: &str, prompt: &str) -> usize {
        let n = self.next.fetch_add(1, Ordering::SeqCst);
        let meta = CallMeta {
            n,
            kind: kind.as_str().into(),
            label: label.into(),
            model: model.into(),
            status: "running".into(),
            started_at: Utc::now(),
            ended_at: None,
            cost_usd: None,
            error: None,
            steps: 0,
            prompt: prompt.into(),
        };
        self.save(&meta);
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(meta);
        let status = if kind == CallKind::Analyzer { Status::Analyzing } else { Status::Organizing };
        project(self.project, |p| p.status = status);
        n
    }

    fn line(&self, n: usize, line: &str) {
        let parsed: Vec<Step> = steps::parse(line, &self.workdir).into_iter().map(scrub).collect();
        if !parsed.is_empty() {
            if self.append(n, &parsed).is_err() {
                tracing::warn!(target: "dream", codigo = "dream_call_unrecorded", "passo do sonho não gravado");
            }
            if let Some(meta) = self.calls.lock().unwrap_or_else(|e| e.into_inner()).iter_mut().find(|m| m.n == n) {
                meta.steps += parsed.len();
            }
        }
        touch();
    }

    fn finished(&self, n: usize, cost_usd: Option<f64>, error: Option<&str>) {
        let meta = {
            let mut calls = self.calls.lock().unwrap_or_else(|e| e.into_inner());
            let Some(meta) = calls.iter_mut().find(|m| m.n == n) else { return };
            meta.status = if error.is_some() { "failed" } else { "done" }.into();
            meta.ended_at = Some(Utc::now());
            meta.cost_usd = cost_usd;
            meta.error = error.map(super::code_of);
            meta.clone()
        };
        self.save(&meta);
        project(self.project, |p| {
            p.calls_done += 1;
            p.cost_usd += cost_usd.unwrap_or(0.0);
        });
    }
}

/// A pasta das chamadas de um projeto numa rodada.
pub(crate) fn calls_dir(runs: &Path, run_id: &str, project: &str) -> PathBuf {
    runs.join(run_id).join("calls").join(super::round::project_stem(project))
}

/// As chamadas gravadas, sem o pedido. Chamada que ficou `running` numa rodada que não está mais no ar
/// (servidor caiu no meio) sai como falha.
pub(crate) fn list_calls(dir: &Path, live: bool) -> std::io::Result<Vec<CallMeta>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".steps.jsonl") || !name.ends_with(".json") {
            continue;
        }
        let Ok(mut meta) = serde_json::from_slice::<CallMeta>(&std::fs::read(entry.path())?) else { continue };
        meta.prompt.clear();
        if meta.status == "running" {
            meta.steps = read_steps(dir, meta.n)?.len();
            if !live {
                meta.status = "failed".into();
                meta.error = Some("dream_interrupted".into());
            }
        }
        out.push(meta);
    }
    out.sort_by_key(|m| m.n);
    Ok(out)
}

pub(crate) fn read_call(dir: &Path, n: usize) -> std::io::Result<CallMeta> {
    let bytes = std::fs::read(dir.join(format!("{n}.json")))?;
    serde_json::from_slice(&bytes).map_err(std::io::Error::other)
}

pub(crate) fn read_steps(dir: &Path, n: usize) -> std::io::Result<Vec<Step>> {
    match std::fs::read_to_string(dir.join(format!("{n}.steps.jsonl"))) {
        Ok(text) => Ok(text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}
