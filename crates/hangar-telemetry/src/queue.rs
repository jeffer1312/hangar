//! A fila em disco: um arquivo por dia (UTC) em `~/.hangar/telemetry-scope-2/queue/<dia>.json`, e as marcas
//! de `once`.
use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// O resto da pasta (temporários da gravação atômica) não passa no formato de dia.
use crate::wire::{Counter, LogLine, is_day};

/// Teto da fila: acima dele sai o dia mais antigo.
pub(crate) const MAX_DAYS: usize = 14;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct DayFile {
    pub(crate) day: String,
    /// Gravado antes do primeiro envio: a repetição leva o mesmo, e o receptor não grava duas vezes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) batch_id: Option<String>,
    pub(crate) counters: Vec<Counter>,
    /// Marcas dos retratos do dia (servidor e apps), gravadas com os contadores e nunca copiadas ao pacote.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) snapshot_keys: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) logs: Vec<LogLine>,
}

fn root(home: &Path) -> PathBuf { home.join(".hangar").join("telemetry-scope-2") }

fn queue_dir(home: &Path) -> PathBuf { root(home).join("queue") }

fn day_path(home: &Path, day: &str) -> PathBuf { queue_dir(home).join(format!("{day}.json")) }

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
    hangar_core::fs::atomic_write(&day_path(home, &file.day), &serde_json::to_vec(file)?)
}

pub(crate) fn remove_day(home: &Path, day: &str) -> io::Result<()> {
    match std::fs::remove_file(day_path(home, day)) {
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        outcome => outcome,
    }
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
        if let Some(stem) = name.to_str().and_then(|n| n.strip_suffix(".json")).filter(|s| is_day(s)) {
            days.push(stem.to_owned());
        }
    }
    days.sort();
    Ok(days)
}

/// Quantos dias saíram.
pub(crate) fn cap(home: &Path) -> io::Result<usize> {
    let days = days(home)?;
    let excess = &days[..days.len().saturating_sub(MAX_DAYS)];
    for day in excess {
        remove_day(home, day)?;
    }
    Ok(excess.len())
}

pub(crate) fn wipe(home: &Path) -> io::Result<()> { remove_tree(&root(home)) }

/// A fila e as marcas da versão sem escopo (`~/.hangar/telemetry/`): só a recusa as apaga; o aceite as deixa
/// para o binário revertido.
pub(crate) fn wipe_legacy(home: &Path) -> io::Result<()> { remove_tree(&home.join(".hangar").join("telemetry")) }

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
            write_day(home.path(), &DayFile { day: format!("2026-11-{n:02}"), batch_id: None, counters: Vec::new(), snapshot_keys: Default::default(), logs: Vec::new() }).unwrap();
        }
        assert_eq!(cap(home.path()).unwrap(), 1);
        let days = days(home.path()).unwrap();
        assert_eq!(days.len(), MAX_DAYS);
        assert_eq!(days.first().map(String::as_str), Some("2026-11-02"));
        assert_eq!(days.last().map(String::as_str), Some("2026-11-15"));
    }
}
