use hangar_server::orchestration::codec;
use serde_json::Value;

proptest::proptest! {
    #[test]
    fn json_scrub_accepts_external_utf16_sequences(units in proptest::collection::vec(proptest::num::u16::ANY,0..100)) {
        let escaped=units.iter().map(|unit|format!("\\u{unit:04x}")).collect::<String>();
        let raw=format!("{{\"{escaped}\":[\"{escaped}\"]}}");
        let value=codec::scrub_surrogates(codec::loads(&raw).unwrap());
        let surrogate=codec::contains_surrogate(&value);
        let actual=serde_json::to_string(&value).unwrap();
        let again=codec::scrub_surrogates(codec::clone_value(&value));
        let stable=again==value;
        codec::discard(value);codec::discard(again);
        proptest::prop_assert!(!surrogate);
        proptest::prop_assert!(stable);
        proptest::prop_assert!(serde_json::from_str::<Value>(&actual).is_ok());
    }
}
