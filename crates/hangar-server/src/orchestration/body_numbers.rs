//! A recusa do inteiro ocorre quando o leitor chega nele, depois de qualquer erro anterior.
use std::sync::LazyLock;

pub(super) fn for_limit(text: &str) -> Option<(usize, String)> {
    static NUMBER: LazyLock<regex::Regex> = LazyLock::new(|| {
        regex::Regex::new(r"^-?(?:0|[1-9][0-9]*)(?:\.[0-9]+)?(?:[eE][+-]?[0-9]+)?").unwrap()
    });
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut quoted = false;
    let mut first = None;
    let mut edits = Vec::new();
    while at < bytes.len() {
        if quoted {
            if bytes[at] == b'"' { quoted = false; at += 1; }
            else if bytes[at] == b'\\' {
                if bytes.get(at + 1) == Some(&b'u') && at + 6 <= bytes.len()
                    && let Some(code) = text.get(at + 2..at + 6).and_then(|digits| u32::from_str_radix(digits, 16).ok())
                    && (0xd800..=0xdfff).contains(&code) {
                    edits.push((at + 2, 4, true));
                    at += 6;
                } else {
                    at += 1;
                    if let Some(c) = text.get(at..).and_then(|s| s.chars().next()) { at += c.len_utf8(); }
                }
            } else { at += text[at..].chars().next()?.len_utf8(); }
            continue;
        }
        if bytes[at] == b'"' { quoted = true; at += 1; continue; }
        if let Some(token) = ["-Infinity", "Infinity", "NaN"].into_iter().find(|token| text[at..].starts_with(token)) {
            edits.push((at, token.len(), false));
            at += token.len();
        } else if matches!(bytes[at], b'-' | b'0'..=b'9') && let Some(token) = NUMBER.find(&text[at..]) {
            let raw = token.as_str();
            if !raw.contains(['.', 'e', 'E']) && raw.trim_start_matches('-').len() > 4300 {
                first.get_or_insert(at);
                edits.push((at, raw.len(), false));
            }
            at += raw.len();
        } else { at += text[at..].chars().next()?.len_utf8(); }
    }
    let first = first?;
    let mut normalized = bytes.to_vec();
    for (start, length, surrogate) in edits {
        if surrogate { normalized[start..start + length].copy_from_slice(b"fffd"); }
        else {
            normalized[start] = b'0';
            normalized[start + 1..start + length].fill(b' ');
        }
    }
    // Só trechos ASCII foram substituídos por ASCII de mesmo tamanho.
    Some((first, String::from_utf8(normalized).ok()?))
}
