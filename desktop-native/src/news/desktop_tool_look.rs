//! `estilo-claude-desktop`: as chamadas seguidas viram uma linha cinza com o resumo, que abre no cartão com uma linha por
//! chamada e "Falhou" nas que deram erro.

use std::time::Instant;
use gpui_kit::*;
use serde_json::{Map, Value};
use crate::{api::dto::ChatEvent, app, theme};

const CYCLE: f32 = 6.5;
/// A linha do resumo abre no cartão.
const OPEN: f32 = 1.6;
/// Altura do quadro com o cartão aberto: o cartão da novidade não pula quando ele abre.
const STAGE_HEIGHT: f32 = 214.;
/// Os comandos da amostra e se cada um falhou.
const SAMPLE: [(&str, bool); 4] = [("npm run build", false), ("cargo test -p hangar-api", true), ("git status --short", false), ("npm test", true)];

fn open(t: f32) -> bool { t >= OPEN }

/// Segundos até o próximo instante em que o quadro muda: só a abertura e o fim do ciclo.
fn until_next_step(t: f32) -> f32 { [OPEN, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

fn bash(n: usize, command: &str) -> ChatEvent {
    let mut input = Map::new();
    input.insert("command".into(), Value::String(command.into()));
    let id = format!("news-desktop-{n}");
    ChatEvent { kind: "tool_use".into(), tool_name: Some("Bash".into()), tool_input: Some(input), tool_use_id: Some(id.clone()), id, ..Default::default() }
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado com o cartão aberto.
    let open = if cx.reduce_motion() { true } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        open(t)
    };
    let calls: Vec<(ChatEvent, bool)> = SAMPLE.iter().enumerate().map(|(n, (command, failed))| (bash(n, command), *failed)).collect();
    let summary = app::desktop_summary(calls.iter().map(|(call, failed)| (call, *failed, false)));
    let header = app::desktop_summary_button("news-desktop-summary".into(), summary, open, cx).tab_stop(false);
    let rows = if open {
        calls.iter().map(|(call, failed)| app::desktop_call_button(format!("news-{}", call.id), call, *failed, false, cx).tab_stop(false).into_any_element()).collect()
    } else { Vec::new() };
    div().id("news-desktop-tool-look").relative().w(width).min_h(px(STAGE_HEIGHT)).p(px(14.)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(app::desktop_group_block(header, open, rows))
        // Miniatura só leitura: nada aqui dentro recebe o mouse.
        .child(div().absolute().top_0().left_0().size_full().block_mouse_except_scroll())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_summary_opens_after_a_moment() {
        assert!(!open(0.5));
        assert!(open(OPEN));
        assert!(open(6.));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 1.1));
        assert!(close(until_next_step(2.), 4.5));
    }

    #[test]
    fn sample_summary_counts_the_failures() {
        crate::i18n::english_in_this_thread(false);
        let calls: Vec<ChatEvent> = SAMPLE.iter().enumerate().map(|(n, (c, _))| bash(n, c)).collect();
        assert_eq!(app::desktop_summary(calls.iter().zip(SAMPLE).map(|(call, (_, failed))| (call, failed, false))), "Executou 4 comandos (2 falhas)");
    }
}
