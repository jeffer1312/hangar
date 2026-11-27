//! A linha que abre o primeiro prompt da filha ligada. A marca nas superfícies do pai é conferida no
//! `hangar-server` (`tests/it/delegation_guidance.rs`): elas moram em crates acima deste, e só lá a
//! seleção do CI por dependentes alcança a mudança delas.
use hangar_delegation::guidance::{child_prefix, without_child_prefix, CHILD};

#[test]
fn guidance_child_prefix_is_one_line_naming_the_parent() {
    let line = child_prefix("hangar-pai");
    assert!(line.contains("'hangar-pai'"), "{line}");
    assert!(line.ends_with(CHILD), "{line}");
    assert!(!line.contains('\n'), "{line}");
}

#[test]
fn guidance_without_child_prefix_leaves_only_the_request() {
    assert_eq!(without_child_prefix(&format!("{} faz a busca", child_prefix("hangar-pai"))), "faz a busca");
    assert_eq!(without_child_prefix("faz a busca"), "faz a busca");
    // Começo parecido sem a linha inteira não é a linha: o texto fica como veio.
    let lookalike = "Você é filha ligada de 'x': outra coisa";
    assert_eq!(without_child_prefix(lookalike), lookalike);
}
