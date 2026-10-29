use hangar_server::orchestration::{body, codec};
use proptest::prelude::*;

#[test]
fn json_duplicate_keys_discard_previous_values_like_python() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/json-numbers.json"
    )).unwrap();
    for row in fixture["cases"].as_array().unwrap().iter().filter(|row|row["name"].as_str().unwrap().starts_with("duplicate_")) {
        let expected = codec::loads(row["value_json"].as_str().unwrap()).unwrap();
        assert_eq!(hangar_server::transcript::pyjson::loads_lossless(row["raw"].as_str().unwrap()), Some(expected), "{}", row["name"]);
    }
}

#[test]
fn json_deep_errors_keep_python_diagnostics_without_aborting() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/json-deep-errors.json"
    )).unwrap();
    for (index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "json_large_input::json_deep_error_child", "--ignored"])
            .env("HANGAR_JSON_ERROR_CASE", index.to_string()).output().unwrap();
        assert!(output.status.success(), "{} níveis {}: {}", row["depth"], row["name"], String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
#[ignore = "diagnóstico profundo executado em subprocesso"]
fn json_deep_error_child() {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/json-deep-errors.json"
    )).unwrap();
    let index: usize = std::env::var("HANGAR_JSON_ERROR_CASE").unwrap().parse().unwrap();
    let row = &fixture["rows"][index];
    let (status, error) = body::parse(row["raw"].as_str().unwrap().as_bytes()).unwrap_err();
    assert_eq!(u64::from(status.as_u16()), row["status"].as_u64().unwrap());
    if status.as_u16() == 422 {
        assert_eq!(error["detail"][0]["loc"][1], row["position"]);
        assert_eq!(error["detail"][0]["ctx"]["error"], row["message"]);
    }
}

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
