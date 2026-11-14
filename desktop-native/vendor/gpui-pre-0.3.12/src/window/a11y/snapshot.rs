// Texto do último quadro de acessibilidade, para quem lê a tela sem leitor de tela (a voz do Hangar).
// Só depende do accesskit: a suíte do app inclui este arquivo (`include!`), porque os testes da lib não compilam
// nesta cópia. Por isso nada de `//!` e todo nome do accesskit vem dos `use` de cima.

use accesskit::{Node, NodeId, Toggled, TreeUpdate};
#[cfg(test)]
use accesskit::{Role, Tree, TreeId};
use std::collections::HashMap;

/// Separa os ids no id de acessibilidade automático: ids do app já levam `/` (caminho de pasta) e `::` (sessão remota).
pub(crate) const ID_SEPARATOR: char = '›';

/// Nodes whose id is `id` or ends with `›id`, the same match as the `root` of [`snapshot_text`].
pub(crate) fn find_by_id<'a>(update: &'a TreeUpdate, id: &str) -> Vec<(NodeId, &'a Node)> {
    update.nodes.iter().filter(|(_, node)| node.author_id().is_some_and(|author| author == id || author.ends_with(&format!("{ID_SEPARATOR}{id}"))))
        .map(|(nid, node)| (*nid, node)).collect()
}

/// One line per node, indented by depth: `role "name" = "value" [states] #id`, the id cut to its last segment.
/// `root` limits it to the subtree of the node whose id is `root` or ends with `›root`; `None` when it is absent.
pub(crate) fn snapshot_text(update: &TreeUpdate, root: Option<&str>) -> Option<String> {
    let nodes: HashMap<NodeId, &Node> = update.nodes.iter().map(|(id, node)| (*id, node)).collect();
    let start = match root {
        None => update.tree.as_ref().map_or(NodeId(0), |tree| tree.root),
        Some(root) => update.nodes.iter().find_map(|(id, node)| {
            let author = node.author_id()?;
            (author == root || author.ends_with(&format!("{ID_SEPARATOR}{root}"))).then_some(*id)
        })?,
    };
    let mut out = String::new();
    let mut stack = vec![(start, 0usize)];
    while let Some((id, depth)) = stack.pop() {
        let Some(node) = nodes.get(&id) else { continue };
        out.push_str(&"  ".repeat(depth));
        out.push_str(&format!("{:?}", node.role()));
        if let Some(label) = node.label() {
            out.push_str(&format!(" {label:?}"));
        }
        if let Some(value) = node.value() {
            out.push_str(&format!(" = {value:?}"));
        }
        let mut states = Vec::new();
        if node.is_selected() == Some(true) {
            states.push("selected");
        }
        match node.toggled() {
            Some(Toggled::True) => states.push("checked"),
            Some(Toggled::Mixed) => states.push("mixed"),
            _ => {}
        }
        match node.is_expanded() {
            Some(true) => states.push("expanded"),
            Some(false) => states.push("collapsed"),
            None => {}
        }
        if node.is_disabled() {
            states.push("disabled");
        }
        if update.focus == id && id != start {
            states.push("focused");
        }
        if !states.is_empty() {
            out.push_str(&format!(" [{}]", states.join(", ")));
        }
        if let Some(short) = node.author_id().and_then(|author| author.rsplit(ID_SEPARATOR).next()) {
            out.push_str(&format!(" #{short}"));
        }
        out.push('\n');
        stack.extend(node.children().iter().rev().map(|child| (*child, depth + 1)));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> TreeUpdate {
        let mut window = Node::new(Role::Window);
        window.set_label("Hangar");
        window.set_children(vec![NodeId(1)]);
        let mut dialog = Node::new(Role::Dialog);
        dialog.set_label("Configurações");
        dialog.set_author_id("root›hangar-root›settings-dialog");
        dialog.set_children(vec![NodeId(2), NodeId(3), NodeId(4)]);
        let mut tab = Node::new(Role::Tab);
        tab.set_label("Avançado");
        tab.set_selected(true);
        tab.set_author_id("root›hangar-root›settings-dialog›settings-tab-advanced");
        let mut text = Node::new(Role::Label);
        text.set_value("Porta do servidor");
        let mut switch = Node::new(Role::Switch);
        switch.set_label("Mostrar raciocínio");
        switch.set_toggled(Toggled::True);
        switch.set_disabled();
        TreeUpdate {
            nodes: vec![(NodeId(0), window), (NodeId(1), dialog), (NodeId(2), tab), (NodeId(3), text), (NodeId(4), switch)],
            tree: Some(Tree::new(NodeId(0))),
            tree_id: TreeId::ROOT,
            focus: NodeId(2),
        }
    }

    #[test]
    fn find_by_id_matches_the_last_segment_only() {
        let tree = tree();
        let ids = |id: &str| find_by_id(&tree, id).into_iter().map(|(nid, _)| nid).collect::<Vec<_>>();
        assert_eq!(ids("settings-tab-advanced"), vec![NodeId(2)]);
        assert_eq!(ids("settings-dialog"), vec![NodeId(1)], "o pai não casa pelo segmento do meio");
        assert!(ids("advanced").is_empty(), "pedaço de segmento não casa");
    }

    #[test]
    fn whole_window_one_line_per_node() {
        assert_eq!(
            snapshot_text(&tree(), None).unwrap(),
            "Window \"Hangar\"\n  Dialog \"Configurações\" #settings-dialog\n    Tab \"Avançado\" [selected, focused] #settings-tab-advanced\n    Label = \"Porta do servidor\"\n    Switch \"Mostrar raciocínio\" [checked, disabled]\n"
        );
    }

    #[test]
    fn root_limits_to_the_subtree_by_short_or_full_id() {
        let short = snapshot_text(&tree(), Some("settings-tab-advanced")).unwrap();
        assert_eq!(short, "Tab \"Avançado\" [selected] #settings-tab-advanced\n");
        assert_eq!(snapshot_text(&tree(), Some("root›hangar-root›settings-dialog")).unwrap().lines().count(), 4);
        assert_eq!(snapshot_text(&tree(), Some("dialog")), None);
        assert_eq!(snapshot_text(&tree(), Some("missing")), None);
    }
}
