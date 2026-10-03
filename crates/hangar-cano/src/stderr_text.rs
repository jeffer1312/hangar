//! Texto de uma linha do stderr do filho (`cano.py:326-341`). O claude escreve UTF-8; scripts do
//! Windows no meio (hangar-engine.CMD, cmd) escrevem na codepage local, e decodificar como UTF-8
//! punha U+FFFD no aviso de problema.

pub fn stderr_text(raw: &[u8]) -> String {
    match std::str::from_utf8(raw) {
        Ok(s) => s.to_owned(),
        Err(_) => local_codepage(raw),
    }
}

/// Locale UTF-8 cai no cp1252: repetir o UTF-8 só trocaria os acentos por U+FFFD.
// ponytail: fora do Windows o locale é UTF-8 na prática; locale Latin-1 no Linux decodificaria
// igual ao cp1252 em tudo menos 0x80-0x9F.
#[cfg(not(windows))]
fn local_codepage(raw: &[u8]) -> String {
    cp1252(raw)
}

#[cfg(windows)]
fn local_codepage(raw: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{GetACP, MultiByteToWideChar};
    // SAFETY: sem argumentos.
    let acp = unsafe { GetACP() };
    if acp == 65001 || acp == 1252 {
        return cp1252(raw);
    }
    let Ok(len) = i32::try_from(raw.len()) else { return cp1252(raw) };
    // SAFETY: ponteiro e tamanho vêm do mesmo slice; a primeira chamada só mede.
    let need = unsafe { MultiByteToWideChar(acp, 0, raw.as_ptr(), len, std::ptr::null_mut(), 0) };
    if need <= 0 {
        return cp1252(raw);
    }
    let mut wide = vec![0u16; need as usize];
    // SAFETY: `wide` tem exatamente `need` posições.
    let got = unsafe { MultiByteToWideChar(acp, 0, raw.as_ptr(), len, wide.as_mut_ptr(), need) };
    if got <= 0 {
        return cp1252(raw);
    }
    String::from_utf16_lossy(&wide[..got as usize])
}

/// cp1252 como o codec do Python com "replace": os cinco bytes sem caractere viram U+FFFD.
pub fn cp1252(raw: &[u8]) -> String {
    const HIGH: [char; 32] = [
        '\u{20AC}', '\u{FFFD}', '\u{201A}', '\u{0192}', '\u{201E}', '\u{2026}', '\u{2020}', '\u{2021}',
        '\u{02C6}', '\u{2030}', '\u{0160}', '\u{2039}', '\u{0152}', '\u{FFFD}', '\u{017D}', '\u{FFFD}',
        '\u{FFFD}', '\u{2018}', '\u{2019}', '\u{201C}', '\u{201D}', '\u{2022}', '\u{2013}', '\u{2014}',
        '\u{02DC}', '\u{2122}', '\u{0161}', '\u{203A}', '\u{0153}', '\u{FFFD}', '\u{017E}', '\u{0178}',
    ];
    raw.iter()
        .map(|&b| match b {
            0x80..=0x9F => HIGH[usize::from(b - 0x80)],
            _ => char::from(b),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Equivalente a `test_stderr_na_codepage_do_windows_nao_vira_caractere_quebrado`
    /// (backend/tests/test_claude_headless_cano.py:295).
    #[test]
    fn stderr_in_windows_codepage_is_not_a_broken_character() {
        assert_eq!(stderr_text("não existe".as_bytes()), "não existe");
        let cp = stderr_text(b"n\xe3o existe"); // "não existe" em cp1252
        assert!(!cp.contains('\u{FFFD}') && cp.starts_with('n') && cp.ends_with("o existe"));
    }

    #[test]
    fn cp1252_matches_the_python_codec() {
        assert_eq!(cp1252(b"n\xe3o \x80 \x81 \x9f"), "não € \u{FFFD} Ÿ");
    }
}
