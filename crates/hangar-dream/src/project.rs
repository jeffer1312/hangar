//! Qual projeto é o mesmo entre máquinas e onde mora a memória dele.

use std::path::{Path, PathBuf};
use std::process::Command;

use hangar_core::project::ProjectId;

/// Nome da pasta em `projects/`, como o Claude Code: cada unidade UTF-16 fora de `[A-Za-z0-9]` vira
/// `-` (um emoji vira dois).
/// shortcut: o Claude Code encurta com hash os nomes acima de 200 caracteres e isto não; caminho tão
/// longo aponta para outra pasta. Ler o algoritmo no binário se aparecer um caso.
pub fn sanitize(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else {
            for _ in 0..c.len_utf16() {
                out.push('-');
            }
        }
    }
    out
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
        // Submódulo (`.git/modules/x`) ou git-dir fora do lugar: a raiz é a da árvore de trabalho.
        git(cwd, &["rev-parse", "--show-toplevel"]).map(PathBuf::from)
    }
}

pub fn project_of(root: &Path) -> ProjectId {
    git(root, &["config", "--get", "remote.origin.url"])
        .and_then(|url| ProjectId::from_origin(&url))
        .unwrap_or_else(|| ProjectId::from_dir(root))
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").output().ok()?;
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
    fn sanitize_matches_claude_code_folder_names() {
        assert_eq!(
            sanitize("/home/jefferson/.hangar/worktrees/hangar/dreaming"),
            "-home-jefferson--hangar-worktrees-hangar-dreaming"
        );
        assert_eq!(sanitize("/home/jefferson/Área de trabalho"), "-home-jefferson--rea-de-trabalho");
        assert_eq!(sanitize("/a/🚀"), "-a---");
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
