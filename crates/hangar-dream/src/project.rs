//! Qual projeto é o mesmo entre máquinas e onde mora a memória dele.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(pub String);

impl ProjectId {
    /// `git:<host>/<dono>/<repo>`: o mesmo repositório clonado por https ou ssh dá o mesmo id.
    pub fn from_origin(url: &str) -> Option<Self> {
        let mut rest = url.trim().to_ascii_lowercase();
        let mut had_scheme = false;
        if let Some(i) = rest.find("://") {
            rest = rest[i + 3..].to_string();
            had_scheme = true;
        }
        if let Some(at) = rest.find('@') {
            if !rest[..at].contains('/') {
                rest = rest[at + 1..].to_string();
            }
        }
        if let Some(colon) = rest.find(':') {
            if !rest[..colon].contains('/') {
                let host = &rest[..colon];
                let mut path = &rest[colon + 1..];
                // Porta só existe com esquema; sem ele, `host:123org/repo` é dono que começa com dígito.
                if had_scheme {
                    if let Some(slash) = path.find('/') {
                        if slash > 0 && path[..slash].bytes().all(|b| b.is_ascii_digit()) {
                            path = &path[slash + 1..];
                        }
                    }
                }
                rest = format!("{host}/{}", path.trim_start_matches('/'));
            }
        }
        let trimmed = rest.trim_end_matches('/');
        let trimmed = trimmed.strip_suffix(".git").unwrap_or(trimmed).trim_end_matches('/');
        let (host, path) = trimmed.split_once('/')?;
        if host.is_empty() || path.is_empty() {
            return None;
        }
        Some(Self(format!("git:{trimmed}")))
    }

    pub fn from_dir(root: &Path) -> Self {
        let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Self(format!("dir:{name}"))
    }

    /// Projeto sem git só junta máquinas depois de confirmado na tela.
    pub fn is_dir(&self) -> bool {
        self.0.starts_with("dir:")
    }
}

/// Nome da pasta em `projects/`, como o Claude Code: todo caractere fora de `[A-Za-z0-9]` vira `-`.
pub fn sanitize(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

pub fn memory_dir(claude_base: &Path, root: &Path) -> PathBuf {
    claude_base.join("projects").join(sanitize(&root.to_string_lossy())).join("memory")
}

/// Raiz do checkout principal: a memória de uma worktree é a do repositório dela.
pub fn canonical_root(cwd: &Path) -> Option<PathBuf> {
    if !cwd.is_dir() {
        return None;
    }
    let common = git(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = PathBuf::from(common);
    if common.file_name().is_some_and(|n| n == ".git") {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(common)
    }
}

pub fn project_of(root: &Path) -> ProjectId {
    git(root, &["config", "--get", "remote.origin.url"])
        .and_then(|url| ProjectId::from_origin(&url))
        .unwrap_or_else(|| ProjectId::from_dir(root))
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_and_ssh_are_the_same_project() {
        let want = Some(ProjectId("git:github.com/jeffer1312/hangar".into()));
        assert_eq!(ProjectId::from_origin("git@github.com:jeffer1312/hangar.git"), want);
        assert_eq!(ProjectId::from_origin("https://github.com/jeffer1312/hangar"), want);
        assert_eq!(ProjectId::from_origin("https://github.com/Jeffer1312/hangar.git/"), want);
        assert_eq!(ProjectId::from_origin("ssh://git@github.com:22/jeffer1312/hangar.git"), want);
    }

    #[test]
    fn scp_owner_starting_with_digit_keeps_it() {
        assert_eq!(
            ProjectId::from_origin("git@gitlab.example:1password/app.git"),
            Some(ProjectId("git:gitlab.example/1password/app".into()))
        );
    }

    #[test]
    fn origin_without_path_is_not_a_project() {
        assert_eq!(ProjectId::from_origin("github.com"), None);
        assert_eq!(ProjectId::from_origin(""), None);
    }

    #[test]
    fn sanitize_matches_claude_code_folder_names() {
        assert_eq!(
            sanitize("/home/jefferson/.hangar/worktrees/hangar/dreaming"),
            "-home-jefferson--hangar-worktrees-hangar-dreaming"
        );
        assert_eq!(sanitize("/home/jefferson/Área de trabalho"), "-home-jefferson--rea-de-trabalho");
    }

    #[test]
    fn worktree_resolves_to_main_checkout() {
        let tmp = tempfile::tempdir().unwrap();
        let main = tmp.path().join("main");
        std::fs::create_dir(&main).unwrap();
        let run = |dir: &Path, args: &[&str]| {
            let ok = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@t")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@t")
                .status()
                .unwrap()
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&main, &["init", "-q"]);
        run(&main, &["commit", "-q", "--allow-empty", "-m", "x"]);
        let wt = tmp.path().join("wt");
        run(&main, &["worktree", "add", "-q", wt.to_str().unwrap()]);
        let want = main.canonicalize().unwrap();
        assert_eq!(canonical_root(&wt).map(|p| p.canonicalize().unwrap()), Some(want.clone()));
        assert_eq!(canonical_root(&main).map(|p| p.canonicalize().unwrap()), Some(want));
        assert_eq!(canonical_root(&tmp.path().join("sumiu")), None);
    }
}
