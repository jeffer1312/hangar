//! Ambiente do filho: a autoridade do servidor não atravessa. Arquivo próprio porque mexe no
//! ambiente do processo de teste, antes de qualquer outra thread.
use hangar_connectors::host::{Host, HostOptions};
use hangar_connectors::manifest::Manifest;
use hangar_connectors::registry::Registry;
use hangar_connectors::work_tracking::{Filter, ListParams};
use serde_json::json;

#[test]
fn connector_does_not_inherit_the_server_secrets() {
    // SAFETY: único teste deste binário, antes de o runtime criar threads.
    unsafe {
        std::env::set_var("CP_AUTH_TOKEN", "dono");
        std::env::set_var("HANGAR_INTERNAL_SECRET", "interno");
        std::env::set_var("HANGAR_CONECTOR_PROVA", "fica");
    }
    let dir = tempfile::tempdir().unwrap();
    let env_file = dir.path().join("ambiente");
    let command = [env!("CARGO_BIN_EXE_hangar-fixture-connector"), "--id", "ambiente", "--env-file", env_file.to_str().unwrap()];
    let value = json!({"manifest_version": 1, "id": "ambiente", "name": "ambiente", "version": "1", "protocol_version": 1,
        "capabilities": {"work_tracking": 1}, "command": command});
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let host = Host::new(Registry::new(dir.path().join("connectors.json")), HostOptions::default());
        host.add(Manifest::parse(value.to_string().as_bytes(), dir.path()).unwrap()).unwrap();
        host.list_tickets("ambiente", ListParams { filter: Filter::default(), limit: 1, cursor: None }).await.unwrap();
        host.shutdown().await;
    });
    let names = std::fs::read_to_string(env_file).unwrap();
    let names: Vec<&str> = names.lines().collect();
    assert!(names.contains(&"HANGAR_CONECTOR_PROVA"));
    assert!(!names.contains(&"CP_AUTH_TOKEN"));
    assert!(!names.contains(&"HANGAR_INTERNAL_SECRET"));
}
