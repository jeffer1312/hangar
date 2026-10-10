use serde::Deserialize;
use std::sync::LazyLock;

#[derive(Deserialize)]
struct Spec { base: String, max_characters: usize }

static SPEC: LazyLock<Spec> = LazyLock::new(|| serde_json::from_str(
    include_str!("../../../../resources/dictation-vocabulary.json"))
    .expect("Vocabulário de ditado incluído no binário"));

pub(crate) fn assemble(extra: &str) -> String {
    let extra = extra.trim();
    let joined = if extra.is_empty() { SPEC.base.clone() } else { format!("{}, {extra}", SPEC.base) };
    let count = joined.chars().count();
    if count > SPEC.max_characters {
        tracing::warn!(code = "dictation_vocabulary_truncated", excess = count - SPEC.max_characters,
            "O vocabulário excede o limite; os termos finais não serão enviados.");
    }
    joined.chars().take(SPEC.max_characters).collect()
}
