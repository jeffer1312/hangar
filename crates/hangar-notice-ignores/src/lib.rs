//! Avisos da CLI que o dono mandou não mostrar mais nesta máquina: as chaves `notice_meta.key` guardadas em
//! `~/.hangar/notice-ignores.json`. Os clientes escondem o aviso de chave ignorada.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const MAX_KEYS: usize = 200;
const MAX_KEY_CHARS: usize = 200;

#[derive(Debug)]
pub enum Error {
    InvalidKey,
    Full,
    Io(io::Error),
}

#[derive(Default, Serialize, Deserialize)]
struct Doc {
    #[serde(default)]
    keys: BTreeSet<String>,
}

pub fn path(home: &Path) -> PathBuf {
    home.join(".hangar").join("notice-ignores.json")
}

/// Só as chaves que o servidor emite (`hook:…`, `note:…`), com algo depois do prefixo.
pub fn valid_key(key: &str) -> bool {
    ["hook:", "note:"].iter().any(|prefix| key.strip_prefix(prefix).is_some_and(|rest| !rest.is_empty()))
        && key.chars().count() <= MAX_KEY_CHARS
        && !key.chars().any(char::is_control)
}

/// Arquivo ausente é lista vazia; arquivo ilegível é erro, nunca lista vazia: a próxima gravação o apagaria.
pub fn load(home: &Path) -> io::Result<BTreeSet<String>> {
    match hangar_core::fs::read_replaced(&path(home)) {
        Ok(raw) => serde_json::from_slice::<Doc>(&raw).map(|doc| doc.keys).map_err(io::Error::other),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(error) => Err(error),
    }
}

/// Marca ou desmarca uma chave e devolve a lista gravada.
pub fn set(home: &Path, key: &str, ignored: bool) -> Result<BTreeSet<String>, Error> {
    if !valid_key(key) {
        return Err(Error::InvalidKey);
    }
    let path = path(home);
    std::fs::create_dir_all(path.parent().ok_or_else(|| Error::Io(io::Error::other("sem pasta")))?).map_err(Error::Io)?;
    // Dois aparelhos gravando ao mesmo tempo: a leitura e a gravação de cada um não se intercalam.
    let lock = std::fs::OpenOptions::new().read(true).append(true).create(true).open(path.with_extension("lock")).map_err(Error::Io)?;
    lock.lock().map_err(Error::Io)?;
    let mut keys = load(home).map_err(Error::Io)?;
    if !ignored {
        keys.remove(key);
    } else if !keys.contains(key) {
        if keys.len() >= MAX_KEYS {
            return Err(Error::Full);
        }
        keys.insert(key.to_owned());
    }
    let bytes = serde_json::to_vec(&Doc { keys: keys.clone() }).map_err(|error| Error::Io(io::Error::other(error)))?;
    hangar_core::fs::atomic_write(&path, &bytes).map_err(Error::Io)?;
    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|key| (*key).to_owned()).collect()
    }

    #[test]
    fn missing_file_is_empty() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(load(home.path()).unwrap(), BTreeSet::new());
    }

    #[test]
    fn set_adds_and_removes() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(set(home.path(), "hook:Stop", true).unwrap(), keys(&["hook:Stop"]));
        assert_eq!(set(home.path(), "note:retry", true).unwrap(), keys(&["hook:Stop", "note:retry"]));
        assert_eq!(set(home.path(), "hook:Stop", true).unwrap(), keys(&["hook:Stop", "note:retry"]), "repetir não duplica");
        assert_eq!(set(home.path(), "hook:Stop", false).unwrap(), keys(&["note:retry"]));
        assert_eq!(load(home.path()).unwrap(), keys(&["note:retry"]));
        assert_eq!(set(home.path(), "hook:nunca-marcada", false).unwrap(), keys(&["note:retry"]));
    }

    #[test]
    fn unreadable_file_is_an_error_and_set_does_not_overwrite_it() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(path(home.path()).parent().unwrap()).unwrap();
        std::fs::write(path(home.path()), b"{\"keys\": [").unwrap();
        assert!(load(home.path()).is_err());
        assert!(matches!(set(home.path(), "hook:Stop", true), Err(Error::Io(_))));
        assert_eq!(std::fs::read(path(home.path())).unwrap(), b"{\"keys\": [", "o arquivo estragado fica como estava");
    }

    #[test]
    fn invalid_key_is_refused() {
        let home = tempfile::tempdir().unwrap();
        let long = format!("hook:{}", "a".repeat(196));
        for key in ["", "x:y", "hook:", "note:", "hook:a\nb", long.as_str()] {
            assert!(matches!(set(home.path(), key, true), Err(Error::InvalidKey)), "{key:?}");
        }
        assert!(!path(home.path()).exists(), "chave recusada não cria o arquivo");
        assert!(valid_key(&format!("hook:{}", "a".repeat(195))), "200 caracteres cabem");
        assert!(valid_key("note:notification:update") && valid_key("hook:PostToolUse:Bash"));
    }

    #[test]
    fn full_list_refuses_the_201st() {
        let home = tempfile::tempdir().unwrap();
        for i in 0..MAX_KEYS {
            set(home.path(), &format!("hook:h{i}"), true).unwrap();
        }
        assert!(matches!(set(home.path(), "hook:a-mais", true), Err(Error::Full)));
        assert_eq!(set(home.path(), "hook:h0", true).unwrap().len(), MAX_KEYS, "a que já está não conta como nova");
        assert_eq!(set(home.path(), "hook:h0", false).unwrap().len(), MAX_KEYS - 1);
        assert_eq!(set(home.path(), "hook:a-mais", true).unwrap().len(), MAX_KEYS);
    }
}
