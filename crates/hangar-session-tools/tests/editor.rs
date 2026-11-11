use hangar_session_tools::editor;

#[test]
fn editor_configuration_is_live_and_overrides_the_environment() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("runtime-config.json");
    assert_eq!(editor::configured(&path, "code"), "code");
    std::fs::write(&path, r#"{"editor":"zed"}"#).unwrap();
    assert_eq!(editor::configured(&path, "code"), "zed");
    std::fs::write(&path, r#"{"editor":"kate"}"#).unwrap();
    assert_eq!(editor::configured(&path, "code"), "kate");
    std::fs::write(&path, [0xff]).unwrap();
    assert_eq!(editor::configured(&path, "code"), "code");
}

#[tokio::test]
async fn missing_editor_reports_the_existing_error_contract() {
    let root = tempfile::tempdir().unwrap();
    let binary = root.path().join("missing-editor");
    let error = editor::open(
        binary.to_str().unwrap(),
        root.path().to_str().unwrap(),
        &Default::default(),
    )
    .await
    .unwrap_err();
    assert_eq!((error.status, error.code), (500, "erro_editor_falhou"));
    assert_eq!(error.params["binario"], binary.to_str().unwrap());
    assert!(error.params["erro"].is_string());
}
