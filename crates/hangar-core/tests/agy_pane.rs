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

// Os rodapés abaixo são os medidos no agy 1.5.15; o resto da tela é o de `idle.txt`.
use hangar_core::terminal_state::{agy_footer, agy_model_key, agy_model_set};

#[test]
fn the_footer_gives_the_mode_in_claude_names_and_the_model() {
    for (name, mode, model, effort) in [
        ("idle.txt", "auto", "Gemini 3.8 Flash", "low"),
        ("working.txt", "auto", "Gemini 3.8 Flash", "low"),
        ("footer_plan.txt", "plan", "Gemini 4 Neon", "medium"),
        ("footer_accept_edits.txt", "acceptEdits", "Gemini 4 Neon", "medium"),
        ("draft.txt", "auto", "Gemini 4 Neon", "low"),
    ] {
        let footer = agy_footer(&fixture(name)).unwrap_or_else(|| panic!("{name}: sem rodapé"));
        assert_eq!((footer.mode, footer.model.as_str(), footer.effort.as_deref()), (Some(mode), model, Some(effort)), "{name}");
    }
}

/// O modo padrão (o que pergunta) não tem rótulo: é o `manual`, inclusive com o pedido de permissão aberto.
#[test]
fn the_footer_without_a_label_is_the_default_mode() {
    for name in ["footer_default.txt", "permission.txt"] {
        assert_eq!(agy_footer(&fixture(name)).and_then(|footer| footer.mode), Some("manual"), "{name}");
    }
}

#[test]
fn an_unknown_label_is_no_mode_and_a_line_without_the_separator_is_no_footer() {
    let footer = agy_footer("texto\n? for shortcuts      yolo · Gemini 4 Neon · high\n").unwrap();
    assert_eq!((footer.mode, footer.model.as_str()), (None, "Gemini 4 Neon"));
    assert_eq!(agy_footer("texto\nRode Isto Agora\n"), None);
    assert_eq!(agy_footer("x · y · z · w\n"), None);
    assert_eq!(agy_footer(""), None);
}

#[test]
fn the_footer_model_is_compared_with_the_agy_ids() {
    let footer = agy_footer(&fixture("footer_plan.txt")).unwrap();
    assert_eq!(footer.model_id(), "gemini-4-neon-medium");
    assert!(footer.is_model("gemini-4-neon-medium"));
    assert!(!footer.is_model("gemini-4-neon-low"));
    let flash = agy_footer(&fixture("idle.txt")).unwrap();
    assert!(flash.is_model("gemini-3.8-flash-low") && flash.is_model("gemini-3-8-flash-low"));
    assert_eq!(agy_model_key(" Gemini 4_Neon "), "gemini-4-neon");
}

#[test]
fn the_model_set_line_is_read_from_the_screen() {
    assert_eq!(agy_model_set(&fixture("model_set.txt")), ["Gemini 4 Neon (Medium)"]);
    assert!(agy_model_set(&fixture("idle.txt")).is_empty());
}

/// A dica do modo na caixa vazia e o rodapé não mudam o estado: parado continua parado.
#[test]
fn the_mode_hint_and_footers_keep_the_idle_state() {
    for name in ["footer_plan.txt", "footer_accept_edits.txt", "footer_default.txt", "model_set.txt"] {
        assert_eq!(agy_analyze(&fixture(name)).state, "idle", "{name}");
    }
}
