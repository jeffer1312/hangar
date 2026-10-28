//! As regras medidas do condutor, usadas também na leitura da linha do tempo.
use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;
static RULES: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("fixtures/triage-rules.json")).unwrap());
static STATUS: LazyLock<Vec<(String, Regex)>> = LazyLock::new(|| {
    super::array(&RULES["status"])
        .iter()
        .map(|v| {
            (
                super::text(&v[0]).to_owned(),
                Regex::new(&format!("(?im){}", super::text(&v[1]))).unwrap(),
            )
        })
        .collect()
});
static VETO: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    super::array(&RULES["veto"])
        .iter()
        .enumerate()
        .filter(|(i, _)| !matches!(i, 2 | 12))
        .map(|(_, v)| Regex::new(&format!("(?im){}", super::text(v))).unwrap())
        .collect()
});
static WAITING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?im)\b(aguardo|espero|aguardando)\b").unwrap());
static ALLOWED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\b(veredito|parecer|revisor|a revis)").unwrap());
static NOTED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?im)\bNOTED\b").unwrap());
static ADDRESSED: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?im)para ti\b|\b[ée] teu\b|\b[ée] tua\b").unwrap());
pub fn decide(text: &str) -> (&'static str, Option<String>) {
    if text.chars().count() > 600 || VETO.iter().any(|r| r.is_match(text)) {
        return ("wake", None);
    }
    for m in WAITING.find_iter(text) {
        let tail = text[m.end()..].split(['.', '\n']).next().unwrap_or("");
        let ok = ALLOWED.find_iter(tail).any(|a| {
            tail[..a.start()].chars().count() <= 30
                && !(a.as_str().eq_ignore_ascii_case("revisor")
                    && tail[a.end()..].get(..4).is_some_and(|prefix| prefix.eq_ignore_ascii_case(" que")))
        });
        if !ok {
            return ("wake", None);
        }
    }
    if NOTED.find_iter(text).any(|m| {
        let prefix = &text[..m.start()];
        !prefix.ends_with("0 ")
            || prefix[..prefix.len().saturating_sub(2)]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_alphanumeric() || c == '_')
    }) || ADDRESSED.is_match(text)
    {
        return ("wake", None);
    }
    for (category, rx) in STATUS.iter() {
        if rx.is_match(text) {
            return ("drop", Some(category.clone()));
        }
    }
    ("wake", None)
}

#[cfg(test)]
mod tests {
    #[test]
    fn or039_waiting_veto_preserves_python_case_insensitivity() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../backend/tests/fixtures/contract/orq/triage-case.json"
        )).unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let text = case["text"].as_str().unwrap();
            let (verdict, category) = super::decide(text);
            assert_eq!(serde_json::json!([verdict, category]), case["expected"], "{text}");
        }
    }
}
