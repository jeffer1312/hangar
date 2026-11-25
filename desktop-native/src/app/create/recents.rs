//! As pastas recentes da tela sem sessão e as cópias (worktrees) de cada repositório no menu de pasta, como no app do
//! celular (`folderGroups.ts`).
use crate::api::dto::SessionInfo;
use super::super::worktrees::{inside, WorktreeRepo, WorktreeStatus};

/// A pasta pessoal é o "sem projeto": o servidor às vezes manda o caminho inteiro em vez de `~`.
pub(super) fn is_home_dir(cwd: &str) -> bool {
    let p = cwd.trim().trim_end_matches(['/', '\\']);
    if p == "~" || p == "/root" { return true; }
    let parts: Vec<&str> = p.split(['/', '\\']).filter(|s| !s.is_empty()).collect();
    match parts.as_slice() {
        ["home" | "Users", _] => p.starts_with('/'),
        [drive, users, _] => drive.len() == 2 && drive.ends_with(':') && users.eq_ignore_ascii_case("users"),
        _ => false,
    }
}

/// Mesma pasta com outra grafia (barra final, `\`): uma contém a outra.
pub(super) fn same_path(a: &str, b: &str) -> bool { inside(a, b) && inside(b, a) }

/// As pastas das sessões, da mais recente para a mais antiga, sem repetir e sem a pasta pessoal.
pub(in crate::app) fn recent_cwds(sessions: &[SessionInfo]) -> Vec<String> {
    let mut ordered: Vec<&SessionInfo> = sessions.iter().collect();
    ordered.sort_by(|a, b| b.last_activity.unwrap_or(0.).total_cmp(&a.last_activity.unwrap_or(0.)));
    let mut out: Vec<String> = Vec::new();
    for session in ordered {
        let Some(cwd) = session.cwd.as_deref().map(str::trim).filter(|c| !c.is_empty()) else { continue };
        if is_home_dir(cwd) || out.iter().any(|p| same_path(p, cwd)) { continue; }
        out.push(cwd.to_owned());
    }
    out
}

/// Worktree conhecida conta como o repositório dela: a worktree some depois do merge, o projeto fica.
pub(super) fn recent_projects(cwds: &[String], repos: &[WorktreeRepo], exclude: Option<&str>, limit: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for cwd in cwds {
        let project = repos.iter().find(|r| r.worktrees.iter().any(|w| same_path(&w.path, cwd))).map_or(cwd.as_str(), |r| r.repo.as_str());
        if exclude.is_some_and(|e| same_path(project, e)) || out.iter().any(|p| same_path(p, project)) { continue; }
        out.push(project.to_owned());
        if out.len() >= limit { break; }
    }
    out
}

/// As cópias de `repo`: as que ainda importam (com sessão ou não mescladas, as com sessão primeiro) e as já mescladas,
/// que ficam atrás de um clique.
pub(super) fn copies_of<'a>(repos: &'a [WorktreeRepo], repo: &str) -> (Vec<&'a WorktreeStatus>, Vec<&'a WorktreeStatus>) {
    let Some(found) = repos.iter().find(|r| same_path(&r.repo, repo)) else { return (Vec::new(), Vec::new()) };
    let (mut visible, merged): (Vec<_>, Vec<_>) = found.worktrees.iter()
        .filter(|w| w.exists && !same_path(&w.path, &found.repo))
        .partition(|w| !w.sessions.is_empty() || !w.merged);
    visible.sort_by_key(|w| w.sessions.is_empty());
    (visible, merged)
}

/// Pasta listada que é worktree de um repositório também listado: entra dentro dele, não solta na lista.
pub(super) fn nested_in_listed(repos: &[WorktreeRepo], listed: &[&str], path: &str) -> bool {
    repos.iter().any(|r| !same_path(&r.repo, path) && listed.iter().any(|l| same_path(l, &r.repo))
        && r.worktrees.iter().any(|w| same_path(&w.path, path)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(cwd: &str, at: Option<f64>) -> SessionInfo {
        SessionInfo { cwd: Some(cwd.to_owned()), last_activity: at, ..Default::default() }
    }

    fn worktree(path: &str, repo: &str, merged: bool, sessions: &[&str]) -> WorktreeStatus {
        serde_json::from_value(serde_json::json!({
            "path": path, "repo": repo, "exists": true, "merged": merged, "ahead": 0, "dirty": 0, "closed": 0,
            "sessions": sessions,
        })).unwrap()
    }

    fn repo(path: &str, worktrees: Vec<WorktreeStatus>) -> WorktreeRepo { WorktreeRepo { server: String::new(), repo: path.to_owned(), worktrees } }

    #[test]
    fn recent_cwds_orders_by_activity_and_skips_home_and_repeats() {
        let list = [session("/p/a", Some(1.)), session("/p/b", Some(5.)), session("~", Some(9.)), session("/p/b/", Some(3.)),
            session("/p/c", None), session("/home/ana", Some(8.)), session("C:\\Users\\ana", Some(7.))];
        assert_eq!(recent_cwds(&list), ["/p/b", "/p/a", "/p/c"]);
    }

    #[test]
    fn home_dir_is_only_the_personal_folder() {
        assert!(is_home_dir("/home/ana/") && is_home_dir("/Users/ana") && is_home_dir("C:\\Users\\ana") && is_home_dir("/root"));
        assert!(!is_home_dir("/home/ana/proj") && !is_home_dir("/home") && !is_home_dir("/srv/home/x"));
    }

    #[test]
    fn recent_projects_fold_worktrees_and_skip_the_picked_folder() {
        let repos = [repo("/p/hangar", vec![worktree("/w/hangar-menu", "/p/hangar", false, &[])])];
        let cwds = ["/w/hangar-menu", "/p/hangar", "/p/forge", "/p/web", "/p/api"].map(str::to_owned);
        assert_eq!(recent_projects(&cwds, &repos, Some("/p/forge"), 3), ["/p/hangar", "/p/web", "/p/api"]);
    }

    #[test]
    fn copies_split_merged_without_session_and_put_busy_first() {
        let repos = [repo("/p/hangar", vec![
            worktree("/p/hangar", "/p/hangar", false, &[]),
            worktree("/w/old", "/p/hangar", true, &[]),
            worktree("/w/idle", "/p/hangar", false, &[]),
            worktree("/w/busy", "/p/hangar", true, &["s1"]),
        ])];
        let (visible, merged) = copies_of(&repos, "/p/hangar/");
        assert_eq!(visible.iter().map(|w| w.path.as_str()).collect::<Vec<_>>(), ["/w/busy", "/w/idle"]);
        assert_eq!(merged.iter().map(|w| w.path.as_str()).collect::<Vec<_>>(), ["/w/old"]);
        assert!(copies_of(&repos, "/p/other").0.is_empty());
    }

    #[test]
    fn worktree_listed_next_to_its_repo_is_nested() {
        let repos = [repo("/p/hangar", vec![worktree("/p/hangar-menu", "/p/hangar", false, &[])])];
        assert!(nested_in_listed(&repos, &["/p/hangar", "/p/hangar-menu"], "/p/hangar-menu"));
        assert!(!nested_in_listed(&repos, &["/p/hangar-menu"], "/p/hangar-menu"));
        assert!(!nested_in_listed(&repos, &["/p/hangar", "/p/hangar-menu"], "/p/hangar"));
    }
}
