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

/// Falha com os caminhos envolvidos: sem eles o erro não diz qual arquivo travou a exclusão.
#[derive(Debug)]
pub struct MergeError {
    pub source: Option<PathBuf>,
    pub target: PathBuf,
    pub error: io::Error,
}

impl std::fmt::Display for MergeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(source) = &self.source {
            write!(f, "{} → ", source.display())?;
        }
        write!(f, "{}: {}", self.target.display(), self.error)
    }
}

fn at(source: Option<&Path>, target: &Path) -> impl FnOnce(io::Error) -> MergeError {
    let (source, target) = (source.map(Path::to_owned), target.to_owned());
    move |error| MergeError {
        source,
        target,
        error,
    }
}

/// Junta várias árvores sob o mesmo rótulo; `finish` só volta depois de as pastas tocadas
/// estarem no disco, porque logo em seguida a origem é apagada.
pub struct Merge<'a> {
    label: &'a str,
    /// Raízes onde um link não guarda nada da conta (a conta padrão de destino), resolvidas.
    homes: Vec<PathBuf>,
    count: MergeCount,
    touched: BTreeSet<PathBuf>,
}

impl<'a> Merge<'a> {
    pub fn new(label: &'a str, homes: &[&Path]) -> Self {
        Self {
            label,
            homes: homes.iter().map(|home| resolve_loose(home)).collect(),
            count: MergeCount::default(),
            touched: BTreeSet::new(),
        }
    }

    /// Copia `from` para `to` no mesmo caminho relativo, sem sobrescrever nada.
    pub fn tree(&mut self, from: &Path, to: &Path) -> Result<(), MergeError> {
        for entry in fs::read_dir(from).map_err(at(Some(from), to))? {
            let entry = entry.map_err(at(Some(from), to))?;
            let path = entry.path();
            let kind = entry.file_type().map_err(at(Some(&path), to))?;
            let target = to.join(entry.file_name());
            if kind.is_dir() {
                self.tree(&path, &target)?;
            } else if kind.is_file() {
                self.file(&path, &target)?;
            } else if !self.links_into_home(&path) {
                // Link para fora, socket ou fifo: seguir levaria à conta padrão o que mora fora
                // da conta, e pular deixaria a exclusão apagá-lo sem aviso. Recusa e a conta fica.
                return Err(at(Some(&path), &target)(io::Error::other("não é arquivo nem pasta")));
            }
        }
        Ok(())
    }

    /// Link cujo alvo fica numa das raízes (o `memory/` de cada projeto do Claude aponta para o
    /// compartilhado) não tem nada da conta: é pulado e sai com ela. Quebrado, vale o alvo
    /// escrito no link, resolvido contra a pasta dele.
    pub fn links_into_home(&self, path: &Path) -> bool {
        if !fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
            return false;
        }
        let target = match fs::canonicalize(path) {
            Ok(real) => real,
            Err(error) if error.kind() == io::ErrorKind::NotFound => match fs::read_link(path) {
                Ok(written) => resolve_loose(&path.parent().unwrap_or(Path::new("")).join(written)),
                Err(_) => return false,
            },
            Err(_) => return false,
        };
        self.homes.iter().any(|home| target.starts_with(home))
    }

    pub fn finish(self) -> Result<MergeCount, MergeError> {
        #[cfg(unix)]
        for dir in &self.touched {
            fs::File::open(dir)
                .and_then(|d| d.sync_all())
                .map_err(at(None, dir))?;
        }
        Ok(self.count)
    }

    fn file(&mut self, source: &Path, target: &Path) -> Result<(), MergeError> {
        if let Some(parent) = target.parent() {
            self.make_dirs(parent).map_err(at(Some(source), parent))?;
        }
        let mut attempt = 0u32;
        loop {
            let candidate = if attempt == 0 {
                target.to_owned()
            } else {
                renamed(target, self.label, attempt)
            };
            match fs::symlink_metadata(&candidate) {
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    match copy_new(source, &candidate) {
                        Ok(()) => {}
                        // Uma sessão da conta padrão criou o mesmo nome agora: reavalia o candidato.
                        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                        Err(error) => return Err(at(Some(source), &candidate)(error)),
                    }
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
                Err(error) => return Err(at(Some(source), &candidate)(error)),
                Ok(meta) if meta.is_file() => {
                    if same_bytes(source, &candidate).map_err(at(Some(source), &candidate))? {
                        self.count.skipped += 1;
                        return Ok(());
                    }
                }
                Ok(_) => {}
            }
            attempt += 1;
        }
    }

    /// Cria só para o dono as pastas que faltam (as do Claude são 0700) e marca o pai de cada
    /// uma para o `finish`: a entrada da pasta nova também precisa chegar ao disco.
    fn make_dirs(&mut self, dir: &Path) -> io::Result<()> {
        if dir.is_dir() {
            return Ok(());
        }
        if let Some(parent) = dir.parent() {
            self.make_dirs(parent)?;
        }
        let mut builder = fs::DirBuilder::new();
        #[cfg(unix)]
        std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
        match builder.create(dir) {
            Ok(()) => {
                if let Some(parent) = dir.parent() {
                    self.touched.insert(parent.to_owned());
                }
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists && dir.is_dir() => Ok(()),
            Err(error) => Err(error),
        }
    }
}

/// `canonicalize` de um caminho que pode não existir (alvo de link quebrado, padrão ainda sem
/// pasta): resolve o maior prefixo que existe e anexa o resto. Raiz e alvo ficam na mesma forma
/// mesmo com um link no caminho (no macOS o `/var` é `/private/var`).
fn resolve_loose(path: &Path) -> PathBuf {
    let path = lexical(path);
    let mut rest = Vec::new();
    let mut prefix = path.as_path();
    loop {
        if let Ok(real) = fs::canonicalize(prefix) {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (prefix.parent(), prefix.file_name()) {
            (Some(parent), Some(name)) => { rest.push(name.to_owned()); prefix = parent; }
            _ => return path,
        }
    }
}

/// Tira `.` e `..` sem tocar no disco: o alvo de um link quebrado não existe para o `canonicalize`.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            std::path::Component::ParentDir => { out.pop(); }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
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
    let mut input = fs::File::open(source)?;
    let meta = input.metadata()?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut output = options.open(target)?;
    let result = (|| {
        if io::copy(&mut input, &mut output)? != meta.len() {
            return Err(io::Error::other("cópia incompleta"));
        }
        output.set_modified(meta.modified()?)?;
        // Transcript do Claude é 0600: a cópia não pode ficar legível por outros usuários.
        #[cfg(unix)]
        output.set_permissions(meta.permissions())?;
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
        let mut merge = Merge::new("work", &[]);
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
        let mut again = Merge::new("work", &[]);
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
        let mut merge = Merge::new("work", &[]);
        merge.tree(&from, &to).unwrap();
        assert_eq!(merge.finish().unwrap().renamed, 1);
        assert_eq!(
            fs::read_to_string(to.join("a.from-work-2.jsonl")).unwrap(),
            "three"
        );
    }

    #[cfg(unix)]
    #[test]
    fn copies_keep_the_file_mode_and_new_dirs_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("from"), root.path().join("to"));
        write(&from.join("-p/s/a.jsonl"), "x");
        fs::set_permissions(from.join("-p/s/a.jsonl"), fs::Permissions::from_mode(0o600)).unwrap();
        let mut merge = Merge::new("work", &[]);
        merge.tree(&from, &to).unwrap();
        merge.finish().unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&to.join("-p/s/a.jsonl")), 0o600);
        assert_eq!(mode(&to), 0o700);
        assert_eq!(mode(&to.join("-p/s")), 0o700);
    }

    #[test]
    fn failure_names_source_and_target() {
        let root = tempfile::tempdir().unwrap();
        let (from, to) = (root.path().join("from"), root.path().join("to"));
        write(&from.join("-p/a.jsonl"), "x");
        write(&to.join("-p"), "um arquivo no lugar da pasta");
        let error = Merge::new("work", &[]).tree(&from, &to).unwrap_err();
        assert_eq!(error.source.as_deref(), Some(from.join("-p/a.jsonl").as_path()));
        assert_eq!(error.target, to.join("-p"));
    }
}
