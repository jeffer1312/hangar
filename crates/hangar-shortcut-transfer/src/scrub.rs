//! Troca cada valor de segredo por `⟦SEGREDO:<nome>⟧`. Nunca registra o texto nem o valor achado.
use std::sync::LazyLock;

use fancy_regex::{Captures, Regex};

// Um valor concatena trechos entre aspas, escapados e literais, sem espaço entre eles.
const SINGLE: &str = r"'[^']*'";
const FISH_SINGLE: &str = r"'(?:\\.|[^'\\])*'";
const DOUBLE: &str = r#""(?:\\.|[^"\\])*""#;
const BACKTICK: &str = r"`(?:\\.|[^`\\])*`";
const PARENS: &str = r"\((?:[^()\\]|\\.|\([^()]*\))*\)";
const BARE: &str = r#"[^\s"'\\;|&()`]+"#;
// Fora do shell (Python, JS) o valor nu também para em `,`, `]` e `}`.
const LITERAL_BARE: &str = r#"[^\s"'\\;|&()`,\]}]+"#;
const MARK_ANY: &str = r"⟦SEGREDO:([A-Za-z0-9_.-]+)⟧";

const SECRET_WORDS: [&str; 7] = ["secret", "token", "password", "senha", "_key", "apikey", "private"];
const FLAG_NAMES: [(&str, &str); 10] = [
    ("password", "senha"),
    ("passwd", "senha"),
    ("pass", "senha"),
    ("token", "token"),
    ("access-token", "token"),
    ("auth-token", "token"),
    ("api-key", "api_key"),
    ("apikey", "api_key"),
    ("secret", "segredo"),
    ("client-secret", "segredo"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Dialect {
    Bash,
    Fish,
    Literal,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Fixed(&'static str),
    Flag,
    Env,
    FishSet,
}

struct Rule {
    regex: Regex,
    kind: Kind,
}

static RULES: LazyLock<[Vec<Rule>; 3]> =
    LazyLock::new(|| [rules(Dialect::Bash), rules(Dialect::Fish), rules(Dialect::Literal)]);
static FISH_SET: LazyLock<Rule> = LazyLock::new(|| Rule {
    regex: compile(&format!(
        r"\bset\s+(?:-[a-zA-Z]+\s+)*(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s+{}",
        value(FISH_SINGLE, BARE)
    )),
    kind: Kind::FishSet,
});
static TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    compile(concat!(
        r"\b(?:gsk_[A-Za-z0-9]{20,}|sk-(?:proj-|ant-)?[A-Za-z0-9_-]{20,}|gh[pousr]_[A-Za-z0-9]{20,}",
        r"|github_pat_[A-Za-z0-9_]{20,}|glpat-[A-Za-z0-9_-]{20,}|xox[abprs]-[A-Za-z0-9-]{10,}",
        r"|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})"
    ))
});
static PLACEHOLDER: LazyLock<regex::Regex> = LazyLock::new(|| plain(MARK_ANY));
static PLACEHOLDER_FULL: LazyLock<regex::Regex> = LazyLock::new(|| plain(&format!(r"\A{MARK_ANY}\z")));
static QUOTED_BASH: LazyLock<regex::Regex> = LazyLock::new(|| plain(&format!(r"\A(?:{SINGLE}|{DOUBLE})\z")));
static QUOTED_FISH: LazyLock<regex::Regex> = LazyLock::new(|| plain(&format!(r"\A(?:{FISH_SINGLE}|{DOUBLE})\z")));
static VARIABLE: LazyLock<regex::Regex> =
    LazyLock::new(|| plain(r"\A(?:\$[A-Za-z_]\w*|\$\{[A-Za-z_]\w*\})\z"));
static SUBSTITUTION: LazyLock<regex::Regex> = LazyLock::new(|| plain(&format!(r"\A(?:\${PARENS}|{BACKTICK})\z")));
static PARENS_FULL: LazyLock<regex::Regex> = LazyLock::new(|| plain(&format!(r"\A(?:{PARENS})\z")));
static PASS_WORD: LazyLock<regex::Regex> = LazyLock::new(|| plain(r"(?i)(?:^|_)pass(?:$|_)"));

/// Troca os segredos de `text` por marcadores; devolve o texto e os nomes usados, na ordem.
/// `fish` usa as aspas simples do fish; `shell = false` é conteúdo de outra linguagem.
pub fn scrub(text: &str, fish: bool, shell: bool) -> (String, Vec<String>) {
    let mut names = Vec::new();
    match scrub_into(text, fish, shell, &mut names) {
        Ok(text) => (text, names),
        // Regex que estoura o limite de retrocesso: o texto inteiro sai como segredo, nunca cru.
        Err(_) => {
            names.clear();
            let marker = mark(&mut names, "segredo");
            (marker, names)
        }
    }
}

/// Nome do primeiro marcador ainda sem valor no texto.
pub fn has_placeholder(text: Option<&str>) -> Option<String> {
    PLACEHOLDER.captures(text?).map(|found| found[1].to_owned())
}

/// Os nomes dos marcadores do texto, na ordem, com repetição.
pub(crate) fn placeholder_names(text: &str) -> Vec<String> {
    PLACEHOLDER.captures_iter(text).map(|found| found[1].to_owned()).collect()
}

/// Troca cada marcador pelo valor de `values`; valor vazio ou ausente mantém o marcador.
pub(crate) fn fill(text: &str, value: impl Fn(&str) -> Option<String>) -> String {
    PLACEHOLDER
        .replace_all(text, |found: &regex::Captures<'_>| {
            value(&found[1]).filter(|v| !v.is_empty()).unwrap_or_else(|| found[0].to_owned())
        })
        .into_owned()
}

fn scrub_into(text: &str, fish: bool, shell: bool, names: &mut Vec<String>) -> Result<String, fancy_regex::Error> {
    let dialect = match (shell, fish) {
        (false, _) => Dialect::Literal,
        (true, true) => Dialect::Fish,
        (true, false) => Dialect::Bash,
    };
    let rules = &RULES[dialect as usize];
    let mut text = text.to_owned();
    for rule in rules {
        text = substitute(rule, &text, fish, shell, names)?;
    }
    if shell {
        text = substitute(&FISH_SET, &text, fish, shell, names)?;
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for found in TOKEN.find_iter(text.as_str()) {
        let found = found?;
        out.push_str(&text[last..found.start()]);
        out.push_str(&mark(names, "token"));
        last = found.end();
    }
    out.push_str(&text[last..]);
    Ok(out)
}

fn substitute(rule: &Rule, text: &str, fish: bool, shell: bool, names: &mut Vec<String>) -> Result<String, fancy_regex::Error> {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for captures in rule.regex.captures_iter(text) {
        let captures = captures?;
        let whole = captures.get(0).expect("grupo 0 de toda captura");
        out.push_str(&text[last..whole.start()]);
        out.push_str(&replacement(&captures, rule.kind, fish, shell, names));
        last = whole.end();
    }
    out.push_str(&text[last..]);
    Ok(out)
}

fn replacement(captures: &Captures<'_, str>, kind: Kind, fish: bool, shell: bool, names: &mut Vec<String>) -> String {
    let whole = captures.get(0).expect("grupo 0 de toda captura");
    let found = captures.name("v").expect("toda regra tem o valor");
    let token = found.as_str();
    let fish_rule = kind == Kind::FishSet;
    let quoted = if fish || !shell || fish_rule { &QUOTED_FISH } else { &QUOTED_BASH };
    let quote = quoted.is_match(token).then(|| token.chars().next().expect("valor entre aspas"));
    let value = if quote.is_some() { &token[1..token.len() - 1] } else { token };
    if is_reference(value, quote, fish || fish_rule, shell) {
        return whole.as_str().to_owned();
    }
    let base = match kind {
        Kind::Fixed(base) => base.to_owned(),
        Kind::Flag => {
            let flag = captures.name("flag").map(|m| m.as_str().to_lowercase()).unwrap_or_default();
            FLAG_NAMES.iter().find(|(name, _)| *name == flag).map_or("segredo", |(_, base)| *base).to_owned()
        }
        Kind::Env | Kind::FishSet => {
            let name = captures.name("name").map(|m| m.as_str()).unwrap_or_default();
            if !secret_name(name) {
                return whole.as_str().to_owned();
            }
            name.to_owned()
        }
    };
    let marker = mark(names, &base);
    let marker = match quote {
        Some(quote) => format!("{quote}{marker}{quote}"),
        None if !shell => format!("'{marker}'"),
        None => marker,
    };
    let source = whole.as_str();
    let (start, end) = (found.start() - whole.start(), found.end() - whole.start());
    format!("{}{marker}{}", &source[..start], &source[end..])
}

fn is_reference(value: &str, quote: Option<char>, fish: bool, shell: bool) -> bool {
    if PLACEHOLDER_FULL.is_match(value) {
        return true;
    }
    if quote == Some('\'') || !shell {
        return false;
    }
    VARIABLE.is_match(value)
        || SUBSTITUTION.is_match(value)
        || (fish && quote.is_none() && PARENS_FULL.is_match(value))
}

fn secret_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    SECRET_WORDS.iter().any(|word| lower.contains(word)) || PASS_WORD.is_match(name)
}

/// Dois segredos do mesmo tipo no mesmo texto ganham nomes distintos (`senha`, `senha_2`).
fn mark(names: &mut Vec<String>, base: &str) -> String {
    let prefix = format!("{base}_");
    let count = names.iter().filter(|name| *name == base || name.starts_with(&prefix)).count();
    let name = if count == 0 { base.to_owned() } else { format!("{base}_{}", count + 1) };
    let marker = format!("⟦SEGREDO:{name}⟧");
    names.push(name);
    marker
}

fn rules(dialect: Dialect) -> Vec<Rule> {
    let single = if dialect == Dialect::Bash { SINGLE } else { FISH_SINGLE };
    let bare = if dialect == Dialect::Literal { LITERAL_BARE } else { BARE };
    let v = value(single, bare);
    // Os nomes mais longos primeiro: `--password` não pode casar como `--pass`.
    let flags = "client-secret|access-token|auth-token|password|api-key|passwd|apikey|secret|token|pass";
    let env_start = if dialect == Dialect::Literal { r"(?<![\w])\$?" } else { r"(?<![\w$])" };
    let rule = |pattern: String, kind: Kind| Rule { regex: compile(&pattern), kind };
    vec![
        rule(format!(r"(?<![\w/])/(?:p|password):{v}"), Kind::Fixed("senha")),
        rule(format!(r"(?<![\w-])--(?P<flag>{flags})(?:=|\s+){v}"), Kind::Flag),
        rule(format!(r"\bsshpass\s+(?:-\w+\s+)*?-p\s*{v}"), Kind::Fixed("senha")),
        rule(
            format!(r"\b(?:mysql|mysqldump|mysqladmin|mariadb|mariadb-dump)\b[^\n|;&]*?\s-p{v}"),
            Kind::Fixed("senha"),
        ),
        rule(format!(r"(?i)\bAuthorization:\s*(?:Bearer|Basic|Token)\s+{v}"), Kind::Fixed("token")),
        rule(r"(?i)\b[a-z][a-z0-9+.-]*://[^\s:/@]+:(?P<v>[^\s@/]+)@".to_owned(), Kind::Fixed("senha")),
        rule(
            format!(r"{env_start}(?P<name>[A-Za-z_][A-Za-z0-9_]*)[ \t]*=(?![=>])[ \t]*{v}"),
            Kind::Env,
        ),
    ]
}

fn value(single: &str, bare: &str) -> String {
    format!(r"(?P<v>(?:{single}|{DOUBLE}|\${PARENS}|{BACKTICK}|{PARENS}|\\.|{bare})+)")
}

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("regra de segredo constante")
}

fn plain(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).expect("expressão constante")
}
