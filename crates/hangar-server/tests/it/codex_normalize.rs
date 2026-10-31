use hangar_server::codex_setup::normalize;
use serde_json::json;
proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config::with_cases(48))]
    #[test]
    fn codex_normalize_preserves_unrelated_commands(command in ".{0,160}",windows in proptest::bool::ANY) {
        let config=json!({"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":format!("other {command}")}]}]}});
        let actual=normalize::hooks(&config,"/fixture/python","/fixture/wrapper",windows);
        proptest::prop_assert_eq!(actual,config);
    }
}
