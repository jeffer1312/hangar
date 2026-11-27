//! Toda superfície em que o pai decide abrir uma filha ligada leva o texto de quando delegar, e o
//! catálogo do MCP tem as tools que esse texto cita.
use hangar_delegation::guidance::{child_prefix, CHILD, MARKER};

const CATALOG: &str = include_str!("../../hangar-server/src/mcp/catalog.json");
const HELP: &str = include_str!("../../hangar-send/src/help.txt");
const INSTALL: &str = include_str!("../../../scripts/install-hangar-send.sh");
const MCP: &str = include_str!("../../hangar-server/src/mcp/mod.rs");

fn new_session_description() -> String {
    let tools: serde_json::Value = serde_json::from_str(CATALOG).expect("catalog.json legível");
    let tool = tools.as_array().expect("catálogo é uma lista")
        .iter()
        .find(|t| t["name"] == "new_session")
        .expect("catálogo tem new_session");
    tool["description"].as_str().expect("new_session tem descrição").to_owned()
}

#[test]
fn every_surface_carries_the_delegation_marker() {
    let surfaces = [
        ("descrição do new_session (catalog.json)", new_session_description()),
        ("hangar-send/src/help.txt", HELP.to_owned()),
        ("scripts/install-hangar-send.sh", INSTALL.to_owned()),
    ];
    for (name, text) in &surfaces {
        assert!(text.contains(MARKER), "{name} sem a marca {MARKER:?}");
        assert!(text.contains("child_status"), "{name} não cita child_status");
    }
    for tool in ["child_status", "child_answer"] {
        assert!(MCP.contains(tool), "mcp/mod.rs não conhece a tool {tool}");
    }
}

#[test]
fn child_prefix_is_one_line_naming_the_parent() {
    let line = child_prefix("hangar-pai");
    assert!(line.contains("'hangar-pai'"), "{line}");
    assert!(line.ends_with(CHILD), "{line}");
    assert!(!line.contains('\n'), "{line}");
}
