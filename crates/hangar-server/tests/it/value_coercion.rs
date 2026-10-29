//! Coerção escalar isolada dos handlers, com resultado capturado do Settings Python.
use hangar_server::value_coercion::{boolean, integer};
use serde_json::Value;
use proptest::prelude::*;

#[test]
fn value_coercion_matches_python_settings() {
    let fixture:Value=serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/settings.json")).unwrap();
    for row in fixture["rows"].as_array().unwrap() {
        let input=&row["input"];
        let actual=match row["field"].as_str().unwrap() {
            "sync"=>boolean(input).map(Value::Bool),
            "sync_rate_max"|"sync_rate_window"=>integer(input),
            _=>continue,
        };
        let expected=(row["accepted"]==true).then(||row["value"].clone());
        assert_eq!(actual,expected,"{}: {:?}",row["field"],input);
    }
}

proptest! {
    #[test]
    fn value_coercion_external_scalars_never_panic(text in ".{0,2000}", number in any::<i64>()) {
        for value in [Value::String(text),Value::Number(number.into()),Value::Null] {
            let _=integer(&value);let _=boolean(&value);
        }
    }
}

#[test]
fn value_coercion_matches_python_http_models() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/guests/scalar-models.json")).unwrap();
    let mut differences = Vec::new();
    for (index, row) in fixture["rows"].as_array().unwrap().iter().enumerate() {
        // O limite do decodificador ocorre antes de o modelo receber o valor.
        if row["status"] == 400 { continue; }
        let input = hangar_server::orchestration::codec::loads(row["raw_value"].as_str().unwrap()).unwrap();
        let (actual, field) = if row["kind"] == "integer" {
            (integer(&input), "base_rev")
        } else { (boolean(&input).map(Value::Bool), "sees_owner") };
        let expected = (row["status"] == 200).then(|| row["response"][field].clone());
        if actual != expected { differences.push(index); }
    }
    assert!(differences.is_empty(), "casos divergentes: {differences:?}");
}
