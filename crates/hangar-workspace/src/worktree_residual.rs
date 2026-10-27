//! Recuperação manual de uma pasta registrada que perdeu o próprio `.git`.
use crate::{Result, error, git, real, worktrees};
use std::{
    fs,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
};

const REMOVAL_LOCK: &str = "hangar-removal\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Registration {
    pub admin: PathBuf,
    pub path: PathBuf,
    pub common: PathBuf,
    pub branch: Option<String>,
    head: String,
    gitdir: String,
    commondir: String,
}

fn plain_file(path: &Path) -> Result<String> {
    if !fs::symlink_metadata(path).is_ok_and(|m| m.is_file() && !m.is_symlink()) {
        return Err(error(409, "registro administrativo da worktree inválido"));
    }
    fs::read_to_string(path).map_err(|_| error(409, "registro administrativo da worktree ilegível"))
}

pub(crate) fn missing_git(path: &Path) -> bool {
    fs::symlink_metadata(path.join(".git")).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
}

pub(crate) fn registration(main: &str, path: &str) -> Result<Registration> {
    let main = real(Path::new(main));
    let path = Path::new(path);
    if fs::symlink_metadata(path).is_ok_and(|m| m.is_symlink() || !m.is_dir()) {
        return Err(error(409, "a pasta residual foi substituída"));
    }
    let path = real(path);
    if main.starts_with(&path) || path.starts_with(main.join(".git")) {
        return Err(error(409, "o checkout principal é protegido"));
    }
    let common = real(&main.join(".git"));
    let entries = fs::read_dir(common.join("worktrees"))
        .map_err(|_| error(404, "não é uma worktree deste repositório"))?;
    let mut found = None;
    for entry in entries {
        let entry = entry.map_err(|_| error(409, "registro de worktrees ilegível"))?;
        if !entry
            .file_type()
            .is_ok_and(|t| t.is_dir() && !t.is_symlink())
        {
            continue;
        }
        let admin = entry.path();
        let gitdir = plain_file(&admin.join("gitdir"))?;
        let target = worktrees::normpath(&admin.join(gitdir.trim()));
        if target.file_name().is_none_or(|n| n != ".git") || real(target.parent().unwrap()) != path
        {
            continue;
        }
        let commondir = plain_file(&admin.join("commondir"))?;
        if real(&admin.join(commondir.trim())) != common {
            return Err(error(409, "vínculo administrativo da worktree inválido"));
        }
        let head = plain_file(&admin.join("HEAD"))?;
        let branch = head
            .trim()
            .strip_prefix("ref: refs/heads/")
            .filter(|b| !b.is_empty())
            .map(str::to_owned);
        if found.is_some() {
            return Err(error(409, "registro administrativo da worktree ambíguo"));
        }
        found = Some(Registration {
            admin,
            path: path.clone(),
            common: common.clone(),
            branch,
            head,
            gitdir,
            commondir,
        });
    }
    found.ok_or_else(|| error(404, "não é uma worktree deste repositório"))
}

impl Registration {
    pub fn head(&self, main: &str) -> Option<String> {
        let admin = self.admin.to_str()?;
        // O diretório pai nunca fornece HEAD ou branch a uma pasta residual.
        git::command(
            Path::new(main),
            &["--git-dir", admin, "rev-parse", "--verify", "HEAD^{commit}"],
        )
        .ok()
        .filter(|o| o.code == 0)
        .map(|o| o.stdout.trim().to_owned())
    }

    pub fn locked(&self) -> Result<bool> {
        match fs::symlink_metadata(self.admin.join("locked")) {
            Ok(_) => Ok(plain_file(&self.admin.join("locked"))? != REMOVAL_LOCK),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(_) => Err(error(409, "trava da worktree ilegível")),
        }
    }

    pub fn removing(&self) -> bool {
        plain_file(&self.admin.join("locked")).is_ok_and(|s| s == REMOVAL_LOCK)
    }

    fn check_git_locks(&self) -> Result<()> {
        let mut paths = vec![
            self.admin.join("HEAD.lock"),
            self.admin.join("index.lock"),
            self.common.join("packed-refs.lock"),
            self.common.join("config.lock"),
        ];
        if let Some(branch) = &self.branch {
            // A referência só vira caminho depois de passar pela validação do Git.
            let reference = format!("refs/heads/{branch}");
            git::checked(&self.common, &["check-ref-format", &reference], 409)?;
            paths.push(self.common.join(format!("{reference}.lock")));
        }
        for path in paths {
            match fs::symlink_metadata(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(error(409, "worktree ocupada pelo Git")),
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Inventory {
    pub files: Vec<String>,
    pub entries: u64,
    stamp: u64,
}

pub(crate) fn inventory(root: &Path) -> Result<Inventory> {
    let mut stack = vec![root.to_path_buf()];
    let mut records = Vec::new();
    let mut files = Vec::new();
    while let Some(path) = stack.pop() {
        let meta = fs::symlink_metadata(&path)
            .map_err(|_| error(409, "arquivo residual ilegível; confira a pasta"))?;
        let rel = path.strip_prefix(root).unwrap().to_path_buf();
        if !rel.as_os_str().is_empty() {
            files.push(rel.to_string_lossy().into_owned());
        }
        #[cfg(unix)]
        let identity = {
            use std::os::unix::fs::MetadataExt;
            (meta.dev(), meta.ino())
        };
        #[cfg(not(unix))]
        let identity = (0u64, 0u64);
        records.push((
            rel,
            meta.len(),
            meta.modified()
                .map_err(|_| error(409, "data de arquivo residual ilegível"))?,
            meta.is_dir(),
            meta.is_symlink(),
            identity,
        ));
        if meta.is_dir() && !meta.is_symlink() {
            for entry in fs::read_dir(&path)
                .map_err(|_| error(409, "pasta residual ilegível; confira a pasta"))?
            {
                stack.push(
                    entry
                        .map_err(|_| error(409, "pasta residual ilegível"))?
                        .path(),
                );
            }
        }
    }
    records.sort();
    files.sort();
    let entries = files.len() as u64;
    files.truncate(50);
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    records.hash(&mut hash);
    Ok(Inventory {
        files,
        entries,
        stamp: hash.finish(),
    })
}

/// O lock do Git impede o prune; o lock do sistema distingue execução viva de retomada.
pub(crate) fn remove(main: &str, path: &str, guard: &dyn Fn(&str) -> Result<()>) -> Result<()> {
    use std::io::Write;
    let before = registration(main, path)?;
    if before.locked()? {
        return Err(error(409, "worktree bloqueada"));
    }
    let lease = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(before.admin.join("hangar-removal.lock"))
        .map_err(|_| error(409, "trava de remoção indisponível"))?;
    lease
        .try_lock()
        .map_err(|_| error(409, "remoção da worktree em andamento"))?;
    let marker = before.admin.join("locked");
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&marker)
    {
        Ok(mut file) => file
            .write_all(REMOVAL_LOCK.as_bytes())
            .map_err(|_| error(409, "não consegui proteger o registro"))?,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && !before.locked()? => {}
        Err(_) => return Err(error(409, "worktree bloqueada")),
    }
    let files = if before.path.exists() {
        Some(inventory(&before.path)?)
    } else {
        None
    };
    guard(path)?;
    if registration(main, path)? != before
        || before.locked()?
        || !missing_git(&before.path)
        || (if before.path.exists() {
            Some(inventory(&before.path)?)
        } else {
            None
        }) != files
    {
        return Err(error(409, "a pasta residual mudou durante a remoção"));
    }
    before.check_git_locks()?;
    // Não há subprocesso com prazo que possa matar a remoção no meio; o registro sai por último.
    if files.is_some() {
        // A biblioteca remove por handles e não segue uma troca concorrente por link.
        fs::remove_dir_all(&before.path).map_err(|_| {
            error(
                409,
                "remoção incompleta; confira os arquivos restantes e tente de novo",
            )
        })?;
    }
    guard(path)?;
    if registration(main, path)? != before || before.locked()? {
        return Err(error(409, "o registro mudou durante a remoção"));
    }
    before.check_git_locks()?;
    drop(lease);
    fs::remove_file(before.admin.join("hangar-removal.lock"))
        .map_err(|_| error(409, "não consegui concluir a remoção"))?;
    fs::remove_file(marker).map_err(|_| error(409, "não consegui concluir a remoção"))?;
    git::remove_worktree(Path::new(main), path, false)?;
    Ok(())
}
