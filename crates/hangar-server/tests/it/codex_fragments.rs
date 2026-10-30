use hangar_server::codex_setup::fragments;
use serde_json::{Value,json};
#[test]
fn codex_service_fragments_match_original_provenance_and_python_equality(){
 let corpus:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/fragments.json")).unwrap();
 let mut failures=Vec::new();let mut count=0;
 for case in corpus["cases"].as_array().unwrap(){
  let trusted=case["trusted"].as_array().unwrap().iter().map(|v|v.as_str().unwrap().to_owned()).collect();
  let result=fragments::merge(&case["current"],&case["source"],&case["previous"],&trusted);
  let actual=match result{Ok(value)=>json!({"ok":value}),Err(message)=>json!({"error":"ValueError","message":message})};
  if actual!=case["result"]{count+=1;if failures.len()<10{failures.push(json!({"case":case,"actual":actual}));}}
 }
 assert_eq!(count,0,"{failures:#?}");
}

proptest::proptest! {
 #![proptest_config(proptest::test_runner::Config::with_cases(48))]
 #[test]
 fn codex_service_fragments_preserve_unmanaged_names_and_local_fields(name in "[a-z]{1,24}", text in ".{0,64}", number in proptest::num::u64::ANY) {
  proptest::prop_assume!(name!="managed");
  let current=json!({name.clone():text.clone(),"managed":{"local":number}});
  let source=json!({"managed":{"requested":true}});
  let merged=fragments::merge(&current,&source,&json!({}),&std::collections::BTreeSet::from(["managed".into()])).unwrap();
  proptest::prop_assert_eq!(&merged["merged"][&name],&json!(text));
  proptest::prop_assert_eq!(&merged["merged"]["managed"],&json!({"local":number,"requested":true}));
  proptest::prop_assert_eq!(&merged["manifest"],&source);
 }
}
