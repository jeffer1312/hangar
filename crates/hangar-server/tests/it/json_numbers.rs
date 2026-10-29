use hangar_server::orchestration::{body, codec};
use serde_json::Value;

#[test]
fn json_integer_limit_preserves_python_failure_order() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/json-numbers.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let actual = body::parse(case["raw"].as_str().unwrap().as_bytes());
        if let Some(expected) = case["value_json"].as_str() {
            assert_eq!(actual.unwrap_or_else(|e| panic!("{}: {e:?}", case["name"])),
                codec::loads(expected).unwrap(), "{}", case["name"]);
        } else {
            let (status, error) = actual.expect_err(case["name"].as_str().unwrap());
            assert_eq!(u64::from(status.as_u16()), case["status"].as_u64().unwrap(), "{}", case["name"]);
            if status.as_u16() == 422 {
                assert_eq!(error["detail"][0]["loc"][1], case["position"], "{}", case["name"]);
                assert_eq!(error["detail"][0]["ctx"]["error"], case["message"], "{}", case["name"]);
            }
        }
    }
}
