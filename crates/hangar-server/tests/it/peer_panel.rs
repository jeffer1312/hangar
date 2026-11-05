use hangar_server::machine_peers::panel::projection;
use proptest::strategy::Strategy;
use serde_json::{Value, json};
proptest::proptest! {
    #[test]
    fn panel_fields_never_coerce_non_text_values(value in proptest::prop_oneof![proptest::prelude::any::<bool>().prop_map(Value::Bool),proptest::prelude::any::<i64>().prop_map(|v|json!(v))]){
        proptest::prop_assert_eq!(projection::fields(&json!({"base_url":value,"token":value,"web_url":value})),(String::new(),String::new(),String::new()));
    }
}
