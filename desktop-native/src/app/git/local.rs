//! `git` direto no disco, para a sessão desta máquina: as mesmas respostas das rotas `/git*` do backend e as mesmas
//! travas de `backend/app/git_ops.py` (argv sem shell, `--` antes de caminho, caminho só da lista real de alterados, nome
//! de branch e sha validados antes do git, `LC_ALL=C`, teto de tempo com o processo morto, sem `--force`, credencial de
//! URL apagada da saída do push). Roda fora da thread da janela (`spawn_blocking`).
use serde_json::{Value, json};
use std::{collections::{HashMap, HashSet}, io::Read, path::Path, process::{Command, Stdio}, time::{Duration, Instant}};

const TIMEOUT: Duration = Duration::from_secs(20);
/// Fetch, pull e push falam com o remoto: o `_FETCH_TIMEOUT` do backend.
const NET_TIMEOUT: Duration = Duration::from_secs(120);
/// Teto do diff, o `_DIFF_MAX` do backend.
const DIFF_MAX: usize = 200_000;
/// O `_LOG_FMT` do backend: campos por \x1f, commits por \x1e.
const LOG_FMT: &str = "%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%at%x1f%ar%x1f%s%x1f%b%x1e";

/// Recusa com o status HTTP que o backend daria.
pub(super) type Refusal = (u16, String);

struct Out { code: i32, stdout: String, stderr: String }

impl Out {
    fn both(&self) -> String { format!("{}{}", self.stdout, self.stderr).trim().to_owned() }
    fn fail(&self, fallback: &str) -> String { Some(self.stderr.trim()).filter(|s| !s.is_empty()).unwrap_or(fallback).to_owned() }
}

/// A raiz do repositório: o `status` dá caminhos a partir dela, e o descarte por caminho só acerta rodando dali.
pub(super) fn toplevel(cwd: &Path) -> Option<std::path::PathBuf> {
    let out = run(cwd, &["rev-parse", "--show-toplevel"]).ok().filter(|out| out.code == 0)?;
    Some(std::path::PathBuf::from(out.stdout.trim())).filter(|p| !p.as_os_str().is_empty())
}

fn run(cwd: &Path, args: &[&str]) -> Result<Out, Refusal> { run_for(cwd, args, TIMEOUT) }

fn run_for(cwd: &Path, args: &[&str], timeout: Duration) -> Result<Out, Refusal> {
    let mut command = Command::new("git");
    // App de janela no Windows: sem isto cada git abre um console piscando.
    #[cfg(windows)] { use std::os::windows::process::CommandExt; command.creation_flags(0x0800_0000); }
    let mut child = command.arg("-C").arg(cwd).args(args)
        // Saída lida por código: sem tradução. Sem trava opcional do índice. Sem pergunta de senha, que prenderia até o teto.
        .env("LC_ALL", "C").env("LANGUAGE", "C").env("GIT_OPTIONAL_LOCKS", "0").env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .map_err(|error| (500, if error.kind() == std::io::ErrorKind::NotFound { "git nao encontrado".into() } else { format!("git falhou: {error}") }))?;
    // Os dois canos são lidos ao mesmo tempo: saída grande encheria um deles e o git pararia esperando.
    let drain = |pipe: Option<Box<dyn Read + Send>>| std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe { let _ = pipe.read_to_end(&mut bytes); }
        String::from_utf8_lossy(&bytes).into_owned()
    });
    let stdout = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|error| (500, format!("git falhou: {error}")))? { break status; }
        if start.elapsed() > timeout {
            // ponytail: mata só o git; um ssh filho de push fica até fechar o cano. Matar o grupo quando isso aparecer.
            let _ = child.kill();
            let _ = child.wait();
            return Err((504, "git timeout".into()));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    Ok(Out { code: status.code().unwrap_or(-1), stdout: stdout.join().unwrap_or_default(), stderr: stderr.join().unwrap_or_default() })
}

/// Apaga `usuario:token@` de URLs, como o `_scrub` do backend.
fn scrub(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("://") {
        out.push_str(&rest[..at + 3]);
        rest = &rest[at + 3..];
        let end = rest.find(|c: char| c == '/' || c.is_whitespace()).unwrap_or(rest.len());
        if let Some(user) = rest[..end].rfind('@') { out.push_str("***"); rest = &rest[user..]; }
    }
    out.push_str(rest);
    out
}

fn cap(diff: String) -> (String, bool) {
    if diff.len() <= DIFF_MAX { return (diff, false); }
    let mut cut = DIFF_MAX;
    while !diff.is_char_boundary(cut) { cut -= 1; }
    (diff[..cut].to_owned(), true)
}

fn valid_sha(sha: &str) -> bool { (7..=40).contains(&sha.len()) && sha.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) }

/// Um arquivo alterado do `status --porcelain -z` e, num renomeado, o caminho antigo.
struct Changed { path: String, code: String, old: Option<String> }

fn changed(cwd: &Path) -> Result<Vec<Changed>, Refusal> {
    // `-z`: caminho cru, sem as aspas e escapes do porcelain; o renomeado traz o antigo no campo seguinte.
    let out = run(cwd, &["status", "--porcelain", "-z"])?;
    if out.code != 0 { return Err((409, out.fail("git status falhou"))); }
    let mut fields = out.stdout.split('\0').filter(|f| !f.is_empty());
    let mut files = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 { continue; }
        let (code, path) = (field[..2].to_owned(), field[3..].to_owned());
        let old = matches!(code.as_bytes()[0], b'R' | b'C').then(|| fields.next().map(str::to_owned)).flatten();
        files.push(Changed { path, code, old });
    }
    Ok(files)
}

fn in_list<'a>(files: &'a [Changed], path: &str) -> Result<&'a Changed, Refusal> {
    files.iter().find(|f| f.path == path).ok_or((400, "arquivo nao esta na lista de alterados".into()))
}

fn branches(cwd: &Path) -> Result<Value, Refusal> {
    let out = run(cwd, &["branch", "--sort=-committerdate", "--format=%(refname:short)"])?;
    if out.code != 0 { return Err((409, out.fail("git branch falhou"))); }
    let local: Vec<String> = out.stdout.lines().map(str::trim).filter(|b| !b.is_empty()).map(str::to_owned).collect();
    let mut seen: HashSet<String> = local.iter().cloned().collect();
    let mut remotes = Vec::new();
    let remote = run(cwd, &["branch", "-r", "--sort=-committerdate", "--format=%(refname:short)"])?;
    if remote.code == 0 {
        for full in remote.stdout.lines().map(str::trim) {
            let Some((_, short)) = full.split_once('/') else { continue };
            if short != "HEAD" && seen.insert(short.to_owned()) { remotes.push(short.to_owned()); }
        }
    }
    let head = run(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let status = run(cwd, &["status", "--porcelain"])?;
    Ok(json!({"current": (head.code == 0).then(|| head.stdout.trim().to_owned()), "branches": local, "remotes": remotes,
        "dirty": status.code == 0 && !status.stdout.trim().is_empty()}))
}

fn diff(cwd: &Path, path: &str) -> Result<Value, Refusal> {
    let files = changed(cwd)?;
    let file = in_list(&files, path)?;
    let out = if file.code == "??" { run(cwd, &["diff", "--no-index", "--", "/dev/null", path])? } else { run(cwd, &["diff", "HEAD", "--", path])? };
    // `git diff` sai 1 quando há diferença; só 128 em diante é erro.
    if out.code >= 128 { return Err((409, out.fail("git diff falhou"))); }
    let (text, truncated) = cap(out.stdout);
    Ok(json!({"path": path, "diff": text, "truncated": truncated}))
}

fn discard(cwd: &Path, path: &str) -> Result<Value, Refusal> {
    let files = changed(cwd)?;
    let file = in_list(&files, path)?;
    let out = if file.code == "??" { run(cwd, &["clean", "-f", "--", path])? }
        else { run(cwd, &["restore", "--staged", "--worktree", "--source=HEAD", "--", path])? };
    if out.code != 0 { return Err((409, out.fail("descartar falhou"))); }
    Ok(json!({"ok": true, "path": path}))
}

fn last_message(cwd: &Path) -> Result<Value, Refusal> {
    let out = run(cwd, &["log", "-1", "--pretty=%B"])?;
    if out.code != 0 { return Err((409, "sem commits pra amend".into())); }
    Ok(json!({"message": out.stdout.trim_end_matches('\n')}))
}

/// Só os caminhos marcados (`commit --only`), com o antigo do renomeado junto para o rename não virar "add".
fn commit(cwd: &Path, message: &str, paths: &[String], amend: bool) -> Result<Value, Refusal> {
    if message.trim().is_empty() { return Err((400, "mensagem vazia".into())); }
    if paths.is_empty() && !amend { return Err((400, "nenhum arquivo selecionado".into())); }
    let files = changed(cwd)?;
    for path in paths { if !files.iter().any(|f| &f.path == path) { return Err((400, format!("arquivo nao esta na lista de alterados: {path}"))); } }
    if amend { last_message(cwd)?; }
    let renames: HashMap<&str, &str> = files.iter().filter_map(|f| Some((f.path.as_str(), f.old.as_deref()?))).collect();
    let extra: Vec<&str> = paths.iter().filter_map(|p| renames.get(p.as_str()).copied()).collect();
    let out = if paths.is_empty() { run(cwd, &["commit", "--amend", "--only", "-m", message])? } else {
        // `add` antes: `--only` sozinho falha em arquivo novo.
        let mut add = vec!["add", "--"];
        add.extend(paths.iter().map(String::as_str));
        run(cwd, &add)?;
        let mut argv = vec!["commit"];
        if amend { argv.push("--amend"); }
        argv.extend(["--only", "-m", message, "--"]);
        argv.extend(paths.iter().map(String::as_str));
        argv.extend(extra);
        run(cwd, &argv)?
    };
    if out.code != 0 {
        let reason = Some(out.stderr.trim()).filter(|s| !s.is_empty()).or(Some(out.stdout.trim()).filter(|s| !s.is_empty())).unwrap_or("commit falhou");
        return Err((409, reason.to_owned()));
    }
    Ok(json!({"ok": true, "output": out.both()}))
}

/// Push da branch atual; sem upstream, `-u origin <branch>`. Nunca `--force`.
fn push(cwd: &Path) -> Result<Value, Refusal> {
    let branch = run(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?.stdout.trim().to_owned();
    if branch.is_empty() || branch == "HEAD" { return Err((409, "sem branch atual (detached HEAD)".into())); }
    let upstream = run(cwd, &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{upstream}"])?;
    let out = if upstream.code == 0 && !upstream.stdout.trim().is_empty() { run_for(cwd, &["push"], NET_TIMEOUT)? } else {
        if !run(cwd, &["remote"])?.stdout.split_whitespace().any(|r| r == "origin") {
            return Err((409, "branch sem upstream e sem remote 'origin' — configure um remote antes".into()));
        }
        run_for(cwd, &["push", "-u", "origin", &branch], NET_TIMEOUT)?
    };
    if out.code != 0 {
        let reason = Some(out.stderr.trim()).filter(|s| !s.is_empty()).or(Some(out.stdout.trim()).filter(|s| !s.is_empty())).unwrap_or("push falhou");
        return Err((409, scrub(reason)));
    }
    Ok(json!({"ok": true, "output": scrub(&out.both())}))
}

/// Fetch e pull do `_ACTIONS` do backend: recusa do git volta 200 com `ok: false`. Pull só avança (`--ff-only`).
fn remote_action(cwd: &Path, args: &[&str]) -> Result<Value, Refusal> {
    let out = run_for(cwd, args, NET_TIMEOUT)?;
    Ok(json!({"ok": out.code == 0, "output": scrub(&out.both())}))
}

fn log(cwd: &Path, n: usize) -> Result<Value, Refusal> {
    let n = n.clamp(1, 2000).to_string();
    let format = format!("--pretty=format:{LOG_FMT}");
    let out = run(cwd, &["log", "--topo-order", "-n", &n, &format])?;
    let mut commits = Vec::new();
    if out.code != 0 {
        // Repositório sem commit ainda: lista vazia, não erro.
        if !(out.stderr.contains("does not have any commits") || out.stderr.contains("bad default revision")) {
            return Err((409, out.fail("git log falhou")));
        }
    }
    // Só aqui, ainda não no upstream. Sem upstream fica vazio: não há com o que comparar.
    let unpushed: HashSet<String> = run(cwd, &["rev-list", "@{upstream}..HEAD"]).ok().filter(|o| o.code == 0)
        .map(|o| o.stdout.lines().map(|l| l.trim().to_owned()).filter(|l| !l.is_empty()).collect()).unwrap_or_default();
    for record in out.stdout.split('\x1e') {
        let record = record.trim_matches('\n');
        let f: Vec<&str> = record.splitn(9, '\x1f').collect();
        if f.len() < 9 { continue; }
        commits.push(json!({"hash": f[0], "short": f[1], "parents": f[2].split_whitespace().collect::<Vec<_>>(), "refs": f[3].trim(),
            "author": f[4], "ts": f[5].parse::<i64>().unwrap_or(0), "rel": f[6], "subject": f[7], "body": f[8].trim_matches('\n'),
            "local": unpushed.contains(f[0])}));
    }
    // À frente/atrás só com upstream de verdade, como o `git_summary` do backend.
    let status = run(cwd, &["status", "--porcelain=v1", "--branch"]).ok().filter(|o| o.code == 0);
    let header = status.as_ref().and_then(|o| o.stdout.lines().next().map(str::to_owned)).unwrap_or_default();
    let count = |word: &str| -> Option<i64> {
        if !header.contains("...") || header.contains("[gone]") { return None; }
        Some(header.split(word).nth(1).and_then(|r| r.trim_start().split(|c: char| !c.is_ascii_digit()).next()).and_then(|d| d.parse().ok()).unwrap_or(0))
    };
    Ok(json!({"commits": commits, "ahead": count("ahead "), "behind": count("behind ")}))
}

fn commit_diff(cwd: &Path, sha: &str) -> Result<Value, Refusal> {
    if !valid_sha(sha) { return Err((400, "sha invalido".into())); }
    let out = run(cwd, &["show", "--format=", "-m", "--first-parent", sha])?;
    if out.code >= 128 { return Err((409, out.fail("git show falhou"))); }
    let (diff, truncated) = cap(out.stdout);
    Ok(json!({"sha": sha, "diff": diff, "truncated": truncated}))
}

/// Só para uma branch que existe (local ou remota pelo nome curto).
fn checkout(cwd: &Path, branch: &str) -> Result<Value, Refusal> {
    let info = branches(cwd)?;
    let known = ["branches", "remotes"].iter().any(|key| info[key].as_array().is_some_and(|l| l.iter().any(|b| b.as_str() == Some(branch))));
    if !known { return Err((400, "branch inexistente".into())); }
    let out = run(cwd, &["switch", branch])?;
    if out.code != 0 { return Err((409, out.fail("switch falhou"))); }
    Ok(json!({"current": branch, "output": out.both()}))
}

/// `stash push --include-untracked`: recusa do git volta 200 com `ok: false`, como o `git_action` do backend.
fn stash(cwd: &Path) -> Result<Value, Refusal> {
    let out = run(cwd, &["stash", "push", "--include-untracked"])?;
    Ok(json!({"ok": out.code == 0, "output": out.both()}))
}

/// Nome de branch nova: a regra do backend antes do git, depois o `check-ref-format` e a lista das que existem.
fn create_branch(cwd: &Path, name: &str) -> Result<Value, Refusal> {
    let shape = name.len() <= 128 && name.chars().next().is_some_and(|c| c.is_ascii_alphanumeric())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || "._/-".contains(c)) && !name.contains("..");
    if !shape { return Err((400, "nome de branch invalido".into())); }
    if run(cwd, &["check-ref-format", &format!("refs/heads/{name}")])?.code != 0 { return Err((400, "nome de branch invalido".into())); }
    if run(cwd, &["branch", "--format=%(refname:short)"])?.stdout.lines().any(|b| b.trim() == name) {
        return Err((400, format!("branch ja existe: {name}")));
    }
    let out = run(cwd, &["switch", "-c", name])?;
    if out.code != 0 { return Err((409, out.fail("criar branch falhou"))); }
    Ok(json!({"ok": true, "output": out.both()}))
}

pub(super) fn call(cwd: &Path, op: &super::Op) -> Result<Value, Refusal> {
    use super::Op;
    match op {
        Op::Branches => branches(cwd),
        Op::Files => Ok(json!({"files": changed(cwd)?.into_iter().map(|f| json!({"path": f.path, "code": f.code})).collect::<Vec<_>>()})),
        Op::Diff(path) => diff(cwd, path),
        Op::Discard(path) => discard(cwd, path),
        Op::Commit { message, paths, amend } => commit(cwd, message, paths, *amend),
        Op::Push => push(cwd),
        Op::Fetch => remote_action(cwd, &["fetch", "--all", "--prune"]),
        Op::Pull => remote_action(cwd, &["pull", "--ff-only"]),
        Op::LastMessage => last_message(cwd),
        Op::Log(n) => log(cwd, *n),
        Op::CommitDiff(sha) => commit_diff(cwd, sha),
        Op::Checkout(branch) => checkout(cwd, branch),
        Op::Stash => stash(cwd),
        Op::CreateBranch(name) => create_branch(cwd, name),
    }
}

#[cfg(test)]
mod tests {
    use super::{cap, scrub, valid_sha};

    #[test]
    fn guards_match_the_backend() {
        assert_eq!(scrub("To https://joao:ghp_x@github.com/a/b.git"), "To https://***@github.com/a/b.git");
        assert_eq!(scrub("sem url"), "sem url");
        assert!(valid_sha("abc1234") && !valid_sha("-abc123") && !valid_sha("ABC1234") && !valid_sha("abc"));
        assert!(cap("é".repeat(150_000)).1);
    }
}
