//! Um atalho importado com segredo pendente não pode ser executado.
use std::sync::LazyLock;

pub fn has_placeholder(text: &str) -> Option<String> {
    static PLACEHOLDER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"⟦SEGREDO:([A-Za-z0-9_.-]+)⟧").expect("expressão constante")
    });
    PLACEHOLDER.captures(text).map(|found| found[1].to_owned())
}
