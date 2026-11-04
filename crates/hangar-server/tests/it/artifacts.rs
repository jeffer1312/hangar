use super::fake::{OWNER, append_raw, client};
use axum::http::StatusCode;
use hangar_server::{
    auth::TrustedHosts,
    config::Config,
    locate::{BoxFuture, InfoFailed, Locate},
    routes::{AppState, router},
    transcript::InternalInfo,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct Sessions(Mutex<HashMap<String, InternalInfo>>);
impl Locate for Sessions {
    fn info<'a>(
        &'a self,
        name: &'a str,
    ) -> BoxFuture<'a, Result<Option<InternalInfo>, InfoFailed>> {
        Box::pin(async move { Ok(self.0.lock().unwrap().get(name).cloned()) })
    }
}
struct Scene {
    _dir: tempfile::TempDir,
    path: PathBuf,
    sessions: Arc<Sessions>,
    address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Scene {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Scene {
    async fn new(lines: &[Value]) -> Self {
        Self::with_upstream(lines, "127.0.0.1:9".parse().unwrap()).await
    }
    async fn with_upstream(lines: &[Value], upstream: SocketAddr) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session-a.jsonl");
        write_lines(&path, lines);
        let sessions = Arc::new(Sessions::default());
        sessions
            .0
            .lock()
            .unwrap()
            .insert("s".into(), info(&path, "life-a", "claude"));
        let cfg = Config {
            listen: "127.0.0.1:0".parse().unwrap(),
            upstream,
            auth_token: OWNER.into(),
            internal_secret: "isolated".into(),
            log_path: None,
            trusted: TrustedHosts::parse("127.0.0.1"),
        };
        let mut state = AppState::new(cfg);
        state.side.locate = sessions.clone();
        // Só o Router: sem supervisor, runtime, varredura de órfãos ou backend Python.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(
                listener,
                router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await
            .unwrap();
        });
        Self {
            _dir: dir,
            path,
            sessions,
            address,
            task,
        }
    }
    async fn get(&self, name: &str) -> (StatusCode, Value) {
        self.request(name, Some(OWNER), "").await
    }
    async fn request(&self, name: &str, token: Option<&str>, query: &str) -> (StatusCode, Value) {
        let req = client().get(format!(
            "http://{}/api/sessions/{name}/artifacts{query}",
            self.address
        ));
        let req = if let Some(token) = token {
            req.bearer_auth(token)
        } else {
            req
        };
        let response = req.send().await.unwrap();
        let status = response.status();
        (
            status,
            serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null),
        )
    }
    async fn items(&self) -> Value {
        let (status, body) = self.get("s").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["items"].clone()
    }
}
fn info(path: &Path, life: &str, provider: &str) -> InternalInfo {
    serde_json::from_value(json!({"provider":provider,"jsonl":path,"session_key":path.file_stem().unwrap().to_str().unwrap(),"lifecycle_id":life,"history":{}})).unwrap()
}
fn write_lines(path: &Path, lines: &[Value]) {
    std::fs::write(
        path,
        lines
            .iter()
            .map(|line| format!("{line}\n"))
            .collect::<String>(),
    )
    .unwrap();
}
fn tool(id: &str, name: &str, input: Value) -> Value {
    json!({"type":"assistant","sessionId":"session-a","message":{"content":[{"type":"tool_use","id":id,"name":name,"input":input}]}})
}
fn result(id: &str, metadata: Value, error: bool) -> Value {
    json!({"type":"user","sessionId":"session-a","toolUseResult":metadata,"message":{"content":[{"type":"tool_result","tool_use_id":id,"is_error":error,"content":"Resultado da ferramenta"}]}})
}
fn frame(id: &str, title: &str) -> Value {
    json!({"type":"frame-link","sessionId":"session-a","path":"/fonte/nao-ler.html","frameUrl":format!("https://claude.ai/artifact/{id}"),"title":title,"artifactCount":1})
}
fn metadata(id: &str, title: &str, version: &str) -> Value {
    json!({"url":format!("https://claude.ai/artifact/{id}"),"artifact_id":id,"title":title,"version":version,"updated":false,"seq":1,"audience":"owner","path":"/nao-ler.html"})
}

#[tokio::test]
async fn artifacts_isolated_api_proof() {
    let rows: Vec<Value> = include_str!("../fixtures/artifacts/catalog.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let scene = Scene::new(&rows[..1]).await;
    assert_eq!(scene.items().await, json!([]));
    for row in &rows[1..4] {
        append_raw(&scene.path, &format!("{row}\n"));
    }
    assert_eq!(
        scene.items().await,
        json!([{"id":"demo-chart","url":"https://claude.ai/artifact/demo-chart","title":"Gráfico publicado","version":"demo-v1"}])
    );
    for row in &rows[4..] {
        append_raw(&scene.path, &format!("{row}\n"));
    }
    let (status, body) = scene.get("s").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body,
        json!({"items":[
        {"id":"demo-chart","url":"https://claude.ai/artifact/demo-chart","title":"Gráfico atualizado","version":"demo-v2"},
        {"id":"123e4567-e89b-12d3-a456-426614174000","url":"https://claude.ai/code/artifact/123e4567-e89b-12d3-a456-426614174000","title":"Página do modelo","version":"template-v1"}]})
    );
    println!("API isolada: HTTP {status}; {body}");
    assert_eq!(scene.request("s", None, "").await.0, StatusCode::FORBIDDEN);
    assert_eq!(scene.get("missing").await.0, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn artifacts_frame_and_implicit_publish_are_one_item_with_latest_metadata() {
    let scene = Scene::new(&[frame("a", "Título inicial"),
        json!({"type":"frame-link","sessionId":"session-a","artifactCount":1}),
        tool("p", "Artifact", json!({"file_path":"/nao-ler.html","icon":"chart","description":"Descrição"})),
        result("p", metadata("a", "Título publicado", "v1"), false),
        tool("u", "Artifact", json!({"action":"publish","url":"https://claude.ai/artifact/a","file_path":"/nao-ler.html"})),
        result("u", json!({"url":"https://claude.ai/artifact/a","artifact_id":"a","title":"Título atualizado","version":"v2","updated":true,"seq":2}), false)]).await;
    assert_eq!(
        scene.items().await,
        json!([{"id":"a","url":"https://claude.ai/artifact/a","title":"Título atualizado","version":"v2"}])
    );
}

#[tokio::test]
async fn artifacts_template_and_code_urls_need_no_html_source() {
    let scene = Scene::new(&[tool("t", "functions.Artifact", json!({"type_url":"https://claude.ai/artifact/template","title":"Do modelo"})),
        result("t", json!({"created_from_type":true,"url":"https://claude.ai/code/artifact/123e4567-e89b-12d3-a456-426614174000","title":"Do modelo","version":"template-v1","type":{},"own_files":[],"type_files":[]}), false),
        frame("b", "")]).await;
    assert_eq!(
        scene.items().await,
        json!([
        {"id":"123e4567-e89b-12d3-a456-426614174000","url":"https://claude.ai/code/artifact/123e4567-e89b-12d3-a456-426614174000","title":"Do modelo","version":"template-v1"},
        {"id":"b","url":"https://claude.ai/artifact/b","title":"","version":null}])
    );
}

#[tokio::test]
async fn artifacts_read_list_quickstart_failures_and_plain_text_are_not_publications() {
    let mut lines = vec![
        json!({"type":"assistant","message":{"content":[{"type":"text","text":"https://claude.ai/artifact/inventado"}]}}),
        result("orphan", metadata("orphan", "Não publicar", "v"), false),
    ];
    for (id, action) in [
        ("r", "read"),
        ("l", "list"),
        ("q", "quickstart"),
        ("unknown", "open"),
    ] {
        lines.push(tool(id, "Artifact", json!({"action":action})));
        lines.push(result(id, metadata(id, "Não publicar", "v"), false));
    }
    lines.extend([
        tool("f", "Artifact", json!({"action":"publish"})),
        result("f", metadata("failed", "Falhou", "v"), true),
        tool("deny", "Artifact", json!({})),
        result("deny", json!({"error":"negado"}), false),
        tool("other", "OtherArtifact", json!({})),
        result("other", metadata("other", "Outra ferramenta", "v"), false),
    ]);
    let scene = Scene::new(&lines).await;
    assert_eq!(scene.items().await, json!([]));
}

#[tokio::test]
async fn artifacts_reject_unsafe_urls_even_in_structured_metadata() {
    let urls = [
        "http://claude.ai/artifact/a",
        "javascript:alert(1)",
        "https://evil.test/artifact/a",
        "https://sub.claude.ai/artifact/a",
        "https://claude.ai.evil.test/artifact/a",
        "https://u:p@claude.ai/artifact/a",
        "https://claude.ai:444/artifact/a",
        "https://claude.ai/artifact/a?token=secret",
        "https://claude.ai/artifact/a#secret",
        "https://claude.ai/artifact/a/extra",
        "https://claude.ai/artifact/%2e%2e",
        "https://claude.ai/not-artifact/a",
        "https://claude.ai/artifact/",
        "https://claude.ai/code/artifact/not-a-uuid",
        "https://claude.ai/code/artifact/gggggggg-e89b-12d3-a456-426614174000",
        "https://claude.ai/code/artifact/123e4567e89b12d3a456426614174000",
    ];
    let mut lines = Vec::new();
    for (i, url) in urls.iter().enumerate() {
        let id = format!("p{i}");
        lines.extend([
            tool(&id, "Artifact", json!({})),
            result(&id, json!({"url":url,"title":"Inválido"}), false),
            json!({"type":"frame-link","sessionId":"session-a","frameUrl":url,"title":"Inválido"}),
        ]);
    }
    let scene = Scene::new(&lines).await;
    assert_eq!(scene.items().await, json!([]));
}

#[tokio::test]
async fn artifacts_catalog_is_not_the_paginated_history_and_cannot_select_another_transcript() {
    let mut lines = vec![frame("early", "Do início")];
    for i in 0..600 {
        lines.push(json!({"type":"assistant","message":{"content":[{"type":"text","text":format!("Mensagem {i}")}]}}));
    }
    let scene = Scene::new(&lines).await;
    let other = scene._dir.path().join("other.jsonl");
    write_lines(
        &other,
        &[
            json!({"type":"frame-link","frameUrl":"https://claude.ai/artifact/other","title":"Outra sessão"}),
        ],
    );
    scene
        .sessions
        .0
        .lock()
        .unwrap()
        .insert("other".into(), info(&other, "other-life", "claude"));
    let (status, body) = scene
        .request(
            "s",
            Some(OWNER),
            &format!(
                "?limit=1&jsonl={}&path={}&session=other",
                other.display(),
                other.display()
            ),
        )
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["items"],
        json!([{"id":"early","url":"https://claude.ai/artifact/early","title":"Do início","version":null}])
    );
    let (status, other_body) = scene.get("other").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(other_body["items"][0]["id"], "other");
    let response = client()
        .get(format!(
            "http://{}/api/sessions/s/history?limit=400",
            scene.address
        ))
        .bearer_auth(OWNER)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let history: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap();
    assert_eq!(history.as_array().unwrap().len(), 400);
    assert_eq!(scene.items().await.as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn artifacts_increment_waits_for_partial_line_and_preserves_order() {
    let scene = Scene::new(&[frame("a", "Primeiro")]).await;
    assert_eq!(scene.items().await.as_array().unwrap().len(), 1);
    let second = frame("b", "Segundo").to_string();
    let cut = second.len() / 2;
    append_raw(&scene.path, &second[..cut]);
    assert_eq!(scene.items().await.as_array().unwrap().len(), 1);
    append_raw(&scene.path, &format!("{}\n", &second[cut..]));
    let items = scene.items().await;
    assert_eq!(
        items
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["a", "b"]
    );
    assert_eq!(scene.items().await, items);
}

#[tokio::test]
async fn artifacts_tool_result_in_next_poll_keeps_only_its_sessions_pending_publication() {
    let scene = Scene::new(&[tool("pending", "Artifact", json!({}))]).await;
    assert_eq!(scene.items().await, json!([]));
    let other = scene._dir.path().join("other.jsonl");
    let mut foreign = result("pending", metadata("foreign", "Outra sessão", "v1"), false);
    foreign["sessionId"] = json!("other");
    write_lines(&other, &[foreign]);
    scene
        .sessions
        .0
        .lock()
        .unwrap()
        .insert("other".into(), info(&other, "other-life", "claude"));
    assert_eq!(scene.get("other").await.1["items"], json!([]));
    let own = result("pending", metadata("own", "Desta sessão", "v1"), false);
    append_raw(&scene.path, &format!("{own}\n"));
    assert_eq!(scene.items().await[0]["id"], "own");
}

#[tokio::test]
async fn artifacts_truncation_and_same_size_replacement_invalidate_the_index() {
    let scene = Scene::new(&[frame("a", "Antigo")]).await;
    assert_eq!(scene.items().await[0]["id"], "a");
    write_lines(&scene.path, &[]);
    assert_eq!(scene.items().await, json!([]));
    write_lines(&scene.path, &[frame("b", "Novo")]);
    assert_eq!(scene.items().await[0]["id"], "b");
    let replacement = scene.path.with_extension("new");
    write_lines(&replacement, &[frame("c", "Novo")]);
    std::fs::rename(replacement, &scene.path).unwrap();
    assert_eq!(scene.items().await[0]["id"], "c");
    write_lines(&scene.path, &[frame("d", "Novo")]);
    assert_eq!(scene.items().await[0]["id"], "d");
    assert_eq!(scene.items().await[0]["title"], "Novo");
    let before = std::fs::metadata(&scene.path).unwrap();
    write_lines(&scene.path, &[frame("d", "Tela")]);
    // Mantém até o mtime: não depender do relógio nem de sleep para comprovar a substituição.
    std::fs::OpenOptions::new()
        .write(true)
        .open(&scene.path)
        .unwrap()
        .set_modified(before.modified().unwrap())
        .unwrap();
    let after = std::fs::metadata(&scene.path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.dev(), after.dev());
    }
    assert_eq!(scene.items().await[0]["title"], "Tela");
}

#[tokio::test]
async fn artifacts_larger_file_swap_with_preserved_read_edges_invalidates_cache() {
    let padding = json!({"type":"progress","padding":"x".repeat(2048)});
    let scene = Scene::new(&[padding.clone(), frame("center", "Antigo"), padding.clone()]).await;
    assert_eq!(scene.items().await[0]["title"], "Antigo");
    let original = std::fs::read(&scene.path).unwrap();
    let before = std::fs::metadata(&scene.path).unwrap();
    let replacement = scene.path.with_extension("replacement");
    write_lines(
        &replacement,
        &[
            padding.clone(),
            frame("center", "Mudado"),
            padding,
            json!({"type":"progress","extra":true}),
        ],
    );
    let rewritten = std::fs::read(&replacement).unwrap();
    assert!(rewritten.len() > original.len());
    assert_eq!(&original[..512], &rewritten[..512]);
    assert_eq!(
        &original[original.len() - 512..],
        &rewritten[original.len() - 512..original.len()]
    );
    std::fs::rename(replacement, &scene.path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_ne!(before.ino(), std::fs::metadata(&scene.path).unwrap().ino());
    }
    #[cfg(not(unix))]
    let _ = before;
    assert_eq!(scene.items().await[0]["title"], "Mudado");
}

#[tokio::test]
async fn artifacts_rewrite_of_indexed_middle_followed_by_append_invalidates_prefix() {
    let padding = json!({"type":"progress","padding":"x".repeat(2048)});
    let scene = Scene::new(&[padding.clone(), frame("center", "Antigo"), padding.clone()]).await;
    assert_eq!(scene.items().await[0]["title"], "Antigo");
    let before = std::fs::metadata(&scene.path).unwrap();
    let original = std::fs::read(&scene.path).unwrap();
    write_lines(
        &scene.path,
        &[padding.clone(), frame("center", "Mudado"), padding],
    );
    append_raw(&scene.path, "{\"type\":\"progress\",\"extra\":true}\n");
    let after = std::fs::metadata(&scene.path).unwrap();
    let rewritten = std::fs::read(&scene.path).unwrap();
    assert!(rewritten.len() > original.len());
    assert_eq!(before.created().ok(), after.created().ok());
    assert_eq!(&original[..512], &rewritten[..512]);
    assert_eq!(
        &original[original.len() - 512..],
        &rewritten[original.len() - 512..original.len()]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.dev(), after.dev());
    }
    assert_eq!(scene.items().await[0]["title"], "Mudado");
    assert_eq!(scene.items().await[0]["title"], "Mudado");
    println!(
        "Reescrita seguida de acréscimo: prefixo P={} bytes; arquivo novo F={} bytes; custo lógico esperado P+F={} bytes",
        original.len(),
        rewritten.len(),
        original.len() + rewritten.len()
    );
}

#[tokio::test]
async fn artifacts_same_size_rewrite_in_the_middle_with_restored_mtime_invalidates_cache() {
    let padding = json!({"type":"progress","padding":"x".repeat(2048)});
    let scene = Scene::new(&[padding.clone(), frame("center", "Antigo"), padding.clone()]).await;
    assert_eq!(scene.items().await[0]["title"], "Antigo");
    let before = std::fs::metadata(&scene.path).unwrap();
    let original = std::fs::read(&scene.path).unwrap();
    write_lines(
        &scene.path,
        &[padding.clone(), frame("center", "Mudado"), padding],
    );
    std::fs::OpenOptions::new()
        .write(true)
        .open(&scene.path)
        .unwrap()
        .set_modified(before.modified().unwrap())
        .unwrap();
    let after = std::fs::metadata(&scene.path).unwrap();
    let rewritten = std::fs::read(&scene.path).unwrap();
    assert_eq!(before.len(), after.len());
    assert_eq!(before.created().ok(), after.created().ok());
    assert_eq!(before.modified().unwrap(), after.modified().unwrap());
    assert_eq!(&original[..512], &rewritten[..512]);
    assert_eq!(
        &original[original.len() - 512..],
        &rewritten[rewritten.len() - 512..]
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_eq!(before.ino(), after.ino());
        assert_eq!(before.dev(), after.dev());
    }
    assert_eq!(scene.items().await[0]["title"], "Mudado");
}

#[tokio::test]
async fn artifacts_preflight_uses_the_existing_cors_upstream_without_bypassing_get_auth() {
    use axum::{extract::Request, response::IntoResponse};
    struct Stop(tokio::task::JoinHandle<()>);
    impl Drop for Stop {
        fn drop(&mut self) {
            self.0.abort();
        }
    }
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = upstream_listener.local_addr().unwrap();
    let cors = axum::Router::new()
        .route(
            "/api/sessions/{name}/artifacts",
            axum::routing::options(|req: Request| async move {
                assert_eq!(req.headers()["origin"], "https://app.example.invalid");
                assert_eq!(req.headers()["access-control-request-method"], "GET");
                assert_eq!(
                    req.headers()["access-control-request-headers"],
                    "authorization"
                );
                assert!(req.headers().get("authorization").is_none());
                (
                    StatusCode::OK,
                    [
                        ("access-control-allow-origin", "https://app.example.invalid"),
                        ("access-control-allow-methods", "GET"),
                        ("access-control-allow-headers", "authorization"),
                    ],
                    "",
                )
                    .into_response()
            }),
        )
        .fallback(|| async { StatusCode::UNAUTHORIZED });
    let _stop = Stop(tokio::spawn(async move {
        axum::serve(upstream_listener, cors).await.unwrap();
    }));
    let scene = Scene::with_upstream(&[frame("a", "Privado")], upstream).await;
    let response = client()
        .request(
            reqwest::Method::OPTIONS,
            format!("http://{}/api/sessions/s/artifacts", scene.address),
        )
        .header("origin", "https://app.example.invalid")
        .header("access-control-request-method", "GET")
        .header("access-control-request-headers", "authorization")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "https://app.example.invalid"
    );
    assert_eq!(response.headers()["access-control-allow-methods"], "GET");
    assert_eq!(
        response.headers()["access-control-allow-headers"],
        "authorization"
    );
    assert_eq!(scene.request("s", None, "").await.0, StatusCode::FORBIDDEN);
    assert_eq!(scene.items().await[0]["id"], "a");
}

#[tokio::test]
async fn artifacts_missing_file_is_an_error_and_an_invalid_line_is_skipped() {
    let scene = Scene::new(&[frame("a", "Antigo")]).await;
    assert_eq!(scene.items().await[0]["id"], "a");
    std::fs::remove_file(&scene.path).unwrap();
    for _ in 0..2 {
        let (status, body) = scene.get("s").await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "sumido continua erro na leitura seguinte");
        assert_eq!(body["error_code"], "artifacts_io");
    }
    write_lines(&scene.path, &[frame("b", "Novo")]);
    assert_eq!(scene.items().await[0]["id"], "b");
    // Dois registros colados e o primeiro cortado, como o Claude Code grava às vezes: a linha sai, o resto vale.
    append_raw(&scene.path, "{json quebrado}\n");
    append_raw(&scene.path, &format!("{}\n", frame("d", "Depois")));
    let ids: Vec<_> = scene.items().await.as_array().unwrap().iter().map(|i| i["id"].clone()).collect();
    assert_eq!(ids, [json!("b"), json!("d")]);
}

/// Vida nova (sessão recém-aberta, depois do `/clear`): o Claude só cria o transcript na primeira mensagem.
/// O que ainda não existe é catálogo vazio; o que já foi lido e sumiu continua erro (acima).
#[tokio::test]
async fn artifacts_transcript_not_yet_created_is_an_empty_catalog() {
    let scene = Scene::new(&[]).await;
    let fresh = scene.path.with_file_name("ainda-nao.jsonl");
    scene.sessions.0.lock().unwrap().insert("s".into(), info(&fresh, "fresh-life", "claude"));
    assert_eq!(scene.items().await, json!([]));
    write_lines(&fresh, &[frame("c", "Primeiro")]);
    assert_eq!(scene.items().await[0]["id"], "c");
}

#[tokio::test]
async fn artifacts_absent_session_unsupported_provider_and_no_transcript_follow_the_contract() {
    let scene = Scene::new(&[frame("a", "Antigo")]).await;
    assert_eq!(scene.get("missing").await.0, StatusCode::NOT_FOUND);
    scene
        .sessions
        .0
        .lock()
        .unwrap()
        .insert("s".into(), info(&scene.path, "codex-life", "codex"));
    assert_eq!(scene.items().await, json!([]));
    let mut no_transcript = info(&scene.path, "new-life", "claude-headless");
    no_transcript.jsonl = None;
    scene
        .sessions
        .0
        .lock()
        .unwrap()
        .insert("s".into(), no_transcript);
    assert_eq!(scene.items().await, json!([]));
}

#[tokio::test]
async fn artifacts_auth_refuses_missing_or_guest_token_without_reading_transcript() {
    let scene = Scene::new(&[frame("a", "Privado")]).await;
    for token in [None, Some("guest-token")] {
        let (status, body) = scene.request("s", token, "").await;
        assert!(
            status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN,
            "{status}: {body}"
        );
        assert!(body.get("items").is_none());
    }
}

#[tokio::test]
async fn artifacts_recreated_renamed_and_other_life_never_reuse_old_items() {
    let scene = Scene::new(&[frame("a", "Antigo")]).await;
    assert_eq!(scene.items().await[0]["id"], "a");
    let old = scene.sessions.0.lock().unwrap().remove("s").unwrap();
    scene
        .sessions
        .0
        .lock()
        .unwrap()
        .insert("renamed".into(), old);
    assert_eq!(scene.get("s").await.0, StatusCode::NOT_FOUND);
    assert_eq!(scene.get("renamed").await.1["items"][0]["id"], "a");
    let new_path = scene._dir.path().join("new-session.jsonl");
    write_lines(
        &new_path,
        &[
            json!({"type":"frame-link","sessionId":"new-session","frameUrl":"https://claude.ai/artifact/new","title":"Novo"}),
        ],
    );
    scene
        .sessions
        .0
        .lock()
        .unwrap()
        .insert("s".into(), info(&new_path, "life-new", "claude"));
    assert_eq!(scene.items().await[0]["id"], "new");
}

#[tokio::test]
async fn artifacts_sidechain_records_are_ignored_but_resume_keeps_the_selected_files_history() {
    let mut resumed = frame("resumed", "Da conversa retomada");
    resumed["sessionId"] = json!("session-before-resume");
    let mut child = frame("child", "Subagente");
    child["isSidechain"] = json!(true);
    let scene = Scene::new(&[resumed, child, frame("right", "Desta sessão")]).await;
    let items = scene.items().await;
    assert_eq!(
        items
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["resumed", "right"]
    );
}

#[tokio::test]
async fn artifacts_resume_rewrite_does_not_revert_the_latest_version() {
    let mut publish = tool("p", "Artifact", json!({}));
    publish["timestamp"] = json!("2026-01-01T00:00:00Z");
    let mut first = result("p", metadata("a", "Primeiro", "v1"), false);
    first["timestamp"] = json!("2026-01-01T00:00:01Z");
    let mut update = tool("u", "Artifact", json!({"action":"publish"}));
    update["timestamp"] = json!("2026-01-01T00:00:02Z");
    let mut latest = result("u", metadata("a", "Último", "v2"), false);
    latest["toolUseResult"]["seq"] = json!(2);
    latest["toolUseResult"]["updated"] = json!(true);
    latest["timestamp"] = json!("2026-01-01T00:00:03Z");
    let early_frame = frame("a", "Antes da publicação");
    let scene = Scene::new(&[
        early_frame.clone(),
        publish.clone(),
        first.clone(),
        update,
        latest,
    ])
    .await;
    assert_eq!(scene.items().await[0]["version"], "v2");
    append_raw(&scene.path, &format!("{early_frame}\n{publish}\n{first}\n"));
    assert_eq!(scene.items().await[0]["version"], "v2");
    assert_eq!(scene.items().await[0]["title"], "Último");
}

#[tokio::test]
async fn artifacts_revision_is_ordered_by_seq_not_uuid_or_wall_clock() {
    let scene = Scene::new(&[]).await;
    for (call, seq, version, timestamp, expected) in [
        (
            "p2",
            2,
            "00000000-0000-4000-8000-000000000002",
            "2026-01-01T01:00:00Z",
            "00000000-0000-4000-8000-000000000002",
        ),
        (
            "old",
            1,
            "ffffffff-ffff-4fff-8fff-ffffffffffff",
            "2026-01-01T01:00:01Z",
            "00000000-0000-4000-8000-000000000002",
        ),
        (
            "p3",
            3,
            "00000000-0000-4000-8000-000000000001",
            "2026-01-01T00:00:00Z",
            "00000000-0000-4000-8000-000000000001",
        ),
    ] {
        let mut call_line = tool(call, "Artifact", json!({"action":"publish"}));
        call_line["timestamp"] = json!(timestamp);
        let mut done = result(call, metadata("a", "Título", version), false);
        done["timestamp"] = json!(timestamp);
        done["toolUseResult"]["seq"] = json!(seq);
        append_raw(&scene.path, &format!("{call_line}\n{done}\n"));
        assert_eq!(scene.items().await[0]["version"], expected);
    }
}

#[tokio::test]
async fn artifacts_update_without_version_never_reports_prior_version_as_current() {
    let scene = Scene::new(&[
        tool("p", "Artifact", json!({})),
        result("p", metadata("a", "Primeiro", "v1"), false),
    ])
    .await;
    assert_eq!(scene.items().await[0]["version"], "v1");
    for row in [
        tool("u", "Artifact", json!({"action":"publish"})),
        result(
            "u",
            json!({"url":"https://claude.ai/artifact/a","title":"Atual","seq":2,"updated":true}),
            false,
        ),
    ] {
        append_raw(&scene.path, &format!("{row}\n"));
    }
    assert_eq!(
        scene.items().await,
        json!([{"id":"a","url":"https://claude.ai/artifact/a","title":"Atual","version":null}])
    );
}

#[tokio::test]
async fn artifacts_delete_needs_positive_confirmation_and_old_records_never_restore_it() {
    let publication = tool("publish-b", "Artifact", json!({}));
    let published = result("publish-b", metadata("b", "Segundo", "b-v1"), false);
    let scene = Scene::new(&[
        frame("a", "Primeiro"),
        frame("b", "Segundo"),
        publication.clone(),
        published.clone(),
    ])
    .await;
    assert_eq!(scene.items().await.as_array().unwrap().len(), 2);
    for row in [
        tool(
            "failed",
            "Artifact",
            json!({"action":"delete","url":"https://claude.ai/artifact/a"}),
        ),
        result("failed", json!({"deleted":true}), true),
        tool(
            "d",
            "Artifact",
            json!({"action":"delete","url":"https://claude.ai/artifact/b"}),
        ),
        result("d", json!({"deleted":true}), false),
    ] {
        append_raw(&scene.path, &format!("{row}\n"));
    }
    assert_eq!(scene.items().await.as_array().unwrap().len(), 1);
    for row in [
        frame("b", "Segundo"),
        publication,
        published,
        tool("replay", "Artifact", json!({})),
        result("replay", metadata("b", "Segundo", "b-v1"), false),
    ] {
        append_raw(&scene.path, &format!("{row}\n"));
    }
    assert_eq!(
        scene.items().await,
        json!([{"id":"a","url":"https://claude.ai/artifact/a","title":"Primeiro","version":null}])
    );
}
