//! `contagens-anonimas`: a linha "Contagens anônimas de uso" das Configurações passa de não respondida a ligada.

use std::time::Instant;
use gpui_kit::{component::switch::Switch, *};
use crate::{app::{self, Consent, ConsentChoice}, i18n::tr_shared, theme};

const CYCLE: f32 = 6.;
/// Quando o interruptor liga no ciclo.
const ON: f32 = 1.6;

/// A resposta no instante `t` do ciclo: sem resposta até o interruptor ser ligado.
fn frame(t: f32) -> Option<bool> { (t >= ON).then_some(true) }

/// Segundos até o quadro mudar: o roteiro anda em degraus.
fn until_next_step(t: f32) -> f32 { if t < ON { ON - t } else { CYCLE - t } }

/// Abaixo disto, a linha das Configurações não cabe.
const COMPACT_BELOW: f32 = 240.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado na escolha feita.
    let answer = if cx.reduce_motion() { Some(true) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let choice = ConsentChoice { answer, disabled: false, inactive: false, scope: Some(app::TELEMETRY_B_SCOPE) };
    // Miniatura só leitura: o interruptor fica fora do Tab, e a tampa por cima não deixa o mouse levar o foco para ele.
    let toggle = Switch::new("news-telemetry-switch").tab_stop(false);
    let cover = div().absolute().top_0().left_0().size_full().block_mouse_except_scroll();
    let stage = div().id("news-telemetry-consent").relative().w(width).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .text_color(theme::text());
    if width < px(COMPACT_BELOW) {
        let value = tr_shared(if answer.is_some() { "auto_report_on" } else { "auto_report_value_unanswered" }, &[]);
        let toggle = toggle.checked(answer == Some(true)).accessibility_label(tr_shared("telemetry_setting", &[]));
        return stage.p(px(8.)).flex().flex_col().gap(px(8.)).text_size(px(11.))
            .child(div().font_weight(FontWeight::MEDIUM).whitespace_normal().child(tr_shared("telemetry_setting", &[])))
            .child(div().flex().items_center().gap(px(8.)).child(toggle).child(div().text_color(theme::muted()).child(value)))
            .child(cover).into_any_element();
    }
    stage.p(px(14.))
        .child(app::settings_box().child(app::consent_row(Consent::Telemetry, false, Some(choice), false, false, false, toggle))
            .child(app::telemetry_doc_row(false, choice.scope)))
        .child(cover).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_turns_the_unanswered_row_on() {
        assert_eq!(frame(0.5), None);
        assert_eq!(frame(1.7), Some(true));
        assert_eq!(frame(5.9), Some(true));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.6), 1.0));
        assert!(close(until_next_step(2.0), 4.0));
    }
}
