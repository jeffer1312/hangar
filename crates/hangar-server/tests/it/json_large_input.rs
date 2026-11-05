use hangar_server::orchestration::{body, codec};
use proptest::prelude::*;

#[test]
fn json_deep_input_does_not_abort_the_process() {
    for depth in [900, 1500, 10000] {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "json_large_input::json_deep_input_child", "--ignored"])
            .env("HANGAR_JSON_TEST_DEPTH", depth.to_string())
            .output().unwrap();
        assert!(output.status.success(), "profundidade {depth}: {}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
#[ignore = "auxiliar de pilha executado em subprocesso"]
fn json_deep_input_child() {
    let depth: usize = std::env::var("HANGAR_JSON_TEST_DEPTH").unwrap().parse().unwrap();
    for (open, close) in [("[", "]"), ("{\"x\":", "}")] {
        let input = format!("{}0{}", open.repeat(depth), close.repeat(depth));
        let value = body::parse(input.as_bytes()).unwrap();
        assert!(!codec::contains_surrogate(&value));
        assert!(!codec::contains_nonfinite(&value));
        let copy = codec::clone_value(&value);
        codec::discard(codec::normalize_response(copy));
        codec::discard(value);
    }
}

#[test]
fn json_deep_typed_http_error_never_aborts() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "json_large_input::json_deep_typed_http_child", "--ignored"])
        .output().unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
}

#[tokio::test]
#[ignore = "erro HTTP profundo executado em subprocesso"]
async fn json_deep_typed_http_child() {
    use hangar_server::{accounts::{AccountService, environment::AccountEnvironment},
        auth::TrustedHosts, config::Config, routes::AppState};
    let home = tempfile::tempdir().unwrap();
    let mut state = AppState::new(Config {
        listen: "127.0.0.1:0".parse().unwrap(), upstream: crate::refused_address(),
        internal_secret: "fixture-internal".into(), auth_token: "fixture-owner".into(),
        log_path: None, trusted: TrustedHosts::parse("127.0.0.1"),
    });
    state.accounts = AccountService::new(AccountEnvironment::from_pairs([
        ("HOME", home.path().to_str().unwrap()), ("USERPROFILE", home.path().to_str().unwrap()),
    ]));
    let nested = format!("{}0{}", "[".repeat(10000), "]".repeat(10000));
    let input = format!("{{\"mtime\":0,\"extra\":{nested}}}");
    let request = axum::http::Request::builder().method("POST")
        .uri("/api/sessions/fixture/orq/comecar").header("content-type", "application/json")
        .body(axum::body::Body::from(input)).unwrap();
    let response = hangar_server::orchestration::serve(&std::sync::Arc::new(state), request).await;
    assert_eq!(response.status().as_u16(), 422);
    let bytes = axum::body::to_bytes(response.into_body(), 100000).await.unwrap();
    let error = codec::loads(std::str::from_utf8(&bytes).unwrap()).unwrap();
    let kind = error["detail"][0]["type"].as_str().unwrap().to_owned();
    let returned = codec::dumps(&error["detail"][0]["input"], false);
    codec::discard(error);
    assert_eq!(kind, "extra_forbidden");
    assert_eq!(returned, nested);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn json_large_integer_diagnostics_never_panic(
        prefix in ".{0,80}", suffix in ".{0,80}", digits in 4300usize..4400,
    ) {
        let input = format!("{prefix}{}{suffix}", "9".repeat(digits));
        let _ = body::parse(input.as_bytes());
    }
}
