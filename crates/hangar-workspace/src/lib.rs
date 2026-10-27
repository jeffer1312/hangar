//! Operações de arquivos e Git compartilhadas pelo servidor e pelo desktop.
pub mod citations;
pub mod files;
pub mod git;
pub mod process;
pub mod worktrees;
pub mod worktree_settings;
pub mod worktree_merge;
pub mod worktree_cleanup;
mod worktree_residual;
mod process_lifetime;
pub use hangar_api::workspace::WorkspaceError;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
pub type Result<T> = std::result::Result<T, WorkspaceError>;
pub fn error(status: u16, detail: impl Into<String>) -> WorkspaceError {
    WorkspaceError {
        status,
        detail: json!(detail.into()),
        code: None,
    }
}
pub fn file_error(status: u16, code: &str, detail: impl Into<String>) -> WorkspaceError {
    WorkspaceError {
        code: Some(code.into()),
        ..error(status, detail)
    }
}
/// Código de quem não chegou a rodar a operação: pode ir ao Python sem risco de repetir efeito.
pub const UNAVAILABLE: &str = "workspace_unavailable";
pub fn unavailable(detail: impl Into<String>) -> WorkspaceError {
    file_error(503, UNAVAILABLE, detail)
}
/// Vagas cheias: também não rodou, mas é carga, não defeito; não conta como falha do Rust.
pub const BUSY: &str = "workspace_busy";
pub fn busy() -> WorkspaceError {
    file_error(503, BUSY, "vagas cheias")
}
pub fn is_unavailable(e: &WorkspaceError) -> bool {
    matches!(e.code.as_deref(), Some(UNAVAILABLE | BUSY))
}
fn fifty() -> usize {
    50
}
fn fourteen() -> usize {
    14
}
fn yes() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(
    tag = "op",
    content = "args",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Operation {
    HeadInfo {
        cwd: Option<String>,
    },
    BranchOf {
        cwd: Option<String>,
    },
    GitSummary {
        cwd: Option<String>,
    },
    GitDiffstat {
        cwd: Option<String>,
    },
    ListBranches {
        cwd: String,
    },
    SwitchBranch {
        cwd: String,
        branch: String,
    },
    CreateWorktree {
        cwd: String,
        branch: String,
        name: String,
        allowed_root: String,
        #[serde(default)]
        new_branch: bool,
        #[serde(default)]
        base: Option<String>,
    },
    RemoveWorktree {
        cwd: String,
        path: String,
        #[serde(default)]
        force: bool,
    },
    GitAction {
        cwd: String,
        action: String,
    },
    GitLog {
        cwd: String,
        #[serde(default = "fifty")]
        n: usize,
        #[serde(default)]
        grep: Option<String>,
    },
    GitLogSince {
        cwd: String,
        desde: f64,
        #[serde(default = "fourteen")]
        n: usize,
    },
    AssignLanes {
        commits: Vec<Value>,
    },
    ChangedFiles {
        cwd: String,
    },
    FileDiff {
        cwd: String,
        path: String,
    },
    DiscardFile {
        cwd: String,
        path: String,
    },
    CommitFiles {
        cwd: String,
        sha: String,
    },
    CommitFileDiff {
        cwd: String,
        sha: String,
        path: String,
    },
    PathDiff {
        cwd: String,
        path: String,
        escopo: String,
    },
    CommitDiff {
        cwd: String,
        sha: String,
    },
    RevertCommit {
        cwd: String,
        sha: String,
    },
    CherryPick {
        cwd: String,
        sha: String,
    },
    ResetTo {
        cwd: String,
        sha: String,
        mode: String,
    },
    CreateBranchAt {
        cwd: String,
        name: String,
        #[serde(default)]
        sha: Option<String>,
        #[serde(default)]
        switch_after: bool,
    },
    CreateTag {
        cwd: String,
        name: String,
        #[serde(default)]
        sha: Option<String>,
        #[serde(default)]
        message: Option<String>,
    },
    DiffVsWorktree {
        cwd: String,
        sha: String,
    },
    SequencerState {
        cwd: String,
    },
    BranchesContaining {
        cwd: String,
        sha: String,
    },
    FolderStatus {
        cwd: String,
    },
    FolderFetch {
        cwd: String,
    },
    FolderPull {
        cwd: String,
    },
    FolderSwitch {
        cwd: String,
        branch: String,
        sessions: Vec<String>,
        confirm_sessions: bool,
    },
    FolderCreateBranch {
        cwd: String,
        name: String,
        #[serde(default)]
        base: Option<String>,
        checkout: bool,
        sessions: Vec<String>,
        confirm_sessions: bool,
    },
    LastCommitMessage {
        cwd: String,
    },
    Commit {
        cwd: String,
        message: String,
        #[serde(default)]
        paths: Vec<String>,
        #[serde(default)]
        amend: bool,
        #[serde(default)]
        new_branch: Option<String>,
    },
    Push {
        cwd: String,
    },
    ListDir {
        cwd: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default = "yes")]
        so_modificados: bool,
    },
    ReadFile {
        cwd: String,
        path: String,
    },
    ReadAt {
        alvo: String,
        path: String,
    },
    WriteFile {
        cwd: String,
        path: String,
        texto: String,
        #[serde(default)]
        digest_lido: Option<String>,
    },
    WriteAt {
        alvo: String,
        path: String,
        texto: String,
        #[serde(default)]
        digest_lido: Option<String>,
    },
    Search {
        cwd: String,
        q: String,
        mode: String,
    },
    Resolver {
        cwd: String,
        caminhos: Vec<String>,
        #[serde(default = "yes")]
        suffix: bool,
    },
    ScanDir {
        root: String,
        #[serde(default)]
        path: Option<String>,
        roots: Vec<String>,
    },
    MakeDir {
        root: String,
        #[serde(default)]
        path: Option<String>,
        name: String,
        roots: Vec<String>,
    },
    ListRoots {
        roots: Vec<String>,
    },
    CitationCwds {
        jsonl: String,
        needles: Vec<String>,
        #[serde(default)]
        rows: Option<Vec<String>>,
    },
    CitedElsewhere {
        jsonl: String,
        path: String,
        #[serde(default)]
        rows: Option<Vec<String>>,
    },
    FindElsewhere {
        jsonl: String,
        cwd: Option<String>,
        path: String,
        worked: Vec<String>,
        siblings: bool,
        #[serde(default)]
        rows: Option<Vec<String>>,
    },
    ResolveCited {
        cwd: String,
        jsonl: String,
        path: String,
        #[serde(default)]
        write: bool,
    },
    ListWorktrees {
        cwds: Vec<String>,
        sessions: Vec<worktrees::Session>,
        #[serde(default)]
        roots: Option<Vec<String>>,
        #[serde(default)]
        repo: Option<String>,
        #[serde(default = "yes")]
        measure: bool,
        #[serde(default)]
        project_bases: Vec<String>,
    },
    WorktreeStatus {
        path: String,
        sessions: Vec<worktrees::Session>,
        #[serde(default = "yes")]
        measure: bool,
        #[serde(default)]
        project_bases: Vec<String>,
    },
}

impl Operation {
    pub fn is_mutation(&self) -> bool {
        matches!(
            self,
            Self::SwitchBranch { .. }
                | Self::CreateWorktree { .. }
                | Self::RemoveWorktree { .. }
                | Self::DiscardFile { .. }
                | Self::RevertCommit { .. }
                | Self::CherryPick { .. }
                | Self::ResetTo { .. }
                | Self::CreateBranchAt { .. }
                | Self::CreateTag { .. }
                | Self::FolderFetch { .. }
                | Self::FolderPull { .. }
                | Self::FolderSwitch { .. }
                | Self::FolderCreateBranch { .. }
                | Self::Commit { .. }
                | Self::Push { .. }
                | Self::WriteFile { .. }
                | Self::WriteAt { .. }
                | Self::MakeDir { .. }
        ) || matches!(self, Self::GitAction{action,..} if !matches!(action.as_str(), "status"|"log"))
    }
}

pub fn execute(op: Operation) -> Result<Value> {
    use Operation::*;
    match op {
        ListWorktrees {
            cwds,
            sessions,
            roots,
            repo,
            measure,
            project_bases,
        } => Ok(worktrees::list_all(
            &cwds,
            &sessions,
            roots.as_deref(),
            repo.as_deref(),
            measure,
            &project_bases,
        )),
        WorktreeStatus {
            path,
            sessions,
            measure,
            project_bases,
        } => Ok(worktrees::status_of(&path, &sessions, measure, &project_bases)),
        HeadInfo { cwd } => Ok(json!(git::head_info(cwd.as_deref()))),
        BranchOf { cwd } => Ok(json!(git::head_info(cwd.as_deref()).0)),
        GitSummary { cwd } => Ok(git::summary(cwd.as_deref(), false)),
        GitDiffstat { cwd } => Ok(git::summary(cwd.as_deref(), true)),
        ListBranches { cwd } => git::branches(Path::new(&cwd)),
        SwitchBranch { cwd, branch } => git::switch(Path::new(&cwd), &branch),
        CreateWorktree {
            cwd,
            branch,
            name,
            allowed_root,
            new_branch,
            base,
        } => git::create_worktree(
            Path::new(&cwd),
            &branch,
            &name,
            Path::new(&allowed_root),
            new_branch,
            base.as_deref(),
        ),
        RemoveWorktree { cwd, path, force } => git::remove_worktree(Path::new(&cwd), &path, force),
        GitAction { cwd, action } => git::action(Path::new(&cwd), &action),
        GitLog { cwd, n, grep } => git::log(Path::new(&cwd), n, grep.as_deref()),
        GitLogSince { cwd, desde, n } => Ok(git::log_since(Path::new(&cwd), desde, n)),
        AssignLanes { commits } => Ok(git::lanes(commits)),
        ChangedFiles { cwd } => git::changed(Path::new(&cwd)),
        FileDiff { cwd, path } => git::file_diff(Path::new(&cwd), &path),
        DiscardFile { cwd, path } => git::discard(Path::new(&cwd), &path),
        CommitFiles { cwd, sha } => git::commit_files(Path::new(&cwd), &sha),
        CommitFileDiff { cwd, sha, path } => git::commit_file_diff(Path::new(&cwd), &sha, &path),
        PathDiff { cwd, path, escopo } => git::path_diff(Path::new(&cwd), &path, &escopo),
        CommitDiff { cwd, sha } => git::commit_diff(Path::new(&cwd), &sha),
        RevertCommit { cwd, sha } => {
            git::revision_action(Path::new(&cwd), &sha, &["revert", "--no-edit"])
        }
        CherryPick { cwd, sha } => git::revision_action(Path::new(&cwd), &sha, &["cherry-pick"]),
        ResetTo { cwd, sha, mode } => {
            if !["soft", "mixed", "hard"].contains(&mode.as_str()) {
                return Err(error(400, "modo inválido"));
            }
            git::revision_action(Path::new(&cwd), &sha, &["reset", &format!("--{mode}")])
        }
        CreateBranchAt {
            cwd,
            name,
            sha,
            switch_after,
        } => git::create_ref(
            Path::new(&cwd),
            "heads",
            &name,
            sha.as_deref(),
            None,
            switch_after,
        ),
        CreateTag {
            cwd,
            name,
            sha,
            message,
        } => git::create_ref(
            Path::new(&cwd),
            "tags",
            &name,
            sha.as_deref(),
            message.as_deref(),
            false,
        ),
        DiffVsWorktree { cwd, sha } => git::diff_worktree(Path::new(&cwd), &sha),
        SequencerState { cwd } => git::sequencer(Path::new(&cwd)),
        BranchesContaining { cwd, sha } => git::containing(Path::new(&cwd), &sha),
        FolderStatus { cwd } => git::folder_status(Path::new(&cwd)),
        FolderFetch { cwd } => {
            git::network_checked(Path::new(&cwd), &["fetch", "--all", "--prune"])?;
            git::folder_status(Path::new(&cwd))
        }
        FolderPull { cwd } => git::folder_pull(Path::new(&cwd)),
        FolderSwitch {
            cwd,
            branch,
            sessions,
            confirm_sessions,
        } => {
            git::guard_folder(
                &git::folder_status(Path::new(&cwd))?,
                &sessions,
                confirm_sessions,
                true,
            )?;
            git::switch(Path::new(&cwd), &branch)?;
            git::folder_status(Path::new(&cwd))
        }
        FolderCreateBranch {
            cwd,
            name,
            base,
            checkout,
            sessions,
            confirm_sessions,
        } => git::folder_create(
            Path::new(&cwd),
            &name,
            base.as_deref(),
            checkout,
            &sessions,
            confirm_sessions,
        ),
        LastCommitMessage { cwd } => git::last_message(Path::new(&cwd)),
        Commit {
            cwd,
            message,
            paths,
            amend,
            new_branch,
        } => git::commit(
            Path::new(&cwd),
            &message,
            &paths,
            amend,
            new_branch.as_deref(),
        ),
        Push { cwd } => git::push(Path::new(&cwd)),
        ListDir {
            cwd,
            path,
            so_modificados,
        } => files::list(Path::new(&cwd), path.as_deref(), so_modificados),
        ReadFile { cwd, path } => files::read(&files::resolve(Path::new(&cwd), &path)?, &path),
        ReadAt { alvo, path } => files::read(Path::new(&alvo), &path),
        WriteFile {
            cwd,
            path,
            texto,
            digest_lido,
        } => files::write(
            &files::resolve(Path::new(&cwd), &path)?,
            &path,
            &texto,
            digest_lido.as_deref(),
        ),
        WriteAt {
            alvo,
            path,
            texto,
            digest_lido,
        } => files::write(Path::new(&alvo), &path, &texto, digest_lido.as_deref()),
        Search { cwd, q, mode } => files::search(Path::new(&cwd), &q, &mode),
        Resolver {
            cwd,
            caminhos,
            suffix,
        } => files::resolver(Path::new(&cwd), &caminhos, suffix),
        ScanDir { root, path, roots } => files::scan(&root, path.as_deref(), &roots),
        MakeDir {
            root,
            path,
            name,
            roots,
        } => files::mkdir(&root, path.as_deref(), &name, &roots),
        ListRoots { roots } => files::roots(&roots),
        CitationCwds {
            jsonl,
            needles,
            rows,
        } => Ok(citations::cwds(transcript(&jsonl, &rows), &needles)),
        CitedElsewhere { jsonl, path, rows } => {
            Ok(citations::elsewhere(transcript(&jsonl, &rows), &path))
        }
        FindElsewhere {
            jsonl,
            cwd,
            path,
            worked,
            siblings,
            rows,
        } => {
            let jsonl = transcript(&jsonl, &rows);
            let result = if let Some(cwd) = cwd {
                citations::find_elsewhere(jsonl, Path::new(&cwd), &path, &worked, siblings)
            } else {
                citations::elsewhere(jsonl, &path)[0]
                    .as_array()
                    .and_then(|a| a.first())
                    .and_then(Value::as_str)
                    .map(PathBuf::from)
            };
            Ok(json!(result.map(|p| p.to_string_lossy().into_owned())))
        }
        ResolveCited {
            cwd,
            jsonl,
            path,
            write,
        } => Ok(json!(
            citations::resolve(
                Path::new(&cwd),
                citations::Transcript::File(Path::new(&jsonl)),
                &path,
                write
            )?
            .to_string_lossy()
        )),
    }
}

fn transcript<'a>(jsonl: &'a str, rows: &'a Option<Vec<String>>) -> citations::Transcript<'a> {
    match rows {
        Some(rows) => citations::Transcript::Rows(rows),
        None => citations::Transcript::File(Path::new(jsonl)),
    }
}

pub fn real(path: &Path) -> PathBuf {
    real_inner(path, 0)
}

fn real_inner(path: &Path, depth: usize) -> PathBuf {
    let expanded = if path.starts_with("~") {
        home()
            .map(|h| h.join(path.strip_prefix("~").unwrap()))
            .unwrap_or_else(|| path.to_path_buf())
    } else {
        path.to_path_buf()
    };
    if let Ok(real) = std::fs::canonicalize(&expanded) {
        return simplify(real);
    }
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir().unwrap_or_default().join(expanded)
    };
    let mut result = PathBuf::new();
    for part in absolute.components() {
        match part {
            std::path::Component::ParentDir => {
                result.pop();
            }
            std::path::Component::CurDir => {}
            _ => {
                result.push(part.as_os_str());
                if depth < 40
                    && result
                        .symlink_metadata()
                        .is_ok_and(|m| m.file_type().is_symlink())
                    && let Ok(link) = std::fs::read_link(&result)
                {
                    let target = if link.is_absolute() {
                        link
                    } else {
                        result.parent().unwrap_or(Path::new(".")).join(link)
                    };
                    result = real_inner(&target, depth + 1);
                }
            }
        }
    }
    result
}
/// Como o `expanduser` do Python para o próprio usuário: `~outro/...` continua relativo e
/// passa pela trava de `..`, em vez de virar absoluto a partir da pasta do servidor.
pub fn expand_home(path: &str) -> PathBuf {
    let own = |rest: &str| {
        rest.is_empty() || rest.starts_with('/') || cfg!(windows) && rest.starts_with('\\')
    };
    match (path.strip_prefix('~'), home()) {
        (Some(rest), Some(home)) if own(rest) => home.join(rest.trim_start_matches(['/', '\\'])),
        _ => PathBuf::from(path),
    }
}
pub fn home() -> Option<PathBuf> {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from)
}
pub fn simplify(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(p) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{p}"));
        }
        if let Some(p) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(p);
        }
    }
    path
}
