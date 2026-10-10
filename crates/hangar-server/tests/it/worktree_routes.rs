use crate::fake;
use crate::workspace_fixture;
use fake::{OWNER, client};
use serde_json::Value;
use workspace_fixture::{fixture, refusal};

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

fn scene() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main"]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "--allow-empty", "-m", "i"]);
    git(&repo, &["worktree", "add", "-b", "wt", root.join("repo-wt").to_str().unwrap()]);
    dir
}

#[tokio::test]
async fn owner_lists_in_rust_and_guest_goes_to_python() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let f = fixture(&root, true).await;
    let c = client();
    let url = format!("http://{}/api/worktrees?sizes=false", f.addr);
    let r = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    let wt = &body["repos"][0]["worktrees"][0];
    assert_eq!(body["repos"][0]["repo"], root.join("repo").to_string_lossy().as_ref());
    assert_eq!((wt["branch"].as_str(), wt["sessions"][0].as_str()), (Some("wt"), Some("fixture")));
    let detail = format!("http://{}/api/worktrees/detail?path={}", f.addr, root.join("repo-wt").display());
    let r = c.get(&detail).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(f.hits(), 0);
    let guest = c.get(&url).bearer_auth("convidado").send().await.unwrap();
    assert_eq!(guest.text().await.unwrap(), "python-reserva");
    assert_eq!(f.hits(), 1);
}

#[tokio::test]
async fn path_outside_the_roots_is_refused_like_python() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let f = fixture(&root.join("repo"), true).await;
    let c = client();
    for url in [
        format!("http://{}/api/worktrees/detail?path={}", f.addr, std::env::temp_dir().display()),
        format!("http://{}/api/worktrees?repo={}", f.addr, std::env::temp_dir().display()),
    ] {
        let r = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(r.status(), 403);
        let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
        assert_eq!(body["detail"], "root not allowed");
    }
    // A worktree fora da raiz liberada vale pelo repositório que a registra, que está nela.
    let f = fixture(&root.join("repo"), true).await;
    let wt = format!("http://{}/api/worktrees/detail?path={}", f.addr, root.join("repo-wt").display());
    assert_eq!(c.get(&wt).bearer_auth(OWNER).send().await.unwrap().status(), 200);
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn missing_context_is_a_503_with_code_never_python() {
    let dir = scene();
    let f = fixture(dir.path(), false).await;
    let r = client()
        .get(format!("http://{}/api/worktrees", f.addr))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    let body = refusal(r, "worktrees_context").await;
    assert_eq!(body["detail"]["params"]["motivo"], "status");
    assert_eq!(f.diag("rust.worktrees_failed").await["codigo"], "worktrees_context");
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn malformed_query_keeps_the_fastapi_422() {
    let dir = scene();
    let f = fixture(dir.path(), true).await;
    let c = client();
    for url in [format!("http://{}/api/worktrees?sizes=talvez", f.addr), format!("http://{}/api/worktrees/detail", f.addr)] {
        assert_eq!(c.get(&url).bearer_auth(OWNER).send().await.unwrap().text().await.unwrap(), "python-reserva");
    }
    assert_eq!(f.hits(), 2);
}
