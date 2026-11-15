//! Lista de worktrees e a situação de cada uma, no formato do `worktrees.py`.
//!
//! Configuração e referências são compartilhadas por repositório; o reflog recupera a base das
//! branches criadas fora do app. `rev-list --left-right` dá ahead, behind e a ancestralidade de
//! uma vez. As worktrees rodam em paralelo.
use crate::{Result, error, git, real, worktree_residual};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
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
    pub git_cwd: Option<String>,
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
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// A pasta ausente ainda pode ter ancestrais com nome curto ou caixa diferente no Windows.
pub(crate) fn registration_path(path: &Path) -> PathBuf {
    let path = real(path);
    for ancestor in path.ancestors() {
        if let Ok(resolved) = std::fs::canonicalize(ancestor) {
            let suffix = path.strip_prefix(ancestor).unwrap();
            let resolved = crate::simplify(resolved);
            return if suffix.as_os_str().is_empty() { resolved } else { resolved.join(suffix) };
        }
    }
    path
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
        && let Some(target) = String::from_utf8_lossy(&raw)
            .trim()
            .strip_prefix("gitdir: ")
    {
        // normpath: com `worktree.useRelativePaths` o ponteiro vem com `..`.
        let gitdir = normpath(&Path::new(root).join(target));
        if gitdir
            .parent()
            .and_then(Path::file_name)
            .is_some_and(|n| n == "worktrees")
            && let Some(main) = gitdir
                .parent()
                .and_then(Path::parent)
                .and_then(Path::parent)
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
    Ok(map
        .into_iter()
        .filter_map(|(k, v)| v.as_str().map(|v| (k, v.to_owned())))
        .collect())
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
        .find(|c| c.join(".git").is_dir()
            && worktree_paths(&text(c)).iter().any(|w| same_path(&normpath(Path::new(w)), &normpath(p))))
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
        let wanted = normpath(Path::new(path));
        let (admin, _) = self.admins.iter().find(|(_, p)| same_path(&normpath(Path::new(p)), &wanted))?;
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
                tracing::warn!(
                    command = args[0],
                    status = e.status,
                    "worktrees: git falhou"
                );
                self.failed = true;
                None
            }
        }
    }

    fn ok(&mut self, cwd: &str, args: &[&str]) -> Option<String> {
        self.git(cwd, args)
            .filter(|o| o.code == 0)
            .map(|o| o.stdout)
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

/// O que o `--force` apaga sem estar no status: ignorados (diretórios inteiros e arquivos sem
/// cópia idêntica na principal) e submódulos inicializados, cujo conteúdo o Git não remove sem força.
fn ignored_lost(probe: &mut Probe, path: &str, main: &str) -> Vec<String> {
    let submodules = probe
        .ok(path, &["ls-files", "--stage", "-z"])
        .unwrap_or_default()
        .split('\0')
        .filter_map(|entry| entry.strip_prefix("160000 ")?.split_once('\t').map(|(_, rel)| rel.to_owned()))
        .filter(|rel| Path::new(path).join(rel).join(".git").exists())
        .map(|rel| format!("{rel}/"))
        .collect::<Vec<_>>();
    let out = probe
        .ok(
            path,
            &[
                "ls-files",
                "--others",
                "--ignored",
                "--exclude-standard",
                "--directory",
                "-z",
            ],
        )
        .unwrap_or_default();
    let mut lost = out
        .split('\0')
        .filter(|rel| !rel.is_empty())
        .filter(|rel| !same_file(&Path::new(path).join(rel), &Path::new(main).join(rel)))
        .map(str::to_owned)
        .chain(submodules)
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

fn session_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return crate::simplify(canonical);
    }
    let path = real(path);
    // A cauda pode ter sumido; o ancestral existente ainda resolve aliases 8.3 e junctions.
    for ancestor in path.ancestors().skip(1) {
        if let Ok(canonical) = std::fs::canonicalize(ancestor) {
            return crate::simplify(canonical).join(path.strip_prefix(ancestor).unwrap());
        }
    }
    path
}

/// A lista e a exclusão usam a mesma identidade física, inclusive com uma cauda inexistente.
pub(crate) fn session_inside(s: &Session, real_path: &Path) -> bool {
    let root = session_path(real_path);
    if let Some(wt) = s.worktree_path.as_deref().filter(|w| !w.is_empty())
        && same_path(&session_path(Path::new(wt)), &root)
    {
        return true;
    }
    [s.cwd.as_deref(), s.git_cwd.as_deref()]
        .into_iter()
        .flatten()
        .filter(|c| !c.is_empty())
        .any(|c| {
            session_path(Path::new(c))
                .ancestors()
                .any(|p| same_path(p, &root))
        })
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
            && stem.char_indices().all(|(i, c)| {
                if matches!(i, 8 | 13 | 18 | 23) {
                    c == '-'
                } else {
                    c.is_ascii_hexdigit()
                }
            })
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
    if exists && worktree_residual::missing_git(Path::new(path)) {
        let registration = worktree_residual::registration(main, path).ok();
        let inventory = worktree_residual::summary(Path::new(path)).ok();
        let names = sessions.iter().filter(|s| session_inside(s, Path::new(path)))
            .map(|s| s.name.clone()).collect::<Vec<_>>();
        let live = sessions.iter().filter(|s| session_inside(s, Path::new(path)))
            .filter_map(|s| s.jsonl.as_deref()).map(|p| real(Path::new(p))).collect();
        let created_at = repo.created_at(path);
        let size = disk_usage(path, created_at, measure);
        return json!({
            "path":path,"repo":main,"exists":true,"residual":registration.is_some(),
            "branch":registration.as_ref().and_then(|r| r.branch.as_deref()),
            "head":registration.as_ref().and_then(|r| r.head(main)),"main_branch":repo.main_branch,
            "base":null,"merged":false,"degraded":true,"ahead":0,"behind":0,
            "dirty":0,"dirty_files":[],"ignored":[],"commits":[],"last_commit":null,
            "residual_files":inventory.as_ref().map_or(&[][..], |i| i.files.as_slice()),
            "residual_entries":inventory.as_ref().map(|i| i.entries),
            "created_at":created_at,"sessions":names,"closed":closed_count(path,&live,bases),
            "size":size.as_ref().and_then(|s| s.bytes),"size_biggest":size.as_ref().and_then(|s| s.biggest.clone()),
            "size_pending":size.is_none(),"size_error":size.as_ref().is_some_and(|s| s.error),
            "size_partial":size.as_ref().is_some_and(|s| s.partial),
        });
    }
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
    let head_ref = branch
        .as_deref()
        .map_or("HEAD".into(), |b| format!("refs/heads/{b}"));
    let head = probe
        .ok(
            cwd,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{head_ref}^{{commit}}"),
            ],
        )
        .map(|head| head.trim().to_owned());
    probe.failed |= head.is_none();
    let (mut ahead, mut behind, mut ancestor) = (0u64, 0u64, false);
    let mut commits = Vec::new();
    let mut last = Vec::new();
    if let (Some(branch), Some(base), Some(head)) = (&branch, &base, &head)
        && branch != base
    {
        // Nome curto perde para uma tag homônima, e o commit dela pareceria já mesclado.
        let range = format!("{base}...{head}");
        if let Some(out) = probe.ok(cwd, &["rev-list", "--left-right", "--count", &range]) {
            let mut n = out
                .split_whitespace()
                .map(|s| s.parse::<u64>().unwrap_or(0));
            behind = n.next().unwrap_or(0);
            ahead = n.next().unwrap_or(0);
            // Recém-criada aponta pro mesmo commit da base: ancestral trivial, não mesclada.
            ancestor = ahead == 0 && behind > 0;
        }
        commits = probe.log(cwd, &format!("{base}..{head}"), 3);
        if ahead > 0 && !commits.is_empty() {
            // O primeiro do `base..branch` é a ponta da branch.
            last = vec![commits[0].clone()];
        }
    }
    if last.is_empty() && (exists || branch.is_some()) {
        let reference = branch
            .as_deref()
            .map_or("HEAD".into(), |b| format!("refs/heads/{b}"));
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
    // O provedor só complementa a prova local; sua indisponibilidade não desfaz a ancestralidade.
    let equivalence = if !probe.failed && !ancestor && ahead > 0 {
        base.as_deref().zip(head.as_deref()).map(|(base,head)|crate::worktree_merge::equivalent(Path::new(cwd),base,head)).unwrap_or_default()
    } else {crate::worktree_merge::Equivalence::default()};
    let external = if !probe.failed && !ancestor && !equivalence.merged && ahead > 0 {
        match (branch.as_deref(), base.as_deref()) {
            (Some(branch), Some(base)) => {
                crate::worktree_merge::probe(Path::new(cwd), branch, base)
            }
            _ => crate::worktree_merge::Proof::default(),
        }
    } else {
        crate::worktree_merge::Proof::default()
    };
    let merged = ancestor || equivalence.merged || external.merged;
    let ignored = if exists {
        ignored_lost(&mut probe, path, main)
    } else {
        Vec::new()
    };
    let real_path = real(Path::new(path));
    let inside = sessions
        .iter()
        .filter(|s| session_inside(s, &real_path))
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
    if merged && exists {
        let current = probe
            .ok(cwd, &["rev-parse", "--verify", "HEAD^{commit}"])
            .map(|head| head.trim().to_owned());
        let current_branch = git::head_info(Some(path)).0;
        probe.failed |= current != head || current_branch != branch;
    }
    let failed = probe.failed;
    json!({
        "path": path, "repo": main, "exists": exists, "branch": branch, "base": base,
        "main_branch": repo.main_branch, "head": head,
        "merge_method": if ancestor { Some("ancestor") } else if equivalence.merged { Some("equivalent") } else if external.merged { Some("pull_request") } else { None },
        "equivalent_commits":equivalence.commits,
        "merge_problem": external.problem,
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
    let root = if is_dir(path) && !worktree_residual::missing_git(Path::new(path)) {
        repo_root_of(path)
    } else {
        None
    };
    let main = match root {
        Some(root) => main_repo_of(&root),
        None => main_of_missing(path),
    };
    status(path, sessions, &Repo::read(&main), measure, bases)
}

#[derive(Clone, Default)]
pub struct Discovery {
    pub repos: Vec<String>,
    pub partial: bool,
}

type DiscoveryCache = HashMap<Vec<String>, (Instant, Discovery)>;
static DISCOVERY: LazyLock<Mutex<DiscoveryCache>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const DISCOVERY_TTL: Duration = Duration::from_secs(30);
const DISCOVERY_LIMIT: usize = 20_000;
const DISCOVERY_BUDGET: Duration = Duration::from_secs(2);

fn scan_repositories(locations: &[String], roots: &[String]) -> Discovery {
    let start = Instant::now();
    let mut scan = Discovery::default();
    let mut pending = locations
        .iter()
        .map(|p| real(Path::new(p)))
        .collect::<VecDeque<_>>();
    let mut visited = HashSet::new();
    let mut repos = HashSet::new();
    let mut inspected = 0;
    'scan: while let Some(path) = pending.pop_front() {
        if !visited.insert(path.clone()) {
            continue;
        }
        inspected += 1;
        if inspected > DISCOVERY_LIMIT || start.elapsed() > DISCOVERY_BUDGET {
            scan.partial = true;
            break;
        }
        if path.join(".git").exists() {
            let main = text(&real(Path::new(&main_repo_of(&text(&path)))));
            if roots.iter().any(|root| within(Path::new(&main), root)) {
                repos.insert(main);
            }
        }
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => {
                scan.partial = true;
                continue;
            }
        };
        for entry in entries {
            if start.elapsed() > DISCOVERY_BUDGET {
                scan.partial = true;
                break 'scan;
            }
            let Ok(entry) = entry else {
                scan.partial = true;
                continue;
            };
            let Ok(kind) = entry.file_type() else {
                scan.partial = true;
                continue;
            };
            if kind.is_symlink() || !kind.is_dir() {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                scan.partial = true;
                continue;
            };
            let link = false;
            #[cfg(windows)]
            let link = {
                use std::os::windows::fs::MetadataExt;
                link || meta.file_attributes() & 0x400 != 0
            };
            if link || !meta.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child = entry.path();
            if name == ".git" {
                continue;
            }
            // Um repositório com nome de artefato continua sendo um repositório.
            if matches!(
                name.as_ref(),
                "node_modules" | "target" | ".venv" | "venv" | "__pycache__" | ".cache"
            ) && !child.join(".git").exists()
            {
                continue;
            }
            // O limite é de pastas: arquivos de um build não disputam a descoberta dos repos.
            if visited.len() + pending.len() >= DISCOVERY_LIMIT {
                scan.partial = true;
                continue;
            }
            pending.push_back(child);
        }
    }
    scan.repos = repos.into_iter().collect();
    scan.repos.sort();
    scan
}

/// A pasta principal de cada pasta de trabalho, pelo caminho real e sem repetir: o mesmo repo por um symlink
/// apareceria duas vezes.
pub fn main_repos<'a>(dirs: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut mains = dirs.into_iter().filter_map(repo_root_of)
        .map(|root| text(&real(Path::new(&main_repo_of(&root))))).collect::<Vec<_>>();
    mains.sort();
    mains.dedup();
    mains
}

/// Repositórios autorizados, inclusive quando já não há conversa recente deles.
pub fn discover_repositories(roots: &[String]) -> Discovery {
    let mut locations = roots.to_vec();
    if let Some(home) = crate::home()
        && let Ok(settings) = crate::worktree_settings::load_default()
        && let Ok(location) = settings.location_path(&home)
        && location.is_dir()
        && roots.iter().any(|root| within(&location, root))
    {
        locations.push(text(&location));
    }
    locations.sort();
    locations.dedup();
    let mut key = roots.to_vec();
    key.sort();
    key.push(String::new());
    key.extend(locations.iter().cloned());
    let mut cache = DISCOVERY.lock().unwrap_or_else(|error| error.into_inner());
    if let Some((at, scan)) = cache.get(&key)
        && at.elapsed() < DISCOVERY_TTL
    {
        return scan.clone();
    }
    let scan = scan_repositories(&locations, roots);
    if cache.len() >= 16 {
        cache.clear();
    }
    cache.insert(key, (Instant::now(), scan.clone()));
    scan
}

/// Ausência de repositórios não esconde uma varredura que ficou incompleta.
pub fn discovery_problem(roots: &[String]) -> Option<&'static str> {
    discover_repositories(roots)
        .partial
        .then_some("worktree_discovery_incomplete")
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
    let discovery = if repo.is_none() {
        roots.map(discover_repositories).unwrap_or_default()
    } else {
        Discovery::default()
    };
    let mut mains = main_repos(wanted);
    mains.extend(discovery.repos);
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
    let mut results = results
        .into_inner()
        .unwrap_or_else(|e| e.into_inner())
        .into_iter();
    Value::Array(
        repos
            .iter()
            .map(|r| {
                let worktrees = results.by_ref().take(r.admins.len()).collect::<Vec<_>>();
                json!({"repo": r.main, "worktrees": worktrees, "discovery_partial": discovery.partial})
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
    if worktree_residual::missing_git(Path::new(path)) {
        return residual_main(path, roots).is_some();
    }
    let main = if is_dir(path) {
        let Some(root) = repo_root_of(path) else {
            return false;
        };
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

pub fn allowed_fetch_repo(path: &str, roots: &[String]) -> Result<String> {
    if worktree_residual::missing_git(Path::new(path))
        && let Err(error) = allowed_scan_root(path, roots)
    {
        return Err(error);
    }
    allowed_repo(path, roots)
}

pub fn allowed_worktree(path: &str, roots: &[String]) -> Result<String> {
    let path = allowed_repo(path, roots)?;
    // Pasta que existe mas não é raiz de repo/worktree daria uma situação inventada.
    if is_dir(&path) && !Path::new(&path).join(".git").exists()
        && residual_main(&path, roots).is_none() {
        return Err(error(404, "não é um repositório git"));
    }
    Ok(path)
}

fn residual_main(path: &str, roots: &[String]) -> Option<String> {
    let candidate = main_of_missing(path);
    let valid = |main: &str| allowed_scan_root(main, roots).is_ok()
        && worktree_residual::registration(main, path).is_ok();
    if valid(&candidate) { return Some(candidate); }
    discover_repositories(roots).repos.into_iter().find(|main| valid(main))
}

pub fn status_allowed(path: &str, roots: &[String], sessions: &[Session], measure: bool, bases: &[String]) -> Result<Value> {
    let path = allowed_worktree(path, roots)?;
    if worktree_residual::missing_git(Path::new(&path))
        && let Some(main) = residual_main(&path, roots) {
        return Ok(status(&path, sessions, &Repo::read(&main), measure, bases));
    }
    Ok(status_of(&path, sessions, measure, bases))
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
            let size = Size {
                bytes: None,
                biggest: None,
                error: true,
                partial: false,
            };
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
        tracing::warn!(
            skipped,
            "worktrees: pastas ilegíveis ao medir; o total é parcial"
        );
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
        return entry
            .metadata()
            .map(|m| entry_bytes(&m))
            .unwrap_or_else(|_| {
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
    fn discovery_finds_repositories_without_recent_conversations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("projects");
        let main = root.join("group/repository");
        let linked = dir.path().join("old-worktree");
        std::fs::create_dir_all(&main).unwrap();
        let empty = dir.path().join("empty-gitconfig");
        std::fs::write(&empty, "").unwrap();
        let run = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(args)
                .env("GIT_CONFIG_GLOBAL", &empty)
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", dir.path())
                .env("GIT_AUTHOR_NAME", "Teste")
                .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
                .env("GIT_COMMITTER_NAME", "Teste")
                .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "core.hooksPath", ""]);
        run(&["config", "commit.gpgSign", "false"]);
        run(&["commit", "--allow-empty", "-m", "Base"]);
        run(&[
            "worktree",
            "add",
            "-b",
            "old-feature",
            linked.to_str().unwrap(),
            "main",
        ]);
        let rows = list_all(&[], &[], Some(&[text(&root)]), None, false, &[]);
        let repos = rows.as_array().unwrap();
        assert_eq!(
            repos.len(),
            1,
            "Repositório antigo deve aparecer sem conversa recente"
        );
        assert_eq!(repos[0]["repo"], text(&real(&main)));
        let worktrees = repos[0]["worktrees"].as_array().unwrap();
        assert_eq!(
            worktrees.len(),
            1,
            "Checkout principal deve ficar fora das worktrees"
        );
        assert_eq!(
            std::fs::canonicalize(worktrees[0]["path"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(&linked).unwrap()
        );
        let head = worktrees[0]["head"]
            .as_str()
            .expect("SHA completo para revalidar a exclusão");
        assert_eq!(
            head.len(),
            40,
            "Identidade do commit não pode levar quebra de linha"
        );
    }

    #[test]
    fn discovery_large_asset_directory_keeps_nested_repositories_and_artifact_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("projects");
        let parent = root.join("project");
        let assets = parent.join("apps/web/dist/assets");
        let nested = parent.join("tools/target");
        std::fs::create_dir_all(&assets).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        for repo in [&parent, &nested] {
            let out = std::process::Command::new("git")
                .args(["init", "-b", "main"])
                .arg(repo)
                .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env("HOME", dir.path())
                .output().unwrap();
            assert!(out.status.success());
        }
        for i in 0..20_001 {
            std::fs::write(assets.join(format!("asset-{i}.js")), "").unwrap();
        }
        let scan = discover_repositories(&[text(&root), text(&parent)]);
        assert!(!scan.partial, "arquivos comuns não podem esgotar a descoberta de pastas");
        let mut expected = vec![text(&real(&parent)), text(&real(&nested))];
        expected.sort();
        assert_eq!(scan.repos, expected);
    }

    #[test]
    fn discovery_preserves_real_repositories_named_like_build_artifacts_and_reports_incomplete_reads()
     {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("projects");
        let repo = root.join("target");
        std::fs::create_dir_all(&repo).unwrap();
        let config = dir.path().join("gitconfig");
        std::fs::write(&config, "").unwrap();
        let out = std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .arg(&repo)
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", dir.path())
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            discover_repositories(&[text(&root)]).repos,
            vec![text(&real(&repo))]
        );
        let invalid_root = dir.path().join("not-a-directory");
        std::fs::write(&invalid_root, "file").unwrap();
        assert_eq!(
            discovery_problem(&[text(&invalid_root)]),
            Some("worktree_discovery_incomplete")
        );
    }

    #[cfg(unix)]
    #[test]
    fn discovery_does_not_follow_a_link_outside_the_authorized_roots() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("projects");
        let external = dir.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir_all(external.join(".git")).unwrap();
        std::os::unix::fs::symlink(&external, root.join("linked")).unwrap();
        let discovery = discover_repositories(&[text(&root)]);
        assert!(discovery.repos.is_empty());
        assert!(!discovery.partial);
    }

    fn t4_isolated(name: &str) -> bool {
        if std::env::var("HANGAR_T4_ISOLATED").as_deref() == Ok(name) { return false; }
        let home = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("worktrees::tests::{name}")])
            .env("HANGAR_T4_ISOLATED", name).env("HOME", home.path()).env("USERPROFILE", home.path())
            .env("XDG_CONFIG_HOME", home.path().join("config")).env("XDG_CACHE_HOME", home.path().join("cache"))
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" }).env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE").output().unwrap();
        assert!(output.status.success(), "{name}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        true
    }

    fn fixture_git(cwd: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git").arg("-C").arg(cwd)
            .args(["-c", "core.hooksPath=", "-c", "commit.gpgSign=false", "-c", "user.name=Teste", "-c", "user.email=teste@example.invalid"])
            .args(args).env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_TERMINAL_PROMPT", "0").output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    fn published_fixture(root: &Path, source: &str, remote: &str) -> (PathBuf, PathBuf) {
        let repo = root.join("repo");
        let feature = root.join("feature");
        std::fs::create_dir(&repo).unwrap();
        fixture_git(&repo, &["init", "-b", "main"]);
        fixture_git(&repo, &["commit", "--allow-empty", "-m", "Inicial"]);
        let origin = root.join("origin.git");
        fixture_git(root, &["init", "--bare", "-b", "main", origin.to_str().unwrap()]);
        fixture_git(&repo, &["remote", "add", remote, origin.to_str().unwrap()]);
        fixture_git(&repo, &["push", "-u", remote, "main"]);
        fixture_git(&repo, &["branch", "server"]);
        fixture_git(&repo, &["push", remote, "server"]);
        let base = if source == "remote-main" { "main" } else { "server" };
        let start = if source == "local-server" { "server".to_owned() } else { format!("{remote}/{base}") };
        fixture_git(&repo, &["worktree", "add", "--no-track", "-b", "feature", feature.to_str().unwrap(), &start]);
        std::fs::write(feature.join("feature.txt"), "Implementação\n").unwrap();
        fixture_git(&feature, &["add", "feature.txt"]);
        fixture_git(&feature, &["commit", "-m", "Implementação"]);
        let integration = root.join("integration");
        fixture_git(&repo, &["worktree", "add", "--detach", integration.to_str().unwrap(), &format!("{remote}/{base}")]);
        fixture_git(&integration, &["merge", "--no-ff", "-m", "Integração", "feature"]);
        fixture_git(&integration, &["push", remote, &format!("HEAD:{base}")]);
        (repo, feature)
    }

    fn creation_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = real(dir.path());
        let repo = root.join("repo");
        std::fs::create_dir(&repo).unwrap();
        fixture_git(&repo, &["init", "-b", "main"]);
        fixture_git(&repo, &["config", "core.hooksPath", ""]);
        std::fs::write(repo.join("tracked.txt"), "linha original\n").unwrap();
        fixture_git(&repo, &["add", "tracked.txt"]);
        fixture_git(&repo, &["commit", "-m", "Inicial"]);
        fixture_git(&repo, &["branch", "feature"]);
        let location = root.join("configured");
        (dir, repo, location)
    }

    #[test]
    fn t4_core_creation_preserves_dirty_source_and_rejects_occupied_invalid_and_foreign_targets() {
        if t4_isolated("t4_core_creation_preserves_dirty_source_and_rejects_occupied_invalid_and_foreign_targets") { return; }
        let (dir, repo, location) = creation_fixture();
        let root = real(dir.path());
        std::fs::write(repo.join("untracked.txt"), "keep").unwrap();
        let same = git::create_worktree_at(&repo, "main", "same", &root, false, None, &location).unwrap();
        assert_eq!(same, json!([repo, false]));
        let made = git::create_worktree_at(&repo, "feature", "chat", &root, false, None, &location).unwrap();
        let path = location.join("repo").join("chat");
        assert_eq!(made, json!([path, true]));
        assert_eq!(fixture_git(&path, &["branch", "--show-current"]), "feature");
        assert_eq!(std::fs::read_to_string(repo.join("untracked.txt")).unwrap(), "keep");
        git::remove_worktree(&repo, path.to_str().unwrap(), false).unwrap();
        assert!(!path.exists());
        std::fs::create_dir(&path).unwrap();
        let occupied = git::create_worktree_at(&repo, "feature", "chat", &root, false, None, &location).unwrap_err();
        assert_eq!((occupied.status, occupied.detail), (409, json!("destino da worktree já existe")));
        let foreign = root.join("not-authorized");
        std::fs::create_dir(&foreign).unwrap();
        assert_eq!(git::create_worktree_at(&repo, "feature", "other", &foreign, false, None, &location).unwrap_err().status, 400);
        for branch in ["nao-existe", "--detach"] {
            assert_eq!(git::create_worktree_at(&repo, branch, "x", &root, false, None, &location).unwrap_err().status, 400);
            assert!(!location.join("repo/x").exists());
        }
        for branch in ["-x", "com espaco", "a..b", "", "nova "] {
            assert_eq!(git::create_worktree_at(&repo, branch, "invalid", &root, true, None, &location).unwrap_err().status, 400);
            assert!(!location.join("repo/invalid").exists());
        }
        assert_eq!(git::create_worktree_at(&repo, "feature", "existing", &root, true, None, &location).unwrap_err().status, 409);
        assert_eq!(git::create_worktree_at(&repo, "nova", "missing-base", &root, true, Some("ausente"), &location).unwrap_err().status, 400);
        assert_eq!(fixture_git(&repo, &["branch", "--show-current"]), "main");
    }

    #[test]
    fn t4_core_creation_serializes_same_branch_without_duplicate_registration() {
        if t4_isolated("t4_core_creation_serializes_same_branch_without_duplicate_registration") { return; }
        let (dir, repo, location) = creation_fixture();
        let root = real(dir.path());
        let barrier = std::sync::Barrier::new(2);
        let results = std::thread::scope(|scope| {
            let attempts = ["a", "b"].map(|name| {
                let (repo, root, location, barrier) = (&repo, &root, &location, &barrier);
                scope.spawn(move || {
                    barrier.wait();
                    git::create_worktree_at(repo, "feature", name, root, false, None, location)
                })
            });
            attempts.map(|thread| thread.join().unwrap())
        });
        assert_eq!(results.iter().filter(|result| result.as_ref().is_ok_and(|value| value[1] == true)).count(), 1);
        assert_eq!(results.iter().filter(|result| result.as_ref().is_err_and(|error| error.status == 409)).count(), 1);
        assert_eq!(fixture_git(&repo, &["worktree", "list", "--porcelain"]).matches("branch refs/heads/feature").count(), 1);
    }

    #[test]
    fn t4_core_creation_records_base_copies_only_small_ignored_files_and_preserves_versioned_content() {
        if t4_isolated("t4_core_creation_records_base_copies_only_small_ignored_files_and_preserves_versioned_content") { return; }
        let (dir, repo, location) = creation_fixture();
        let root = real(dir.path());
        std::fs::write(repo.join(".gitignore"), ".env\nlocal.ini\nbig.bin\ncache/\n").unwrap();
        std::fs::write(repo.join("local.ini"), "versioned").unwrap();
        fixture_git(&repo, &["add", ".gitignore"]);
        fixture_git(&repo, &["add", "-f", "local.ini"]);
        fixture_git(&repo, &["commit", "-m", "ignore"]);
        fixture_git(&repo, &["branch", "-f", "feature"]);
        fixture_git(&repo, &["rm", "--cached", "local.ini"]);
        fixture_git(&repo, &["commit", "-m", "untrack"]);
        std::fs::write(repo.join("local.ini"), "main-local").unwrap();
        std::fs::write(repo.join(".env"), "SECRET=1").unwrap();
        std::fs::write(repo.join("big.bin"), vec![b'x';1024 * 1024 + 1]).unwrap();
        std::fs::create_dir(repo.join("cache")).unwrap();
        std::fs::write(repo.join("cache/a"), "a").unwrap();
        let first = git::create_worktree_at(&repo, "feature", "a", &root, false, None, &location).unwrap();
        let first = Path::new(first[0].as_str().unwrap());
        std::fs::write(first.join(".env"), "FROM-WORKTREE").unwrap();
        let next = git::create_worktree_at(first, "nova", "b", &root, true, Some("feature"), &location).unwrap();
        let next = Path::new(next[0].as_str().unwrap());
        assert_eq!(next, location.join("repo/b"));
        assert_eq!(fixture_git(next, &["branch", "--show-current"]), "nova");
        assert_eq!(fixture_git(next, &["config", "--get", "branch.nova.hangar-base"]), "feature");
        assert_eq!(std::fs::read_to_string(next.join(".env")).unwrap(), "SECRET=1");
        assert_eq!(std::fs::read_to_string(next.join("local.ini")).unwrap(), "versioned");
        assert!(!next.join("big.bin").exists());
        assert!(!next.join("cache").exists());
        let default = git::create_worktree_at(&repo, "default-base", "default", &root, true, None, &location).unwrap();
        assert_eq!(fixture_git(Path::new(default[0].as_str().unwrap()), &["config", "--get", "branch.default-base.hangar-base"]), "main");
    }

    #[test]
    fn t4_core_creation_from_remote_base_does_not_track_it() {
        if t4_isolated("t4_core_creation_from_remote_base_does_not_track_it") { return; }
        let (dir, repo, location) = creation_fixture();
        let root = real(dir.path());
        let remote = root.join("upstream.git");
        fixture_git(&root, &["clone", "--bare", repo.to_str().unwrap(), remote.to_str().unwrap()]);
        fixture_git(&remote, &["branch", "only-remote", "main"]);
        fixture_git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
        fixture_git(&repo, &["fetch", "origin"]);
        let made = git::create_worktree_at(&repo, "nova", "new", &root, true, Some("only-remote"), &location).unwrap();
        let path = Path::new(made[0].as_str().unwrap());
        assert_eq!(fixture_git(path, &["config", "--get", "branch.nova.hangar-base"]), "only-remote");
        assert_eq!(git::command(path, &["config", "--get", "branch.nova.remote"]).unwrap().code, 1);
        let tracked = git::create_worktree_at(&repo, "only-remote", "remote", &root, false, None, &location).unwrap();
        assert_eq!(fixture_git(Path::new(tracked[0].as_str().unwrap()), &["branch", "--show-current"]), "only-remote");
    }

    #[test]
    fn t4_core_low_level_remove_and_disk_diff_preserve_their_contracts() {
        if t4_isolated("t4_core_low_level_remove_and_disk_diff_preserve_their_contracts") { return; }
        for force in [false, true] {
            let (dir, repo, location) = creation_fixture();
            let root = real(dir.path());
            let made = git::create_worktree_at(&repo, "feature", "dirty", &root, false, None, &location).unwrap();
            let path = Path::new(made[0].as_str().unwrap());
            std::fs::write(path.join("sujo.txt"), "não versionado\n").unwrap();
            let result = git::remove_worktree(&repo, path.to_str().unwrap(), force);
            assert_eq!(result.is_ok(), force);
            if let Err(error) = result { assert_eq!(error.status, 500); }
            assert_eq!(path.exists(), !force);
            let head = fixture_git(&repo, &["rev-parse", "HEAD"]);
            std::fs::write(repo.join("tracked.txt"), "linha original\nlinha nova nao commitada\n").unwrap();
            let diff = git::diff_worktree(&repo, &head).unwrap();
            assert!(diff["diff"].as_str().unwrap().contains("+linha nova nao commitada"));
            assert_ne!(git::commit_diff(&repo, &head).unwrap()["diff"], diff["diff"]);
        }
    }

    #[cfg(unix)]
    #[test]
    fn t4_core_reflog_read_failure_cannot_prove_merge() {
        if std::env::var_os("HANGAR_T4_REFLOG_FAILURE").is_none() {
            let dir = tempfile::tempdir().unwrap();
            let bin = dir.path().join("bin");
            let home = dir.path().join("home");
            std::fs::create_dir(&bin).unwrap();
            std::fs::create_dir(&home).unwrap();
            let git = std::env::split_paths(&std::env::var_os("PATH").unwrap()).map(|path| path.join("git"))
                .find(|path| path.is_file()).unwrap().canonicalize().unwrap();
            let wrapper = bin.join("git");
            crate::write_test_executable(&wrapper, format!("#!/bin/sh\nfor arg do\n  if [ \"$arg\" = reflog ]; then printf '%s\\n' 'Falha ao ler o reflog' >&2; exit 128; fi\ndone\nexec '{}' \"$@\"\n", git.to_string_lossy().replace('\'', "'\\''")));
            let child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "worktrees::tests::t4_core_reflog_read_failure_cannot_prove_merge"])
                .env("HANGAR_T4_REFLOG_FAILURE", "1")
                .env("PATH", std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap())
                .env("HOME", &home).env("USERPROFILE", &home)
                .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
            assert!(child.status.success(), "{}{}", String::from_utf8_lossy(&child.stdout), String::from_utf8_lossy(&child.stderr));
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let (_repo, feature) = published_fixture(dir.path(), "remote-main", "origin");
        assert_eq!(git::command(&feature, &["reflog", "show", "--format=%gs", "refs/heads/feature"]).unwrap().code, 128);
        let status = status_of(feature.to_str().unwrap(), &[], false, &[]);
        assert_eq!(status["base"], "origin/main");
        assert_eq!(status["merged"], false);
        assert_eq!(status["degraded"], true);
    }

    #[cfg(unix)]
    fn t4_timeout_process(name: &str, command: &str) -> bool {
        if std::env::var("HANGAR_T4_TIMEOUT_TEST").as_deref() == Ok(name) { return false; }
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let home = dir.path().join("home");
        std::fs::create_dir(&bin).unwrap();
        std::fs::create_dir(&home).unwrap();
        let marker = dir.path().join("armed");
        let fifo = dir.path().join("command-fifo");
        let path = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(path.as_ptr(), 0o600) }, 0);
        let git = std::env::split_paths(&std::env::var_os("PATH").unwrap()).map(|path| path.join("git"))
            .find(|path| path.is_file()).unwrap().canonicalize().unwrap();
        let wrapper = bin.join("git");
        crate::write_test_executable(&wrapper, format!("#!/bin/sh\nif [ -e '{}' ]; then\n  for arg do\n    if [ \"$arg\" = '{command}' ]; then IFS= read -r line < '{}'; exit 0; fi\n  done\nfi\nexec '{}' \"$@\"\n", marker.display(), fifo.display(), git.to_string_lossy().replace('\'', "'\\''")));
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &format!("worktrees::tests::{name}")])
            .env("HANGAR_T4_TIMEOUT_TEST", name).env("HANGAR_T4_TIMEOUT_MARKER", marker)
            .env("PATH", std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap())
            .env("HOME", &home).env("USERPROFILE", &home).env("XDG_CONFIG_HOME", home.join("config"))
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE").output().unwrap();
        assert!(output.status.success(), "{name}: {}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        true
    }

    #[cfg(unix)]
    #[test]
    fn t4_remaining_status_timeout_504_degrades_list_without_hiding_local_work() {
        if t4_timeout_process("t4_remaining_status_timeout_504_degrades_list_without_hiding_local_work", "status") { return; }
        let (dir, repo, location) = creation_fixture();
        let root = real(dir.path());
        let made = git::create_worktree_at(&repo, "feature", "status-timeout", &root, false, None, &location).unwrap();
        let path = Path::new(made[0].as_str().unwrap());
        std::fs::write(path.join("solto.txt"), "?").unwrap();
        std::fs::write(std::env::var_os("HANGAR_T4_TIMEOUT_MARKER").unwrap(), "armed").unwrap();
        let failure = git::command(path, &["status", "--porcelain"]).err().expect("o comando bloqueado deveria vencer o prazo");
        assert_eq!(failure.status, 504);
        assert_eq!(failure.detail, "git timeout");
        let rows = list_all(&[text(&repo)], &[], Some(&[text(&root)]), None, false, &[]);
        let status = &rows[0]["worktrees"][0];
        assert_eq!(status["dirty"], 0);
        assert_eq!(status["degraded"], true);
        assert_eq!(status["merged"], false);
        assert_eq!(std::fs::read_to_string(path.join("solto.txt")).unwrap(), "?");
    }

    #[cfg(unix)]
    #[test]
    fn t4_remaining_ancestry_timeout_504_revokes_merge_and_prevents_removal() {
        if t4_timeout_process("t4_remaining_ancestry_timeout_504_revokes_merge_and_prevents_removal", "rev-list") { return; }
        let dir = tempfile::tempdir().unwrap();
        let (repo, feature) = published_fixture(dir.path(), "remote-main", "origin");
        let path = feature.to_str().unwrap();
        let healthy = status_of(path, &[], false, &[]);
        assert_eq!(healthy["merged"], true);
        assert_eq!(healthy["degraded"], false);
        std::fs::write(std::env::var_os("HANGAR_T4_TIMEOUT_MARKER").unwrap(), "armed").unwrap();
        let failure = git::command(&feature, &["rev-list", "--left-right", "--count", "origin/main...refs/heads/feature"]).err().expect("a prova bloqueada deveria vencer o prazo");
        assert_eq!(failure.status, 504);
        assert_eq!(failure.detail, "git timeout");
        let status = status_of(path, &[], false, &[]);
        assert_eq!(status["merged"], false);
        assert_eq!(status["degraded"], true);
        let removed = crate::worktree_cleanup::remove(repo.to_str().unwrap(), path, &[], &[], dir.path(),
            crate::worktree_cleanup::RemoveOptions { merged_only:true, ..Default::default() });
        assert_eq!(removed.unwrap_err().status, 409);
        assert!(feature.join("feature.txt").exists());
    }

    #[test]
    fn t4_remaining_deleted_work_branch_upstream_cannot_prove_merge() {
        if t4_isolated("t4_remaining_deleted_work_branch_upstream_cannot_prove_merge") { return; }
        let dir = tempfile::tempdir().unwrap();
        let root = real(dir.path());
        let remote = root.join("remote");
        std::fs::create_dir(&remote).unwrap();
        fixture_git(&remote, &["init", "-b", "main"]);
        fixture_git(&remote, &["commit", "--allow-empty", "-m", "Inicial"]);
        fixture_git(&remote, &["branch", "x"]);
        let repo = root.join("clone");
        fixture_git(&root, &["clone", remote.to_str().unwrap(), repo.to_str().unwrap()]);
        fixture_git(&repo, &["switch", "x"]);
        std::fs::write(repo.join("a.txt"), "Não integrada\n").unwrap();
        fixture_git(&repo, &["add", "a.txt"]);
        fixture_git(&repo, &["commit", "-m", "Não integrada"]);
        fixture_git(&repo, &["switch", "main"]);
        let x = root.join("repo-x");
        fixture_git(&repo, &["worktree", "add", x.to_str().unwrap(), "x"]);
        assert_eq!(status_of(x.to_str().unwrap(), &[], false, &[])["merged"], false);
        fixture_git(&remote, &["branch", "-D", "x"]);
        fixture_git(&repo, &["fetch", "--prune"]);
        assert_ne!(git::command(&repo, &["show-ref", "--verify", "refs/remotes/origin/x"]).unwrap().code, 0);
        assert_eq!(fixture_git(&repo, &["for-each-ref", "--format=%(upstream)", "refs/heads/x"]), "refs/remotes/origin/x");
        let status = status_of(x.to_str().unwrap(), &[], false, &[]);
        assert_eq!(status["merged"], false);
        assert_eq!(status["ahead"], 1);
        assert_eq!(std::fs::read_to_string(x.join("a.txt")).unwrap(), "Não integrada\n");
    }

    #[test]
    fn t4_status_list_detail_sessions_and_closed_conversations_share_identity() {
        if t4_isolated("t4_status_list_detail_sessions_and_closed_conversations_share_identity") { return; }
        let (dir, repo, location) = creation_fixture();
        let root = real(dir.path());
        std::fs::write(repo.join(".gitignore"), ".env\nnotas.txt\nnode_modules/\n").unwrap();
        fixture_git(&repo, &["add", ".gitignore"]);
        fixture_git(&repo, &["commit", "-m", "ignore"]);
        fixture_git(&repo, &["branch", "-f", "feature"]);
        std::fs::write(repo.join(".env"), "S=1").unwrap();
        let made = git::create_worktree_at(&repo, "feature", "status", &root, false, None, &location).unwrap();
        let path = made[0].as_str().unwrap();
        fixture_git(Path::new(path), &["config", "branch.feature.hangar-base", "main"]);
        assert_eq!(status_of(path, &[], false, &[])["merged"], false);
        std::fs::write(Path::new(path).join("a.txt"), "a").unwrap();
        fixture_git(Path::new(path), &["add", "a.txt"]);
        fixture_git(Path::new(path), &["commit", "-m", "a.txt"]);
        std::fs::write(Path::new(path).join("solto.txt"), "?").unwrap();
        std::fs::write(Path::new(path).join("notas.txt"), "minha").unwrap();
        let projects = root.join("projects");
        let project = projects.join(sanitize_cwd(path));
        std::fs::create_dir_all(&project).unwrap();
        let closed = project.join("d0000001-0000-4000-8000-000000000000.jsonl");
        let live = project.join("d0000003-0000-4000-8000-000000000000.jsonl");
        std::fs::write(&closed, "{}\n").unwrap();
        std::fs::write(&live, "{}\n").unwrap();
        std::fs::write(project.join("d0000003-0000-4000-8000-000000000000.from-account.jsonl"), "{}\n").unwrap();
        let sessions = [Session { name:"s1".into(), cwd:Some(text(&repo)), worktree_path:Some(path.into()), jsonl:Some(text(&live)), ..Default::default() }];
        let bases = [text(&projects)];
        let status = status_of(path, &sessions, false, &bases);
        for (key, expected) in [("branch",json!("feature")),("base",json!("main")),("ahead",json!(1)),
            ("merged",json!(false)),("dirty",json!(1)),("ignored",json!(["notas.txt"])),
            ("sessions",json!(["s1"])),("closed",json!(1)),("exists",json!(true)),("repo",json!(repo))] {
            assert_eq!(status[key], expected, "{key}: {status}");
        }
        fixture_git(&repo, &["switch", "-c", "dev"]);
        let status = status_of(path, &sessions, false, &bases);
        assert_eq!(status["base"], "main");
        assert_eq!(status["main_branch"], "dev");
        let roots = [text(&root)];
        let rows = list_all(&[text(&repo),path.into()], &sessions, Some(&roots), None, false, &bases);
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["worktrees"].as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["worktrees"][0]["closed"], 1);
        assert_eq!(list_all(&[], &sessions, Some(&roots), Some(path), false, &bases)[0]["repo"], json!(repo));
        let foreign = root.join("other-root");
        std::fs::create_dir(&foreign).unwrap();
        assert_eq!(list_all(&[path.into()], &[], Some(&[text(&foreign)]), None, false, &[]), json!([]));
        assert_eq!(status_allowed(path, &[text(&foreign)], &[], false, &[]).unwrap_err().status, 403);
        assert_eq!(status_allowed(foreign.to_str().unwrap(), &roots, &[], false, &[]).unwrap_err().status, 404);
    }

    #[test]
    fn t4_status_relative_pointers_keep_main_and_registration_identity() {
        if t4_isolated("t4_status_relative_pointers_keep_main_and_registration_identity") { return; }
        let (dir, repo, _location) = creation_fixture();
        let wt = real(dir.path()).join("repo-x");
        fixture_git(&repo, &["worktree", "add", wt.to_str().unwrap(), "feature"]);
        std::fs::create_dir(wt.join("sub")).unwrap();
        assert_eq!(repo_root_of(wt.join("sub").to_str().unwrap()).as_deref(), wt.to_str());
        std::fs::write(wt.join(".git"), "gitdir: ../repo/.git/worktrees/repo-x\n").unwrap();
        std::fs::write(repo.join(".git/worktrees/repo-x/gitdir"), "../../../../repo-x/.git\n").unwrap();
        assert_eq!(main_repo_of(wt.to_str().unwrap()), text(&repo));
        assert_eq!(main_repo_of(repo.to_str().unwrap()), text(&repo));
        assert_eq!(worktree_paths(repo.to_str().unwrap()), [text(&wt)]);
    }

    #[test]
    fn t4_status_missing_nested_registration_and_equal_tips_are_protected() {
        if t4_isolated("t4_status_missing_nested_registration_and_equal_tips_are_protected") { return; }
        let (dir, repo, _location) = creation_fixture();
        let wt = repo.join(".claude").join("worktrees").join("nested");
        fixture_git(&repo, &["worktree", "add", wt.to_str().unwrap(), "feature"]);
        let path = wt.to_str().unwrap();
        let fresh = status_of(path, &[], false, &[]);
        assert_eq!(fresh["merged"], false);
        assert_eq!((fresh["ahead"].as_u64(), fresh["behind"].as_u64()), (Some(0), Some(0)));
        fixture_git(&wt, &["commit", "--allow-empty", "-m", "feature"]);
        assert_eq!(status_of(path, &[], false, &[])["merged"], false);
        fixture_git(&repo, &["merge", "--no-ff", "-m", "Integração", "feature"]);
        assert_eq!(status_of(path, &[], false, &[])["merged"], true);
        std::fs::remove_dir_all(&wt).unwrap();
        let missing = status_of(path, &[], false, &[]);
        assert_eq!(missing["exists"], false);
        assert_eq!(missing["repo"], json!(repo));
        assert_eq!(missing["branch"], "feature");
        let roots = [text(&real(dir.path()))];
        let listed = list_all(&[text(&repo)], &[], Some(&roots), None, false, &[]);
        assert_eq!(listed[0]["worktrees"][0]["repo"], json!(repo));
        assert_eq!(listed[0]["worktrees"][0]["branch"], "feature");
    }

    #[cfg(unix)]
    #[test]
    fn t4_status_aliases_deduplicate_repositories_and_match_live_sessions() {
        if t4_isolated("t4_status_aliases_deduplicate_repositories_and_match_live_sessions") { return; }
        let (dir, repo, location) = creation_fixture();
        let root = real(dir.path());
        let made = git::create_worktree_at(&repo, "feature", "alias", &root, false, None, &location).unwrap();
        let path = made[0].as_str().unwrap();
        let link = root.join("link");
        std::os::unix::fs::symlink(&root, &link).unwrap();
        let alias = link.join(Path::new(path).strip_prefix(&root).unwrap());
        let session = Session { name:"s1".into(), cwd:Some(text(&alias)), ..Default::default() };
        assert_eq!(status_of(path, &[session], false, &[])["sessions"], json!(["s1"]));
        let rows = list_all(&[text(&repo),text(&link.join("repo")),path.into()], &[], Some(&[text(&root)]), None, false, &[]);
        assert_eq!(rows.as_array().unwrap().len(), 1);
    }

    #[test]
    fn t4_seventeen_creation_bases_use_published_history() {
        if t4_isolated("t4_seventeen_creation_bases_use_published_history") { return; }
        // Esperados literais de test_worktree_merge_base.py, sem eleger a base pelo candidato.
        let cases = [
            ("remote-main", None, "", "origin", "origin/main", true, false),
            ("remote-server", None, "", "origin", "origin/server", true, false),
            ("local-server", None, "", "origin", "origin/server", true, false),
            ("remote-server", Some("server"), "", "origin", "origin/server", true, false),
            ("remote-server", Some("origin/server"), "", "origin", "origin/server", true, false),
            ("remote-server", Some("main"), "", "origin", "origin/main", false, false),
            ("remote-server", None, "expire", "origin", "origin/main", false, false),
            ("remote-server", None, "switch", "origin", "origin/server", true, false),
            ("remote-server", None, "new-commit", "origin", "origin/server", false, false),
            ("remote-server", None, "missing", "origin", "origin/server", false, true),
            ("remote-server", Some("origin/missing"), "", "origin", "origin/missing", false, true),
            ("local-server", Some("server"), "", "team", "team/server", true, false),
            ("remote-server", None, "tag", "origin", "refs/remotes/origin/server", true, false),
            ("remote-server", Some("main"), "tag-main", "origin", "refs/remotes/origin/main", false, false),
            ("local-server", None, "ambiguous", "origin", "server", false, true),
            ("local-server", None, "upstream", "origin", "origin/server", true, false),
            ("remote-server", None, "track", "origin", "origin/server", true, false),
        ];
        assert_eq!(cases.len(), 17);
        for (index, (source, configured, action, remote, base, merged, degraded)) in cases.into_iter().enumerate() {
            let dir = tempfile::tempdir().unwrap();
            let (repo, feature) = published_fixture(dir.path(), source, remote);
            if let Some(base) = configured { fixture_git(&repo, &["config", "branch.feature.hangar-base", base]); }
            match action {
                "expire" => { fixture_git(&repo, &["reflog", "expire", "--expire=all", "refs/heads/feature"]); }
                "switch" => { fixture_git(&repo, &["switch", "-c", "unrelated", &format!("{remote}/server")]); }
                "new-commit" => {
                    std::fs::write(feature.join("pending.txt"), "Não integrada\n").unwrap();
                    fixture_git(&feature, &["add", "pending.txt"]);
                    fixture_git(&feature, &["commit", "-m", "Ainda em andamento"]);
                }
                "track" => { fixture_git(&repo, &["branch", "--set-upstream-to=origin/server", "feature"]); }
                "missing" => { fixture_git(&repo, &["update-ref", "-d", &format!("refs/remotes/{remote}/server")]); }
                "tag" => { fixture_git(&repo, &["tag", "origin/server", "main"]); }
                "tag-main" => { fixture_git(&repo, &["tag", "origin/main", "refs/remotes/origin/server"]); }
                "ambiguous" | "upstream" => {
                    fixture_git(&repo, &["remote", "add", "team", dir.path().join("origin.git").to_str().unwrap()]);
                    fixture_git(&repo, &["fetch", "team"]);
                    if action == "upstream" { fixture_git(&repo, &["branch", "--set-upstream-to=origin/server", "server"]); }
                }
                _ => {}
            }
            let status = status_of(feature.to_str().unwrap(), &[], false, &[]);
            assert_eq!(status["base"], base, "case {}: {status}", index + 1);
            assert_eq!(status["merged"], merged, "case {}: {status}", index + 1);
            assert_eq!(status["degraded"], degraded, "case {}: {status}", index + 1);
            if merged { assert_eq!((status["ahead"].as_u64(), status["behind"].as_u64()), (Some(0), Some(1))); }
            if action == "new-commit" { assert_eq!(status["ahead"], 1); }
        }
    }

    #[test]
    fn t4_tag_named_like_branch_does_not_hide_pending_commits() {
        if t4_isolated("t4_tag_named_like_branch_does_not_hide_pending_commits") { return; }
        let dir = tempfile::tempdir().unwrap();
        let (repo, feature) = published_fixture(dir.path(), "remote-server", "origin");
        fixture_git(&repo, &["tag", "feature", "feature"]);
        std::fs::write(feature.join("pending.txt"), "Não integrada\n").unwrap();
        fixture_git(&feature, &["add", "pending.txt"]);
        fixture_git(&feature, &["commit", "-m", "Ainda em andamento"]);
        let status = status_of(feature.to_str().unwrap(), &[], false, &[]);
        assert_eq!(status["merged"], false);
        assert_eq!(status["ahead"], 1);
        assert_eq!(status["last_commit"]["subject"], "Ainda em andamento");
    }

    #[test]
    fn sanitize_matches_the_registry() {
        assert_eq!(sanitize_cwd("/home/x/ação/"), "-home-x-a--o");
        assert_eq!(sanitize_cwd("/"), "-");
        assert_eq!(sanitize_cwd("C:\\x\\"), "C--x");
    }

    #[test]
    fn registration_path_is_stable_after_the_folder_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worktree");
        std::fs::create_dir(&path).unwrap();
        let before = super::registration_path(&path);
        std::fs::remove_dir(&path).unwrap();
        let after = super::registration_path(&path);
        assert_eq!(before.as_os_str(), after.as_os_str());
    }

    #[test]
    fn normpath_is_lexical() {
        assert_eq!(
            normpath(Path::new("/a/b/../c/./d")),
            PathBuf::from("/a/c/d")
        );
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
