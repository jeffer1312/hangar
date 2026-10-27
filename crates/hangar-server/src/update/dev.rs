//! Modo desenvolvedor: o `current` aponta para um checkout git sem `manifest.json`. Atualizar é
//! `git pull --ff-only` na branch dele e reiniciar; árvore com edição, sem upstream ou à frente dele
//! recusa com o motivo, sem tocar em nada. Nada é recompilado.
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// O checkout como o `git` o vê agora.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checkout {
    pub branch: String,
    /// `origin/main`; `None` sem upstream configurado.
    pub upstream: Option<String>,
    /// Arquivos rastreados com edição (não rastreado não atrapalha o fast-forward).
    pub dirty: usize,
    /// Arquivos novos fora do `.gitignore` (`??`): a migração os trata como edição local.
    pub untracked: usize,
    pub ahead: u64,
    pub behind: u64,
    pub head: String,
    pub upstream_head: Option<String>,
    /// `(sha curto, título)` dos commits do upstream que ainda não estão aqui.
    pub changes: Vec<(String, String)>,
    /// O `git fetch` pedido falhou: o resto é a leitura do que já havia no disco.
    pub fetch_error: Option<String>,
}

/// Teto de um comando `git`: rede presa não segura a tela nem o executor.
const GIT_TIMEOUT: Duration = Duration::from_secs(120);

/// Sem pergunta nenhuma: nem usuário/senha nem a confirmação de chave do ssh.
fn git_command(root: &Path, args: &[&str]) -> Command {
    let mut command = Command::new("git");
    command.arg("-C").arg(root).args(args).env("LC_ALL", "C").env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes").stdin(Stdio::null());
    // O teste não usa a config nem os hooks de quem roda.
    #[cfg(test)]
    command.env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1");
    command
}

/// Roda com prazo: estourou, mata e devolve erro. `(sucesso, stdout, stderr)`.
fn run_timed(mut command: Command, timeout: Duration) -> Result<(bool, Vec<u8>, Vec<u8>), String> {
    use std::io::Read;
    let mut child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("não rodou: {e}"))?;
    let readers = [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)]
        .map(|pipe| std::thread::spawn(move || { let mut buf = Vec::new(); if let Some(mut pipe) = pipe { let _ = pipe.read_to_end(&mut buf); } buf }));
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("passou do prazo de {} s e foi encerrado", timeout.as_secs()));
            }
        }
    };
    let [out, err] = readers.map(|r| r.join().unwrap_or_default());
    Ok((status.success(), out, err))
}

fn git(root: &Path, args: &[&str]) -> Result<String, String> {
    let (ok, stdout, stderr) = run_timed(git_command(root, args), GIT_TIMEOUT).map_err(|e| format!("git {}: {e}", args.join(" ")))?;
    if ok { return Ok(String::from_utf8_lossy(&stdout).trim_end().to_owned()); }
    let text = String::from_utf8_lossy(&stderr);
    let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    Err(format!("git {} falhou: {}", args.join(" "), tail[tail.len().saturating_sub(4)..].join(" | ")))
}

/// Lê o checkout; `fetch` vai à rede antes (falha de rede não impede a leitura do que já há).
pub fn read(root: &Path, fetch: bool) -> Result<Checkout, String> {
    let fetch_error = if fetch { git(root, &["fetch", "--quiet"]).err() } else { None };
    if let Some(e) = &fetch_error { tracing::warn!(code = "update_dev_fetch_failed", "update: fetch do modo desenvolvedor falhou: {e}"); }
    let status = git(root, &["status", "--porcelain=v1"])?;
    let mut checkout = Checkout {
        fetch_error,
        dirty: status.lines().filter(|l| !l.starts_with("??")).count(),
        untracked: status.lines().filter(|l| l.starts_with("??")).count(),
        branch: git(root, &["rev-parse", "--abbrev-ref", "HEAD"])?,
        head: git(root, &["rev-parse", "--short", "HEAD"])?,
        upstream: git(root, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"]).ok(),
        ..Checkout::default()
    };
    if checkout.upstream.is_some() {
        let counts = git(root, &["rev-list", "--left-right", "--count", "@{u}...HEAD"])?;
        let mut parts = counts.split_whitespace().map(|n| n.parse::<u64>().unwrap_or(0));
        (checkout.behind, checkout.ahead) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
        checkout.upstream_head = git(root, &["rev-parse", "--short", "@{u}"]).ok();
        checkout.changes = git(root, &["log", "--format=%h%x00%s", "HEAD..@{u}"])?.lines()
            .filter_map(|l| l.split_once('\0')).map(|(sha, title)| (sha.to_owned(), title.to_owned())).collect();
    }
    Ok(checkout)
}

/// Por que este checkout não atualiza sozinho; `None` = pode.
pub fn refusal(checkout: &Checkout) -> Option<String> {
    let branch = &checkout.branch;
    if checkout.dirty > 0 {
        return Some(format!("o checkout do modo desenvolvedor tem {} arquivo(s) com edição; faça commit ou descarte antes de atualizar", checkout.dirty));
    }
    if branch == "HEAD" { return Some("o checkout do modo desenvolvedor está sem branch (HEAD solto)".into()); }
    let Some(upstream) = &checkout.upstream else {
        return Some(format!("a branch {branch} não tem upstream; o modo desenvolvedor atualiza só pelo git pull dela"));
    };
    (checkout.ahead > 0).then(|| format!("a branch {branch} tem {} commit(s) à frente de {upstream}; envie ou descarte antes de atualizar", checkout.ahead))
}

/// `git pull --ff-only` na branch do checkout; a saída vai ao log da tela. Sem hooks: o `post-merge`
/// do checkout roda o instalador inteiro (e reinicia) dentro do prazo do `git`, que só mata o `git`.
pub fn pull(root: &Path) -> Result<String, String> {
    let hooks = format!("core.hooksPath={}", if cfg!(windows) { "nul" } else { "/dev/null" });
    git(root, &["-c", &hooks, "pull", "--ff-only"])
}

/// O commit em que o checkout estava quando este servidor subiu (o `VERSAO_EM_EXECUCAO` do Python).
pub fn boot_head() -> Option<&'static str> { BOOT.get().and_then(Option::as_deref) }

static BOOT: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Anota o commit da subida; só o `main` chama, e só uma vez vale. Bloqueante.
pub fn note_boot(home: &Path) {
    let head = match super::http::mode(home) {
        super::http::Mode::Developer(root) => git(&root, &["rev-parse", "--short", "HEAD"]).ok(),
        _ => None,
    };
    let _ = BOOT.set(head);
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Repositório isolado: sem config global nem hooks.
    pub(crate) fn sh(dir: &Path, script: &str) {
        let ok = Command::new("sh").arg("-c").arg(script).current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "t").env("GIT_AUTHOR_EMAIL", "t@t").env("GIT_COMMITTER_NAME", "t").env("GIT_COMMITTER_EMAIL", "t@t")
            .status().unwrap().success();
        assert!(ok, "{script}");
    }

    /// `origin` com dois commits e um clone um commit atrás.
    pub(crate) fn behind_clone(dir: &Path) -> std::path::PathBuf {
        sh(dir, "git init -q -b main origin && cd origin && echo a > a && git add a && git commit -qm um \
                 && cd .. && git clone -q origin clone && cd origin && echo b > b && git add b && git commit -qm dois && cd ../clone && git fetch -q");
        dir.join("clone")
    }

    #[cfg(unix)]
    #[test]
    fn update_dev_reads_the_checkout_and_refuses_edits_and_local_commits() {
        let dir = tempfile::tempdir().unwrap();
        let clone = behind_clone(dir.path());
        let checkout = read(&clone, false).unwrap();
        assert_eq!((checkout.branch.as_str(), checkout.upstream.as_deref(), checkout.behind, checkout.ahead, checkout.dirty),
            ("main", Some("origin/main"), 1, 0, 0));
        assert_eq!(checkout.changes.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(), ["dois"]);
        assert_eq!(refusal(&checkout), None);

        std::fs::write(clone.join("a"), "editado").unwrap();
        let dirty = refusal(&read(&clone, false).unwrap()).unwrap();
        assert!(dirty.contains("1 arquivo(s) com edição"), "{dirty}");
        sh(&clone, "git commit -qam local");
        let ahead = refusal(&read(&clone, false).unwrap()).unwrap();
        assert!(ahead.contains("à frente de origin/main"), "{ahead}");
        sh(&clone, "git checkout -qb solta");
        assert!(refusal(&read(&clone, false).unwrap()).unwrap().contains("não tem upstream"));
    }

    #[test]
    fn update_dev_git_never_prompts() {
        let command = git_command(Path::new("."), &["fetch"]);
        let envs: std::collections::HashMap<_, _> = command.get_envs().filter_map(|(k, v)| Some((k.to_owned(), v?.to_owned()))).collect();
        assert_eq!(envs.get(std::ffi::OsStr::new("GIT_TERMINAL_PROMPT")).map(|v| v.to_str().unwrap()), Some("0"));
        assert_eq!(envs.get(std::ffi::OsStr::new("GIT_SSH_COMMAND")).map(|v| v.to_str().unwrap()), Some("ssh -o BatchMode=yes"));
    }

    #[cfg(unix)]
    #[test]
    fn update_dev_command_past_the_deadline_is_killed() {
        let started = std::time::Instant::now();
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 30"]);
        let error = run_timed(command, std::time::Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("prazo"), "{error}");
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
        let mut ok = Command::new("sh");
        ok.args(["-c", "echo oi"]);
        let out = run_timed(ok, std::time::Duration::from_secs(10)).unwrap();
        assert_eq!((out.0, String::from_utf8_lossy(&out.1).as_ref()), (true, "oi\n"));
    }

    #[cfg(unix)]
    #[test]
    fn update_dev_fetch_failure_is_kept_with_the_reading() {
        let dir = tempfile::tempdir().unwrap();
        let clone = behind_clone(dir.path());
        assert_eq!(read(&clone, true).unwrap().fetch_error, None);
        sh(&clone, "git remote set-url origin ../nao-existe");
        let checkout = read(&clone, true).unwrap();
        assert!(checkout.fetch_error.as_deref().is_some_and(|e| e.contains("fetch")), "{checkout:?}");
        assert_eq!(checkout.behind, 1, "o que já havia segue lido");
    }

    #[cfg(unix)]
    #[test]
    fn update_dev_pull_fast_forwards() {
        let dir = tempfile::tempdir().unwrap();
        let clone = behind_clone(dir.path());
        pull(&clone).unwrap();
        assert!(clone.join("b").exists());
        assert_eq!(read(&clone, false).unwrap().behind, 0);
    }

    /// O `post-merge` do checkout roda o instalador inteiro; o pull do modo desenvolvedor não o dispara.
    #[cfg(unix)]
    #[test]
    fn update_dev_pull_does_not_run_the_checkout_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let clone = behind_clone(dir.path());
        let marker = dir.path().join("hook-rodou");
        let hook = clone.join(".git/hooks/post-merge");
        std::fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        pull(&clone).unwrap();
        assert!(clone.join("b").exists());
        assert!(!marker.exists(), "o post-merge rodou dentro do pull");
    }
}
