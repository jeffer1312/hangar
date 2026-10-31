use hangar_server::codex_setup::fragments;
use serde_json::json;
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
