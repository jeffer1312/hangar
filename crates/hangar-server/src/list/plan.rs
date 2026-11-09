//! Progresso do plano do superpowers que a sessão executa (porte da leitura de
//! `backend/app/planprog.py`). A lista e as rotas de plano (`crate::plans`) usam o mesmo
//! rastreador: o plano eleito no painel é o da linha.
//!
//! Roda por sessão a cada rodada da lista, então tudo é cacheado e nada levanta: uma falha aqui
//! derrubaria a lista inteira. Arquivo ilegível é avisado no log e o plano some da linha.
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};
use std::time::{SystemTime, UNIX_EPOCH};

use hangar_api::session::SessionRow;

use super::capped::Capped;
use regex::bytes::Regex;

/// Quantos níveis subir do cwd atrás da pasta de planos; a subida para no primeiro `.git`, senão
/// uma worktree sem planos mostraria o plano do checkout principal.
const MAX_PARENTS: usize = 6;
/// Plano parado há mais de 14 dias não reaparece.
const MAX_AGE_S: f64 = 14.0 * 86400.0;
const DISCOVERY_TTL: f64 = 3.0;
const PIN_FILE: &str = "cp-plan-pin";
/// "Nenhum plano": um pin como os outros, que nenhum stem de plano real (`AAAA-MM-DD-*`) alcança.
pub const PIN_NONE: &str = "!none";
/// `registry._MAX_PLAN_TASK_SEGMENTS`: a barra segmentada não precisa de mais.
const MAX_TASK_SEGMENTS: usize = 9;

/// Um regex para descoberta, contagem e escrita: o checkbox que a escrita troca é o grupo 1.
pub static STEP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^- \[([ xX])\] \*\*(Step\b[^*]*)\*\*").unwrap());
static TASK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^### (Task\b[^\n]*)$").unwrap());
static MANUAL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)verifica(?:ção|cao|r)\s+manual|manual\s+verification").unwrap());
static DATE_PREFIX_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^\d{4}-\d{2}-\d{2}-").unwrap());

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct StepProgress {
    pub title: String,
    pub done: bool,
    pub manual: bool,
    /// Posição na ordem do documento, atravessando as Tasks: é a chave que o cliente devolve para
    /// marcar, porque o título se repete entre Tasks.
    pub idx: u32,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TaskProgress {
    pub title: String,
    pub done: u32,
    pub total: u32,
    pub steps: Vec<StepProgress>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub name: String,
    pub path: PathBuf,
    /// Posição ordinal (1-based) da primeira Task com step pendente.
    pub task_idx: u32,
    pub task_total: u32,
    pub done: u32,
    pub total: u32,
    pub complete: bool,
    pub tasks: Vec<TaskProgress>,
}

#[derive(Default)]
pub struct PlanTracker {
    /// (arquivo, exige começado) → (mtime, plano). O `None` memoriza "li e não serve".
    /// `Arc`: a decoração lê o plano a cada rodada e só usa os números; cópia seria dos títulos todos.
    files: Capped<(PathBuf, bool), (SystemTime, Option<Arc<Plan>>)>,
    /// pasta de planos → (relógio monotônico, eleito).
    discovery: Capped<PathBuf, (f64, Option<PathBuf>)>,
    /// Eleito anterior: mantém o posto enquanto tiver step pendente.
    sticky: Capped<PathBuf, PathBuf>,
}

impl PlanTracker {
    /// `plan_progress`: o plano que o painel mostra para o cwd, o mesmo eleito da lista.
    pub fn active(&mut self, cwd: &Path, wall: f64, mono: f64) -> Option<Arc<Plan>> {
        let root = plans_dir(cwd)?;
        let pin = read_pin(&root);
        if pin.as_deref() == Some(PIN_NONE) {
            return None;
        }
        self.progress(&root, pin.as_deref(), wall, mono)
    }

    /// `list_plans`: todos os planos da pasta para o seletor, inclusive os que a eleição descarta
    /// (não começados e completos), mais recente primeiro. `None` = repositório sem pasta de planos.
    pub fn catalog(&mut self, cwd: &Path) -> Option<serde_json::Value> {
        let root = plans_dir(cwd)?;
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(e) => {
                plan_unreadable(&e);
                return None;
            }
        };
        let mut items = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let file = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Some(stem) = file.strip_suffix(".md") else { continue };
            let got = match fs::metadata(&path) {
                // A variante do pin conta também o plano não começado, e fica no cache.
                Ok(meta) if meta.is_file() => meta.modified().and_then(|m| self.load(&path, m, false)),
                Ok(_) => continue,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => Err(e),
            };
            // Um arquivo ilegível sai do seletor sem derrubar a lista inteira, mas deixa rastro.
            let got = match got {
                Ok(got) => got,
                Err(e) => {
                    plan_unreadable(&e);
                    continue;
                }
            };
            items.push(serde_json::json!({
                "stem": stem,
                "name": DATE_PREFIX_RE.replace(stem, ""),
                "done": got.as_ref().map_or(0, |p| p.done),
                "total": got.as_ref().map_or(0, |p| p.total),
                "complete": got.as_ref().is_some_and(|p| p.complete),
            }));
        }
        items.sort_by(|a, b| b["stem"].as_str().cmp(&a["stem"].as_str()));
        Some(serde_json::json!({"plans": items, "pinned": read_pin(&root)}))
    }

    /// Pin trocado ou plano movido: a descoberta e o eleito dessa pasta são refeitos. O cache por
    /// arquivo se renova sozinho pelo mtime.
    pub fn forget_root(&mut self, root: &Path) {
        self.discovery.remove(root);
        self.sticky.remove(root);
    }

    /// `_decorate_plan`: preenche `plan_*` da linha, ou `plan_hidden` quando o repo escondeu.
    pub fn decorate(&mut self, row: &mut SessionRow, wall: f64, mono: f64) {
        let Some(root) = row.cwd.as_deref().filter(|c| !c.is_empty()).and_then(|c| plans_dir(Path::new(c)))
        else {
            return;
        };
        // Sem plano tem dois motivos; só o pin "nenhum" mantém o painel (e o seletor que desfaz).
        let pin = read_pin(&root);
        if pin.as_deref() == Some(PIN_NONE) {
            row.plan_hidden = Some(true);
            return;
        }
        let Some(p) = self.progress(&root, pin.as_deref(), wall, mono) else { return };
        row.plan_name = Some(p.name.clone());
        row.plan_task = Some(p.task_idx);
        row.plan_task_total = Some(p.task_total);
        row.plan_done = Some(p.done);
        row.plan_total = Some(p.total);
        row.plan_complete = Some(p.complete);
        row.plan_tasks = Some(p.tasks.iter().take(MAX_TASK_SEGMENTS).map(|t| (t.done, t.total)).collect());
    }

    fn progress(&mut self, root: &Path, pin: Option<&str>, wall: f64, mono: f64) -> Option<Arc<Plan>> {
        // Pin vence a eleição só enquanto tiver step pendente; completo, volta ao automático.
        if let Some(stem) = pin {
            let path = root.join(format!("{stem}.md"));
            if let Ok(Some(p)) = mtime(&path).and_then(|m| self.load(&path, m, false)) {
                if !p.complete {
                    return Some(p);
                }
            }
        }
        let path = match self.discovery.get(root) {
            Some((at, path)) if mono - at < DISCOVERY_TTL => path.clone(),
            _ => match self.discover(root, wall) {
                Ok(path) => {
                    self.discovery.insert(root.to_path_buf(), (mono, path.clone()));
                    path
                }
                Err(e) => {
                    if crate::warn_limit::allow(None, "list_plan_dir_unreadable") {
                        tracing::warn!(code = "list_plan_dir_unreadable", kind = ?e.kind(), "pasta de planos ilegivel");
                    }
                    super::facts::note("rust.list_plan_unreadable", &row_key(root), format!("list_plan_dir_unreadable:{:?}", e.kind()),
                        "pasta de planos ilegível; o plano some da linha");
                    return None;
                }
            },
        }?;
        let m = mtime(&path).ok()?;
        match self.load(&path, m, true) {
            Ok(p) => p,
            Err(e) => {
                plan_unreadable(&e);
                None
            }
        }
    }

    fn load(&mut self, path: &Path, mtime: SystemTime, require_started: bool) -> io::Result<Option<Arc<Plan>>> {
        // A chave leva o require_started: o mesmo arquivo dá None na eleição e plano pelo pin.
        let key = (path.to_path_buf(), require_started);
        if let Some((m, p)) = self.files.get(&key) {
            if *m == mtime {
                return Ok(p.clone());
            }
        }
        let got = parse(&fs::read(path)?, path, require_started).map(Arc::new);
        self.files.insert(key, (mtime, got.clone()));
        Ok(got)
    }

    /// Plano com step pendente de mtime mais novo; o eleito anterior fica enquanto andar.
    fn discover(&mut self, root: &Path, wall: f64) -> io::Result<Option<PathBuf>> {
        let mut cands: Vec<(SystemTime, PathBuf, bool)> = Vec::new();
        for entry in fs::read_dir(root)? {
            let path = entry?.path();
            if !path.file_name().is_some_and(|n| n.to_string_lossy().ends_with(".md")) {
                continue;
            }
            // Segue link como o `is_file()` do Python; sumido ou link quebrado é "não é arquivo".
            let meta = match fs::metadata(&path) {
                Ok(meta) => meta,
                Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
                Err(e) => {
                    plan_unreadable(&e);
                    continue;
                }
            };
            if !meta.is_file() {
                continue;
            }
            // Um arquivo ilegível não pode apagar o plano do repo inteiro.
            let loaded = meta.modified().and_then(|m| {
                if wall - secs(m) > MAX_AGE_S {
                    return Ok(None);
                }
                Ok(self.load(&path, m, true)?.map(|p| (m, p)))
            });
            match loaded {
                Ok(Some((m, p))) => cands.push((m, path, p.complete)),
                Ok(None) => {}
                Err(e) => plan_unreadable(&e),
            }
        }
        if let Some(prev) = self.sticky.get(root) {
            if cands.iter().any(|(_, p, complete)| p == prev && !complete) {
                return Ok(Some(prev.clone()));
            }
        }
        let any_pending = cands.iter().any(|c| !c.2);
        let mut chosen: Option<&(SystemTime, PathBuf, bool)> = None;
        for c in cands.iter().filter(|c| !any_pending || !c.2) {
            if chosen.is_none_or(|best| c.0 > best.0) {
                chosen = Some(c);
            }
        }
        let chosen = chosen.map(|c| c.1.clone());
        if let Some(path) = &chosen {
            self.sticky.insert(root.to_path_buf(), path.clone());
        }
        Ok(chosen)
    }
}

/// Roda a cada tique: um aviso por minuto, no log e no diário.
fn plan_unreadable(e: &io::Error) {
    if crate::warn_limit::allow(None, "list_plan_unreadable") {
        tracing::warn!(code = "list_plan_unreadable", kind = ?e.kind(), "plano ilegivel");
    }
    super::facts::note("rust.list_plan_unreadable", "", format!("list_plan_unreadable:{:?}", e.kind()), "plano ilegível; fica de fora da eleição");
}

/// A pasta de planos no diário: o repositório, sem o caminho inteiro.
fn row_key(root: &Path) -> String {
    root.ancestors().nth(3).and_then(Path::file_name).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

fn mtime(path: &Path) -> io::Result<SystemTime> {
    fs::metadata(path)?.modified()
}

fn secs(t: SystemTime) -> f64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

/// `docs/superpowers/plans` subindo do cwd, parando no primeiro nível com `.git`.
pub fn plans_dir(cwd: &Path) -> Option<PathBuf> {
    let mut cur = cwd;
    for _ in 0..=MAX_PARENTS {
        let cand = cur.join("docs").join("superpowers").join("plans");
        if cand.is_dir() {
            return Some(cand);
        }
        if cur.join(".git").exists() {
            return None;
        }
        cur = cur.parent()?;
    }
    None
}

/// Dentro do `.git/` do repo dono dos planos; `.git` arquivo (worktree) cai na pasta de planos.
pub fn pin_path(root: &Path) -> PathBuf {
    let mut cur = Some(root);
    for _ in 0..=MAX_PARENTS {
        let Some(dir) = cur else { break };
        let git = dir.join(".git");
        if git.is_dir() {
            return git.join(PIN_FILE);
        }
        cur = dir.parent();
    }
    root.join(format!(".{PIN_FILE}"))
}

/// Stem só, sem separador: o valor vira nome de arquivo, venha do disco ou do cliente.
pub fn safe_stem(v: &str) -> bool {
    !v.is_empty() && !v.contains(['/', '\\']) && v != "." && v != ".."
}

/// Stem fixado, ou None. Pin ilegível, inseguro ou de `.md` que sumiu é sem pin.
pub fn read_pin(root: &Path) -> Option<String> {
    let raw = match fs::read(pin_path(root)) {
        Ok(raw) => raw,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            // Calado, o plano fixado sumiria e a eleição automática trocaria o da linha.
            super::facts::note("rust.list_plan_unreadable", &row_key(root), format!("list_plan_pin_unreadable:{:?}", e.kind()),
                "pin do plano ilegível; vale a eleição automática");
            return None;
        }
    };
    let v = String::from_utf8_lossy(&raw).trim().to_owned();
    if !safe_stem(&v) {
        return None;
    }
    if v == PIN_NONE || root.join(format!("{v}.md")).is_file() {
        return Some(v);
    }
    None
}

/// Blocos cercados (``` ou ~~~) viram espaço, `\n` mantido: planos mostram steps de exemplo
/// dentro de bloco de código. Equivale a `^(```|~~~).*?^\1[^\n]*$` (M|S), sem retrorreferência.
pub fn blank_fences(b: &mut [u8]) {
    let eol = |b: &[u8], from: usize| b[from..].iter().position(|&c| c == b'\n').map_or(b.len(), |p| from + p);
    let mut i = 0;
    while i < b.len() {
        let marker: Option<&[u8]> = [b"```".as_slice(), b"~~~"].into_iter().find(|m| b[i..].starts_with(m));
        if let Some(m) = marker {
            let mut j = eol(b, i) + 1;
            while j < b.len() && !b[j..].starts_with(m) {
                j = eol(b, j) + 1;
            }
            if j < b.len() {
                let end = eol(b, j);
                b[i..end].iter_mut().filter(|c| **c != b'\n').for_each(|c| *c = b' ');
                i = end + 1;
                continue;
            }
        }
        i = eol(b, i) + 1;
    }
}

/// `parse_plan`: None sem step nenhum ou, com `require_started`, sem step marcado.
pub(crate) fn parse(raw: &[u8], path: &Path, require_started: bool) -> Option<Plan> {
    let mut text = String::from_utf8_lossy(raw).into_owned().into_bytes();
    blank_fences(&mut text);
    let steps: Vec<(usize, StepProgress)> = STEP_RE
        .captures_iter(&text)
        .enumerate()
        .map(|(i, c)| {
            let title = String::from_utf8_lossy(&c[2]).trim().to_owned();
            let manual = MANUAL_RE.is_match(&title);
            (c.get(0).map_or(0, |m| m.start()), StepProgress { done: &c[1] != b" ", manual, idx: i as u32, title })
        })
        .collect();
    if steps.is_empty() {
        return None;
    }
    let done = steps.iter().filter(|s| s.1.done).count();
    if done == 0 && require_started {
        return None;
    }
    let mut heads: Vec<(usize, String)> = TASK_RE
        .captures_iter(&text)
        .map(|c| (c.get(0).map_or(0, |m| m.start()), String::from_utf8_lossy(&c[1]).trim().to_owned()))
        .collect();
    // Steps antes da 1ª Task ganham uma Task implícita, senão somem da barra segmentada.
    match heads.first() {
        None => heads.push((0, "Task 1".to_owned())),
        Some(&(h, _)) if h > 0 && steps.iter().any(|s| s.0 < h) => heads.insert(0, (0, "(sem Task)".to_owned())),
        Some(_) => {}
    }
    let tasks: Vec<TaskProgress> = heads
        .iter()
        .enumerate()
        .map(|(i, (pos, title))| {
            let end = heads.get(i + 1).map_or(text.len(), |h| h.0);
            let mine: Vec<StepProgress> = steps.iter().filter(|s| *pos <= s.0 && s.0 < end).map(|s| s.1.clone()).collect();
            let done = mine.iter().filter(|s| s.done).count() as u32;
            TaskProgress { title: title.clone(), done, total: mine.len() as u32, steps: mine }
        })
        .collect();
    // Ordinal, não o N do título: existe "### Task 0".
    let task_idx = tasks.iter().position(|t| t.done < t.total).map_or(tasks.len(), |i| i + 1);
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    Some(Plan {
        name: DATE_PREFIX_RE.replace(&stem, "").into_owned(),
        path: path.to_path_buf(),
        task_idx: task_idx as u32,
        task_total: tasks.len() as u32,
        done: done as u32,
        total: steps.len() as u32,
        complete: done == steps.len(),
        tasks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(text: &str) -> Option<Plan> {
        parse(text.as_bytes(), Path::new("/x/2027-01-01-nome.md"), true)
    }

    #[test]
    fn fenced_steps_and_loose_steps() {
        let p = plan(
            "- [x] **Step 0: solto**\n\n### Task 1: A\n\n```md\n- [x] **Step 9: exemplo**\n```\n\
             - [ ] **Step 1: a**\n\n~~~\n- [x] **Step 8: aberto sem fechar**\n",
        )
        .unwrap();
        assert_eq!(p.name, "nome");
        assert_eq!((p.done, p.total), (2, 3)); // o ~~~ sem fechamento não esconde nada
        assert_eq!(p.tasks.iter().map(|t| (t.done, t.total)).collect::<Vec<_>>(), vec![(1, 1), (1, 2)]);
        assert_eq!((p.task_idx, p.task_total), (2, 2));
        assert_eq!(plan("```\n- [x] **Step 1: a**\n```\n- [ ] **Step 2: b**\n"), None);
    }

    /// Repositório com `.git` e pasta de planos; devolve (cwd, pasta de planos).
    fn repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let plans = repo.join("docs/superpowers/plans");
        fs::create_dir_all(&plans).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();
        (dir, repo, plans)
    }

    fn now() -> f64 { secs(SystemTime::now()) }

    #[test]
    fn active_plan_carries_titles_steps_manual_flag_and_document_index() {
        let (_dir, repo, plans) = repo();
        fs::write(plans.join("2026-07-29-plano.md"),
            "### Task 1: X\n\n- [x] **Step 1: A**\n- [ ] **Step 2: verificação manual**\n\n\
             ### Task 2: Y\n\n- [ ] **Step 1: Manual Verification of B**\n").unwrap();
        let p = PlanTracker::default().active(&repo, now(), 1.0).unwrap();
        assert_eq!(p.name, "plano");
        assert_eq!(p.path, plans.join("2026-07-29-plano.md"));
        assert_eq!((p.done, p.total, p.complete, p.task_idx, p.task_total), (1, 3, false, 1, 2));
        let titles: Vec<&str> = p.tasks.iter().map(|t| t.title.as_str()).collect();
        assert_eq!(titles, ["Task 1: X", "Task 2: Y"]);
        let steps: Vec<(&str, bool, bool, u32)> = p.tasks.iter().flat_map(|t| &t.steps)
            .map(|s| (s.title.as_str(), s.done, s.manual, s.idx)).collect();
        assert_eq!(steps, [("Step 1: A", true, false, 0), ("Step 2: verificação manual", false, true, 1),
            ("Step 1: Manual Verification of B", false, true, 2)]);
    }

    #[test]
    fn loose_steps_get_an_implicit_task_and_planless_steps_a_default_one() {
        let (_dir, repo, plans) = repo();
        fs::write(plans.join("2026-07-29-a.md"), "- [x] **Step 0: solto**\n\n### Task 1: A\r\n\n- [ ] **Step 1: a**\n").unwrap();
        let p = PlanTracker::default().active(&repo, now(), 1.0).unwrap();
        assert_eq!(p.tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["(sem Task)", "Task 1: A"]);
        fs::write(plans.join("2026-07-29-a.md"), "- [x] **Step 1: a**\n- [ ] **Step 2: b**\n").unwrap();
        let p = PlanTracker::default().active(&repo, now(), 1.0).unwrap();
        assert_eq!(p.tasks.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Task 1"]);
    }

    #[test]
    fn hidden_or_missing_folder_has_no_active_plan() {
        let (_dir, repo, plans) = repo();
        fs::write(plans.join("2026-07-29-a.md"), "- [x] **Step 1: a**\n- [ ] **Step 2: b**\n").unwrap();
        fs::write(repo.join(".git").join(PIN_FILE), "!none\n").unwrap();
        assert_eq!(PlanTracker::default().active(&repo, now(), 1.0), None);
        let empty = tempfile::tempdir().unwrap();
        fs::create_dir_all(empty.path().join(".git")).unwrap();
        assert_eq!(PlanTracker::default().active(empty.path(), now(), 1.0), None);
    }

    #[test]
    fn catalog_lists_unstarted_and_complete_plans_newest_first() {
        let (_dir, repo, plans) = repo();
        fs::write(plans.join("2026-07-01-pendente.md"), "- [x] **Step 1: a**\n- [ ] **Step 2: b**\n").unwrap();
        fs::write(plans.join("2026-07-02-nao-comecado.md"), "- [ ] **Step 1: a**\n- [ ] **Step 2: b**\n- [ ] **Step 3: c**\n").unwrap();
        fs::write(plans.join("2026-07-03-completo.md"), "- [x] **Step 1: a**\n").unwrap();
        fs::write(plans.join("notas.txt"), "- [x] **Step 1: a**\n").unwrap();
        let got = PlanTracker::default().catalog(&repo).unwrap();
        assert_eq!(got, serde_json::json!({"plans": [
            {"stem": "2026-07-03-completo", "name": "completo", "done": 1, "total": 1, "complete": true},
            {"stem": "2026-07-02-nao-comecado", "name": "nao-comecado", "done": 0, "total": 3, "complete": false},
            {"stem": "2026-07-01-pendente", "name": "pendente", "done": 1, "total": 2, "complete": false},
        ], "pinned": null}));
    }

    #[test]
    fn catalog_pin_is_null_when_its_plan_left_and_kept_when_hidden() {
        let (_dir, repo, plans) = repo();
        fs::write(plans.join("2026-07-01-pendente.md"), "- [x] **Step 1: a**\n- [ ] **Step 2: b**\n").unwrap();
        fs::write(repo.join(".git").join(PIN_FILE), "2026-07-09-sumiu\n").unwrap();
        assert_eq!(PlanTracker::default().catalog(&repo).unwrap()["pinned"], serde_json::Value::Null);
        fs::write(repo.join(".git").join(PIN_FILE), "!none\n").unwrap();
        assert_eq!(PlanTracker::default().catalog(&repo).unwrap()["pinned"], "!none");
        let empty = tempfile::tempdir().unwrap();
        fs::create_dir_all(empty.path().join(".git")).unwrap();
        assert_eq!(PlanTracker::default().catalog(empty.path()), None);
    }

    #[test]
    fn forgetting_the_folder_drops_the_sticky_choice() {
        let (_dir, repo, plans) = repo();
        let t0 = now();
        let write = |name: &str, age: f64| {
            let p = plans.join(name);
            fs::write(&p, "- [x] **Step 1: a**\n- [ ] **Step 2: b**\n").unwrap();
            fs::File::options().write(true).open(&p).unwrap()
                .set_modified(UNIX_EPOCH + std::time::Duration::from_secs_f64(t0 - age)).unwrap();
        };
        write("2027-01-01-a.md", 10.0);
        let mut t = PlanTracker::default();
        assert_eq!(t.active(&repo, t0, 100.0).unwrap().name, "a");
        write("2027-01-02-b.md", 1.0);
        assert_eq!(t.active(&repo, t0, 104.0).unwrap().name, "a", "sticky");
        t.forget_root(&plans);
        assert_eq!(t.active(&repo, t0, 104.5).unwrap().name, "b", "sem sticky vence o mais novo");
    }

    /// Plano com mtime de `age` segundos atrás.
    fn aged(plans: &Path, name: &str, body: &str, age: f64) {
        let p = plans.join(name);
        fs::write(&p, body).unwrap();
        fs::File::options().write(true).open(&p).unwrap()
            .set_modified(UNIX_EPOCH + std::time::Duration::from_secs_f64(now() - age)).unwrap();
    }
    const PENDING: &str = "- [x] **Step 1: a**\n- [ ] **Step 2: b**\n";

    #[test]
    fn election_prefers_pending_then_newest_and_skips_old_plans() {
        let (_dir, repo, plans) = repo();
        aged(&plans, "2026-07-01-completo.md", "- [x] **Step 1: a**\n", 1.0);
        aged(&plans, "2026-07-02-pendente-velho.md", PENDING, 100.0);
        aged(&plans, "2026-07-03-pendente-novo.md", PENDING, 50.0);
        aged(&plans, "2026-07-04-abandonado.md", PENDING, 15.0 * 86400.0);
        aged(&plans, "2026-07-05-checklist.md", "- [x] item solto\n- [ ] outro\n", 1.0);
        assert_eq!(PlanTracker::default().active(&repo, now(), 1.0).unwrap().name, "pendente-novo");
        let (_dir, repo, plans) = super::tests::repo();
        aged(&plans, "2026-07-01-so-completo.md", "- [x] **Step 1: a**\n", 1.0);
        let p = PlanTracker::default().active(&repo, now(), 1.0).unwrap();
        assert_eq!((p.name.as_str(), p.complete, p.task_idx), ("so-completo", true, 1), "completo só quando não há pendente");
    }

    #[test]
    fn pin_wins_while_pending_even_unstarted_and_falls_back_when_done_or_gone() {
        let (_dir, repo, plans) = repo();
        aged(&plans, "2026-07-01-pendente.md", PENDING, 10.0);
        aged(&plans, "2026-07-02-nao-comecado.md", "- [ ] **Step 1: a**\n", 20.0);
        aged(&plans, "2026-07-03-completo.md", "- [x] **Step 1: a**\n", 30.0);
        let pin = repo.join(".git").join(PIN_FILE);
        let elected = |mono: f64| PlanTracker::default().active(&repo, now(), mono).map(|p| p.name.clone());
        fs::write(&pin, "2026-07-02-nao-comecado\n").unwrap();
        assert_eq!(elected(1.0).as_deref(), Some("nao-comecado"));
        fs::write(&pin, "2026-07-03-completo\n").unwrap();
        assert_eq!(elected(1.0).as_deref(), Some("pendente"), "pin em plano completo volta ao automático");
        fs::write(&pin, "2026-07-09-sumiu\n").unwrap();
        assert_eq!(elected(1.0).as_deref(), Some("pendente"));
        for bad in ["../fora", "a/b", "a\\b", "..", ".", ""] {
            fs::write(&pin, format!("{bad}\n")).unwrap();
            assert_eq!(read_pin(&plans), None, "{bad:?}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn one_unreadable_plan_does_not_hide_the_others() {
        use std::os::unix::fs::PermissionsExt;
        let (_dir, repo, plans) = repo();
        aged(&plans, "2026-07-01-legivel.md", PENDING, 10.0);
        aged(&plans, "2026-07-02-ilegivel.md", PENDING, 1.0);
        fs::set_permissions(plans.join("2026-07-02-ilegivel.md"), fs::Permissions::from_mode(0o000)).unwrap();
        if fs::read(plans.join("2026-07-02-ilegivel.md")).is_ok() {
            return; // root lê tudo: o caso não existe aqui.
        }
        let mut t = PlanTracker::default();
        assert_eq!(t.active(&repo, now(), 1.0).unwrap().name, "legivel");
        let stems: Vec<String> = t.catalog(&repo).unwrap()["plans"].as_array().unwrap().iter()
            .map(|p| p["stem"].as_str().unwrap().to_owned()).collect();
        assert_eq!(stems, ["2026-07-01-legivel"]);
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_pin_reaches_the_diary() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("plan-pin-repo");
        let plans = repo.join("docs/superpowers/plans");
        fs::create_dir_all(&plans).unwrap();
        fs::create_dir_all(repo.join(".git").join(PIN_FILE)).unwrap(); // pasta no lugar do arquivo: EISDIR
        assert_eq!(read_pin(&plans), None);
        assert_eq!(super::super::facts::notes_for("plan-pin-repo"), ["list_plan_pin_unreadable:IsADirectory"]);
    }

    #[test]
    fn sticky_and_stop_at_git() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let plans = repo.join("docs/superpowers/plans");
        fs::create_dir_all(&plans).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();
        let wt = repo.join("wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: x").unwrap();
        let now = secs(SystemTime::now());
        let write = |name: &str, body: &str, age: f64| {
            let p = plans.join(name);
            fs::write(&p, body).unwrap();
            let at = UNIX_EPOCH + std::time::Duration::from_secs_f64(now - age);
            fs::File::options().write(true).open(&p).unwrap().set_modified(at).unwrap();
        };
        write("2027-01-01-a.md", "- [x] **Step 1: a**\n- [ ] **Step 2: b**\n", 10.0);
        let mut t = PlanTracker::default();
        let mut row: SessionRow = serde_json::from_value(serde_json::json!({"name": "s",
            "cwd": repo.join("src").to_str().unwrap()}))
        .unwrap();
        t.decorate(&mut row, now, 100.0);
        assert_eq!(row.plan_name.as_deref(), Some("a"));
        write("2027-01-02-b.md", "- [x] **Step 1: a**\n- [ ] **Step 2: b**\n", 1.0);
        let mut row2 = row.clone();
        t.decorate(&mut row2, now, 104.0);
        assert_eq!(row2.plan_name.as_deref(), Some("a"), "sticky");
        let mut inside: SessionRow =
            serde_json::from_value(serde_json::json!({"name": "w", "cwd": wt.to_str().unwrap()})).unwrap();
        t.decorate(&mut inside, now, 104.0);
        assert_eq!(inside.plan_name, None, "worktree não sobe ao checkout principal");
    }
}
