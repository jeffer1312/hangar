//! Prova de integração por PR, complementar à ancestralidade local.
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Proof {
    pub merged: bool,
    pub problem: Option<&'static str>,
}

/// A fronteira externa devolve o JSON do provedor, sem transportar credenciais.
pub trait Hosting: Send + Sync {
    fn pull_requests(&self, repository: &str, branch: &str) -> Result<String, &'static str>;
}

pub struct GitHubCli {
    program: std::ffi::OsString,
    timeout: Duration,
}
impl GitHubCli {
    pub fn new(program: impl Into<std::ffi::OsString>) -> Self {
        Self {
            program: program.into(),
            timeout: Duration::from_secs(5),
        }
    }
}
impl Default for GitHubCli {
    fn default() -> Self {
        Self::new("gh")
    }
}
const MAX_RESPONSE: usize = 1024 * 1024;
impl Hosting for GitHubCli {
    fn pull_requests(&self, repository: &str, branch: &str) -> Result<String, &'static str> {
        use std::{
            io::Read,
            process::{Command, Stdio},
            sync::mpsc,
        };
        let guard =
            crate::process_lifetime::Guard::new().map_err(|_| "worktree_merge_host_unavailable")?;
        let mut command = Command::new(&self.program);
        command.args(["pr", "list", "--state=all", "--limit=100", &format!("--repo=github.com/{repository}"), &format!("--head={branch}"),
            "--json=number,state,headRefName,baseRefName,headRefOid,headRepository,headRepositoryOwner,isCrossRepository,url,createdAt,mergeCommit"])
            .env("GH_PROMPT_DISABLED", "1").env("GH_NO_UPDATE_NOTIFIER", "1").env("GH_PAGER", "")
            .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        guard.configure(&mut command);
        let mut child = command
            .spawn()
            .map_err(|_| "worktree_merge_host_unavailable")?;
        if guard.attach(&child).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("worktree_merge_host_unavailable");
        }
        let start = Instant::now();
        let (tx, rx) = mpsc::channel();
        let drain = |pipe: Box<dyn Read + Send>, stdout, tx: mpsc::Sender<_>| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                let result = pipe
                    .take((MAX_RESPONSE + 1) as u64)
                    .read_to_end(&mut bytes)
                    .map(|_| bytes);
                let _ = tx.send((stdout, result));
            });
        };
        drain(Box::new(child.stdout.take().unwrap()), true, tx.clone());
        drain(Box::new(child.stderr.take().unwrap()), false, tx);
        let result = (|| {
            let mut output = Vec::new();
            for _ in 0..2 {
                let (stdout, bytes) = rx
                    .recv_timeout(self.timeout.saturating_sub(start.elapsed()))
                    .map_err(|_| "worktree_merge_host_timeout")?;
                let bytes = bytes.map_err(|_| "worktree_merge_invalid_response")?;
                if bytes.len() > MAX_RESPONSE {
                    return Err("worktree_merge_response_too_large");
                }
                if stdout {
                    output = bytes;
                }
            }
            loop {
                if let Some(status) = child
                    .try_wait()
                    .map_err(|_| "worktree_merge_host_unavailable")?
                {
                    return if status.success() {
                        String::from_utf8(output).map_err(|_| "worktree_merge_invalid_response")
                    } else {
                        Err("worktree_merge_host_unavailable")
                    };
                }
                if start.elapsed() >= self.timeout {
                    return Err("worktree_merge_host_timeout");
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        })();
        if result.is_err() {
            guard.kill();
            let _ = child.kill();
            let _ = child.wait();
        } else {
            guard.disarm();
        }
        result
    }
}
#[derive(Clone, Debug, Hash, PartialEq, Eq)]
struct Identity {
    cwd: std::path::PathBuf,
    branch: String,
    head: String,
    base_branch: String,
    base_head: String,
    repository: String,
    head_repository: String,
}

fn read(cwd: &Path, args: &[&str]) -> Option<String> {
    let out = crate::git::command(cwd, args).ok()?;
    (out.code == 0).then(|| out.stdout.trim().to_owned())
}

fn github_repository(raw: &str) -> Option<String> {
    let raw = raw
        .trim()
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(raw.trim().trim_end_matches('/'));
    let path = if let Some(rest) = raw
        .strip_prefix("https://")
        .or_else(|| raw.strip_prefix("ssh://"))
    {
        let (host, path) = rest.split_once('/')?;
        let host = host.rsplit('@').next()?;
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("github.com:22") {
            return None;
        }
        path
    } else {
        raw.strip_prefix("git@github.com:")?
    };
    let parts = path.split('/').collect::<Vec<_>>();
    if parts.len() != 2
        || parts.iter().any(|part| {
            part.is_empty()
                || *part == "."
                || *part == ".."
                || !part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        })
    {
        return None;
    }
    Some(path.to_lowercase())
}

fn remote_repository(cwd: &Path, remote: &str, push: bool) -> Option<String> {
    let args = if push {
        vec!["remote", "get-url", "--push", "--all", "--", remote]
    } else {
        vec!["remote", "get-url", "--all", "--", remote]
    };
    let urls = read(cwd, &args)?;
    let repositories = urls
        .lines()
        .map(github_repository)
        .collect::<Option<std::collections::HashSet<_>>>()?;
    (repositories.len() == 1)
        .then(|| repositories.into_iter().next())
        .flatten()
}

impl Identity {
    fn read(cwd: &Path, branch: &str, base: &str) -> Option<Self> {
        if read(cwd, &["symbolic-ref", "--quiet", "HEAD"])? != format!("refs/heads/{branch}") {
            return None;
        }
        let reference = read(
            cwd,
            &[
                "rev-parse",
                "--symbolic-full-name",
                "--verify",
                "--end-of-options",
                base,
            ],
        )?;
        let mut remotes = read(cwd, &["remote"])?
            .lines()
            .map(str::to_owned)
            .collect::<Vec<_>>();
        remotes.sort_by_key(|remote| std::cmp::Reverse(remote.len()));
        let (remote, base_branch) = remotes.iter().find_map(|remote| {
            reference
                .strip_prefix(&format!("refs/remotes/{remote}/"))
                .map(|name| (remote, name))
        })?;
        let repository = remote_repository(cwd, remote, false)?;
        let head_remote = read(
            cwd,
            &["config", "--get", &format!("branch.{branch}.pushRemote")],
        )
        .or_else(|| read(cwd, &["config", "--get", "remote.pushDefault"]))
        .or_else(|| {
            read(
                cwd,
                &["config", "--get", &format!("branch.{branch}.remote")],
            )
        });
        let head_remote = head_remote.as_deref().unwrap_or(remote);
        let head_repository = remote_repository(cwd, head_remote, true)?;
        Some(Self {
            cwd: crate::real(cwd),
            branch: branch.to_owned(),
            head: read(cwd, &["rev-parse", "--verify", "HEAD^{commit}"])?,
            base_branch: base_branch.to_owned(),
            base_head: read(
                cwd,
                &[
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    &format!("{reference}^{{commit}}"),
                ],
            )?,
            repository,
            head_repository,
        })
    }
}

type CacheSlot = Arc<Mutex<Option<(Instant, Proof)>>>;
pub struct Verifier<H> {
    host: H,
    cache: Mutex<HashMap<Identity, CacheSlot>>,
}
impl<H: Hosting> Verifier<H> {
    pub fn new(host: H) -> Self {
        Self {
            host,
            cache: Mutex::new(HashMap::new()),
        }
    }
    pub fn verify(&self, cwd: &Path, branch: &str, base: &str) -> Proof {
        let Some(identity) = Identity::read(cwd, branch, base) else {
            return Proof::default();
        };
        let slot = {
            let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
            if cache.len() >= 512 && !cache.contains_key(&identity) {
                cache.clear();
            }
            cache
                .entry(identity.clone())
                .or_insert_with(|| Arc::new(Mutex::new(None)))
                .clone()
        };
        let mut cached = slot.lock().unwrap_or_else(|error| error.into_inner());
        if let Some((at, proof)) = cached.as_ref()
            && at.elapsed() < Duration::from_secs(60)
        {
            return proof.clone();
        }
        let proof = self.lookup(cwd, branch, base, &identity);
        *cached = Some((Instant::now(), proof.clone()));
        proof
    }
    fn lookup(&self, cwd: &Path, branch: &str, base: &str, identity: &Identity) -> Proof {
        let raw = match self.host.pull_requests(&identity.repository, branch) {
            Ok(raw) if raw.len() <= MAX_RESPONSE => raw,
            Ok(_) => {
                return Proof {
                    merged: false,
                    problem: Some("worktree_merge_response_too_large"),
                };
            }
            Err(problem) => {
                return Proof {
                    merged: false,
                    problem: Some(problem),
                };
            }
        };
        let Ok(rows) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
            return Proof {
                merged: false,
                problem: Some("worktree_merge_invalid_response"),
            };
        };
        let candidate = rows
            .iter()
            .filter(|pr| {
                let repository = pr["headRepository"]["nameWithOwner"].as_str().unwrap_or("");
                pr["headRefName"] == branch
                    && repository.eq_ignore_ascii_case(&identity.head_repository)
            })
            .max_by_key(|pr| {
                (
                    pr["createdAt"].as_str().unwrap_or(""),
                    pr["number"].as_u64().unwrap_or(0),
                )
            });
        let merged = candidate.is_some_and(|pr| {
            let owner = pr["headRepositoryOwner"]["login"].as_str().unwrap_or("");
            let number = pr["number"].as_u64().unwrap_or(0);
            let url = pr["url"].as_str().unwrap_or("").to_lowercase();
            pr["state"] == "MERGED"
                && pr["baseRefName"] == identity.base_branch
                && pr["headRefOid"] == identity.head
                && owner
                    .eq_ignore_ascii_case(identity.head_repository.split('/').next().unwrap_or(""))
                && number > 0
                && url == format!("https://github.com/{}/pull/{number}", identity.repository)
        });
        if merged {
            let commit = candidate.and_then(|pr| pr["mergeCommit"]["oid"].as_str());
            let included = commit
                .filter(|sha| sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
                .is_some_and(|sha| {
                    read(
                        cwd,
                        &["merge-base", "--is-ancestor", sha, &identity.base_head],
                    )
                    .is_some()
                });
            if !included {
                return Proof {
                    merged: false,
                    problem: Some("worktree_merge_base_unconfirmed"),
                };
            }
        }
        if merged && Identity::read(cwd, branch, base).as_ref() != Some(identity) {
            return Proof {
                merged: false,
                problem: Some("worktree_merge_identity_changed"),
            };
        }
        Proof {
            merged,
            problem: None,
        }
    }
}
static VERIFIER: LazyLock<Verifier<GitHubCli>> =
    LazyLock::new(|| Verifier::new(GitHubCli::default()));
pub fn probe(cwd: &Path, branch: &str, base: &str) -> Proof {
    VERIFIER.verify(cwd, branch, base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};
    use std::{
        path::PathBuf,
        process::Command,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    struct Repository {
        _dir: tempfile::TempDir,
        path: PathBuf,
        head: String,
    }
    impl Repository {
        fn git(&self, args: &[&str]) -> String {
            let out = Command::new("git")
                .args(["-C", self.path.to_str().unwrap()])
                .args(args)
                .env("GIT_CONFIG_GLOBAL", self.path.join("empty-config"))
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", self._dir.path())
                .env("GIT_AUTHOR_NAME", "Teste")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "Teste")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .env("GIT_AUTHOR_DATE", "2026-01-01T00:00:00Z")
                .env("GIT_COMMITTER_DATE", "2026-01-01T00:00:00Z")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {args:?}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8(out.stdout).unwrap().trim().to_owned()
        }
        fn squashed() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("repository");
            std::fs::create_dir(&path).unwrap();
            std::fs::write(path.join("empty-config"), "").unwrap();
            let mut repo = Self {
                _dir: dir,
                path,
                head: String::new(),
            };
            repo.git(&["init", "-b", "main"]);
            repo.git(&["config", "core.hooksPath", ""]);
            repo.git(&["config", "commit.gpgSign", "false"]);
            std::fs::write(repo.path.join("base.txt"), "base\n").unwrap();
            repo.git(&["add", "base.txt"]);
            repo.git(&["commit", "-m", "Base"]);
            repo.git(&[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/project.git",
            ]);
            repo.git(&["switch", "-c", "feature"]);
            std::fs::write(repo.path.join("feature.txt"), "feature\n").unwrap();
            repo.git(&["add", "feature.txt"]);
            repo.git(&["commit", "-m", "Mudança"]);
            repo.head = repo.git(&["rev-parse", "HEAD"]);
            repo.git(&["update-ref", "refs/remotes/origin/feature", &repo.head]);
            repo.git(&["config", "branch.feature.remote", "origin"]);
            repo.git(&["config", "branch.feature.merge", "refs/heads/feature"]);
            repo.git(&["switch", "main"]);
            repo.git(&["merge", "--squash", "feature"]);
            repo.git(&["commit", "-m", "Integração"]);
            repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
            repo.git(&["switch", "feature"]);
            repo
        }
        fn pull_request(&self) -> Value {
            json!({"number": 42,"state":"MERGED","headRefName":"feature","baseRefName":"main",
                "headRefOid":self.head,"headRepository":{"nameWithOwner":"acme/project"},
                "headRepositoryOwner":{"login":"acme"},"isCrossRepository":false,
                "url":"https://github.com/acme/project/pull/42","createdAt":"2026-01-01T00:00:00Z",
                "mergeCommit":{"oid":self.git(&["rev-parse", "refs/remotes/origin/main"])}})
        }
    }
    struct Host {
        response: Result<String, &'static str>,
        calls: Arc<AtomicUsize>,
    }
    impl Host {
        fn new(value: Value) -> Self {
            Self {
                response: Ok(value.to_string()),
                calls: Arc::default(),
            }
        }
    }
    impl Hosting for Host {
        fn pull_requests(&self, repository: &str, branch: &str) -> Result<String, &'static str> {
            assert_eq!(repository, "acme/project");
            assert_eq!(branch, "feature");
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.response.clone()
        }
    }
    #[test]
    fn merged_pull_request_proves_squashed_head_without_ancestry() {
        let repo = Repository::squashed();
        assert_eq!(
            repo.git(&[
                "rev-list",
                "--left-right",
                "--count",
                "origin/main...feature"
            ]),
            "1\t1"
        );
        let verifier = Verifier::new(Host::new(json!([repo.pull_request()])));
        assert!(verifier.verify(&repo.path, "feature", "origin/main").merged);
    }
    #[test]
    fn pull_request_identity_and_exact_head_are_required() {
        let repo = Repository::squashed();
        let valid = repo.pull_request();
        let cases = [
            ("headRepository", json!({"nameWithOwner":"other/project"})),
            ("headRepositoryOwner", json!({"login":"other"})),
            ("url", json!("https://github.com/other/project/pull/42")),
            ("headRefOid", json!("0".repeat(40))),
            ("baseRefName", json!("release")),
            ("headRefName", json!("another-branch")),
            ("state", json!("CLOSED")),
        ];
        for (field, value) in cases {
            let mut pr = valid.clone();
            pr[field] = value;
            let verifier = Verifier::new(Host::new(json!([pr])));
            assert!(
                !verifier.verify(&repo.path, "feature", "origin/main").merged,
                "Campo incompatível: {field}"
            );
        }
    }
    #[test]
    fn a_reused_branch_cannot_reuse_an_older_merged_pull_request() {
        let repo = Repository::squashed();
        let merged = repo.pull_request();
        let mut current = merged.clone();
        current["number"] = json!(43);
        current["state"] = json!("OPEN");
        current["createdAt"] = json!("2026-01-02T00:00:00Z");
        let verifier = Verifier::new(Host::new(json!([current, merged])));
        assert!(!verifier.verify(&repo.path, "feature", "origin/main").merged);
    }

    #[test]
    fn host_answers_and_failures_are_cached_but_a_later_commit_is_not_merged() {
        let repo = Repository::squashed();
        let host = Host::new(json!([repo.pull_request()]));
        let calls = host.calls.clone();
        let verifier = Verifier::new(host);
        assert!(verifier.verify(&repo.path, "feature", "origin/main").merged);
        assert!(verifier.verify(&repo.path, "feature", "origin/main").merged);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "Polling não pode consultar o provedor de novo"
        );
        std::fs::write(repo.path.join("later.txt"), "new work\n").unwrap();
        repo.git(&["add", "later.txt"]);
        repo.git(&["commit", "-m", "Depois do merge"]);
        assert!(!verifier.verify(&repo.path, "feature", "origin/main").merged);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let host = Host {
            response: Err("worktree_merge_host_unavailable"),
            calls: Arc::default(),
        };
        let calls = host.calls.clone();
        let verifier = Verifier::new(host);
        for _ in 0..2 {
            let proof = verifier.verify(&repo.path, "feature", "origin/main");
            assert!(!proof.merged);
            assert_eq!(proof.problem, Some("worktree_merge_host_unavailable"));
        }
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "Falha também precisa de intervalo de repetição"
        );
    }

    #[cfg(unix)]
    #[test]
    fn github_cli_reads_with_explicit_repository_and_bounded_process() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("fake-gh");
        let arguments = dir.path().join("arguments");
        crate::write_test_executable(
            &program,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' '[]'\n",
                arguments.display()
            ),
        );
        let cli = GitHubCli::new(program);
        assert_eq!(cli.pull_requests("acme/project", "feature").unwrap(), "[]");
        let arguments = std::fs::read_to_string(arguments).unwrap();
        assert!(
            arguments
                .lines()
                .any(|arg| arg == "--repo=github.com/acme/project")
        );
        assert!(arguments.lines().any(|arg| arg == "--head=feature"));
    }

    #[test]
    fn a_fork_push_remote_is_the_head_repository_identity() {
        let repo = Repository::squashed();
        repo.git(&[
            "remote",
            "add",
            "fork",
            "git@github.com:developer/project.git",
        ]);
        repo.git(&["config", "branch.feature.pushRemote", "fork"]);
        let mut pr = repo.pull_request();
        pr["headRepository"] = json!({"nameWithOwner":"developer/project"});
        pr["headRepositoryOwner"] = json!({"login":"developer"});
        pr["isCrossRepository"] = json!(true);
        let host = Host::new(json!([pr]));
        let calls = host.calls.clone();
        let verifier = Verifier::new(host);
        assert!(verifier.verify(&repo.path, "feature", "origin/main").merged);
        repo.git(&[
            "remote",
            "set-url",
            "fork",
            "https://github.com/another/project.git",
        ]);
        assert!(!verifier.verify(&repo.path, "feature", "origin/main").merged);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "Troca de identidade invalida o cache"
        );
    }

    #[test]
    fn merged_pull_request_proves_rebase_without_ancestry() {
        let repo = Repository::squashed();
        repo.git(&["switch", "-c", "integration-base", "feature~1"]);
        repo.git(&["commit", "--allow-empty", "-m", "Avanço da base"]);
        repo.git(&["branch", "integrated", "feature"]);
        repo.git(&[
            "rebase",
            "--onto",
            "integration-base",
            "feature~1",
            "integrated",
        ]);
        repo.git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        repo.git(&["switch", "feature"]);
        assert_eq!(
            repo.git(&[
                "rev-list",
                "--left-right",
                "--count",
                "origin/main...feature"
            ]),
            "2\t1"
        );
        let verifier = Verifier::new(Host::new(json!([repo.pull_request()])));
        assert!(verifier.verify(&repo.path, "feature", "origin/main").merged);
    }
    #[cfg(unix)]
    #[test]
    fn github_cli_rejects_oversized_output_and_ends_a_stuck_process() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("fake-gh");
        crate::write_test_executable(
            &program,
            "#!/bin/sh\ndd if=/dev/zero bs=1048577 count=1 2>/dev/null\n",
        );
        let mut cli = GitHubCli::new(&program);
        assert_eq!(
            cli.pull_requests("acme/project", "feature"),
            Err("worktree_merge_response_too_large")
        );
        std::fs::write(&program, "#!/bin/sh\nexec sleep 30\n").unwrap();
        cli.timeout = Duration::from_millis(200);
        assert_eq!(
            cli.pull_requests("acme/project", "feature"),
            Err("worktree_merge_host_timeout")
        );
    }
    #[test]
    fn changing_head_during_the_host_request_cancels_the_proof() {
        struct MovingHost<'a>(&'a Repository);
        impl Hosting for MovingHost<'_> {
            fn pull_requests(&self, _: &str, _: &str) -> Result<String, &'static str> {
                let response = json!([self.0.pull_request()]).to_string();
                self.0
                    .git(&["commit", "--allow-empty", "-m", "Trabalho concorrente"]);
                Ok(response)
            }
        }
        let repo = Repository::squashed();
        let proof = Verifier::new(MovingHost(&repo)).verify(&repo.path, "feature", "origin/main");
        assert!(!proof.merged);
        assert_eq!(proof.problem, Some("worktree_merge_identity_changed"));
    }
    #[test]
    fn historical_merge_needs_its_merge_commit_in_the_published_base() {
        let repo = Repository::squashed();
        let mut missing = repo.pull_request();
        missing["mergeCommit"] = Value::Null;
        let verifier = Verifier::new(Host::new(json!([missing])));
        assert!(
            !verifier.verify(&repo.path, "feature", "origin/main").merged,
            "PR sem commit de integração não prova inclusão na base"
        );
        let verifier = Verifier::new(Host::new(json!([repo.pull_request()])));
        assert!(verifier.verify(&repo.path, "feature", "origin/main").merged);
        repo.git(&["update-ref", "refs/remotes/origin/main", "feature~1"]);
        assert!(
            !verifier.verify(&repo.path, "feature", "origin/main").merged,
            "Base que deixou de incluir o merge não autoriza apagar o trabalho"
        );
    }
    #[test]
    fn a_fork_push_url_is_resolved_and_changing_it_invalidates_the_cache() {
        let repo = Repository::squashed();
        repo.git(&[
            "config",
            "remote.origin.pushurl",
            "git@github.com:developer/project.git",
        ]);
        let mut pr = repo.pull_request();
        pr["headRepository"] = json!({"nameWithOwner":"developer/project"});
        pr["headRepositoryOwner"] = json!({"login":"developer"});
        pr["isCrossRepository"] = json!(true);
        let host = Host::new(json!([pr]));
        let calls = host.calls.clone();
        let verifier = Verifier::new(host);
        assert!(
            verifier.verify(&repo.path, "feature", "origin/main").merged,
            "A origem do PR é a URL de push, mantendo a base na URL de fetch"
        );
        repo.git(&[
            "config",
            "remote.origin.pushurl",
            "https://github.com/another/project.git",
        ]);
        assert!(!verifier.verify(&repo.path, "feature", "origin/main").merged);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "Trocar somente a URL de push invalida o cache"
        );
    }
    #[test]
    fn distinct_push_repositories_are_ambiguous_even_if_one_matches_the_merged_pr() {
        let repo = Repository::squashed();
        repo.git(&[
            "config",
            "--add",
            "remote.origin.pushurl",
            "https://github.com/acme/project.git",
        ]);
        repo.git(&[
            "config",
            "--add",
            "remote.origin.pushurl",
            "https://github.com/another/project.git",
        ]);
        let verifier = Verifier::new(Host::new(json!([repo.pull_request()])));
        assert!(
            !verifier.verify(&repo.path, "feature", "origin/main").merged,
            "Vários destinos de push não permitem escolher uma identidade de origem"
        );
    }
    #[test]
    fn equivalent_push_urls_and_git_push_url_rewrites_preserve_repository_identity() {
        let repo = Repository::squashed();
        repo.git(&[
            "config",
            "--add",
            "remote.origin.pushurl",
            "https://github.com/acme/project.git",
        ]);
        repo.git(&[
            "config",
            "--add",
            "remote.origin.pushurl",
            "git@github.com:acme/project.git",
        ]);
        let verifier = Verifier::new(Host::new(json!([repo.pull_request()])));
        assert!(verifier.verify(&repo.path, "feature", "origin/main").merged);
        repo.git(&["config", "--unset-all", "remote.origin.pushurl"]);
        repo.git(&[
            "config",
            "url.git@github.com:developer/.pushInsteadOf",
            "https://github.com/acme/",
        ]);
        let mut pr = repo.pull_request();
        pr["headRepository"] = json!({"nameWithOwner":"developer/project"});
        pr["headRepositoryOwner"] = json!({"login":"developer"});
        pr["isCrossRepository"] = json!(true);
        let verifier = Verifier::new(Host::new(json!([pr])));
        assert!(verifier.verify(&repo.path, "feature", "origin/main").merged);
    }
}
