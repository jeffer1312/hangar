//! As conversas da sonda viram dados de teste num repositório público: nada pessoal pode entrar.
use crate::mods_support;

use mods_support::fixtures;
use serde_json::Value;

const FILES: [&str; 3] = ["vitrine.jsonl", "sem-plugins.jsonl", "sem-vitrine.jsonl"];

/// `@` solto (e-mail). O `@@` de cabeçalho de diff (caso V21 da vitrine) é conteúdo do mod.
fn lone_at(raw: &str) -> usize {
    let bytes = raw.as_bytes();
    (0..bytes.len()).filter(|&i| bytes[i] == b'@' && !(i > 0 && bytes[i - 1] == b'@')
        && !(i + 1 < bytes.len() && bytes[i + 1] == b'@')).count()
}

#[test]
fn fixtures_have_only_interface_lines_and_no_personal_data() {
    for name in FILES {
        let raw = std::fs::read_to_string(fixtures().join(name)).unwrap();
        assert!(!raw.is_empty(), "{name} vazio");
        assert_eq!(lone_at(&raw), 0, "{name}: arroba fora de um `@@` de diff");
        for forbidden in ["session_id", "\"uuid\"", "initialize", "/home/", "C:\\\\Users"] {
            assert!(!raw.contains(forbidden), "{name}: contém {forbidden}");
        }
        for line in raw.lines() {
            let entry: Value = serde_json::from_str(line).unwrap();
            assert!(matches!(entry["dir"].as_str(), Some("out" | "in")), "{name}: direção {}", entry["dir"]);
            let message = &entry["msg"];
            let interface = match message["type"].as_str() {
                Some("control_request") => message["request"]["subtype"].as_str().is_some_and(|s| s.starts_with("ui_")),
                Some("control_response") => true,
                Some("system") => message["subtype"].as_str().is_some_and(|s| s.starts_with("ui_")),
                _ => false,
            };
            assert!(interface, "{name}: linha fora da interface: {}", message["type"]);
        }
    }
}

#[test]
fn fixtures_folder_holds_only_the_cleaned_files() {
    let mut names: Vec<String> = std::fs::read_dir(fixtures()).unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    assert_eq!(names, ["limpar_sonda.py", "sem-plugins.jsonl", "sem-vitrine.jsonl", "vitrine.jsonl"]);
}
