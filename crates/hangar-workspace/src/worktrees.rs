//! Lista de worktrees e a situação de cada uma, no formato do `worktrees.py`.
//!
//! Configuração e referências são compartilhadas por repositório; o reflog recupera a base das
//! branches criadas fora do app. `rev-list --left-right` dá ahead, behind e a ancestralidade de
//! uma vez. As worktrees rodam em paralelo.
use crate::{Result, error, git, real};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet},
    path::{Component, Path, PathBuf},
    sync::{
        LazyLock, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const PARALLEL: usize = 8;
const DIRTY_LIST: usize = 50;

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Session {
    pub name: String,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub worktree_path: Option<String>,
    #[serde(default)]
    pub jsonl: Option<String>,
}

/// Lexical, como o `os.path.normpath`: `..` sobe sem seguir link.
pub fn normpath(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                } else if !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Caminhos iguais: no Windows sem distinguir maiúsculas, como o `normcase` do Python.
fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// `target` dentro de `root`, os dois pelo mesmo `real`: a caixa que o macOS devolve no
/// `canonicalize` pode não ser a do texto das raízes que o Python manda.
fn within(target: &Path, root: &str) -> bool {
    target.starts_with(real(Path::new(root)))
}

/// Teto de `git` simultâneos da lista, somando todos os pedidos: dois aparelhos abrindo a tela
/// juntos não dobram os processos.
struct Gate {
    used: Mutex<usize>,
    freed: std::sync::Condvar,
}

impl Gate {
    fn run<T>(&self, f: impl FnOnce() -> T) -> T {
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        while *used >= PARALLEL {
            used = self.freed.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += 1;
        drop(used);
        struct Release<'a>(&'a Gate);
        impl Drop for Release<'_> {
            fn drop(&mut self) {
                *self.0.used.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
                self.0.freed.notify_one();
            }
        }
        let _release = Release(self);
        f()
    }
}

static GATE: Gate = Gate {
    used: Mutex::new(0),
    freed: std::sync::Condvar::new(),
};

fn is_dir(path: &str) -> bool {
    !path.is_empty() && Path::new(path).is_dir()
}

pub fn repo_root_of(path: &str) -> Option<String> {
    if path.is_empty() {
        return None;
    }
    Path::new(path)
        .ancestors()
        .find(|d| d.join(".git").exists())
        .map(text)
}

pub fn main_repo_of(root: &str) -> String {
    let dot = Path::new(root).join(".git");
    if dot.is_file()
        && let Ok(raw) = std::fs::read(&dot)
        && let Some(target) = String::from_utf8_lossy(&raw).trim().strip_prefix("gitdir: ")
    {
        // normpath: com `worktree.useRelativePaths` o ponteiro vem com `..`.
        let gitdir = normpath(&Path::new(root).join(target));
        if gitdir.parent().and_then(Path::file_name).is_some_and(|n| n == "worktrees")
            && let Some(main) = gitdir.parent().and_then(Path::parent).and_then(Path::parent)
        {
            return text(main);
        }
    }
    root.to_owned()
}

/// `(pasta de administração, worktree)` de `.git/worktrees/*`, na ordem do nome.
fn admin_entries(main: &str) -> Vec<(PathBuf, String)> {
    let Ok(dir) = std::fs::read_dir(Path::new(main).join(".git").join("worktrees")) else {
        return Vec::new();
    };
    let mut entries = dir.flatten().map(|e| e.path()).collect::<Vec<_>>();
    entries.sort();
    entries
        .into_iter()
        .filter_map(|e| {
            let raw = std::fs::read(e.join("gitdir")).ok()?;
            let g = String::from_utf8_lossy(&raw).trim().to_owned();
            if g.is_empty() {
                return None;
            }
            let target = normpath(&e.join(g));
            let path = text(target.parent().unwrap_or(Path::new("")));
            Some((e, path))
        })
        .collect()
}

pub fn worktree_paths(main: &str) -> Vec<String> {
    admin_entries(main).into_iter().map(|(_, p)| p).collect()
}

fn removed_file() -> Option<PathBuf> {
    crate::home().map(|h| h.join(".hangar").join("worktrees-removidas.json"))
}

pub fn removed() -> HashMap<String, String> {
    removed_file().map(|f| removed_at(&f)).unwrap_or_default()
}

/// O mapa de remoções de um arquivo dado; ilegível ou torto vale vazio.
pub fn removed_at(file: &Path) -> HashMap<String, String> {
    try_removed_at(file).unwrap_or_default()
}

/// O mapa de remoções, com a falha à vista: ausente é vazio, ilegível ou torto é `Err` (a lista
/// recebe a pasta de casa por parâmetro e avisa, senão a worktree apagada vira pasta comum calada).
pub fn try_removed_at(file: &Path) -> std::io::Result<HashMap<String, String>> {
    let raw = match std::fs::read(file) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(HashMap::new()),
        Err(e) => return Err(e),
    };
    let map: HashMap<String, Value> = serde_json::from_slice(&raw)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(map.into_iter().filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_owned()))).collect())
}

/// Pasta sumida: o repo que ainda a lista, pelo mapa de remoções, pela irmã ou subindo.
pub fn main_of_missing(path: &str) -> String {
    if let Some(mapped) = removed().get(path).filter(|m| !m.is_empty()) {
        return mapped.clone();
    }
    let p = Path::new(path);
    let mut cands = p
        .parent()
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|d| d.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default();
    cands.sort();
    cands.extend(p.ancestors().skip(1).map(Path::to_path_buf));
    cands
        .into_iter()
        .find(|c| c.join(".git").is_dir() && worktree_paths(&text(c)).iter().any(|w| w == path))
        .map(|c| text(&c))
        .unwrap_or_else(|| path.to_owned())
}

/// O que é do repositório e serve a todas as worktrees dele: lido uma vez.
pub struct Repo {
    main: String,
    main_branch: Option<String>,
    admins: Vec<(PathBuf, String)>,
    /// `None` quando a leitura falhou; metadados ausentes nunca tornam a exclusão segura.
    config: Option<HashMap<String, String>>,
    references: Option<HashMap<String, String>>,
    remotes: Option<Vec<String>>,
}

impl Repo {
    pub fn read(main: &str) -> Repo {
        let config = git::command(
            Path::new(main),
            &["config", "-z", "--get-regexp", r"^branch\..*\.hangar-base$"],
        )
        .ok()
        // Código 1 é ausência de chave; outros erros não podem tornar a exclusão segura.
        .filter(|out| out.code == 0 || out.code == 1)
        .map(|out| {
            let mut map = HashMap::new();
            let found = if out.code == 0 {
                out.stdout.as_str()
            } else {
                ""
            };
            for entry in found.split('\0') {
                let (key, value) = entry.split_once('\n').unwrap_or((entry, ""));
                let Some(name) = key
                    .strip_prefix("branch.")
                    .and_then(|s| s.strip_suffix(".hangar-base"))
                else {
                    continue;
                };
                if !value.trim().is_empty() {
                    map.insert(name.to_owned(), value.trim().to_owned());
                }
            }
            map
        });
        let references = git::command(
            Path::new(main),
            &[
                "for-each-ref",
                "--format=%(refname)%00%(upstream)",
                "refs/heads/",
                "refs/remotes/",
                "refs/tags/",
            ],
        )
        .ok()
        .filter(|out| out.code == 0)
        .map(|out| {
            out.stdout
                .lines()
                .filter_map(|line| line.split_once('\0'))
                .map(|(name, upstream)| (name.to_owned(), upstream.to_owned()))
                .collect()
        });
        let remotes = git::command(Path::new(main), &["remote"])
            .ok()
            .filter(|out| out.code == 0)
            .map(|out| out.stdout.lines().map(str::to_owned).collect());
        Repo {
            main: main.to_owned(),
            main_branch: git::head_info(Some(main)).0,
            admins: admin_entries(main),
            config,
            references,
            remotes,
        }
    }

    fn published_base(&self, cwd: &str, base: &str, probe: &mut Probe) -> String {
        let Some(refs) = self.references.as_ref() else {
            return base.to_owned();
        };
        let remotes = self.remotes.as_deref().unwrap_or_default();
        let matches = [format!("refs/heads/{base}"), format!("refs/remotes/{base}")]
            .into_iter()
            .filter(|r| refs.contains_key(r))
            .collect::<Vec<_>>();
        if matches.len() > 1 {
            probe.failed = true;
            return base.to_owned();
        }
        let mut reference = matches.first().cloned().unwrap_or_else(|| base.to_owned());
        if matches.is_empty() && remotes.iter().any(|r| base.starts_with(&format!("{r}/"))) {
            reference = format!("refs/remotes/{base}");
        }
        if let Some(name) = reference.strip_prefix("refs/heads/")
            && let Some(upstream) = refs.get(&reference)
        {
            if upstream.starts_with("refs/remotes/") {
                reference = upstream.clone();
            } else {
                let candidates = remotes
                    .iter()
                    .map(|r| format!("refs/remotes/{r}/{name}"))
                    .filter(|r| refs.contains_key(r))
                    .collect::<Vec<_>>();
                if candidates.len() == 1 {
                    reference = candidates[0].clone();
                } else if candidates.len() > 1 {
                    probe.failed = true;
                }
            }
        }
        if !refs.contains_key(&reference)
            && probe
                .ok(
                    cwd,
                    &[
                        "rev-parse",
                        "--verify",
                        "--end-of-options",
                        &format!("{reference}^{{commit}}"),
                    ],
                )
                .is_none()
        {
            probe.failed = true;
        }
        let short = reference
            .strip_prefix("refs/heads/")
            .or_else(|| reference.strip_prefix("refs/remotes/"))
            .unwrap_or(&reference);
        // Tags têm precedência sobre branches; abreviar não pode trocar a referência escolhida.
        if refs.contains_key(&format!("refs/tags/{short}"))
            || (reference.starts_with("refs/remotes/")
                && refs.contains_key(&format!("refs/heads/{short}")))
        {
            reference
        } else {
            short.to_owned()
        }
    }

    fn base_of(&self, cwd: &str, branch: &str, probe: &mut Probe) -> Option<String> {
        let configured = self.config.as_ref().and_then(|c| c.get(branch)).cloned();
        let base = configured.or_else(|| {
            let log = probe.ok(
                cwd,
                &[
                    "reflog",
                    "show",
                    "--format=%gs",
                    &format!("refs/heads/{branch}"),
                ],
            );
            if log.is_none() {
                probe.failed = true;
            }
            let log = log.unwrap_or_default();
            let source = log
                .lines()
                .rev()
                .find_map(|l| l.strip_prefix("branch: Created from "));
            let refs = self.references.as_ref();
            let remotes = self.remotes.as_deref().unwrap_or_default();
            let source = source.filter(|s| {
                let named = s.starts_with("refs/heads/")
                    || s.starts_with("refs/remotes/")
                    || refs.is_some_and(|r| r.contains_key(&format!("refs/heads/{s}")))
                    || remotes.iter().any(|r| s.starts_with(&format!("{r}/")));
                let own = *s == branch
                    || *s == format!("refs/heads/{branch}")
                    || remotes.iter().any(|r| {
                        *s == format!("{r}/{branch}") || *s == format!("refs/remotes/{r}/{branch}")
                    });
                // HEAD e hashes não registram o destino; o remoto homônimo é a própria branch
                // publicada. O upstream não entra: `worktree add -b x ../w origin/release` rastreia a base.
                named && !own
            });
            source
                .map(str::to_owned)
                .or_else(|| self.main_branch.clone())
        });
        base.map(|b| self.published_base(cwd, &b, probe))
    }

    fn admin_of(&self, path: &str) -> Option<&PathBuf> {
        let wanted = real(Path::new(path));
        self.admins
            .iter()
            .find(|(_, p)| same_path(&real(Path::new(p)), &wanted))
            .map(|(e, _)| e)
    }

    /// Branch de uma worktree cuja pasta sumiu, pelo HEAD guardado na administração dela.
    fn gone_branch(&self, path: &str) -> Option<String> {
        let (admin, _) = self.admins.iter().find(|(_, p)| p == path)?;
        let head = std::fs::read_to_string(admin.join("HEAD")).ok()?;
        head.trim()
            .strip_prefix("ref: refs/heads/")
            .filter(|b| !b.is_empty())
            .map(str::to_owned)
    }

    fn created_at(&self, path: &str) -> Option<i64> {
        // `commondir` é escrito só no `worktree add`; `gitdir` e `HEAD` mudam depois.
        let at = std::fs::metadata(self.admin_of(path)?.join("commondir"))
            .ok()?
            .modified()
            .ok()?;
        Some(match at.duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs() as i64,
            Err(e) => -(e.duration().as_secs() as i64),
        })
    }
}

/// Uma leitura da situação: falha de `git` (prazo, não iniciou) marca `degraded`.
struct Probe {
    failed: bool,
}

impl Probe {
    fn git(&mut self, cwd: &str, args: &[&str]) -> Option<crate::process::Output> {
        match GATE.run(|| git::command(Path::new(cwd), args)) {
            Ok(out) => Some(out),
            Err(e) => {
                // Sem o detalhe: o stderr do git pode citar caminhos do usuário.
                tracing::warn!(command = args[0], status = e.status, "worktrees: git falhou");
                self.failed = true;
                None
            }
        }
    }

    fn ok(&mut self, cwd: &str, args: &[&str]) -> Option<String> {
        self.git(cwd, args).filter(|o| o.code == 0).map(|o| o.stdout)
    }

    fn log(&mut self, cwd: &str, rev: &str, n: usize) -> Vec<Value> {
        let count = format!("-{n}");
        let out = self
            .ok(cwd, &["log", &count, "--format=%h%x00%s%x00%ct", rev, "--"])
            .unwrap_or_default();
        out.split('\n')
            .filter_map(|line| {
                let (sha, rest) = line.split_once('\0')?;
                let (subject, at) = rest.rsplit_once('\0')?;
                let at = (!at.is_empty() && at.bytes().all(|b| b.is_ascii_digit()))
                    .then(|| at.parse::<i64>().ok())
                    .flatten()?;
                (!sha.is_empty()).then(|| json!({"sha":sha,"subject":subject,"at":at}))
            })
            .collect()
    }
}

/// Arquivos ignorados (pastas nunca) que só existem aqui; cópia idêntica à da principal fica de fora.
fn ignored_lost(probe: &mut Probe, path: &str, main: &str) -> Vec<String> {
    let out = probe
        .ok(
            path,
            &["ls-files", "--others", "--ignored", "--exclude-standard", "--directory", "-z"],
        )
        .unwrap_or_default();
    let mut lost = out
        .split('\0')
        .filter(|rel| !rel.is_empty() && !rel.ends_with('/'))
        .filter(|rel| !same_file(&Path::new(path).join(rel), &Path::new(main).join(rel)))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    lost.sort();
    lost
}

/// Em blocos, como o `filecmp`: um dump grande ignorado não vai inteiro para a memória.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else {
        return false;
    };
    if !mb.is_file() || ma.len() != mb.len() {
        return false;
    }
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else {
        return false;
    };
    let (mut ba, mut bb) = (vec![0u8; 64 * 1024], vec![0u8; 64 * 1024]);
    loop {
        let (Ok(n), Ok(m)) = (fa.read(&mut ba), fb.read(&mut bb)) else {
            return false;
        };
        // `read` pode devolver menos que o pedido; só segue em passo igual, senão relê inteiro.
        if n != m {
            return matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y);
        }
        if n == 0 {
            return true;
        }
        if ba[..n] != bb[..n] {
            return false;
        }
    }
}

/// Por realpath: home atrás de symlink dá dois textos para a mesma pasta.
fn inside(s: &Session, real_path: &Path) -> bool {
    if let Some(wt) = s.worktree_path.as_deref().filter(|w| !w.is_empty())
        && real(Path::new(wt)) == real_path
    {
        return true;
    }
    s.cwd
        .as_deref()
        .filter(|c| !c.is_empty())
        .is_some_and(|c| real(Path::new(c)).starts_with(real_path))
}

/// Como o `sanitize_cwd` do registry: o Claude indexa a pasta assim.
pub fn sanitize_cwd(cwd: &str) -> String {
    let trimmed = cwd.trim_end_matches(['/', '\\']);
    let clean = if trimmed.is_empty() || trimmed.ends_with(':') {
        cwd
    } else {
        trimmed
    };
    clean
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// `<uuid>.jsonl` é conversa do Claude. A cópia `<uuid>.from-<conta>.jsonl` que a exclusão de uma
/// conta deixa não é: retomá-la é recusado e adotá-la pelo mtime troca o transcript de uma sessão.
pub fn is_conversation_file(name: &str) -> bool {
    name.strip_suffix(".jsonl").is_some_and(|stem| {
        stem.len() == 36
            && stem.char_indices().all(|(i, c)| if matches!(i, 8 | 13 | 18 | 23) { c == '-' } else { c.is_ascii_hexdigit() })
    })
}

/// Conversas guardadas na pasta, menos as das sessões vivas dentro dela.
fn closed_count(path: &str, live: &HashSet<PathBuf>, bases: &[String]) -> usize {
    let project = sanitize_cwd(path);
    bases
        .iter()
        .filter_map(|b| std::fs::read_dir(Path::new(b).join(&project)).ok())
        .flat_map(|d| d.flatten())
        .filter(|e| is_conversation_file(&e.file_name().to_string_lossy()))
        .filter(|e| !live.contains(&real(&e.path())))
        .count()
}

pub fn status(
    path: &str,
    sessions: &[Session],
    repo: &Repo,
    measure: bool,
    bases: &[String],
) -> Value {
    let main = repo.main.as_str();
    let exists = is_dir(path);
    let branch = if exists {
        git::head_info(Some(path)).0
    } else {
        repo.gone_branch(path)
    };
    let mut probe = Probe {
        failed: branch.is_some()
            && (repo.config.is_none() || repo.references.is_none() || repo.remotes.is_none()),
    };
    let cwd = if exists { path } else { main };
    let base = branch
        .as_deref()
        .and_then(|b| repo.base_of(cwd, b, &mut probe));
    let (mut ahead, mut behind, mut ancestor) = (0u64, 0u64, false);
    let mut commits = Vec::new();
    let mut last = Vec::new();
    if let (Some(branch), Some(base)) = (&branch, &base)
        && branch != base
    {
        // Nome curto perde para uma tag homônima, e o commit dela pareceria já mesclado.
        let range = format!("{base}...refs/heads/{branch}");
        if let Some(out) = probe.ok(cwd, &["rev-list", "--left-right", "--count", &range]) {
            let mut n = out
                .split_whitespace()
                .map(|s| s.parse::<u64>().unwrap_or(0));
            behind = n.next().unwrap_or(0);
            ahead = n.next().unwrap_or(0);
            // Recém-criada aponta pro mesmo commit da base: ancestral trivial, não mesclada.
            ancestor = ahead == 0 && behind > 0;
        }
        commits = probe.log(cwd, &format!("{base}..refs/heads/{branch}"), 3);
        if ahead > 0 && !commits.is_empty() {
            // O primeiro do `base..branch` é a ponta da branch.
            last = vec![commits[0].clone()];
        }
    }
    if last.is_empty() && (exists || branch.is_some()) {
        let reference = branch.as_deref().map_or("HEAD".into(), |b| format!("refs/heads/{b}"));
        last = probe.log(cwd, &reference, 1);
    }
    let mut dirty_files = Vec::new();
    if exists && let Some(out) = probe.ok(path, &["status", "--porcelain"]) {
        dirty_files = out
            .split('\n')
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let code = l.get(..2).unwrap_or(l).trim();
                json!({"code": if code.is_empty() {"?"} else {code}, "path": l.get(3..).unwrap_or("")})
            })
            .collect();
    }
    // Apagar o upstream também acontece sem merge: não autoriza excluir os commits locais.
    let merged = ancestor;
    let ignored = if exists {
        ignored_lost(&mut probe, path, main)
    } else {
        Vec::new()
    };
    let real_path = real(Path::new(path));
    let inside = sessions
        .iter()
        .filter(|s| inside(s, &real_path))
        .collect::<Vec<_>>();
    let mut names = inside.iter().map(|s| s.name.clone()).collect::<Vec<_>>();
    names.sort();
    let live = inside
        .iter()
        .filter_map(|s| s.jsonl.as_deref())
        .map(|j| real(Path::new(j)))
        .collect::<HashSet<_>>();
    let created_at = repo.created_at(path);
    let size = exists
        .then(|| disk_usage(path, created_at, measure))
        .flatten();
    let failed = probe.failed;
    json!({
        "path": path, "repo": main, "exists": exists, "branch": branch, "base": base,
        "main_branch": repo.main_branch,
        // Leitura que falhou deixa dirty/ignored zerados: a situação nunca pode parecer segura.
        "merged": merged && !failed, "degraded": failed,
        "ahead": ahead, "behind": behind, "dirty": dirty_files.len(),
        "dirty_files": &dirty_files[..dirty_files.len().min(DIRTY_LIST)], "ignored": ignored,
        "last_commit": last.first(), "commits": commits,
        "created_at": created_at,
        "size": size.as_ref().map(|s| s.bytes), "size_biggest": size.as_ref().map(|s| s.biggest.clone()),
        "size_pending": exists && size.is_none(),
        "size_error": size.as_ref().is_some_and(|s| s.error),
        "size_partial": size.as_ref().is_some_and(|s| s.partial),
        "sessions": names,
        "closed": closed_count(path, &live, bases),
    })
}

/// Situação de uma pasta sem saber o repositório: o mesmo `status()` sem `main` do Python.
pub fn status_of(path: &str, sessions: &[Session], measure: bool, bases: &[String]) -> Value {
    let root = if is_dir(path) { repo_root_of(path) } else { None };
    let main = match root {
        Some(root) => main_repo_of(&root),
        None => main_of_missing(path),
    };
    status(path, sessions, &Repo::read(&main), measure, bases)
}

/// `roots`: só repos dentro delas. `repo`: só o desse repositório, mesmo sem sessão aberta.
pub fn list_all(
    cwds: &[String],
    sessions: &[Session],
    roots: Option<&[String]>,
    repo: Option<&str>,
    measure: bool,
    bases: &[String],
) -> Value {
    let mut wanted: Vec<&str> = match repo {
        Some(r) => vec![r],
        None => cwds.iter().map(String::as_str).collect(),
    };
    wanted.sort_unstable();
    wanted.dedup();
    let mut mains = wanted
        .into_iter()
        .filter_map(repo_root_of)
        // realpath: o mesmo repo por um symlink apareceria duas vezes.
        .map(|root| text(&real(Path::new(&main_repo_of(&root)))))
        .collect::<Vec<_>>();
    mains.sort();
    mains.dedup();
    let repos = mains
        .into_iter()
        .filter(|m| {
            roots.is_none_or(|roots| {
                let m = real(Path::new(m));
                roots.iter().any(|r| within(&m, r))
            })
        })
        // Sem worktree o repo sai da lista: não paga o `git config` do Repo::read.
        .filter(|m| !admin_entries(m).is_empty())
        .map(|m| Repo::read(&m))
        .filter(|r| !r.admins.is_empty())
        .collect::<Vec<_>>();
    let jobs = repos
        .iter()
        .enumerate()
        .flat_map(|(i, r)| r.admins.iter().map(move |(_, p)| (i, p.as_str())))
        .collect::<Vec<_>>();
    let results = Mutex::new(vec![Value::Null; jobs.len()]);
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..PARALLEL.min(jobs.len()) {
            scope.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((r, path)) = jobs.get(i) else { break };
                    let value = status(path, sessions, &repos[*r], measure, bases);
                    results.lock().unwrap_or_else(|e| e.into_inner())[i] = value;
                }
            });
        }
    });
    let mut results = results.into_inner().unwrap_or_else(|e| e.into_inner()).into_iter();
    Value::Array(
        repos
            .iter()
            .map(|r| {
                let worktrees = results.by_ref().take(r.admins.len()).collect::<Vec<_>>();
                json!({"repo": r.main, "worktrees": worktrees})
            })
            .collect(),
    )
}

/// Pasta dentro de uma raiz liberada: a fronteira do `_allowed_scan_root` do `api.py`.
fn allowed_scan_root(path: &str, roots: &[String]) -> Result<()> {
    let target = real(Path::new(path));
    if !roots.iter().any(|r| within(&target, r)) {
        return Err(error(403, "root not allowed"));
    }
    if !target.exists() {
        return Err(error(404, "path not found"));
    }
    if !target.is_dir() {
        return Err(error(400, "not a directory"));
    }
    Ok(())
}

/// Worktree fora das raízes (o Codex cria em `~/.codex/worktrees`) vale pelo repo principal
/// que a registra, se ele estiver numa raiz.
fn registered_in_allowed_repo(path: &str, roots: &[String]) -> bool {
    let main = if is_dir(path) {
        let Some(root) = repo_root_of(path) else { return false };
        if real(Path::new(&root)) != real(Path::new(path)) {
            return false;
        }
        main_repo_of(&root)
    } else {
        main_of_missing(path)
    };
    let real_path = real(Path::new(path));
    real(Path::new(&main)) != real_path
        && allowed_scan_root(&main, roots).is_ok()
        && worktree_paths(&main)
            .iter()
            .any(|p| p == path || real(Path::new(p)) == real_path)
}

/// Repo/worktree dentro de uma raiz autorizada; pasta sumida valida pela pasta-mãe.
pub fn allowed_repo(path: &str, roots: &[String]) -> Result<String> {
    let dir = is_dir(path);
    let probe = if dir {
        path.to_owned()
    } else {
        text(Path::new(path).parent().unwrap_or(Path::new("")))
    };
    if let Err(e) = allowed_scan_root(&probe, roots)
        && !registered_in_allowed_repo(path, roots)
    {
        return Err(e);
    }
    Ok(if dir {
        text(&real(Path::new(path)))
    } else {
        path.to_owned()
    })
}

pub fn allowed_worktree(path: &str, roots: &[String]) -> Result<String> {
    let path = allowed_repo(path, roots)?;
    // Pasta que existe mas não é raiz de repo/worktree daria uma situação inventada.
    if is_dir(&path) && !Path::new(&path).join(".git").exists() {
        return Err(error(404, "não é um repositório git"));
    }
    Ok(path)
}

// --- Espaço em disco: medido por trás, uma pasta por vez, com cache ---

const SIZE_TTL: Duration = Duration::from_secs(15 * 60);
/// Medição que falhou volta a ser tentada depois disso, não a cada leitura.
const SIZE_RETRY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug)]
pub struct Size {
    pub bytes: Option<u64>,
    pub biggest: Option<Value>,
    pub error: bool,
    pub partial: bool,
}

/// Chave com a data de criação: worktree recriada no mesmo caminho não herda o tamanho velho.
type SizeKey = (String, Option<i64>);

#[derive(Default)]
struct Sizes {
    done: HashMap<SizeKey, (Instant, Size)>,
    running: HashSet<SizeKey>,
    queue: Option<mpsc::Sender<SizeKey>>,
}

static SIZES: LazyLock<Mutex<Sizes>> = LazyLock::new(Mutex::default);

/// Do cache; vencido ou ausente agenda a medição e devolve o que houver. A lista nunca espera.
pub fn disk_usage(path: &str, created_at: Option<i64>, schedule: bool) -> Option<Size> {
    let key = (path.to_owned(), created_at);
    let mut sizes = SIZES.lock().unwrap_or_else(|e| e.into_inner());
    let hit = sizes.done.get(&key).cloned();
    let stale = hit.as_ref().is_none_or(|(due, _)| Instant::now() >= *due);
    if schedule && stale && !sizes.running.contains(&key) {
        if sizes.queue.is_none() {
            let (tx, rx) = mpsc::channel::<SizeKey>();
            // ponytail: uma medição por vez para não disputar disco com o resto da máquina.
            let spawned = std::thread::Builder::new()
                .name("wt-size".into())
                .spawn(move || {
                    for key in rx {
                        measure(key);
                    }
                });
            match spawned {
                Ok(_) => sizes.queue = Some(tx),
                // Sem thread a medição fica pendente e a próxima leitura tenta de novo.
                Err(_) => tracing::error!("worktrees: não abri a thread de medição"),
            }
        }
        if let Some(queue) = &sizes.queue
            && queue.send(key.clone()).is_ok()
        {
            sizes.running.insert(key);
        } else {
            // A fila morreu: sem isto, toda worktree ficaria "medindo" para sempre, calada.
            tracing::error!("worktrees: fila de medição parou; recriando");
            sizes.queue = None;
        }
    }
    hit.map(|(_, size)| size)
}

fn measure(key: SizeKey) {
    let path = Path::new(&key.0);
    let (due, size) = match std::panic::catch_unwind(|| tree_size(path)) {
        Ok(Ok(size)) => (Instant::now() + SIZE_TTL, size),
        failed => {
            if !matches!(failed, Ok(Err(_))) {
                tracing::error!("worktrees: pânico ao medir o espaço");
            } else {
                tracing::warn!("worktrees: não medi o espaço de uma worktree");
            }
            let size = Size { bytes: None, biggest: None, error: true, partial: false };
            (Instant::now() + SIZE_RETRY, size)
        }
    };
    let mut sizes = SIZES.lock().unwrap_or_else(|e| e.into_inner());
    sizes.running.remove(&key);
    // Chave de worktree apagada ou recriada nunca mais é lida: sai quando venceu há tempo.
    let now = Instant::now();
    sizes.done.retain(|_, (due, _)| now < *due + SIZE_TTL);
    // Apagada durante a medição: não volta para o cache.
    if path.is_dir() {
        sizes.done.insert(key, (due, size));
    } else {
        sizes.done.remove(&key);
    }
}

fn entry_bytes(meta: &std::fs::Metadata) -> u64 {
    // Blocos ocupados, como o `du`; o Windows não os informa.
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.blocks() * 512
    }
    #[cfg(not(unix))]
    {
        meta.len()
    }
}

/// Link e junction não contam: o `is_symlink` do Windows cobre os dois.
fn tree_size(path: &Path) -> std::io::Result<Size> {
    let mut skipped = 0usize;
    let mut biggest: Option<(String, u64)> = None;
    let mut total = 0u64;
    for entry in std::fs::read_dir(path)? {
        let Ok(entry) = entry else {
            skipped += 1;
            continue;
        };
        let bytes = entry_tree(&entry, &mut skipped);
        total += bytes;
        if biggest.as_ref().is_none_or(|(_, b)| bytes > *b) {
            biggest = Some((entry.file_name().to_string_lossy().into_owned(), bytes));
        }
    }
    if skipped > 0 {
        tracing::warn!(skipped, "worktrees: pastas ilegíveis ao medir; o total é parcial");
    }
    Ok(Size {
        bytes: Some(total),
        biggest: biggest.map(|(name, bytes)| json!({"name": name, "bytes": bytes})),
        error: false,
        partial: skipped > 0,
    })
}

fn entry_tree(entry: &std::fs::DirEntry, skipped: &mut usize) -> u64 {
    let Ok(kind) = entry.file_type() else {
        *skipped += 1;
        return 0;
    };
    if kind.is_symlink() {
        return 0;
    }
    if !kind.is_dir() {
        return entry.metadata().map(|m| entry_bytes(&m)).unwrap_or_else(|_| {
            *skipped += 1;
            0
        });
    }
    let mut total = 0;
    let mut stack = vec![entry.path()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            *skipped += 1;
            continue;
        };
        for e in read {
            let Ok(e) = e else {
                *skipped += 1;
                continue;
            };
            match e.file_type() {
                Ok(k) if k.is_symlink() => {}
                Ok(k) if k.is_dir() => stack.push(e.path()),
                Ok(_) => match e.metadata() {
                    Ok(m) => total += entry_bytes(&m),
                    Err(_) => *skipped += 1,
                },
                Err(_) => *skipped += 1,
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_matches_the_registry() {
        assert_eq!(sanitize_cwd("/home/x/ação/"), "-home-x-a--o");
        assert_eq!(sanitize_cwd("/"), "-");
        assert_eq!(sanitize_cwd("C:\\x\\"), "C--x");
    }

    #[test]
    fn normpath_is_lexical() {
        assert_eq!(normpath(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(normpath(Path::new("../../x")), PathBuf::from("../../x"));
        assert_eq!(normpath(Path::new("/../x")), PathBuf::from("/x"));
    }

    #[test]
    fn size_waits_for_the_background_and_recreation_misses_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wt");
        std::fs::create_dir_all(path.join("big")).unwrap();
        std::fs::write(path.join("big/a"), vec![1u8; 64 * 1024]).unwrap();
        std::fs::write(path.join("b"), b"x").unwrap();
        let p = text(&path);
        assert!(disk_usage(&p, Some(1), true).is_none());
        let size = (0..500)
            .find_map(|_| {
                std::thread::sleep(Duration::from_millis(10));
                disk_usage(&p, Some(1), false)
            })
            .expect("medição");
        assert!(!size.error && !size.partial);
        assert!(size.bytes.unwrap() >= 64 * 1024);
        assert_eq!(size.biggest.unwrap()["name"], "big");
        assert!(disk_usage(&p, Some(2), false).is_none());
    }
}
