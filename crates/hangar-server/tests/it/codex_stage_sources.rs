use hangar_server::codex_setup::stage_sources;
use serde_json::json;
proptest::proptest! {
 #![proptest_config(proptest::test_runner::Config::with_cases(64))]
 #[test]
 fn codex_service_stage_hooks_are_idempotent_for_external_commands(command in ".{0,1024}"){
  let input=json!({"Event":[{"private":true,"hooks":[{"type":"command","command":command},{"type":"prompt","prompt":"preservado"}]}]});
  let once=stage_sources::without_app_hooks(&input);let twice=stage_sources::without_app_hooks(&once);
  proptest::prop_assert_eq!(&once,&twice);
  proptest::prop_assert_eq!(&once["Event"][0]["private"],&json!(true));
  proptest::prop_assert!(once["Event"][0]["hooks"].as_array().unwrap().iter().any(|hook|hook["type"]=="prompt"));
 }
}
