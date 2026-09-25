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

fn messages() -> &'static HashMap<String, serde_json::Value> {
    static MESSAGES: OnceLock<[HashMap<String, serde_json::Value>; 2]> = OnceLock::new();
    &MESSAGES.get_or_init(|| [include_str!("../../messages/pt.json"), include_str!("../../messages/en.json")]
        .map(|source| serde_json::from_str(source).expect("valid bundled translations")))[english() as usize]
}

pub fn tr(key: &str) -> String {
    messages().get(&format!("native_{key}")).and_then(|v| v.as_str()).unwrap_or(key).to_owned()
}

/// Texto do web pela chave dele, com os parâmetros `{nome}` trocados. Só para as tabelas longas de códigos do servidor
/// (erros e etapas do Codex), que o web já traduz: copiá-las como `native_*` seria manter dois dicionários iguais.
pub fn tr_web(key: &str, params: &HashMap<String, String>) -> Option<String> {
    let mut text = messages().get(key)?.as_str()?.to_owned();
    for (name, value) in params { text = text.replace(&format!("{{{name}}}"), value); }
    Some(text)
}
