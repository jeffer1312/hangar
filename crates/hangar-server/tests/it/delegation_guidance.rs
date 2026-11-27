//! Toda superfície em que o pai decide abrir uma filha ligada leva o texto de quando delegar, e o
//! `mcp/mod.rs` conhece os nomes das tools que esse texto cita. Mora aqui, não no `hangar-delegation`:
//! o CI escolhe os testes pelos crates mudados e por quem depende deles, e só o servidor alcança as três
//! superfícies.
use hangar_delegation::guidance::MARKER;

const CATALOG: &str = include_str!("../../src/mcp/catalog.json");
const MCP: &str = include_str!("../../src/mcp/mod.rs");
const HELP: &str = include_str!("../../../hangar-send/src/help.txt");
// Caminho a partir da raiz: é por ele que o `scripts/gate-areas.py` acha quem lê o script.
const INSTALL: &str = include_str!("../../../../scripts/install-hangar-send.sh");

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
        assert!(MCP.contains(tool), "mcp/mod.rs não cita a tool {tool}");
    }
}
