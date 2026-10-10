use axum::http::StatusCode;
use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
use serde_json::{Value, json};
use std::sync::Arc;

async fn call(operation: &str, body: Value) -> (StatusCode, Value) {
    call_with_environment(operation, body, None).await
}

async fn call_with_environment(
    operation: &str,
    body: Value,
    environment: Option<hangar_server::accounts::environment::AccountEnvironment>,
) -> (StatusCode, Value) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let mut state = AppState::new(Config {
        listen: address,
        upstream: crate::refused_address(),
        internal_secret: "catalog-secret".into(),
        auth_token: "owner".into(),
        log_path: None,
        trusted: TrustedHosts::parse("127.0.0.1"),
    });
    if let Some(environment) = environment {
        state.accounts = hangar_server::accounts::AccountService::new(environment);
    }
    let router = hangar_server::routes::terminal_router(Arc::new(state));
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let response = reqwest::Client::new()
        .post(format!(
            "http://{address}/__hangar_server/dictation/{operation}"
        ))
        .header("x-hangar-internal", "catalog-secret")
        .header("content-type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.bytes().await.unwrap();
    task.abort();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn dictation_catalog_preserves_model_identity_capabilities_and_hides_unavailable_models() {
    let (status, result) = call("parse_models", json!({"provider":"codex","catalog":{"data":[
        {"model":"modelo-real", "displayName":"Nome visível", "description":"Descrição", "supportedReasoningEfforts":[{"reasoningEffort":"low"},{"reasoningEffort":"high"}],
         "defaultReasoningEffort":"low", "serviceTiers":[{"id":"priority"},{"id":"oculto","hidden":true}], "additionalSpeedTiers":[]},
        {"model":"não-oferecer", "hidden":true}]}})).await;
    assert_eq!(status, 200, "catálogo não foi processado: {result}");
    let models = result["result"]["models"].as_array().unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0]["id"], "modelo-real");
    assert_eq!(models[0]["efforts"], json!(["low", "high"]));
    assert_eq!(models[0]["service_tiers"], json!([{"id":"priority"}]));
}

#[tokio::test]
async fn dictation_catalog_rejects_empty_catalog_instead_of_inventing_model_aliases() {
    let (status, result) = call(
        "parse_models",
        json!({"provider":"codex","catalog":{"data":[]}}),
    )
    .await;
    assert_eq!(status, 502);
    assert_eq!(result["error"]["code"], "dictation_catalog_unavailable");
}

#[tokio::test]
async fn dictation_catalog_validates_effort_and_service_tier_against_the_selected_model() {
    let models = json!([{"id":"modelo-real","efforts":["low"],"service_tiers":[]}]);
    let (status, _) = call(
        "validate_model",
        json!({"models":models,"model":"modelo-real","effort":"high"}),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = call(
        "validate_model",
        json!({"models":models,"model":"modelo-real","service_tier":"priority"}),
    )
    .await;
    assert_eq!(status, 400);
    let (status, result) = call(
        "validate_model",
        json!({"models":models,"model":"modelo-real","effort":"low"}),
    )
    .await;
    assert_eq!(status, 200, "escolha válida recusada: {result}");
}

#[tokio::test]
async fn dictation_catalog_uses_the_requested_account_and_keeps_session_authority_out_of_child() {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_BIN_EXE_hangar-dictation-fixture"));
    for name in ["claude", "codex"] {
        std::fs::copy(
            &fixture,
            bin.join(if cfg!(windows) {
                format!("{name}.exe")
            } else {
                name.into()
            }),
        )
        .unwrap();
    }
    let environment = hangar_server::accounts::environment::AccountEnvironment::from_map(
        [
            ("HOME".into(), root.path().to_string_lossy().into_owned()),
            ("PATH".into(), bin.to_string_lossy().into_owned()),
            ("CP_AUTH_TOKEN".into(), "não-deve-entrar".into()),
            ("TMUX_PANE".into(), "não-deve-entrar".into()),
        ]
        .into(),
    );
    for provider in ["claude", "codex"] {
        for account in ["account-a", "account-b"] {
            let home = root.path().join(account);
            std::fs::create_dir_all(&home).unwrap();
            let (status, value) = call_with_environment(
                "catalog_models",
                json!({"provider":provider,"home":home,"fresh":true}),
                Some(environment.clone()),
            )
            .await;
            assert_eq!(status, 200, "catálogo da conta foi recusado: {value}");
            assert_eq!(
                value["result"]["models"][0]["id"],
                format!("model-{account}")
            );
        }
    }
}
