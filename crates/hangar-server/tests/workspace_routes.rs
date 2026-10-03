mod fake;
use axum::{
    Router,
    extract::{Query, State},
    routing::get,
};
use fake::{OWNER, SECRET, client, config, spawn_server};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

async fn fixture(root: &std::path::Path) -> (std::net::SocketAddr, Arc<AtomicUsize>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let context = json!({"roots":[root],"sessions":[{"name":"fixture","cwd":root}],"session":{"name":"fixture","cwd":root,"jsonl":root.join("fixture.jsonl")}});
    let app = Router::new()
        .route(
            "/internal/workspace/context",
            get(
                move |headers: axum::http::HeaderMap,
                      Query(_): Query<std::collections::HashMap<String, String>>| {
                    let context = context.clone();
                    async move {
                        assert_eq!(headers["x-hangar-internal"], SECRET);
                        (
                            [(axum::http::header::CONTENT_TYPE, "application/json")],
                            context.to_string(),
                        )
                    }
                },
            ),
        )
        .fallback(|State(hits): State<Arc<AtomicUsize>>| async move {
            hits.fetch_add(1, Ordering::Relaxed);
            "python-reserva"
        })
        .with_state(hits.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (spawn_server(config(upstream, "127.0.0.1")).await, hits)
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
