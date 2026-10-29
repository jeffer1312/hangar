use hangar_server::orchestration::{body, codec};
use serde_json::Value;
use proptest::prelude::*;

proptest! {
    #[test]
    fn arbitrary_json_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let _ = body::parse(&bytes);
    }
}

#[test]
fn json_bytes_match_python_encoding_and_positions() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/json-bytes.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let hex = case["hex"].as_str().unwrap();
        let bytes: Vec<_> = (0..hex.len()).step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect();
        let actual = body::parse(&bytes);
        if let Some(expected) = case["value_json"].as_str() {
            assert_eq!(actual.unwrap_or_else(|e| panic!("{}: {e:?}", case["name"])),
                codec::loads(expected).unwrap(), "{}", case["name"]);
        } else {
            let (status, error) = actual.expect_err("entrada recusada pelo Python");
            assert_eq!(u64::from(status.as_u16()), case["status"].as_u64().unwrap(), "{}", case["name"]);
            if status.as_u16() == 422 {
                assert_eq!(error["detail"][0]["loc"][1], case["position"], "{}", case["name"]);
                assert_eq!(error["detail"][0]["ctx"]["error"], case["message"], "{}", case["name"]);
            }
        }
    }
}

#[test]
fn json_special_scalars_preserve_later_python_diagnostics() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/config_sync/json-diagnostic-special.json"
    )).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let raw = case["raw"].as_str().unwrap();
        let actual = body::parse(raw.as_bytes());
        if let Some(expected) = case["value_json"].as_str() {
            assert_eq!(actual.unwrap(), codec::loads(expected).unwrap(), "{raw}");
        } else {
            let (status, error) = actual.expect_err("entrada recusada pelo Python");
            assert_eq!(status.as_u16(), 422, "{raw}");
            assert_eq!(error["detail"][0]["loc"][1], case["position"], "{raw}");
            assert_eq!(error["detail"][0]["ctx"]["error"], case["message"], "{raw}");
        }
    }
}
