use hangar_server::orchestration::codec;
use serde_json::Value;

#[test]
fn json_literal_unicode_preserves_every_character_in_the_legacy_marker_range() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/json-literal-unicode.json")).unwrap();
    let mut failures = Vec::new();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let value = codec::loads(case["input"].as_str().unwrap()).unwrap();
        for (sorted, key) in [(false,"ordinary"),(true,"sorted")] {
            if codec::dumps(&value,sorted) != case[key].as_str().unwrap() {
                failures.push((index,sorted));
            }
        }
        codec::discard(value);
    }
    assert!(failures.is_empty(), "CFG-066: bytes divergentes em {failures:?}");
}
