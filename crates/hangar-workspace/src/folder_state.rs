//! A pasta de uma abertura vista pelo balanceamento: raiz git, `origin`, branch e o que ainda não foi
//! publicado. Trabalho que só existe aqui prende a sessão nesta máquina.
use crate::{Result, process::run, unavailable};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

/// Com a máquina ocupada um `git status` passa de 2 s sem nada errado.
const TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pending {
    /// Mudança não commitada ou arquivo novo; só fim de linha não conta.
    Dirty,
    /// Commit que o upstream não tem.
    Ahead,
    /// Branch sem upstream, upstream apagado, HEAD solto ou repositório sem commit.
    NoUpstream,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitFolder {
    pub root: PathBuf,
    pub origin: Option<String>,
    pub branch: Option<String>,
    pub pending: Option<Pending>,
}

/// A raiz git que contém `dir` (já resolvido por [`crate::real`]), pelo `.git` em disco: sem processo,
/// para comparar dezenas de sessões a cada abertura.
pub fn git_root(dir: &Path) -> Option<PathBuf> {
    dir.ancestors().find(|a| a.join(".git").exists()).map(Path::to_path_buf)
}

/// Chave de comparação exata de pastas: no Windows `\` e `/` e maiúsculas não distinguem.
pub fn path_key(path: &Path) -> String {
    normalize(&path.to_string_lossy(), cfg!(windows))
}

fn normalize(text: &str, windows: bool) -> String {
    let text = if windows { text.replace('\\', "/").to_lowercase() } else { text.to_owned() };
    match text.trim_end_matches('/') {
        "" => text,
        trimmed => trimmed.to_owned(),
    }
}

/// O estado de `root`, uma raiz de [`git_root`]. Falha do git é erro, nunca "limpo".
pub fn read(root: &Path) -> Result<GitFolder> {
    let status = run(root, &["status", "--porcelain=v2", "--branch", "--untracked-files=normal"], TIMEOUT)?;
    if status.code != 0 {
        return Err(unavailable(status.reason("git status falhou")));
    }
    let parsed = Status::parse(&status.stdout);
    let dirty = parsed.untracked || (parsed.changed && (parsed.initial || differs_beyond_eol(root)?));
    let pending = if dirty {
        Some(Pending::Dirty)
    } else if parsed.branch.is_none() || !parsed.tracked {
        Some(Pending::NoUpstream)
    } else if parsed.ahead > 0 {
        Some(Pending::Ahead)
    } else {
        None
    };
    let origin = run(root, &["config", "--get", "remote.origin.url"], TIMEOUT)?;
    let origin = (origin.code == 0).then(|| origin.stdout.trim().to_owned()).filter(|o| !o.is_empty());
    Ok(GitFolder { root: root.to_path_buf(), origin, branch: parsed.branch, pending })
}

/// `core.autocrlf` no Windows marca arquivo como mudado só pelo `\r`; isso não é trabalho a perder.
fn differs_beyond_eol(root: &Path) -> Result<bool> {
    let out = run(root, &["diff", "HEAD", "--ignore-cr-at-eol", "--quiet"], TIMEOUT)?;
    match out.code {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(unavailable(out.reason("git diff falhou"))),
    }
}

#[derive(Default)]
struct Status {
    branch: Option<String>,
    /// Upstream configurado e presente (`branch.ab` só sai assim).
    tracked: bool,
    ahead: u64,
    initial: bool,
    changed: bool,
    untracked: bool,
}

impl Status {
    fn parse(out: &str) -> Self {
        let mut s = Self::default();
        for line in out.lines() {
            if let Some(oid) = line.strip_prefix("# branch.oid ") {
                s.initial = oid == "(initial)";
            } else if let Some(head) = line.strip_prefix("# branch.head ") {
                s.branch = (head != "(detached)").then(|| head.to_owned());
            } else if let Some(ab) = line.strip_prefix("# branch.ab ") {
                s.tracked = true;
                s.ahead = ab.split_whitespace().next().and_then(|a| a.strip_prefix('+')).and_then(|a| a.parse().ok()).unwrap_or(0);
            } else if line.starts_with("? ") || line.starts_with("u ") {
                s.untracked = true;
            } else if line.starts_with("1 ") || line.starts_with("2 ") {
                s.changed = true;
            }
        }
        s
    }
}

#[cfg(test)]
mod placement_tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "init.defaultBranch=main"])
            .args(args)
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    /// Repositório com um commit, publicado num remoto local e com upstream.
    fn published() -> (tempfile::TempDir, PathBuf) {
        let base = tempfile::tempdir().unwrap();
        let (remote, repo) = (base.path().join("remote.git"), crate::real(base.path()).join("repo"));
        std::fs::create_dir_all(&repo).unwrap();
        git(base.path(), &["init", "-q", "--bare", remote.to_str().unwrap()]);
        git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("a.txt"), "a\nb\n").unwrap();
        git(&repo, &["add", "a.txt"]);
        git(&repo, &["commit", "-qm", "um"]);
        git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
        git(&repo, &["push", "-q", "-u", "origin", "main"]);
        (base, repo)
    }

    #[test]
    fn a_published_clean_branch_has_nothing_pending() {
        let (base, repo) = published();
        let folder = read(&repo).unwrap();
        assert_eq!(folder.pending, None);
        assert_eq!(folder.branch.as_deref(), Some("main"));
        assert_eq!(folder.origin, Some(base.path().join("remote.git").to_string_lossy().into_owned()));
        assert_eq!(folder.root, repo);
    }

    #[test]
    fn a_new_file_is_dirty() {
        let (_base, repo) = published();
        std::fs::write(repo.join("novo.txt"), "x").unwrap();
        assert_eq!(read(&repo).unwrap().pending, Some(Pending::Dirty));
    }

    #[test]
    fn an_edited_or_staged_file_is_dirty() {
        let (_base, repo) = published();
        std::fs::write(repo.join("a.txt"), "a\nc\n").unwrap();
        assert_eq!(read(&repo).unwrap().pending, Some(Pending::Dirty));
        git(&repo, &["add", "a.txt"]);
        assert_eq!(read(&repo).unwrap().pending, Some(Pending::Dirty));
    }

    #[test]
    fn a_line_ending_only_change_is_not_pending() {
        let (_base, repo) = published();
        std::fs::write(repo.join("a.txt"), "a\r\nb\r\n").unwrap();
        assert_eq!(read(&repo).unwrap().pending, None);
    }

    #[test]
    fn a_commit_not_pushed_is_ahead() {
        let (_base, repo) = published();
        std::fs::write(repo.join("a.txt"), "a\nc\n").unwrap();
        git(&repo, &["commit", "-qam", "dois"]);
        assert_eq!(read(&repo).unwrap().pending, Some(Pending::Ahead));
    }

    #[test]
    fn a_branch_without_upstream_or_with_a_deleted_one_is_not_published() {
        let (_base, repo) = published();
        git(&repo, &["switch", "-q", "-c", "nova"]);
        assert_eq!(read(&repo).unwrap().pending, Some(Pending::NoUpstream));
        git(&repo, &["push", "-q", "-u", "origin", "nova"]);
        assert_eq!(read(&repo).unwrap().pending, None);
        git(&repo, &["push", "-q", "origin", "--delete", "nova"]);
        assert_eq!(read(&repo).unwrap().pending, Some(Pending::NoUpstream));
    }

    #[test]
    fn a_detached_head_or_a_repo_without_commits_is_not_published() {
        let (_base, repo) = published();
        git(&repo, &["switch", "-q", "--detach"]);
        let folder = read(&repo).unwrap();
        assert_eq!((folder.pending, folder.branch), (Some(Pending::NoUpstream), None));
        let empty = tempfile::tempdir().unwrap();
        git(empty.path(), &["init", "-q"]);
        assert_eq!(read(empty.path()).unwrap().pending, Some(Pending::NoUpstream));
        std::fs::write(empty.path().join("a.txt"), "a").unwrap();
        git(empty.path(), &["add", "a.txt"]);
        assert_eq!(read(empty.path()).unwrap().pending, Some(Pending::Dirty));
    }

    #[test]
    fn a_folder_without_origin_has_no_origin() {
        let empty = tempfile::tempdir().unwrap();
        git(empty.path(), &["init", "-q"]);
        assert_eq!(read(empty.path()).unwrap().origin, None);
    }

    #[test]
    fn the_root_is_found_from_a_subfolder_and_a_worktree() {
        let (base, repo) = published();
        let sub = repo.join("src").join("deep");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(git_root(&sub), Some(repo.clone()));
        let tree = crate::real(base.path()).join("tree");
        git(&repo, &["worktree", "add", "-q", "-b", "w", tree.to_str().unwrap()]);
        assert_eq!(git_root(&tree), Some(tree.clone()));
        let plain = crate::real(base.path()).join("plain");
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(git_root(&plain), None);
    }

    #[test]
    fn a_folder_outside_any_repo_is_an_error_not_clean() {
        let plain = tempfile::tempdir().unwrap();
        assert!(read(plain.path()).is_err());
    }

    #[test]
    fn windows_keys_ignore_separator_case_and_trailing_slash() {
        assert_eq!(normalize(r"C:\Users\Jeff\Hangar\", true), "c:/users/jeff/hangar");
        assert_eq!(normalize("c:/users/jeff/hangar", true), "c:/users/jeff/hangar");
        assert_eq!(normalize("/home/Jeff/hangar/", false), "/home/Jeff/hangar");
        assert_ne!(normalize("/home/Jeff/hangar", false), normalize("/home/jeff/hangar", false));
        assert_eq!(normalize("/", false), "/");
    }
}
