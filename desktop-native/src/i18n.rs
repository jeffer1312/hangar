use std::{collections::HashMap, sync::{OnceLock, atomic::{AtomicBool, Ordering}}};
use crate::appearance::Language;

/// Inglês na tela agora. Troca ao vivo: `tr` lê a cada chamada, e o desenho seguinte já sai no idioma novo.
static ENGLISH: AtomicBool = AtomicBool::new(false);

/// Aplica a escolha do Geral. Sistema volta a seguir `HANGAR_NATIVE_LANG`/`LANG`.
pub fn set_language(language: Language) {
    let english = match language {
        Language::System => std::env::var("HANGAR_NATIVE_LANG").or_else(|_| std::env::var("LANG")).unwrap_or_default().starts_with("en"),
        Language::Pt => false,
        Language::En => true,
    };
    ENGLISH.store(english, Ordering::Relaxed);
}

pub fn english() -> bool { ENGLISH.load(Ordering::Relaxed) }

pub fn tr(key: &str) -> String {
    static MESSAGES: OnceLock<[HashMap<String, serde_json::Value>; 2]> = OnceLock::new();
    let messages = MESSAGES.get_or_init(|| [include_str!("../../messages/pt.json"), include_str!("../../messages/en.json")]
        .map(|source| serde_json::from_str(source).expect("valid bundled translations")));
    messages[english() as usize].get(&format!("native_{key}")).and_then(|v| v.as_str()).unwrap_or(key).to_owned()
}
