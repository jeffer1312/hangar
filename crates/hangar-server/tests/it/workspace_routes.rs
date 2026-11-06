use crate::fake;
use crate::workspace_fixture;
use fake::{OWNER, SECRET, client};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use workspace_fixture::refusal;

// A fixture mantém o HOME e o executável do Mux vivos durante os pedidos.
async fn fixture(root: &std::path::Path) -> (std::net::SocketAddr, Arc<AtomicUsize>, workspace_fixture::Fixture) {
    let f = workspace_fixture::session_fixture(root, true).await;
    (f.addr, f.hits.clone(), f)
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test]
async fn p01_owner_reads_and_saves_without_using_python_io() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ação.txt"), "original").unwrap();
    let (addr, hits, _fixture) = fixture(dir.path()).await;
    let c = client();
    let url = format!("http://{addr}/api/sessions/fixture/files/read?path=ação.txt");
    let read: Value = serde_json::from_str(
        &c.get(&url)
            .bearer_auth(OWNER)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(read["text"], "original");
    let save = c
        .post(format!("http://{addr}/api/sessions/fixture/files/write"))
        .bearer_auth(OWNER)
        .header("content-type", "application/json")
        .body(json!({"path":"ação.txt","text":"alterada","digest":read["digest"]}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(save.status(), 200);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("ação.txt")).unwrap(),
        "alterada"
    );
    assert_eq!(hits.load(Ordering::Relaxed), 0);
    let refused = c.get(&url).bearer_auth("convidado").send().await.unwrap();
    assert_eq!(refused.status(), 401);
    assert_eq!(refused.json::<Value>().await.unwrap(), json!({"detail":{
        "code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}));
    assert_eq!(hits.load(Ordering::Relaxed), 0);
}

fn reader_asset_url(endpoint: &str, path: &str, root: bool) -> reqwest::Url {
    let mut url = reqwest::Url::parse(endpoint).unwrap();
    url.query_pairs_mut().append_pair("path", path);
    if root { url.query_pairs_mut().append_pair("root", "1"); }
    url
}

#[tokio::test]
async fn p01_reader_asset_without_its_own_citation_uses_only_the_authorized_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    std::fs::create_dir_all(root.join("docs/assets")).unwrap();
    image::RgbaImage::new(2, 2).save(root.join("docs/assets/example.png")).unwrap();
    let image = std::fs::read(root.join("docs/assets/example.png")).unwrap();
    std::fs::write(root.join("fixture.jsonl"), json!({"cwd":root,"text":"docs/guide.md"}).to_string()).unwrap();
    std::fs::write(dir.path().join("outside.png"), &image).unwrap();
    std::fs::create_dir(root.join(".git")).unwrap();
    std::fs::write(root.join(".git/private.png"), &image).unwrap();
    let (addr, hits, _fixture) = fixture(&root).await;
    let c = client();
    let endpoint = format!("http://{addr}/api/sessions/fixture/file");
    assert_eq!(c.get(reader_asset_url(&endpoint, "docs/assets/example.png", false)).bearer_auth(OWNER).send().await.unwrap().status(), 403);
    for path in ["docs/assets/example.png".to_owned(), root.join("docs/assets/example.png").to_string_lossy().into_owned()] {
        let reply = c.get(reader_asset_url(&endpoint, &path, true)).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(reply.status(), 200);
        assert_eq!(reply.headers()["content-type"], "image/png");
        assert_eq!(reply.bytes().await.unwrap().as_ref(), image.as_slice());
    }
    for (path, expected) in [("../outside.png", 400), (".git/private.png", 403)] {
        assert_eq!(c.get(reader_asset_url(&endpoint, path, true)).bearer_auth(OWNER).send().await.unwrap().status(), expected);
    }
    let outside = dir.path().join("outside.png").to_string_lossy().into_owned();
    assert_eq!(c.get(reader_asset_url(&endpoint, &outside, true)).bearer_auth(OWNER).send().await.unwrap().status(), 400);
    assert_eq!(hits.load(Ordering::Relaxed), 0);
    let refused = c.get(reader_asset_url(&endpoint, "docs/assets/example.png", true)).bearer_auth("convidado").send().await.unwrap();
    assert_eq!(refused.status(), 401);
    assert_eq!(refused.json::<Value>().await.unwrap(), json!({"detail":{
        "code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}));
    assert_eq!(hits.load(Ordering::Relaxed), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn reader_root_asset_cannot_follow_a_symlink_outside_the_root() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace");
    std::fs::create_dir(&root).unwrap();
    std::fs::write(dir.path().join("outside.png"), b"private-fixture").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside.png"), root.join("alias.png")).unwrap();
    let (addr, _, _fixture) = fixture(&root).await;
    let response = client().get(reader_asset_url(&format!("http://{addr}/api/sessions/fixture/file"), "alias.png", true))
        .bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn private_route_is_unreachable_on_the_public_listener() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _, _fixture) = fixture(dir.path()).await;
    let response = client()
        .post(format!("http://{addr}/__hangar_server/workspace"))
        .header("x-hangar-internal", SECRET)
        .header("content-type", "application/json")
        .body(json!({"op":"head_info","args":{"cwd":dir.path()}}).to_string())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn cited_video_range_and_cache_still_require_authorization() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("clip.mp4"), b"0123456789").unwrap();
    std::fs::write(
        dir.path().join("fixture.jsonl"),
        json!({"cwd":dir.path(),"text":"clip.mp4"}).to_string(),
    )
    .unwrap();
    let (addr, _, _fixture) = fixture(dir.path()).await;
    let c = client();
    let url = format!("http://{addr}/api/sessions/fixture/file?path=clip.mp4");
    let response = c
        .get(&url)
        .bearer_auth(OWNER)
        .header("range", "bytes=2-5")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.headers()["content-range"], "bytes 2-5/10");
    let etag = response.headers()["etag"].clone();
    assert_eq!(response.text().await.unwrap(), "2345");
    assert_eq!(
        c.get(&url)
            .bearer_auth(OWNER)
            .header("if-none-match", etag)
            .send()
            .await
            .unwrap()
            .status(),
        304
    );
    let blocked = c
        .get(format!(
            "http://{addr}/api/sessions/fixture/file?path=ausente.mp4"
        ))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(blocked.status(), 403);
}

#[tokio::test]
async fn g02_invalid_external_write_body_is_rejected_before_any_mutation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ação.txt"), "original").unwrap();
    std::fs::write(dir.path().join("fixture.jsonl"), json!({"cwd":dir.path(),"text":"ação.txt"}).to_string()).unwrap();
    let (addr, hits, _fixture) = fixture(dir.path()).await;
    let result = client().post(format!("http://{addr}/api/sessions/fixture/file/text"))
        .bearer_auth(OWNER).header("content-type", "application/json")
        .body(json!({"path":"ação.txt", "text":"perdida", "digest":"antigo", "extra":true}).to_string()).send().await.unwrap();
    assert_eq!(result.status(), 422);
    assert_eq!(result.json::<Value>().await.unwrap(), json!({"detail":[{
        "type":"extra_forbidden","loc":["body","extra"],"msg":"Extra inputs are not permitted","input":true}]}));
    assert_eq!(hits.load(Ordering::Relaxed), 0);
    assert_eq!(std::fs::read_to_string(dir.path().join("ação.txt")).unwrap(), "original");
}

#[tokio::test]
async fn private_gate_rejects_forwarded_clients_and_wrong_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _, _fixture) = fixture(dir.path()).await;
    let c = client();
    let health: Value = serde_json::from_str(&c.get(format!("http://{addr}/__hangar_server/health")).send().await.unwrap().text().await.unwrap()).unwrap();
    let private = health["terminal_address"].as_str().unwrap();
    for (secret, forwarded) in [("errado", "127.0.0.1"), (SECRET, "203.0.113.9")] {
        let r = c.post(format!("http://{private}/__hangar_server/workspace")).header("x-hangar-internal", secret)
            .header("x-forwarded-for", forwarded).body("corpo inválido").send().await.unwrap();
        assert_eq!(r.status(), 404);
    }
    let r = c.post(format!("http://{private}/__hangar_server/workspace")).header("x-hangar-internal", SECRET)
        .body(json!({"op":"head_info","args":{"cwd":dir.path()}}).to_string()).send().await.unwrap();
    assert_eq!(r.status(), 200);
}

#[tokio::test]
async fn workspace_responses_keep_cors_for_other_servers() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ação.txt"), "original").unwrap();
    let (addr, _, _fixture) = fixture(dir.path()).await;
    let result = client().get(format!("http://{addr}/api/sessions/fixture/files/read?path=ação.txt"))
        .bearer_auth(OWNER).header("origin", "http://outro-servidor.invalid").send().await.unwrap();
    assert_eq!(result.status(), 200);
    assert_eq!(result.headers().get("access-control-allow-origin").map(|v|v.to_str().unwrap()), Some("*"));
    assert_eq!(result.headers().get("access-control-expose-headers").map(|v|v.to_str().unwrap()), Some("ETag"));
}

#[tokio::test]
async fn ranges_preserve_python_validation_and_merge_overlaps() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("clip.mp4"), b"0123456789").unwrap();
    std::fs::write(dir.path().join("fixture.jsonl"), json!({"cwd":dir.path(),"text":"clip.mp4"}).to_string()).unwrap();
    let (addr, _, _fixture) = fixture(dir.path()).await;
    let url = format!("http://{addr}/api/sessions/fixture/file?path=clip.mp4");
    let c = client();
    for (range, status) in [("invalid", 400), ("items=0-1", 400), ("bytes=7-2", 400), ("bytes=20-30", 416), ("Bytes = 2-5", 206), ("bytes=invalid,2-5",206), ("bytes=5-4",206), ("bytes=-0",416)] {
        let response = c.get(&url).bearer_auth(OWNER).header("range", range).send().await.unwrap();
        assert_eq!(response.status().as_u16(), status, "{range}");
    }
    let response = c.get(&url).bearer_auth(OWNER).header("range", "bytes=0-3,2-5").send().await.unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.headers()["content-range"], "bytes 0-5/10");
    assert_eq!(response.text().await.unwrap(), "012345");
}

#[tokio::test]
async fn g02_public_mutation_rejects_private_context_fields_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("arquivo.txt"), "original").unwrap();
    for args in [&["init", "-b", "main"][..], &["config", "user.name", "Teste"], &["config", "user.email", "teste@example.invalid"], &["add", "."], &["commit", "-m", "Inicial"]] {
        assert!(std::process::Command::new("git").arg("-C").arg(dir.path()).args(args).output().unwrap().status.success());
    }
    std::fs::write(dir.path().join("arquivo.txt"), "alteração preservada").unwrap();
    let (addr, hits, _fixture) = fixture(dir.path()).await;
    let result = client().post(format!("http://{addr}/api/sessions/fixture/git/discard")).bearer_auth(OWNER)
        .header("content-type", "application/json").body(json!({"path":"arquivo.txt", "cwd":"/outro"}).to_string()).send().await.unwrap();
    assert_eq!(result.status(), 422);
    assert_eq!(result.json::<Value>().await.unwrap(), json!({"detail":[{
        "type":"extra_forbidden","loc":["body","cwd"],"msg":"Extra inputs are not permitted","input":"/outro"}]}));
    assert_eq!(hits.load(Ordering::Relaxed), 0);
    assert_eq!(std::fs::read_to_string(dir.path().join("arquivo.txt")).unwrap(), "alteração preservada");
}

#[tokio::test]
async fn workspace_uses_distinct_native_cwds_without_python_context_g01() {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let work = root.join("work");
    let repo = work.join("repo");
    let feature = work.join("feature");
    let home = root.join("home");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::create_dir(&home).unwrap();
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "core.hooksPath=", "-c", "commit.gpgSign=false"])
            .arg("-C").arg(&repo).args(args)
            .env("GIT_CONFIG_GLOBAL", if cfg!(windows) { "NUL" } else { "/dev/null" })
            .env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    };
    git(&["init", "-b", "main"]);
    std::fs::write(repo.join("sub/ação.txt"), "primeira\nlinha\n").unwrap();
    git(&["add", "sub/ação.txt"]);
    git(&["-c", "user.name=T", "-c", "user.email=t@example.invalid", "commit", "-m", "initial"]);
    git(&["worktree", "add", "-b", "feature", feature.to_str().unwrap()]);
    std::fs::write(repo.join("sub/ação.txt"), "primeira\nlinha mudada\n").unwrap();
    let jsonl = home.join("fixture.jsonl");
    std::fs::write(&jsonl, format!("{}\n", json!({"type":"assistant","cwd":repo,
        "message":{"content":[{"type":"tool_use","name":"Bash","input":{
            "command":format!("cd '{}' && git status", feature.display())}}]}}))).unwrap();
    #[cfg(unix)]
    let mux = {
        let path = home.join("g01-mux");
        crate::write_executable(&path, format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\nprintf 'fixture\\t1\\t\\t{}\\t%%1\\t\\tclaude\\t\\t0\\t0\\n'\n", repo.display()));
        path
    };
    #[cfg(windows)]
    let mux = {
        let path = home.join("g01-mux.cmd");
        std::fs::write(&path, format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\necho fixture\t1\t\t{}\t%%1\t\tclaude\t\t0\t0\r\n", repo.display())).unwrap();
        path
    };
    let f = workspace_fixture::fixture_with_mux(&work, false, Some(&home), Some(&mux)).await;
    f.list.seed("fixture", jsonl.to_str().unwrap());
    let rows = f.list.discover(None).await.unwrap();
    let row = rows.iter().find(|row| row.name == "fixture").unwrap();
    assert_eq!(row.cwd.as_deref(), repo.to_str());
    assert_eq!(row.git_cwd.as_deref(), feature.to_str());
    assert_eq!(row.jsonl.as_deref(), jsonl.to_str());
    let c = client();
    let mut bodies = Vec::new();
    let mut statuses = Vec::new();
    for route in ["branches", "files/read?path=sub%2Fa%C3%A7%C3%A3o.txt"] {
        let response = c.get(format!("http://{}/api/sessions/fixture/{route}", f.addr))
            .bearer_auth(OWNER).send().await.unwrap();
        statuses.push(response.status().as_u16());
        bodies.push(response.json::<Value>().await.unwrap());
    }
    assert_eq!(statuses, [200, 200], "{bodies:?}; consultas ao contexto Python: {}", f.context_hits.load(Ordering::Relaxed));
    assert_eq!(bodies[0], json!({"current":"feature","branches":["feature","main"],
        "remotes":[],"dirty":false,"elsewhere":{"main":{"path":repo,"main":true}}}));
    assert_eq!(bodies[1], json!({"path":"sub/ação.txt","text":"primeira\nlinha mudada\n",
        "size":22,"truncated":false,"digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}));
    assert_eq!(f.context_hits.load(Ordering::Relaxed), 0);
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn workspace_reads_orq_cwd_outside_scan_roots_and_reloads_same_name_g01() {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let scan = root.join("scan");
    let home = root.join("home");
    let first = root.join("first");
    let second = root.join("second");
    let run = home.join(".hangar/orq/context-run");
    for path in [&scan, &first, &second, &run] { std::fs::create_dir_all(path).unwrap(); }
    std::fs::write(first.join("ação.txt"), "primeira\nlinha mudada\n").unwrap();
    std::fs::write(second.join("ação.txt"), "original").unwrap();
    let stamp = chrono::Utc::now().to_rfc3339();
    std::fs::write(run.join("eventos.jsonl"), format!("{}\n", json!({
        "tipo":"execucao_inicio","gid":"context-group","ts":stamp}))).unwrap();
    std::fs::write(run.join("timeline-context-run.jsonl"), "").unwrap();
    std::fs::write(run.join("vigia.json"), json!({"pid":std::process::id(),
        "interval_s":3600,"ts":stamp,"arbiter":"a"}).to_string()).unwrap();
    let f = workspace_fixture::fixture_with_home(&scan, false, Some(&home)).await;
    for (cwd, text, size, digest) in [
        (&first, "primeira\nlinha mudada\n", 22, "0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"),
        (&second, "original", 8, "0682c5f2076f099c34cfdd15a9e063849ed437a49677e6fcc5b4198c76575be5"),
    ] {
        std::fs::write(run.join("orq.json"), json!({"auto":true,"repo":cwd,"arbiter":"a"}).to_string()).unwrap();
        let response = client().get(format!("http://{}/api/sessions/context-group-orq/files/read?path=a%C3%A7%C3%A3o.txt", f.addr))
            .bearer_auth(OWNER).send().await.unwrap();
        let status = response.status();
        let raw = response.text().await.unwrap();
        let body = serde_json::from_str::<Value>(&raw);
        assert_eq!(status, 200, "{raw}");
        assert_eq!(body.ok(), Some(json!({"path":"ação.txt","text":text,"size":size,
            "truncated":false,"digest":digest})), "{raw}");
        assert_eq!(f.context_hits.load(Ordering::Relaxed), 0);
        assert_eq!(f.hits(), 0);
    }
}

fn active_orq(home: &std::path::Path, cwd: &std::path::Path) -> std::path::PathBuf {
    let run = home.join(".hangar/orq/review-run");
    std::fs::create_dir_all(&run).unwrap();
    let stamp = chrono::Utc::now().to_rfc3339();
    std::fs::write(run.join("orq.json"), json!({"auto":true,"repo":cwd,"arbiter":"a"}).to_string()).unwrap();
    std::fs::write(run.join("eventos.jsonl"), format!("{}\n", json!({"tipo":"execucao_inicio","gid":"review-group","ts":stamp}))).unwrap();
    std::fs::write(run.join("timeline-review-run.jsonl"), "").unwrap();
    std::fs::write(run.join("vigia.json"), json!({"pid":std::process::id(),"interval_s":3600,"ts":stamp,"arbiter":"a"}).to_string()).unwrap();
    run
}

async fn check_unreadable_orq_source(warm: bool) {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    let cwd = root.join("repo");
    std::fs::create_dir(&cwd).unwrap();
    std::fs::write(cwd.join("ação.txt"), "original").unwrap();
    let run = active_orq(&home, &cwd);
    let f = workspace_fixture::fixture_with_home(&root, false, Some(&home)).await;
    let c = client();
    let url = format!("http://{}/api/sessions/review-group-orq/files/read?path=a%C3%A7%C3%A3o.txt", f.addr);
    if warm {
        let response = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.json::<Value>().await.unwrap(), json!({"path":"ação.txt","text":"original","size":8,
            "truncated":false,"digest":"0682c5f2076f099c34cfdd15a9e063849ed437a49677e6fcc5b4198c76575be5"}));
    }
    std::fs::write(run.join("orq.json"), "{").unwrap();
    let read = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    let write = c.post(format!("http://{}/api/sessions/review-group-orq/files/write", f.addr))
        .bearer_auth(OWNER).json(&json!({"path":"ação.txt","text":"perdida",
            "digest":"0682c5f2076f099c34cfdd15a9e063849ed437a49677e6fcc5b4198c76575be5"})).send().await.unwrap();
    let statuses = [read.status().as_u16(), write.status().as_u16()];
    assert_eq!(std::fs::read_to_string(cwd.join("ação.txt")).unwrap(), "original", "warm={warm}, status={statuses:?}");
    for response in [read, write] {
        let body = refusal(response, "workspace_context").await;
        assert_eq!(body["detail"]["params"]["motivo"], "orquestracoes");
    }
    assert_eq!(f.context_hits.load(Ordering::Relaxed), 0);
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn g01_review_orq_warm_context_refuses_unreadable_source() {
    check_unreadable_orq_source(true).await;
}

#[tokio::test]
async fn g01_review_orq_cold_context_refuses_unreadable_source() {
    check_unreadable_orq_source(false).await;
}

#[tokio::test]
async fn g01_review_recreated_session_uses_first_request_cwd_without_cached_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    let first = root.join("first");
    let second = root.join("second");
    for path in [&home, &first, &second] { std::fs::create_dir_all(path).unwrap(); }
    std::fs::write(first.join("ação.txt"), "original").unwrap();
    std::fs::write(second.join("ação.txt"), "primeira\nlinha mudada\n").unwrap();
    let panes = home.join("panes.txt");
    let pane = |cwd: &std::path::Path, birth| format!("fixture\t1\t\t{}\t%1\t\tclaude\t{birth}\t0\t0\n", cwd.display());
    std::fs::write(&panes, pane(&first, 1700000000)).unwrap();
    #[cfg(unix)]
    let mux = {
        let mux = home.join("review-mux");
        crate::write_executable(&mux, format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\ncat '{}'\n", panes.display()));
        mux
    };
    #[cfg(windows)]
    let mux = {
        let mux = home.join("review-mux.cmd");
        std::fs::write(&mux, format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\ntype \"{}\"\r\n", panes.display())).unwrap();
        mux
    };
    let f = workspace_fixture::fixture_with_mux(&root, false, Some(&home), Some(&mux)).await;
    let rows = f.list.discover(None).await.unwrap();
    let row = rows.iter().find(|row| row.name == "fixture").unwrap();
    assert_eq!(row.cwd.as_deref(), first.to_str());
    assert_eq!(row.lifecycle_id.as_deref(), Some("t:1700000000"));
    let c = client();
    let url = format!("http://{}/api/sessions/fixture/files/read?path=a%C3%A7%C3%A3o.txt", f.addr);
    let first_read = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(first_read.status(), 200);
    assert_eq!(first_read.json::<Value>().await.unwrap(), json!({"path":"ação.txt","text":"original","size":8,
        "truncated":false,"digest":"0682c5f2076f099c34cfdd15a9e063849ed437a49677e6fcc5b4198c76575be5"}));
    std::fs::write(&panes, pane(&second, 1700000001)).unwrap();
    let response = c.get(&url).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap(), json!({"path":"ação.txt","text":"primeira\nlinha mudada\n","size":22,
        "truncated":false,"digest":"0fe20587a6b919286e72d57b501eec339f1afdeeef022141171485dbac529e26"}));
    let rows = f.list.discover(None).await.unwrap();
    let row = rows.iter().find(|row| row.name == "fixture").unwrap();
    assert_eq!(row.cwd.as_deref(), second.to_str());
    assert_eq!(row.lifecycle_id.as_deref(), Some("t:1700000001"));
    assert_eq!(f.context_hits.load(Ordering::Relaxed), 0);
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn unavailable_native_context_answers_503_with_reason() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-mux");
    let f = workspace_fixture::fixture_with_mux(dir.path(), false, None, Some(&missing)).await;
    let response = client()
        .get(format!("http://{}/api/sessions/fixture/files/list", f.addr))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    let body = refusal(response, "workspace_context").await;
    assert_eq!(body["detail"]["params"]["motivo"], "sessoes");
    assert_eq!(f.context_hits.load(Ordering::Relaxed), 0);
    assert_eq!(f.hits(), 0, "o Python não pode atender");
    let diag = f.diag("rust.workspace_failed").await;
    assert_eq!((&diag["sessao"], &diag["codigo"]), (&json!("fixture"), &json!("workspace_context")));
}

#[cfg(unix)]
#[tokio::test]
async fn full_write_slots_answer_busy_without_python() {
    let dir = tempfile::tempdir().unwrap();
    let remote = dir.path().join("remoto.git");
    let repo = dir.path().join("repo");
    std::fs::create_dir(&repo).unwrap();
    git(dir.path(), &["init", "-q", "--bare", "remoto.git"]);
    crate::write_executable(&remote.join("hooks/pre-receive"), "#!/bin/sh\nsleep 3\n");
    // Um core.hooksPath global de quem roda faria o remoto ignorar este hook, e os pushes
    // terminariam antes de o commit chegar.
    git(&remote, &["config", "core.hooksPath", remote.join("hooks").to_str().unwrap()]);
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["-c", "user.name=T", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "c"]);
    git(&repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&repo, &["config", "branch.main.remote", "origin"]);
    git(&repo, &["config", "branch.main.merge", "refs/heads/main"]);
    std::fs::write(repo.join("novo.txt"), "x").unwrap();
    let f = workspace_fixture::session_fixture(&repo, true).await;
    let addr = f.addr;
    let c = client();
    let post = |route: &str, body: Value| {
        c.post(format!("http://{addr}/api/sessions/fixture/{route}"))
            .bearer_auth(OWNER)
            .header("content-type", "application/json")
            .body(body.to_string())
            .send()
    };
    let pushes = (0..4).map(|_| tokio::spawn(post("git/push", json!({})))).collect::<Vec<_>>();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let started = std::time::Instant::now();
    let commit = post("git/commit", json!({"message":"m","paths":["novo.txt"]})).await.unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(1), "esperou a vaga");
    assert!(commit.headers().get("retry-after").is_some(), "sem Retry-After");
    refusal(commit, "workspace_busy").await;
    assert_eq!(f.diag("rust.workspace_busy").await["codigo"], "workspace_busy");
    let log = std::process::Command::new("git").arg("-C").arg(&repo).args(["log", "--oneline"]).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&log.stdout).lines().count(), 1, "o Rust não pode ter commitado");
    for push in pushes {
        assert_ne!(push.await.unwrap().unwrap().text().await.unwrap(), "python-reserva");
    }
    assert_eq!(f.hits(), 0, "o Python não pode atender");
}

#[tokio::test]
async fn served_text_declares_utf8_and_xhtml_wrapper_is_html() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("nota.txt"), "ação").unwrap();
    std::fs::write(dir.path().join("pagina.xhtml"), "<html/>").unwrap();
    std::fs::write(
        dir.path().join("fixture.jsonl"),
        json!({"cwd":dir.path(),"text":"nota.txt pagina.xhtml"}).to_string(),
    )
    .unwrap();
    let (addr, _, _fixture) = fixture(dir.path()).await;
    for (file, expected) in [
        ("nota.txt", "text/plain; charset=utf-8"),
        ("pagina.xhtml", "text/html; charset=utf-8"),
    ] {
        let response = client()
            .get(format!("http://{addr}/api/sessions/fixture/file?path={file}"))
            .bearer_auth(OWNER)
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], expected);
    }
}

#[cfg(unix)]
#[tokio::test]
async fn download_of_a_name_with_control_characters_is_encoded() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a\u{7f}b c.txt"), "x").unwrap();
    std::fs::write(
        dir.path().join("fixture.jsonl"),
        json!({"cwd":dir.path(),"text":"a\u{7f}b c.txt"}).to_string(),
    )
    .unwrap();
    let (addr, _, _fixture) = fixture(dir.path()).await;
    let response = client()
        .get(format!("http://{addr}/api/sessions/fixture/file?path=a%7Fb%20c.txt&download=1"))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename*=utf-8''a%7Fb%20c.txt"
    );
}

#[tokio::test]
async fn folder_git_refusal_keeps_the_python_error_shape() {
    let dir = tempfile::tempdir().unwrap();
    let other = tempfile::tempdir().unwrap();
    let (addr, _, _fixture) = fixture(dir.path()).await;
    for route in ["git", "branches"] {
        let response = client()
            .get(format!(
                "http://{addr}/api/fs/{route}?root={}",
                other.path().display()
            ))
            .bearer_auth(OWNER)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 403);
        let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
        assert_eq!(
            body["detail"],
            json!({"code":"erro_criacao_sessao","params":{},"msg":"root not allowed"})
        );
    }
}
