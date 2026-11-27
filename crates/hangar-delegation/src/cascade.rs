//! Quem parar junto quando o pai para: as filhas ligadas e as filhas delas, até `MAX_DEPTH` níveis.

use std::collections::{BTreeMap, HashSet, VecDeque};

/// Netas além disso ficam de fora: filha de filha de filha já é árvore que ninguém abriu de propósito.
pub const MAX_DEPTH: usize = 3;

/// Uma descendente a interromper; `depth` 1 é filha direta.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CascadeTarget {
    pub name: String,
    pub depth: usize,
}

/// `links` é filha → pai. Em largura a partir de `root`, irmãs por nome; quem já apareceu (a raiz
/// inclusive) não volta, então um vínculo em ciclo termina.
pub fn plan(links: &BTreeMap<String, String>, root: &str) -> Vec<CascadeTarget> {
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    // O `BTreeMap` já percorre por nome: cada lista de irmãs sai em ordem.
    for (child, parent) in links { children.entry(parent.as_str()).or_default().push(child.as_str()); }
    let mut visited: HashSet<&str> = HashSet::from([root]);
    let mut queue: VecDeque<(&str, usize)> = VecDeque::from([(root, 0)]);
    let mut found = Vec::new();
    while let Some((node, depth)) = queue.pop_front() {
        if depth == MAX_DEPTH { continue; }
        for &child in children.get(node).map(Vec::as_slice).unwrap_or_default() {
            if !visited.insert(child) { continue; }
            found.push(CascadeTarget { name: child.to_owned(), depth: depth + 1 });
            queue.push_back((child, depth + 1));
        }
    }
    found
}
