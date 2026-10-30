//! Coerção escalar isolada dos handlers.
use hangar_server::value_coercion::{boolean, integer};
use serde_json::Value;
use proptest::prelude::*;

proptest! {
    #[test]
    fn value_coercion_external_scalars_never_panic(text in ".{0,2000}", number in any::<i64>()) {
        for value in [Value::String(text),Value::Number(number.into()),Value::Null] {
            let _=integer(&value);let _=boolean(&value);
        }
    }
}

proptest::proptest! {
    #[test]
    fn login_guests_path_normalization_is_idempotent(value in ".*", windows in proptest::bool::ANY) {
        let once=hangar_server::value_coercion::path_text(&value,windows);
        proptest::prop_assert_eq!(hangar_server::value_coercion::path_text(&once,windows),once);
    }
}
