use hangar_server::codex_setup::normalize;
use serde_json::{Value, json};
#[test]
fn codex_normalize_matches_python_commands_and_wrappers() {
    let corpus: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/normalize.json"
    ))
    .unwrap();
    let mut failures = Vec::new();
    for (index, case) in corpus["cases"].as_array().unwrap().iter().enumerate() {
        let windows = case["windows"].as_bool().unwrap();
        let actual = normalize::hooks(
            &case["config"],
            "/python com espaço",
            "/wrapper/codex-hook-allow.py",
            windows,
        );
        let security = normalize::security(
            &case["config"],
            "/python com espaço",
            "/wrapper/codex-hook-json.py",
            windows,
        )
        .unwrap();
        let installed = json!(normalize::installed(&actual, windows));
        if actual != case["normalized"]
            || security != case["security"]
            || installed != case["installed"]
        {
            failures.push(index);
        }
    }
    assert!(failures.is_empty(), "cenários divergentes: {failures:?}");
}
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(48))]
    #[test]
    fn codex_normalize_preserves_unrelated_commands(command in ".{0,160}",windows in proptest::bool::ANY) {
        let config=json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":format!("other {command}")}]}]}});
        let actual=normalize::hooks(&config,"/fixture/python","/fixture/wrapper",windows);
        proptest::prop_assert_eq!(actual,config);
    }
}
