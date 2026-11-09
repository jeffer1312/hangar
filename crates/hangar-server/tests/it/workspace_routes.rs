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
async fn retired_private_workspace_route_stays_absent() {
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
    assert_eq!(r.status(), 404);
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
    // O caminho vem do `git worktree list` como o git escreve: com `/` também no Windows, como no Python.
    let listed = if cfg!(windows) { repo.to_string_lossy().replace('\\', "/") } else { repo.to_string_lossy().into_owned() };
    assert_eq!(bodies[0], json!({"current":"feature","branches":["feature","main"],
        "remotes":[],"dirty":false,"elsewhere":{"main":{"path":listed,"main":true}},"upstreams":{}}));
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

#[cfg(unix)]
#[tokio::test]
async fn workspace_orq_panel_cited_external_report_is_served_without_python() {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    let repo = root.join("repo");
    std::fs::create_dir(&repo).unwrap();
    let run = active_orq(&home, &repo);
    let report = root.join("parecer.md");
    assert!(!report.starts_with(&repo));
    std::fs::write(&report, "# parecer\n").unwrap();
    std::fs::write(run.join("timeline-review-run.jsonl"), format!("{}\n", json!({
        "ts":"2026-09-28T10:05:00-03:00", "kind":"woke", "task":null,
        "text":format!("acordou o árbitro: [decisao] T4: incluir a tela? Parecer: {}", report.display())
    }))).unwrap();
    let f = workspace_fixture::fixture_with_home(&repo, false, Some(&home)).await;
    let c = client();
    let base = format!("http://{}/api/sessions/review-group-orq", f.addr);
    let response = c.get(format!("{base}/orq/panel")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 200);
    let panel: Value = response.json().await.unwrap();
    let reports: Vec<_> = panel["decisions"].as_array().unwrap().iter().map(|row| row["parecer"].clone()).collect();
    assert_eq!(reports, vec![json!(report)]);
    let response = c.get(reader_asset_url(&format!("{base}/file"), report.to_str().unwrap(), false))
        .bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.text().await.unwrap(), "# parecer\n");
    assert_eq!(f.context_hits.load(Ordering::Relaxed), 0);
    assert_eq!(f.hits(), 0);
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
async fn session_routes_do_not_read_scan_roots_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    let cwd = root.join("repo");
    for path in [&home, &cwd] { std::fs::create_dir_all(path).unwrap(); }
    std::fs::write(cwd.join("ação.txt"), "original").unwrap();
    // `.env` que não se lê: só quem consome as raízes pode recusar por ele.
    std::fs::create_dir_all(home.join(".hangar").join("config").join(".env")).unwrap();
    let panes = home.join("panes.txt");
    std::fs::write(&panes, format!("fixture\t1\t\t{}\t%1\t\tclaude\t1700000000\t0\t0\n", cwd.display())).unwrap();
    #[cfg(unix)]
    let mux = {
        let mux = home.join("roots-mux");
        crate::write_executable(&mux, format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\ncat '{}'\n", panes.display()));
        mux
    };
    #[cfg(windows)]
    let mux = {
        let mux = home.join("roots-mux.cmd");
        std::fs::write(&mux, format!("@echo off\r\nif not \"%~1\"==\"list-panes\" exit /b 0\r\ntype \"{}\"\r\n", panes.display())).unwrap();
        mux
    };
    let f = workspace_fixture::fixture_with_mux(&root, false, Some(&home), Some(&mux)).await;
    let c = client();
    let read = c.get(format!("http://{}/api/sessions/fixture/files/read?path=a%C3%A7%C3%A3o.txt", f.addr))
        .bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(read.status(), 200);
    assert_eq!(read.json::<Value>().await.unwrap()["text"], "original");
    let roots = c.get(format!("http://{}/api/fs/roots", f.addr)).bearer_auth(OWNER).send().await.unwrap();
    let body = refusal(roots, "workspace_context").await;
    assert_eq!(body["detail"]["params"]["motivo"], "configuracao");
    assert_eq!(f.hits(), 0);
}

#[tokio::test]
async fn folder_routes_without_sessions_skip_session_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let root = hangar_workspace::real(dir.path());
    let home = root.join("home");
    for path in [&home, &root.join("pasta")] { std::fs::create_dir_all(path).unwrap(); }
    let missing = root.join("missing-mux");
    let f = workspace_fixture::fixture_with_mux(&root, false, Some(&home), Some(&missing)).await;
    let c = client();
    let roots = c.get(format!("http://{}/api/fs/roots", f.addr)).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(roots.status(), 200);
    let url = |route: &str| {
        let mut url = reqwest::Url::parse(&format!("http://{}{route}", f.addr)).unwrap();
        url.query_pairs_mut().append_pair("root", root.to_str().unwrap());
        url
    };
    let scan = c.get(url("/api/fs/scan")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(scan.status(), 200);
    // Pasta com Git lista as sessões dentro dela: sem multiplexador, recusa como antes.
    let git = c.get(url("/api/fs/git")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(refusal(git, "workspace_context").await["detail"]["params"]["motivo"], "sessoes");
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

const TRANSFER_THREAD: &str = "019f99de-9572-7221-89c0-80c62f883d44";
const FILE_TRANSFER: &str = "a563a017-af37-42e8-a8e5-2bd837584f8e";
const ORIGINAL_DIGEST: &str = "0682c5f2076f099c34cfdd15a9e063849ed437a49677e6fcc5b4198c76575be5";

fn sha(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes).as_ref().iter().map(|b| format!("{b:02x}")).collect()
}
fn json_line(value: Value) -> String { format!("{value}\n") }

pub(super) struct TransferredFiles {
    pub(super) f: workspace_fixture::Fixture,
    pub(super) home: std::path::PathBuf,
    pub(super) born: std::path::PathBuf,
    pub(super) old: std::path::PathBuf,
    pub(super) rollout: std::path::PathBuf,
    pub(super) record_path: std::path::PathBuf,
    pub(super) sidecar: std::path::PathBuf,
    pub(super) snapshot: std::path::PathBuf,
    pub(super) record: Value,
}

impl TransferredFiles {
    pub(super) async fn new(root: &std::path::Path) -> Self {
        // Forma longa: no Windows a pasta temporária pode vir curta (`RUNNER~1`) e o resultado sai resolvido.
        let root = &hangar_workspace::real(root);
        let home = root.join("home");
        let born = root.join("born");
        let old = root.join("original");
        for path in [&home, &born, &old] { std::fs::create_dir_all(path).unwrap(); }
        std::fs::write(old.join("historical-report.md"), "original").unwrap();
        std::fs::write(old.join("uncited.md"), "private").unwrap();
        std::fs::write(old.join("fork.md"), "private").unwrap();
        let snapshot = home.join("snapshot.jsonl");
        // A citação está nos dados crus, não numa mensagem renderizada, e antes de qualquer janela visual.
        let mut rows = json_line(json!({"type":"user","uuid":"first","parentUuid":null,"cwd":old,
            "message":{"role":"user","content":"início"},"snapshot":{"historical-report.md":{}}}));
        let mut ids = vec!["first".to_owned()];
        let mut parent = "first".to_owned();
        for n in 0..320 {
            let id = format!("old-{n}");
            rows.push_str(&json_line(json!({"type":"assistant","uuid":id,"parentUuid":parent,"cwd":born,
                "message":{"role":"assistant","content":"mais recente"}})));
            parent = id.clone(); ids.push(id);
        }
        rows.push_str(&json_line(json!({"type":"assistant","uuid":"fork","parentUuid":"first","isSidechain":true,
            "cwd":old,"message":{"content":"fork.md"}})));
        rows.push_str(&json_line(json!({"type":"last-prompt","leafUuid":parent})));
        std::fs::write(&snapshot, &rows).unwrap();
        let codex_home = home.join(".codex");
        let rollout = codex_home.join("sessions").join(format!("rollout-fixture-{TRANSFER_THREAD}.jsonl"));
        std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        let imported = json_line(json!({"type":"response_item","payload":{"type":"message","id":"imported",
            "role":"user","content":[{"type":"input_text","text":old.join("uncited.md")}]}}));
        let prefix = json_line(json!({"type":"session_meta","payload":{"id":TRANSFER_THREAD,"cwd":born}})) + &imported;
        let active = json_line(json!({"type":"response_item","id":"active","cwd":old,"payload":{"type":"message",
            "id":"new","role":"user","content":[{"type":"input_text","text":"active.md"}]}}));
        std::fs::write(old.join("active.md"), "active").unwrap();
        std::fs::write(&rollout, prefix.clone() + &active + &imported).unwrap();
        let record = json!({"id":FILE_TRANSFER,"name":"fixture","source_life":"k:files","phase":"complete",
            "source":{"provider":"claude","path":snapshot,"digest":sha(rows.as_bytes()),"selected_uuids":ids},
            "origin_meta":{"provider":"claude","cwd":old,"jsonl":home.join("original.jsonl"),"key":"files"},
            "destination_meta":{"provider":"codex","codex_home":codex_home,"thread_id":TRANSFER_THREAD,
                "lifecycle_id":"k:files","transfer_id":FILE_TRANSFER},
            "boundary":{"thread_id":TRANSFER_THREAD,"rollout_path":rollout,"min_offset":prefix.len(),
                "prefix_digest":sha(prefix.as_bytes()),"imported_item_ids":["imported"]},"error_code":null});
        let store = home.join(".hangar/conversation-transfers");
        std::fs::create_dir_all(store.join("indexes")).unwrap();
        let record_path = store.join(format!("{FILE_TRANSFER}.json"));
        std::fs::write(&record_path, record.to_string()).unwrap();
        let key = format!("[{}, {}]", json!(hangar_workspace::real(&codex_home)), json!(TRANSFER_THREAD));
        std::fs::write(store.join("indexes").join(format!("thread-{}.json", sha(key.as_bytes()))), json!([FILE_TRANSFER]).to_string()).unwrap();
        let sidecar = home.join(".hangar/codex-sessions/fixture.json");
        std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
        std::fs::write(&sidecar, json!({"name":"fixture","provider":"codex","key":"files","cwd":born,
            "codex_home":codex_home,"thread_id":TRANSFER_THREAD,"rollout_path":rollout,"headless":true,"transfer_id":FILE_TRANSFER}).to_string()).unwrap();
        #[cfg(unix)]
        let mux = { let path = home.join("files-mux"); crate::write_executable(&path, "#!/bin/sh\nexit 0\n"); path };
        #[cfg(windows)]
        let mux = { let path = home.join("files-mux.cmd"); std::fs::write(&path, "@exit /b 0\r\n").unwrap(); path };
        let f = workspace_fixture::fixture_with_mux(root, false, Some(&home), Some(&mux)).await;
        let discovered = f.list.discover(None).await.unwrap();
        let row = discovered.iter().find(|row| row.name == "fixture").unwrap();
        assert_eq!(row.provider, "codex");
        assert_eq!(row.jsonl.as_deref(), rollout.to_str());
        assert_eq!(row.lifecycle_id.as_deref(), Some("k:files"));
        Self { f, home, born, old, rollout, record_path, sidecar, snapshot, record }
    }

    fn url(&self, route: &str, path: &str) -> reqwest::Url {
        reader_asset_url(&format!("http://{}/api/sessions/fixture/{route}", self.f.addr), path, false)
    }
    pub(super) fn no_python(&self) {
        assert_eq!(self.f.hits(), 0);
        assert_eq!(self.f.context_hits.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test]
async fn task3_f03_verified_raw_rows_authorize_old_and_active_files_without_a_visual_window() {
    let dir = tempfile::tempdir().unwrap();
    let h = TransferredFiles::new(dir.path()).await;
    let c = client();
    let read = c.get(h.url("file/text", "historical-report.md")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(read.status(), 200);
    assert_eq!(read.json::<Value>().await.unwrap(), json!({"path":"historical-report.md","text":"original",
        "size":8,"truncated":false,"digest":ORIGINAL_DIGEST}));
    let file = c.get(h.url("file", "historical-report.md")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(file.status(), 200);
    assert_eq!(file.text().await.unwrap(), "original");
    let resolved = c.post(format!("http://{}/api/sessions/fixture/files/resolver", h.f.addr)).bearer_auth(OWNER)
        .json(&json!({"caminhos":["historical-report.md"]})).send().await.unwrap();
    assert_eq!(resolved.status(), 200);
    assert_eq!(resolved.json::<Value>().await.unwrap(), json!({"faltam":[],"ok":{"historical-report.md":{
        "relativo":null,"real":h.old.join("historical-report.md")}}}));
    let save = c.post(h.url("file/text", "historical-report.md")).bearer_auth(OWNER)
        .json(&json!({"path":"historical-report.md","text":"alterado","digest":ORIGINAL_DIGEST})).send().await.unwrap();
    assert_eq!(save.status(), 200);
    assert_eq!(std::fs::read_to_string(h.old.join("historical-report.md")).unwrap(), "alterado");
    let stale = c.post(h.url("file/text", "historical-report.md")).bearer_auth(OWNER)
        .json(&json!({"path":"historical-report.md","text":"perdida","digest":ORIGINAL_DIGEST})).send().await.unwrap();
    assert_eq!(stale.status(), 409);
    assert_eq!(std::fs::read_to_string(h.old.join("historical-report.md")).unwrap(), "alterado");
    let active = c.get(h.url("file/text", "active.md")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(active.status(), 200);
    assert_eq!(active.json::<Value>().await.unwrap()["text"], "active");
    for file in ["uncited.md", "fork.md"] {
        let response = c.get(h.url("file/text", file)).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(), 403, "{file}");
    }
    h.no_python();
}

#[tokio::test]
async fn task3_f03_archive_relocation_uses_account_thread_not_the_old_name() {
    let dir = tempfile::tempdir().unwrap();
    let h = TransferredFiles::new(dir.path()).await;
    let moved = h.home.join(".codex/archived_sessions").join(h.rollout.file_name().unwrap());
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    std::fs::rename(&h.rollout, &moved).unwrap();
    let mut meta: Value = serde_json::from_slice(&std::fs::read(&h.sidecar).unwrap()).unwrap();
    meta["rollout_path"] = json!(moved);
    meta.as_object_mut().unwrap().remove("transfer_id");
    meta["key"] = json!("resumed-new-life");
    std::fs::write(&h.sidecar, meta.to_string()).unwrap();
    let mut record = h.record.clone(); record["name"] = json!("closed-original");
    std::fs::write(&h.record_path, record.to_string()).unwrap();
    let response = client().get(h.url("file/text", "historical-report.md")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(response.json::<Value>().await.unwrap()["text"], "original");
    h.no_python();
}

#[tokio::test]
async fn task3_f03_clear_and_other_account_do_not_inherit_frozen_citations() {
    let dir = tempfile::tempdir().unwrap();
    let h = TransferredFiles::new(dir.path()).await;
    let original_meta: Value = serde_json::from_slice(&std::fs::read(&h.sidecar).unwrap()).unwrap();
    let c = client();
    for other_account in [false, true] {
        let mut meta = original_meta.clone();
        let path = if other_account {
            let home = h.home.join("other-codex");
            let path = home.join("sessions").join(h.rollout.file_name().unwrap());
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::copy(&h.rollout, &path).unwrap();
            meta["codex_home"] = json!(home);
            path
        } else {
            let path = h.home.join(".codex/sessions/rollout-fixture-019f99de-9572-7221-89c0-80c62f883d45.jsonl");
            std::fs::write(&path, json_line(json!({"type":"session_meta","payload":{"id":"019f99de-9572-7221-89c0-80c62f883d45","cwd":h.born}}))).unwrap();
            meta["thread_id"] = json!("019f99de-9572-7221-89c0-80c62f883d45");
            path
        };
        meta["rollout_path"] = json!(path);
        std::fs::write(&h.sidecar, meta.to_string()).unwrap();
        let response = c.get(h.url("file/text", "historical-report.md")).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(), 403, "other_account={other_account}");
        let post = c.post(h.url("file/text", "historical-report.md")).bearer_auth(OWNER)
            .json(&json!({"path":"historical-report.md","text":"perdida","digest":ORIGINAL_DIGEST})).send().await.unwrap();
        assert_eq!(post.status(), 403, "other_account={other_account}");
        assert_eq!(std::fs::read_to_string(h.old.join("historical-report.md")).unwrap(), "original");
    }
    h.no_python();
}

#[cfg(unix)]
#[tokio::test]
async fn task3_f03_snapshot_alias_to_git_is_denied_and_partial_active_line_is_not_a_citation() {
    let dir = tempfile::tempdir().unwrap();
    let h = TransferredFiles::new(dir.path()).await;
    std::fs::create_dir(h.old.join(".git")).unwrap();
    std::fs::write(h.old.join(".git/config"), "private").unwrap();
    std::os::unix::fs::symlink(".git/config", h.old.join("git-alias")).unwrap();
    let original = std::fs::read_to_string(&h.snapshot).unwrap();
    let mut lines: Vec<Value> = original.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
    lines[0]["message"]["content"] = json!("git-alias");
    let changed: String = lines.into_iter().map(json_line).collect();
    std::fs::write(&h.snapshot, &changed).unwrap();
    let mut record = h.record.clone();
    record["source"]["digest"] = json!(sha(changed.as_bytes()));
    std::fs::write(&h.record_path, record.to_string()).unwrap();
    use std::io::Write;
    let partial = json_line(json!({"type":"response_item","cwd":h.old,"payload":{"text":"uncited.md"}}));
    let mut rollout = std::fs::OpenOptions::new().append(true).open(&h.rollout).unwrap();
    rollout.write_all(partial.trim_end().as_bytes()).unwrap();
    let c = client();
    for route in ["file", "file/text"] {
        let get = c.get(h.url(route, "git-alias")).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(get.status(), 403);
        assert_eq!(get.json::<Value>().await.unwrap()["detail"]["code"], "erro_arq_area_do_git");
    }
    let post = c.post(h.url("file/text", "git-alias")).bearer_auth(OWNER)
        .json(&json!({"path":"git-alias","text":"perdida","digest":ORIGINAL_DIGEST})).send().await.unwrap();
    assert_eq!(post.status(), 403);
    let get = c.get(h.url("file/text", "uncited.md")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(get.status(), 403);
    assert_eq!(std::fs::read_to_string(h.old.join(".git/config")).unwrap(), "private");
    h.no_python();
}

#[tokio::test]
async fn task3_f03_corrupt_source_and_boundary_refuse_before_cache_or_write() {
    let dir = tempfile::tempdir().unwrap();
    let h = TransferredFiles::new(dir.path()).await;
    let c = client();
    let snapshot = std::fs::read(&h.snapshot).unwrap();
    let rollout = std::fs::read(&h.rollout).unwrap();
    let invalid = json!({"detail":{"code":"session_transfer_history_invalid","params":{},"msg":"histórico da transferência indisponível"}});
    for change in ["digest", "missing", "uuids", "prefix", "offset", "thread", "active-json", "source-null", "boundary-null", "missing-prefix-digest"] {
        let mut record = h.record.clone();
        match change {
            "digest" => { std::fs::write(&h.snapshot, b"corrupt\n").unwrap(); }
            "missing" => { std::fs::remove_file(&h.snapshot).unwrap(); }
            "uuids" => record["source"]["selected_uuids"] = json!(["fork"]),
            "source-null" => record["source"] = Value::Null,
            "boundary-null" => record["boundary"] = Value::Null,
            "missing-prefix-digest" => record["boundary"]["prefix_digest"] = Value::Null,
            "prefix" => record["boundary"]["prefix_digest"] = json!("0".repeat(64)),
            "offset" => record["boundary"]["min_offset"] = json!(1),
            "thread" => record["boundary"]["thread_id"] = json!("other"),
            "active-json" => { let mut bad = rollout.clone(); bad.extend_from_slice(b"invalid-json\n"); std::fs::write(&h.rollout, bad).unwrap(); }
            _ => unreachable!(),
        }
        std::fs::write(&h.record_path, record.to_string()).unwrap();
        let get = c.get(h.url("file/text", "historical-report.md")).bearer_auth(OWNER).header("if-none-match", "ignored").send().await.unwrap();
        assert_eq!(get.status(), 409, "{change}");
        assert_eq!(get.json::<Value>().await.unwrap(), invalid, "{change}");
        let post = c.post(h.url("file/text", "historical-report.md")).bearer_auth(OWNER)
            .json(&json!({"path":"historical-report.md","text":"perdida","digest":ORIGINAL_DIGEST})).send().await.unwrap();
        if change == "thread" {
            // Registro fora da forma já é recusado pela guarda de escrita, antes do leitor histórico.
            let body = refusal(post, "workspace_context").await;
            assert_eq!(body["detail"]["params"]["motivo"], "transferencia");
        } else {
            assert_eq!(post.status(), 409, "{change}");
            assert_eq!(post.json::<Value>().await.unwrap(), invalid, "{change}");
        }
        assert_eq!(std::fs::read_to_string(h.old.join("historical-report.md")).unwrap(), "original");
        std::fs::write(&h.snapshot, &snapshot).unwrap();
        std::fs::write(&h.rollout, &rollout).unwrap();
    }
    h.no_python();
}

#[tokio::test]
async fn reads_the_python_guarded_answer_busy_while_the_transfer_runs() {
    let dir = tempfile::tempdir().unwrap();
    let h = TransferredFiles::new(dir.path()).await;
    let mut record = h.record.clone();
    record["phase"] = json!("publishing");
    std::fs::write(&h.record_path, record.to_string()).unwrap();
    let c = client();
    let base = format!("http://{}/api/sessions/fixture", h.f.addr);
    for (route, body) in [("files/resolver", json!({"caminhos":["historical-report.md"]})),
        ("git/diff", json!({"path":"historical-report.md"})),
        ("git/path-diff", json!({"path":"historical-report.md","escopo":"nao_commitado"}))] {
        let response = c.post(format!("{base}/{route}")).bearer_auth(OWNER).json(&body).send().await.unwrap();
        assert_eq!(response.status(), 409, "{route}");
        assert_eq!(response.json::<Value>().await.unwrap(), json!({"detail":{"code":"session_transfer_busy",
            "msg":"A sessão está trocando de agente; tente novamente quando terminar.","params":{}}}), "{route}");
    }
    h.no_python();
}

#[tokio::test]
async fn task3_review_p2_unrelated_corrupt_transfer_does_not_block_healthy_codex_files() {
    let dir=tempfile::tempdir().unwrap(); let h=TransferredFiles::new(dir.path()).await;
    let target=h.born.join("healthy.txt"); std::fs::write(&target,"original").unwrap();
    std::fs::write(&h.rollout,json_line(json!({"type":"session_meta","payload":{"id":TRANSFER_THREAD,"cwd":h.born}}))+
        &json_line(json!({"type":"response_item","cwd":h.born,"payload":{"type":"message","role":"user","content":[{"type":"input_text","text":target}]}}))).unwrap();
    let mut meta:Value=serde_json::from_slice(&std::fs::read(&h.sidecar).unwrap()).unwrap();
    meta.as_object_mut().unwrap().remove("transfer_id"); meta["key"]=json!("healthy-life"); std::fs::write(&h.sidecar,meta.to_string()).unwrap();
    std::fs::remove_file(&h.record_path).unwrap();
    let key=format!("[{}, {}]",json!(hangar_workspace::real(&h.home.join(".codex"))),json!(TRANSFER_THREAD));
    std::fs::remove_file(h.home.join(".hangar/conversation-transfers/indexes").join(format!("thread-{}.json",sha(key.as_bytes())))).unwrap();
    std::fs::write(h.record_path.parent().unwrap().join("aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee.json"),"{broken").unwrap();
    let c=client(); let mut results=Vec::new();
    for route in ["file","file/text"] {
        let response=c.get(h.url(route,target.to_str().unwrap())).bearer_auth(OWNER).send().await.unwrap();
        let status=response.status().as_u16(); let text=response.text().await.unwrap(); results.push((status,text));
    }
    assert_eq!(results.iter().map(|(status,_)|*status).collect::<Vec<_>>(),[200,200],"{results:?}");
    assert_eq!(results[0].1,"original"); assert_eq!(serde_json::from_str::<Value>(&results[1].1).unwrap()["text"],"original");
    // A leitura protegida pela transferência também só lê o registro da própria sessão.
    let resolved=c.post(format!("http://{}/api/sessions/fixture/files/resolver",h.f.addr)).bearer_auth(OWNER)
        .json(&json!({"caminhos":[target]})).send().await.unwrap();
    let status=resolved.status().as_u16(); assert_eq!(status,200,"{}",resolved.text().await.unwrap());
    h.no_python();
}

#[tokio::test]
async fn task3_review_p1_missing_indexed_transfer_record_never_authorizes_imported_prefix() {
    let dir=tempfile::tempdir().unwrap(); let h=TransferredFiles::new(dir.path()).await;
    let mut meta:Value=serde_json::from_slice(&std::fs::read(&h.sidecar).unwrap()).unwrap();
    meta.as_object_mut().unwrap().remove("transfer_id"); meta["key"]=json!("resumed-life"); std::fs::write(&h.sidecar,meta.to_string()).unwrap();
    std::fs::remove_file(&h.record_path).unwrap();
    let target=h.old.join("uncited.md"); let c=client();
    let get=c.get(h.url("file/text",target.to_str().unwrap())).bearer_auth(OWNER).send().await.unwrap();
    let get_status=get.status().as_u16(); let get_body=get.json::<Value>().await.unwrap();
    let post=c.post(h.url("file/text",target.to_str().unwrap())).bearer_auth(OWNER)
        .json(&json!({"path":target,"text":"lost","digest":sha(b"private")})).send().await.unwrap();
    let post_status=post.status().as_u16(); let post_body=post.json::<Value>().await.unwrap();
    assert_eq!([get_status,post_status],[409,409],"GET={get_body}; POST={post_body}");
    let invalid=json!({"detail":{"code":"session_transfer_history_invalid","params":{},"msg":"histórico da transferência indisponível"}});
    assert_eq!(get_body,invalid); assert_eq!(post_body,invalid);
    assert_eq!(std::fs::read_to_string(target).unwrap(),"private"); h.no_python();
}

#[tokio::test]
async fn task3_f03_incomplete_destination_never_reads_imported_account_citations() {
    let dir=tempfile::tempdir().unwrap(); let h=TransferredFiles::new(dir.path()).await;
    let mut record=h.record.clone(); record["phase"]=json!("imported"); std::fs::write(&h.record_path,record.to_string()).unwrap();
    let path=h.old.join("uncited.md");
    let get=client().get(h.url("file/text",path.to_str().unwrap())).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(get.status(),409,"o destino incompleto não autoriza linhas importadas do rollout cru");
    assert_eq!(get.json::<Value>().await.unwrap(),json!({"detail":{"code":"session_transfer_history_invalid","params":{},"msg":"histórico da transferência indisponível"}}));
    assert_eq!(std::fs::read_to_string(path).unwrap(),"private"); h.no_python();
}

#[tokio::test]
async fn task3_f03_rewritten_prefix_with_restored_mtime_is_rejected_before_304() {
    let dir=tempfile::tempdir().unwrap(); let h=TransferredFiles::new(dir.path()).await; let c=client();
    let first=c.get(h.url("file","historical-report.md")).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(first.status(),200);
    let etag=first.headers()["etag"].clone(); assert_eq!(first.text().await.unwrap(),"original");
    let modified=std::fs::metadata(&h.rollout).unwrap().modified().unwrap();
    let raw=std::fs::read_to_string(&h.rollout).unwrap(); let changed=raw.replacen("uncited.md","private.md",1);
    assert_ne!(raw,changed); assert_eq!(raw.len(),changed.len());
    std::fs::write(&h.rollout,changed).unwrap(); std::fs::OpenOptions::new().write(true).truncate(false).open(&h.rollout).unwrap().set_modified(modified).unwrap();
    let denied=c.get(h.url("file","historical-report.md")).bearer_auth(OWNER).header("if-none-match",etag).send().await.unwrap();
    assert_eq!(denied.status(),409); assert_eq!(denied.json::<Value>().await.unwrap()["detail"]["code"],"session_transfer_history_invalid"); h.no_python();
}

#[tokio::test]
async fn task3_f03_literal_unicode_line_separators_remain_original_citations() {
    let dir=tempfile::tempdir().unwrap(); let h=TransferredFiles::new(dir.path()).await;
    let raw=std::fs::read_to_string(&h.snapshot).unwrap();
    for separator in ['\u{2028}','\u{2029}'] {
        let path=h.old.join(format!("before{separator}after.md")); std::fs::write(&path,"original").unwrap();
        let mut rows:Vec<Value>=raw.split('\n').filter(|line|!line.is_empty()).map(|line|serde_json::from_str(line).unwrap()).collect();
        rows[0]["message"]["content"]=json!(path);
        let snapshot:String=rows.into_iter().map(json_line).collect(); assert!(snapshot.contains(separator));
        std::fs::write(&h.snapshot,&snapshot).unwrap();
        let mut record=h.record.clone(); record["source"]["digest"]=json!(sha(snapshot.as_bytes())); std::fs::write(&h.record_path,record.to_string()).unwrap();
        let response=client().get(h.url("file/text",path.to_str().unwrap())).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(),200); assert_eq!(response.json::<Value>().await.unwrap(),json!({"path":path,"text":"original","size":8,"truncated":false,"digest":ORIGINAL_DIGEST}));
    }
    h.no_python();
}

#[tokio::test]
async fn task3_f04_documents_keep_download_name_mime_bytes_and_auth_before_304() {
    let dir = tempfile::tempdir().unwrap(); let root = dir.path();
    let content = b"original-document-bytes\x00\xff";
    for name in ["Relatório final.docx", "Relatório final.pdf"] { std::fs::write(root.join(name), content).unwrap(); }
    let log = root.join("fixture.jsonl");
    std::fs::write(&log, json_line(json!({"cwd":root,"text":"Relatório final.docx Relatório final.pdf"}))).unwrap();
    let (addr,hits,_fixture) = fixture(root).await; let c=client();
    let mut last_etag = None;
    for (name,mime,encoded) in [
        ("Relatório final.docx","application/vnd.openxmlformats-officedocument.wordprocessingml.document","Relat%C3%B3rio%20final.docx"),
        ("Relatório final.pdf","application/pdf","Relat%C3%B3rio%20final.pdf"),
    ] {
        let mut url = reader_asset_url(&format!("http://{addr}/api/sessions/fixture/file"), name, false);
        let inline=c.get(url.clone()).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(inline.status(),200);
        assert_eq!(inline.headers()["cache-control"],"max-age=60"); assert!(inline.headers().get("content-disposition").is_none());
        let etag=inline.headers()["etag"].clone(); last_etag = Some(etag.clone());
        assert_eq!(inline.bytes().await.unwrap().as_ref(),content);
        let cached=c.get(url.clone()).bearer_auth(OWNER).header("if-none-match",&etag).send().await.unwrap();
        assert_eq!(cached.status(),304); assert!(cached.bytes().await.unwrap().is_empty());
        let denied=c.get(url.clone()).bearer_auth("wrong").header("if-none-match",&etag).send().await.unwrap(); assert_eq!(denied.status(),401);
        url.query_pairs_mut().append_pair("download","true");
        let download=c.get(url).bearer_auth(OWNER).header("if-none-match",&etag).send().await.unwrap();
        assert_eq!(download.status(),200); assert_eq!(download.headers()["content-type"],mime);
        assert_eq!(download.headers()["content-disposition"],format!("attachment; filename*=utf-8''{encoded}"));
        assert_ne!(download.headers()["etag"],etag); assert_eq!(download.bytes().await.unwrap().as_ref(),content);
    }
    std::fs::write(&log,"{}\n").unwrap();
    let denied=c.get(reader_asset_url(&format!("http://{addr}/api/sessions/fixture/file"),"Relatório final.pdf",false))
        .bearer_auth(OWNER).header("if-none-match",last_etag.unwrap()).send().await.unwrap(); assert_eq!(denied.status(),403);
    assert_eq!(hits.load(Ordering::Relaxed),0);
}

#[tokio::test]
async fn task3_f04_html_isolated_svg_scriptless_and_rewrite_invalidates_etag() {
    use base64::Engine;
    let dir=tempfile::tempdir().unwrap(); let root=dir.path();
    let html=b"<button>run</button></iframe><script>parent.localStorage.getItem('cp_servers')</script>";
    let svg=b"<svg xmlns=\"http://www.w3.org/2000/svg\"><script>alert(1)</script></svg>";
    std::fs::write(root.join("mock.html"),html).unwrap(); std::fs::write(root.join("image.svg"),svg).unwrap();
    std::fs::write(root.join("fixture.jsonl"),json_line(json!({"cwd":root,"text":"mock.html image.svg"}))).unwrap();
    let (addr,hits,_fixture)=fixture(root).await; let c=client();
    let url=format!("http://{addr}/api/sessions/fixture/file?path=mock.html");
    let first=c.get(&url).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(first.status(),200);
    assert_eq!(first.headers()["referrer-policy"],"no-referrer"); assert_eq!(first.headers()["x-content-type-options"],"nosniff");
    let etag=first.headers()["etag"].clone(); let body=first.text().await.unwrap();
    assert!(body.contains("sandbox=\"allow-scripts allow-popups\"")); assert!(!body.contains("allow-same-origin"));
    assert!(body.contains("referrerpolicy=\"no-referrer\"")); assert!(body.contains("<meta name=\"viewport\"")); assert!(!body.contains("<script>"));
    let source=body.split("src=\"data:text/html;charset=utf-8;base64,").nth(1).unwrap().split('"').next().unwrap();
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(source).unwrap(),html);
    std::fs::write(root.join("mock.html"),"<p>dois</p>").unwrap();
    std::fs::OpenOptions::new().write(true).truncate(false).open(root.join("mock.html")).unwrap().set_modified(std::time::UNIX_EPOCH).unwrap();
    let changed=c.get(&url).bearer_auth(OWNER).header("if-none-match",etag).send().await.unwrap(); assert_eq!(changed.status(),200);
    let body=changed.text().await.unwrap(); let source=body.split("src=\"data:text/html;charset=utf-8;base64,").nth(1).unwrap().split('"').next().unwrap();
    assert_eq!(base64::engine::general_purpose::STANDARD.decode(source).unwrap(),b"<p>dois</p>");
    let response=c.get(format!("http://{addr}/api/sessions/fixture/file?path=image.svg")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.headers()["content-type"],"image/svg+xml"); assert_eq!(response.headers()["content-security-policy"],"sandbox; script-src 'none'");
    assert_eq!(response.bytes().await.unwrap().as_ref(),svg); assert_eq!(hits.load(Ordering::Relaxed),0);
}

#[tokio::test]
async fn task3_x01_busy_mutation_does_not_take_read_or_metadata_slots() {
    let dir=tempfile::tempdir().unwrap(); let root=dir.path();
    std::fs::create_dir(root.join(".git")).unwrap(); std::fs::write(root.join(".git/HEAD"),"ref: refs/heads/main\n").unwrap();
    std::fs::write(root.join("read.txt"),"available").unwrap();
    let (addr,hits,f)=fixture(root).await;
    let all=f.workspace_slots.clone().try_acquire_many_owned(4).unwrap();
    let c=client();
    let metadata=hangar_workspace::execute(hangar_workspace::Operation::HeadInfo {cwd:Some(root.to_string_lossy().into_owned())}).unwrap();
    assert_eq!(metadata,json!(["main",false]));
    let read=c.get(format!("http://{addr}/api/sessions/fixture/files/read?path=read.txt")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(read.status(),200); assert_eq!(read.json::<Value>().await.unwrap()["text"],"available");
    let busy=c.post(format!("http://{addr}/api/sessions/fixture/files/write")).bearer_auth(OWNER)
        .json(&json!({"path":"read.txt","text":"lost","digest":"any"})).send().await.unwrap();
    refusal(busy,"workspace_busy").await;
    assert_eq!(std::fs::read_to_string(root.join("read.txt")).unwrap(),"available");
    drop(all); assert_eq!(hits.load(Ordering::Relaxed),0);
}

#[tokio::test]
async fn task3_f01_public_literal_diff_subfolder_tree_scan_and_mkdir() {
    // Forma longa: no Windows a pasta temporária pode vir curta (`RUNNER~1`) e o resultado sai resolvido.
    let dir=tempfile::tempdir().unwrap(); let root=&hangar_workspace::real(dir.path());
    std::fs::create_dir_all(root.join("backend/app")).unwrap();
    std::fs::write(root.join("backend/app/x.txt"),"a\n").unwrap();
    std::fs::write(root.join("a[b].txt"),"colchete\n").unwrap(); std::fs::write(root.join("ab.txt"),"irmão\n").unwrap();
    for args in [&["init","-q","-b","main"][..],&["config","user.name","Teste"],&["config","user.email","teste@example.invalid"],
        &["config","commit.gpgSign","false"],&["add","backend/app/x.txt","a[b].txt","ab.txt"],&["commit","-qm","initial"]] {git(root,args);}
    std::fs::write(root.join("a[b].txt"),"colchete\nCOLCHETE\n").unwrap(); std::fs::write(root.join("ab.txt"),"irmão\nIRMAO\n").unwrap();
    std::fs::write(root.join("backend/app/x.txt"),"a\nb\n").unwrap();
    let (addr,hits,_fixture)=fixture(root).await; let c=client();
    let response=c.get(format!("http://{addr}/api/sessions/fixture/files/list?path=backend")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status(),200); let tree=response.json::<Value>().await.unwrap();
    assert_eq!(tree["entries"][0]["name"],"app"); assert_eq!(tree["entries"][0]["add"],1);
    let response=c.post(format!("http://{addr}/api/sessions/fixture/git/path-diff")).bearer_auth(OWNER)
        .json(&json!({"path":"a[b].txt","escopo":"nao_commitado"})).send().await.unwrap();
    assert_eq!(response.status(),200); let diff=response.json::<Value>().await.unwrap();
    assert!(diff["diff"].as_str().unwrap().contains("+COLCHETE")); assert!(!diff["diff"].as_str().unwrap().contains("+IRMAO"));
    let response=c.post(format!("http://{addr}/api/fs/mkdir")).bearer_auth(OWNER)
        .json(&json!({"root":root,"name":"new-folder"})).send().await.unwrap();
    assert_eq!(response.status(),200); let created=response.json::<Value>().await.unwrap();
    assert_eq!(created["path"],json!(root.join("new-folder"))); assert_eq!(created["is_git"],false); assert!(root.join("new-folder").is_dir());
    let mut url=reqwest::Url::parse(&format!("http://{addr}/api/fs/scan")).unwrap(); url.query_pairs_mut().append_pair("root",root.to_str().unwrap());
    let response=c.get(url).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(response.status(),200);
    assert!(response.json::<Value>().await.unwrap()["entries"].as_array().unwrap().iter().any(|e|e["name"]=="new-folder"));
    assert_eq!(hits.load(Ordering::Relaxed),0);
}

#[tokio::test]
async fn task3_f01_effective_roots_expand_deduplicate_drop_missing_and_follow_runtime_override() {
    let dir=tempfile::tempdir().unwrap(); let root=&hangar_workspace::real(dir.path()); let home=root.join("home");
    for folder in [home.join(".claude"),home.join("one"),home.join("two")] {std::fs::create_dir_all(folder).unwrap();}
    let f=workspace_fixture::fixture_with_home(root,false,Some(&home)).await;
    let c=client(); let url=format!("http://{}/api/fs/roots",f.addr);
    let env=c.get(&url).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(env.status(),200);
    assert_eq!(env.json::<Value>().await.unwrap(),json!([{"name":root.file_name().unwrap().to_str().unwrap(),"path":root}]));
    let config=home.join(".claude/runtime-config.json");
    std::fs::write(&config,json!({"scan_roots":"~/one,~/missing,~/one/,~/two"}).to_string()).unwrap();
    let app=c.get(&url).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(app.status(),200);
    assert_eq!(app.json::<Value>().await.unwrap(),json!([{"name":"one","path":home.join("one")},{"name":"two","path":home.join("two")}]));
    std::fs::write(&config,json!({"scan_roots":""}).to_string()).unwrap();
    let reset=c.get(&url).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(reset.status(),200);
    assert_eq!(reset.json::<Value>().await.unwrap(),json!([{"name":root.file_name().unwrap().to_str().unwrap(),"path":root}]));
    assert_eq!(f.hits(),0); assert_eq!(f.context_hits.load(Ordering::Relaxed),0);
}

#[tokio::test]
async fn task3_f02_public_digest_binary_path_and_search_errors_keep_envelopes() {
    let dir=tempfile::tempdir().unwrap(); let resolved=hangar_workspace::real(dir.path()); let root=resolved.as_path();
    std::fs::write(root.join("read.txt"),"original").unwrap(); std::fs::write(root.join("late.bin"),[vec![b'a';8192],vec![0]].concat()).unwrap();
    let (addr,hits,_fixture)=fixture(root).await; let c=client();
    for (route,status,code) in [("files/read?path=late.bin",415,"erro_arq_binario"),("files/read?path=..%2Foutside",400,"erro_arq_fora_da_raiz"),
        ("files/read?path=.git%2Fconfig",403,"erro_arq_area_do_git"),("files/search?q=empty",409,"erro_arq_nao_e_repo_git"),
        ("files/search?q=%20%20",400,"erro_arq_busca_vazia"),("files/search?q=x&mode=evil",400,"erro_arq_modo_invalido")] {
        let response=c.get(format!("http://{addr}/api/sessions/fixture/{route}")).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(),status); assert_eq!(response.json::<Value>().await.unwrap()["detail"]["code"],code);
    }
    for route in ["files/read","files/list"] {
        let absolute=reader_asset_url(&format!("http://{addr}/api/sessions/fixture/{route}"),root.join("read.txt").to_str().unwrap(),false);
        let response=c.get(absolute).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(response.status(),400);
        assert_eq!(response.json::<Value>().await.unwrap()["detail"]["code"],"erro_arq_caminho_invalido");
        let response=c.get(format!("http://{addr}/api/sessions/fixture/{route}?path=%00")).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(),400); assert_eq!(response.json::<Value>().await.unwrap()["detail"]["code"],"erro_arq_caminho_invalido");
    }
    for (path,scope,status) in [("read.txt","branch",409),("../outside","nao_commitado",400),("\0","nao_commitado",400),("read.txt","evil",400)] {
        let response=c.post(format!("http://{addr}/api/sessions/fixture/git/path-diff")).bearer_auth(OWNER).json(&json!({"path":path,"escopo":scope})).send().await.unwrap();
        assert_eq!(response.status(),status); assert_eq!(response.json::<Value>().await.unwrap()["detail"]["code"],"erro_git_diff");
    }
    std::fs::write(root.join("config"),"common\n").unwrap(); std::fs::create_dir(root.join("objects")).unwrap(); std::fs::create_dir(root.join("refs")).unwrap();
    let plain=c.get(format!("http://{addr}/api/sessions/fixture/files/list")).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(plain.status(),200);
    assert!(plain.json::<Value>().await.unwrap()["entries"].as_array().unwrap().iter().all(|entry|entry["changed"].is_null()&&entry["add"]==0&&entry["del"]==0));
    for (body,code) in [(json!({"path":"read.txt","text":"lost"}),"erro_arq_sem_digest"),
        (json!({"path":"read.txt","text":"lost","digest":"old"}),"erro_arq_mudou_no_disco")] {
        let response=c.post(format!("http://{addr}/api/sessions/fixture/files/write")).bearer_auth(OWNER).json(&body).send().await.unwrap();
        assert_eq!(response.status(),409); assert_eq!(response.json::<Value>().await.unwrap()["detail"]["code"],code);
        assert_eq!(std::fs::read_to_string(root.join("read.txt")).unwrap(),"original");
    }
    assert_eq!(hits.load(Ordering::Relaxed),0);
}

#[cfg(unix)]
#[tokio::test]
async fn task3_remaining_fs_auth_roots_sessions_and_branches() {
    let dir=tempfile::tempdir().unwrap(); let root=dir.path(); let repo=root.join("repo"); let home=root.join("home");
    std::fs::create_dir(&repo).unwrap(); std::fs::create_dir_all(home.join(".claude")).unwrap();
    for args in [&["init","-q","-b","main"][..],&["config","user.name","Teste"],&["config","user.email","teste@example.invalid"],&["config","commit.gpgSign","false"],&["commit","-qm","initial","--allow-empty"],&["branch","extra"]] {git(&repo,args);}
    let mux=home.join("fs-mux"); crate::write_executable(&mux,format!("#!/bin/sh\n[ \"$1\" = list-panes ] || exit 0\nprintf 'fixture\\t1\\t\\t{}\\t%%1\\t\\tclaude\\t1700000000\\t0\\t0\\noutside\\t1\\t\\t{}\\t%%2\\t\\tclaude\\t1700000000\\t0\\t0\\n'\n",repo.display(),root.display()));
    let f=workspace_fixture::fixture_with_mux(root,false,Some(&home),Some(&mux)).await; let c=client();
    let endpoint=|route:&str,path:&std::path::Path| {
        let mut url=reqwest::Url::parse(&format!("http://{}/api/fs/{route}",f.addr)).unwrap();
        url.query_pairs_mut().append_pair("root",root.to_str().unwrap()).append_pair("path",path.to_str().unwrap()); url
    };
    for route in [format!("http://{}/api/fs/roots",f.addr),endpoint("scan",root).to_string(),format!("http://{}/api/sessions/fixture/files/list",f.addr)] {
        let denied=c.get(route).send().await.unwrap(); assert_eq!(denied.status(),401);
        assert_eq!(denied.json::<Value>().await.unwrap(),json!({"detail":{"code":"erro_nao_autorizado","params":{},"msg":"unauthorized"}}));
    }
    let branches=c.get(endpoint("branches",&repo)).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(branches.status(),200);
    let mut names=branches.json::<Value>().await.unwrap()["branches"].as_array().unwrap().clone(); names.sort_by_key(|v|v.as_str().unwrap().to_owned()); assert_eq!(names,json!(["extra","main"]).as_array().unwrap().clone());
    let mut forbidden=endpoint("branches",&repo); forbidden.query_pairs_mut().clear().append_pair("root",repo.to_str().unwrap());
    assert_eq!(c.get(forbidden).bearer_auth(OWNER).send().await.unwrap().status(),403);
    let status=c.get(endpoint("git",&repo)).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(status.status(),200);
    assert_eq!(status.json::<Value>().await.unwrap()["sessions"],json!(["fixture"]));
    let body=json!({"root":root,"path":repo,"branch":"extra"});
    let refused=c.post(format!("http://{}/api/fs/git/switch",f.addr)).bearer_auth(OWNER).json(&body).send().await.unwrap(); assert_eq!(refused.status(),409);
    assert_eq!(refused.json::<Value>().await.unwrap()["detail"]["code"],"erro_git_folder_sessions");
    let mut confirmed=body; confirmed["confirm_sessions"]=json!(true);
    let changed=c.post(format!("http://{}/api/fs/git/switch",f.addr)).bearer_auth(OWNER).json(&confirmed).send().await.unwrap(); assert_eq!(changed.status(),200);
    assert_eq!(changed.json::<Value>().await.unwrap()["current"],"extra");
    let inner=repo.join("inner"); std::fs::create_dir(&inner).unwrap();
    std::fs::write(home.join(".claude/runtime-config.json"),json!({"scan_roots":inner}).to_string()).unwrap();
    let mut read=endpoint("git",&inner); read.query_pairs_mut().clear().append_pair("root",inner.to_str().unwrap());
    assert_eq!(c.get(read).bearer_auth(OWNER).send().await.unwrap().status(),200);
    let fetch=c.post(format!("http://{}/api/fs/git/fetch",f.addr)).bearer_auth(OWNER).json(&json!({"root":inner})).send().await.unwrap(); assert_eq!(fetch.status(),400);
    assert_eq!(c.get(endpoint("scan",root)).bearer_auth(OWNER).send().await.unwrap().status(),403);
    assert_eq!(f.hits(),0); assert_eq!(f.context_hits.load(Ordering::Relaxed),0);
}

#[cfg(unix)]
#[tokio::test]
async fn task3_remaining_public_symlink_escape_and_ancestor_git_boundaries() {
    let dir=tempfile::tempdir().unwrap(); let root=dir.path().join(".git/project"); std::fs::create_dir_all(&root).unwrap();
    for args in [&["init","-q","-b","main"][..],&["config","user.name","Teste"],&["config","user.email","teste@example.invalid"],&["config","commit.gpgSign","false"]] {git(&root,args);}
    std::fs::write(root.join("read.txt"),"original").unwrap(); git(&root,&["add","read.txt"]); git(&root,&["commit","-qm","initial"]);
    let outside=dir.path().join("outside.txt"); std::fs::write(&outside,"private").unwrap();
    std::os::unix::fs::symlink(&outside,root.join("escape")).unwrap(); std::os::unix::fs::symlink(".git",root.join("alias")).unwrap();
    let (addr,hits,_fixture)=fixture(&root).await; let c=client();
    let read=c.get(format!("http://{addr}/api/sessions/fixture/files/read?path=read.txt")).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(read.status(),200);
    assert_eq!(read.json::<Value>().await.unwrap(),json!({"path":"read.txt","text":"original","size":8,"truncated":false,"digest":ORIGINAL_DIGEST}));
    let list=c.get(format!("http://{addr}/api/sessions/fixture/files/list?so_modificados=false")).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(list.status(),200);
    assert!(list.json::<Value>().await.unwrap()["entries"].as_array().unwrap().iter().any(|e|e["name"]=="read.txt"));
    for (path,status,code) in [("escape",400,"erro_arq_fora_da_raiz"),("alias/config",403,"erro_arq_area_do_git")] {
        let response=c.get(reader_asset_url(&format!("http://{addr}/api/sessions/fixture/files/read"),path,false)).bearer_auth(OWNER).send().await.unwrap(); assert_eq!(response.status(),status);
        assert_eq!(response.json::<Value>().await.unwrap()["detail"]["code"],code);
    }
    let diff=c.post(format!("http://{addr}/api/sessions/fixture/git/path-diff")).bearer_auth(OWNER).json(&json!({"path":"escape","escopo":"nao_commitado"})).send().await.unwrap(); assert_eq!(diff.status(),400);
    assert_eq!(diff.json::<Value>().await.unwrap()["detail"]["code"],"erro_git_diff");
    assert_eq!(std::fs::read_to_string(outside).unwrap(),"private"); assert_eq!(hits.load(Ordering::Relaxed),0);
}

#[cfg(unix)]
#[tokio::test]
async fn task3_remaining_missing_git_probe_and_search_failures_are_visible_without_python() {
    use std::process::Command;
    const TEST:&str="workspace_routes::task3_remaining_missing_git_probe_and_search_failures_are_visible_without_python";
    if let Some(root)=std::env::var_os("HANGAR_TASK3_MISSING_GIT_ROOT") {
        let root=std::path::Path::new(&root); let (addr,hits,_fixture)=fixture(root).await; let c=client();
        for (route,code) in [("branches","workspace_unavailable"),("files/list","erro_arq_lista_falhou"),
            ("files/search?q=x&mode=names","erro_arq_busca_falhou"),("files/search?q=x&mode=contents","erro_arq_busca_falhou")] {
            let response=c.get(format!("http://{addr}/api/sessions/fixture/{route}")).bearer_auth(OWNER).send().await.unwrap();
            assert_eq!(response.status(),503,"{route}"); let body=response.json::<Value>().await.unwrap(); assert_eq!(body["detail"]["code"],code);
        }
        assert_eq!(hits.load(Ordering::Relaxed),0); return;
    }
    let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo"); let empty=dir.path().join("empty-path");
    std::fs::create_dir(&root).unwrap(); std::fs::create_dir(&empty).unwrap(); git(&root,&["init","-q","-b","main"]);
    let child=Command::new(std::env::current_exe().unwrap()).args(["--exact",TEST,"--nocapture"])
        .env("PATH",empty).env("HANGAR_TASK3_MISSING_GIT_ROOT",root).output().unwrap();
    assert!(child.status.success(),"{}\n{}",String::from_utf8_lossy(&child.stdout),String::from_utf8_lossy(&child.stderr));
}

#[cfg(unix)]
#[tokio::test]
async fn task3_remaining_search_launch_failure_after_successful_probe_keeps_both_modes() {
    use std::{os::unix::fs::PermissionsExt,process::Command};
    const TEST:&str="workspace_routes::task3_remaining_search_launch_failure_after_successful_probe_keeps_both_modes";
    if let Ok(mode)=std::env::var("HANGAR_TASK3_AFTER_PROBE_MODE") {
        let root=std::path::PathBuf::from(std::env::var_os("HANGAR_TASK3_AFTER_PROBE_ROOT").unwrap());
        let (addr,hits,_fixture)=fixture(&root).await;
        let response=client().get(format!("http://{addr}/api/sessions/fixture/files/search?q=x&mode={mode}")).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(),503); assert_eq!(response.json::<Value>().await.unwrap()["detail"]["code"],"erro_arq_busca_falhou");
        assert!(root.parent().unwrap().join("bin/git.disabled").is_file(),"a consulta de repositório precisa ter sido concluída");
        assert_eq!(hits.load(Ordering::Relaxed),0); return;
    }
    for mode in ["names","contents"] {
        let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo"); let bin=dir.path().join("bin"); std::fs::create_dir(&root).unwrap(); std::fs::create_dir(&bin).unwrap();
        git(&root,&["init","-q","-b","main"]); let shim=bin.join("git");
        std::fs::write(&shim,"#!/bin/sh\ncase \"$*\" in *'rev-parse --is-inside-work-tree'*) /bin/mv \"$HANGAR_TASK3_AFTER_PROBE_GIT\" \"$HANGAR_TASK3_AFTER_PROBE_GIT.disabled\"; printf 'true\\n'; exit 0;; esac\nexec /usr/bin/git \"$@\"\n").unwrap();
        std::fs::set_permissions(&shim,std::fs::Permissions::from_mode(0o755)).unwrap();
        let child=Command::new(std::env::current_exe().unwrap()).args(["--exact",TEST,"--nocapture"])
            .env("PATH",bin).env("HANGAR_TASK3_AFTER_PROBE_MODE",mode).env("HANGAR_TASK3_AFTER_PROBE_ROOT",root).env("HANGAR_TASK3_AFTER_PROBE_GIT",shim).output().unwrap();
        assert!(child.status.success(),"{mode}: {}\n{}",String::from_utf8_lossy(&child.stdout),String::from_utf8_lossy(&child.stderr));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn task3_f01_git_failures_keep_status_and_hide_stderr() {
    use std::{os::unix::fs::PermissionsExt,process::Command};
    const TEST:&str="workspace_routes::task3_f01_git_failures_keep_status_and_hide_stderr";
    if let Ok(kind)=std::env::var("HANGAR_TASK3_FAIL_KIND") {
        let root=std::path::PathBuf::from(std::env::var_os("HANGAR_TASK3_FAIL_ROOT").unwrap());
        let (addr,hits,_fixture)=fixture(&root).await;
        let c=hangar_server::http_client().timeout(std::time::Duration::from_secs(40)).build().unwrap();
        let (response,status,code)=match kind.as_str() {
            "search" => (c.get(format!("http://{addr}/api/sessions/fixture/files/search?q=needle&mode=contents")).bearer_auth(OWNER).send().await.unwrap(),409,"erro_arq_busca_falhou"),
            "diff" => (c.post(format!("http://{addr}/api/sessions/fixture/git/path-diff")).bearer_auth(OWNER)
                .json(&json!({"path":"tracked.txt","escopo":"nao_commitado"})).send().await.unwrap(),409,"erro_git_diff"),
            _ => (c.get(format!("http://{addr}/api/sessions/fixture/files/list")).bearer_auth(OWNER).send().await.unwrap(),
                if kind=="head-timeout" {504}else{500},"erro_arq_lista_falhou"),
        };
        assert_eq!(response.status(),status,"{kind}"); let text=response.text().await.unwrap();
        assert!(!text.contains("/private/fixture")&&!text.contains("synthetic-secret"),"{text}");
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap()["detail"]["code"],code);
        assert_eq!(hits.load(Ordering::Relaxed),0); return;
    }
    let dir=tempfile::tempdir().unwrap(); let root=dir.path().join("repo"); let bin=dir.path().join("bin"); std::fs::create_dir(&root).unwrap(); std::fs::create_dir(&bin).unwrap();
    for args in [&["init","-q","-b","main"][..],&["config","user.name","Teste"],&["config","user.email","teste@example.invalid"],&["config","commit.gpgSign","false"]] {git(&root,args);}
    std::fs::write(root.join("tracked.txt"),"one\n").unwrap(); git(&root,&["add","tracked.txt"]); git(&root,&["commit","-qm","initial"]); std::fs::write(root.join("tracked.txt"),"changed\n").unwrap();
    let shim=bin.join("git");
    std::fs::write(&shim,"#!/bin/sh\ncase \"$HANGAR_TASK3_FAIL_KIND:$*\" in\nnumstat:*--numstat*|head:*--verify*|search:*grep*|diff:*' diff '*) printf '/private/fixture: synthetic-secret\\n' >&2; exit 128;;\nhead-timeout:*--verify*) sleep 60; exit 128;;\nesac\nexec /usr/bin/git \"$@\"\n").unwrap();
    std::fs::set_permissions(&shim,std::fs::Permissions::from_mode(0o755)).unwrap();
    let path=std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
    for kind in ["numstat","head","search","diff","head-timeout"] {
        let child=Command::new(std::env::current_exe().unwrap()).args(["--exact",TEST,"--nocapture"])
            .env("PATH",&path).env("HANGAR_TASK3_FAIL_KIND",kind).env("HANGAR_TASK3_FAIL_ROOT",&root).output().unwrap();
        assert!(child.status.success(),"{kind}: {}\n{}",String::from_utf8_lossy(&child.stdout),String::from_utf8_lossy(&child.stderr));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn task3_g03_public_read_bodies_match_frozen_original_capture() {
    let dir=tempfile::tempdir().unwrap(); let repo=dir.path().join("repo"); std::fs::create_dir_all(repo.join("sub")).unwrap();
    let fixed=|args:&[&str]| {
        let out=std::process::Command::new("git").arg("-C").arg(&repo).args(args)
            .env("GIT_CONFIG_GLOBAL","/dev/null").env("GIT_CONFIG_NOSYSTEM","1").env("GIT_AUTHOR_DATE","2020-01-01T00:00:00Z")
            .env("GIT_COMMITTER_DATE","2020-01-01T00:00:00Z").env("LC_ALL","C").output().unwrap();
        assert!(out.status.success(),"{}",String::from_utf8_lossy(&out.stderr)); String::from_utf8(out.stdout).unwrap()
    };
    for args in [&["init","-q","-b","main"][..],&["config","user.name","Teste"],&["config","user.email","teste@example.invalid"],&["config","commit.gpgSign","false"]] {fixed(args);}
    std::fs::write(repo.join("sub/ação.txt"),"primeira\nlinha\n").unwrap(); std::fs::write(repo.join("outro.txt"),"outro\n").unwrap();
    fixed(&["add","sub/ação.txt","outro.txt"]); fixed(&["commit","-qm","Inicial"]);
    const HASH:&str="20bc9e7267b8e5a4a32370c1855c4db636e589e6";
    assert_eq!(fixed(&["rev-parse","HEAD"]).trim(),HASH,"a cena precisa do mesmo commit da captura");
    std::fs::write(repo.join("sub/ação.txt"),"primeira\nlinha mudada\n").unwrap(); std::fs::write(repo.join("novo.txt"),"novo\n").unwrap();
    std::fs::write(repo.join("bin.dat"),b"bin\0").unwrap();
    std::fs::write(dir.path().join("outside.txt"),"outside").unwrap(); std::os::unix::fs::symlink(dir.path().join("outside.txt"),repo.join("escaping-link")).unwrap();
    let modified="diff --git \"a/sub/a\\303\\247\\303\\243o.txt\" \"b/sub/a\\303\\247\\303\\243o.txt\"\nindex 512146f..ef2305a 100644\n--- \"a/sub/a\\303\\247\\303\\243o.txt\"\n+++ \"b/sub/a\\303\\247\\303\\243o.txt\"\n@@ -1,2 +1,2 @@\n primeira\n-linha\n+linha mudada\n";
    let added="diff --git \"a/sub/a\\303\\247\\303\\243o.txt\" \"b/sub/a\\303\\247\\303\\243o.txt\"\nnew file mode 100644\nindex 0000000..512146f\n--- /dev/null\n+++ \"b/sub/a\\303\\247\\303\\243o.txt\"\n@@ -0,0 +1,2 @@\n+primeira\n+linha\n";
    let full=format!("diff --git a/outro.txt b/outro.txt\nnew file mode 100644\nindex 0000000..a249728\n--- /dev/null\n+++ b/outro.txt\n@@ -0,0 +1 @@\n+outro\n{added}");
    // Só o texto relativo vem do relógio do Git da cena; os outros campos são literais da captura original.
    let relative=fixed(&["log","-1","--format=%ar"]).trim().to_owned();
    let (addr,hits,_fixture)=fixture(&repo).await; let c=client();
    let revision=format!("git/commit/{HASH}");
    let cases=[
        ("branches".to_owned(),json!({"current":"main","branches":["main"],"remotes":[],"dirty":true,"elsewhere":{},"upstreams":{}})),
        ("git/files".to_owned(),json!({"files":[{"path":"sub/ação.txt","code":" M","staged":false,"added":1,"removed":1},
            {"path":"bin.dat","code":"??","staged":false,"added":null,"removed":null},{"path":"escaping-link","code":"??","staged":false,"added":null,"removed":null},
            {"path":"novo.txt","code":"??","staged":false,"added":null,"removed":null}],"sequencer":null})),
        ("git/last-message".to_owned(),json!({"message":"Inicial"})),
        ("git/log".to_owned(),json!({"commits":[{"hash":HASH,"short":"20bc9e7","parents":[],"refs":"HEAD -> main","author":"Teste","ts":1577836800,
            "rel":relative,"subject":"Inicial","body":"","local":false,"col":0,"passthrough":[],"edges":[]}],"ahead":null,"behind":null})),
        (format!("{revision}/files"),json!({"files":[{"path":"outro.txt","code":"A"},{"path":"sub/ação.txt","code":"A"}]})),
        (format!("{revision}/diff?path=sub%2Fa%C3%A7%C3%A3o.txt"),json!({"path":"sub/ação.txt","diff":added})),
        (format!("{revision}/diff-full"),json!({"sha":HASH,"diff":full,"truncated":false})),
        (format!("{revision}/diff-worktree"),json!({"sha":HASH,"diff":modified,"truncated":false})),
        (format!("{revision}/branches"),json!({"local":["main"],"remote":[]})),
    ];
    for (route,expected) in cases {
        let response=c.get(format!("http://{addr}/api/sessions/fixture/{route}")).bearer_auth(OWNER).send().await.unwrap();
        assert_eq!(response.status(),200,"{route}"); assert_eq!(response.json::<Value>().await.unwrap(),expected,"{route}");
    }
    let diff=c.post(format!("http://{addr}/api/sessions/fixture/git/diff")).bearer_auth(OWNER).json(&json!({"path":"sub/ação.txt"})).send().await.unwrap();
    assert_eq!(diff.status(),200); assert_eq!(diff.json::<Value>().await.unwrap(),json!({"path":"sub/ação.txt","diff":modified,"truncated":false}));
    let action=c.post(format!("http://{addr}/api/sessions/fixture/git")).bearer_auth(OWNER).json(&json!({"action":"status"})).send().await.unwrap();
    assert_eq!(action.status(),200); assert_eq!(action.json::<Value>().await.unwrap(),json!({"ok":true,
        "output":"## main\n M \"sub/a\\303\\247\\303\\243o.txt\"\n?? bin.dat\n?? escaping-link\n?? novo.txt"}));
    assert_eq!(hits.load(Ordering::Relaxed),0);
}

#[cfg(unix)]
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn task3_g05_cancelled_push_is_completed_once_without_python_retry() {
    use std::{io::Write,os::fd::AsRawFd};
    let dir=tempfile::tempdir().unwrap(); let root=dir.path(); let repo=root.join("repo"); let remote=root.join("remote.git");
    std::fs::create_dir(&repo).unwrap(); git(root,&["init","-q","--bare","remote.git"]);
    git(&repo,&["init","-q","-b","main"]); git(&repo,&["config","user.name","Teste"]); git(&repo,&["config","user.email","teste@example.invalid"]);
    git(&repo,&["config","commit.gpgSign","false"]); git(&repo,&["commit","-qm","initial","--allow-empty"]);
    git(&repo,&["remote","add","origin",remote.to_str().unwrap()]); git(&repo,&["config","branch.main.remote","origin"]); git(&repo,&["config","branch.main.merge","refs/heads/main"]);
    let ready=root.join("ready"); let release=root.join("release"); let done=root.join("done"); let count=root.join("executions");
    for path in [&ready,&release,&done] {assert!(std::process::Command::new("mkfifo").arg(path).status().unwrap().success());}
    let ready=std::fs::OpenOptions::new().read(true).write(true).open(&ready).unwrap();
    let mut release_file=std::fs::OpenOptions::new().read(true).write(true).open(&release).unwrap();
    let done=std::fs::OpenOptions::new().read(true).write(true).open(&done).unwrap();
    crate::write_executable(&remote.join("hooks/pre-receive"),format!("#!/bin/sh\nprintf 'once\\n' >> '{}'\nprintf R > '{}'\nIFS= read -r value < '{}'\n",count.display(),root.join("ready").display(),release.display()));
    crate::write_executable(&remote.join("hooks/post-receive"),format!("#!/bin/sh\nprintf D > '{}'\n",root.join("done").display()));
    git(&remote,&["config","core.hooksPath",remote.join("hooks").to_str().unwrap()]);
    let f=workspace_fixture::session_fixture(&repo,false).await;
    let home=&f.list.env().dirs.as_ref().unwrap().home;
    let mut state=hangar_server::routes::AppState::new(fake::config("127.0.0.1:9".parse().unwrap(),"127.0.0.1"));
    state.list=f.list.clone(); state.origins_home=home.clone(); state.workspace_slots=f.workspace_slots.clone();
    state.accounts=hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map([
        ("HOME".into(),home.to_string_lossy().into_owned()),("USERPROFILE".into(),home.to_string_lossy().into_owned()),
        ("CLAUDE_CONFIG_DIR".into(),home.join(".claude").to_string_lossy().into_owned()),("CP_SCAN_ROOTS".into(),repo.to_string_lossy().into_owned())].into()));
    assert!(state.state.runtime.set(f.runtime.clone()).is_ok());
    let input=axum::http::Request::builder().method("POST").uri("/api/sessions/fixture/git/push")
        .header("authorization",format!("Bearer {OWNER}")).header("content-type","application/json").body(axum::body::Body::from("{}")).unwrap();
    // Cancelamos a própria futura da rota: abortar só o cliente não confirma que o servidor viu a desconexão.
    let request=tokio::spawn(hangar_server::workspace_routes::public(Arc::new(state),"127.0.0.1:12345".parse().unwrap(),input));
    let started=tokio::task::spawn_blocking(move || {
        let mut event=libc::pollfd {fd:ready.as_raw_fd(),events:libc::POLLIN,revents:0};
        (unsafe {libc::poll(&mut event,1,15_000)})==1 && event.revents&libc::POLLIN!=0
    }).await.unwrap();
    request.abort(); let cancelled=request.await;
    let before=f.workspace_slots.clone().try_acquire_many_owned(4);
    let slot_retained=before.is_err(); drop(before);
    let mut closing=Box::pin(f.runtime.ingress().close("fixture",std::time::Duration::from_secs(15)));
    let ingress_pending=std::future::poll_fn(|cx|std::task::Poll::Ready(std::future::Future::poll(closing.as_mut(),cx).is_pending())).await;
    release_file.write_all(b"finish\n").unwrap();
    let finished=tokio::task::spawn_blocking(move || {
        let mut event=libc::pollfd {fd:done.as_raw_fd(),events:libc::POLLIN,revents:0};
        (unsafe {libc::poll(&mut event,1,15_000)})==1 && event.revents&libc::POLLIN!=0
    }).await.unwrap();
    let all=tokio::time::timeout(std::time::Duration::from_secs(15),f.workspace_slots.clone().acquire_many_owned(4)).await.unwrap().unwrap();
    closing.await.unwrap(); f.runtime.ingress().open("fixture");
    assert!(started&&finished); assert!(cancelled.unwrap_err().is_cancelled());
    assert!(slot_retained,"cancelar a rota liberou a vaga com o hook ainda bloqueado");
    assert!(ingress_pending,"fechar ingresso terminou com a escrita ainda bloqueada no hook");
    assert_eq!(std::fs::read_to_string(count).unwrap(),"once\n");
    let head=std::process::Command::new("git").arg("-C").arg(&repo).args(["rev-parse","HEAD"]).output().unwrap();
    let pushed=std::process::Command::new("git").arg("-C").arg(&remote).args(["rev-parse","refs/heads/main"]).output().unwrap();
    assert!(head.status.success()&&pushed.status.success()); assert_eq!(pushed.stdout,head.stdout);
    assert_eq!(f.hits(),0); assert_eq!(f.context_hits.load(Ordering::Relaxed),0); drop(all);
}

#[cfg(unix)]
#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn workspace_slow_diff_keeps_session_operation_input_and_interrupt_available() {
    use std::{io::Write,os::fd::AsRawFd,time::Duration};
    const TEST: &str = "workspace_routes::workspace_slow_diff_keeps_session_operation_input_and_interrupt_available";
    if let Some(root) = std::env::var_os("HANGAR_WORKSPACE_SLOW_DIFF_ROOT") {
        let root = std::path::PathBuf::from(root);
        let repo = root.join("repo");
        let ready = std::fs::OpenOptions::new().read(true).write(true).open(root.join("ready")).unwrap();
        let mut release = std::fs::OpenOptions::new().read(true).write(true).open(root.join("release")).unwrap();
        let f = workspace_fixture::session_fixture(&repo, false).await;
        let home = &f.list.env().dirs.as_ref().unwrap().home;
        let mut state = hangar_server::routes::AppState::new(fake::config("127.0.0.1:9".parse().unwrap(), "127.0.0.1"));
        state.list = f.list.clone(); state.origins_home = home.clone(); state.workspace_slots = f.workspace_slots.clone();
        state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map([
            ("HOME".into(),home.to_string_lossy().into_owned()),("USERPROFILE".into(),home.to_string_lossy().into_owned()),
            ("CLAUDE_CONFIG_DIR".into(),home.join(".claude").to_string_lossy().into_owned()),("CP_SCAN_ROOTS".into(),repo.to_string_lossy().into_owned())].into()));
        let runtime = crate::runtime_open::with_hubs(hangar_server::runtime::gateway::RuntimeRegistry::new(
            "127.0.0.1:9".parse().unwrap(), "secret-test".into(), "slow-diff-test".into())
            .with_codex_environment(state.accounts.env.clone())
            .with_processes(Arc::new(crate::runtime_open::RecipeProcesses("sid-1"))));
        crate::session_write_routes::open_entry_with(&runtime, &root, "slow-diff", "fixture", 1,
            crate::session_write_routes::cano_with(None).await).await;
        assert!(state.state.runtime.set(runtime.clone()).is_ok());
        let addr = fake::spawn_state(state).await;
        let c = hangar_server::http_client().timeout(Duration::from_secs(40)).build().unwrap();
        let base = format!("http://{addr}/api/sessions/fixture");
        let diff = c.post(format!("{base}/git/diff")).bearer_auth(OWNER).json(&json!({"path":"tracked.txt"}));
        let mut request = tokio::spawn(async move { diff.send().await });
        let checking = runtime.clone();
        // A tarefa captura pânico para liberar o Git e fechar o runtime antes de falhar o teste.
        let checks = tokio::spawn(async move {
            let started = tokio::task::spawn_blocking(move || {
                let mut event = libc::pollfd {fd:ready.as_raw_fd(),events:libc::POLLIN,revents:0};
                (unsafe {libc::poll(&mut event,1,15_000)}) == 1 && event.revents & libc::POLLIN != 0
            }).await.unwrap();
            assert!(started, "o Git não sinalizou a consulta bloqueada");
            checking.ingress().close_exclusive("fixture", Duration::from_secs(15)).await.unwrap();
            checking.ingress().open_exclusive("fixture");
            let input = c.post(format!("{base}/input")).bearer_auth(OWNER)
                .json(&json!({"text":"continue"})).send().await.unwrap();
            assert_eq!(input.status(), 200);
            let interrupt = c.post(format!("{base}/interrupt")).bearer_auth(OWNER).json(&json!({})).send().await.unwrap();
            assert_eq!(interrupt.status(), 200);
        }).await;
        runtime.ingress().open_exclusive("fixture");
        let pending = !request.is_finished();
        let released = release.write_all(b"finish\n");
        let result = tokio::time::timeout(Duration::from_secs(40), &mut request).await;
        if result.is_err() { request.abort(); let _ = request.await; }
        let closed = runtime.close("slow-diff", 1).await;
        checks.unwrap(); released.unwrap(); closed.unwrap();
        assert!(pending, "a consulta terminou antes de liberar o Git");
        let response = result.unwrap().unwrap().unwrap();
        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert_eq!(body["path"], "tracked.txt");
        assert!(body["diff"].as_str().unwrap().contains("+changed\n"));
        assert_eq!(f.context_hits.load(Ordering::Relaxed), 0);
        assert_eq!(f.hits(), 0);
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path(); let repo = root.join("repo"); let bin = root.join("bin");
    std::fs::create_dir(&repo).unwrap(); std::fs::create_dir(&bin).unwrap();
    let fixed = |args: &[&str]| {
        let out = std::process::Command::new("git").arg("-C").arg(&repo).args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    };
    for args in [&["init","-q","-b","main"][..], &["config","user.name","Teste"], &["config","user.email","teste@example.invalid"],
        &["config","commit.gpgSign","false"], &["config","core.hooksPath","/dev/null"]] { fixed(args); }
    std::fs::write(repo.join("tracked.txt"), "one\n").unwrap();
    fixed(&["add","tracked.txt"]); fixed(&["commit","-qm","initial"]);
    std::fs::write(repo.join("tracked.txt"), "changed\n").unwrap();
    for name in ["ready", "release"] {
        assert!(std::process::Command::new("mkfifo").arg(root.join(name)).status().unwrap().success());
    }
    crate::write_executable(&bin.join("git"), format!("#!/bin/sh\ncase \" $* \" in\n*' diff HEAD -- tracked.txt '*) printf R > '{}'\nIFS= read -r value < '{}' ;;\nesac\nexec /usr/bin/git \"$@\"\n",
        root.join("ready").display(), root.join("release").display()));
    let path = std::env::join_paths(std::iter::once(bin).chain(std::env::split_paths(&std::env::var_os("PATH").unwrap()))).unwrap();
    let child = std::process::Command::new(std::env::current_exe().unwrap()).args(["--exact", TEST, "--nocapture"])
        .env("PATH", path).env("HANGAR_WORKSPACE_SLOW_DIFF_ROOT", root)
        .env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1").output().unwrap();
    assert!(child.status.success(), "{}\n{}", String::from_utf8_lossy(&child.stdout), String::from_utf8_lossy(&child.stderr));
}

#[test]
fn task3_external_citation_rows_do_not_panic() {
    use proptest::{prelude::*,test_runner::{Config,RngSeed,TestRunner}};
    let mut runner=TestRunner::new(Config {cases:128,rng_seed:RngSeed::Fixed(0xf0304),..Config::default()});
    runner.run(&".{0,256}",|text| {
        let rows=vec![json!({"cwd":"/fixture","text":text}).to_string()];
        let needles=vec![text.clone(),"config.json".into(),"config.json.bak".into()];
        let value=hangar_workspace::citations::cwds(hangar_workspace::citations::Transcript::Rows(&rows),&needles);
        prop_assert!(value.is_object());
        Ok(())
    }).unwrap();
}
