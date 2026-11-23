//! `portas-da-sessao`: a sessão abre o Next e o Postgres, as portas aparecem, uma é encaminhada e a outra ganha
//! o link de fora, com as partes da seção Portas da aba Contexto.

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 7.5;
/// As portas aparecem, a 5432 é encaminhada, a 3000 ganha o link de fora.
const TRANSITIONS: [f32; 3] = [1.4, 3.0, 4.6];
/// A 5432 deste computador já está ocupada: a encaminhada vira a seguinte.
const LOCAL_PG: u16 = 5433;
const LINK: &str = "https://casa--preview.ana.hangar.dev.br/";

fn step_at(t: f32) -> usize { TRANSITIONS.partition_point(|at| *at <= t) }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: as duas portas, uma encaminhada e a outra com o link de fora.
    let step = if cx.reduce_motion() { TRANSITIONS.len() } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        let step = step_at(t);
        super::repaint_in(TRANSITIONS.get(step).copied().unwrap_or(CYCLE) - t, window, cx);
        step
    };
    let row = |port: u16, process: &str, forwarded: bool, link: bool, cx: &App| {
        let b = app::port_buttons(port);
        app::port_row_view(format!("{port} · {process}"), forwarded.then_some(LOCAL_PG))
            .child(app::port_actions()
                .child(b.open.tab_stop(false))
                .child(if forwarded { b.stop } else { b.forward }.tab_stop(false))
                .when(forwarded, |el| el.child(b.copy.tab_stop(false)))
                .child(b.link.tab_stop(false)))
            .when(link, |el| {
                let (copy, open) = app::port_link_buttons(port, cx);
                el.child(app::port_link_view(LINK.into(), copy.tab_stop(false), open.tab_stop(false)))
            })
    };
    let body = if step == 0 {
        app::port_note(tr("ports_empty"), theme::muted()).into_any_element()
    } else {
        div().flex().flex_col().gap(px(10.))
            .child(row(3000, "next-server", false, step >= 3, cx))
            .child(row(5432, "postgres", step >= 2, false, cx))
            .into_any_element()
    };
    // Altura do quadro mais alto: o cartão não pula entre os passos.
    div().id("news-session-ports").w(width).h(px(240.)).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(app::chrome::section_label(tr("ports_title")))
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn the_ports_appear_then_one_is_forwarded_and_the_other_gets_the_link() {
        assert_eq!([0.5, 2., 3.5, 6.].map(step_at), [0, 1, 2, 3]);
    }
}
