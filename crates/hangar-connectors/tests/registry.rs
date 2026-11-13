//! Registro persistente: catálogo por servidor, sem iniciar processo.
use hangar_connectors::manifest::Manifest;
use hangar_connectors::registry::Registry;
use serde_json::json;
use std::path::Path;

fn manifest(dir: &Path, id: &str, command: serde_json::Value) -> Manifest {
    let value = json!({
        "manifest_version": 1, "id": id, "name": format!("Conector {id}"), "version": "0.1.0",
        "protocol_version": 1, "capabilities": {"work_tracking": 1}, "command": command
    });
    Manifest::parse(value.to_string().as_bytes(), dir).unwrap()
}

#[test]
fn add_persists_and_another_instance_reads_the_same_catalog() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connectors.json");
    let registry = Registry::new(path.clone());
    registry.add(manifest(dir.path(), "help-test", json!(["./bin/help"]))).unwrap();
    registry.add(manifest(dir.path(), "linear-demo", json!(["node", "./linear.mjs"]))).unwrap();

    let again = Registry::new(path.clone());
    let entries = again.list().unwrap();
    let ids: Vec<_> = entries.iter().map(|entry| entry.id.as_str()).collect();
    assert_eq!(ids, ["help-test", "linear-demo"]);
    assert!(entries.iter().all(|entry| entry.enabled));
    assert_eq!(again.get("linear-demo").unwrap().command, ["node", "./linear.mjs"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn same_manifest_again_is_idempotent_but_a_different_one_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Registry::new(dir.path().join("connectors.json"));
    registry.add(manifest(dir.path(), "help-test", json!(["./bin/help"]))).unwrap();
    registry.set_enabled("help-test", false).unwrap();
    let again = registry.add(manifest(dir.path(), "help-test", json!(["./bin/help"]))).unwrap();
    assert!(!again.enabled, "recadastrar o mesmo manifesto não religa");
    let error = registry.add(manifest(dir.path(), "help-test", json!(["./bin/outro"]))).unwrap_err();
    assert_eq!(error.code(), "connector_exists");
    assert_eq!(registry.get("help-test").unwrap().command[0], dir.path().join("./bin/help").to_string_lossy());
}

#[test]
fn enable_disable_and_remove_change_only_the_named_entry() {
    let dir = tempfile::tempdir().unwrap();
    let registry = Registry::new(dir.path().join("connectors.json"));
    registry.add(manifest(dir.path(), "a-one", json!(["prog"]))).unwrap();
    registry.add(manifest(dir.path(), "b-two", json!(["prog"]))).unwrap();
    assert!(!registry.set_enabled("a-one", false).unwrap().enabled);
    assert!(registry.get("b-two").unwrap().enabled);
    assert_eq!(registry.remove("a-one").unwrap().id, "a-one");
    assert_eq!(registry.list().unwrap().len(), 1);
    assert_eq!(registry.remove("a-one").unwrap_err().code(), "connector_not_found");
    assert_eq!(registry.set_enabled("nada", true).unwrap_err().code(), "connector_not_found");
}

#[test]
fn unknown_registry_version_is_an_error_and_the_file_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("connectors.json");
    std::fs::write(&path, r#"{"version":2,"connectors":[]}"#).unwrap();
    let registry = Registry::new(path.clone());
    assert_eq!(registry.list().unwrap_err().code(), "registry_invalid");
    assert_eq!(registry.add(manifest(dir.path(), "a-one", json!(["prog"]))).unwrap_err().code(), "registry_invalid");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), r#"{"version":2,"connectors":[]}"#);
}

#[cfg(unix)]
#[test]
fn catalog_operations_never_start_the_connector() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("iniciou");
    let registry = Registry::new(dir.path().join("connectors.json"));
    registry.add(manifest(dir.path(), "marca", json!(["/bin/sh", "-c", format!("touch '{}'", marker.display())]))).unwrap();
    registry.list().unwrap();
    registry.set_enabled("marca", false).unwrap();
    registry.set_enabled("marca", true).unwrap();
    registry.remove("marca").unwrap();
    assert!(!marker.exists());
}
