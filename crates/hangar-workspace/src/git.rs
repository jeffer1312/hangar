//! Operações existentes de Git; caminhos e referências não se tornam opções do comando.
use crate::{
    Result, error,
    process::{Output, run},
    real,
};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
const TIMEOUT: Duration = Duration::from_secs(20);
const NETWORK: Duration = Duration::from_secs(120);
pub const DIFF_MAX: usize = 200_000;
const MAINLINES: &[&str] = &[
    "origin/HEAD",
    "origin/develop",
    "origin/main",
    "origin/master",
];

pub fn command(cwd: &Path, args: &[&str]) -> Result<Output> {
    run(cwd, args, TIMEOUT)
}
pub fn checked(cwd: &Path, args: &[&str], status: u16) -> Result<Output> {
    let out = command(cwd, args)?;
    if out.code != 0 {
        return Err(error(status, scrub(&out.reason("git falhou"))));
    }
    Ok(out)
}
pub fn network_checked(cwd: &Path, args: &[&str]) -> Result<Output> {
    let out = run(cwd, args, NETWORK)?;
    if out.code != 0 {
        return Err(error(409, scrub(&out.reason("git falhou"))));
    }
    Ok(out)
}
pub fn scrub(text: &str) -> String {
    static RE: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(://)[^/?#\s]*@").unwrap());
    let text = RE.replace_all(text, "${1}***@").into_owned();
    // Credencial malformada também pode conter barra; a mensagem nunca devolve o trecho inteiro.
    static BROKEN: LazyLock<regex::Regex> =
        LazyLock::new(|| regex::Regex::new(r"(://)([^?#\s]*:[^?#\s]*)@").unwrap());
    BROKEN
        .replace_all(&text, |c: &regex::Captures<'_>| {
            if c[2].starts_with("***@") {
                return c[0].to_owned();
            }
            let authority = c[2].split('/').next().unwrap_or("");
            let port = authority.rsplit(':').next().unwrap_or("");
            if !port.is_empty() && port.chars().all(|x| x.is_ascii_digit()) {
                c[0].to_owned()
            } else {
                format!("{}***@", &c[1])
            }
        })
        .into_owned()
}
pub fn cap(diff: &str) -> (String, bool) {
    (
        diff.chars().take(DIFF_MAX).collect(),
        diff.chars().count() > DIFF_MAX,
    )
}
fn sha(value: &str) -> Result<()> {
    if !(7..=40).contains(&value.len())
        || !value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        Err(error(400, "sha inválido"))
    } else {
        Ok(())
    }
}
fn null_file() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

pub fn head_info(cwd: Option<&str>) -> (Option<String>, bool) {
    let Some(cwd) = cwd else { return (None, false) };
    let mut dot = PathBuf::from(cwd).join(".git");
    let worktree = dot.is_file();
    if worktree {
        let Ok(text) = std::fs::read_to_string(&dot) else {
            return (None, true);
        };
        let Some(target) = text.trim().strip_prefix("gitdir: ") else {
            return (None, true);
        };
        dot = Path::new(cwd).join(target);
    }
    let branch = std::fs::read_to_string(dot.join("HEAD"))
        .ok()
        .and_then(|s| {
            s.trim()
                .strip_prefix("ref: refs/heads/")
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        });
    (branch, worktree)
}
pub fn parse_summary(out: &str) -> Value {
    let mut lines = out.lines();
    let Some(header) = lines.next().filter(|s| s.starts_with("## ")) else {
        return Value::Null;
    };
    let count = |word: &str| -> Value {
        if !header.contains("...") || header.contains("[gone]") {
            Value::Null
        } else {
            json!(
                header
                    .split(word)
                    .nth(1)
                    .and_then(|s| s.trim_start().split(|c: char| !c.is_ascii_digit()).next())
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0)
            )
        }
    };
    json!({"dirty":lines.count(),"ahead":count("ahead "),"behind":count("behind ")})
}
struct Cache {
    at: Instant,
    value: Value,
    ttl: Duration,
    fingerprint: Option<Vec<u128>>,
}
type CacheSlot = Arc<Mutex<Option<Cache>>>;
static CACHE: LazyLock<Mutex<HashMap<(PathBuf, bool), CacheSlot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
fn fingerprint(cwd: &Path) -> Option<Vec<u128>> {
    let mut dot = cwd.join(".git");
    if dot.is_file() {
        dot = cwd.join(
            std::fs::read_to_string(&dot)
                .ok()?
                .trim()
                .strip_prefix("gitdir: ")?,
        );
    }
    let common = std::fs::read_to_string(dot.join("commondir"))
        .map(|s| dot.join(s.trim()))
        .unwrap_or_else(|_| dot.clone());
    let head = std::fs::read_to_string(dot.join("HEAD")).ok()?;
    let mut paths = vec![
        dot.join("index"),
        dot.join("HEAD"),
        common.join("packed-refs"),
        common.join("FETCH_HEAD"),
    ];
    if let Some(target) = head.trim().strip_prefix("ref: ") {
        paths.push(common.join(target));
        // O push só regrava a ref remota da branch: sem ela o "à frente" ficava velho até o prazo.
        if let Some(branch) = target.strip_prefix("refs/heads/")
            && let Ok(remotes) = std::fs::read_dir(common.join("refs").join("remotes"))
        {
            let mut found: Vec<PathBuf> = remotes.flatten().map(|r| r.path()).filter(|p| p.is_dir())
                .map(|p| p.join(branch)).collect();
            found.sort();
            paths.extend(found);
        }
    }
    paths
        .iter()
        .map(|p| match std::fs::metadata(p) {
            Ok(m) => Some(
                m.modified()
                    .ok()?
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_nanos(),
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(0),
            Err(_) => None,
        })
        .collect()
}
pub fn summary(cwd: Option<&str>, diff: bool) -> Value {
    let Some(cwd) = cwd.filter(|c| Path::new(c).join(".git").exists()) else {
        return Value::Null;
    };
    let cwd = real(Path::new(cwd));
    // O mesmo cwd não dispara duas capturas simultâneas, mesmo com vários aparelhos.
    let key = (cwd.clone(), diff);
    let slot = {
        let mut entries = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() > 1024 {
            entries.retain(|_, slot| {
                slot.try_lock().map_or(true, |c| {
                    c.as_ref()
                        .is_some_and(|c| c.at.elapsed() < Duration::from_secs(30))
                })
            });
        }
        entries
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(None)))
            .clone()
    };
    // O Git lento de uma pasta não segura a consulta das outras.
    let mut cache = slot.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = cache.as_ref()
        && (c.at.elapsed() < c.ttl
            || (!c.value.is_null()
                && c.at.elapsed() < Duration::from_secs(10)
                && c.fingerprint.is_some()
                && c.fingerprint == fingerprint(&cwd)))
    {
        return c.value.clone();
    }
    let result = run(
        &cwd,
        if diff {
            &["diff", "--numstat", "HEAD"]
        } else {
            &["status", "--porcelain=v1", "--branch"]
        },
        // Com a máquina ocupada um `git status` passa de 2 s sem nada errado.
        Duration::from_secs(8),
    );
    let mut ttl = Duration::from_secs(3);
    let value = match result {
        Ok(out) if out.code == 0 => {
            if diff {
                let nums = numstat(&out.stdout);
                json!({"added":nums.values().map(|v|v.0).sum::<u64>(),"removed":nums.values().map(|v|v.1).sum::<u64>()})
            } else {
                parse_summary(&out.stdout)
            }
        }
        // Repositório sem commit: `HEAD` não resolve e não há diferença a contar. Objeto vazio, não nulo:
        // nulo é falha da consulta e iria ao diário a cada rodada. O primeiro commit liga o número.
        Ok(out) if diff && (out.stderr.contains("ambiguous argument") || out.stderr.contains("unknown revision")) => json!({}),
        Ok(_) => Value::Null,
        Err(_) => {
            ttl = Duration::from_secs(10);
            tracing::warn!(code = "git_summary_failed", "consulta de Git sem resposta");
            // O painel fica com o último valor bom em vez de ficar vazio.
            cache.as_ref().map_or(Value::Null, |c| c.value.clone())
        }
    };
    *cache = Some(Cache {
        at: Instant::now(),
        value: value.clone(),
        ttl,
        fingerprint: fingerprint(&cwd),
    });
    value
}
pub fn invalidate(cwd: &Path) {
    CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .retain(|(p, _), _| p != &real(cwd));
}

pub fn branches(cwd: &Path) -> Result<Value> {
    let local = checked(
        cwd,
        &[
            "branch",
            "--sort=-committerdate",
            "--format=%(refname:short)",
        ],
        409,
    )?
    .stdout
    .lines()
    .map(|s| s.trim().to_owned())
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>();
    let out = command(
        cwd,
        &[
            "branch",
            "-r",
            "--sort=-committerdate",
            "--format=%(refname:short)",
        ],
    )?;
    let mut seen = HashSet::new();
    let remote = out
        .stdout
        .lines()
        .filter_map(|s| s.trim().split_once('/').map(|(_, b)| b.to_owned()))
        .filter(|b| b != "HEAD" && !local.contains(b) && seen.insert(b.clone()))
        .collect::<Vec<_>>();
    let cur = command(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let st = command(cwd, &["status", "--porcelain"])?;
    Ok(
        json!({"current":if cur.code==0 {Some(cur.stdout.trim())} else {None},"branches":local,"remotes":remote,"dirty":st.code==0&&!st.stdout.trim().is_empty()}),
    )
}
/// `switch` não aceita `--` antes da branch, e um remoto pode anunciar `origin/--detach`.
fn option_like(branch: &str) -> Result<()> {
    if branch.starts_with('-') {
        return Err(error(400, "branch inexistente"));
    }
    Ok(())
}
pub fn switch(cwd: &Path, branch: &str) -> Result<Value> {
    option_like(branch)?;
    let info = branches(cwd)?;
    if !info["branches"]
        .as_array()
        .unwrap()
        .iter()
        .chain(info["remotes"].as_array().unwrap())
        .any(|b| b == branch)
    {
        return Err(error(400, "branch inexistente"));
    }
    let out = checked(cwd, &["switch", branch], 409)?;
    invalidate(cwd);
    Ok(json!({"current":branch,"output":out.both()}))
}
fn remote_ref(cwd: &Path, branch: &str) -> Result<String> {
    let out = checked(
        cwd,
        &["for-each-ref", "--format=%(refname:short)", "refs/remotes"],
        409,
    )?;
    let refs = out
        .stdout
        .lines()
        .filter(|s| s.split_once('/').is_some_and(|(_, b)| b == branch))
        .collect::<Vec<_>>();
    if refs.len() != 1 {
        return Err(error(409, "branch remota ambígua"));
    }
    Ok(refs[0].into())
}
fn branch_name_ok(cwd: &Path, name: &str) -> Result<bool> {
    // Nome começando com "-" viraria flag do `worktree add -b`.
    if name.is_empty() || name.starts_with('-') || name != name.trim() {
        return Ok(false);
    }
    Ok(command(cwd, &["check-ref-format", "--branch", name])?.code == 0)
}
pub fn create_worktree(
    cwd: &Path,
    branch: &str,
    name: &str,
    root: &Path,
    new_branch: bool,
    base: Option<&str>,
) -> Result<Value> {
    if !new_branch {
        option_like(branch)?;
    }
    static LOCK: Mutex<()> = Mutex::new(());
    let _lock = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let top = command(cwd, &["rev-parse", "--show-toplevel"])?;
    if top.code != 0 {
        return Err(error(409, "pasta sem repositório Git"));
    }
    let repo = real(Path::new(top.stdout.trim()));
    let root = real(root);
    if !repo.starts_with(&root) {
        return Err(error(400, "repositório fora da raiz autorizada"));
    }
    let info = branches(cwd)?;
    let local = info["branches"].as_array().unwrap();
    let known = |b: &str| {
        local
            .iter()
            .chain(info["remotes"].as_array().unwrap())
            .any(|x| x == b)
    };
    let mut start = String::new();
    if new_branch {
        if !branch_name_ok(cwd, branch)? {
            return Err(error(400, "nome de branch inválido"));
        }
        if known(branch) {
            return Err(error(409, "já existe uma branch com esse nome"));
        }
        start = base
            .filter(|b| !b.is_empty())
            .or(info["current"].as_str())
            .unwrap_or_default()
            .to_owned();
        if start.is_empty() || !known(&start) {
            return Err(error(400, "branch base inexistente"));
        }
    } else {
        if !known(branch) {
            return Err(error(400, "branch inexistente"));
        }
        if info["current"] == branch {
            return Ok(json!([cwd.to_string_lossy(), false]));
        }
    }
    if name.contains(['/', '\\', '\0']) || name == ".." {
        return Err(error(400, "nome inválido"));
    }
    let target = repo.parent().unwrap_or(&repo).join(format!(
        "{}-{name}",
        repo.file_name().unwrap_or_default().to_string_lossy()
    ));
    if !target.starts_with(&root) {
        return Err(error(400, "worktree fora da raiz autorizada"));
    }
    if target.symlink_metadata().is_ok() {
        return Err(error(409, "destino da worktree já existe"));
    }
    let text = target.to_string_lossy();
    let created = if new_branch {
        let from = if local.iter().any(|b| b == start.as_str()) {
            start.clone()
        } else {
            remote_ref(cwd, &start)?
        };
        // Sem --no-track a branch nova herdaria o upstream da base: o pull puxaria a base.
        command(
            cwd,
            &["worktree", "add", "--no-track", "-b", branch, &text, &from],
        )?
    } else if local.iter().any(|b| b == branch) {
        command(cwd, &["worktree", "add", &text, branch])?
    } else {
        let remote = remote_ref(cwd, branch)?;
        command(
            cwd,
            &["worktree", "add", "--track", "-b", branch, &text, &remote],
        )?
    };
    if created.code != 0 {
        return Err(error(
            409,
            scrub(&created.reason("não consegui criar a worktree")),
        ));
    }
    if new_branch {
        // A worktree já existe: falha em registrar a base só fica no log.
        let key = format!("branch.{branch}.hangar-base");
        if !command(&target, &["config", &key, &start]).is_ok_and(|o| o.code == 0) {
            tracing::warn!("hangar-base da branch nova não gravada");
        }
    }
    copy_ignored(&main_root(cwd, &repo, &root), &target);
    Ok(json!([text, true]))
}
/// Raiz do repositório principal: com `cwd` numa worktree ligada, a config mora no principal.
fn main_root(cwd: &Path, repo: &Path, root: &Path) -> PathBuf {
    match command(
        cwd,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    ) {
        Ok(out) if out.code == 0 => {
            let common = real(Path::new(out.stdout.trim()));
            match common.parent() {
                // Fora da raiz autorizada não é lido; fica a raiz de onde se partiu.
                Some(main) if main.starts_with(root) => main.to_path_buf(),
                _ => repo.to_path_buf(),
            }
        }
        _ => repo.to_path_buf(),
    }
}
const COPY_MAX: u64 = 1024 * 1024;
/// A worktree nasce sem os arquivos ignorados (`.env`, `pserver.ini`) e o projeto não sobe;
/// copia os da raiz. Falha aqui só registra: a worktree já existe e serve.
pub fn copy_ignored(repo: &Path, target: &Path) -> Vec<String> {
    let args = [
        "ls-files",
        "--others",
        "--ignored",
        "--exclude-standard",
        "--directory",
        "-z",
    ];
    let out = match command(repo, &args) {
        Ok(out) if out.code == 0 => out,
        _ => {
            tracing::warn!("copy_ignored: ls-files falhou");
            return Vec::new();
        }
    };
    let mut copied = Vec::new();
    // Pasta ignorada vem com "/" no fim; subpastas ficam de fora.
    for rel in out
        .stdout
        .split('\0')
        .filter(|r| !r.is_empty() && !r.contains('/'))
    {
        let (src, dst) = (repo.join(rel), target.join(rel));
        let Ok(meta) = src.symlink_metadata() else {
            continue;
        };
        if !meta.is_file() || meta.len() > COPY_MAX {
            continue;
        }
        // Nunca sobrescreve o que a branch versiona nem segue link para fora da worktree.
        if dst.symlink_metadata().is_ok() {
            continue;
        }
        match std::fs::copy(&src, &dst) {
            Ok(_) => copied.push(rel.to_owned()),
            Err(e) => tracing::warn!(error = %e.kind(), "copy_ignored: arquivo não copiado"),
        }
    }
    copied
}
pub fn remove_worktree(cwd: &Path, path: &str, force: bool) -> Result<Value> {
    let mut args = vec!["worktree", "remove"];
    if force {
        args.push("--force");
    }
    args.push(path);
    let out = command(cwd, &args)?;
    if out.code != 0 {
        return Err(error(
            500,
            scrub(&out.reason("não consegui remover a worktree")),
        ));
    }
    Ok(Value::Null)
}
pub fn action(cwd: &Path, action: &str) -> Result<Value> {
    let args: &[&str] = match action {
        "status" => &["status", "--short", "--branch"],
        "pull" => &["pull", "--ff-only"],
        "fetch" => &["fetch", "--all", "--prune"],
        "stash" => &["stash", "push", "--include-untracked"],
        "stash-pop" => &["stash", "pop"],
        "log" => &["log", "-n", "30", "--pretty=format:%h  %s  (%an, %ar)"],
        "revert-abort" => &["revert", "--abort"],
        "cherry-pick-abort" => &["cherry-pick", "--abort"],
        _ => return Err(error(400, "ação inválida")),
    };
    let out = run(
        cwd,
        args,
        if ["pull", "fetch"].contains(&action) {
            NETWORK
        } else {
            TIMEOUT
        },
    )?;
    invalidate(cwd);
    Ok(json!({"ok":out.code==0,"output":scrub(&out.both())}))
}
pub fn log(cwd: &Path, n: usize, grep: Option<&str>) -> Result<Value> {
    let count = n.clamp(1, 2000).to_string();
    let format = "--pretty=format:%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%at%x1f%ar%x1f%s%x1f%b%x1e";
    let pattern = grep.map(|s| format!("--grep={s}"));
    let mut args = vec!["log", "--topo-order", "-n", &count, format];
    if let Some(p) = &pattern {
        args.extend([p, "-F", "-i"]);
    }
    let out = command(cwd, &args)?;
    if out.code != 0 {
        if out.stderr.contains("does not have any commits")
            || out.stderr.contains("bad default revision")
        {
            return Ok(json!([]));
        }
        return Err(error(409, out.reason("git log falhou")));
    }
    let local = command(cwd, &["rev-list", "@{upstream}..HEAD"])
        .ok()
        .filter(|o| o.code == 0)
        .map(|o| o.stdout.lines().map(str::to_owned).collect::<HashSet<_>>())
        .unwrap_or_default();
    Ok(json!(out.stdout.split('\x1e').filter_map(|r|{let f=r.trim_matches('\n').splitn(9,'\x1f').collect::<Vec<_>>();(f.len()==9).then(||json!({"hash":f[0],"short":f[1],"parents":f[2].split_whitespace().collect::<Vec<_>>(),"refs":f[3].trim(),"author":f[4],"ts":f[5].parse::<i64>().unwrap_or(0),"rel":f[6],"subject":f[7],"body":f[8].trim_matches('\n'),"local":local.contains(f[0])}))}).collect::<Vec<_>>()))
}
pub fn log_since(cwd: &Path, desde: f64, n: usize) -> Value {
    let out = command(
        cwd,
        &[
            "log",
            &format!("--since=@{}", desde as i64),
            "-n",
            &n.to_string(),
            "--reverse",
            "--pretty=format:%h\x1f%s",
        ],
    );
    json!(
        out.ok()
            .filter(|o| o.code == 0)
            .map(|o| o
                .stdout
                .lines()
                .filter_map(|s| s
                    .split_once('\x1f')
                    .map(|(short, subject)| json!({"short":short,"subject":subject.trim()})))
                .collect::<Vec<_>>())
            .unwrap_or_default()
    )
}
pub fn lanes(mut commits: Vec<Value>) -> Value {
    fn free(lanes: &mut Vec<Option<String>>) -> usize {
        if let Some(i) = lanes.iter().position(Option::is_none) {
            i
        } else {
            lanes.push(None);
            lanes.len() - 1
        }
    }
    let mut lanes = Vec::<Option<String>>::new();
    for c in &mut commits {
        let hash = c["hash"].as_str().unwrap_or("").to_owned();
        let waiting = lanes
            .iter()
            .enumerate()
            .filter_map(|(i, h)| (h.as_deref() == Some(&hash)).then_some(i))
            .collect::<Vec<_>>();
        let col = if let Some(i) = waiting.first() {
            for extra in &waiting[1..] {
                lanes[*extra] = None;
            }
            *i
        } else {
            free(&mut lanes)
        };
        let parents = c["parents"].as_array().cloned().unwrap_or_default();
        let mut edges = Vec::new();
        if parents.is_empty() {
            lanes[col] = None;
        } else {
            for (idx, p) in parents.iter().enumerate() {
                let parent = p.as_str().unwrap_or("").to_owned();
                if let Some(i) = lanes.iter().position(|x| x.as_deref() == Some(&parent)) {
                    edges.push(json!({"to_col":i,"curved":i!=col}));
                    if idx == 0 && i != col {
                        lanes[col] = None;
                    }
                } else if idx == 0 {
                    lanes[col] = Some(parent);
                    edges.push(json!({"to_col":col,"curved":false}));
                } else {
                    let i = free(&mut lanes);
                    lanes[i] = Some(parent);
                    edges.push(json!({"to_col":i,"curved":true}));
                }
            }
        }
        let touched = edges
            .iter()
            .filter_map(|e| e["to_col"].as_u64().map(|i| i as usize))
            .chain([col])
            .collect::<HashSet<_>>();
        c["col"] = json!(col);
        c["passthrough"] = json!(
            lanes
                .iter()
                .enumerate()
                .filter_map(|(i, h)| (h.is_some() && !touched.contains(&i)).then_some(i))
                .collect::<Vec<_>>()
        );
        c["edges"] = json!(edges);
    }
    json!(commits)
}
pub fn unquote(path: &str) -> String {
    if !path.starts_with('"') || !path.ends_with('"') || path.len() < 2 {
        return path.into();
    }
    let source = &path.as_bytes()[1..path.len() - 1];
    let mut bytes = Vec::new();
    let mut i = 0;
    while i < source.len() {
        if source[i] != b'\\' || i + 1 == source.len() {
            bytes.push(source[i]);
            i += 1;
            continue;
        }
        i += 1;
        if (b'0'..=b'7').contains(&source[i]) {
            let mut value = 0u16;
            let mut n = 0;
            while i < source.len() && n < 3 && (b'0'..=b'7').contains(&source[i]) {
                value = value * 8 + u16::from(source[i] - b'0');
                i += 1;
                n += 1;
            }
            bytes.push(value as u8);
        } else {
            bytes.push(match source[i] {
                b'n' => b'\n',
                b't' => b'\t',
                b'r' => b'\r',
                b'b' => 8,
                b'f' => 12,
                b'v' => 11,
                b'a' => 7,
                other => other,
            });
            i += 1;
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn numstat(text: &str) -> HashMap<String, (u64, u64)> {
    text.lines()
        .filter_map(|s| {
            let mut p = s.splitn(3, '\t');
            Some((
                unquote(p.nth(2)?),
                (
                    s.split('\t').next()?.parse().ok()?,
                    s.split('\t').nth(1)?.parse().ok()?,
                ),
            ))
        })
        .collect()
}
pub fn changed(cwd: &Path) -> Result<Value> {
    let out = checked(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "status",
            "--porcelain=v1",
            "-z",
        ],
        409,
    )?;
    let nums = command(
        cwd,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--numstat",
            "--no-renames",
            "HEAD",
        ],
    )
    .ok()
    .filter(|o| o.code == 0)
    .map(|o| numstat(&o.stdout))
    .unwrap_or_default();
    let mut fields = out.stdout.split('\0');
    let mut list = Vec::new();
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let code = &field[..2];
        let path = &field[3..];
        if code.contains(['R', 'C']) {
            fields.next();
        }
        let n = nums.get(path);
        list.push(json!({"path":path,"code":code,"staged":!matches!(code.as_bytes()[0],b' '|b'?'),"added":n.map(|v|v.0),"removed":n.map(|v|v.1)}));
    }
    Ok(json!(list))
}
fn changed_item(cwd: &Path, path: &str) -> Result<Value> {
    changed(cwd)?
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["path"] == path)
        .cloned()
        .ok_or_else(|| error(400, "arquivo não está na lista de alterados"))
}
pub fn file_diff(cwd: &Path, path: &str) -> Result<Value> {
    let item = changed_item(cwd, path)?;
    let out = if item["code"] == "??" {
        command(cwd, &["diff", "--no-index", "--", null_file(), path])?
    } else {
        command(cwd, &["--literal-pathspecs", "diff", "HEAD", "--", path])?
    };
    if out.code >= 128 {
        return Err(error(409, out.reason("git diff falhou")));
    }
    let (diff, truncated) = cap(&out.stdout);
    Ok(json!({"path":path,"diff":diff,"truncated":truncated}))
}
pub fn discard(cwd: &Path, path: &str) -> Result<Value> {
    let item = changed_item(cwd, path)?;
    if item["code"] == "??" {
        checked(
            cwd,
            &["--literal-pathspecs", "clean", "-f", "--", path],
            409,
        )?;
    } else {
        checked(
            cwd,
            &[
                "--literal-pathspecs",
                "restore",
                "--staged",
                "--worktree",
                "--source=HEAD",
                "--",
                path,
            ],
            409,
        )?;
    }
    invalidate(cwd);
    Ok(json!({"ok":true,"path":path}))
}
pub fn commit_files(cwd: &Path, revision: &str) -> Result<Value> {
    sha(revision)?;
    let out = checked(
        cwd,
        &[
            "show",
            "--name-status",
            "-z",
            "--format=",
            "-m",
            "--first-parent",
            revision,
        ],
        409,
    )?;
    let mut f = out.stdout.split('\0').filter(|s| !s.is_empty());
    let mut list = Vec::new();
    while let Some(code) = f.next() {
        let Some(mut path) = f.next() else { break };
        if code.starts_with(['R', 'C']) {
            path = f.next().unwrap_or(path);
        }
        list.push(json!({"path":path,"code":code.chars().next().map(|c|c.to_string()).unwrap_or_default()}));
    }
    Ok(json!(list))
}
pub fn commit_file_diff(cwd: &Path, revision: &str, path: &str) -> Result<Value> {
    if !commit_files(cwd, revision)?
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f["path"] == path)
    {
        return Err(error(400, "arquivo não está nesse commit"));
    }
    let out = command(
        cwd,
        &[
            "--literal-pathspecs",
            "show",
            "--format=",
            "-m",
            "--first-parent",
            revision,
            "--",
            path,
        ],
    )?;
    if out.code >= 128 {
        return Err(error(409, out.reason("git show falhou")));
    }
    Ok(json!({"path":path,"diff":out.stdout}))
}
pub fn commit_diff(cwd: &Path, revision: &str) -> Result<Value> {
    sha(revision)?;
    let out = command(
        cwd,
        &["show", "--format=", "-m", "--first-parent", revision],
    )?;
    if out.code >= 128 {
        return Err(error(409, out.reason("git show falhou")));
    }
    let (diff, truncated) = cap(&out.stdout);
    Ok(json!({"sha":revision,"diff":diff,"truncated":truncated}))
}
fn base(cwd: &Path) -> Result<(Option<String>, Option<&'static str>)> {
    let head = command(cwd, &["rev-parse", "--verify", "-q", "HEAD"])?;
    if head.code != 0 {
        return Ok((None, Some("arq_motivo_sem_commit")));
    }
    let current = head.stdout.trim();
    let up = command(
        cwd,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    let upstream = if up.code == 0 { up.stdout.trim() } else { "" };
    let name = command(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let mut best: Option<String> = None;
    let mut on_main = false;
    for r in std::iter::once(upstream)
        .filter(|s| !s.is_empty())
        .chain(MAINLINES.iter().copied())
    {
        let out = command(cwd, &["merge-base", "HEAD", r])?;
        let b = out.stdout.trim();
        if out.code != 0 || b.is_empty() {
            continue;
        }
        if b == current {
            if MAINLINES.contains(&r)
                || (r == upstream && !r.ends_with(&format!("/{}", name.stdout.trim())))
            {
                on_main = true;
            }
            continue;
        }
        if best.is_none()
            || command(
                cwd,
                &["merge-base", "--is-ancestor", best.as_ref().unwrap(), b],
            )?
            .code
                == 0
        {
            best = Some(b.into());
        }
    }
    if on_main {
        Ok((None, Some("arq_motivo_sem_commit_proprio")))
    } else if best.is_some() {
        Ok((best, None))
    } else {
        Ok((
            None,
            Some(if upstream.is_empty() {
                "arq_motivo_sem_base_conhecida"
            } else {
                "arq_motivo_sem_commit_proprio"
            }),
        ))
    }
}
pub fn path_diff(cwd: &Path, path: &str, scope: &str) -> Result<Value> {
    if !["branch", "nao_commitado"].contains(&scope) {
        return Err(error(400, "escopo inválido"));
    }
    if path.starts_with('-') || path.contains('\0') || Path::new(path).is_absolute() {
        return Err(error(400, "caminho inválido"));
    }
    let root = real(cwd);
    let target = real(&cwd.join(path));
    if !target.starts_with(&root) {
        return Err(error(400, "caminho fora do cwd da sessão"));
    }
    let top = real(Path::new(
        checked(cwd, &["rev-parse", "--show-toplevel"], 409)?
            .stdout
            .trim(),
    ));
    if !target.starts_with(&top) {
        return Err(error(400, "caminho fora do repositório"));
    }
    if !target.is_file() {
        return Err(error(404, "arquivo não encontrado"));
    }
    if target
        .strip_prefix(&top)
        .unwrap()
        .components()
        .any(|p| p.as_os_str() == ".git")
    {
        return Err(error(403, "área interna do git"));
    }
    let (base, reason) = if scope == "branch" {
        base(cwd)?
    } else {
        (None, None)
    };
    let used = if scope == "branch" && base.is_none() {
        "nao_commitado"
    } else {
        scope
    };
    let tracked = command(
        cwd,
        &[
            "--literal-pathspecs",
            "ls-files",
            "--error-unmatch",
            "--",
            path,
        ],
    )?
    .code
        == 0;
    let (mut original, args) = (
        Value::Null,
        vec!["-c", "core.quotePath=false", "--literal-pathspecs", "diff"],
    );
    let mut args = args;
    let out = if !tracked {
        original = json!("");
        command(
            cwd,
            &[
                "-c",
                "core.quotePath=false",
                "diff",
                "--no-index",
                "--",
                null_file(),
                path,
            ],
        )?
    } else {
        let rev = if used == "branch" {
            base.as_deref()
        } else {
            Some("HEAD")
        };
        if let Some(rev) = rev {
            if used == "branch" || command(cwd, &["rev-parse", "--verify", "-q", "HEAD"])?.code == 0
            {
                args.push(rev);
            }
            let content = command(cwd, &["show", &format!("{rev}:./{path}")])?;
            if content.code == 0 && content.stdout.chars().count() <= DIFF_MAX {
                original = json!(content.stdout);
            }
        }
        args.extend(["--", path]);
        command(cwd, &args)?
    };
    if out.code >= 128 || out.code != 0 && out.stdout.is_empty() {
        return Err(error(409, out.reason("git diff falhou")));
    }
    let (diff, truncated) = cap(&out.stdout);
    Ok(
        json!({"path":path,"diff":diff,"truncated":truncated,"original":original,"escopo_pedido":scope,"escopo_usado":used,"base":base,"motivo":reason}),
    )
}
pub fn revision_action(cwd: &Path, revision: &str, prefix: &[&str]) -> Result<Value> {
    sha(revision)?;
    let mut args = prefix.to_vec();
    args.push(revision);
    let out = command(cwd, &args)?;
    invalidate(cwd);
    if out.code != 0 {
        return Err(error(409, scrub(&out.both())));
    }
    Ok(json!({"ok":true,"output":scrub(&out.both())}))
}
pub fn validate_ref(cwd: &Path, kind: &str, name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name.as_bytes()[0].is_ascii_alphanumeric()
        || name.contains("..")
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._/-".contains(&b))
    {
        return Err(error(
            400,
            format!(
                "nome de {} inválido",
                if kind == "heads" { "branch" } else { "tag" }
            ),
        ));
    }
    checked(
        cwd,
        &["check-ref-format", &format!("refs/{kind}/{name}")],
        400,
    )?;
    if command(
        cwd,
        &[
            if kind == "heads" { "branch" } else { "tag" },
            "--format=%(refname:short)",
        ],
    )?
    .stdout
    .lines()
    .any(|s| s.trim() == name)
    {
        return Err(error(
            400,
            format!(
                "{} já existe: {name}",
                if kind == "heads" { "branch" } else { "tag" }
            ),
        ));
    }
    Ok(())
}
pub fn create_ref(
    cwd: &Path,
    kind: &str,
    name: &str,
    revision: Option<&str>,
    message: Option<&str>,
    switch: bool,
) -> Result<Value> {
    validate_ref(cwd, kind, name)?;
    if let Some(s) = revision {
        sha(s)?;
    }
    let message = message.filter(|m| !m.trim().is_empty());
    let mut args = if kind == "tags" {
        if let Some(message) = message {
            vec!["tag", "-a", name, "-m", message]
        } else {
            vec!["tag", name]
        }
    } else if switch {
        vec!["switch", "-c", name]
    } else {
        vec!["branch", name]
    };
    if let Some(s) = revision {
        args.push(s);
    }
    let out = checked(cwd, &args, 409)?;
    invalidate(cwd);
    Ok(json!({"ok":true,"output":out.both()}))
}
pub fn diff_worktree(cwd: &Path, revision: &str) -> Result<Value> {
    sha(revision)?;
    let out = checked(cwd, &["diff", revision], 409)?;
    let (diff, truncated) = cap(&out.stdout);
    Ok(json!({"sha":revision,"diff":diff,"truncated":truncated}))
}
pub fn sequencer(cwd: &Path) -> Result<Value> {
    for (marker, label) in [
        ("CHERRY_PICK_HEAD", "cherry-pick"),
        ("REVERT_HEAD", "revert"),
    ] {
        let out = command(cwd, &["rev-parse", "--git-path", marker])?;
        if out.code == 0 && cwd.join(out.stdout.trim()).exists() {
            return Ok(json!(label));
        }
    }
    Ok(Value::Null)
}
pub fn containing(cwd: &Path, revision: &str) -> Result<Value> {
    sha(revision)?;
    let out = checked(
        cwd,
        &[
            "branch",
            "-a",
            "--contains",
            revision,
            "--format=%(refname)",
        ],
        409,
    )?;
    let mut local = Vec::new();
    let mut remote = Vec::new();
    for r in out.stdout.lines() {
        if let Some(s) = r.strip_prefix("refs/heads/") {
            local.push(s);
        } else if let Some(s) = r
            .strip_prefix("refs/remotes/")
            .filter(|s| !s.ends_with("/HEAD"))
        {
            remote.push(s);
        }
    }
    Ok(json!({"local":local,"remote":remote}))
}
pub fn folder_status(cwd: &Path) -> Result<Value> {
    let out = command(cwd, &["status", "--porcelain=v1", "--branch"])?;
    if out.code != 0 {
        if out.stderr.contains("not a git repository") {
            return Ok(json!({"repo":false}));
        }
        return Err(error(409, scrub(&out.reason("git status falhou"))));
    }
    let mut v = parse_summary(&out.stdout);
    if v.is_null() {
        v = json!({"dirty":0,"ahead":null,"behind":null});
    }
    let head = command(cwd, &["symbolic-ref", "--short", "-q", "HEAD"])?;
    let up = command(
        cwd,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    let top = command(cwd, &["rev-parse", "--show-toplevel"])?;
    let fetch = command(cwd, &["rev-parse", "--git-path", "FETCH_HEAD"])?;
    let last = cwd
        .join(fetch.stdout.trim())
        .metadata()
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|t| t.as_secs_f64());
    v["repo"] = json!(true);
    v["current"] = if head.code == 0 {
        json!(head.stdout.trim())
    } else {
        Value::Null
    };
    v["upstream"] = if up.code == 0 {
        json!(up.stdout.trim())
    } else {
        Value::Null
    };
    v["toplevel"] = if top.code == 0 {
        json!(real(Path::new(top.stdout.trim())).to_string_lossy())
    } else {
        Value::Null
    };
    v["last_fetch"] = json!(last);
    Ok(v)
}
fn envelope(code: &str, msg: String, params: Value) -> crate::WorkspaceError {
    crate::WorkspaceError {
        status: 409,
        detail: json!({"code":code,"msg":msg,"params":params}),
        code: None,
    }
}
pub fn guard_folder(st: &Value, sessions: &[String], confirm: bool, check: bool) -> Result<()> {
    if st["repo"] != true {
        return Err(envelope(
            "erro_git_folder_not_repo",
            "a pasta não é um repositório Git".into(),
            json!({}),
        ));
    }
    let n = st["dirty"].as_u64().unwrap_or(0);
    if n > 0 {
        return Err(envelope(
            "erro_git_folder_dirty",
            format!(
                "{n} arquivo(s) com alteração não commitada: nada foi feito. Commite ou guarde essas alterações antes; o Hangar não descarta trabalho."
            ),
            json!({"n":n}),
        ));
    }
    if check && !sessions.is_empty() && !confirm {
        let s = sessions.join(", ");
        return Err(envelope(
            "erro_git_folder_sessions",
            format!(
                "sessões abertas nesta pasta: {s}. Os arquivos delas mudariam; confirme para trocar."
            ),
            json!({"sessoes":s}),
        ));
    }
    Ok(())
}
pub fn folder_pull(cwd: &Path) -> Result<Value> {
    guard_folder(&folder_status(cwd)?, &[], false, false)?;
    network_checked(cwd, &["fetch", "--prune"])?;
    let st = folder_status(cwd)?;
    if st["upstream"].is_null() {
        return Err(envelope(
            "erro_git_folder_no_upstream",
            "a branch atual não acompanha nenhuma branch remota".into(),
            json!({}),
        ));
    }
    let (a, b) = (
        st["ahead"].as_u64().unwrap_or(0),
        st["behind"].as_u64().unwrap_or(0),
    );
    if a > 0 && b > 0 {
        return Err(envelope(
            "erro_git_folder_diverged",
            format!(
                "a branch divergiu de {} ({a} à frente, {b} atrás): pull só avança, resolva no terminal.",
                st["upstream"].as_str().unwrap_or("")
            ),
            json!({"ahead":a,"behind":b,"upstream":st["upstream"]}),
        ));
    }
    if b > 0 {
        checked(cwd, &["merge", "--ff-only", "@{upstream}"], 409)?;
        invalidate(cwd);
    }
    folder_status(cwd)
}
pub fn folder_create(
    cwd: &Path,
    name: &str,
    base: Option<&str>,
    checkout: bool,
    sessions: &[String],
    confirm: bool,
) -> Result<Value> {
    let st = folder_status(cwd)?;
    if checkout || st["repo"] != true {
        guard_folder(&st, sessions, confirm, checkout)?;
    }
    let hash = if let Some(base) = base {
        let info = branches(cwd)?;
        let r = if info["branches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == base)
        {
            format!("refs/heads/{base}")
        } else if info["remotes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|b| b == base)
        {
            remote_ref(cwd, base)?
        } else {
            return Err(error(400, "branch inexistente"));
        };
        Some(
            checked(
                cwd,
                &["rev-parse", "--verify", "-q", &format!("{r}^{{commit}}")],
                409,
            )?
            .stdout
            .trim()
            .to_owned(),
        )
    } else {
        None
    };
    create_ref(cwd, "heads", name, hash.as_deref(), None, checkout)?;
    folder_status(cwd)
}
pub fn last_message(cwd: &Path) -> Result<Value> {
    let out = command(cwd, &["log", "-1", "--pretty=%B"])?;
    if out.code != 0 {
        return Err(error(409, "sem commits para amend"));
    }
    Ok(json!({"message":out.stdout.trim_end_matches('\n')}))
}
pub fn commit(
    cwd: &Path,
    message: &str,
    paths: &[String],
    amend: bool,
    new_branch: Option<&str>,
) -> Result<Value> {
    if message.trim().is_empty() {
        return Err(error(400, "mensagem vazia"));
    }
    if paths.is_empty() && !amend {
        return Err(error(400, "nenhum arquivo selecionado"));
    }
    let snapshot = changed(cwd)?;
    let valid = snapshot
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f["path"].as_str())
        .collect::<HashSet<_>>();
    for path in paths {
        if !valid.contains(path.as_str()) {
            return Err(error(
                400,
                format!("arquivo não está na lista de alterados: {path}"),
            ));
        }
    }
    if amend {
        last_message(cwd)?;
    }
    if let Some(name) = new_branch {
        validate_ref(cwd, "heads", name)?;
        checked(cwd, &["switch", "-c", name], 409)?;
    }
    let out = command(cwd, &["status", "--porcelain=v1", "-z"])?;
    let mut fields = out.stdout.split('\0');
    let mut extra = Vec::new();
    while let Some(f) = fields.next() {
        let (Some(code), Some(path)) = (f.get(..2), f.get(3..)) else {
            continue;
        };
        // Cópia e renomeação trazem a origem no campo seguinte; só a renomeação entra no commit.
        if code.contains(['R', 'C']) {
            let old = fields.next().unwrap_or("");
            if code.starts_with('R') && paths.iter().any(|p| p == path) {
                extra.push(old.to_owned());
            }
        }
    }
    let mut args = vec!["--literal-pathspecs", "commit"];
    if amend {
        args.push("--amend");
    }
    args.extend(["--only", "-m", message]);
    if !paths.is_empty() {
        let mut add = vec!["--literal-pathspecs", "add", "--"];
        add.extend(paths.iter().map(String::as_str));
        checked(cwd, &add, 409)?;
        args.push("--");
        args.extend(paths.iter().map(String::as_str));
        args.extend(extra.iter().map(String::as_str));
    }
    let out = command(cwd, &args)?;
    invalidate(cwd);
    if out.code != 0 {
        return Err(error(
            409,
            scrub(&if out.stderr.trim().is_empty() {
                out.both()
            } else {
                out.stderr.trim().into()
            }),
        ));
    }
    Ok(json!({"ok":true,"output":scrub(&out.both())}))
}
pub fn push(cwd: &Path) -> Result<Value> {
    let branch = command(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    let name = branch.stdout.trim();
    if name.is_empty() || name == "HEAD" {
        return Err(error(409, "sem branch atual (detached HEAD)"));
    }
    let up = command(
        cwd,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )?;
    let out = if up.code == 0 && !up.stdout.trim().is_empty() {
        network_checked(cwd, &["push"])?
    } else {
        if !command(cwd, &["remote"])?
            .stdout
            .split_whitespace()
            .any(|r| r == "origin")
        {
            return Err(error(
                409,
                "branch sem upstream e sem remote 'origin' — configure um remote antes",
            ));
        }
        network_checked(cwd, &["push", "-u", "origin", name])?
    };
    invalidate(cwd);
    Ok(json!({"ok":true,"output":scrub(&out.both())}))
}
