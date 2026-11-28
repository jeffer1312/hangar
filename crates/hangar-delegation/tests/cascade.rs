//! A árvore que o fechamento do pai interrompe: filhas ligadas em largura, até três níveis, sem laço.
use std::collections::BTreeMap;

use hangar_delegation::cascade::{CascadeTarget, MAX_DEPTH, plan};

fn links(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs.iter().map(|(child, parent)| ((*child).to_owned(), (*parent).to_owned())).collect()
}

fn names(targets: &[CascadeTarget]) -> Vec<(&str, usize)> {
    targets.iter().map(|t| (t.name.as_str(), t.depth)).collect()
}

#[test]
fn cascade_stops_at_the_third_level() {
    let tree = links(&[("a", "pai"), ("b", "a"), ("c", "b"), ("d", "c")]);
    assert_eq!(MAX_DEPTH, 3);
    assert_eq!(names(&plan(&tree, "pai")), vec![("a", 1), ("b", 2), ("c", 3)], "d é o quarto nível");
}

#[test]
fn cascade_with_a_cycle_ends() {
    let cycle = links(&[("a", "b"), ("b", "a")]);
    assert_eq!(names(&plan(&cycle, "a")), vec![("b", 1)], "a raiz não volta como descendente");
}

#[test]
fn cascade_lists_sisters_by_name_level_by_level() {
    let tree = links(&[("z", "pai"), ("a", "pai"), ("m", "pai"), ("a1", "a"), ("outra", "longe")]);
    assert_eq!(names(&plan(&tree, "pai")), vec![("a", 1), ("m", 1), ("z", 1), ("a1", 2)]);
}

#[test]
fn cascade_without_linked_children_is_empty() {
    assert!(plan(&links(&[("a", "outro")]), "pai").is_empty());
}
