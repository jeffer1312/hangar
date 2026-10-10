//! As travas comparam conteúdo e preservam as quebras de linha da organização.
use std::collections::HashSet;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};
use super::{model::DictationStyle, prompts::SPEC};

pub fn normalized(text: &str) -> String {
    let clean: String = text.chars().filter(|c| !matches!(c, '\u{200b}'|'\u{200c}'|'\u{200d}'|'\u{2060}'|'\u{feff}')).collect();
    let mut lines = Vec::new();
    for line in clean.trim().lines().map(str::trim) {
        if line.is_empty() && lines.last().is_none_or(|last: &&str| last.is_empty()) { continue; }
        lines.push(line);
    }
    lines.join("\n").trim().to_owned()
}
fn words(text: &str) -> Vec<String> {
    let text: String = text.to_lowercase().nfkd().filter(|c| !is_combining_mark(*c)).collect();
    text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|s| !s.is_empty())
        .map(|s| SPEC.contractions.get(s).cloned().unwrap_or_else(|| s.into())).collect()
}
fn singular(word: &str) -> &str { if word.ends_with('s') && word.len() > 4 { &word[..word.len()-1] } else { word } }
fn cut<'a>(word: &'a str, suffixes: &[String]) -> Option<&'a str> {
    for candidate in [word, singular(word)] { for suffix in suffixes {
        if let Some(root) = candidate.strip_suffix(suffix.as_str()) && root.len() >= 4 { return Some(root); }
    }}
    None
}
fn content(words: &[String], roots: &HashSet<String>) -> HashSet<String> {
    words.iter().filter(|w| w.len() >= 3 && !SPEC.noise.contains(*w)).map(|w| {
        if let Some(root) = cut(w, &SPEC.verb_suffixes).or_else(|| cut(w, &SPEC.derivation_suffixes)) { return root.to_owned(); }
        let one = singular(w);
        if one.len() > 4 && one.ends_with(['a','e','i','o']) && roots.contains(&one[..one.len()-1]) { one[..one.len()-1].to_owned() }
        else { one.into() }
    }).collect()
}
pub fn validate(raw: &str, output: &str, style: DictationStyle) -> Result<(), &'static str> {
    if output.is_empty() { return Err("A organização devolveu texto vazio; foi mantida a transcrição original."); }
    let (inflate, shrink, coverage, invention) = match style {
        DictationStyle::Clean => (1.5, 0.5, 0.80, true),
        DictationStyle::Prose => (1.3, 0.3, 0.60, true),
        DictationStyle::Briefing => (1.4, 0.48, 0.45, false),
    };
    let (raw_len, output_len) = (raw.chars().count() as f64, output.chars().count() as f64);
    if output_len > inflate * raw_len { return Err("A organização respondeu em vez de organizar; foi mantido o original."); }
    let (before, after) = (words(raw), words(output));
    let roots: HashSet<String> = before.iter().chain(&after).filter_map(|w| cut(w, &SPEC.verb_suffixes).map(str::to_owned)).collect();
    let (a, b) = (content(&before, &roots), content(&after, &roots));
    if (raw_len > 120.0 || a.is_empty()) && output_len < shrink * raw_len { return Err("A organização resumiu a fala; foi mantido o original."); }
    if !a.is_empty() && a.intersection(&b).count() as f64 / (a.len() as f64) < coverage { return Err("A organização descartou parte da fala; foi mantido o original."); }
    if invention && (b.difference(&a).count() as f64) > 2.0_f64.max(0.02 * b.len() as f64) { return Err("A organização acrescentou conteúdo; foi mantido o original."); }
    Ok(())
}
