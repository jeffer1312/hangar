use super::*;

#[test]
fn preparation_matches_the_executed_python_reference() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/speech/tts_python.json"
    ))
    .unwrap();
    for case in fixture["preparation"].as_array().unwrap() {
        assert_eq!(
            prepare(case["input"].as_str().unwrap()),
            case["output"].as_str().unwrap(),
            "{case}"
        );
    }
}

#[tokio::test]
async fn an_unknown_provider_is_rejected_before_touching_the_cache() {
    let config = SpeechConfig {
        values: serde_json::json!({}),
        home: PathBuf::new(),
        cache_dir: PathBuf::from("must-not-exist"),
    };
    assert_eq!(
        Service::default()
            .synthesize(&config, "oi", "", "other", "")
            .await
            .unwrap_err()
            .status,
        400
    );
}
