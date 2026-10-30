#[test]
fn instruction_limit_matches_original_toml_and_source_sizes() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../../backend/tests/fixtures/contract/codex-integration/instruction-limits.json"
    ))
    .unwrap();
    let mut differences = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let home = tempfile::tempdir().unwrap();
        if let Some(value) = case["config"].as_str() {
            fs::write(
                home.path().join("config.toml"),
                format!("project_doc_max_bytes = {value}\n"),
            )
            .unwrap();
        }
        if matches!(
            case["name"].as_str(),
            Some("source-size" | "missing-source")
        ) {
            let records = home.path().join(".hangar-instrucoes");
            fs::create_dir(&records).unwrap();
            let source = home.path().join("source.md");
            if case["name"] == "source-size" {
                fs::write(
                    &source,
                    vec![b'x'; case["source_size"].as_u64().unwrap() as usize],
                )
                .unwrap();
            }
            fs::write(
                records.join("source.json"),
                serde_json::to_vec(&json!({"fonte":source})).unwrap(),
            )
            .unwrap();
        }
        let actual = match limit(home.path()) {
            Ok(value) => json!({"result":crate::orchestration::codec::dumps(&value,false)}),
            Err(error) => {
                json!({"error":match error {Error::Invalid(_)=>"ValueError",_=>"OSError"},"message":error.to_string()})
            }
        };
        let expected = if case.get("error").is_some() {
            json!({"error":case["error"],"message":case["message"]})
        } else {
            json!({"result":case["result"]})
        };
        if actual != expected {
            differences.push((case["name"].clone(), actual, expected));
        }
    }
    assert!(
        differences.is_empty(),
        "limites divergentes: {differences:?}"
    );
}
