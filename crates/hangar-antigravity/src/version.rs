//! Versão mínima do `agy`. Testado em 1.5.13 e 1.5.15; abaixo de 1.5 o protocolo e o transcript são outros.
pub const MIN: (u32, u32, u32) = (1, 5, 13);
/// A partir daqui a barra padrão continua com o `statusLine` do Hangar (`stack_with_default`), medido no 1.5.19; abaixo,
/// a statusline sem saída poderia apagar o rodapé de onde o pane lê modo e modelo.
pub const STATUS_LINE_MIN: (u32, u32, u32) = (1, 5, 19);

/// O primeiro `x.y.z` da saída de `agy --version`.
pub fn parse(text: &str) -> Option<(u32, u32, u32)> {
    text.split_whitespace().find_map(|word| {
        let mut parts = word.trim_start_matches('v').split('.');
        let v = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?, parts.next()?.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok()?);
        Some(v)
    })
}

pub fn supported(text: &str) -> Option<bool> { parse(text).map(|v| v >= MIN) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_version_and_compares_with_the_minimum() {
        assert_eq!(parse("1.5.15\n"), Some((1, 5, 15)));
        assert_eq!(supported("1.5.15"), Some(true));
        assert_eq!(supported("1.5.13"), Some(true));
        assert_eq!(supported("1.5.12"), Some(false));
        assert_eq!(supported("1.4.99"), Some(false));
        assert_eq!(supported("Antigravity CLI 1.6.0"), Some(true));
        assert_eq!(supported("sem versão"), None);
    }
}
