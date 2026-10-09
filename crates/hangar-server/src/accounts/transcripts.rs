//! Conversas de uma conta apagada vão para a conta padrão antes de a pasta sumir.
use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MergeCount {
    pub merged: u64,
    pub skipped: u64,
    pub renamed: u64,
}

/// Junta várias árvores sob o mesmo rótulo; `finish` só volta depois de as pastas tocadas
/// estarem no disco, porque logo em seguida a origem é apagada.
pub struct Merge<'a> {
    label: &'a str,
    count: MergeCount,
    touched: BTreeSet<PathBuf>,
}

impl<'a> Merge<'a> {
    pub fn new(label: &'a str) -> Self {
        Self {
            label,
            count: MergeCount::default(),
            touched: BTreeSet::new(),
        }
    }

    /// Copia `from` para `to` no mesmo caminho relativo, sem sobrescrever nada.
    pub fn tree(&mut self, from: &Path, to: &Path) -> io::Result<()> {
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let target = to.join(entry.file_name());
            if kind.is_dir() {
                self.tree(&entry.path(), &target)?;
            } else if kind.is_file() {
                self.file(&entry.path(), &target)?;
            }
            // Link não é seguido: levaria para a conta padrão o que mora fora da conta.
        }
        Ok(())
    }

    pub fn finish(self) -> io::Result<MergeCount> {
        #[cfg(unix)]
        for dir in &self.touched {
            fs::File::open(dir)?.sync_all()?;
        }
        Ok(self.count)
    }

    fn file(&mut self, source: &Path, target: &Path) -> io::Result<()> {
        for attempt in 0u32.. {
            let candidate = if attempt == 0 {
                target.to_owned()
            } else {
                renamed(target, self.label, attempt)
            };
            match fs::symlink_metadata(&candidate) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    copy_new(source, &candidate)?;
                    if let Some(parent) = candidate.parent() {
                        self.touched.insert(parent.to_owned());
                    }
                    if attempt == 0 {
                        self.count.merged += 1;
                    } else {
                        self.count.renamed += 1;
                    }
                    return Ok(());
                }
                Err(error) => return Err(error),
                Ok(meta) if meta.is_file() && same_bytes(source, &candidate)? => {
                    self.count.skipped += 1;
                    return Ok(());
                }
                Ok(_) => {}
            }
        }
        unreachable!("a sequência de nomes alternativos não termina")
    }
}

/// `<stem>.from-<label><ext>`, e `-N` a partir da segunda colisão.
fn renamed(target: &Path, label: &str, attempt: u32) -> PathBuf {
    let mut name = OsString::from(target.file_stem().unwrap_or_default());
    name.push(format!(".from-{label}"));
    if attempt > 1 {
        name.push(format!("-{attempt}"));
    }
    if let Some(ext) = target.extension() {
        name.push(".");
        name.push(ext);
    }
    target.with_file_name(name)
}

fn copy_new(source: &Path, target: &Path) -> io::Result<()> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut input = fs::File::open(source)?;
    let meta = input.metadata()?;
    let mut output = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(target)?;
    let result = (|| {
        if io::copy(&mut input, &mut output)? != meta.len() {
            return Err(io::Error::other("cópia incompleta"));
        }
        output.set_modified(meta.modified()?)?;
        output.sync_all()
    })();
    // O arquivo é nosso (create_new): cópia pela metade não fica para a próxima tentativa.
    if result.is_err() {
        let _ = fs::remove_file(target);
    }
    result
}

fn same_bytes(a: &Path, b: &Path) -> io::Result<bool> {
    let (mut a, mut b) = (fs::File::open(a)?, fs::File::open(b)?);
    if a.metadata()?.len() != b.metadata()?.len() {
        return Ok(false);
    }
    let (mut x, mut y) = (Vec::with_capacity(1 << 16), Vec::with_capacity(1 << 16));
    loop {
        x.clear();
        y.clear();
        (&mut a).take(1 << 16).read_to_end(&mut x)?;
        (&mut b).take(1 << 16).read_to_end(&mut y)?;
        if x != y {
            return Ok(false);
        }
        if x.is_empty() {
            return Ok(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn merges_skips_identical_and_renames_conflicts() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("from"), root.path().join("to"));
        write(&from.join("-p/new.jsonl"), "new");
        write(&from.join("-p/same.jsonl"), "same");
        write(&from.join("-p/diff.jsonl"), "mine");
        write(&from.join("-p/uuid/subagents/a.jsonl"), "sub");
        write(&to.join("-p/same.jsonl"), "same");
        write(&to.join("-p/diff.jsonl"), "theirs");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000);
        fs::File::options()
            .write(true)
            .open(from.join("-p/new.jsonl"))
            .unwrap()
            .set_modified(old)
            .unwrap();
        let mut merge = Merge::new("work");
        merge.tree(&from, &to).unwrap();
        let count = merge.finish().unwrap();
        assert_eq!(
            count,
            MergeCount {
                merged: 2,
                skipped: 1,
                renamed: 1
            }
        );
        assert_eq!(fs::read_to_string(to.join("-p/new.jsonl")).unwrap(), "new");
        assert_eq!(
            fs::metadata(to.join("-p/new.jsonl")).unwrap().modified().unwrap(),
            old
        );
        assert_eq!(fs::read_to_string(to.join("-p/diff.jsonl")).unwrap(), "theirs");
        assert_eq!(
            fs::read_to_string(to.join("-p/diff.from-work.jsonl")).unwrap(),
            "mine"
        );
        assert_eq!(
            fs::read_to_string(to.join("-p/uuid/subagents/a.jsonl")).unwrap(),
            "sub"
        );
        // Repetir depois de uma falha não duplica nada.
        let mut again = Merge::new("work");
        again.tree(&from, &to).unwrap();
        assert_eq!(
            again.finish().unwrap(),
            MergeCount {
                merged: 0,
                skipped: 4,
                renamed: 0
            }
        );
    }

    #[test]
    fn second_conflict_gets_a_numbered_name() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("from"), root.path().join("to"));
        write(&from.join("a.jsonl"), "three");
        write(&to.join("a.jsonl"), "one");
        write(&to.join("a.from-work.jsonl"), "two");
        let mut merge = Merge::new("work");
        merge.tree(&from, &to).unwrap();
        assert_eq!(merge.finish().unwrap().renamed, 1);
        assert_eq!(
            fs::read_to_string(to.join("a.from-work-2.jsonl")).unwrap(),
            "three"
        );
    }
}
