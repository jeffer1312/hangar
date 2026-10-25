//! Política e remoção compartilhadas pelas ações manuais e pela limpeza periódica.
use crate::{
    Result, error, git, real,
    worktree_settings::{self, KeepWhen, WorktreeSettings},
    worktrees::{self, Session},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    time::{Duration, Instant, UNIX_EPOCH},
};

pub fn in_use(path: &str, sessions: &[Session]) -> bool {
    let root = real(Path::new(path));
    sessions.iter().any(|s| worktrees::session_inside(s, &root))
}

enum Registration {
    Ready(String),
    Locked,
}

fn registration(repo: &str, path: &str) -> Result<Registration> {
    let out = git::checked(
        Path::new(repo),
        &["worktree", "list", "--porcelain", "-z"],
        409,
    )?;
    if out.stdout.contains('\u{fffd}') {
        return Err(error(409, "registro de worktrees ilegível"));
    }
    let real_path = real(Path::new(path));
    for record in out.stdout.split("\0\0") {
        let fields = record.split('\0').collect::<Vec<_>>();
        let Some(p) = fields.first().and_then(|f| f.strip_prefix("worktree ")) else {
            continue;
        };
        if real(Path::new(p)) != real_path {
            continue;
        }
        if real(Path::new(repo)) == real_path {
            return Err(error(409, "o checkout principal é protegido"));
        }
        if fields
            .iter()
            .any(|f| *f == "locked" || f.starts_with("locked "))
        {
            return Ok(Registration::Locked);
        }
        return Ok(Registration::Ready(p.into()));
    }
    Err(error(404, "não é uma worktree deste repositório"))
}

fn registered(repo: &str, path: &str) -> Result<String> {
    match registration(repo, path)? {
        Registration::Ready(path) => Ok(path),
        Registration::Locked => Err(error(409, "worktree bloqueada")),
    }
}

fn with_removed<T>(
    home: &Path,
    path: &str,
    repo: &str,
    operation: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let file = home.join(".hangar/worktrees-removidas.json");
    worktree_settings::locked(&file, || {
        let mut map = worktrees::try_removed_at(&file)
            .map_err(|_| error(503, "mapa de remoções ilegível"))?;
        let previous = map.insert(path.into(), repo.into());
        worktree_settings::write_json(&file, &map)?;
        match operation() {
            Ok(result) => Ok(result),
            Err(err) => {
                if let Some(previous) = previous {
                    map.insert(path.into(), previous);
                } else {
                    map.remove(path);
                }
                worktree_settings::write_json(&file, &map)?;
                Err(err)
            }
        }
    })
}

#[derive(Clone, Copy, Default)]
pub struct RemoveOptions {
    pub confirm: bool,
    pub delete_branch: bool,
    pub keep_branch: bool,
    pub merged_only: bool,
    pub require_clean: bool,
}
pub fn remove_merged(
    repo: &str,
    path: &str,
    sessions: &[Session],
    bases: &[String],
    home: &Path,
    confirm: bool,
) -> Result<Value> {
    remove(
        repo,
        path,
        sessions,
        bases,
        home,
        RemoveOptions {
            confirm,
            merged_only: true,
            ..RemoveOptions::default()
        },
    )
}
pub fn remove(
    repo: &str,
    path: &str,
    sessions: &[Session],
    bases: &[String],
    home: &Path,
    options: RemoveOptions,
) -> Result<Value> {
    remove_with_guard(repo, path, sessions, bases, home, options, &|_| Ok(()))
}
pub fn remove_with_guard(
    repo: &str,
    path: &str,
    sessions: &[Session],
    bases: &[String],
    home: &Path,
    options: RemoveOptions,
    guard: &dyn Fn(&str) -> Result<()>,
) -> Result<Value> {
    let RemoveOptions {
        confirm,
        delete_branch,
        keep_branch,
        merged_only: require_merged,
        require_clean,
    } = options;
    let root =
        worktrees::repo_root_of(repo).ok_or_else(|| error(404, "repositório não encontrado"))?;
    let main = worktrees::main_repo_of(&root);
    let path = registered(&main, path)?;
    if in_use(&path, sessions) {
        return Err(error(409, "sessão aberta dentro da worktree"));
    }
    // Sem pasta, o registro pode sobreviver até à branch: sua limpeza não depende do HEAD.
    if missing_path(&path)? && !require_merged {
        with_removed(home, &path, &main, || {
            registered(&main, &path)?;
            guard(&path)?;
            if !missing_path(&path)? {
                return Err(error(409, "a pasta reapareceu; confira a worktree antes de remover"));
            }
            git::remove_worktree(Path::new(&main), &path, false)?;
            if worktrees::worktree_paths(&main).contains(&path) {
                return Err(error(409, "a worktree continua registrada"));
            }
            Ok(())
        })?;
        return Ok(json!({"removed":path,"branch_deleted":false,"moved":0}));
    }
    let repo_info = worktrees::Repo::read(&main);
    let st = worktrees::status(&path, sessions, &repo_info, false, bases);
    if st["degraded"] != false || st["head"].as_str().is_none() {
        return Err(error(409, "não consegui ler a worktree; tente de novo"));
    }
    if require_clean && st["dirty"].as_u64() != Some(0) {
        return Err(error(409, "a worktree deixou de estar limpa"));
    }
    if !confirm
        && (st["dirty"].as_u64().unwrap_or(1) > 0
            || st["ignored"].as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(error(409, "há arquivos que serão perdidos; confirme"));
    }
    let branch = st["branch"].as_str().unwrap_or("");
    let locks = Path::new(&path)
        .is_dir()
        .then(|| git_locks(&path, branch))
        .transpose()?;
    let latest = worktrees::status(&path, sessions, &worktrees::Repo::read(&main), false, bases);
    if latest["degraded"] != false
        || latest["head"] != st["head"]
        || latest["branch"] != st["branch"]
    {
        return Err(error(409, "a worktree mudou durante a remoção"));
    }
    if require_clean && latest["dirty"].as_u64() != Some(0) {
        return Err(error(409, "a worktree deixou de estar limpa"));
    }
    if !confirm
        && (latest["dirty"].as_u64().unwrap_or(1) > 0
            || latest["ignored"].as_array().is_none_or(|a| !a.is_empty()))
    {
        return Err(error(409, "há arquivos que serão perdidos; confirme"));
    }
    if require_merged && latest["merged"] != true {
        return Err(error(409, "a worktree deixou de estar mesclada"));
    }
    let keep = if require_clean { KeepWhen::UncommittedChanges } else { KeepWhen::TrackedChanges };
    let manual_stamp = if Path::new(&path).is_dir() {
        let (stamp, _, protected) = fingerprint(&path, &latest, &keep)?;
        if require_clean && protected {
            return Err(error(409, "a worktree deixou de estar limpa"));
        }
        Some(stamp)
    } else {
        None
    };
    let config = git_config(&main)?;
    let base_head = latest["base"]
        .as_str()
        .map(|base| reference_head(&main, base))
        .transpose()?;
    with_removed(home, &path, &main, || {
        registered(&main, &path)?;
        guard(&path)?;
        if let Some(stamp) = &manual_stamp
            && (&fingerprint(&path, &latest, &keep)?.0 != stamp
                || reference_head(&path, "HEAD")? != latest["head"].as_str().unwrap().trim()
                || git::head_info(Some(&path)).0.as_deref() != latest["branch"].as_str()
                || git_config(&main)? != config
                || latest["base"]
                    .as_str()
                    .map(|base| reference_head(&main, base))
                    .transpose()?
                    != base_head)
        {
            return Err(error(409, "a worktree mudou após a admissão da remoção"));
        }
        git::remove_worktree(Path::new(&main), &path, confirm)?;
        if worktrees::worktree_paths(&main).contains(&path) {
            return Err(error(409, "a worktree continua registrada"));
        }
        Ok(())
    })?;
    drop(locks);
    let branch_deleted =
        if !branch.is_empty() && !keep_branch && (latest["merged"] == true || delete_branch) {
            let reference = format!("refs/heads/{branch}");
            let checked_out = git::command(
                Path::new(&main),
                &["worktree", "list", "--porcelain", "-z"],
            )
            .map_or(true, |o| {
                o.code != 0
                    || o.stdout
                        .split('\0')
                        .any(|f| f == format!("branch {reference}"))
            });
            !checked_out
                && git::command(
                    Path::new(&main),
                    &[
                        "update-ref",
                        "-d",
                        &reference,
                        latest["head"].as_str().unwrap().trim(),
                    ],
                )
                .is_ok_and(|o| o.code == 0)
        } else {
            false
        };
    if branch_deleted && clear_deleted_branch_config(Path::new(&main), branch).is_err() {
        tracing::warn!(
            "configuração da branch removida foi conservada por concorrência ou falha de leitura"
        );
    }
    Ok(json!({"removed":path,"branch_deleted":branch_deleted,"moved":0}))
}

fn missing_path(path: &str) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(_) => Err(error(409, "não consegui conferir a pasta da worktree")),
    }
}

pub fn clear_deleted_branch_config(repo: &Path, branch: &str) -> Result<bool> {
    let reference = format!("refs/heads/{branch}");
    let out = git::checked(
        repo,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            &reference,
        ],
        409,
    )?;
    let path = PathBuf::from(format!("{}.lock", out.stdout.trim()));
    let mut locks = GitLocks(Vec::new());
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|_| error(409, "pasta da referência indisponível"))?;
    }
    std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .map_err(|_| error(409, "branch ocupada; configuração conservada"))?;
    locks.0.push(path);
    let current = git::command(repo, &["show-ref", "--verify", "--quiet", &reference])?;
    if current.code == 0 {
        return Ok(false);
    }
    if current.code != 1 {
        return Err(error(409, "estado da branch desconhecido"));
    }
    let cleared = git::command(
        repo,
        &["config", "--remove-section", &format!("branch.{branch}")],
    )?;
    if !matches!(cleared.code, 0 | 5) {
        return Err(error(409, "não consegui limpar a configuração da branch"));
    }
    Ok(true)
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Observation {
    fingerprint: String,
    last_activity: u64,
}
#[derive(Clone, Debug)]
pub struct Candidate {
    pub path: String,
    pub head: String,
    fingerprint: String,
    pub settings: WorktreeSettings,
    base: String,
    base_head: String,
    config: String,
}
fn reference_head(path: &str, reference: &str) -> Result<String> {
    Ok(git::checked(
        Path::new(path),
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{reference}^{{commit}}"),
        ],
        409,
    )?
    .stdout
    .trim()
    .into())
}
fn git_config(path: &str) -> Result<String> {
    let out = git::checked(Path::new(path), &["config", "--null", "--list"], 409)?;
    if out.stdout.contains('\u{fffd}') {
        return Err(error(409, "configuração Git ilegível"));
    }
    Ok(out.stdout)
}
fn tree_stamp(root: &Path) -> Result<(String, u64)> {
    let start = Instant::now();
    let mut stack = vec![root.to_path_buf()];
    let mut entries = Vec::new();
    let mut newest = 0;
    while let Some(path) = stack.pop() {
        if entries.len() > 100_000 || start.elapsed() > Duration::from_secs(5) {
            return Err(error(409, "leitura do disco incompleta"));
        }
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|_| error(409, "arquivo da worktree ilegível"))?;
        let modified = meta
            .modified()
            .map_err(|_| error(409, "data de arquivo indisponível"))?
            .duration_since(UNIX_EPOCH)
            .map_err(|_| error(409, "data de arquivo inválida"))?;
        newest = newest.max(modified.as_secs());
        entries.push((
            path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
            meta.len(),
            modified.as_nanos(),
            meta.is_dir(),
        ));
        if meta.is_dir() && !meta.is_symlink() {
            for entry in
                std::fs::read_dir(&path).map_err(|_| error(409, "pasta da worktree ilegível"))?
            {
                stack.push(
                    entry
                        .map_err(|_| error(409, "pasta da worktree ilegível"))?
                        .path(),
                );
            }
        }
    }
    entries.sort();
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    entries.hash(&mut hash);
    Ok((format!("{:016x}", hash.finish()), newest))
}
fn local_status(path: &str, keep: &KeepWhen) -> Result<(String, bool)> {
    let out = git::checked(
        Path::new(path),
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignored=matching",
            "--ignore-submodules=none",
        ],
        409,
    )?;
    if out.stdout.len() > 16 * 1024 * 1024 || out.stdout.contains('\u{fffd}') {
        return Err(error(409, "estado de arquivos incompleto"));
    }
    let mut fields = out.stdout.split('\0').filter(|s| !s.is_empty());
    let mut protected = false;
    while let Some(field) = fields.next() {
        let code = field
            .get(..2)
            .ok_or_else(|| error(409, "estado de arquivos inválido"))?;
        if field.as_bytes().get(2) != Some(&b' ') {
            return Err(error(409, "estado de arquivos inválido"));
        }
        let tracked = !matches!(code, "??" | "!!");
        protected |= tracked
            || match keep {
                KeepWhen::TrackedChanges => false,
                KeepWhen::UncommittedChanges => code == "??",
                KeepWhen::AnyLocalFiles => true,
            };
        if code.contains(['R', 'C']) && fields.next().is_none() {
            return Err(error(409, "estado de arquivos incompleto"));
        }
    }
    Ok((out.stdout, protected))
}
fn fingerprint(path: &str, st: &Value, keep: &KeepWhen) -> Result<(String, u64, bool)> {
    let (stamp, newest) = tree_stamp(Path::new(path))?;
    let (local, protected) = local_status(path, keep)?;
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    (stamp, local, st["head"].as_str(), st["branch"].as_str()).hash(&mut hash);
    Ok((format!("{:016x}", hash.finish()), newest, protected))
}
pub fn candidate(
    path: &str,
    sessions: &[Session],
    bases: &[String],
    home: &Path,
    settings: &WorktreeSettings,
    now: u64,
    activity: Option<u64>,
) -> Result<Option<Candidate>> {
    if !settings.enabled() || !Path::new(path).is_dir() || in_use(path, sessions) {
        return Ok(None);
    }
    let can_observe_inactivity = settings.inactive_days.is_some() && activity.is_some();
    if !settings.delete_merged && !settings.delete_unchanged && !can_observe_inactivity {
        return Ok(None);
    }
    let st = worktrees::status_of(path, sessions, false, bases);
    let Some(repo) = st["repo"].as_str() else {
        return Ok(None);
    };
    if matches!(registration(repo, path)?, Registration::Locked) {
        return Ok(None);
    }
    if st["degraded"] != false || st["base"].as_str().is_none() || st["head"].as_str().is_none() {
        return Ok(None);
    }
    let by_state = settings.delete_merged && st["merged"] == true
        || settings.delete_unchanged && st["ahead"].as_u64() == Some(0);
    if !by_state && !can_observe_inactivity {
        return Ok(None);
    }
    let (fingerprint, newest, protected) = fingerprint(path, &st, &settings.keep_when)?;
    if protected {
        return Ok(None);
    }
    let mut inactive = false;
    if let Some(days) = settings.inactive_days
        && let Some(activity) = activity
    {
        let file = home.join(".hangar/worktree-observations.json");
        let last_activity = worktree_settings::locked(&file, || {
            let mut observations: BTreeMap<String, Observation> = match std::fs::read(&file) {
                Ok(bytes) => serde_json::from_slice(&bytes)
                    .map_err(|_| error(503, "histórico de atividade ilegível"))?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
                Err(_) => return Err(error(503, "histórico de atividade ilegível")),
            };
            let old = observations.get(path);
            let last = old
                .filter(|o| o.fingerprint == fingerprint)
                .map_or(now, |o| o.last_activity)
                .max(newest)
                .max(activity);
            observations.insert(
                path.into(),
                Observation {
                    fingerprint: fingerprint.clone(),
                    last_activity: last,
                },
            );
            worktree_settings::write_json(&file, &observations)?;
            Ok(last)
        })?;
        inactive = now.saturating_sub(last_activity) >= u64::from(days) * 86400;
    }
    let eligible = inactive || by_state;
    if !eligible {
        return Ok(None);
    }
    let base = st["base"].as_str().unwrap().to_owned();
    let base_head = reference_head(path, &base)?;
    let config = git_config(path)?;
    Ok(Some(Candidate {
        path: path.into(),
        head: st["head"].as_str().unwrap().trim().into(),
        fingerprint,
        settings: settings.clone(),
        base,
        base_head,
        config,
    }))
}

struct GitLocks(Vec<PathBuf>);
impl Drop for GitLocks {
    fn drop(&mut self) {
        for p in self.0.iter().rev() {
            let _ = std::fs::remove_file(p);
        }
    }
}
fn git_locks(path: &str, branch: &str) -> Result<GitLocks> {
    let mut locks = GitLocks(Vec::new());
    for name in ["HEAD".to_owned(), "index".to_owned()]
        .into_iter()
        .chain((!branch.is_empty()).then(|| format!("refs/heads/{branch}")))
    {
        let out = git::checked(
            Path::new(path),
            &["rev-parse", "--path-format=absolute", "--git-path", &name],
            409,
        )?;
        let lock = PathBuf::from(format!("{}.lock", out.stdout.trim()));
        if let Some(parent) = lock.parent() {
            std::fs::create_dir_all(parent).map_err(|_| error(409, "trava Git indisponível"))?;
        }
        std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&lock)
            .map_err(|_| error(409, "worktree ocupada pelo Git"))?;
        locks.0.push(lock);
    }
    Ok(locks)
}

/// Recebe sessões e atividade recém-lidas; `git worktree remove` sem force é a última proteção de edição rastreada.
pub fn remove_candidate(
    candidate: &Candidate,
    sessions: &[Session],
    bases: &[String],
    home: &Path,
    now: u64,
    activity: Option<u64>,
) -> Result<bool> {
    remove_candidate_with_guard(candidate, sessions, bases, home, now, activity, &|_| Ok(()))
}
pub fn remove_candidate_with_guard(
    candidate: &Candidate,
    sessions: &[Session],
    bases: &[String],
    home: &Path,
    now: u64,
    activity: Option<u64>,
    guard: &dyn Fn(&str) -> Result<()>,
) -> Result<bool> {
    let file = home.join(".hangar/worktree-settings.json");
    worktree_settings::locked(&file, || {
        let settings = WorktreeSettings::load_at(&file)?;
        if settings != candidate.settings {
            return Ok(false);
        }
        let st = worktrees::status_of(&candidate.path, sessions, false, bases);
        let Some(branch) = st["branch"].as_str() else {
            return Ok(false);
        };
        let _locks = git_locks(&candidate.path, branch)?;
        let Some(fresh) = self::candidate(
            &candidate.path,
            sessions,
            bases,
            home,
            &settings,
            now,
            activity,
        )?
        else {
            return Ok(false);
        };
        if fresh.head != candidate.head || fresh.fingerprint != candidate.fingerprint {
            return Ok(false);
        }
        let repo = st["repo"]
            .as_str()
            .ok_or_else(|| error(409, "repositório indisponível"))?;
        with_removed(home, &candidate.path, repo, || {
            // A admissão de fatos precede toda perda; depois dela só se lê estado local sob as travas.
            guard(&candidate.path)?;
            let (stamp, _, protected) = fingerprint(&candidate.path, &st, &settings.keep_when)?;
            let branch_now = git::checked(
                Path::new(&candidate.path),
                &["symbolic-ref", "--quiet", "--short", "HEAD"],
                409,
            )?;
            if protected
                || stamp != candidate.fingerprint
                || branch_now.stdout.trim() != branch
                || reference_head(&candidate.path, "HEAD")? != candidate.head
                || reference_head(&candidate.path, &candidate.base)? != candidate.base_head
                || git_config(&candidate.path)? != candidate.config
            {
                return Err(error(409, "a worktree mudou após a admissão da limpeza"));
            }
            registered(repo, &candidate.path)?;
            if settings.keep_when == KeepWhen::TrackedChanges {
                git::checked(Path::new(&candidate.path), &["clean", "-fdx"], 409)?;
            }
            // Sem force: edição rastreada que surgiu durante a fase destrutiva continua protegida pelo Git.
            git::remove_worktree(Path::new(repo), &candidate.path, false)
        })?;
        Ok(true)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn git(dir: &Path, args: &[&str]) {
        let o = crate::git::command(dir, args).unwrap();
        assert_eq!(o.code, 0, "{}", o.stderr);
    }
    fn policy_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = crate::real(dir.path());
        let repo = root.join("repo");
        let wt = root.join("wt");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=Teste",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "Base",
            ],
        );
        git(
            &repo,
            &["worktree", "add", "-b", "topic", wt.to_str().unwrap()],
        );
        (dir, repo, wt)
    }

    #[test]
    fn missing_worktree_removes_only_its_registration_without_needing_the_branch() {
        let (dir, repo, wt) = policy_fixture();
        let other = repo.parent().unwrap().join("other");
        git(&repo, &["worktree", "add", "-b", "other", other.to_str().unwrap()]);
        std::fs::remove_dir_all(&wt).unwrap();
        std::fs::remove_dir_all(&other).unwrap();
        git(&repo, &["update-ref", "-d", "refs/heads/topic"]);
        let result = remove(
            repo.to_str().unwrap(), wt.to_str().unwrap(), &[], &[], dir.path(),
            RemoveOptions { delete_branch: true, ..Default::default() },
        ).unwrap();
        assert_eq!(Path::new(result["removed"].as_str().unwrap()), wt);
        assert_eq!(result["branch_deleted"], false);
        assert_eq!(worktrees::worktree_paths(repo.to_str().unwrap()).into_iter().map(PathBuf::from).collect::<Vec<_>>(), vec![other]);
        git(&repo, &["rev-parse", "--verify", "refs/heads/other"]);
    }

    #[test]
    fn missing_worktree_preserves_its_branch_and_protects_locks_and_sessions() {
        let (dir, repo, wt) = policy_fixture();
        git(&repo, &["worktree", "lock", wt.to_str().unwrap()]);
        std::fs::remove_dir_all(&wt).unwrap();
        let run = |sessions: &[Session]| remove(
            repo.to_str().unwrap(), wt.to_str().unwrap(), sessions, &[], dir.path(),
            RemoveOptions { delete_branch: true, ..Default::default() },
        );
        assert_eq!(run(&[]).unwrap_err().status, 409);
        git(&repo, &["worktree", "unlock", wt.to_str().unwrap()]);
        let sessions = [Session { name: "busy".into(), cwd: Some(wt.to_string_lossy().into_owned()), ..Default::default() }];
        assert_eq!(run(&sessions).unwrap_err().status, 409);
        assert_eq!(run(&[]).unwrap()["branch_deleted"], false);
        git(&repo, &["rev-parse", "--verify", "refs/heads/topic"]);
    }

    #[test]
    fn missing_worktree_that_reappears_during_admission_is_preserved() {
        let (dir, repo, wt) = policy_fixture();
        std::fs::remove_dir_all(&wt).unwrap();
        let result = remove_with_guard(
            repo.to_str().unwrap(), wt.to_str().unwrap(), &[], &[], dir.path(),
            RemoveOptions::default(), &|_| {
                std::fs::create_dir(&wt).unwrap();
                std::fs::write(wt.join("local"), "trabalho recuperado").unwrap();
                Ok(())
            },
        );
        assert_eq!(result.unwrap_err().status, 409);
        assert_eq!(std::fs::read_to_string(wt.join("local")).unwrap(), "trabalho recuperado");
        assert!(worktrees::worktree_paths(repo.to_str().unwrap()).contains(&wt.to_string_lossy().into_owned()));
    }

    #[test]
    fn locked_worktree_is_protected_without_a_read_error() {
        let (dir, repo, wt) = policy_fixture();
        git(&repo, &["worktree", "lock", wt.to_str().unwrap()]);
        let settings = WorktreeSettings {
            delete_unchanged: true,
            ..WorktreeSettings::default()
        };
        assert!(
            candidate(
                wt.to_str().unwrap(),
                &[],
                &[],
                dir.path(),
                &settings,
                0,
                None
            )
            .unwrap()
            .is_none()
        );
        assert!(wt.exists());
        git(&repo, &["worktree", "unlock", wt.to_str().unwrap()]);
        std::fs::write(repo.join(".git/worktrees/wt/gitdir"), [0xff]).unwrap();
        assert!(
            candidate(
                wt.to_str().unwrap(),
                &[],
                &[],
                dir.path(),
                &settings,
                0,
                None
            )
            .is_err(),
            "registro ilegível continua sendo falha de leitura"
        );
    }

    #[test]
    fn ineligible_worktree_without_activity_does_not_read_the_tree() {
        let (dir, _repo, wt) = policy_fixture();
        git(
            &wt,
            &[
                "-c",
                "user.name=Teste",
                "-c",
                "user.email=test@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "Ainda não integrada",
            ],
        );
        let poison = wt.join("local-artifact");
        let file = std::fs::File::create(poison).unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH - Duration::from_secs(1)))
            .unwrap();
        let path = wt.to_str().unwrap();
        let mut settings = WorktreeSettings {
            delete_merged: true,
            ..WorktreeSettings::default()
        };
        assert!(
            candidate(
                path,
                &[],
                &["main".into()],
                dir.path(),
                &settings,
                100_000,
                None
            )
            .unwrap()
            .is_none()
        );
        settings.inactive_days = Some(1);
        assert!(
            candidate(
                path,
                &[],
                &["main".into()],
                dir.path(),
                &settings,
                100_000,
                None
            )
            .unwrap()
            .is_none()
        );
        assert!(
            candidate(
                path,
                &[],
                &["main".into()],
                dir.path(),
                &settings,
                100_000,
                Some(99_999)
            )
            .is_err(),
            "atividade recente ainda exige observação do disco"
        );
        assert!(
            candidate(
                path,
                &[],
                &["main".into()],
                dir.path(),
                &settings,
                100_000,
                Some(0)
            )
            .is_err(),
            "candidata potencial ainda exige leitura segura"
        );
    }

    #[test]
    fn recent_activity_does_not_postpone_the_inactivity_deadline() {
        let (dir, _repo, wt) = policy_fixture();
        let settings = WorktreeSettings {
            inactive_days: Some(1),
            ..WorktreeSettings::default()
        };
        let observed_at = 4_000_000_000;
        assert!(
            candidate(
                wt.to_str().unwrap(),
                &[],
                &[],
                dir.path(),
                &settings,
                observed_at,
                Some(observed_at)
            )
            .unwrap()
            .is_none()
        );
        assert!(
            candidate(
                wt.to_str().unwrap(),
                &[],
                &[],
                dir.path(),
                &settings,
                observed_at + 86_400,
                Some(observed_at)
            )
            .unwrap()
            .is_some(),
            "o prazo conta da observação inicial, sem um segundo dia artificial"
        );
    }

    #[test]
    fn removal_protects_live_dirty_locked_and_preserves_branch_and_conversations() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        git(
            &repo,
            &["worktree", "add", "-b", "topic", wt.to_str().unwrap()],
        );
        let repo = repo.to_str().unwrap();
        let path = wt.to_str().unwrap();
        let sessions = vec![Session {
            name: "live".into(),
            cwd: Some(path.into()),
            ..Session::default()
        }];
        assert!(
            remove(
                repo,
                path,
                &sessions,
                &[],
                dir.path(),
                RemoveOptions {
                    keep_branch: true,
                    ..RemoveOptions::default()
                }
            )
            .is_err()
        );
        std::fs::write(wt.join("local.txt"), "keep").unwrap();
        assert!(
            remove(
                repo,
                path,
                &[],
                &[],
                dir.path(),
                RemoveOptions {
                    keep_branch: true,
                    ..RemoveOptions::default()
                }
            )
            .is_err()
        );
        git(Path::new(repo), &["worktree", "lock", path]);
        assert!(
            remove(
                repo,
                path,
                &[],
                &[],
                dir.path(),
                RemoveOptions {
                    confirm: true,
                    keep_branch: true,
                    ..RemoveOptions::default()
                }
            )
            .is_err()
        );
        git(Path::new(repo), &["worktree", "unlock", path]);
        let expected_wt = real(&wt);
        let out = remove(
            repo,
            path,
            &[],
            &[],
            dir.path(),
            RemoveOptions {
                confirm: true,
                keep_branch: true,
                ..RemoveOptions::default()
            },
        )
        .unwrap();
        let removed_path = out["removed"].as_str().unwrap();
        assert_eq!(Path::new(removed_path), expected_wt.as_path());
        assert!(!wt.exists());
        git(
            Path::new(repo),
            &["show-ref", "--verify", "refs/heads/topic"],
        );
        let map = worktrees::try_removed_at(&dir.path().join(".hangar/worktrees-removidas.json"))
            .unwrap();
        assert_eq!(
            real(Path::new(map.get(removed_path).unwrap())),
            real(Path::new(repo))
        );
        assert!(
            remove(
                repo,
                repo,
                &[],
                &[],
                dir.path(),
                RemoveOptions {
                    confirm: true,
                    keep_branch: true,
                    ..RemoveOptions::default()
                }
            )
            .is_err()
        );
    }
    #[test]
    fn automatic_candidate_uses_disk_and_conversation_activity_and_file_policy() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        git(
            &repo,
            &["worktree", "add", "-b", "topic", wt.to_str().unwrap()],
        );
        let path = wt.to_str().unwrap();
        let mut settings = WorktreeSettings {
            inactive_days: Some(1),
            ..WorktreeSettings::default()
        };
        let now = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        assert!(
            candidate(path, &[], &[], dir.path(), &settings, now, Some(0))
                .unwrap()
                .is_none(),
            "primeira observação não prova inatividade"
        );
        assert!(
            candidate(path, &[], &[], dir.path(), &settings, now + 86401, Some(0))
                .unwrap()
                .is_some()
        );
        assert!(
            candidate(
                path,
                &[],
                &[],
                dir.path(),
                &settings,
                now + 86401,
                Some(now + 86400)
            )
            .unwrap()
            .is_none(),
            "conversa recente protege"
        );
        assert!(
            candidate(path, &[], &[], dir.path(), &settings, now + 172803, None)
                .unwrap()
                .is_none(),
            "fatos desconhecidos protegem"
        );
        settings.inactive_days = None;
        settings.delete_unchanged = true;
        std::fs::write(wt.join("new.txt"), "local").unwrap();
        assert!(
            candidate(path, &[], &[], dir.path(), &settings, now, None)
                .unwrap()
                .is_none()
        );
        settings.keep_when = KeepWhen::TrackedChanges;
        assert!(
            candidate(path, &[], &[], dir.path(), &settings, now, None)
                .unwrap()
                .is_some()
        );
        std::fs::write(wt.join(".gitignore"), "ignored.txt\n").unwrap();
        git(&wt, &["add", ".gitignore"]);
        git(
            &wt,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-m",
                "ignore",
            ],
        );
        git(&repo, &["merge", "topic"]);
        std::fs::remove_file(wt.join("new.txt")).unwrap();
        std::fs::write(wt.join("ignored.txt"), "secret").unwrap();
        settings.keep_when = KeepWhen::AnyLocalFiles;
        assert!(
            candidate(path, &[], &[], dir.path(), &settings, now, None)
                .unwrap()
                .is_none()
        );
        settings.keep_when = KeepWhen::UncommittedChanges;
        assert!(
            candidate(path, &[], &[], dir.path(), &settings, now, None)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn automatic_removal_revalidates_and_never_forces_tracked_edits() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("tracked"), "original").unwrap();
        git(&repo, &["add", "tracked"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-m",
                "initial",
            ],
        );
        git(
            &repo,
            &["worktree", "add", "-b", "topic", wt.to_str().unwrap()],
        );
        let path = wt.to_str().unwrap();
        let settings = WorktreeSettings {
            delete_unchanged: true,
            keep_when: KeepWhen::TrackedChanges,
            ..WorktreeSettings::default()
        };
        settings
            .save_at(
                &dir.path().join(".hangar/worktree-settings.json"),
                dir.path(),
            )
            .unwrap();
        std::fs::write(wt.join("untracked"), "discardable").unwrap();
        let c = candidate(path, &[], &[], dir.path(), &settings, 0, None)
            .unwrap()
            .unwrap();
        std::fs::write(wt.join("tracked"), "new version").unwrap();
        assert!(!remove_candidate(&c, &[], &[], dir.path(), 0, None).unwrap());
        assert_eq!(
            std::fs::read_to_string(wt.join("tracked")).unwrap(),
            "new version"
        );
        std::fs::write(wt.join("tracked"), "original").unwrap();
        let c = candidate(path, &[], &[], dir.path(), &settings, 0, None)
            .unwrap()
            .unwrap();
        let live = [Session {
            name: "new-session".into(),
            cwd: Some(path.into()),
            ..Session::default()
        }];
        assert!(!remove_candidate(&c, &live, &[], dir.path(), 0, None).unwrap());
        WorktreeSettings::default()
            .save_at(
                &dir.path().join(".hangar/worktree-settings.json"),
                dir.path(),
            )
            .unwrap();
        assert!(!remove_candidate(&c, &[], &[], dir.path(), 0, None).unwrap());
        settings
            .save_at(
                &dir.path().join(".hangar/worktree-settings.json"),
                dir.path(),
            )
            .unwrap();
        let c = candidate(path, &[], &[], dir.path(), &settings, 0, None)
            .unwrap()
            .unwrap();
        assert!(remove_candidate(&c, &[], &[], dir.path(), 0, None).unwrap());
        assert!(!wt.exists());
        git(&repo, &["show-ref", "--verify", "refs/heads/topic"]);
    }
    #[test]
    fn final_guard_and_git_refusal_restore_removed_map_without_losing_new_tracked_data() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("tracked"), "old").unwrap();
        git(&repo, &["add", "tracked"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-m",
                "initial",
            ],
        );
        git(
            &repo,
            &["worktree", "add", "-b", "topic", wt.to_str().unwrap()],
        );
        let path = wt.to_str().unwrap();
        let settings = WorktreeSettings {
            delete_unchanged: true,
            ..Default::default()
        };
        settings
            .save_at(
                &dir.path().join(".hangar/worktree-settings.json"),
                dir.path(),
            )
            .unwrap();
        let c = candidate(path, &[], &[], dir.path(), &settings, 0, None)
            .unwrap()
            .unwrap();
        let previous =
            std::collections::HashMap::from([(path.to_owned(), "previous/main".to_owned())]);
        let map = dir.path().join(".hangar/worktrees-removidas.json");
        worktree_settings::write_json(&map, &previous).unwrap();
        assert!(
            remove_candidate_with_guard(&c, &[], &[], dir.path(), 0, None, &|_| Err(error(
                409,
                "sessão recém-aberta"
            )))
            .is_err()
        );
        assert!(wt.exists());
        assert_eq!(worktrees::try_removed_at(&map).unwrap(), previous);
        let guard = |_: &str| {
            std::fs::write(wt.join("tracked"), "new version").unwrap();
            Ok(())
        };
        assert!(remove_candidate_with_guard(&c, &[], &[], dir.path(), 0, None, &guard).is_err());
        assert_eq!(
            std::fs::read_to_string(wt.join("tracked")).unwrap(),
            "new version"
        );
        assert_eq!(worktrees::try_removed_at(&map).unwrap(), previous);
    }
    #[test]
    fn session_working_in_another_checkout_is_protected_by_git_cwd() {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().join("wt");
        std::fs::create_dir(&wt).unwrap();
        let missing = wt.join("subdir");
        let missing = if cfg!(windows) {
            missing.to_string_lossy().to_uppercase()
        } else {
            missing.to_string_lossy().into_owned()
        };
        let sessions: Vec<Session> =
            serde_json::from_value(json!([{"name":"live","cwd":dir.path(),"git_cwd":missing}]))
                .unwrap();
        assert!(in_use(wt.to_str().unwrap(), &sessions));
        let outside = [Session {
            git_cwd: Some(
                wt.with_file_name("wt-other")
                    .join("subdir")
                    .to_string_lossy()
                    .into(),
            ),
            ..Session::default()
        }];
        assert!(
            !in_use(wt.to_str().unwrap(), &outside),
            "prefixo textual não identifica um descendente"
        );
    }
    #[test]
    fn refused_admission_or_invalid_map_never_cleans_local_files() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join(".gitignore"), ".env\n").unwrap();
        git(&repo, &["add", ".gitignore"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "-m",
                "initial",
            ],
        );
        git(
            &repo,
            &["worktree", "add", "-b", "topic", wt.to_str().unwrap()],
        );
        let path = wt.to_str().unwrap();
        let settings = WorktreeSettings {
            delete_unchanged: true,
            keep_when: KeepWhen::TrackedChanges,
            ..Default::default()
        };
        settings
            .save_at(
                &dir.path().join(".hangar/worktree-settings.json"),
                dir.path(),
            )
            .unwrap();
        std::fs::write(wt.join("local"), "preserve").unwrap();
        std::fs::write(wt.join(".env"), "preserve ignored").unwrap();
        let c = candidate(path, &[], &[], dir.path(), &settings, 0, None)
            .unwrap()
            .unwrap();
        assert!(
            remove_candidate_with_guard(&c, &[], &[], dir.path(), 0, None, &|_| Err(error(
                409,
                "sessão abriu"
            )))
            .is_err()
        );
        assert!(wt.join("local").exists());
        assert!(wt.join(".env").exists());
        let next = crate::git::checked(
            &repo,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit-tree",
                "HEAD^{tree}",
                "-p",
                "HEAD",
                "-m",
                "new base",
            ],
            409,
        )
        .unwrap()
        .stdout
        .trim()
        .to_owned();
        let original = reference_head(path, "main").unwrap();
        let changed_base = |_: &str| {
            git(&repo, &["update-ref", "refs/heads/main", &next]);
            Ok(())
        };
        assert!(
            remove_candidate_with_guard(&c, &[], &[], dir.path(), 0, None, &changed_base).is_err()
        );
        assert!(wt.join("local").exists());
        assert!(wt.join(".env").exists());
        git(&repo, &["update-ref", "refs/heads/main", &original]);
        std::fs::write(
            dir.path().join(".hangar/worktrees-removidas.json"),
            "invalid map",
        )
        .unwrap();
        assert!(remove_candidate(&c, &[], &[], dir.path(), 0, None).is_err());
        assert!(wt.join("local").exists());
        assert!(wt.join(".env").exists());
    }
    #[test]
    fn deleted_branch_loses_its_old_config_and_recreated_branch_keeps_new_config() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        let wt = dir.path().join("wt");
        std::fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=T",
                "-c",
                "user.email=t@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ],
        );
        git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feature/topic",
                wt.to_str().unwrap(),
            ],
        );
        git(
            &repo,
            &["config", "branch.feature/topic.hangar-base", "main"],
        );
        let options = RemoveOptions {
            delete_branch: true,
            ..Default::default()
        };
        let out = remove(
            repo.to_str().unwrap(),
            wt.to_str().unwrap(),
            &[],
            &[],
            dir.path(),
            options,
        )
        .unwrap();
        assert_eq!(out["branch_deleted"], true);
        assert_eq!(
            crate::git::command(
                &repo,
                &["config", "--get", "branch.feature/topic.hangar-base"]
            )
            .unwrap()
            .code,
            1
        );
        git(&repo, &["branch", "feature/topic"]);
        git(
            &repo,
            &["config", "branch.feature/topic.hangar-base", "new-base"],
        );
        assert!(!clear_deleted_branch_config(&repo, "feature/topic").unwrap());
        assert_eq!(
            crate::git::command(
                &repo,
                &["config", "--get", "branch.feature/topic.hangar-base"]
            )
            .unwrap()
            .stdout
            .trim(),
            "new-base"
        );
    }
}
