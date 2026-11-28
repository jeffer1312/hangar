/// `names.sanitize_session_name`: NFKD e descarte do que não é ASCII (a letra acentuada vira a base),
/// o resto fora de `[A-Za-z0-9_-]` vira `-`. Nome já ASCII sai idêntico: sidecars e filas são por ele.
pub fn sanitize_session_name(name: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let clean = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
    if name.bytes().all(clean) && !name.starts_with('-') && !name.ends_with('-') { return name.to_owned(); }
    let ascii: String = name.nfkd().filter(char::is_ascii).collect();
    ascii.trim().chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '-' }).collect::<String>().trim_matches('-').to_owned()
}
