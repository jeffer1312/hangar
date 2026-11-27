//! A linha que abre o primeiro prompt da filha ligada. A marca nas superfícies do pai é conferida no
//! `hangar-server` (`tests/it/delegation_guidance.rs`): elas moram em crates acima deste, e só lá a
//! seleção do CI por dependentes alcança a mudança delas.
use hangar_delegation::guidance::{child_prefix, CHILD};

#[test]
fn guidance_child_prefix_is_one_line_naming_the_parent() {
    let line = child_prefix("hangar-pai");
    assert!(line.contains("'hangar-pai'"), "{line}");
    assert!(line.ends_with(CHILD), "{line}");
    assert!(!line.contains('\n'), "{line}");
}
