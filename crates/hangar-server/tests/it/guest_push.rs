//! Contrato HTTP do push original, com persistência isolada e sem envio.
use axum::{
    body::Body,
    extract::ConnectInfo,
    http::Request,
};
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use std::{net::SocketAddr, sync::Arc};
use tower::ServiceExt;

#[tokio::test]
async fn guest_push_projects_dir_selects_original_storage_parent() {
    let home = tempfile::tempdir().unwrap();
    let projects = home.path().join("custom/projects");
    let mut st = AppState::new(Config {
        listen: "127.0.0.1:1".parse().unwrap(),
        upstream: crate::refused_address(),
        internal_secret: "fixture".into(),
        auth_token: "owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse(""),
    });
    st.accounts = AccountService::new(AccountEnvironment::from_pairs([
        ("HOME", home.path().to_str().unwrap()),
        ("USERPROFILE", home.path().to_str().unwrap()),
        ("CP_PROJECTS_DIR", projects.to_str().unwrap()),
    ]));
    st.share = None;
    let legacy = st.accounts.env.claude_base.join(".hangar-push");
    let mut request = Request::builder()
        .method("POST")
        .uri("/api/push/mute")
        .header("authorization", "Bearer owner")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"session":"s","muted":true}"#))
        .unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
    let response = hangar_server::routes::router(Arc::new(st))
        .oneshot(request)
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        projects
            .parent()
            .unwrap()
            .join(".hangar-push/push_prefs.json")
            .is_file(),
        "GUE-082: _file usa o pai de Settings.projects_dir"
    );
    assert!(!legacy.join("push_prefs.json").exists());
}
