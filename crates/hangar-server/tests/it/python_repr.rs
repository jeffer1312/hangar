use hangar_server::orchestration::codec;
use serde_json::json;
proptest::proptest! {
    #[test]
    fn container_repr_does_not_emit_raw_control_characters(items in proptest::collection::vec(proptest::arbitrary::any::<String>(),0..64)) {
        let value=json!(items);let output=codec::python_string(&value);
        proptest::prop_assert!(!output.chars().any(char::is_control));
    }
}
