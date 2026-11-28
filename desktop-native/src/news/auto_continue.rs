//! `continuar-depois-da-queda`: a conexão cai no meio do turno e a nota espera a rede, com o Cancelar; quando ela
//! volta, a mesma nota diz a hora em que continuou e a sessão volta a trabalhar.

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app::{self, chrome, RowSub}, i18n::tr_shared, theme};

const CYCLE: f32 = 6.5;
/// A conexão cai: o turno para e a nota espera a rede.
const DROP: f32 = 0.9;
/// A rede volta: a nota diz quando continuou e a sessão trabalha de novo.
const BACK: f32 = 3.6;
/// Horários da amostra: a queda e a volta.
const STOPPED: &str = "14:02";
const CONTINUED: &str = "14:05";
/// Altura fixa: o cartão não pula quando a nota e a segunda linha da sessão entram e saem.
const STAGE_HEIGHT: f32 = 132.;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step { Working, Waiting, Continued }

fn step(t: f32) -> Step { if t >= BACK { Step::Continued } else if t >= DROP { Step::Waiting } else { Step::Working } }

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 { [DROP, BACK, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: o desfecho, que diz a queda e a hora em que continuou.
    let step = if cx.reduce_motion() { Step::Continued } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        self::step(t)
    };
    let pad = if width < px(400.) { 6. } else { 14. };
    let list_width = (f32::from(width) - 2. * pad).min(300.);
    // Enquanto espera a rede o turno está parado; antes da queda e depois da volta, trabalhando.
    let working = step != Step::Waiting;
    let avatar = chrome::session_avatar(&chrome::AvatarLook::new("claude", None, if working { "working" } else { "idle" }, false), 30., None);
    let row = app::session_row_frame("news-auto-continue-row", false)
        .child(avatar)
        .child(app::session_row_text().child(app::session_row_name(tr_shared("news_tags_sample_session", &[])))
            .when(working, |el| el.child(app::session_row_sub(tr_shared("news_codex_agent_task", &[]), RowSub::Working, theme::muted(), true))));
    let note = match step {
        Step::Working => None,
        Step::Waiting => Some(app::auto_continue_row(tr_shared("auto_continue_waiting_line", &[("stopped", STOPPED)]),
            Some(app::notice_action_button("news-auto-continue-cancel", tr_shared("auto_continue_cancel", &[])).tab_stop(false).into_any_element()))),
        Step::Continued => Some(app::auto_continue_row(tr_shared("auto_continue_line", &[("stopped", STOPPED), ("continued", CONTINUED)]), None)),
    };
    div().id("news-auto-continue").relative().w(width).h(px(STAGE_HEIGHT)).p(px(pad)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(12.))
        .child(div().w(px(list_width)).child(row))
        .children(note.map(|note| app::notice_rule_line(note)))
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
    fn the_turn_drops_waits_for_the_network_and_continues() {
        assert_eq!(step(0.5), Step::Working);
        assert_eq!(step(2.), Step::Waiting);
        assert_eq!(step(5.), Step::Continued);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 0.4));
        assert!(close(until_next_step(2.), 1.6));
        assert!(close(until_next_step(5.), 1.5));
    }
}
