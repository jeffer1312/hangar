use crate::fake;
use crate::workspace_fixture;
use fake::{OWNER, client};
use serde_json::Value;
use workspace_fixture::{fixture, refusal};

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(["-c", "core.hooksPath=", "-c", "commit.gpgSign=false"])
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn scene() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
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
            "i",
        ],
    );
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-b",
            "wt",
            root.join("repo-wt").to_str().unwrap(),
        ],
    );
    dir
}

#[tokio::test]
async fn initialized_submodule_and_ignored_directories_require_visible_confirmation() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let path = root.join("with-submodules");
    let source = root.join("submodule-source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-b", "main"]);
    git(&source, &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "--allow-empty", "-m", "Base"]);
    for name in ["Hooks", "Src/shared"] {
        git(&repo, &["-c", "protocol.file.allow=always", "submodule", "add", source.to_str().unwrap(), name]);
    }
    std::fs::write(repo.join(".gitignore"), ".claude/\nSrc/bin/\ndocs/\n").unwrap();
    git(&repo, &["add", ".gitignore"]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "-am", "Submódulos"]);
    git(&repo, &["worktree", "add", "-b", "with-submodules", path.to_str().unwrap()]);
    git(&path, &["-c", "protocol.file.allow=always", "submodule", "update", "--init", "Hooks"]);
    for name in [".claude", "Src/bin", "docs"] {
        std::fs::create_dir_all(path.join(name)).unwrap();
        std::fs::write(path.join(name).join("local.txt"), "conteúdo local").unwrap();
    }
    let f = fixture(&root, true).await;
    let c = client();
    let mut url = reqwest::Url::parse(&format!("http://{}/api/worktrees/detail", f.addr)).unwrap();
    url.query_pairs_mut().append_pair("path", path.to_str().unwrap());
    let detail: Value = c.get(url).bearer_auth(OWNER).send().await.unwrap().json().await.unwrap();
    assert_eq!(detail["dirty"], 0);
    for name in [".claude/", "Src/bin/", "docs/"] {
        assert!(detail["ignored"].as_array().unwrap().iter().any(|v| v == name), "{detail}");
    }
    for confirm in [false, true] {
        let response = c.post(format!("http://{}/api/worktrees/delete", f.addr)).bearer_auth(OWNER)
            .json(&serde_json::json!({"repo":repo,"path":path,"confirm":confirm,"keep_branch":true}))
            .send().await.unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, if confirm { 200 } else { 409 }, "{body}");
        if !confirm {
            assert_eq!(std::fs::read_to_string(path.join("docs/local.txt")).unwrap(), "conteúdo local");
            assert!(path.join("Hooks/.git").exists());
        } else {
            assert_eq!(std::path::Path::new(body["removed"].as_str().unwrap()), path);
            assert_eq!(body["branch_deleted"], false);
        }
    }
    assert!(!path.exists());
    git(&repo, &["show-ref", "--verify", "refs/heads/with-submodules"]);
    assert!(repo.join("Hooks/.git").exists());
}

#[tokio::test]
async fn initialized_submodule_alone_is_listed_as_loss() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let path = root.join("only-submodule");
    let source = root.join("submodule-source");
    std::fs::create_dir(&source).unwrap();
    git(&source, &["init", "-b", "main"]);
    git(&source, &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "--allow-empty", "-m", "Base"]);
    git(&repo, &["-c", "protocol.file.allow=always", "submodule", "add", source.to_str().unwrap(), "Hooks"]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "-am", "Submódulo"]);
    git(&repo, &["worktree", "add", "-b", "only-submodule", path.to_str().unwrap()]);
    git(&path, &["-c", "protocol.file.allow=always", "submodule", "update", "--init", "Hooks"]);
    let f = fixture(&root, true).await;
    let c = client();
    let mut url = reqwest::Url::parse(&format!("http://{}/api/worktrees/detail", f.addr)).unwrap();
    url.query_pairs_mut().append_pair("path", path.to_str().unwrap());
    let detail: Value = c.get(url).bearer_auth(OWNER).send().await.unwrap().json().await.unwrap();
    assert_eq!(detail["dirty"], 0, "{detail}");
    assert_eq!(detail["ignored"], serde_json::json!(["Hooks/"]), "{detail}");
    for confirm in [false, true] {
        let response = c.post(format!("http://{}/api/worktrees/delete", f.addr)).bearer_auth(OWNER)
            .json(&serde_json::json!({"repo":repo,"path":path,"confirm":confirm,"keep_branch":true}))
            .send().await.unwrap();
        let status = response.status();
        let body: Value = response.json().await.unwrap();
        assert_eq!(status, if confirm { 200 } else { 409 }, "{body}");
    }
    assert!(!path.exists());
}

#[tokio::test]
async fn residual_detail_and_manual_delete_keep_identity_files_and_branch_until_confirmation() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let path = root.join("residual");
    git(&repo, &["worktree", "add", "-b", "residual", path.to_str().unwrap()]);
    std::fs::write(path.join("source.rs"), "fn main() {}\n").unwrap();
    std::fs::create_dir(path.join("node_modules")).unwrap();
    std::fs::write(path.join("node_modules/cache"), "cache").unwrap();
    std::fs::remove_file(path.join(".git")).unwrap();
    let f = fixture(&root, true).await;
    let c = client();
    let mut url = reqwest::Url::parse(&format!("http://{}/api/worktrees/detail", f.addr)).unwrap();
    url.query_pairs_mut().append_pair("path", path.to_str().unwrap());
    let r = c.get(url).bearer_auth(OWNER).send().await.unwrap();
    let status = r.status();
    let detail: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(status, 200, "{detail}");
    assert_eq!(detail["residual"], true);
    assert_eq!(detail["branch"], "residual");
    assert_eq!(detail["merged"], false);
    assert_eq!(detail["degraded"], true);
    assert!(detail["residual_files"].as_array().unwrap().iter().any(|v| v == "source.rs"));
    let ignored = std::path::Path::new("node_modules").join("cache");
    assert!(detail["residual_files"].as_array().unwrap().iter().any(|v| v == ignored.to_string_lossy().as_ref()));
    for confirm in [false, true] {
        let r = c.post(format!("http://{}/api/worktrees/delete", f.addr)).bearer_auth(OWNER)
            .header("content-type", "application/json")
            .body(serde_json::json!({"repo":repo,"path":path,"confirm":confirm}).to_string()).send().await.unwrap();
        let status = r.status();
        let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
        assert_eq!(status, if confirm { 200 } else { 409 }, "{body}");
        if !confirm { assert!(path.join("source.rs").exists()); }
        else { assert_eq!(body["branch_deleted"], false); }
    }
    assert!(!path.exists());
    assert!(!hangar_workspace::worktrees::worktree_paths(repo.to_str().unwrap()).contains(&path.to_string_lossy().into_owned()));
    git(&repo, &["show-ref", "--verify", "refs/heads/residual"]);
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn cleanup_known_repo_is_safe_when_another_discovery_root_is_missing() {
    use std::{sync::Arc, time::Duration};
    use hangar_server::{list::{bridge::{ListBridge, ListEnv}, facts::FactsClient, mux::Mux}, worktree_policy};
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    let dirty = root.join("dirty");
    let residual = root.join("residual");
    for (branch, path) in [("dirty", &dirty), ("residual", &residual)] {
        git(&repo, &["worktree", "add", "-b", branch, path.to_str().unwrap()]);
        std::fs::write(path.join("work.txt"), "trabalho local").unwrap();
    }
    std::fs::remove_file(residual.join(".git")).unwrap();
    let app = axum::Router::new().fallback(|| async { axum::http::StatusCode::NOT_FOUND });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut state = hangar_server::routes::AppState::new(fake::config(upstream, "127.0.0.1"));
    state.origins_home = home.clone();
    state.accounts = hangar_server::accounts::AccountService::new(
        hangar_server::accounts::environment::AccountEnvironment::from_map([
            ("HOME".into(),home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(),home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR".into(),home.join(".claude").to_string_lossy().into_owned()),
            ("CP_SCAN_ROOTS".into(),format!("{},{}",root.display(),root.join("unavailable").display())),
        ].into()));
    let mux = home.join(if cfg!(windows) { "mux.cmd" } else { "mux" });
    std::fs::write(&mux, if cfg!(windows) { "@echo off\r\nexit /b 0\r\n" } else { "#!/bin/sh\nexit 0\n" }).unwrap();
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&mux, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut env = ListEnv::from_env();
    env.dirs = hangar_server::list::bridge::parse_dirs(&serde_json::json!({
        "home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
        "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),
        "omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")
    }).to_string());
    env.mux = Mux::with_program(&mux, Duration::from_secs(5));
    state.list = Arc::new(ListBridge::new(env, FactsClient::new(upstream, fake::SECRET.into())));
    let settings=hangar_workspace::worktree_settings::WorktreeSettings { delete_unchanged: true, ..Default::default() };
    settings.save_at(&home.join(".hangar/worktree-settings.json"), &home).unwrap();
    let wt=root.join("repo-wt");
    let past=std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()-2*86400;
    let git_config=tempfile::NamedTempFile::new().unwrap();
    let admin=std::process::Command::new("git").arg("-C").arg(&wt)
        .args(["rev-parse","--path-format=absolute","--git-path","commondir"])
        .env("GIT_CONFIG_GLOBAL",git_config.path()).env("GIT_CONFIG_NOSYSTEM","1")
        .env_remove("GIT_CONFIG_PARAMETERS").env_remove("GIT_CONFIG_COUNT").output().unwrap();
    assert!(admin.status.success());
    // A descoberta parcial é conferida numa worktree já observada pelo prazo de segurança.
    for path in [&wt, &wt.join(".git"), std::path::Path::new(std::str::from_utf8(&admin.stdout).unwrap().trim())] {
        let mut options=std::fs::OpenOptions::new();options.read(true);
        #[cfg(windows)] {
            use std::os::windows::fs::OpenOptionsExt;
            use windows_sys::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS,FILE_WRITE_ATTRIBUTES,FILE_READ_ATTRIBUTES};
            options.access_mode(FILE_WRITE_ATTRIBUTES|FILE_READ_ATTRIBUTES).custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
        }
        options.open(path).unwrap().set_times(std::fs::FileTimes::new()
            .set_modified(std::time::UNIX_EPOCH+Duration::from_secs(past))).unwrap();
    }
    assert!(hangar_workspace::worktree_cleanup::candidate(wt.to_str().unwrap(),&[],&[],&home,&settings,past,None).unwrap().is_none());
    let rows = hangar_workspace::worktrees::list_all(&[repo.to_string_lossy().into()], &[],
        Some(&[root.to_string_lossy().into(), root.join("unavailable").to_string_lossy().into()]), None, false, &[]);
    assert_eq!(rows[0]["discovery_partial"], true);
    let result = worktree_policy::cleanup_once(Arc::new(state)).await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert_eq!(result.unwrap(), [root.join("repo-wt").to_string_lossy().into_owned()]);
    assert_eq!(std::fs::read_to_string(dirty.join("work.txt")).unwrap(), "trabalho local");
    assert_eq!(std::fs::read_to_string(residual.join("work.txt")).unwrap(), "trabalho local");
    git(&repo, &["show-ref", "--verify", "refs/heads/wt"]);
}

#[tokio::test]
async fn ready_only_batch_keeps_new_local_work_even_when_ignored_loss_was_confirmed() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let path = root.join("bulk");
    git(&repo, &["worktree", "add", "-b", "bulk", path.to_str().unwrap()]);
    std::fs::write(path.join(".gitignore"), "cache\n").unwrap();
    git(&path, &["add", ".gitignore"]);
    git(&path, &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "-m", "ignore"]);
    git(&repo, &["merge", "bulk"]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "--allow-empty", "-m", "next"]);
    std::fs::write(path.join("cache"), "descartável confirmado").unwrap();
    std::fs::write(path.join("new.txt"), "trabalho novo").unwrap();
    let f = fixture(&root, true).await;
    let response = client().post(format!("http://{}/api/worktrees/delete-merged", f.addr))
        .bearer_auth(OWNER).header("content-type", "application/json")
        .body(serde_json::json!({"repo":repo, "paths":[path], "lossy":[path], "confirm":true, "ready_only":true}).to_string())
        .send().await.unwrap();
    assert_eq!(response.status(), 200);
    let result: Value = serde_json::from_slice(&response.bytes().await.unwrap()).unwrap();
    assert_eq!(result["removed"], serde_json::json!([]));
    assert_eq!(std::fs::read_to_string(path.join("new.txt")).unwrap(), "trabalho novo");
    assert_eq!(std::fs::read_to_string(path.join("cache")).unwrap(), "descartável confirmado");
}

#[tokio::test]
async fn t4_context_unreadable_orchestration_preserves_worktree() {
    for warm in [false, true] {
        let dir = scene();
        let root = hangar_workspace::real(dir.path());
        let home = root.join("home");
        let run = home.join(".hangar/orq/t4-run");
        std::fs::create_dir_all(&run).unwrap();
        // estavel: só mantém o vigia vivo por mais de duas horas; não há asserção de tempo.
        let stamp = chrono::Utc::now().to_rfc3339();
        let config = run.join("orq.json");
        std::fs::write(&config, serde_json::json!({"auto":true,"repo":root.join("repo-wt"),"arbiter":"a"}).to_string()).unwrap();
        std::fs::write(run.join("eventos.jsonl"), format!("{}\n", serde_json::json!({"tipo":"execucao_inicio","gid":"t4-group","ts":stamp}))).unwrap();
        std::fs::write(run.join("timeline-t4-run.jsonl"), "").unwrap();
        std::fs::write(run.join("vigia.json"), serde_json::json!({"pid":std::process::id(),"interval_s":3600,"ts":stamp,"arbiter":"a"}).to_string()).unwrap();
        let f = workspace_fixture::fixture_with_home(&root, false, Some(&home)).await;
        if warm {
            let response = client().get(format!("http://{}/api/worktrees?sizes=false", f.addr)).bearer_auth(OWNER).send().await.unwrap();
            assert_eq!(response.status(), 200);
            let body = response.json::<Value>().await.unwrap();
            assert!(body["repos"][0]["worktrees"][0]["sessions"].as_array().unwrap().iter().any(|name| name == "t4-group-orq"));
        }
        std::fs::write(config, "{").unwrap();
        let path = root.join("repo-wt");
        let response = client().post(format!("http://{}/api/worktrees/delete", f.addr)).bearer_auth(OWNER)
            .json(&serde_json::json!({"repo":root.join("repo"),"path":path,"keep_branch":true})).send().await.unwrap();
        assert_eq!(response.status(), 503, "warm={warm}");
        assert!(path.join(".git").exists());
        assert_eq!(f.hits(), 0);
    }
}

#[tokio::test]
async fn t4_context_first_request_after_session_recreation_uses_current_checkout() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    let panes = home.join("panes.txt");
    let pane = |path: &std::path::Path, birth| format!("fixture\t1\t\t{}\t%1\t\tclaude\t{birth}\t0\t0\n", path.display());
    std::fs::write(&panes, pane(&root.join("repo"), 1700000000)).unwrap();
    #[cfg(unix)]
    let mux = {
        let mux = home.join("t4-mux");
        crate::write_executable(&mux, format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\ncat '{}'\n", panes.display()));
        mux
    };
    #[cfg(windows)]
    let mux = {
        let mux = home.join("t4-mux.cmd");
        std::fs::write(&mux, format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\ntype \"{}\"\r\n", panes.display())).unwrap();
        mux
    };
    let f = workspace_fixture::fixture_with_mux(&root, false, Some(&home), Some(&mux)).await;
    let url = format!("http://{}/api/worktrees/detail?path={}", f.addr, root.join("repo-wt").display());
    let first = client().get(&url).bearer_auth(OWNER).send().await.unwrap().json::<Value>().await.unwrap();
    assert_eq!(first["sessions"], serde_json::json!([]));
    std::fs::write(&panes, pane(&root.join("repo-wt"), 1700000001)).unwrap();
    let second = client().get(&url).bearer_auth(OWNER).send().await.unwrap().json::<Value>().await.unwrap();
    assert_eq!(second["sessions"], serde_json::json!(["fixture"]));
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn t4_http_family_methods_and_slashes_never_reach_python() {
    let dir = scene();
    let f = fixture(dir.path(), false).await;
    let c = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    for (path, method, allow) in [
        ("/api/worktrees", "POST", "GET"),
        ("/api/worktrees/detail", "PUT", "GET"),
        ("/api/worktrees/settings", "DELETE", "GET"),
        ("/api/worktrees/create", "GET", "POST"),
        ("/api/worktrees/delete", "GET", "POST"),
        ("/api/worktrees/delete-merged", "GET", "POST"),
    ] {
        let response = c.request(method.parse::<reqwest::Method>().unwrap(), format!("http://{}{path}", f.addr)).send().await.unwrap();
        assert_eq!(response.status(), 405, "{method} {path}");
        assert!(response.headers()["allow"].to_str().unwrap().split(", ").any(|value| value == allow));
        assert_eq!(response.json::<Value>().await.unwrap(), serde_json::json!({"detail":"Method Not Allowed"}));
        let response = c.get(format!("http://{}{path}/?keep=1", f.addr)).send().await.unwrap();
        assert_eq!(response.status(), 307, "{path}");
        assert_eq!(response.headers()["location"], format!("http://{}{path}?keep=1", f.addr));
    }
    assert_eq!(f.hits(), 0);
    assert_eq!(f.context_hits.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn t4_delete_batch_limits_confirmation_and_preserves_degraded_worktrees() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let a = root.join("repo-a");
    let b = root.join("repo-b");
    for (branch, path) in [("a",&a),("b",&b)] {
        git(&repo, &["worktree","add","-b",branch,path.to_str().unwrap()]);
        std::fs::write(path.join(format!("{branch}.txt")), branch).unwrap();
        git(path, &["add",&format!("{branch}.txt")]);
        git(path, &["-c","user.name=T","-c","user.email=t@example.invalid","commit","-m",branch]);
        git(&repo, &["-c","user.name=T","-c","user.email=t@example.invalid","merge","--no-ff","-m","merged",branch]);
    }
    git(&repo, &["-c","user.name=T","-c","user.email=t@example.invalid","commit","--allow-empty","-m","next"]);
    std::fs::write(b.join("new-work.txt"), "preservar").unwrap();
    let f = workspace_fixture::session_fixture(&root, false).await;
    let url = format!("http://{}/api/worktrees/delete-merged", f.addr);
    let invalid = client().post(&url).bearer_auth(OWNER).json(&serde_json::json!({"repo":repo,"confirm":true})).send().await.unwrap();
    assert_eq!(invalid.status(), 422);
    assert_eq!(invalid.json::<Value>().await.unwrap(), serde_json::json!({"detail":"confirmar exige a lista das worktrees mostradas"}));
    let first = client().post(&url).bearer_auth(OWNER).json(&serde_json::json!({"repo":repo,"paths":[a,b]})).send().await.unwrap();
    assert_eq!(first.status(), 200);
    assert_eq!(first.json::<Value>().await.unwrap(), serde_json::json!({"removed":[a]}));
    assert!(!a.exists());
    assert_eq!(std::fs::read_to_string(b.join("new-work.txt")).unwrap(), "preservar");
    assert!(root.join("repo-wt/.git").exists());
    let second = client().post(&url).bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":repo,"paths":[b],"lossy":[b],"confirm":true})).send().await.unwrap();
    assert_eq!(second.status(), 200);
    assert_eq!(second.json::<Value>().await.unwrap(), serde_json::json!({"removed":[b]}));
    assert!(!b.exists());
    let path = root.join("repo-wt");
    let original = std::fs::read(path.join(".git")).unwrap();
    std::fs::write(path.join(".git"), "gitdir: missing\n").unwrap();
    let refused = client().post(format!("http://{}/api/worktrees/delete", f.addr)).bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":repo,"path":path,"confirm":true})).send().await.unwrap();
    let status = refused.status();
    std::fs::write(path.join(".git"), original).unwrap();
    assert_eq!(status, 409);
    assert!(path.exists());
    assert_eq!(f.hits(), 0);
}

#[test]
fn t4_location_native_transcripts_preserve_worktree_and_opening_identity() {
    use hangar_server::list::{bridge::parse_dirs, links::locate, discover::Problems};
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let wt = root.join("repo-wt");
    let wy = root.join("repo-y");
    let gone = root.join("repo-gone");
    git(&repo, &["worktree", "add", "-b", "y", wy.to_str().unwrap()]);
    git(&repo, &["worktree", "add", "-b", "gone", gone.to_str().unwrap()]);
    git(&repo, &["worktree", "remove", gone.to_str().unwrap()]);
    std::fs::create_dir(wt.join("sub")).unwrap();
    std::fs::write(wt.join("a.rs"), "").unwrap();
    std::fs::write(repo.join("note.txt"), "").unwrap();
    let other = root.join("other");
    std::fs::create_dir(&other).unwrap();
    git(&other, &["init", "-b", "main"]);
    git(&other, &["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "--allow-empty", "-m", "other"]);
    let home = root.join("home");
    let dirs = parse_dirs(&serde_json::json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
        "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string()).unwrap();
    let tool = |cwd: &std::path::Path, name, input:Value| serde_json::json!({"type":"assistant","cwd":cwd,
        "message":{"content":[{"type":"tool_use","name":name,"input":input}]}}).to_string();
    let call = |input:Value| serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":input.to_string()}}).to_string();
    let custom = |name, input:String| serde_json::json!({"type":"response_item","payload":{"type":"custom_tool_call","name":name,"input":input}}).to_string();
    let text = |path:&std::path::Path| path.to_string_lossy().into_owned();
    let wt_text = text(&wt);
    let main_text = text(&repo);
    let y_text = text(&wy);
    let gone_text = text(&gone);
    let cases = vec![
        ("claude", &repo, vec![serde_json::json!({"cwd":repo}).to_string(),serde_json::json!({"cwd":wt.join("sub")}).to_string()], "wt", Some(wt_text.as_str()), Some(wt_text.as_str()), false),
        ("claude", &repo, vec![serde_json::json!({"cwd":gone}).to_string()], "", Some(gone_text.as_str()), None, true),
        ("claude", &repo, vec![tool(&repo,"Bash",serde_json::json!({"command":format!("cd '{}' && git status",wt.display())}))], "wt", Some(wt_text.as_str()), Some(wt_text.as_str()), false),
        ("claude", &repo, vec![tool(&repo,"Edit",serde_json::json!({"file_path":wt.join("a.rs")})),tool(&repo,"Write",serde_json::json!({"file_path":repo.join("note.txt")}))], "wt", Some(wt_text.as_str()), Some(wt_text.as_str()), false),
        ("claude", &repo, vec![tool(&repo,"Bash",serde_json::json!({"command":format!("git -C '{}' log -1",wt.display())})),tool(&repo,"Bash",serde_json::json!({"command":format!("cd '{}'; ls; cd '{}' && git status",wt.display(),repo.display())}))], "wt", Some(wt_text.as_str()), Some(wt_text.as_str()), false),
        ("claude", &wt, vec![tool(&wt,"Bash",serde_json::json!({"command":format!("cd '{}' && ls",wy.display())}))], "wt", Some(wt_text.as_str()), None, false),
        ("codex", &wt, vec![call(serde_json::json!({"cmd":format!("cd '{}' && ls",repo.display())}))], "wt", Some(wt_text.as_str()), None, false),
        ("claude", &repo, vec![tool(&repo,"Bash",serde_json::json!({"command":format!("cd '{}' && ls",wt.display())})),tool(&repo,"Bash",serde_json::json!({"command":format!("git -C '{}' status",wy.display())}))], "y", Some(y_text.as_str()), Some(y_text.as_str()), false),
        ("claude", &repo, vec![tool(&wt,"Bash",serde_json::json!({"command":format!("cd '{}' && ls",wt.display())})),serde_json::json!({"type":"user","cwd":repo}).to_string()], "main", None, None, false),
        ("claude", &repo, vec![tool(&repo,"Bash",serde_json::json!({"command":"W=/x; cd $W && cd - && cd build"}))], "main", None, None, false),
        ("claude", &repo, vec![tool(&repo,"Bash",serde_json::json!({"command":format!("cd '{}' && ls",gone.display())}))], "", Some(gone_text.as_str()), None, true),
        ("codex", &repo, vec![custom("exec",format!("tools.exec_command({{cmd:\"git status\", workdir:{:?}}})",wt_text)),call(serde_json::json!({"cmd":format!("cd '{}' && ls",root.join("other").display())}))], "wt", Some(wt_text.as_str()), Some(wt_text.as_str()), false),
        ("codex", &repo, vec![call(serde_json::json!({"cmd":format!("cd {} && ls",wt.display()),"workdir":repo}))], "wt", Some(wt_text.as_str()), Some(wt_text.as_str()), false),
        ("codex", &repo, vec![custom("exec",format!("tools.exec_command({{cmd:\"ls\", workdir:{:?}}})",gone_text))], "", Some(gone_text.as_str()), None, true),
        ("codex", &repo, vec![call(serde_json::json!({"cmd":"ls","workdir":root.join("other/missing")}))], "main", None, None, false),
        ("codex", &repo, vec![custom("apply_patch",format!("*** Begin Patch\n*** Delete File: {}\n*** End Patch",root.join("other/a.txt").display()))], "main", None, None, false),
    ];
    for (index, (provider, cwd, lines, branch, worktree, git_cwd, gone)) in cases.into_iter().enumerate() {
        let file = root.join(format!("location-{index}.jsonl"));
        std::fs::write(&file, lines.join("\n") + "\n").unwrap();
        let location = locate(provider, cwd.to_str(), file.to_str(), &dirs, &mut Problems::default());
        assert_eq!(location.branch.as_deref().unwrap_or(""), branch, "case {index}: {location:?}");
        assert_eq!(location.worktree_path.as_deref(), worktree, "case {index}");
        assert_eq!(location.git_cwd.as_deref(), git_cwd, "case {index}");
        assert_eq!(location.worktree_gone, gone, "case {index}");
        assert_eq!(location.worktree, !gone && worktree.is_some(), "case {index}");
    }
    for (index, malformed) in [r#"["function_call", "cwd"]"#, "42", r#"{"type":"response_item","payload":["function_call"]}"#,
        r#"{"type":"response_item","payload":"custom_tool_call"}"#, r#"{"type":"response_item","payload":{"type":"function_call","arguments":"{\\"cmd\\": \\"cd /"#].into_iter().enumerate() {
        let file = root.join(format!("malformed-{index}.jsonl"));
        std::fs::write(&file, malformed).unwrap();
        for provider in ["claude", "codex"] {
            let location = locate(provider, Some(&main_text), file.to_str(), &dirs, &mut Problems::default());
            assert_eq!(location.branch.as_deref(), Some("main"));
            assert_eq!(location.worktree_path, None);
            assert!(!location.worktree_gone);
        }
    }
    for provider in ["claude", "codex"] {
        for transcript in [None, root.join("missing.jsonl").to_str()] {
            let location = locate(provider, Some(&main_text), transcript, &dirs, &mut Problems::default());
            assert_eq!(location.branch.as_deref(), Some("main"));
            assert_eq!(location.worktree_path, None);
            assert!(!location.worktree_gone);
        }
    }
}

#[cfg(unix)]
#[test]
fn t4_remaining_reader_io_failure_keeps_the_opening_checkout() {
    use std::os::unix::fs::PermissionsExt;
    use hangar_server::list::{bridge::parse_dirs, discover::Problems, links::locate};
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let unreadable = root.join("reader-error.jsonl");
    std::fs::write(&unreadable, "{}\n").unwrap();
    std::fs::set_permissions(&unreadable, std::fs::Permissions::from_mode(0o000)).unwrap();
    assert!(unreadable.is_file());
    let error = std::fs::File::open(&unreadable).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    let home = root.join("home");
    let dirs = parse_dirs(&serde_json::json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
        "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string()).unwrap();
    for provider in ["claude", "codex"] {
        let mut problems = Problems::default();
        let location = locate(provider, repo.to_str(), unreadable.to_str(), &dirs, &mut problems);
        assert_eq!(location.branch.as_deref(), Some("main"), "{provider}: {location:?}");
        assert!(!location.worktree);
        assert_eq!(location.worktree_path, None);
        assert!(!location.worktree_gone);
        assert_eq!(location.git_cwd, None);
        assert!(!problems.into_vec().is_empty(), "{provider}: o erro real do leitor precisa ser observado");
    }
}

#[tokio::test]
async fn t4_delete_keeps_conversations_until_resume_and_preserves_uuid_collisions() {
    use hangar_server::{accounts::environment::AccountEnvironment, list::{mux::Mux, procs::SystemProcs}, session_open::{archive, facts::LocalFacts}};
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    #[cfg(unix)]
    let mux = { let path = home.join("t4-resume-mux"); crate::write_executable(&path, "#!/bin/sh\nexit 0\n"); path };
    #[cfg(windows)]
    let mux = { let path = home.join("t4-resume-mux.cmd"); std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").unwrap(); path };
    let f = workspace_fixture::fixture_with_mux(&root, false, Some(&home), Some(&mux)).await;
    let repo = root.join("repo");
    let path = root.join("repo-wt");
    let projects = home.join(".claude/projects");
    let old_project = hangar_workspace::worktrees::sanitize_cwd(path.to_str().unwrap());
    let new_project = hangar_workspace::worktrees::sanitize_cwd(repo.to_str().unwrap());
    let sid = "d0000001-0000-4000-8000-000000000000";
    let from = projects.join(&old_project);
    let to = projects.join(&new_project);
    std::fs::create_dir_all(from.join(sid)).unwrap();
    std::fs::create_dir_all(to.join(sid)).unwrap();
    let original = serde_json::json!({"cwd":path,"text":"conversa preservada"}).to_string();
    std::fs::write(from.join(format!("{sid}.jsonl")), &original).unwrap();
    std::fs::write(from.join(sid).join("same.txt"), "origem").unwrap();
    std::fs::write(to.join(sid).join("same.txt"), "destino").unwrap();
    std::fs::write(from.join(sid).join("only-source.txt"), "resultado").unwrap();
    let response = client().post(format!("http://{}/api/worktrees/delete", f.addr)).bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":repo,"path":path,"keep_branch":true})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let removed = response.json::<Value>().await.unwrap();
    assert_eq!(removed["moved"], 0);
    assert_eq!(removed["branch_deleted"], false);
    assert!(!path.exists());
    assert_eq!(std::fs::read_to_string(from.join(format!("{sid}.jsonl"))).unwrap(), original);
    assert!(!to.join(format!("{sid}.jsonl")).exists());
    let map = hangar_workspace::worktrees::try_removed_at(&home.join(".hangar/worktrees-removidas.json")).unwrap();
    assert_eq!(map.len(), 1);
    let (removed_path, removed_repo) = map.iter().next().unwrap();
    assert_eq!(std::path::Path::new(removed_path), path);
    assert_eq!(std::path::Path::new(removed_repo), repo);
    let runtime = tokio::runtime::Handle::current();
    let root_copy = root.clone();
    tokio::task::spawn_blocking(move || {
        let env: std::collections::BTreeMap<String,String> = [
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR".into(), home.join(".claude").to_string_lossy().into_owned()),
            ("CP_SCAN_ROOTS".into(), root_copy.to_string_lossy().into_owned()),
        ].into();
        let facts = LocalFacts::new(AccountEnvironment::from_map(env.clone()), runtime)
            .with_terminal(Mux::with_program(mux, std::time::Duration::from_secs(5)), std::sync::Arc::new(SystemProcs::default()));
        let where_ = archive::Where { home:&home, env:&env, server_projects:&projects, config_dirs:&[], live:&[] };
        let plan = archive::plan(&where_, &facts, &old_project, sid, archive::ArchiveBody { provider:"claude".into(), ..Default::default() }).unwrap();
        assert!(plan.relocated);
        assert_eq!(std::path::Path::new(&plan.request.cwd), repo);
        assert_eq!(plan.found.transcript, from.join(format!("{sid}.jsonl")));
        std::fs::write(to.join(format!("{sid}.jsonl")), "outra conversa").unwrap();
        let conflict = archive::move_conversation(&projects, &old_project, &new_project, sid, None, None).unwrap_err();
        assert_eq!(conflict.status, 409);
        assert_eq!(std::fs::read_to_string(from.join(format!("{sid}.jsonl"))).unwrap(), original);
        assert_eq!(std::fs::read_to_string(to.join(format!("{sid}.jsonl"))).unwrap(), "outra conversa");
        std::fs::remove_file(to.join(format!("{sid}.jsonl"))).unwrap();
        assert!(archive::move_conversation(&projects, &old_project, &new_project, sid, None, None).unwrap());
        assert_eq!(std::fs::read_to_string(to.join(format!("{sid}.jsonl"))).unwrap(), original);
        assert!(!from.join(format!("{sid}.jsonl")).exists());
        assert_eq!(std::fs::read_to_string(to.join(sid).join("same.txt")).unwrap(), "destino");
        assert_eq!(std::fs::read_to_string(from.join(sid).join("same.txt")).unwrap(), "origem");
        assert_eq!(std::fs::read_to_string(to.join(sid).join("only-source.txt")).unwrap(), "resultado");
        let resumed = archive::plan(&where_, &facts, "-", sid, archive::ArchiveBody { provider:"claude".into(), ..Default::default() }).unwrap();
        assert_eq!(resumed.found.transcript, to.join(format!("{sid}.jsonl")));
        assert_eq!(std::path::Path::new(&resumed.request.cwd), repo);
    }).await.unwrap();
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn t4_private_forwarding_keeps_method_and_guest_refusals_native() {
    use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, list::{bridge::{ListBridge, ListEnv, parse_dirs}, facts::FactsClient, mux::Mux}};
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    #[cfg(unix)]
    let mux = { let path = home.join("t4-private-mux"); crate::write_executable(&path, "#!/bin/sh\nexit 0\n"); path };
    #[cfg(windows)]
    let mux = { let path = home.join("t4-private-mux.cmd"); std::fs::write(&path, "@echo off\r\nexit /b 0\r\n").unwrap(); path };
    let upstream = "127.0.0.1:9".parse().unwrap();
    let mut state = hangar_server::routes::AppState::new(fake::config(upstream, "127.0.0.1"));
    state.origins_home = home.clone();
    state.accounts = AccountService::new(AccountEnvironment::from_map([
        ("HOME".into(),home.to_string_lossy().into_owned()), ("USERPROFILE".into(),home.to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR".into(),home.join(".claude").to_string_lossy().into_owned()), ("CP_SCAN_ROOTS".into(),root.to_string_lossy().into_owned()),
    ].into()));
    let mut env = ListEnv::from_env();
    env.dirs = parse_dirs(&serde_json::json!({"home":home,"claude":home.join(".claude"),"codex_home":home.join(".codex"),
        "pi_sessions":home.join(".pi/agent/sessions"),"omp_config":home.join(".omp"),"omp_agent":home.join(".omp/agent"),"kimi_home":home.join(".kimi-code")}).to_string());
    env.mux = Mux::with_program(mux, std::time::Duration::from_secs(5));
    state.list = std::sync::Arc::new(ListBridge::new(env, FactsClient::new(upstream, fake::SECRET.into())));
    let app = hangar_server::routes::terminal_router(std::sync::Arc::new(state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    let url = format!("http://{addr}/__hangar_server/forwarded");
    let unauthorized = client().get(&url).header("x-hangar-path", "/api/worktrees/settings").send().await.unwrap();
    assert_eq!(unauthorized.status(), 404);
    for (secret, forwarded) in [("wrong-secret", "127.0.0.1"), (fake::SECRET, "198.51.100.21")] {
        let response = client().put(&url).header("x-hangar-internal", secret).header("x-forwarded-for", forwarded)
            .header("x-hangar-path", "/api/worktrees/settings").json(&serde_json::json!({"location":"/should-not-exist"})).send().await.unwrap();
        assert_eq!(response.status(), 404);
    }
    let redirects = reqwest::Client::builder().no_proxy().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let slash = redirects.get(format!("{url}?keep=1")).header("x-hangar-internal", fake::SECRET)
        .header("x-hangar-path", "/api/worktrees/create/").send().await.unwrap();
    assert_eq!(slash.status(), 307);
    assert_eq!(slash.headers()["location"], format!("http://{addr}/api/worktrees/create?keep=1"));
    let mut seen = Vec::new();
    for (path, guest) in [("/api/worktrees/create", false), ("/api/worktrees/settings", true)] {
        let mut request = client().get(&url).header("x-hangar-internal", fake::SECRET).header("x-hangar-path", path);
        if guest { request = request.header("x-hangar-forwarded-guest", "1"); }
        let response = request.send().await.unwrap();
        seen.push((response.status().as_u16(), response.json::<Value>().await.unwrap()));
    }
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert_eq!(seen, vec![(405, serde_json::json!({"detail":"Method Not Allowed"})),
        (403, serde_json::json!({"detail":"convidado não acessa worktrees"}))]);
    assert!(!home.join(".hangar/worktree-settings.json").exists());
}

#[tokio::test]
async fn t4_http_encoded_detail_keeps_its_operation() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let f = fixture(&root, false).await;
    let response = client().get(format!("http://{}/api/worktrees/%64etail?path={}", f.addr, root.join("repo-wt").display()))
        .bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let body = response.json::<Value>().await.unwrap();
    assert_eq!(body["branch"], "wt", "{body}");
    assert_eq!(body["path"], root.join("repo-wt").to_string_lossy().as_ref());
    let settings = serde_json::json!({"location":root.join("chosen"),"inactive_days":null,"delete_merged":false,"delete_unchanged":false,"keep_when":"uncommitted-changes"});
    let response = client().put(format!("http://{}/api/worktrees/%73ettings", f.addr)).bearer_auth(OWNER).json(&settings).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap(), settings);
    let response = client().post(format!("http://{}/api/worktrees/%63reate", f.addr)).bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":root.join("repo"),"branch":"encoded","name":"encoded","new_branch":true,"base":"main"})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let made = response.json::<Value>().await.unwrap();
    let path = root.join("chosen").join("repo").join("encoded");
    assert_eq!(made["path"], path.to_string_lossy().as_ref());
    git(&path, &["show-ref", "--verify", "refs/heads/encoded"]);
    let response = client().post(format!("http://{}/api/worktrees/%64elete", f.addr)).bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":root.join("repo"),"path":path,"keep_branch":true})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert!(!path.exists());
    let response = client().post(format!("http://{}/api/worktrees/%66etch", f.addr)).bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":root.join("repo")})).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap(), serde_json::json!({"ok":true}));
    let response = client().get(format!("http://{}/api/worktrees/%2564etail", f.addr)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 404);
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn t4_http_native_payload_and_query_refusals_preserve_their_published_format() {
    let dir = scene();
    let f = fixture(dir.path(), false).await;
    for (method, path, body) in [
        ("POST", "create", "{"), ("POST", "create", "{}"),
        ("POST", "delete", "null"), ("POST", "delete", r#"{"repo":"x","path":"x","confirm":"true"}"#),
        ("POST", "delete-merged", r#"{"repo":"x","extra":true}"#),
        ("PUT", "settings", "{}"),
    ] {
        for content_type in ["application/json", "text/plain"] {
            let response = client().request(method.parse::<reqwest::Method>().unwrap(), format!("http://{}/api/worktrees/{path}", f.addr))
                .bearer_auth(OWNER).header("content-type", content_type).body(body).send().await.unwrap();
            assert_eq!(response.status(), 422, "{method} {path} {content_type}");
            assert_eq!(response.json::<Value>().await.unwrap(), serde_json::json!({"detail":"corpo de worktrees inválido"}));
        }
    }
    for path in ["/api/worktrees?sizes=talvez", "/api/worktrees/detail"] {
        let response = client().get(format!("http://{}{path}", f.addr)).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(), 422);
        assert_eq!(response.json::<Value>().await.unwrap(), serde_json::json!({"detail":"parâmetros de worktrees inválidos"}));
    }
    let response = client().post(format!("http://{}/api/worktrees/create", f.addr)).bearer_auth(OWNER)
        .body(" ".repeat(64 * 1024 + 1)).send().await.unwrap();
    assert_eq!(response.status(), 422);
    assert_eq!(response.json::<Value>().await.unwrap(), serde_json::json!({"detail":"corpo de worktrees inválido"}));
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn t4_http_owner_cookie_login_and_invites_keep_global_boundaries() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    std::fs::create_dir(&home).unwrap();
    #[cfg(unix)]
    let mux = {
        let path = home.join("t4-policy-mux");
        crate::write_executable(&path, format!("#!/bin/sh\ncase \"$1\" in\nlist-panes) printf 'fixture\\t1\\t\\t{}\\t%%1\\t\\tclaude\\t1700000000\\t0\\t0\\n' ;;\ndisplay-message) printf '1700000000\\n' ;;\nesac\n", root.join("repo").display()));
        path
    };
    #[cfg(windows)]
    let mux = {
        let path = home.join("t4-policy-mux.cmd");
        std::fs::write(&path, format!("@echo off\r\nif \"%~1\"==\"list-panes\" echo fixture\t1\t\t{}\t%%1\t\tclaude\t1700000000\t0\t0\r\nif \"%~1\"==\"display-message\" echo 1700000000\r\nexit /b 0\r\n", root.join("repo").display())).unwrap();
        path
    };
    let f = workspace_fixture::policy_fixture(&root, &home, &mux).await;
    let policy = f.policy.as_ref().unwrap();
    policy.login.store.create("Ana", root.to_str().unwrap(), false, true, "ana".into(), "login-token".into()).unwrap();
    let full = policy.share.store.create_redeemed("fixture", Some("t:1700000000".into()), hangar_server::share::store::Kind::Share).unwrap().1;
    let view = policy.share.store.create_redeemed("fixture", Some("t:1700000000".into()), hangar_server::share::store::Kind::Pair).unwrap().1;
    let read = client().get(format!("http://{}/api/worktrees?sizes=false", f.addr)).header("cookie", format!("cp_token={OWNER}")).send().await.unwrap();
    assert_eq!(read.status(), 200);
    let write = client().post(format!("http://{}/api/worktrees/create", f.addr)).header("cookie", format!("cp_token={OWNER}")).json(&serde_json::json!({})).send().await.unwrap();
    assert_eq!(write.status(), 401);
    for (addr, token, code, message, status) in [
        (f.addr, "login-token", "erro_fora_do_convidado", "fora do acesso do convidado", 403),
        (policy.guest, full.as_str(), "erro_fora_do_convite", "fora da sessao compartilhada", 403),
        (policy.guest, view.as_str(), "erro_fora_do_convite", "fora da sessao compartilhada", 403),
        (policy.guest, OWNER, "erro_nao_autorizado", "unauthorized", 401),
    ] {
        for (method, path) in [("GET", "/api/worktrees"), ("POST", "/api/worktrees/delete")] {
            let response = client().request(method.parse::<reqwest::Method>().unwrap(), format!("http://{addr}{path}")).bearer_auth(token)
                .json(&serde_json::json!({})).send().await.unwrap();
            assert_eq!(response.status(), status);
            assert_eq!(response.json::<Value>().await.unwrap(), serde_json::json!({"detail":{"code":code,"params":{},"msg":message}}));
        }
    }
    assert_eq!(f.hits(), 0);
}

#[test]
fn t4_external_route_paths_and_methods_do_not_panic() {
    use proptest::{prelude::*, test_runner::{Config, RngSeed, TestRunner}};
    let mut runner = TestRunner::new(Config { cases:128, rng_seed:RngSeed::Fixed(0x40102), ..Config::default() });
    runner.run(&(".{0,128}", any::<bool>()), |(suffix, write)| {
        let method = if write { axum::http::Method::POST } else { axum::http::Method::GET };
        let path = format!("/api/worktrees/{suffix}");
        prop_assert!(hangar_server::worktree_routes::matches(&method, &path));
        let unrelated = format!("/api/worktrees-other/{suffix}");
        prop_assert!(!hangar_server::worktree_routes::matches(&method, &unrelated));
        Ok(())
    }).unwrap();
}

#[tokio::test]
async fn t4_http_auth_refusals_never_reach_python() {
    let dir = scene();
    let f = fixture(dir.path(), false).await;
    for (index, (method, path)) in [
        ("GET", "/api/worktrees"), ("GET", "/api/worktrees/detail"),
        ("GET", "/api/worktrees/settings"), ("PUT", "/api/worktrees/settings"),
        ("POST", "/api/worktrees/create"), ("POST", "/api/worktrees/delete"),
        ("POST", "/api/worktrees/delete-merged"),
    ].into_iter().enumerate() {
        let response = client().request(method.parse::<reqwest::Method>().unwrap(), format!("http://{}{path}", f.addr))
            .header("x-forwarded-for", format!("198.51.100.{}", index + 1)).send().await.unwrap();
        assert_eq!(response.status(), 401, "{method} {path}");
        assert_eq!(response.json::<Value>().await.unwrap(), serde_json::json!({"detail":{"code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}));
    }
    assert_eq!(f.hits(), 0);
    assert_eq!(f.context_hits.load(std::sync::atomic::Ordering::Relaxed), 0);
}

#[tokio::test]
async fn owner_lists_in_rust_and_invalid_token_is_refused_locally() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let f = fixture(&root, true).await;
    let c = client();
    let url = format!("http://{}/api/worktrees?sizes=false", f.addr);
    let r = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    let wt = &body["repos"][0]["worktrees"][0];
    assert_eq!(
        body["repos"][0]["repo"],
        root.join("repo").to_string_lossy().as_ref()
    );
    assert_eq!(
        (wt["branch"].as_str(), wt["sessions"][0].as_str()),
        (Some("wt"), Some("fixture"))
    );
    let detail = format!(
        "http://{}/api/worktrees/detail?path={}",
        f.addr,
        root.join("repo-wt").display()
    );
    let r = c.get(&detail).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(f.hits(), 0);
    let guest = c.get(&url).bearer_auth("convidado").send().await.unwrap();
    assert_eq!(guest.status(), 401);
    assert_eq!(guest.json::<Value>().await.unwrap(), serde_json::json!({"detail":{"code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}));
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn path_outside_the_roots_is_refused_like_python() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let f = fixture(&root.join("repo"), true).await;
    let c = client();
    for url in [
        format!(
            "http://{}/api/worktrees/detail?path={}",
            f.addr,
            std::env::temp_dir().display()
        ),
        format!(
            "http://{}/api/worktrees?repo={}",
            f.addr,
            std::env::temp_dir().display()
        ),
    ] {
        let r = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(r.status(), 403);
        let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
        assert_eq!(body["detail"], "root not allowed");
    }
    // A worktree fora da raiz liberada vale pelo repositório que a registra, que está nela.
    let f = fixture(&root.join("repo"), true).await;
    let wt = format!(
        "http://{}/api/worktrees/detail?path={}",
        f.addr,
        root.join("repo-wt").display()
    );
    assert_eq!(
        c.get(&wt).bearer_auth(OWNER).send().await.unwrap().status(),
        200
    );
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn missing_native_context_is_a_503_with_code_never_python() {
    let dir = scene();
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("missing-multiplexer");
    let f = workspace_fixture::fixture_with_mux(dir.path(), true, Some(home.path()), Some(&missing)).await;
    let r = client()
        .get(format!("http://{}/api/worktrees", f.addr))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    let body = refusal(r, "worktrees_context").await;
    assert_eq!(body["detail"]["params"]["motivo"], "sessoes");
    assert_eq!(
        f.diag("rust.worktrees_failed").await["codigo"],
        "worktrees_context"
    );
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn malformed_query_is_422_without_returning_to_python() {
    let dir = scene();
    let f = fixture(dir.path(), true).await;
    let c = client();
    for url in [
        format!("http://{}/api/worktrees?sizes=talvez", f.addr),
        format!("http://{}/api/worktrees/detail", f.addr),
    ] {
        assert_eq!(
            c.get(&url)
                .bearer_auth(OWNER)
                .send()
                .await
                .unwrap()
                .status(),
            422
        );
    }
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn settings_are_owner_only_strict_and_persist_in_explicit_home() {
    let home = tempfile::tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(fake::config(
        "127.0.0.1:9".parse().unwrap(),
        "127.0.0.1",
    ));
    state.origins_home = home.path().into();
    let addr = fake::spawn_state(state).await;
    let url = format!("http://{addr}/api/worktrees/settings");
    let c = client();
    let r = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let defaults: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(
        defaults,
        serde_json::json!({"location":"","inactive_days":null,"delete_merged":false,"delete_unchanged":false,"keep_when":"uncommitted-changes"})
    );
    let saved = serde_json::json!({"location":"~/my-worktrees","inactive_days":8,"delete_merged":true,"delete_unchanged":false,"keep_when":"any-local-files"});
    assert_eq!(
        c.put(&url)
            .bearer_auth(OWNER)
            .header("content-type", "application/json")
            .body(saved.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let r = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&r.text().await.unwrap()).unwrap(),
        saved
    );
    assert_eq!(
        serde_json::from_slice::<Value>(
            &std::fs::read(home.path().join(".hangar/worktree-settings.json")).unwrap()
        )
        .unwrap(),
        saved
    );
    for body in [
        serde_json::json!({}),
        {
            let mut bad = saved.clone();
            bad["location"] = serde_json::json!("relative");
            bad
        },
        serde_json::json!({"location":"/","inactive_days":8,"delete_merged":false,"delete_unchanged":false,"keep_when":"tracked-changes"}),
        serde_json::json!({"location":"","inactive_days":0,"delete_merged":false,"delete_unchanged":false,"keep_when":"tracked-changes"}),
    ] {
        assert_eq!(
            c.put(&url)
                .bearer_auth(OWNER)
                .header("content-type", "application/json")
                .body(body.to_string())
                .send()
                .await
                .unwrap()
                .status(),
            422
        );
    }
    assert_ne!(
        c.put(&url)
            .bearer_auth("convidado")
            .header("content-type", "application/json")
            .body(defaults.to_string())
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
}

#[tokio::test]
async fn configured_location_is_used_by_public_creation_and_manual_delete_stays_in_rust() {
    let dir = scene();
    let home = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let f = workspace_fixture::fixture_with_home(&root, true, Some(home.path())).await;
    let c = client();
    let settings = serde_json::json!({"location":home.path().join("chosen"),"inactive_days":null,"delete_merged":false,"delete_unchanged":false,"keep_when":"uncommitted-changes"});
    let put = c
        .put(format!("http://{}/api/worktrees/settings", f.addr))
        .bearer_auth(OWNER)
        .header("content-type", "application/json")
        .body(settings.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(put.status(), 200);
    let create = serde_json::json!({"repo":root.join("repo"),"branch":"created-in-rust","name":"future","new_branch":true,"base":"main"});
    let r = c
        .post(format!("http://{}/api/worktrees/create", f.addr))
        .bearer_auth(OWNER)
        .header("content-type", "application/json")
        .body(create.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let created: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    let path = home.path().join("chosen").join("repo").join("future");
    let actual=std::path::Path::new(created["path"].as_str().unwrap());
    assert_eq!(std::fs::canonicalize(actual).unwrap(),std::fs::canonicalize(&path).unwrap());
    assert!(path.join(".git").is_file());
    let mut future_settings = settings.clone();
    future_settings["location"] = serde_json::json!(home.path().join("future"));
    let update = c.put(format!("http://{}/api/worktrees/settings", f.addr)).bearer_auth(OWNER).json(&future_settings).send().await.unwrap();
    assert_eq!(update.status(), 200);
    let future = c.post(format!("http://{}/api/worktrees/create", f.addr)).bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":root.join("repo"),"branch":"future-in-rust","name":"second","new_branch":true,"base":"main"})).send().await.unwrap();
    assert_eq!(future.status(), 200);
    let future_body = future.json::<Value>().await.unwrap();
    let future_path = home.path().join("future").join("repo").join("second");
    assert_eq!(future_body["path"], future_path.to_string_lossy().as_ref());
    assert!(path.join(".git").is_file());
    assert!(future_path.join(".git").is_file());
    let remove = serde_json::json!({"repo":root.join("repo"),"path":path,"keep_branch":true});
    let r = c
        .post(format!("http://{}/api/worktrees/delete", f.addr))
        .bearer_auth(OWNER)
        .header("content-type", "application/json")
        .body(remove.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap());
    assert!(!path.exists());
    git(
        &root.join("repo"),
        &["show-ref", "--verify", "refs/heads/created-in-rust"],
    );
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn missing_configured_root_is_ignored_like_python() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing");
    let f = fixture(&missing, true).await;
    let r = client()
        .get(format!("http://{}/api/worktrees?sizes=false", f.addr))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert!(body["warning"].is_null());
    assert_eq!(body["repos"], serde_json::json!([]));
}

#[test]
fn missing_validated_root_still_marks_discovery_incomplete() {
    let dir = tempfile::tempdir().unwrap();
    let roots = vec![dir.path().join("missing").to_string_lossy().into_owned()];
    let repos = hangar_workspace::worktrees::list_all(&[], &[], Some(&roots), None, false, &[]);
    assert_eq!(repos,serde_json::json!([]));
    assert!(hangar_workspace::worktrees::discovery_problem(&roots).is_some());
}

#[tokio::test]
async fn owner_cleans_a_missing_registration_even_after_its_branch_was_deleted() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let path = root.join("orphan");
    git(&repo, &["worktree", "add", "-b", "orphan", path.to_str().unwrap()]);
    std::fs::remove_dir_all(&path).unwrap();
    git(&repo, &["update-ref", "-d", "refs/heads/orphan"]);
    let f = fixture(&root, true).await;
    let r = client().post(format!("http://{}/api/worktrees/delete", f.addr))
        .bearer_auth(OWNER).header("content-type", "application/json")
        .body(serde_json::json!({"repo": repo, "path": path, "keep_branch": true}).to_string())
        .send().await.unwrap();
    let status = r.status();
    let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(std::path::Path::new(body["removed"].as_str().unwrap()), path);
    assert_eq!(body["branch_deleted"], false);
    assert!(!hangar_workspace::worktrees::worktree_paths(repo.to_str().unwrap()).iter().any(|p| std::path::Path::new(p) == path));
    assert!(root.join("repo-wt/.git").is_file());
    assert_eq!(f.hits(), 0);
}
#[tokio::test]
async fn merged_batch_accepts_a_repository_subdirectory() {
    let dir = scene();
    let root = hangar_workspace::real(dir.path());
    let repo = root.join("repo");
    let wt = root.join("merged-batch");
    git(
        &repo,
        &["worktree", "add", "-b", "batch", wt.to_str().unwrap()],
    );
    git(
        &wt,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "batch",
        ],
    );
    git(
        &repo,
        &[
            "-c",
            "user.name=T",
            "-c",
            "user.email=t@example.invalid",
            "merge",
            "--no-ff",
            "batch",
            "-m",
            "merged",
        ],
    );
    std::fs::create_dir(repo.join("subdir")).unwrap();
    let home = tempfile::tempdir().unwrap();
    let f = workspace_fixture::fixture_with_home(&root, true, Some(home.path())).await;
    let expected=std::fs::canonicalize(&wt).unwrap();
    let body = serde_json::json!({"repo":repo.join("subdir"),"paths":[wt]});
    let r = client()
        .post(format!("http://{}/api/worktrees/delete-merged", f.addr))
        .bearer_auth(OWNER)
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let body: Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    let removed=body["removed"].as_array().unwrap();
    assert_eq!(removed.len(),1);
    let removed=std::path::Path::new(removed[0].as_str().unwrap());
    let actual=removed.parent().unwrap().canonicalize().unwrap().join(removed.file_name().unwrap());
    assert_eq!(actual,expected);
    assert!(!wt.exists());
}

#[tokio::test]
async fn anonymous_worktree_requests_reach_auth_and_record_failures() {
    use std::sync::{Arc,atomic::{AtomicUsize,Ordering}};
    use axum::{Router,http::StatusCode};
    let hits=Arc::new(AtomicUsize::new(0)); let seen=hits.clone();
    let upstream=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let address=upstream.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(upstream,Router::new().fallback(move |uri: axum::http::Uri| {let seen=seen.clone();async move {
        if uri.path().starts_with("/api/worktrees") { seen.fetch_add(1,Ordering::SeqCst); }
        StatusCode::UNAUTHORIZED
    }})).await.unwrap(); });
    let home=tempfile::tempdir().unwrap();
    let mut state=hangar_server::routes::AppState::new(fake::config(address,"127.0.0.1")); state.origins_home=home.path().into();
    let state=Arc::new(state); let app=hangar_server::routes::router(state.clone());
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); let addr=listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener,app.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap(); });
    let methods=[(reqwest::Method::GET,"settings"),(reqwest::Method::PUT,"settings"),(reqwest::Method::POST,"delete"),(reqwest::Method::POST,"delete-merged"),(reqwest::Method::POST,"create")];
    let ip="198.51.100.76";
    for n in 0..8 {
        let (method,tail)=&methods[n%methods.len()];
        let mut req=client().request(method.clone(),format!("http://{addr}/api/worktrees/{tail}")).header("x-forwarded-for",ip);
        if n%2==1 {req=req.bearer_auth("token-errado");}
        assert_eq!(req.send().await.unwrap().status(),401);
    }
    assert_eq!(hits.load(Ordering::SeqCst),0);
    assert!(!state.auth.is_owner(ip,Some(OWNER.as_bytes())),"as recusas entram no bloqueio de autenticação");
}

#[tokio::test]
async fn list_uses_native_sources_when_python_context_is_unavailable() {
    let dir=scene();
    let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let response=client().get(format!("http://{}/api/worktrees?sizes=false",f.addr))
        .bearer_auth(OWNER).send().await.unwrap();
    let status=response.status();
    let text=response.text().await.unwrap();
    assert_eq!(status,200,"{text}");
    let body:Value=serde_json::from_str(&text).unwrap();
    assert!(body["repos"].as_array().unwrap().iter().any(|repo|repo["repo"]==root.join("repo").to_string_lossy().as_ref()),"{body}");
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_http_wrong_method_is_local_and_does_not_require_auth() {
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let response=client().get(format!("http://{}/api/worktrees/fetch",f.addr))
        .header("origin","https://example.test").send().await.unwrap();
    assert_eq!(response.status(),405);
    assert_eq!(response.headers()["allow"],"POST");
    assert_eq!(response.headers()["access-control-allow-origin"],"*");
    assert_eq!(response.json::<Value>().await.unwrap(),serde_json::json!({"detail":"Method Not Allowed"}));
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_http_trailing_slash_redirects_before_auth() {
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let c=reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().unwrap();
    let response=c.post(format!("http://{}/api/worktrees/fetch/",f.addr)).send().await.unwrap();
    assert_eq!(response.status(),307);
    assert_eq!(response.headers()["location"],format!("http://{}/api/worktrees/fetch",f.addr));
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_http_preflight_is_local_without_credentials() {
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let response=client().request(reqwest::Method::OPTIONS,format!("http://{}/api/worktrees/fetch",f.addr))
        .header("origin","https://example.test").header("access-control-request-method","POST").send().await.unwrap();
    assert_eq!(response.status(),200);
    assert_eq!(response.headers()["access-control-allow-origin"],"*");
    assert_eq!(response.text().await.unwrap(),"OK");
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_auth_failures_keep_the_shared_block_and_retry_header() {
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    for _ in 0..8 {
        let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr))
            .header("x-forwarded-for","198.51.100.213").bearer_auth("wrong-owner")
            .json(&serde_json::json!({"repo":root.join("repo")})).send().await.unwrap();
        assert_eq!(response.status(),401);
    }
    let blocked=client().post(format!("http://{}/api/worktrees/fetch",f.addr))
        .header("x-forwarded-for","198.51.100.213").bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":root.join("repo")})).send().await.unwrap();
    assert_eq!(blocked.status(),429);
    assert_eq!(blocked.headers()["retry-after"],"30");
    assert_eq!(blocked.json::<Value>().await.unwrap()["detail"]["code"],"erro_muitas_tentativas");
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_rejects_a_body_over_100_mib_before_reading_it_or_contacting_python() {
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let mut connection=tokio::net::TcpStream::connect(f.addr).await.unwrap();
    connection.write_all(format!("POST /api/worktrees/fetch HTTP/1.1\r\nHost: {}\r\nAuthorization: Bearer {}\r\nOrigin: https://example.test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",f.addr,OWNER,100*1024*1024+1).as_bytes()).await.unwrap();
    let mut bytes=Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(5),connection.read_to_end(&mut bytes)).await.unwrap().unwrap();
    let response=String::from_utf8(bytes).unwrap();
    assert!(response.starts_with("HTTP/1.1 413"),"{response}");
    assert!(response.to_ascii_lowercase().contains("access-control-allow-origin: *"),"{response}");
    assert!(response.contains("request body too large"));
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_final_review_does_not_wait_for_shared_write_slots() {
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let capacity=f.workspace_slots.available_permits() as u32;
    let held=f.workspace_slots.clone().acquire_many_owned(capacity).await.unwrap();
    let response=tokio::time::timeout(std::time::Duration::from_secs(5),
        client().post(format!("http://{}/api/worktrees/fetch",f.addr)).bearer_auth(OWNER)
            .json(&serde_json::json!({"repo":root.join("repo")})).send()).await;
    drop(held);
    let response=response.expect("o fetch esperou na fila das escritas que já estavam ocupadas").unwrap();
    assert_eq!(response.status(),200);
    assert_eq!(response.json::<Value>().await.unwrap(),serde_json::json!({"ok":true}));
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_final_review_declared_size_is_refused_before_auth_without_counting_failures() {
    use tokio::io::{AsyncReadExt,AsyncWriteExt};
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    for _ in 0..8 {
        let mut connection=tokio::net::TcpStream::connect(f.addr).await.unwrap();
        connection.write_all(format!("POST /api/worktrees/fetch HTTP/1.1\r\nHost: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",f.addr,100*1024*1024+1).as_bytes()).await.unwrap();
        let mut bytes=Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5),connection.read_to_end(&mut bytes)).await.unwrap().unwrap();
        let response=String::from_utf8(bytes).unwrap();
        assert!(response.starts_with("HTTP/1.1 413"),"{response}");
    }
    let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr)).bearer_auth(OWNER)
        .json(&serde_json::json!({"repo":root.join("repo")})).send().await.unwrap();
    assert_eq!(response.status(),200);
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_compression_identity_keeps_the_original_vary_header() {
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr)).bearer_auth(OWNER)
        .header("accept-encoding","identity").json(&serde_json::json!({"repo":root.join("repo")})).send().await.unwrap();
    assert_eq!(response.status(),200);
    assert_eq!(response.headers().get("vary").and_then(|v|v.to_str().ok()),Some("Accept-Encoding"));
    assert_eq!(response.json::<Value>().await.unwrap(),serde_json::json!({"ok":true}));
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_compression_small_success_keeps_original_gzip_behavior() {
    use std::io::Read;
    let dir=scene(); let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let c=reqwest::Client::builder().no_gzip().no_proxy().build().unwrap();
    let response=c.post(format!("http://{}/api/worktrees/fetch",f.addr)).bearer_auth(OWNER)
        .header("accept-encoding","gzip").json(&serde_json::json!({"repo":root.join("repo")})).send().await.unwrap();
    assert_eq!(response.status(),200);
    assert_eq!(response.headers().get("content-encoding").and_then(|v|v.to_str().ok()),Some("gzip"));
    assert_eq!(response.headers()["vary"],"Accept-Encoding");
    let bytes=response.bytes().await.unwrap();
    let mut decoded=String::new();
    flate2::read::GzDecoder::new(bytes.as_ref()).read_to_string(&mut decoded).unwrap();
    assert_eq!(decoded,"{\"ok\":true}");
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_is_native_even_when_python_context_is_unavailable() {
    let dir=scene();
    let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr))
        .bearer_auth(OWNER).json(&serde_json::json!({"repo":root.join("repo")})).send().await.unwrap();
    let status=response.status();
    let text=response.text().await.unwrap();
    assert_eq!(status,200,"{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),serde_json::json!({"ok":true}),"{text}");
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn fetch_refuses_invalid_body_before_git_or_python() {
    let dir=scene();
    let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    for body in [serde_json::json!({}),serde_json::json!({"repo":""}),serde_json::json!({"repo":3}),serde_json::json!({"repo":root.join("repo"),"extra":true})] {
        let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr))
            .bearer_auth(OWNER).json(&body).send().await.unwrap();
        assert_eq!(response.status(),422,"{body}");
    }
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn native_fetch_updates_and_prunes_only_remote_references() {
    let dir=scene();
    let root=hangar_workspace::real(dir.path());
    let repo=root.join("repo");
    let remote=root.join("remote.git");
    git(&repo,&["config","core.hooksPath",root.join("no-hooks").to_str().unwrap()]);
    git(&root,&["init","--bare",remote.to_str().unwrap()]);
    git(&repo,&["remote","add","origin",remote.to_str().unwrap()]);
    git(&repo,&["push","origin","main"]);
    git(&repo,&["branch","obsolete"]);
    git(&repo,&["push","origin","obsolete"]);
    git(&repo,&["fetch","--all"]);
    git(&repo,&["push","origin","--delete","obsolete"]);
    git(&repo,&["update-ref","refs/remotes/origin/obsolete","refs/heads/main"]);
    git(&repo,&["show-ref","--verify","refs/remotes/origin/obsolete"]);
    std::fs::create_dir(repo.join("subdir")).unwrap();
    let f=workspace_fixture::fixture(&root,false).await;
    let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr))
        .bearer_auth(OWNER).json(&serde_json::json!({"repo":repo.join("subdir")})).send().await.unwrap();
    let status=response.status();
    let text=response.text().await.unwrap();
    assert_eq!(status,200,"{text}");
    assert_eq!(serde_json::from_str::<Value>(&text).unwrap_or(Value::Null),serde_json::json!({"ok":true}),"{text}");
    let pruned=std::process::Command::new("git").arg("-C").arg(&repo)
        .args(["show-ref","--verify","refs/remotes/origin/obsolete"]).output().unwrap();
    assert!(!pruned.status.success());
    git(&repo,&["show-ref","--verify","refs/heads/obsolete"]);
    git(&repo,&["show-ref","--verify","refs/heads/main"]);
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn native_fetch_preserves_large_validation_errors() {
    let dir=scene();
    let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let extra="x".repeat(8*1024*1024+1024);
    let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr))
        .bearer_auth(OWNER).json(&serde_json::json!({"repo":root.join("repo"),"extra":extra})).send().await.unwrap();
    assert_eq!(response.status(),422);
    let body:Value=response.json().await.unwrap();
    assert_eq!(body["detail"][0]["type"],"extra_forbidden");
    assert_eq!(body["detail"][0]["loc"],serde_json::json!(["body","extra"]));
    assert_eq!(body["detail"][0]["input"].as_str().unwrap(),extra);
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn anonymous_fetch_is_refused_without_python_handler() {
    let dir=scene();
    let root=hangar_workspace::real(dir.path());
    let f=workspace_fixture::fixture(&root,false).await;
    let response=client().post(format!("http://{}/api/worktrees/fetch",f.addr))
        .json(&serde_json::json!({"repo":root.join("repo")})).send().await.unwrap();
    assert_eq!(response.status(),401);
    assert_eq!(f.hits(),0);
}

#[tokio::test]
async fn manual_delete_refuses_unknown_terminal_inventory() {
    let dir=scene(); let root=hangar_workspace::real(dir.path()); let home=tempfile::tempdir().unwrap();
    let missing=home.path().join(if cfg!(windows) { "vanishing-mux.cmd" } else { "vanishing-mux" });
    let called=home.path().join("inventory-read");
    let script=if cfg!(windows) {
        format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\nif exist \"{}\" exit /b 1\r\ntype nul > \"{}\"\r\nexit /b 0\r\n",called.display(),called.display())
    } else {
        format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\n[ -e '{}' ] && exit 1\n: > '{}'\nexit 0\n",called.display(),called.display())
    };
    std::fs::write(&missing,script).unwrap();
    #[cfg(unix)] { use std::os::unix::fs::PermissionsExt; std::fs::set_permissions(&missing,std::fs::Permissions::from_mode(0o700)).unwrap(); }
    let f=workspace_fixture::fixture_with_mux(&root,true,Some(home.path()),Some(&missing)).await;
    let path=root.join("repo-wt");
    let body=serde_json::json!({"repo":root.join("repo"),"path":path,"confirm":true});
    let r=client().post(format!("http://{}/api/worktrees/delete",f.addr)).bearer_auth(OWNER).header("content-type","application/json").body(body.to_string()).send().await.unwrap();
    assert_eq!(r.status(),503);
    let body:Value=serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(body["detail"],"terminais de worktrees indisponíveis");
    assert!(path.exists());
}
