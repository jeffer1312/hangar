use super::*;
use crate::real;

fn git(cwd: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git").arg("-C").arg(cwd).args(args)
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }).env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
        .output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

/// O gabarito é o próprio git, normalizado como o `describe` dos atalhos faz.
fn oracle(cwd: &Path) -> Layout {
    let out = git(cwd, &["rev-parse", "--path-format=absolute", "--git-common-dir", "--show-toplevel"]);
    let lines: Vec<&str> = out.trim().lines().collect();
    Layout::Repo { common: real(Path::new(lines[0])), toplevel: real(Path::new(lines[1])) }
}

fn normalized(found: Layout) -> Layout {
    match found {
        Layout::Repo { common, toplevel } => Layout::Repo { common: real(&common), toplevel: real(&toplevel) },
        other => other,
    }
}

#[test]
fn matches_git_in_a_repository_its_subfolder_and_a_linked_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let root = real(dir.path());
    let repo = root.join("repo");
    std::fs::create_dir_all(repo.join("sub").join("deep")).unwrap();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "um"]);
    git(&repo, &["worktree", "add", "-q", "-b", "feature", root.join("wt").to_str().unwrap()]);
    std::fs::create_dir_all(root.join("wt").join("inside")).unwrap();
    for cwd in [repo.clone(), repo.join("sub").join("deep"), root.join("wt"), root.join("wt").join("inside")] {
        assert_eq!(normalized(layout(&cwd)), oracle(&cwd), "{}", cwd.display());
    }
}

#[test]
fn a_folder_outside_any_repository_is_not_a_repository() {
    let dir = tempfile::tempdir().unwrap();
    let plain = real(dir.path()).join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(layout(&plain), Layout::NotRepo);
}

#[test]
fn an_unreadable_git_file_is_left_to_git() {
    let dir = tempfile::tempdir().unwrap();
    let odd = real(dir.path()).join("odd");
    std::fs::create_dir_all(&odd).unwrap();
    std::fs::write(odd.join(".git"), "isto não é um gitdir").unwrap();
    assert_eq!(layout(&odd), Layout::Unknown);
}
