//! Nomes dos arquivos dos sidecars: a sobra de `tmp+rename` e a chave de cada família.

/// Sobra de `tmp+rename`: `.tmp`, `.tmp<pid>` e `.tmp.<pid>` no FIM do nome (a regra `\.tmp\.?\d*$`).
/// A âncora no fim é o que impede pegar um `<chave>.json` ou `<nome>.jsonl` de verdade.
pub fn is_tmp(name: &str) -> bool {
    let rest = name.trim_end_matches(|c: char| c.is_ascii_digit());
    rest.strip_suffix('.').unwrap_or(rest).ends_with(".tmp")
}

/// A chave de `<chave><sufixo>`, como o glob `*<sufixo>` e o `Path.stem` do Python; `None` = fora da
/// família. No Windows o glob não distingue caixa, e a chave conserva a do nome.
pub fn file_stem<'a>(name: &'a str, suffix: &str) -> Option<&'a str> {
    let cut = name.len().checked_sub(suffix.len())?;
    let (stem, tail) = (name.get(..cut)?, name.get(cut..)?);
    let matches = if cfg!(windows) { tail.eq_ignore_ascii_case(suffix) } else { tail == suffix };
    // `.json` sozinho é arquivo oculto sem sufixo: a chave é o nome inteiro.
    matches.then_some(if stem.is_empty() { name } else { stem })
}
