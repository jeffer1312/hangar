use std::sync::Arc;
use axum::http::StatusCode;
use hangar_server::{auth::TrustedHosts, config::Config, routes::{terminal_router, AppState}, terminal_control::{Limits, TerminalPool}};
use tokio::net::TcpListener;

async fn server() -> (String, tokio::task::JoinHandle<()>) {
    server_with_pool(TerminalPool::with_program("/does-not-exist/hangar-test-tmux", None, Limits::default())).await
}

async fn server_with_pool(pool: TerminalPool) -> (String, tokio::task::JoinHandle<()>) {
    let cfg = Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:1".parse().unwrap(),
        internal_secret: "internal".into(), auth_token: "owner".into(), log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1") };
    // Nenhum pedido válido pode alcançar o tmux do usuário.
    let state = Arc::new(AppState::with_terminal_pool(cfg, pool));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/__hangar_server/terminal", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, terminal_router(state).into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
    (url, task)
}

#[tokio::test]
async fn auth_precedes_body_and_owner_is_not_internal() {
    let (url, task) = server().await;
    let client = reqwest::Client::new();
    for secret in ["", "owner", "wrong"] {
        let r = client.post(&url).header("x-hangar-internal", secret).header("authorization", "Bearer owner")
            .body("not json").send().await.unwrap();
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
    let r = client.post(&url).header("x-hangar-internal", "internal")
        .header("x-forwarded-for", "198.51.100.1").body("not json").send().await.unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    task.abort();
}

#[tokio::test]
async fn typed_operations_limit_body_and_capture_failure_is_not_empty_success() {
    let (url, task) = server().await;
    let client = reqwest::Client::new();
    for body in ["{}", "{\"op\":\"release\",\"consumer\":\"c\",\"extra\":1}", "not json"] {
        let r = client.post(&url).header("x-hangar-internal", "internal").body(body).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert_eq!(r.text().await.unwrap(), "invalid terminal request");
    }
    let r = client.post(&url).header("x-hangar-internal", "internal")
        .body(serde_json::json!({"op":"release", "consumer":"unknown"}).to_string()).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let r = client.post(&url).header("x-hangar-internal", "internal")
        .body(serde_json::json!({"op":"capture", "consumer":"c", "name":"s", "provider":"claude", "binding":"b",
            "target":"%8", "started":1.0, "lines":200, "colors":false, "join":false}).to_string()).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::SERVICE_UNAVAILABLE);
    task.abort();
}

#[tokio::test]
async fn semantic_capture_errors_are_bad_requests() {
    let (url, task) = server().await;
    let client = reqwest::Client::new();
    let capture = serde_json::json!({"op":"capture", "consumer":"c", "name":"s", "provider":"claude", "binding":"b",
        "target":"%8", "started":1.0, "lines":200, "colors":false, "join":false});
    for (field, value) in [("provider", serde_json::json!("kimi")), ("target", serde_json::json!("command")),
        ("binding", serde_json::json!("")), ("lines", serde_json::json!(10001)), ("started", serde_json::json!(true))] {
        let mut body = capture.clone();
        body[field] = value;
        let r = client.post(&url).header("x-hangar-internal", "internal").body(body.to_string()).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST, "{field}");
        assert_eq!(r.text().await.unwrap(), "invalid terminal request");
    }
    let r = client.post(&url).header("x-hangar-internal", "internal").body(vec![b'x'; hangar_server::terminal_routes::MAX_BODY + 1]).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    task.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn route_producers_share_fake_actor_acquire_renews_and_final_release_reaps() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let program = dir.path().join("observer.py");
    std::fs::write(&program, r#"#!/usr/bin/env python3
import os, pathlib, sys
root = pathlib.Path(__file__).parent
with root.joinpath('pids').open('a') as log: log.write(str(os.getpid()) + '\n')
def frame(n, body):
    sys.stdout.write(f'%begin 1 {n} 0\n' + body + f'%end 1 {n} 0\n')
    sys.stdout.flush()
frame(1, '')
for n, line in enumerate(sys.stdin, 2):
    with root.joinpath('commands').open('a') as log: log.write(line)
    parts = line.rstrip('\n').split(' ; ')
    assert len(parts) == 3
    assert parts[0].startswith('display-message -p HG_START_')
    assert parts[2].startswith('display-message -p HG_END_')
    frame(n * 100, parts[0].removeprefix('display-message -p ') + '\n')
    command = parts[1]
    assert command.startswith(('display-message -p -t ', 'capture-pane -p '))
    frame(n * 100 + 17, '%3\tfixture\t20\t4\t0\t0\t0\n' if command.startswith('display-message') else 'ready\n\n\n\n')
    frame(n * 100 + 39, parts[2].removeprefix('display-message -p ') + '\n')
"#).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    let (url, task) = server_with_pool(TerminalPool::with_program(program, None, Limits::default())).await;
    let client = reqwest::Client::new();
    let mut body = serde_json::json!({"op":"acquire", "consumer":"state", "name":"fixture", "provider":"claude", "binding":"b",
        "target":"%3", "started":42.5, "lines":200, "colors":false, "join":false});
    let post = |body: serde_json::Value| client.post(&url).header("x-hangar-internal", "internal").body(body.to_string());
    assert_eq!(post(body.clone()).send().await.unwrap().status(), StatusCode::OK);
    let commands = std::fs::read_to_string(dir.path().join("commands")).unwrap();
    for consumer in ["preview", "state"] {
        body["consumer"] = serde_json::json!(consumer);
        assert_eq!(post(body.clone()).send().await.unwrap().status(), StatusCode::OK);
    }
    assert_eq!(std::fs::read_to_string(dir.path().join("commands")).unwrap(), commands);
    assert_eq!(std::fs::read_to_string(dir.path().join("pids")).unwrap().lines().count(), 1);
    body["op"] = serde_json::json!("capture");
    let r = post(body).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    let captured: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    assert_eq!(captured["binding"], "b");
    assert_eq!(captured["started"], 42.5);
    assert_eq!(captured["text"], "ready\n\n\n\n");
    let pid = std::fs::read_to_string(dir.path().join("pids")).unwrap().trim().to_owned();
    let process_exists = || std::process::Command::new("kill").args(["-0", &pid])
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().is_ok_and(|status| status.success());
    assert!(process_exists());
    for consumer in ["unknown", "state", "preview"] {
        let r = post(serde_json::json!({"op":"release", "consumer":consumer})).send().await.unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(process_exists(), consumer != "preview");
    }
    task.abort();
}

#[tokio::test]
async fn specific_public_bind_advertises_private_loopback_and_stop_closes_both() {
    let public = TcpListener::bind("[::1]:0").await.unwrap();
    let public_addr = public.local_addr().unwrap();
    let cfg = Config { listen: public_addr, upstream: "127.0.0.1:1".parse().unwrap(),
        internal_secret: "internal".into(), auth_token: "owner".into(), log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1") };
    let pool = TerminalPool::with_program("/does-not-exist/hangar-test-tmux", None, Limits::default());
    let (stop, stopping) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        tokio::select! {
            r = hangar_server::routes::serve_with_terminal_pool(public, cfg, pool) => r.unwrap(),
            _ = stopping => (),
        }
    });
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let r = client.get(format!("http://{public_addr}/__hangar_server/health")).send().await.unwrap();
    let health: serde_json::Value = serde_json::from_str(&r.text().await.unwrap()).unwrap();
    let private: std::net::SocketAddr = health["terminal_address"].as_str().unwrap().parse().unwrap();
    assert_eq!(private.ip(), "127.0.0.1".parse::<std::net::IpAddr>().unwrap());
    assert_ne!(private.port(), 0);
    let release = serde_json::json!({"op":"release", "consumer":"unknown"});
    let r = client.post(format!("http://{private}/__hangar_server/terminal")).header("x-hangar-internal", "internal")
        .body(release.to_string()).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(client.get(format!("http://{private}/__hangar_server/health")).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    assert_eq!(client.post(format!("http://{public_addr}/__hangar_server/terminal")).header("x-hangar-internal", "internal")
        .body(release.to_string()).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    let plugin: std::net::SocketAddr = format!("127.0.0.1:{}", public_addr.port()).parse().unwrap();
    assert_eq!(client.get(format!("http://{plugin}/api/sessions")).send().await.unwrap().status(), StatusCode::NOT_FOUND);
    stop.send(()).unwrap();
    task.await.unwrap();
    assert!(tokio::net::TcpStream::connect(public_addr).await.is_err());
    assert!(tokio::net::TcpStream::connect(private).await.is_err());
    assert!(tokio::net::TcpStream::connect(plugin).await.is_err());
}

#[tokio::test]
async fn unauthorized_tcp_origin_never_polls_request_body() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let cfg = Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:1".parse().unwrap(),
        internal_secret: "internal".into(), auth_token: "owner".into(), log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1") };
    let state = Arc::new(AppState::with_terminal_pool(cfg, TerminalPool::with_program("/does-not-exist/hangar-test-tmux", None, Limits::default())));
    for (peer, secret) in [("198.51.100.2:5000", "internal"), ("127.0.0.1:5000", "owner")] {
        let polls = Arc::new(AtomicUsize::new(0));
        let probe = polls.clone();
        let stream = futures_util::stream::once(async move {
            probe.fetch_add(1, Ordering::SeqCst);
            Ok::<bytes::Bytes, std::io::Error>(bytes::Bytes::from_static(b"not-json"))
        });
        let req = axum::http::Request::builder().header("x-hangar-internal", secret)
            .body(axum::body::Body::from_stream(stream)).unwrap();
        let r = hangar_server::terminal_routes::terminal(axum::extract::State(state.clone()), axum::extract::ConnectInfo(peer.parse().unwrap()), req).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        assert_eq!(polls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn forwarded_external_origin_in_any_header_cannot_authorize() {
    let cfg = Config { listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:1".parse().unwrap(),
        internal_secret: "internal".into(), auth_token: "owner".into(), log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1") };
    let state = Arc::new(AppState::with_terminal_pool(cfg, TerminalPool::with_program("/does-not-exist/hangar-test-tmux", None, Limits::default())));
    for second in [axum::http::HeaderValue::from_static("198.51.100.2"), axum::http::HeaderValue::from_bytes(b"\xff").unwrap()] {
        let mut req = axum::http::Request::builder().header("x-hangar-internal", "internal")
            .body(axum::body::Body::from("invalid-json")).unwrap();
        req.headers_mut().append("x-forwarded-for", axum::http::HeaderValue::from_static("127.0.0.1"));
        req.headers_mut().append("x-forwarded-for", second);
        let r = hangar_server::terminal_routes::terminal(axum::extract::State(state.clone()), axum::extract::ConnectInfo("127.0.0.1:12345".parse().unwrap()), req).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
    }
}

#[tokio::test]
async fn removed_pure_reduce_rpc_is_rejected() {
    let (url, task) = server().await;
    let body = serde_json::json!({"op":"reduce","pane":"ready", "memory":{"prev_spinner":null,"frozen":0,
        "no_spinner":0,"held_state":"idle","held_label":null}, "facts":{"open_question":null,
        "plugin_question":null,"plugin_state":null,"hook_state":null,"hook_grace":8,"status_line":null}});
    let r = reqwest::Client::new().post(url).header("x-hangar-internal", "internal").body(body.to_string()).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    assert_eq!(hangar_server::INTERNAL_PROTOCOL, 51);
    task.abort();
}
