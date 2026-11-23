//! A tela do `agy` capturada de verdade (agy 1.5.15, 23/11/2026).
use hangar_core::terminal_state::agy_analyze;

fn fixture(name: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/agy").join(name)).unwrap()
}

#[test]
fn the_permission_prompt_is_a_question_with_its_four_options() {
    let analysis = agy_analyze(&fixture("permission.txt"));
    assert_eq!(analysis.state, "awaiting_input");
    let question = analysis.question.unwrap();
    assert!(question.starts_with("Run this command?"), "{question}");
    assert!(question.contains("rm -rf /tmp/agy-prova/apagar-me3"), "{question}");
    let options = analysis.options.unwrap();
    assert_eq!(options.len(), 4);
    assert_eq!(options[0], "Yes, run command");
    assert_eq!(options[3], "No, cancel");
}

#[test]
fn a_running_command_is_working_with_the_spinner_label() {
    let analysis = agy_analyze(&fixture("working.txt"));
    assert_eq!(analysis.state, "working");
    assert_eq!(analysis.label.as_deref(), Some("Running command..."));
}

#[test]
fn the_idle_footer_is_idle() {
    let analysis = agy_analyze(&fixture("idle.txt"));
    assert_eq!(analysis.state, "idle");
    assert!(analysis.options.is_none());
}
