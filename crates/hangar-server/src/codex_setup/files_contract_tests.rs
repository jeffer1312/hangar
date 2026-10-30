use super::*;

fn unhex(text: &str) -> Vec<u8> {
    text.as_bytes().chunks_exact(2).map(|bytes| u8::from_str_radix(std::str::from_utf8(bytes).unwrap(),16).unwrap()).collect()
}

#[test]
fn file_objects_preserve_original_json_encodings_numbers_and_write_errors() {
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/file-objects.json")).unwrap();
    let mut differences = Vec::new();
    for case in fixture["cases"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("object.json");
        if let Some(raw) = case["raw"].as_str() { fs::write(&path,unhex(raw)).unwrap(); }
        let result = object(&path);
        let actual = match result {
            Ok(value) => {
                let serialized = crate::orchestration::codec::dumps(&value,false);
                let output = bytes(&value);
                crate::orchestration::codec::discard(value);
                match output {
                    Ok(output) => json!({"value":serialized,"output":output.iter().map(|b|format!("{b:02x}")).collect::<String>()}),
                    Err(_) => json!({"value":serialized,"write_error":true}),
                }
            },
            Err(_) => json!({"read_error":true}),
        };
        let expected = if case.get("read_error").is_some() { json!({"read_error":true}) }
            else if case.get("write_error").is_some() { json!({"value":case["value"],"write_error":true}) }
            else { json!({"value":case["value"],"output":case["output"]}) };
        if actual != expected { differences.push((case["name"].clone(),actual,expected)); }
    }
    assert!(differences.is_empty(),"arquivos JSON divergentes: {differences:?}");
}

#[cfg(unix)]
#[test]
fn backup_preserves_existing_directory_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let fixture: Value = serde_json::from_str(include_str!("../../../../backend/tests/fixtures/contract/codex-integration/file-objects.json")).unwrap();
    for case in fixture["modes"].as_array().unwrap() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("backups");
        if case["existing"].as_bool().unwrap() {
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root,fs::Permissions::from_mode(0o755)).unwrap();
        }
        backup(&directory.path().join("source"),b"original",&root).unwrap();
        let file = fs::read_dir(&root).unwrap().next().unwrap().unwrap().path();
        assert_eq!(fs::metadata(&root).unwrap().permissions().mode() & 0o777,case["directory_mode"].as_u64().unwrap() as u32,"{case}");
        assert_eq!(fs::metadata(file).unwrap().permissions().mode() & 0o777,case["file_mode"].as_u64().unwrap() as u32);
    }
}
