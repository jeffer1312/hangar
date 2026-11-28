use std::sync::LazyLock;

use hangar_core::terminal_state;
use hangar_transcript::Provider;
use regex::Regex;

use crate::controls::CURSOR;

/// Marcas de TUI pronta do `_wait_input_ready` do Python, procuradas só no rodapé.
const CODEX_READY: [&str; 5] = [
    "⏵⏵",
    "⏸",
    "bypass permissions",
    "? for shortcuts",
    "for shortcuts",
];
const CODEX_READY_LINES: usize = 12;
const CODEX_MENU_FOOTERS: [&str; 5] = [
    "enter select",
    "enter to confirm",
    "enter to continue",
    "enter confirm",
    "enter continue",
];

static APPROVAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"↵\s*confirm|Type feedback").unwrap());
static PICKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"↵\s*(choose|toggle|save)|type answer").unwrap());
pub fn kimi_approval_open(pane: &str) -> bool {
    APPROVAL.is_match(pane)
}
pub fn kimi_picker_open(pane: &str) -> bool {
    PICKER.is_match(pane)
}

/// A leitura da tela pelo agente: o `agy` tem rodapé e menu próprios; os outros usam o leitor comum.
pub fn screen(provider: Provider, pane: &str) -> terminal_state::PaneAnalysis {
    match provider {
        Provider::Antigravity => terminal_state::agy_analyze(pane),
        // O Kimi desenha a mensagem da pessoa como `❯ texto`, no histórico e na fila da TUI: com número, `❯ 1. …` pareceria
        // o cursor de um menu. Pergunta e aprovação dele vêm do arquivo da conversa; da tela só valem os painéis abertos.
        Provider::Kimi => {
            let mut analysis = terminal_state::analyze(pane);
            if analysis.state == "awaiting_input" {
                analysis.question = None;
                analysis.options = None;
                analysis.label = analysis.spinner.as_deref().map(|spinner| {
                    spinner
                        .chars()
                        .skip(2)
                        .collect::<String>()
                        .trim()
                        .to_owned()
                });
                analysis.state = if analysis.spinner.is_some() {
                    "working"
                } else {
                    "idle"
                }
                .into();
            }
            // O painel de aprovação ou de escolha (rodapé `↵ confirm`, `↵ choose`…) ocupa a caixa: nada se digita sobre ele.
            // Só o fim da tela, abaixo da caixa: o painel aberto ocupa o lugar dela, e o rodapé citado na conversa
            // fica acima.
            let lines: Vec<&str> = pane.trim_end().lines().collect();
            let after_box = lines
                .iter()
                .rposition(|line| line.trim_start().starts_with('╰'))
                .map_or(0, |at| at + 1);
            let bottom = lines[after_box.max(lines.len().saturating_sub(8))..].join("\n");
            analysis.overlay |= kimi_approval_open(&bottom) || kimi_picker_open(&bottom);
            analysis
        }
        _ => terminal_state::analyze(pane),
    }
}

pub fn ready(provider: Provider, pane: &str) -> bool {
    if provider == Provider::Codex {
        // No pane inteiro a marca casaria com o texto da conversa.
        return pane
            .trim_end_matches('\n')
            .split('\n')
            .rev()
            .take(CODEX_READY_LINES)
            .any(|line| CODEX_READY.iter().any(|mark| line.contains(mark)));
    }
    pane.trim_end_matches('\n')
        .lines()
        .rev()
        .take(20)
        .any(|line| line.chars().any(|c| "─━═╰│".contains(c)))
}

/// Menu ou diálogo do Codex na tela: o seletor que a lista reconhece (`codex_menu_of`) ou opções
/// numeradas com o cursor e o rodapé de escolha. O de modelo e esforço ("enter select") fica só aqui:
/// em `CODEX_FOOTERS` ele viraria pergunta para o app.
pub fn codex_menu_open(pane: &str) -> bool {
    if terminal_state::codex_menu_of(pane).is_some() {
        return true;
    }
    // A captura traz o resto do pane em branco: o rodapé é das últimas linhas com texto.
    let footer = pane.trim_end().lines().rev().take(8).any(|line| {
        let line = line.to_lowercase();
        CODEX_MENU_FOOTERS.iter().any(|mark| line.contains(mark))
    });
    footer && CURSOR.is_match(pane)
}
