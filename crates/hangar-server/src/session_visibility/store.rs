//! Marca "oculta da lista" por chave do registro (`~/.hangar/session-visibility.json`). Fora do spec:
//! ele é regravado inteiro por reabertura, troca de conta e restauração, sem trava entre eles, e uma
//! reabertura apagaria um "ocultar" feito no meio.
use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    #[serde(default)]
    pub hidden: bool,
    /// Último "Mostrar na lista": o prazo do automático recomeça dele.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shown_at: Option<f64>,
}

/// Ler-modificar-gravar em um por vez: a rota e a lista gravam o mesmo arquivo.
static LOCK: Mutex<()> = Mutex::new(());

fn path(home: &Path) -> PathBuf { home.join(".hangar").join("session-visibility.json") }

/// Arquivo ausente = nenhuma marca; ilegível é erro, para uma marca nunca sumir calada.
pub fn read(home: &Path) -> io::Result<HashMap<String, Mark>> {
    match crate::runtime::queue::read_replaced(&path(home)) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(HashMap::new()),
        Err(error) => Err(error),
    }
}

/// `change` diz se mexeu; sem mudança, nada é gravado.
fn update(home: &Path, change: impl FnOnce(&mut HashMap<String, Mark>) -> bool) -> io::Result<()> {
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut marks = read(home)?;
    if !change(&mut marks) { return Ok(()); }
    let file = path(home);
    std::fs::create_dir_all(file.parent().ok_or_else(|| io::Error::other("pasta ausente"))?)?;
    crate::runtime::queue::atomic_write(&file, &serde_json::to_vec(&marks)?)
}

/// Mostrar grava `shown_at`: vale para a oculta à mão e para a do automático.
pub fn set_hidden(home: &Path, key: &str, hidden: bool, now: f64) -> io::Result<()> {
    update(home, |marks| {
        let mark = marks.entry(key.to_owned()).or_default();
        mark.hidden = hidden;
        if !hidden { mark.shown_at = Some(now); }
        true
    })
}

/// Relê sob a trava e só troca quem ainda está oculto: um "Mostrar" feito no meio da rodada fica.
pub fn unhide_if_hidden(home: &Path, keys: &[String], now: f64) -> io::Result<()> {
    update(home, |marks| {
        let mut changed = false;
        for key in keys {
            if let Some(mark) = marks.get_mut(key).filter(|m| m.hidden) {
                (mark.hidden, mark.shown_at, changed) = (false, Some(now), true);
            }
        }
        changed
    })
}

pub fn prune(home: &Path, keep: impl Fn(&str) -> bool) -> io::Result<()> {
    update(home, |marks| {
        let before = marks.len();
        marks.retain(|key, _| keep(key));
        marks.len() != before
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hide_then_show_records_when_it_was_shown() {
        let home = tempfile::tempdir().unwrap();
        set_hidden(home.path(), "a", true, 10.0).unwrap();
        assert_eq!(read(home.path()).unwrap()["a"], Mark { hidden: true, shown_at: None });
        set_hidden(home.path(), "a", false, 20.0).unwrap();
        assert_eq!(read(home.path()).unwrap()["a"], Mark { hidden: false, shown_at: Some(20.0) });
    }

    #[test]
    fn writes_to_different_keys_keep_both() {
        let home = tempfile::tempdir().unwrap();
        set_hidden(home.path(), "a", true, 1.0).unwrap();
        set_hidden(home.path(), "b", true, 2.0).unwrap();
        let marks = read(home.path()).unwrap();
        assert!(marks["a"].hidden && marks["b"].hidden);
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_map() {
        let home = tempfile::tempdir().unwrap();
        assert!(read(home.path()).unwrap().is_empty(), "ausente = vazio");
        std::fs::create_dir_all(home.path().join(".hangar")).unwrap();
        std::fs::write(path(home.path()), "{nao json").unwrap();
        assert!(read(home.path()).is_err());
        assert!(set_hidden(home.path(), "a", true, 1.0).is_err(), "não grava por cima do ilegível");
    }

    #[test]
    fn unhide_leaves_a_key_already_shown_untouched() {
        let home = tempfile::tempdir().unwrap();
        set_hidden(home.path(), "shown", true, 1.0).unwrap();
        set_hidden(home.path(), "shown", false, 5.0).unwrap();
        set_hidden(home.path(), "hidden", true, 1.0).unwrap();
        unhide_if_hidden(home.path(), &["shown".into(), "hidden".into(), "absent".into()], 9.0).unwrap();
        let marks = read(home.path()).unwrap();
        assert_eq!(marks["shown"], Mark { hidden: false, shown_at: Some(5.0) });
        assert_eq!(marks["hidden"], Mark { hidden: false, shown_at: Some(9.0) });
        assert!(!marks.contains_key("absent"));
    }

    #[test]
    fn prune_drops_only_the_refused_keys() {
        let home = tempfile::tempdir().unwrap();
        for key in ["a", "b", "c"] { set_hidden(home.path(), key, true, 1.0).unwrap(); }
        prune(home.path(), |key| key != "b").unwrap();
        let mut keys: Vec<_> = read(home.path()).unwrap().into_keys().collect();
        keys.sort();
        assert_eq!(keys, ["a", "c"]);
    }
}
