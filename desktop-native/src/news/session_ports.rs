//! `portas-da-sessao`: a sessão abre o Next e o Postgres, as portas aparecem com Abrir e Abrir no navegador do
//! computador, a 5432 é encaminhada e o servidor da 3000 é parado, com as partes da seção Portas da aba Contexto.

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 7.5;
/// As portas aparecem, a 5432 é encaminhada, o servidor da 3000 para e a porta some.
const TRANSITIONS: [f32; 4] = [1.2, 2.8, 4.4, 5.8];
/// Movimento reduzido: as duas portas, a 5432 encaminhada, todas as ações à vista.
const STILL: usize = 2;
/// A 5432 deste computador já está ocupada: a encaminhada vira a seguinte.
const LOCAL_PG: u16 = 5433;

fn step_at(t: f32) -> usize { TRANSITIONS.partition_point(|at| *at <= t) }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let step = if cx.reduce_motion() { STILL } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        let step = step_at(t);
        super::repaint_in(TRANSITIONS.get(step).copied().unwrap_or(CYCLE) - t, window, cx);
        step
    };
    // Sessão de outra máquina, como a tela real desenha: sem encaminhamento não há endereço para copiar.
    let row = |port: u16, process: &str, forwarded: bool, stopping: bool| {
        let b = app::port_buttons(port);
        app::port_row_view(format!("{port} · {process}"), forwarded.then_some(LOCAL_PG))
            .child(app::port_actions()
                .child(b.open.tab_stop(false))
                .child(b.open_system.tab_stop(false))
                .child(if forwarded { b.stop } else { b.forward }.tab_stop(false))
                .when(forwarded, |el| el.child(b.copy.tab_stop(false)))
                .child(b.link.tab_stop(false))
                .child(b.stop_server.loading(stopping).tab_stop(false)))
    };
    let body = if step == 0 {
        div().into_any_element()
    } else {
        div().flex().flex_col().gap(px(10.))
            .when(step < 4, |el| el.child(row(3000, "next-server", false, step == 3)))
            .child(row(5432, "postgres", step >= 2, false))
            .into_any_element()
    };
    // Altura do quadro mais alto: o cartão não pula entre os passos.
    div().id("news-session-ports").w(width).h(px(250.)).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
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
    fn the_ports_appear_then_one_is_forwarded_and_the_other_server_stops() {
        assert_eq!([0.5, 2., 3.5, 5., 6.].map(step_at), [0, 1, 2, 3, 4]);
    }
}
