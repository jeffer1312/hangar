use hangar_package::{Manifest, PackageIndex, Version};

const VALID: &str = r#"{
  "version": "0.1.0.8207",
  "commit": "88fbfb17f",
  "channel": "latest",
  "protocol": 7,
  "api": 3,
  "os": "linux-x86_64",
  "dist_key": "abc123",
  "files": {"bin/hangar-server": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"}
}"#;

#[test]
fn valid_manifest_parses() {
    let m = Manifest::parse(VALID.as_bytes()).expect("manifesto válido");
    assert_eq!(m.version, "0.1.0.8207");
    assert_eq!(m.protocol, 7);
    assert_eq!(m.api, 3);
    assert_eq!(m.os, "linux-x86_64");
    assert_eq!(m.dist_key.as_deref(), Some("abc123"));
    assert_eq!(
        m.files.get("bin/hangar-server").map(String::as_str),
        Some("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
    );
}

#[test]
fn missing_field_fails() {
    let without_protocol = VALID.replace("\"protocol\": 7,", "");
    let err = Manifest::parse(without_protocol.as_bytes()).unwrap_err();
    assert_eq!(err.code(), "package_manifest_invalid");
}

#[test]
fn bad_version_or_sha_fails() {
    let bad_version = VALID.replace("0.1.0.8207", "0.1.0");
    assert_eq!(Manifest::parse(bad_version.as_bytes()).unwrap_err().code(), "package_manifest_invalid");
    let bad_sha = VALID.replace("e3b0c442", "E3B0C442");
    assert_eq!(Manifest::parse(bad_sha.as_bytes()).unwrap_err().code(), "package_manifest_invalid");
    let bad_path = VALID.replace("bin/hangar-server", "../hangar-server");
    assert_eq!(Manifest::parse(bad_path.as_bytes()).unwrap_err().code(), "package_manifest_invalid");
}

#[test]
fn version_ordering_is_numeric() {
    let a: Version = "0.1.0.9".parse().unwrap();
    let b: Version = "0.1.0.10".parse().unwrap();
    assert!(a < b);
    assert!("0.1.0".parse::<Version>().is_err());
    assert!("0.1.0.x".parse::<Version>().is_err());
}

#[test]
fn package_index_parses() {
    let json = r#"{"platforms": {"windows-x86_64": {"version": "0.1.0.8207", "asset": "hangar-0.1.0.8207-windows-x86_64.tar.gz",
        "sha256": "00", "protocol": 7, "api": 3, "commit": "88fbfb17f", "dist_key": null}}}"#;
    let idx: PackageIndex = serde_json::from_str(json).unwrap();
    let w = &idx.platforms["windows-x86_64"];
    assert_eq!(w.asset, "hangar-0.1.0.8207-windows-x86_64.tar.gz");
    assert_eq!(w.dist_key, None);
}
