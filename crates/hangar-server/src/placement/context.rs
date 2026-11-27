//! O pedido de abertura visto desta máquina: mesma pasta, trabalho não publicado, pasta sem git.
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::Duration,
};

use hangar_core::project::ProjectId;
use hangar_placement::{OpenContext, Unpublished};
use hangar_workspace::folder_state::{self, GitFolder, Pending};

use tokio::time::Instant;

use crate::routes::AppState;

/// Quanto a escolha de uma pasta vale antes de a lista mostrar a sessão que ela abriu.
const HOLD: Duration = Duration::from_secs(30);

/// Pasta → máquina da abertura recente: duas aberturas seguidas na mesma pasta não se separam
/// enquanto a lista ainda não republicou a primeira.
#[derive(Default)]
pub struct Reservations(Mutex<HashMap<String, (String, Instant)>>);

impl Reservations {
    /// `root` é o `OpenContext::cwd_root` da abertura; `server` a máquina escolhida, a local inclusive.
    pub fn reserve(&self, root: &str, server: &str) {
        let now = Instant::now();
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, (_, at)| now.duration_since(*at) < HOLD);
        map.insert(folder_state::path_key(Path::new(root)), (server.to_owned(), now));
    }

    pub fn lookup(&self, root: &str) -> Option<String> {
        let now = Instant::now();
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&folder_state::path_key(Path::new(root)))
            .filter(|(_, at)| now.duration_since(*at) < HOLD)
            .map(|(server, _)| server.clone())
    }
}

/// O contexto da abertura em `cwd` (cru do pedido; `None` = pasta pessoal). `branch` pedida vence a da
/// pasta. `Err` com código quando a lista ou o git não respondem: sem saber, quem chama não balanceia.
pub async fn build(st: &AppState, reservations: &Reservations, cwd: Option<&str>, branch: Option<&str>, agent: bool)
    -> Result<OpenContext, &'static str> {
    let rows = st.list.discover(None).await.map_err(|_| "placement_sessions_unavailable")?;
    let env = &st.accounts.env;
    let home = hangar_workspace::real(&env.home);
    let cwd = cwd.map(str::trim).filter(|c| !c.is_empty())
        .map_or_else(|| home.clone(), |c| hangar_workspace::real(&crate::worktree_context::expanded(c, env)));
    let dirs: Vec<PathBuf> = rows.iter()
        .filter_map(|r| [&r.git_cwd, &r.worktree_path, &r.cwd].into_iter().flatten().find(|d| !d.is_empty()))
        .map(PathBuf::from).collect();
    let scan = tokio::task::spawn_blocking(move || scan(&cwd, &home, &dirs, folder_state::read)).await
        .map_err(|_| "placement_folder_unreadable")??;
    Ok(context(scan, &st.server_id, reservations, branch, agent))
}

/// O que a pasta do pedido é nesta máquina.
#[derive(Debug, PartialEq)]
struct Scan {
    /// Raiz git com o estado dela; pasta cuja raiz é a pessoal conta como sem git.
    folder: Option<GitFolder>,
    plain_not_home: bool,
    /// Uma sessão daqui trabalha na mesma raiz.
    local_match: bool,
}

fn scan(cwd: &Path, home: &Path, rows: &[PathBuf], read: impl Fn(&Path) -> hangar_workspace::Result<GitFolder>)
    -> Result<Scan, &'static str> {
    // A pasta pessoal nunca prende: quase toda sessão nasce nela, e repositório nela é de configuração.
    let Some(root) = folder_state::git_root(cwd).filter(|r| r != home) else {
        return Ok(Scan { folder: None, plain_not_home: cwd != home, local_match: false });
    };
    let key = folder_state::path_key(&root);
    // Raiz exata, nunca prefixo: outra worktree do mesmo repositório é pasta própria.
    let local_match = rows.iter()
        .filter_map(|d| folder_state::git_root(&hangar_workspace::real(d)))
        .any(|r| folder_state::path_key(&r) == key);
    let folder = read(&root).map_err(|_| "placement_folder_unreadable")?;
    Ok(Scan { folder: Some(folder), plain_not_home: false, local_match })
}

fn context(scan: Scan, local_id: &str, reservations: &Reservations, branch: Option<&str>, agent: bool) -> OpenContext {
    let Scan { folder, plain_not_home, local_match } = scan;
    let cwd_root = folder.as_ref().map(|f| f.root.to_string_lossy().into_owned());
    let same_folder_server = if local_match { Some(local_id.to_owned()) } else { cwd_root.as_deref().and_then(|r| reservations.lookup(r)) };
    OpenContext {
        local_id: local_id.to_owned(),
        // Só pelo `origin`: projeto `dir:` juntaria pastas de nome igual em máquinas diferentes.
        cwd_project: folder.as_ref().and_then(|f| f.origin.as_deref()).and_then(ProjectId::from_origin),
        branch: branch.map(str::to_owned).or_else(|| folder.as_ref().and_then(|f| f.branch.clone())),
        unpublished: folder.as_ref().and_then(|f| f.pending).map(|p| match p {
            Pending::Dirty => Unpublished::Dirty,
            Pending::Ahead => Unpublished::Ahead,
            Pending::NoUpstream => Unpublished::NoUpstream,
        }),
        cwd_root,
        same_folder_server,
        plain_dir_not_home: plain_not_home,
        agent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dirs {
        _base: tempfile::TempDir,
        home: PathBuf,
        repo: PathBuf,
    }

    /// `home/repo` com `.git`, `home/plain` sem; o estado do git vem do leitor de teste.
    fn dirs() -> Dirs {
        let base = tempfile::tempdir().unwrap();
        let home = hangar_workspace::real(base.path()).join("home");
        let repo = home.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::create_dir_all(repo.join("src")).unwrap();
        std::fs::create_dir_all(home.join("plain")).unwrap();
        Dirs { _base: base, home, repo }
    }

    fn folder(root: &Path, pending: Option<Pending>) -> GitFolder {
        GitFolder { root: root.to_path_buf(), origin: Some("git@github.com:jeffer1312/hangar.git".into()), branch: Some("main".into()), pending }
    }

    fn clean(root: &Path) -> hangar_workspace::Result<GitFolder> {
        Ok(folder(root, None))
    }

    #[test]
    fn a_session_in_the_same_root_keeps_the_folder_here_even_from_a_subfolder() {
        let d = dirs();
        let s = scan(&d.repo.join("src"), &d.home, &[d.repo.clone()], clean).unwrap();
        assert!(s.local_match);
        let s = scan(&d.repo, &d.home, &[d.repo.join("src")], clean).unwrap();
        assert!(s.local_match);
        let ctx = context(s, "pc", &Reservations::default(), None, false);
        assert_eq!(ctx.same_folder_server.as_deref(), Some("pc"));
        assert_eq!(ctx.cwd_project, ProjectId::from_origin("https://github.com/jeffer1312/hangar"));
        assert_eq!(ctx.branch.as_deref(), Some("main"));
    }

    #[test]
    fn another_worktree_of_the_same_repo_is_its_own_folder() {
        let d = dirs();
        let other = d.home.join("repo-wt");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join(".git"), "gitdir: ../repo/.git/worktrees/wt").unwrap();
        let s = scan(&other, &d.home, &[d.repo.clone(), d.home.clone()], clean).unwrap();
        assert!(!s.local_match);
    }

    #[test]
    fn the_home_folder_never_pins_even_as_a_repo() {
        let d = dirs();
        let s = scan(&d.home, &d.home, &[d.home.clone()], clean).unwrap();
        assert_eq!(s, Scan { folder: None, plain_not_home: false, local_match: false });
        // Pasta pessoal com repositório de configuração: subpasta sem git dela é pasta comum.
        std::fs::create_dir_all(d.home.join(".git")).unwrap();
        let s = scan(&d.home.join("plain"), &d.home, &[d.home.clone()], clean).unwrap();
        assert_eq!(s, Scan { folder: None, plain_not_home: true, local_match: false });
        let ctx = context(s, "pc", &Reservations::default(), None, false);
        assert!(ctx.plain_dir_not_home && ctx.cwd_root.is_none() && ctx.same_folder_server.is_none());
    }

    #[test]
    fn unpublished_work_and_a_git_failure_come_through() {
        let d = dirs();
        let s = scan(&d.repo, &d.home, &[], |r| Ok(folder(r, Some(Pending::Ahead)))).unwrap();
        assert_eq!(context(s, "pc", &Reservations::default(), None, true).unpublished, Some(Unpublished::Ahead));
        let failed = scan(&d.repo, &d.home, &[], |_| Err(hangar_workspace::unavailable("git")));
        assert_eq!(failed, Err("placement_folder_unreadable"));
    }

    #[test]
    fn a_folder_without_origin_has_no_project() {
        let d = dirs();
        let s = scan(&d.repo, &d.home, &[], |r| Ok(GitFolder { origin: None, ..folder(r, None) })).unwrap();
        assert_eq!(context(s, "pc", &Reservations::default(), Some("feat"), false).cwd_project, None);
    }

    #[test]
    fn the_requested_branch_wins_over_the_folder_one() {
        let d = dirs();
        let s = scan(&d.repo, &d.home, &[], clean).unwrap();
        assert_eq!(context(s, "pc", &Reservations::default(), Some("feat"), false).branch.as_deref(), Some("feat"));
    }

    #[test]
    fn a_recent_reservation_sends_the_folder_to_the_same_machine() {
        let d = dirs();
        let r = Reservations::default();
        r.reserve(&d.repo.to_string_lossy(), "nb");
        let ctx = context(scan(&d.repo, &d.home, &[], clean).unwrap(), "pc", &r, None, false);
        assert_eq!(ctx.same_folder_server.as_deref(), Some("nb"));
        // Sessão daqui na mesma raiz vence a reserva.
        let ctx = context(scan(&d.repo, &d.home, &[d.repo.clone()], clean).unwrap(), "pc", &r, None, false);
        assert_eq!(ctx.same_folder_server.as_deref(), Some("pc"));
    }

    #[tokio::test(start_paused = true)]
    async fn a_reservation_expires_after_thirty_seconds() {
        let r = Reservations::default();
        r.reserve("/x/repo", "nb");
        tokio::time::advance(Duration::from_secs(29)).await;
        assert_eq!(r.lookup("/x/repo/").as_deref(), Some("nb"));
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(r.lookup("/x/repo"), None);
        r.reserve("/y/repo", "vm");
        assert!(r.0.lock().unwrap().get("/x/repo").is_none(), "a vencida sai na próxima reserva");
    }
}
