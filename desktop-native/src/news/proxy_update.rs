//! `atualizar-cliproxy`: a faixa de versão nova do CLIProxyAPI em Contas, o Atualizar, as três etapas e o concluído.

use std::time::Instant;
use gpui_kit::*;
use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 6.;
const CLICK: f32 = 1.6;
const STEPS: [(f32, &str); 3] = [(CLICK, "download"), (2.5, "service"), (3.5, "start")];
const DONE: f32 = 4.4;
const OLD: &str = "v8.0.79";
const NEW: &str = "v8.1.0";

#[derive(Debug, PartialEq)]
enum Frame { Available, Running(&'static str), Done }

fn frame(t: f32) -> Frame {
    if t < CLICK { return Frame::Available; }
    if t >= DONE { return Frame::Done; }
    Frame::Running(STEPS.iter().rev().find(|(at, _)| t >= *at).map_or("download", |(_, step)| step))
}

fn until_next_step(t: f32) -> f32 {
    STEPS.iter().map(|(at, _)| *at).chain([DONE, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado na versão nova com o Atualizar, o que a novidade traz.
    let shown = if cx.reduce_motion() { Frame::Available } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let banner = match shown {
        Frame::Available => app::cliproxy_update_available_banner(NEW, OLD, false, app::cliproxy_update_button("news-cliproxy-update").tab_stop(false)),
        Frame::Running(step) => app::cliproxy_update_running_banner(NEW, step),
        Frame::Done => app::cliproxy_update_done_banner(NEW),
    };
    div().id("news-cliproxy-update").w(width).overflow_hidden().p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(tr("settings_page_accounts")))
        .child(div().min_h(px(132.)).child(banner))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn available_then_the_three_steps_then_done() {
        assert_eq!([frame(0.5), frame(2.), frame(3.), frame(4.), frame(5.)],
            [Frame::Available, Frame::Running("download"), Frame::Running("service"), Frame::Running("start"), Frame::Done]);
        assert!((until_next_step(0.5) - 1.1).abs() < 1e-4);
        assert!((until_next_step(5.) - 1.).abs() < 1e-4);
    }
}
