//! Poda dos sidecars de sessão morta em `<conta Claude>/.hangar-*` (statusline, estado, prévia,
//! fila, bastão, bilhetes do Pi e do Kimi). Sidecar de sessão morta mente — um pane reaproveitado
//! já mostrou o custo de outra execução —, mas a leitura tem teto próprio: a poda é higiene.
//!
//! A regra é conservadora: chave viva nunca sai; chave morta só sai depois de `MIN_AGE`, para a
//! sessão morta recente continuar diagnosticável; conjunto desconhecido (ou vazio) deixa a família
//! inteira. A sobra de `tmp+rename` não tem leitor e sai só por idade.
#![forbid(unsafe_code)]

mod names;
mod schedule;

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;

pub use names::{file_stem, is_tmp};
pub use schedule::{INTERVAL, RETRY, every};

/// Idade mínima do sidecar de chave morta.
pub const MIN_AGE: Duration = Duration::from_secs(7 * 86_400);
/// Idade mínima da sobra de `tmp+rename`: o único risco é a escrita em voo, que vive milissegundos.
pub const TMP_MIN_AGE: Duration = Duration::from_secs(3_600);

/// Pela chave da sessão (`session_key` do transcript). O `.hangar-prompt` do `state-hook` usa o
/// `session_id`, que é a mesma chave fora de sessão retomada; a retomada só o deixa para a idade.
const STEM_KEYED: [&str; 6] = [".hangar-status", ".hangar-state", ".hangar-preview", ".hangar-askq", ".hangar-prompt", ".hangar-pi/models"];
/// Pelo nome sanitizado: a fila sobrevive ao `/clear` e o bastão é do destino.
const NAME_KEYED: [(&str, &str); 2] = [(".hangar-queue", ".jsonl"), (".hangar-bastao", ".md")];
/// Bilhetes: pela chave da sessão, pelo nome (psmux) ou pelo pane, conforme quem gravou.
const TICKETS: [&str; 2] = [".hangar-pi", ".hangar-kimi"];

/// Quem está vivo. `None` = não se sabe; nenhum arquivo dessa família sai.
#[derive(Clone, Debug, Default)]
pub struct Live {
    /// `session_key` dos transcripts vivos.
    pub stems: Option<HashSet<String>>,
    /// Nomes vivos já sanitizados como nos arquivos (`[^A-Za-z0-9_.-]` vira `-`).
    pub names: Option<HashSet<String>>,
    /// Pane ids vivos sem o `%`.
    pub panes: Option<HashSet<String>>,
    /// `CP_SESSION_KEY` das sessões vivas; vazio vale, porque vem do registro em disco e não da lista.
    pub keys: Option<HashSet<String>>,
}

/// Família que ficou inteira porque o conjunto que a protege era desconhecido.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family { Stems, Names, Tickets }

impl Family {
    pub fn as_str(self) -> &'static str {
        match self { Self::Stems => "stems", Self::Names => "names", Self::Tickets => "tickets" }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Apagados por pasta da família (`.hangar-pi/models`), e a sobra de tmp em `<pasta> (.tmp)`; só os não zerados.
    pub removed: BTreeMap<String, usize>,
    pub skipped: Vec<Family>,
    /// Pastas que eram link para fora das bases: não seguidas.
    pub outside: usize,
    /// Arquivos ou pastas que não deu para ler ou apagar (sem permissão, em uso no Windows); só aquele item pulou.
    pub failed: usize,
}

impl Report {
    pub fn total(&self) -> usize { self.removed.values().sum() }
}

/// Varre `bases` (pastas de conta Claude) com `now` como agora. Base ausente não é falha.
pub fn prune(bases: &[PathBuf], live: &Live, now: SystemTime) -> Report {
    let mut sweep = Sweep { roots: Vec::new(), now, report: Report::default(), seen_tmp: HashSet::new(), seen_keyed: HashSet::new() };
    // Vazio da lista é "não sei", nunca "ninguém": na subida é comum ainda não haver sessão.
    let known = |set: &Option<HashSet<String>>| set.as_ref().filter(|set| !set.is_empty()).cloned();
    let stems = known(&live.stems);
    let names = known(&live.names);
    let tickets = match (known(&live.panes), &names, &live.keys) {
        (Some(panes), Some(names), Some(keys)) => Some(panes.into_iter().chain(names.iter().cloned()).chain(keys.iter().cloned()).collect::<HashSet<_>>()),
        _ => None,
    };
    for (family, unknown) in [(Family::Stems, stems.is_none()), (Family::Names, names.is_none()), (Family::Tickets, tickets.is_none())] {
        if unknown { sweep.report.skipped.push(family); }
    }
    for base in bases {
        match fs::canonicalize(base) {
            Ok(root) if !sweep.roots.contains(&root) => sweep.roots.push(root),
            Ok(_) => {}
            Err(error) => sweep.note(&error),
        }
    }
    let no_stems = HashSet::new();
    for root in sweep.roots.clone() {
        sweep.tmp(&root);
        sweep.active(&root, stems.as_ref().unwrap_or(&no_stems));
        if let Some(stems) = &stems {
            for sub in STEM_KEYED { sweep.keyed(&root, sub, ".json", stems); }
        }
        if let Some(names) = &names {
            for (sub, suffix) in NAME_KEYED { sweep.keyed(&root, sub, suffix, names); }
        }
        if let Some(tickets) = &tickets {
            for sub in TICKETS { sweep.keyed(&root, sub, ".json", tickets); }
        }
    }
    sweep.report
}

struct Sweep {
    /// Bases canônicas: pasta que resolve para fora delas não é seguida.
    roots: Vec<PathBuf>,
    now: SystemTime,
    report: Report,
    /// Pasta de conta que é link para a do `~/.claude` é varrida uma vez só.
    seen_tmp: HashSet<PathBuf>,
    seen_keyed: HashSet<PathBuf>,
}

/// O que fazer com um arquivo velho o bastante; `Failed` conta como falha e o arquivo fica.
enum Verdict { Keep, Remove, Failed }

impl Sweep {
    fn tmp(&mut self, root: &Path) {
        let Some(entries) = self.list(root) else { return };
        let mut dirs: Vec<(String, PathBuf)> = entries.iter()
            .filter_map(|e| e.file_name().to_str().filter(|n| n.starts_with(".hangar-")).map(|n| (n.to_owned(), e.path())))
            .collect();
        dirs.sort();
        for (name, path) in dirs {
            let Some(dir) = self.dir(&path) else { continue };
            if !self.seen_tmp.insert(dir.clone()) { continue; }
            let Some(children) = self.list(&dir) else { continue };
            // Também um nível abaixo: o catálogo do Pi mora em `.hangar-pi/models`.
            let mut targets = vec![dir.clone()];
            for child in children {
                if !child.file_type().is_ok_and(|t| t.is_dir() || t.is_symlink()) { continue; }
                if let Some(child) = self.dir(&child.path()) && self.seen_tmp.insert(child.clone()) { targets.push(child); }
            }
            let removed: usize = targets.iter()
                .map(|t| self.remove_old(t, TMP_MIN_AGE, |file, _| if is_tmp(file) { Verdict::Remove } else { Verdict::Keep }))
                .sum();
            self.count(format!("{name} (.tmp)"), removed);
        }
    }

    /// `.hangar-active/<boot_id>.json`: o boot_id difere da chave depois de resume/clear, então quem
    /// prova a morte é o `pid` gravado. Sem pid não há prova, e o marcador fica.
    fn active(&mut self, root: &Path, stems: &HashSet<String>) {
        let Some(dir) = self.dir(&root.join(".hangar-active")) else { return };
        if !self.seen_keyed.insert(dir.clone()) { return; }
        let removed = self.remove_old(&dir, MIN_AGE, |file, path| match file_stem(file, ".json") {
            Some(key) if !stems.contains(key) => recorded_pid_dead(path),
            _ => Verdict::Keep,
        });
        self.count(".hangar-active".to_owned(), removed);
    }

    fn keyed(&mut self, root: &Path, sub: &str, suffix: &str, live: &HashSet<String>) {
        // Componente por componente: no Windows a base canônica é `\\?\…`, que não aceita `/`.
        let path = sub.split('/').fold(root.to_path_buf(), |p, c| p.join(c));
        let Some(dir) = self.dir(&path) else { return };
        if !self.seen_keyed.insert(dir.clone()) { return; }
        let removed = self.remove_old(&dir, MIN_AGE, |file, _| match file_stem(file, suffix) {
            Some(key) if !live.contains(key) => Verdict::Remove,
            _ => Verdict::Keep,
        });
        self.count(sub.to_owned(), removed);
    }

    /// Apaga de `dir` os arquivos com mais de `age` que `verdict` condena; devolve quantos saíram.
    fn remove_old(&mut self, dir: &Path, age: Duration, verdict: impl Fn(&str, &Path) -> Verdict) -> usize {
        let mut removed = 0;
        for entry in self.list(dir).unwrap_or_default() {
            // Nome que não é UTF-8 nunca foi gravado pelo Hangar: fica.
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
            let path = entry.path();
            // `metadata` segue o link, como o `is_file`/`stat` do Python; o `remove_file` tira só o link.
            let old = match fs::metadata(&path) {
                Ok(meta) => meta.is_file() && meta.modified().is_ok_and(|at| self.now.duration_since(at).is_ok_and(|d| d >= age)),
                Err(error) => { self.note(&error); false }
            };
            if !old { continue; }
            match verdict(&name, &path) {
                Verdict::Keep => {}
                Verdict::Failed => self.report.failed += 1,
                Verdict::Remove => match fs::remove_file(&path) {
                    Ok(()) => removed += 1,
                    Err(error) => self.note(&error),
                },
            }
        }
        removed
    }

    /// A pasta canônica, se existe, é pasta e fica dentro das bases.
    fn dir(&mut self, path: &Path) -> Option<PathBuf> {
        let real = fs::canonicalize(path).map_err(|error| self.note(&error)).ok()?;
        if !fs::metadata(&real).is_ok_and(|m| m.is_dir()) { return None; }
        if !self.roots.iter().any(|root| real.starts_with(root)) {
            self.report.outside += 1;
            return None;
        }
        Some(real)
    }

    fn list(&mut self, dir: &Path) -> Option<Vec<fs::DirEntry>> {
        let entries = fs::read_dir(dir).map_err(|error| self.note(&error)).ok()?;
        let mut out = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => out.push(entry),
                Err(error) => self.note(&error),
            }
        }
        Some(out)
    }

    /// Sumiu no meio (sessão encerrando) não é falha; o resto conta e só aquele item pula.
    fn note(&mut self, error: &io::Error) {
        if error.kind() != io::ErrorKind::NotFound { self.report.failed += 1; }
    }

    fn count(&mut self, key: String, removed: usize) {
        if removed > 0 { *self.report.removed.entry(key).or_default() += removed; }
    }
}

/// O `pid` gravado é inteiro positivo e o processo não existe; JSON torto, sem pid ou pid vivo, fica.
fn recorded_pid_dead(path: &Path) -> Verdict {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Verdict::Keep,
        Err(_) => return Verdict::Failed,
    };
    // `true`/`false` não são pid: no Python `false` passava por inteiro e apagava o marcador.
    let pid = serde_json::from_slice::<Value>(&bytes).ok()
        .and_then(|value| value.get("pid").and_then(Value::as_u64))
        .and_then(|pid| u32::try_from(pid).ok())
        .filter(|pid| *pid > 0);
    let Some(pid) = pid else { return Verdict::Keep };
    match hangar_core::terminal_process::pid_alive(pid) {
        Ok(true) => Verdict::Keep,
        Ok(false) => Verdict::Remove,
        Err(_) => Verdict::Failed,
    }
}

/// As travas `<nome>.lock` de `~/.hangar/codex-sessions` sem o `<nome>.json` ao lado, com mais de
/// `MIN_AGE` e sem dono agora: o lançador e o servidor nunca as apagam, e quem grava cria de novo.
/// Devolve (apagadas, falhas).
pub fn prune_codex_locks(dir: &Path, now: SystemTime) -> (usize, usize) { prune_codex_locks_with(dir, now, |_| {}) }

/// O arquivo no caminho ainda é o que foi aberto (outro escritor pode ter apagado e recriado a trava).
fn same_file(path: &Path, opened: &fs::File) -> bool {
    let (Ok(now), Ok(held)) = (fs::symlink_metadata(path), opened.metadata()) else { return false };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        now.dev() == held.dev() && now.ino() == held.ino()
    }
    #[cfg(windows)]
    {
        // O índice do arquivo ainda não é estável no std; a criação e o tamanho distinguem o recriado.
        use std::os::windows::fs::MetadataExt;
        now.creation_time() == held.creation_time() && now.file_size() == held.file_size()
    }
}

/// `checked` roda com a trava na mão, antes das conferências finais: os testes simulam ali a corrida.
fn prune_codex_locks_with(dir: &Path, now: SystemTime, checked: impl Fn(&Path)) -> (usize, usize) {
    let Ok(entries) = fs::read_dir(dir) else { return (0, 0) };
    let (mut removed, mut failed) = (0, 0);
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else { continue };
        let Some(stem) = file_stem(&name, ".lock").filter(|stem| *stem != name) else { continue };
        if dir.join(format!("{stem}.json")).exists() { continue; }
        let path = entry.path();
        let old = fs::symlink_metadata(&path)
            .is_ok_and(|meta| meta.is_file() && meta.modified().is_ok_and(|at| now.duration_since(at).is_ok_and(|age| age >= MIN_AGE)));
        if !old { continue; }
        let Ok(lock) = fs::OpenOptions::new().read(true).append(true).open(&path) else { failed += 1; continue };
        // Com dono agora, fica: alguém está gravando esse sidecar.
        if lock.try_lock().is_err() { continue; }
        checked(&path);
        // Entre a listagem e a trava a sessão pode ter gravado o sidecar, ou trocado a trava por outra.
        if dir.join(format!("{stem}.json")).exists() || !same_file(&path, &lock) { continue; }
        // No Windows o arquivo aberto não sai; a trava é solta antes, e quem a pegar depois cria outra.
        #[cfg(windows)]
        drop(lock);
        match fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => failed += 1,
        }
    }
    (removed, failed)
}

#[cfg(test)]
mod codex_lock_tests {
    use super::*;

    #[test]
    fn only_old_orphan_free_locks_leave() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["orfa.lock", "viva.lock", "viva.json", "nova.lock", "presa.lock", "outro.txt"] {
            fs::write(dir.path().join(name), "").unwrap();
        }
        let later = SystemTime::now() + MIN_AGE + Duration::from_secs(60);
        // `nova` é recente: o relógio da poda olha só os antigos.
        let held = fs::OpenOptions::new().read(true).append(true).open(dir.path().join("presa.lock")).unwrap();
        held.lock().unwrap();
        let (removed, failed) = prune_codex_locks(dir.path(), later);
        assert_eq!((removed, failed), (2, 0), "a órfã e a nova (velha no relógio de amanhã); a viva tem sidecar e a presa tem dono");
        let left: std::collections::BTreeSet<String> = fs::read_dir(dir.path()).unwrap().flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, ["outro.txt", "presa.lock", "viva.json", "viva.lock"].into_iter().map(String::from).collect());
        drop(held);
        assert_eq!(prune_codex_locks(dir.path(), SystemTime::now()), (0, 0), "nada é velho hoje");
        assert_eq!(prune_codex_locks(&dir.path().join("sem-pasta"), later), (0, 0));
    }

    /// A corrida: com a trava já na mão, a sessão grava o sidecar, ou outro processo apaga e recria a
    /// trava. Nos dois casos ela fica.
    #[test]
    fn a_lock_that_changed_while_being_taken_stays() {
        let dir = tempfile::tempdir().unwrap();
        let later = SystemTime::now() + MIN_AGE + Duration::from_secs(60);
        fs::write(dir.path().join("a.lock"), "").unwrap();
        let sidecar = dir.path().join("a.json");
        assert_eq!(prune_codex_locks_with(dir.path(), later, |_| fs::write(&sidecar, "{}").unwrap()), (0, 0));
        assert!(dir.path().join("a.lock").exists(), "o sidecar apareceu: a trava é dele");
        fs::remove_file(&sidecar).unwrap();
        #[cfg(unix)]
        {
            let replaced = |path: &Path| {
                fs::remove_file(path).unwrap();
                fs::write(path, "").unwrap();
            };
            assert_eq!(prune_codex_locks_with(dir.path(), later, replaced), (0, 0));
            assert!(dir.path().join("a.lock").exists(), "a trava recriada é de outro dono");
        }
        assert_eq!(prune_codex_locks(dir.path(), later), (1, 0), "sem corrida, a órfã sai");
    }
}
