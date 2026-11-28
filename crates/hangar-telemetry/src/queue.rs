//! Fila e marcas em disco exclusivas do escopo 3: um binário antigo não pode enviar lotes de B.
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// O resto da pasta (temporários da gravação atômica) não passa no formato de dia.
use crate::diary::DayDetail;
use crate::wire::{Counter, Env, is_day, is_name};

/// Teto da fila: acima dele sai o dia mais antigo.
pub(crate) const MAX_DAYS: usize = 14;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct DayFile {
    pub(crate) day: String,
    pub(crate) collection: String,
    /// Gravado antes do primeiro envio: a repetição leva o mesmo, e o receptor não grava duas vezes.
    pub(crate) batch_id: Option<String>,
    pub(crate) counters: Vec<Counter>,
    pub(crate) detail: DayDetail,
    pub(crate) sealed: bool,
    /// Marcas dos retratos do dia (servidor e apps), gravadas com os contadores e nunca copiadas ao pacote.
    #[serde(skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) snapshot_keys: BTreeSet<String>,
    /// O binário que contou (`versão-commit`); vazio só em arquivo de antes da divisão por versão.
    pub(crate) build: String,
    /// O `env` desse binário, que o lote leva mesmo quando outro binário o envia.
    pub(crate) env: Option<Env>,
    /// Arquivo de um binário que já não roda: `<dia>_<build>.json`, sem contagem nova.
    pub(crate) frozen: bool,
}

impl DayFile {
    /// O nome do arquivo sem `.json`: o dia, ou `<dia>_<build>` quando congelado.
    pub(crate) fn stem(&self) -> String { if self.frozen { format!("{}_{}", self.day, self.build) } else { self.day.clone() } }
}

/// O dia UTC de um nome de arquivo da fila.
pub(crate) fn day_of(stem: &str) -> &str { &stem[..10] }

/// O binário de um arquivo congelado; `None` no arquivo do binário que roda.
pub(crate) fn build_of(stem: &str) -> Option<&str> { stem.get(11..) }

fn is_stem(stem: &str) -> bool {
    stem.get(..10).is_some_and(is_day) && (stem.len() == 10 || (stem.as_bytes()[10] == b'_' && is_name(&stem[11..])))
}

fn root(home: &Path) -> PathBuf { home.join(".hangar").join("telemetry-v3") }

fn queue_dir(home: &Path) -> PathBuf { root(home).join("queue") }

fn day_path(home: &Path, stem: &str) -> PathBuf { queue_dir(home).join(format!("{stem}.json")) }

pub(crate) fn once_path(home: &Path) -> PathBuf { root(home).join("once.json") }

fn invalid(e: serde_json::Error) -> io::Error { io::Error::new(io::ErrorKind::InvalidData, e) }

pub(crate) fn read_day(home: &Path, day: &str) -> io::Result<Option<DayFile>> {
    match hangar_core::fs::read_replaced(&day_path(home, day)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(invalid),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub(crate) fn write_day(home: &Path, file: &DayFile) -> io::Result<()> {
    std::fs::create_dir_all(queue_dir(home))?;
    hangar_core::fs::atomic_write(&day_path(home, &file.stem()), &serde_json::to_vec(file)?)
}

pub(crate) fn remove_day(home: &Path, day: &str) -> io::Result<()> {
    match std::fs::remove_file(day_path(home, day)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        outcome => outcome,
    }
}

/// Grava o carimbo deste binário e devolve o que estava lá (o da vida anterior do servidor).
pub(crate) fn swap_build(home: &Path, build: &str) -> io::Result<Option<String>> {
    let path = root(home).join("build");
    let previous = match std::fs::read_to_string(&path) {
        Ok(text) => Some(text.trim().to_owned()).filter(|text| is_name(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => return Err(e),
    };
    if previous.as_deref() != Some(build) {
        std::fs::create_dir_all(root(home))?;
        hangar_core::fs::atomic_write(&path, build.as_bytes())?;
    }
    Ok(previous)
}

/// Os dias na fila, do mais antigo ao mais novo.
pub(crate) fn days(home: &Path) -> io::Result<Vec<String>> {
    let entries = match std::fs::read_dir(queue_dir(home)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut days = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        if let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")).filter(|s| is_stem(s)) {
            days.push(stem.to_owned());
        }
    }
    days.sort();
    Ok(days)
}

/// Quantos arquivos saíram: ficam os de `MAX_DAYS` dias, com todos os binários de cada um.
pub(crate) fn cap(home: &Path) -> io::Result<usize> {
    let stems = days(home)?;
    let distinct: BTreeSet<&str> = stems.iter().map(|stem| day_of(stem)).collect();
    let Some(&oldest_kept) = distinct.iter().rev().take(MAX_DAYS).last() else { return Ok(0) };
    let excess: Vec<&String> = stems.iter().filter(|stem| day_of(stem) < oldest_kept).collect();
    for stem in &excess {
        remove_day(home, stem)?;
    }
    Ok(excess.len())
}

/// Deixa no nome do dia só o arquivo de `build`: o de outro binário vira congelado, o congelado de
/// `build` volta a ser o do dia, e o de antes da divisão por versão sai (devolve quantos saíram).
pub(crate) fn rotate(home: &Path, build: &str) -> io::Result<usize> {
    let mut removed = 0;
    for stem in days(home)?.into_iter().filter(|stem| build_of(stem).is_none()) {
        // Arquivo ilegível fica: a gravação do dia o troca pela memória e avisa o diário.
        let mut file = match read_day(home, &stem) {
            Ok(Some(file)) => file,
            Ok(None) => continue,
            Err(e) if e.kind() == io::ErrorKind::InvalidData => continue,
            Err(e) => return Err(e),
        };
        if file.build == build { continue; }
        remove_day(home, &stem)?;
        if file.build.is_empty() {
            removed += 1;
            continue;
        }
        file.frozen = true;
        write_day(home, &file)?;
    }
    for stem in days(home)?.into_iter().filter(|stem| build_of(stem) == Some(build)) {
        let Some(mut file) = read_day(home, &stem)? else { continue };
        if read_day(home, day_of(&stem))?.is_some() { continue; }
        file.frozen = false;
        write_day(home, &file)?;
        remove_day(home, &stem)?;
    }
    Ok(removed)
}

/// A fila e as marcas do escopo 3.
pub(crate) fn wipe(home: &Path) -> io::Result<()> { remove_tree(&root(home)) }

/// As filas das versões sem escopo (`~/.hangar/telemetry/`) e do escopo 2 (`telemetry-scope-2/`): a
/// recusa e o desligamento (`HANGAR_TELEMETRY=0`, `DO_NOT_TRACK`, CI) as apagam, mesmo com o aceite
/// gravado; o aceite ligado as deixa para o binário revertido, que não envia sem o próprio aceite.
/// Tenta as duas antes de devolver o primeiro erro.
pub(crate) fn wipe_legacy(home: &Path) -> io::Result<()> {
    ["telemetry", "telemetry-scope-2"].map(|name| remove_tree(&home.join(".hangar").join(name))).into_iter().collect()
}

fn remove_tree(root: &Path) -> io::Result<()> {
    match std::fs::remove_dir_all(root) {
        Err(e) if e.kind() == io::ErrorKind::NotFound && !root.try_exists()? => Ok(()),
        outcome => outcome,
    }
}

pub(crate) fn read_once(home: &Path) -> io::Result<Vec<String>> {
    match hangar_core::fs::read_replaced(&once_path(home)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(invalid),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

pub(crate) fn write_once(home: &Path, keys: &[String]) -> io::Result<()> {
    std::fs::create_dir_all(root(home))?;
    hangar_core::fs::atomic_write(&once_path(home), &serde_json::to_vec(keys)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifteen_days_keep_fourteen() {
        let home = tempfile::tempdir().unwrap();
        for n in 1..=15 {
            write_day(home.path(), &DayFile { day: format!("2026-11-{n:02}"), ..DayFile::default() }).unwrap();
        }
        assert_eq!(cap(home.path()).unwrap(), 1);
        let days = days(home.path()).unwrap();
        assert_eq!(days.len(), MAX_DAYS);
        assert_eq!(days.first().map(String::as_str), Some("2026-11-02"));
        assert_eq!(days.last().map(String::as_str), Some("2026-11-15"));
    }

    #[test]
    fn telemetry_each_binary_keeps_its_own_file_of_the_day() {
        let home = tempfile::tempdir().unwrap();
        let file = |day: &str, build: &str| DayFile { day: day.into(), build: build.into(), ..DayFile::default() };
        write_day(home.path(), &file("2026-11-27", "0.1.0-aaaaaaa")).unwrap();
        write_day(home.path(), &file("2026-11-26", "")).unwrap();
        assert_eq!(rotate(home.path(), "0.2.0-bbbbbbb").unwrap(), 1, "o de antes da divisão sai");
        assert_eq!(days(home.path()).unwrap(), ["2026-11-27_0.1.0-aaaaaaa"]);
        let frozen = read_day(home.path(), "2026-11-27_0.1.0-aaaaaaa").unwrap().unwrap();
        assert!(frozen.frozen && frozen.build == "0.1.0-aaaaaaa");
        write_day(home.path(), &file("2026-11-27", "0.2.0-bbbbbbb")).unwrap();
        assert_eq!(rotate(home.path(), "0.1.0-aaaaaaa").unwrap(), 0);
        assert_eq!(days(home.path()).unwrap(), ["2026-11-27", "2026-11-27_0.2.0-bbbbbbb"], "o binário que volta retoma o próprio arquivo");
        assert_eq!(read_day(home.path(), "2026-11-27").unwrap().unwrap().build, "0.1.0-aaaaaaa");
        assert_eq!((day_of("2026-11-27_0.2.0-bbbbbbb"), build_of("2026-11-27_0.2.0-bbbbbbb")), ("2026-11-27", Some("0.2.0-bbbbbbb")));
        assert_eq!(build_of("2026-11-27"), None);
    }

    #[test]
    fn telemetry_cap_counts_days_not_files() {
        let home = tempfile::tempdir().unwrap();
        for n in 1..=15 {
            write_day(home.path(), &DayFile { day: format!("2026-11-{n:02}"), ..DayFile::default() }).unwrap();
        }
        write_day(home.path(), &DayFile { day: "2026-11-15".into(), build: "0.1.0-dev".into(), frozen: true, ..DayFile::default() }).unwrap();
        assert_eq!(cap(home.path()).unwrap(), 1);
        assert_eq!(days(home.path()).unwrap().len(), MAX_DAYS + 1);
    }

    #[test]
    fn legacy_wipe_tries_both_queues_before_failing() {
        let home = tempfile::tempdir().unwrap();
        let hangar = home.path().join(".hangar");
        std::fs::create_dir_all(hangar.join("telemetry-scope-2/queue")).unwrap();
        std::fs::write(hangar.join("telemetry"), "not a directory").unwrap();
        assert!(wipe_legacy(home.path()).is_err());
        assert!(!hangar.join("telemetry-scope-2").exists());
    }
}
