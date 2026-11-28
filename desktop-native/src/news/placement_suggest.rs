//! `maquina-sugerida`: a Nova conversa troca sozinha a pílula da máquina pela sugerida e mostra o motivo.

use std::time::Instant;
use gpui_kit::{assets::IconName, base::Selectable, *};
use serde_json::json;
use crate::{app::{self, chrome}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// A sugestão chega: a pílula passa para a outra máquina e acende.
const SWITCH: f32 = 1.4;
/// O motivo aparece sob a pílula.
const REASON: f32 = 1.8;
/// O motivo some; a pílula fica na sugerida.
const HIDE: f32 = 5.2;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { moved: bool, lit: bool, reason: bool }

fn frame(t: f32) -> Frame {
    Frame { moved: t >= SWITCH, lit: (SWITCH..REASON).contains(&t), reason: (REASON..HIDE).contains(&t) }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [SWITCH, REASON, HIDE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado na sugerida, com o motivo à vista.
    let Frame { moved, lit, reason } = if cx.reduce_motion() { frame(REASON) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let (busy, free) = (tr("news_placement_busy"), tr("news_placement_free"));
    // O mesmo texto que a pílula mostra, montado dos códigos que o servidor devolve.
    let why = app::placement_reason_text(&json!({"server": free, "reason": {"code": "least_loaded", "local_out": "working"}}));
    let field = div().text_size(px(13.5)).min_h(px(22.)).flex().items_center().overflow_hidden()
        .child(div().truncate().text_color(theme::muted()).child(tr("composer").replace("{agent}", "Claude")));
    let send = app::send_button("send", false, tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field).child(app::composer_control_row([], [], send));
    let pills = div().flex().items_center().gap(px(2.)).pt(px(2.)).pl(px(6.))
        .child(app::quiet_button("news-placement-machine", IconName::Laptop, if moved { free } else { busy }, true, cx).selected(lit).tab_stop(false))
        .child(app::quiet_button("news-placement-folder", IconName::Folder, tr_shared("news_tags_sample_session", &[]), true, cx).tab_stop(false));
    let tip = reason.then(|| div().pl(px(6.)).pt(px(4.)).child(div().w_full().max_w(px(360.)).child(chrome::popover(
        div().px(px(10.)).py(px(8.)).text_size(px(12.5)).text_color(theme::muted())
            .child(tr("new_chat_machine_suggested").replace("{motivo}", &why)).into_any_element(), false))));
    // Altura do quadro com o motivo à vista: o cartão não pula entre os passos.
    div().id("news-placement-suggest").w(width).min_h(px(260.)).overflow_hidden().p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(composer).child(pills).children(tip).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_pill_moves_to_the_suggested_machine_and_the_reason_shows_then_leaves() {
        assert_eq!(frame(0.5), Frame { moved: false, lit: false, reason: false });
        assert_eq!(frame(1.5), Frame { moved: true, lit: true, reason: false });
        assert_eq!(frame(3.), Frame { moved: true, lit: false, reason: true });
        assert_eq!(frame(5.5), Frame { moved: true, lit: false, reason: false });
    }
}
