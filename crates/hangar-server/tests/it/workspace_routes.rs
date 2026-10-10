use crate::fake;
use crate::workspace_fixture;
use fake::{OWNER, SECRET, client};
use serde_json::{Value, json};
use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use workspace_fixture::refusal;

async fn fixture(root: &std::path::Path) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let f = workspace_fixture::fixture(root, true).await;
    (f.addr, f.hits)
}

fn git(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
}

#[tokio::test]
async fn owner_reads_and_saves_without_using_python_io() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ação.txt"), "original").unwrap();
    let (addr, hits) = fixture(dir.path()).await;
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
    assert_eq!(
        c.get(&url)
            .bearer_auth("convidado")
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap(),
        "python-reserva"
    );
}

#[tokio::test]
async fn private_route_is_unreachable_on_the_public_listener() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _) = fixture(dir.path()).await;
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
    let (addr, _) = fixture(dir.path()).await;
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
async fn invalid_external_write_body_is_forwarded_before_any_mutation() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ação.txt"), "original").unwrap();
    std::fs::write(dir.path().join("fixture.jsonl"), json!({"cwd":dir.path(),"text":"ação.txt"}).to_string()).unwrap();
    let (addr, hits) = fixture(dir.path()).await;
    let result = client().post(format!("http://{addr}/api/sessions/fixture/file/text"))
        .bearer_auth(OWNER).header("content-type", "application/json")
        .body(json!({"path":"ação.txt", "text":"perdida", "digest":"antigo", "extra":true}).to_string()).send().await.unwrap();
    assert_eq!(result.text().await.unwrap(), "python-reserva");
    assert_eq!(hits.load(Ordering::Relaxed), 1);
    assert_eq!(std::fs::read_to_string(dir.path().join("ação.txt")).unwrap(), "original");
}

#[tokio::test]
async fn private_gate_rejects_forwarded_clients_and_wrong_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let (addr, _) = fixture(dir.path()).await;
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
    let (addr, _) = fixture(dir.path()).await;
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
    let (addr, _) = fixture(dir.path()).await;
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
async fn every_public_mutation_rejects_private_context_fields_before_execution() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("arquivo.txt"), "original").unwrap();
    for args in [&["init", "-b", "main"][..], &["config", "user.name", "Teste"], &["config", "user.email", "teste@example.invalid"], &["add", "."], &["commit", "-m", "Inicial"]] {
        assert!(std::process::Command::new("git").arg("-C").arg(dir.path()).args(args).output().unwrap().status.success());
    }
    std::fs::write(dir.path().join("arquivo.txt"), "alteração preservada").unwrap();
    let (addr, hits) = fixture(dir.path()).await;
    let result = client().post(format!("http://{addr}/api/sessions/fixture/git/discard")).bearer_auth(OWNER)
        .header("content-type", "application/json").body(json!({"path":"arquivo.txt", "cwd":"/outro"}).to_string()).send().await.unwrap();
    assert_eq!(result.text().await.unwrap(), "python-reserva");
    assert_eq!(hits.load(Ordering::Relaxed), 1);
    assert_eq!(std::fs::read_to_string(dir.path().join("arquivo.txt")).unwrap(), "alteração preservada");
}

#[tokio::test]
async fn broken_context_answers_503_with_reason() {
    let dir = tempfile::tempdir().unwrap();
    let f = workspace_fixture::fixture(dir.path(), false).await;
    let response = client()
        .get(format!("http://{}/api/sessions/fixture/files/list", f.addr))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    let body = refusal(response, "workspace_context").await;
    assert_eq!(body["detail"]["params"]["motivo"], "status");
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
    let f = workspace_fixture::fixture(&repo, true).await;
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
    let (addr, _) = fixture(dir.path()).await;
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
    let (addr, _) = fixture(dir.path()).await;
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
    let (addr, _) = fixture(dir.path()).await;
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
