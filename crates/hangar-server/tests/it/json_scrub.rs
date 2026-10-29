use hangar_server::orchestration::codec;
use serde_json::Value;

#[test]
fn json_scrub_matches_original_python_sidecar_normalization() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/scrub-surrogates.json")).unwrap();
    let mut differences=Vec::new();
    for (index,row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        let value=codec::loads(row["raw"].as_str().unwrap()).unwrap();
        let value=codec::scrub_surrogates(value);
        // Este helper transforma valores; os bytes do serializador têm prova separada.
        let expected=codec::loads(row["expected"].as_str().unwrap()).unwrap();
        if value!=expected {differences.push(index);}
        codec::discard(value);codec::discard(expected);
    }
    assert!(differences.is_empty(),"GUE-083: normalização divergente nas entradas {differences:?}");
}

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
