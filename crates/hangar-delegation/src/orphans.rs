//! `~/.hangar/link-orphans.json`: filha ligada cujo pai fechou ou morreu, com o nome dele e quando
//! (`{"<filha>": {"parent": str, "at": f64}}`). A filha segue aberta, e a lista mostra de quem ela era.
use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Orphan {
    pub parent: String,
    pub at: f64,
}

pub type Orphans = BTreeMap<String, Orphan>;

/// Ler e regravar sem perder a escrita de outra tarefa do processo (o fechamento e a varredura).
static LOCK: Mutex<()> = Mutex::new(());

pub fn path(hangar_dir: &Path) -> PathBuf { hangar_dir.join("link-orphans.json") }

/// Ausente é vazio; ilegível é erro: regravar por cima apagaria as marcas das outras filhas.
pub fn read(hangar_dir: &Path) -> io::Result<Orphans> {
    match std::fs::read(path(hangar_dir)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Orphans::new()),
        Err(error) => Err(error),
    }
}

/// Lê, muda e grava (tmp+rename) sob a trava do processo, só quando `change` diz que mudou.
fn modify(hangar_dir: &Path, change: impl FnOnce(&mut Orphans) -> bool) -> io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut all = read(hangar_dir)?;
    if !change(&mut all) { return Ok(()); }
    std::fs::create_dir_all(hangar_dir)?;
    hangar_core::fs::atomic_write(&path(hangar_dir), &serde_json::to_vec_pretty(&all).map_err(io::Error::other)?)
}

/// As filhas de `parent` perderam o pai em `at`; a marca de antes de cada uma dá lugar a esta.
pub fn record(hangar_dir: &Path, parent: &str, children: &[String], at: f64) -> io::Result<()> {
    modify(hangar_dir, |all| {
        for child in children { all.insert(child.clone(), Orphan { parent: parent.to_owned(), at }); }
        !children.is_empty()
    })
}

/// Tira a marca das que fecharam, foram religadas ou sumiram; sem marca nenhuma, nada é gravado.
pub fn clear(hangar_dir: &Path, children: &[String]) -> io::Result<()> {
    modify(hangar_dir, |all| {
        let before = all.len();
        all.retain(|name, _| !children.contains(name));
        all.len() != before
    })
}

/// Filha renomeada leva a marca junto: sem isso a lista deixaria de dizer de quem ela era.
pub fn rename(hangar_dir: &Path, old: &str, new: &str) -> io::Result<()> {
    modify(hangar_dir, |all| match all.remove(old) {
        Some(entry) => { all.insert(new.to_owned(), entry); true }
        None => false,
    })
}
