use std::{fs, path::Path};

use hangar_api::status::StatusSnapshot;
use serde_json::{Value, json};

fn fixture() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/status.json");
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

/// A fixture é lida também pelo core e pelo nativo: nenhum campo pode sumir na ida e volta.
#[test]
fn fixture_round_trips_without_losing_fields() {
    let original = fixture();
    let snapshot: StatusSnapshot = serde_json::from_value(original.clone()).unwrap();
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), original);
    assert!(snapshot.server.is_some() && snapshot.leftovers.is_some(), "fixture sem todos os blocos");
}

/// Servidor antigo manda só o mínimo; os blocos sem fonte ficam ausentes, nunca zerados.
#[test]
fn minimal_snapshot_has_every_block_absent() {
    let minimal = json!({"generated_at": "2026-11-17T14:23:05Z", "attention": []});
    let snapshot: StatusSnapshot = serde_json::from_value(minimal.clone()).unwrap();
    assert_eq!(snapshot.generated_at, "2026-11-17T14:23:05Z");
    assert!(snapshot.server.is_none());
    assert!(snapshot.exits.is_none());
    assert!(snapshot.update.is_none());
    assert!(snapshot.machines.is_none());
    assert!(snapshot.sessions.is_none());
    assert!(snapshot.slow.is_none());
    assert!(snapshot.host.is_none());
    assert!(snapshot.outside.is_none());
    assert!(snapshot.leftovers.is_none());
    assert!(snapshot.attention.is_empty());
    assert_eq!(serde_json::to_value(&snapshot).unwrap(), minimal);
}
