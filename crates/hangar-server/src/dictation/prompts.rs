use serde::Deserialize;
use std::collections::{BTreeMap, HashSet};
use std::sync::LazyLock;
use super::model::DictationStyle;

#[derive(Deserialize)]
pub(crate) struct Spec {
    pub rules: String, pub styles: BTreeMap<String, String>,
    pub contractions: BTreeMap<String, String>, pub noise: HashSet<String>,
    pub verb_suffixes: Vec<String>, pub derivation_suffixes: Vec<String>,
}
pub(crate) static SPEC: LazyLock<Spec> = LazyLock::new(|| serde_json::from_str(include_str!("../../../../resources/dictation-organization.json")).expect("recurso de organização válido"));
pub fn prompt(style: DictationStyle) -> String { format!("{}{}", SPEC.rules, SPEC.styles[style.id()]) }
