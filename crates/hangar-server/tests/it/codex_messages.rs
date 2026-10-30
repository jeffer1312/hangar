use hangar_server::codex_setup::messages;
use serde_json::Value;
#[test]
fn codex_service_messages_match_original_catalog_and_parameter_rendering(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/messages.json")).unwrap();
 for case in corpus["cases"].as_array().unwrap(){
  assert_eq!(messages::message(case["code"].as_str().unwrap(),case["params"].as_object().unwrap()).unwrap(),case["expected"],"{case}");
 }
}
