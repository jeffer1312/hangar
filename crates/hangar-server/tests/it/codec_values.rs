//! Tipos que o JSON do Python aceita e o transporte HTTP serializa de formas distintas.
use hangar_server::orchestration::codec;
use proptest::prelude::*;

proptest! {
    #[test]
    fn codec_values_external_strings_remain_strings(text in ".{0,2000}") {
        let value=codec::loads(&serde_json::to_string(&text).unwrap()).unwrap();
        prop_assert_eq!(codec::normalize_response(value.clone()),value.clone());
        let _=codec::contains_surrogate(&value);
        let _=codec::contains_nonfinite(&value);
    }
}
