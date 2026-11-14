//! Endereço de um modelo compatível com a API da OpenAI, como a página Jev o grava.

/// O `sem_chat_completions` do Python: tira o `/chat/completions` e a barra final do caminho. A query
/// pertence ao endpoint, depois do caminho da operação, e fica.
pub fn base_url(value: &str) -> String {
    let split = value.find(['?', '#']).unwrap_or(value.len());
    let path = value[..split].trim_end_matches('/');
    format!("{}{}", path.strip_suffix("/chat/completions").unwrap_or(path), &value[split..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_keeps_the_query() {
        assert_eq!(base_url("http://x/v1/chat/completions"), "http://x/v1");
        assert_eq!(base_url("https://a.b/v1/chat/completions/?x=1"), "https://a.b/v1?x=1");
        assert_eq!(base_url("http://x/v1/"), "http://x/v1");
    }
}
