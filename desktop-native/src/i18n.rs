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
    gpui_kit::component::set_locale(if english { "en" } else { "pt-BR" });
}

pub fn english() -> bool {
    #[cfg(test)]
    if let Some(english) = TEST_ENGLISH.with(std::cell::Cell::get) { return english; }
    ENGLISH.load(Ordering::Relaxed)
}

// O idioma é global e os testes rodam em paralelo: teste que precisa do inglês o liga só na própria thread.
#[cfg(test)]
thread_local! { static TEST_ENGLISH: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) }; }

#[cfg(test)]
pub fn english_in_this_thread(english: bool) { TEST_ENGLISH.with(|c| c.set(Some(english))); }

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

/// Texto que o web também mostra, pela chave dele: uma frase, um dicionário.
pub fn tr_shared(key: &str, params: &[(&str, &str)]) -> String {
    let params = params.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect();
    tr_web(key, &params).unwrap_or_else(|| key.to_owned())
}

#[cfg(test)]
mod tests {
    /// `tr` procura `native_<chave>`; chave sem tradução aparece crua na tela, sem erro nenhum.
    #[test]
    fn every_literal_tr_key_has_a_native_translation() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let dicts = [include_str!("../../messages/pt.json"), include_str!("../../messages/en.json")]
            .map(|s| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(s).unwrap());
        let mut stack = vec![src];
        let mut missing = Vec::new();
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() { stack.push(path); continue; }
                if path.extension().and_then(|e| e.to_str()) != Some("rs") { continue; }
                let text = std::fs::read_to_string(&path).unwrap();
                for (at, _) in text.match_indices("tr(\"") {
                    // Só a função `tr`: `tr_shared` e `tr_web` também terminam assim.
                    if text[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') { continue; }
                    let key: String = text[at + 4..].chars().take_while(|&c| c != '"').collect();
                    let native = format!("native_{key}");
                    if dicts.iter().any(|d| !d.contains_key(&native)) { missing.push(key); }
                }
            }
        }
        missing.sort();
        missing.dedup();
        assert!(missing.is_empty(), "tr() sem native_* em pt.json/en.json: {missing:?}");
    }
}
