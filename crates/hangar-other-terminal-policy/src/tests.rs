use hangar_transcript::Provider;

use crate::{CapturedPane, Composer, agy_draft, compact, composer, screen};

/// A tarefa em segundo plano (`1 task(s)`) aparece entre linhas abaixo da caixa: lida como rascunho, a fila parava.
#[test]
fn a_background_task_panel_below_the_box_is_not_a_draft() {
    assert_eq!(
        agy_draft(include_str!(
            "../../hangar-core/tests/fixtures/agy/background_task.txt"
        ))
        .as_deref(),
        Some("")
    );
    assert_eq!(
        agy_draft(include_str!(
            "../../hangar-core/tests/fixtures/agy/background_task_draft.txt"
        ))
        .as_deref(),
        Some("arrumar o teste do login")
    );
    assert_eq!(
        agy_draft(include_str!(
            "../../hangar-core/tests/fixtures/agy/draft.txt"
        ))
        .as_deref(),
        Some("arrumar o teste do login")
    );
}

const KIMI_IDLE: &str = include_str!("../tests/fixtures/kimi_idle.txt");
/// O painel de aprovação do Kimi e a fila da TUI com uma mensagem numerada da pessoa esperando o fim do turno.
const KIMI_APPROVAL: &str = include_str!("../tests/fixtures/kimi_approval.txt");
const KIMI_QUEUED: &str = "  ⠋ Working for 6s\n   ❯ 1. corrige o parser\n   2. roda os testes\n   ↑ to edit · ctrl-s to steer immediately\n ╭──╮\n │ > │\n ╰──╯\n";

#[test]
fn kimi_numbered_message_of_the_person_is_not_a_question() {
    let analysis = screen(Provider::Kimi, KIMI_QUEUED);
    assert!(
        matches!(analysis.state.as_str(), "working" | "idle"),
        "estado que o fim de turno do arquivo da conversa corrige: {}",
        analysis.state
    );
    assert_eq!(
        (analysis.question, analysis.options, analysis.overlay),
        (None, None, false)
    );
    let stopped = KIMI_QUEUED.replace("  ⠋ Working for 6s\n", "");
    assert_eq!(
        screen(Provider::Kimi, &stopped).state,
        "idle",
        "sem spinner, a sessão parada"
    );
    assert_eq!(
        screen(Provider::Claude, KIMI_QUEUED).state,
        "awaiting_input",
        "no Claude Code é menu"
    );
}

#[test]
fn kimi_open_panel_occupies_the_box_without_becoming_a_question() {
    let analysis = screen(Provider::Kimi, KIMI_APPROVAL);
    assert!(analysis.overlay);
    assert_ne!(
        analysis.state, "awaiting_input",
        "a aprovação é publicada pelo arquivo da conversa"
    );
}

#[test]
fn kimi_panel_footer_quoted_in_the_conversation_does_not_occupy_the_box() {
    let history = format!(
        "  ↑/↓ select · 1/2 choose · ↵ confirm\n  type answer\n{}",
        "  linha da conversa\n".repeat(10)
    );
    assert!(!screen(Provider::Kimi, &(history + KIMI_IDLE)).overlay);
    let just_above = format!("  ↑/↓ select · 1/2 choose · ↵ confirm\n{KIMI_IDLE}");
    assert!(
        !screen(Provider::Kimi, &just_above).overlay,
        "citado logo acima da caixa"
    );
}

fn codex_composer(text: &str) -> Option<Composer> {
    let pane = CapturedPane {
        text,
        typed: None,
        styled: None,
        editor: None,
    };
    composer(Provider::Codex, pane)
}

#[test]
fn codex_composer_reads_a_real_capture_with_paragraphs() {
    let box_text = codex_composer(include_str!("../tests/fixtures/pane_codex_paragraphs.txt"))
        .unwrap()
        .content;
    assert_eq!(box_text, "Oi, tudo bem?\n\nSegue o plano completo.");
}

// A caixa de uma linha continua sem a barra de estado.
#[test]
fn composer_of_one_line_stops_before_the_status_bar() {
    let typed = codex_composer(include_str!(
        "../tests/fixtures/pane_codex_composer_text.txt"
    ))
    .unwrap();
    assert!(
        !typed.content.is_empty() && !typed.content.contains('\n'),
        "{:?}",
        typed.content
    );
}

// Depois do Enter no `/resume` a tela não é a caixa com o comando: outro Enter retomaria uma conversa.
#[test]
fn resume_selector_is_not_the_composer_with_the_command() {
    for screen in [
        include_str!("../tests/fixtures/pane_codex_resume_loading.txt"),
        include_str!("../tests/fixtures/pane_codex_resume_list.txt"),
    ] {
        let still_typed =
            codex_composer(screen).is_some_and(|now| compact(&now.content) == compact("/resume"));
        assert!(!still_typed);
    }
}
