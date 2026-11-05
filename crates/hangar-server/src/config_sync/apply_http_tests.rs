use super::*;
use crate::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
};
static TESTS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn state(home: &std::path::Path) -> Arc<AppState> {
    let address: SocketAddr = "127.0.0.1:0".parse().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: address,
        internal_secret: "fixture-internal".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
        ]
        .into(),
    ));
    Arc::new(state)
}

#[tokio::test]
async fn apply_reserves_while_reading_and_caps_streamed_bodies() {
    let _test = TESTS.lock().await;
    let home = tempfile::tempdir().unwrap();
    let state = state(home.path());
    let address = "127.0.0.1:0".parse().unwrap();
    let (polled, observed) = tokio::sync::oneshot::channel();
    let body = Body::from_stream(futures_util::stream::once(async move {
        let _ = polled.send(());
        std::future::pending::<Result<axum::body::Bytes, std::io::Error>>().await
    }));
    let request = |body| {
        Request::builder()
            .method("POST")
            .uri("/api/config-sync/apply?items=hangar_prefs")
            .header("authorization", "Bearer fixture-owner")
            .body(body)
            .unwrap()
    };
    let first = tokio::spawn(handle(
        State(state.clone()),
        ConnectInfo(address),
        request(body),
    ));
    observed.await.unwrap();
    let response = handle(
        State(state.clone()),
        ConnectInfo(address),
        request(Body::empty()),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
    first.abort();
    let _ = first.await;
    let chunk = axum::body::Bytes::from(vec![0; 1024 * 1024]);
    let stream =
        futures_util::stream::iter((0..91).map(move |_| Ok::<_, std::io::Error>(chunk.clone())));
    let response = handle(
        State(state.clone()),
        ConnectInfo(address),
        request(Body::from_stream(stream)),
    )
    .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let mut declared = request(Body::empty());
    declared.headers_mut().insert(
        "content-length",
        (super::super::bundle::MAX_BUNDLE + 1)
            .to_string()
            .parse()
            .unwrap(),
    );
    assert_eq!(
        handle(State(state), ConnectInfo(address), declared)
            .await
            .status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
    assert!(APPLYING.try_lock().is_ok());
}

#[cfg(unix)]
#[test]
fn application_resolves_all_portable_command_programs() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    for name in ["claude", "codex"]
        .into_iter()
        .chain(super::super::paths::PROGRAMS)
    {
        let path = home.path().join(name);
        std::fs::write(&path, b"#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let env = AccountEnvironment::from_map(
        [
            ("HOME".into(), home.path().display().to_string()),
            ("PATH".into(), home.path().display().to_string()),
        ]
        .into(),
    );
    let found = programs(&env);
    for name in ["claude", "codex"]
        .into_iter()
        .chain(super::super::paths::PROGRAMS)
    {
        assert_eq!(
            found.get(name),
            Some(&home.path().join(name).display().to_string())
        );
    }
}
