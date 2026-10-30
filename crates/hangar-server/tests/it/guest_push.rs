//! Contrato HTTP do push original, com persistência isolada e sem envio.
use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::Request,
};
use hangar_server::{
    accounts::{AccountService, environment::AccountEnvironment},
    auth::TrustedHosts,
    config::Config,
    routes::AppState,
};
use serde_json::Value;
use std::{net::SocketAddr, sync::Arc};
use tower::ServiceExt;

#[tokio::test]
async fn guest_push_http_matches_original_responses_and_files() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/guests/push-http.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for (index, case) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let mut expected = case.clone();
        // D67 conserva status e efeitos; só a descrição interna do codec ganha código estável.
        if case["status"] == 400
            && case["route"] == "subscribe"
            && case["body"]
                .as_str()
                .unwrap()
                .contains("surrogates not allowed")
        {
            expected["body"] = serde_json::json!(r#"{"detail":"push_encoding_invalid"}"#);
            expected["headers"]["content-length"] =
                serde_json::json!(expected["body"].as_str().unwrap().len().to_string());
        }
        let malformed = case["seed"]["push_prefs.json"]
            .as_str()
            .and_then(|text| serde_json::from_str::<Value>(text).ok())
            .and_then(|value| value.get("muted").cloned())
            .is_some_and(|value| {
                !value
                    .as_array()
                    .is_some_and(|values| values.iter().all(Value::is_string))
            });
        if malformed && matches!(case["route"].as_str(), Some("mute" | "quiet-hours")) {
            // D66 recusa antes de tocar o arquivo, inclusive quando o original normalizava.
            expected["status"] = serde_json::json!(503);
            expected["body"] = serde_json::json!(r#"{"detail":{"code":"push_settings_invalid"}}"#);
            expected["headers"] = fixture["rows"][32]["headers"].clone();
            expected["headers"]["content-length"] =
                serde_json::json!(expected["body"].as_str().unwrap().len().to_string());
            let text = case["seed"]["push_prefs.json"].as_str().unwrap();
            expected["files"] = serde_json::json!({"push_prefs.json":text.as_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>()});
        }
        let home = tempfile::tempdir().unwrap();
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
            ("CP_VAPID_PUBLIC", "fixture-public-key"),
        ]));
        st.share = None;
        let dir = st.accounts.env.claude_base.join(".hangar-push");
        if let Some(seed) = case["seed"].as_object() {
            std::fs::create_dir_all(&dir).unwrap();
            for (file, raw) in seed {
                std::fs::write(dir.join(file), raw.as_str().unwrap()).unwrap();
            }
        }
        let raw = case["body_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect::<Vec<_>>();
        let mut builder = Request::builder()
            .method(case["method"].as_str().unwrap())
            .uri(format!("/api/push/{}", case["route"].as_str().unwrap()))
            .header("host", "testserver")
            .header("origin", "https://fixture.invalid")
            .header(
                "authorization",
                format!("Bearer {}", case["token"].as_str().unwrap()),
            );
        if let Some(media) = case["media"].as_str() {
            builder = builder.header("content-type", media);
        }
        let mut request = builder.body(Body::from(raw)).unwrap();
        request
            .extensions_mut()
            .insert(ConnectInfo("192.0.2.1:1234".parse::<SocketAddr>().unwrap()));
        let response = hangar_server::routes::router(Arc::new(st))
            .oneshot(request)
            .await
            .unwrap();
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .map(|(key, value)| {
                (
                    key.as_str().to_owned(),
                    Value::String(value.to_str().unwrap().to_owned()),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        let body = to_bytes(response.into_body(), 1048576).await.unwrap();
        let files = std::fs::read_dir(&dir)
            .into_iter()
            .flatten()
            .map(|entry| {
                let entry = entry.unwrap();
                let hex = std::fs::read(entry.path())
                    .unwrap()
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>();
                (
                    entry.file_name().to_str().unwrap().to_owned(),
                    Value::String(hex),
                )
            })
            .collect::<serde_json::Map<_, _>>();
        if status != expected["status"].as_u64().unwrap() as u16
            || body.as_ref() != expected["body"].as_str().unwrap().as_bytes()
            || Value::Object(headers.clone()) != expected["headers"]
            || Value::Object(files.clone()) != expected["files"]
        {
            failures.push(serde_json::json!({"case":index,"status":status,"body":String::from_utf8_lossy(&body),"headers":headers,"files":files}));
        }
    }
    assert!(
        failures.is_empty(),
        "GUE-082: {} divergências HTTP push: {}",
        failures.len(),
        serde_json::to_string(&failures).unwrap()
    );
}

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
