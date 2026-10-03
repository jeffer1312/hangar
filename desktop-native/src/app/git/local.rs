//! O desktop usa o mesmo núcleo de Git que atende o celular.
use hangar_workspace::{Operation, execute};
use serde_json::{Value, json};
use std::path::Path;

pub(super) type Refusal = (u16, String);
fn refusal(e: hangar_workspace::WorkspaceError) -> Refusal {
    (
        e.status,
        e.detail
            .as_str()
            .map(str::to_owned)
            .unwrap_or_else(|| e.detail.to_string()),
    )
}

pub(super) fn toplevel(cwd: &Path) -> Option<std::path::PathBuf> {
    let out = hangar_workspace::git::command(cwd, &["rev-parse", "--show-toplevel"])
        .ok()
        .filter(|o| o.code == 0)?;
    Some(std::path::PathBuf::from(out.stdout.trim())).filter(|p| !p.as_os_str().is_empty())
}

pub(super) fn call(cwd: &Path, op: &super::Op) -> Result<Value, Refusal> {
    use super::Op;
    let path = cwd.to_string_lossy().into_owned();
    let operation = match op {
        Op::Branches => Operation::ListBranches { cwd: path },
        Op::Files => {
            let files = execute(Operation::ChangedFiles { cwd: path.clone() }).map_err(refusal)?;
            let sequencer = execute(Operation::SequencerState { cwd: path }).map_err(refusal)?;
            return Ok(json!({"files":files,"sequencer":sequencer}));
        }
        Op::Diff(p) => Operation::FileDiff {
            cwd: path,
            path: p.clone(),
        },
        Op::Discard(p) => Operation::DiscardFile {
            cwd: path,
            path: p.clone(),
        },
        Op::Commit {
            message,
            paths,
            amend,
        } => Operation::Commit {
            cwd: path,
            message: message.clone(),
            paths: paths.clone(),
            amend: *amend,
            new_branch: None,
        },
        Op::Push => Operation::Push { cwd: path },
        Op::LastMessage => Operation::LastCommitMessage { cwd: path },
        Op::Log(n) => {
            let commits = execute(Operation::GitLog {
                cwd: path.clone(),
                n: *n,
                grep: None,
            })
            .map_err(refusal)?;
            let summary = execute(Operation::GitSummary { cwd: Some(path) }).map_err(refusal)?;
            return Ok(
                json!({"commits":commits,"ahead":summary["ahead"],"behind":summary["behind"]}),
            );
        }
        Op::CommitDiff(sha) => Operation::CommitDiff {
            cwd: path,
            sha: sha.clone(),
        },
        Op::Checkout(branch) => Operation::SwitchBranch {
            cwd: path,
            branch: branch.clone(),
        },
        Op::Stash => Operation::GitAction {
            cwd: path,
            action: "stash".into(),
        },
        Op::CreateBranch(name) => Operation::CreateBranchAt {
            cwd: path,
            name: name.clone(),
            sha: None,
            switch_after: true,
        },
    };
    execute(operation).map_err(refusal)
}

#[cfg(test)]
mod tests {
    #[test]
    fn guards_match_the_backend() {
        assert_eq!(
            hangar_workspace::git::scrub("To https://joao:ghp_x@github.com/a/b.git"),
            "To https://***@github.com/a/b.git"
        );
        assert!(!hangar_workspace::git::cap(&"é".repeat(150_000)).1);
        assert!(hangar_workspace::git::cap(&"é".repeat(200_001)).1);
    }
}
