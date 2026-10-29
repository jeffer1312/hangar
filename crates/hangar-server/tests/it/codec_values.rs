//! Tipos que o JSON do Python aceita e o transporte HTTP serializa de formas distintas.
use hangar_server::orchestration::{body,codec};
use serde_json::Value;
use proptest::prelude::*;

#[test]
fn codec_values_surrogates_match_python_parser() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/request-body.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap().iter().filter(|r|r["parser"]["accepted"]==true) {
        let hex=row["raw_hex"].as_str().unwrap();
        let bytes:Vec<_>=(0..hex.len()).step_by(2).map(|i|u8::from_str_radix(&hex[i..i+2],16).unwrap()).collect();
        let value=body::parse(&bytes).unwrap();
        assert_eq!(codec::contains_surrogate(&value),row["parser"]["contains_surrogate"].as_bool().unwrap());
    }
}

#[test]
fn codec_values_distinguish_typed_response_from_jsonresponse() {
    let fixture=codec::loads(include_str!("../../../../backend/tests/fixtures/contract/guests/vault-unicode.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let blob=&row["disk_blob"];
        let strict_failure=codec::contains_surrogate(blob)||codec::contains_nonfinite(blob);
        assert_eq!(strict_failure,row["stale"]["status"]==500);
        let normalized=codec::normalize_response(blob.clone());
        if row["get"]["status"]==200 { assert_eq!(normalized,row["get"]["response"]["enc_blob"]); }
        else { assert!(codec::contains_surrogate(&normalized)); }
        assert!(!codec::contains_nonfinite(&normalized));
    }
}

proptest! {
    #[test]
    fn codec_values_external_strings_remain_strings(text in ".{0,2000}") {
        let value=codec::loads(&serde_json::to_string(&text).unwrap()).unwrap();
        prop_assert_eq!(codec::normalize_response(value.clone()),value.clone());
        let _=codec::contains_surrogate(&value);
        let _=codec::contains_nonfinite(&value);
    }
}
