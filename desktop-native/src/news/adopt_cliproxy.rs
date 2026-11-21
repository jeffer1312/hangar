//! `entregar-cliproxy`: a linha do CLIProxyAPI de fora em Contas, o "Deixar o Hangar cuidar", as quatro etapas e o concluído.

use std::time::Instant;
use gpui_kit::*;
use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 7.;
const CLICK: f32 = 1.8;
const STEPS: [(f32, &str); 4] = [(CLICK, "download"), (2.6, "config"), (3.4, "service"), (4.2, "start")];
const DONE: f32 = 5.;
const OLD: &str = "v8.0.56";
const NEW: &str = "v8.0.79";

#[derive(Debug, PartialEq)]
enum Frame { External, Running(&'static str), Done }

fn frame(t: f32) -> Frame {
    if t < CLICK { return Frame::External; }
    if t >= DONE { return Frame::Done; }
    Frame::Running(STEPS.iter().rev().find(|(at, _)| t >= *at).map_or("download", |(_, step)| step))
}

fn until_next_step(t: f32) -> f32 {
    STEPS.iter().map(|(at, _)| *at).chain([DONE, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no proxy de fora com o "Deixar o Hangar cuidar", o que a novidade traz.
    let shown = if cx.reduce_motion() { Frame::External } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let external = shown != Frame::Done;
    let view = app::CliproxyCardView {
        version: Some(if external { OLD } else { NEW }.to_owned()), external, address: Some("127.0.0.1:8317".to_owned()),
        accounts: Some(3), sessions: None,
    };
    let connect = app::cliproxy_connect_button("news-cliproxy-connect").tab_stop(false).into_any_element();
    let (actions, body) = match shown {
        Frame::External => (vec![app::cliproxy_adopt_button("news-cliproxy-adopt").tab_stop(false).into_any_element(), connect],
            app::cliproxy_card_note(tr("cliproxy_card_external_note"), theme::muted())),
        Frame::Running(step) => (vec![connect], app::cliproxy_adopt_running_banner(NEW, step)),
        Frame::Done => (vec![connect], app::cliproxy_adopt_done_banner(NEW)),
    };
    div().id("news-cliproxy-adopt").w(width).overflow_hidden().p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(div().text_size(px(16.)).font_weight(FontWeight::SEMIBOLD).child(tr("settings_page_accounts")))
        .child(div().min_h(px(210.)).child(app::cliproxy_card_view(&view, actions, Some(body))))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn external_then_the_four_steps_then_done() {
        assert_eq!([frame(0.5), frame(2.), frame(3.), frame(3.8), frame(4.5), frame(6.)],
            [Frame::External, Frame::Running("download"), Frame::Running("config"), Frame::Running("service"), Frame::Running("start"), Frame::Done]);
        assert!((until_next_step(0.5) - 1.3).abs() < 1e-4);
        assert!((until_next_step(6.) - 1.).abs() < 1e-4);
    }
}
