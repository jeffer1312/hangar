#[test]
fn unchanged_write_revalidates_the_destination() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    std::fs::write(&path, b"external").unwrap();
    assert!(matches!(super::write(&path,b"old",Some(b"old"),None),Err(super::Error::Changed(_))));
    assert_eq!(std::fs::read(&path).unwrap(),b"external");
    assert!(!super::write(&path,b"external",Some(b"external"),None).unwrap());
}
