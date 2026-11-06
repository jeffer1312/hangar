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
    hangar_workspace::worktree_settings::WorktreeSettings { delete_unchanged: true, ..Default::default() }
        .save_at(&home.join(".hangar/worktree-settings.json"), &home).unwrap();
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
    assert_eq!(hits.load(Ordering::SeqCst),8);
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
