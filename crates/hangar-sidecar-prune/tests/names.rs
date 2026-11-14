//! Nomes de arquivo: a sobra de `tmp+rename` (R6-11) e a chave de cada família (R6-03, R6-04).
use hangar_sidecar_prune::{file_stem, is_tmp};
use proptest::prelude::*;

/// A regra do Python (`prune.py:_TMP_RE`), com a âncora no fim do texto.
fn python_tmp(name: &str) -> bool { regex::Regex::new(r"\.tmp\.?\d*\z").unwrap().is_match(name) }

#[test]
fn r6_11_the_four_tmp_forms_match_and_a_real_sidecar_never_does() {
    for leftover in ["a.json.tmp", "a.json.4321.tmp", "a.json.tmp.99", "a.json.tmp1234"] {
        assert!(is_tmp(leftover), "{leftover}");
    }
    for real in ["2026-08-22T01-05-00-731Z_01a026ff.json", "aaa-bbb.json", "minha-sessao.jsonl", "a.tmp.json", "a.tmp5.5", "a.tmp.."] {
        assert!(!is_tmp(real), "{real}");
    }
}

#[test]
fn the_key_is_the_name_without_the_family_suffix() {
    assert_eq!(file_stem("aaa.json", ".json"), Some("aaa"));
    assert_eq!(file_stem("a.b.json", ".json"), Some("a.b"));
    assert_eq!(file_stem("minha-sessao.jsonl", ".jsonl"), Some("minha-sessao"));
    assert_eq!(file_stem("viva.md", ".md"), Some("viva"));
    // `Path(".json").stem` é o nome inteiro.
    assert_eq!(file_stem(".json", ".json"), Some(".json"));
    assert_eq!(file_stem("x.jsonl", ".json"), None);
    assert_eq!(file_stem("x.json.tmp", ".json"), None);
    assert_eq!(file_stem("ção", ".json"), None);
}

proptest! {
    #[test]
    fn the_tmp_rule_is_the_python_expression(name in "[a-z0-9.]{0,6}(\\.tmp)?[.0-9]{0,4}") {
        prop_assert_eq!(is_tmp(&name), python_tmp(&name));
    }

    #[test]
    fn a_family_file_is_never_a_tmp_and_its_key_comes_back(key in "[A-Za-z0-9_.-]{1,24}", suffix in prop::sample::select(vec![".json", ".jsonl", ".md"])) {
        let name = format!("{key}{suffix}");
        prop_assert!(!is_tmp(&name));
        prop_assert_eq!(file_stem(&name, suffix), Some(key.as_str()));
    }

    #[test]
    fn a_name_outside_the_family_has_no_key(name in "[a-z.]{0,12}") {
        prop_assume!(!name.ends_with(".json"));
        prop_assert_eq!(file_stem(&name, ".json"), None);
    }
}
