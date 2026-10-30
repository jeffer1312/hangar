use regex::{Captures, Regex};
use std::sync::LazyLock;

// O `\w` do Python inclui letras, números e sublinhado, mas não marcas combinantes.
static PATH: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[\p{L}\p{N}_.-]+/)+([\p{L}\p{N}_-]+)\.([\p{L}\p{N}_]{1,5})").unwrap()
});
static HASH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[0-9a-f]{7,40}").unwrap());
static WORD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[\p{L}\p{N}_]$").unwrap());
static CLOSED_LINE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"([.!?:;\x{2026}])[ \t]*\n+").unwrap());
static LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n+").unwrap());

fn word(value: char) -> bool {
    if value.is_ascii() {
        value.is_ascii_alphanumeric() || value == '_'
    } else {
        WORD.is_match(value.encode_utf8(&mut [0; 4]))
    }
}

fn boundary(text: &str, at: usize) -> bool {
    text[..at].chars().next_back().is_some_and(word) != text[at..].chars().next().is_some_and(word)
}

fn replace_words(
    text: &str,
    pattern: &Regex,
    replacement: impl Fn(&Captures<'_>) -> Option<String>,
) -> String {
    let mut out = String::with_capacity(text.len());
    let (mut search, mut copied) = (0, 0);
    while let Some(captures) = pattern.captures_at(text, search) {
        let matched = captures.get(0).unwrap();
        if boundary(text, matched.start()) && boundary(text, matched.end()) {
            if let Some(value) = replacement(&captures) {
                out.push_str(&text[copied..matched.start()]);
                out.push_str(&value);
                copied = matched.end();
            }
            search = matched.end();
        } else if !boundary(text, matched.start()) {
            // Uma barra inicial sem limite de palavra pode conter um caminho válido mais adiante.
            search = matched.start() + text[matched.start()..].chars().next().unwrap().len_utf8();
        } else {
            search = matched.end();
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn space(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}

pub(super) fn prepare(text: &str) -> String {
    let text = replace_words(text, &PATH, |m| Some(format!("{} ponto {}", &m[1], &m[2])));
    let text = replace_words(&text, &HASH, |m| {
        let value = &m[0];
        (value.bytes().any(|c| c.is_ascii_digit())
            && value.bytes().any(|c| (b'a'..=b'f').contains(&c)))
        .then(|| " ".to_owned())
    });
    let text = text.replace("..", " ").replace('_', " ");
    let mut filtered = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\u{2190}'..='\u{21ff}'
            | '\u{2b00}'..='\u{2bff}'
            | '\u{2600}'..='\u{27bf}'
            | '\u{fe0f}'
            | '\u{1f000}'..='\u{1faff}'
            | '\u{2022}'
            | '\u{00b7}'
            | '\u{25aa}'
            | '\u{25e6}'
            | '\u{2023}'
            | '\u{25cf}'
            | '\u{25cb}'
            | '\u{25c6}'
            | '\u{25a0}' => filtered.push(' '),
            '\u{2014}' | '\u{2013}' => filtered.push_str(", "),
            _ => filtered.push(c),
        }
    }
    let closed = CLOSED_LINE.replace_all(&filtered, "$1 ");
    let lines = LINE.replace_all(&closed, ". ");
    let mut out = String::with_capacity(lines.len());
    let mut pending_space = false;
    for c in lines.chars() {
        if space(c) {
            pending_space = !out.is_empty();
        } else {
            if pending_space {
                out.push(' ');
                pending_space = false;
            }
            out.push(c);
        }
    }
    out
}
