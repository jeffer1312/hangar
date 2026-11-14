//! Ler e gravar a pasta `memory/` sem perder nota de quem escreveu durante a rodada.
//! A trava é a mesma do auto-dream nativo do Claude Code: os dois nunca consolidam juntos.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, SystemTime};

use hangar_core::fs::atomic_write;
use hangar_core::terminal_process::pid_alive;

pub const LOCK: &str = ".consolidate-lock";
pub const LOCK_STALE: Duration = Duration::from_secs(3600);
pub const INDEX: &str = "MEMORY.md";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub files: BTreeMap<String, String>,
    pub digest: String,
}

#[derive(Debug)]
pub enum WriteError {
    /// Alguém mudou a pasta depois da leitura: a rodada daquela máquina fica para a próxima.
    Changed,
    Locked,
    InvalidName(String),
    Io(io::Error),
}

impl WriteError {
    /// O código que vai ao registro da rodada e ao diário; o detalhe pode trazer caminho.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Changed => "dream_memory_changed",
            Self::Locked => "dream_locked",
            Self::InvalidName(_) => "dream_invalid_name",
            Self::Io(_) => "dream_io",
        }
    }
}

impl From<io::Error> for WriteError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// Só os `*.md` da raiz da pasta; pasta que não existe é memória vazia.
pub fn read(dir: &Path) -> io::Result<Snapshot> {
    let mut files = BTreeMap::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Snapshot { digest: digest(&files), files }),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if valid_name(&name) && entry.file_type()?.is_file() {
            files.insert(name, fs::read_to_string(entry.path())?);
        }
    }
    Ok(Snapshot { digest: digest(&files), files })
}

pub fn digest(files: &BTreeMap<String, String>) -> String {
    let mut h = sha1_smol::Sha1::new();
    for (name, content) in files {
        h.update(name.as_bytes());
        h.update(&[0]);
        h.update(content.as_bytes());
        h.update(&[0]);
    }
    h.digest().to_string()
}

/// Nome que pode vir de outra máquina: arquivo simples `.md`, nada de caminho nem oculto.
pub fn valid_name(name: &str) -> bool {
    name.ends_with(".md")
        && !name.starts_with('.')
        && !name.contains(['/', '\\', '\0'])
        && name != ".."
}

pub fn last_consolidated(dir: &Path) -> Option<SystemTime> {
    fs::metadata(dir.join(LOCK)).and_then(|m| m.modified()).ok()
}

/// Grava arquivo a arquivo, `MEMORY.md` por último, só se a pasta ainda for a que foi lida. Nunca
/// troca a pasta: ela pode ser atalho das outras contas.
pub fn write(dir: &Path, files: &BTreeMap<String, String>, expect_digest: &str) -> Result<(), WriteError> {
    if let Some(bad) = files.keys().find(|n| !valid_name(n)) {
        return Err(WriteError::InvalidName(bad.clone()));
    }
    fs::create_dir_all(dir)?;
    let lock = Lock::acquire(dir)?;
    let result = (|| {
        let current = read(dir)?;
        if current.digest != expect_digest {
            return Err(WriteError::Changed);
        }
        for (name, content) in files.iter().filter(|(n, _)| n.as_str() != INDEX) {
            if current.files.get(name) != Some(content) {
                atomic_write(&dir.join(name), content.as_bytes())?;
            }
        }
        for name in current.files.keys().filter(|n| !files.contains_key(*n) && n.as_str() != INDEX) {
            fs::remove_file(dir.join(name))?;
        }
        match files.get(INDEX) {
            Some(index) if current.files.get(INDEX) != Some(index) => atomic_write(&dir.join(INDEX), index.as_bytes())?,
            Some(_) => {}
            None if current.files.contains_key(INDEX) => fs::remove_file(dir.join(INDEX))?,
            None => {}
        }
        Ok(())
    })();
    let released = lock.release(result.is_ok());
    result?;
    // A memória já está gravada; a trava que não soltou ainda precisa aparecer como falha.
    released.map_err(WriteError::Io)
}

/// Pastas que este processo está gravando: o PID no arquivo é o mesmo para duas gravações daqui.
static HELD: LazyLock<Mutex<HashSet<PathBuf>>> = LazyLock::new(Default::default);

struct Lock<'a> {
    dir: &'a Path,
    key: PathBuf,
    previous: Option<SystemTime>,
}

impl<'a> Lock<'a> {
    fn acquire(dir: &'a Path) -> Result<Self, WriteError> {
        let key = fs::canonicalize(dir)?;
        if !HELD.lock().unwrap_or_else(|e| e.into_inner()).insert(key.clone()) {
            return Err(WriteError::Locked);
        }
        let lock = Self { dir, key, previous: None };
        match lock.take() {
            Ok(previous) => Ok(Self { previous, ..lock }),
            Err(e) => {
                HELD.lock().unwrap_or_else(|e| e.into_inner()).remove(&lock.key);
                Err(e)
            }
        }
    }

    fn take(&self) -> Result<Option<SystemTime>, WriteError> {
        let path = self.dir.join(LOCK);
        let pid = std::process::id().to_string();
        // Sem trava: criar com `create_new` fecha a corrida com o nativo.
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                file.write_all(pid.as_bytes())?;
                return Ok(None);
            }
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.into()),
        }
        let previous = fs::metadata(&path).and_then(|m| m.modified()).ok();
        let fresh = previous.is_some_and(|mtime| SystemTime::now().duration_since(mtime).map_or(true, |age| age < LOCK_STALE));
        let holder = fs::read_to_string(&path).ok().and_then(|s| s.trim().parse::<u32>().ok());
        // PID que não dá para conferir conta como vivo só enquanto a trava é recente.
        if fresh && holder.is_some_and(|p| pid_alive(p).unwrap_or(true)) {
            return Err(WriteError::Locked);
        }
        atomic_write(&path, pid.as_bytes())?;
        // Quem assumiu a trava velha junto com outro processo vê o PID dele e desiste.
        if fs::read_to_string(&path)?.trim() != pid {
            return Err(WriteError::Locked);
        }
        Ok(previous)
    }

    /// Como o nativo: conteúdo vazio solta a trava e o mtime guarda a última consolidação; na falha o
    /// mtime volta ao de antes.
    fn release(self, done: bool) -> io::Result<()> {
        HELD.lock().unwrap_or_else(|e| e.into_inner()).remove(&self.key);
        let path = self.dir.join(LOCK);
        match (done, self.previous) {
            (false, None) => match fs::remove_file(&path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            },
            (false, Some(when)) => atomic_write(&path, b"")
                .and_then(|()| fs::File::options().write(true).open(&path))
                .and_then(|f| f.set_modified(when)),
            (true, _) => atomic_write(&path, b""),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(n, c)| (n.to_string(), c.to_string())).collect()
    }

    fn setup(pairs: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("memory");
        fs::create_dir(&dir).unwrap();
        for (n, c) in pairs {
            fs::write(dir.join(n), c).unwrap();
        }
        (tmp, dir)
    }

    #[test]
    fn writes_changes_and_removes_retired_notes() {
        let (_tmp, dir) = setup(&[("MEMORY.md", "- [A](a.md)\n- [B](b.md)\n"), ("a.md", "velha"), ("b.md", "sai")]);
        let before = read(&dir).unwrap();
        let after = files(&[("MEMORY.md", "- [A](a.md)\n- [C](c.md)\n"), ("a.md", "nova"), ("c.md", "entra")]);
        write(&dir, &after, &before.digest).unwrap();
        assert_eq!(read(&dir).unwrap().files, after);
        assert!(last_consolidated(&dir).is_some());
    }

    #[test]
    fn refuses_when_someone_wrote_after_the_read() {
        let (_tmp, dir) = setup(&[("MEMORY.md", "x"), ("a.md", "1")]);
        let before = read(&dir).unwrap();
        fs::write(dir.join("nova-da-sessao.md"), "gravada durante a rodada").unwrap();
        let err = write(&dir, &files(&[("MEMORY.md", "y")]), &before.digest).unwrap_err();
        assert!(matches!(err, WriteError::Changed));
        assert_eq!(fs::read_to_string(dir.join("nova-da-sessao.md")).unwrap(), "gravada durante a rodada");
        assert_eq!(fs::read_to_string(dir.join("MEMORY.md")).unwrap(), "x");
        assert!(!dir.join(LOCK).exists(), "trava nova ficou para trás");
    }

    #[test]
    fn live_lock_refuses_stale_or_dead_is_taken() {
        let (_tmp, dir) = setup(&[("MEMORY.md", "x")]);
        let before = read(&dir).unwrap();
        // PID 1 (no Windows, o 4 do System) vive e não é este processo: trava recente recusa.
        fs::write(dir.join(LOCK), if cfg!(windows) { "4" } else { "1" }).unwrap();
        assert!(matches!(write(&dir, &files(&[("MEMORY.md", "y")]), &before.digest), Err(WriteError::Locked)));
        let old = SystemTime::now() - Duration::from_secs(2 * 3600);
        fs::File::options().write(true).open(dir.join(LOCK)).unwrap().set_modified(old).unwrap();
        write(&dir, &files(&[("MEMORY.md", "y")]), &before.digest).unwrap();
        // Logo depois de uma gravação a trava está solta (conteúdo vazio), não presa por 1 h.
        let before = read(&dir).unwrap();
        write(&dir, &files(&[("MEMORY.md", "z")]), &before.digest).unwrap();
        // PID morto não segura a trava.
        let before = read(&dir).unwrap();
        fs::write(dir.join(LOCK), "999999999").unwrap();
        write(&dir, &files(&[("MEMORY.md", "w")]), &before.digest).unwrap();
        // PID deste processo segura: duas gravações juntas no mesmo servidor também se barram.
        let before = read(&dir).unwrap();
        fs::write(dir.join(LOCK), std::process::id().to_string()).unwrap();
        assert!(matches!(write(&dir, &files(&[("MEMORY.md", "v")]), &before.digest), Err(WriteError::Locked)));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_folder_stays_a_link() {
        let tmp = tempfile::tempdir().unwrap();
        let real = tmp.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("MEMORY.md"), "x").unwrap();
        let link = tmp.path().join("conta-memory");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let before = read(&link).unwrap();
        write(&link, &files(&[("MEMORY.md", "y")]), &before.digest).unwrap();
        assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(fs::read_to_string(real.join("MEMORY.md")).unwrap(), "y");
    }

    #[test]
    fn names_from_outside_cannot_escape() {
        let (_tmp, dir) = setup(&[]);
        let before = read(&dir).unwrap();
        for bad in ["../x.md", ".consolidate-lock", "a/b.md", "notas.txt"] {
            assert!(matches!(write(&dir, &files(&[(bad, "x")]), &before.digest), Err(WriteError::InvalidName(_))), "{bad}");
        }
    }
}
