use super::Service;
use std::collections::BTreeMap;

const LINUX_PATHS: &str = "on:\n  push:\n    paths:\n      - \"crates/**\"\n      - \".github/workflows/server.yml\"\njobs: {}\n";
const WINDOWS_PATHS: &str = "on:\n  push:\n    paths:\n      - \"crates/**\"\n      - \".github/workflows/server-windows.yml\"\njobs: {}\n";

struct Repo {
    _home: tempfile::TempDir,
    root: tempfile::TempDir,
    service: Service,
}

impl Repo {
    fn new() -> Self {
        let home = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let environment = BTreeMap::from([
            ("PATH".to_owned(), std::env::var("PATH").unwrap_or_default()),
            ("HOME".to_owned(), home.path().display().to_string()),
            ("GIT_CONFIG_GLOBAL".to_owned(), "/dev/null".to_owned()),
            ("GIT_CONFIG_NOSYSTEM".to_owned(), "1".to_owned()),
        ]);
        let service = Service::new(home.path().into(), root.path().into(), home.path().join(".claude"), environment);
        let repo = Self { _home: home, root, service };
        repo.git(&["init", "-q", "-b", "main"]);
        repo
    }
    fn git(&self, args: &[&str]) -> String {
        let output = self.service.git(args, 30).unwrap();
        assert_eq!(output.code, 0, "git {args:?}: {}", output.stdout);
        output.stdout.trim().to_owned()
    }
    fn commit(&self, files: &[(&str, &str)]) -> String {
        for (path, text) in files {
            let path = self.root.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        self.git(&["add", "-A"]);
        self.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "c"]);
        self.git(&["rev-parse", "HEAD"])
    }
}

#[test]
fn windows_follows_its_own_workflow_paths_and_linux_the_server_yml() {
    let repo = Repo::new();
    let published = repo.commit(&[
        ("crates/lib.rs", "1"),
        (".github/workflows/server.yml", LINUX_PATHS),
        (".github/workflows/server-windows.yml", WINDOWS_PATHS),
    ]);
    repo.commit(&[(".github/workflows/server.yml", &format!("{LINUX_PATHS}# linux only\n"))]);
    let top = repo.commit(&[("backend/app.py", "x")]);

    assert_eq!(repo.service.last_same_build(&published, &top, "windows-x86_64"), top);
    assert_eq!(repo.service.last_same_build(&published, &top, "linux-x86_64"), published);
}

#[test]
fn windows_reads_the_server_yml_where_its_own_workflow_does_not_exist_yet() {
    let repo = Repo::new();
    let published = repo.commit(&[("crates/lib.rs", "1"), (".github/workflows/server.yml", LINUX_PATHS)]);
    repo.commit(&[(".github/workflows/server.yml", &format!("{LINUX_PATHS}# both systems\n"))]);
    let top = repo.commit(&[("backend/app.py", "x")]);

    assert_eq!(repo.service.last_same_build(&published, &top, "windows-x86_64"), published);
}

fn push_paths(workflow: &str) -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows").join(workflow);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    let mut inside = false;
    let mut paths = vec![];
    for line in text.lines().map(str::trim) {
        if line == "paths:" {
            inside = true;
        } else if inside {
            match line.strip_prefix("- ") {
                Some(path) if !path.contains(workflow) => paths.push(path.trim_matches('"').to_owned()),
                Some(_) => {}
                None if line.is_empty() || line.starts_with('#') => {}
                None => break,
            }
        }
    }
    paths
}

#[test]
fn the_linux_and_windows_server_workflows_build_on_the_same_paths() {
    let linux = push_paths("server.yml");
    assert!(!linux.is_empty());
    assert_eq!(push_paths("server-windows.yml"), linux);
}
