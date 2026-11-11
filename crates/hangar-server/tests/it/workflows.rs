use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::ConnectInfo;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use tower::ServiceExt;

use hangar_core::guest::{Caller, GuestGrant, Kind};
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::locate::ListLocate;
use hangar_server::routes::{AppState, router};

use super::fake::{OWNER, SECRET, config};

async fn scene() -> (tempfile::TempDir, axum::Router, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let script = super::list_support::sessions(root.path(), 2);
    let home = root.path().join("home");
    let empty_project =
        home.join(".claude/projects")
            .join(hangar_workspace::worktrees::sanitize_cwd(
                root.path().join("w1").to_str().unwrap(),
            ));
    std::fs::remove_file(empty_project.join("00000000-0000-0000-0000-000000000001.jsonl")).unwrap();
    let dirs = parse_dirs(
        &json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")})
        .to_string(),
    );
    let mut state = AppState::new(config("127.0.0.1:9".parse().unwrap(), ""));
    state.origins_home = home.clone();
    let list = Arc::new(ListBridge::new(
        ListEnv {
            mux: Mux::with_program(&script, Duration::from_secs(30)),
            capture_program: script.into_os_string(),
            procs: Arc::new(hangar_server::list::procs::SystemProcs::default()),
            dirs,
        },
        FactsClient::new(state.cfg.upstream, SECRET.into()),
    ));
    let row = list.find_row("s0").await.unwrap().unwrap();
    let empty = list.find_row("s1").await.unwrap().unwrap();
    assert!(empty.jsonl.as_ref().is_none_or(|path| path.is_empty()));
    let transcript = PathBuf::from(row.jsonl.unwrap());
    let artifacts = transcript.with_extension("");
    std::fs::create_dir_all(artifacts.join("workflows")).unwrap();
    std::fs::write(artifacts.join("workflows/wf_abc.json"), json!({
        "runId": "wf_abc", "workflowName": "rev", "phases": [{"title": "P", "detail": "d"}],
        "workflowProgress": [{"type": "workflow_agent", "agentId": "a1", "label": "rev", "tokens": 99}],
    }).to_string()).unwrap();
    state.list = list.clone();
    state.side.locate = Arc::new(ListLocate { list });
    (root, router(Arc::new(state)), transcript)
}

async fn request(
    app: &axum::Router,
    path: &str,
    method: &str,
    auth: bool,
    guest: Option<Kind>,
) -> (StatusCode, axum::http::HeaderMap, Value) {
    let mut request = Request::builder().uri(path).method(method);
    if auth {
        request = request.header("authorization", format!("Bearer {OWNER}"));
    }
    let mut request = request.body(Body::empty()).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo::<SocketAddr>("127.0.0.1:1234".parse().unwrap()));
    if let Some(kind) = guest {
        request
            .extensions_mut()
            .insert(Caller::Guest(Arc::new(GuestGrant {
                token: "guest".into(),
                token_hash: "guest-hash".into(),
                sessions: HashMap::from([("s0".into(), kind)]),
                pair_roots: vec![],
                invite: true,
            })));
    }
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, headers, body)
}

#[tokio::test]
async fn workflows_routes_read_all_three_levels_without_python() {
    let (_root, app, _transcript) = scene().await;
    let (status, _, list) = request(&app, "/api/sessions/s0/workflows", "GET", true, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list[0]["runId"], "wf_abc");
    let (status, _, detail) =
        request(&app, "/api/sessions/s0/workflows/wf_abc", "GET", true, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["phases"][0]["title"], "P");
    assert_eq!(detail["agents"][0]["agentId"], "a1");
    let (status, _, agent) = request(
        &app,
        "/api/sessions/s0/workflows/wf_abc/agents/a1",
        "GET",
        true,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(agent["tokens"], 99);
    assert_eq!(agent["label"], "rev");
    for (path, code) in [
        ("/api/sessions/missing/workflows", "erro_sessao_inexistente"),
        ("/api/sessions/s1/workflows", "erro_sessao_inexistente"),
        (
            "/api/sessions/s0/workflows/wf_missing",
            "erro_workflow_inexistente",
        ),
        (
            "/api/sessions/s0/workflows/wf_abc/agents/missing",
            "erro_agente_inexistente",
        ),
    ] {
        let (status, _, body) = request(&app, path, "GET", true, None).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["detail"]["code"], code);
    }
}

#[tokio::test]
async fn workflows_routes_preserve_auth_guest_permissions_and_routing() {
    let (_root, app, _transcript) = scene().await;
    assert_eq!(
        request(&app, "/api/sessions/s0/workflows", "GET", false, None)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &app,
            "/api/sessions/s0/workflows",
            "GET",
            false,
            Some(Kind::Share)
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        request(
            &app,
            "/api/sessions/s0/workflows",
            "GET",
            false,
            Some(Kind::Pair)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &app,
            "/api/sessions/other/workflows",
            "GET",
            false,
            Some(Kind::Share)
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, headers, _) =
        request(&app, "/api/sessions/s0/workflows", "POST", true, None).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(headers["allow"], "GET");
    assert_eq!(
        request(&app, "/api/sessions/s0/workflows", "HEAD", true, None)
            .await
            .0,
        StatusCode::METHOD_NOT_ALLOWED
    );
    let (status, headers, _) =
        request(&app, "/api/sessions/s0/workflows/?x=1", "GET", true, None).await;
    assert_eq!(status, StatusCode::TEMPORARY_REDIRECT);
    assert!(
        headers["location"]
            .to_str()
            .unwrap()
            .ends_with("/api/sessions/s0/workflows?x=1")
    );
    assert_eq!(
        request(
            &app,
            "/api/sessions/s0/workflows/wf%2Fescape",
            "GET",
            true,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}
