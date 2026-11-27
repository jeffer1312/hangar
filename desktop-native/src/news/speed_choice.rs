//! `velocidade-ultra-fast`: no menu do modelo a velocidade passa de Normal a Fast e a Ultra Fast, e a pílula mostra a escolha.

use std::time::Instant;
use gpui_kit::{component::ActiveTheme, *};
use crate::{app::{self, chrome}, i18n::tr, theme};

const CYCLE: f32 = 6.;
const FAST: f32 = 1.8;
const ULTRA: f32 = 3.6;
const MODEL: &str = "gpt-6.2-sol";

/// A posição da escolha em `SPEED_TIERS`.
fn frame(t: f32) -> usize { if t >= ULTRA { 2 } else if t >= FAST { 1 } else { 0 } }

fn until_next_step(t: f32) -> f32 { [FAST, ULTRA, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no Ultra Fast escolhido, o que a novidade traz.
    let selected = if cx.reduce_motion() { 2 } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let (labels, hints) = app::speed_texts();
    let menu = chrome::popover(div().p(px(10.)).flex().flex_col().gap(px(8.))
        .child(div().text_sm().child(tr("ctl_speed")))
        .child(app::segments_preview("news-speed", &labels, selected, cx))
        .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(hints[selected].clone()))
        .into_any_element(), false);
    let field = div().text_size(px(13.5)).min_h(px(22.)).flex().items_center().overflow_hidden()
        .child(div().truncate().text_color(cx.theme().muted_foreground).child(tr("composer").replace("{agent}", "Claude")));
    let pill = app::model_pill("news-speed-model", "codex", MODEL.into(), None, app::speed_label(Some(app::SPEED_TIERS[selected])), cx).tab_stop(false);
    let send = app::send_button("news-speed-send", false, tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field).child(app::composer_control_row([], [pill.into_any_element()], send));
    div().id("news-speed-choice").w(width).overflow_hidden().p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(div().flex().justify_end().child(div().w_full().max_w(px(340.)).child(menu)))
        .child(composer)
        .into_any_element()
}


pub(super) fn render_kimi(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let selected = if cx.reduce_motion() { 2 } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let labels = [tr("kimi_mode_manual"), tr("kimi_mode_yolo"), tr("kimi_mode_auto")];
    let rows = labels.iter().enumerate().map(|(index, label)| {
        let content = div().flex_1().min_w_0().truncate().text_sm().text_color(theme::text()).child(label.clone());
        app::control_option(format!("news-kimi-{index}").into(), content.into_any_element(), index == selected, false, false).tab_stop(false)
    });
    let menu = chrome::popover(div().p(px(10.)).flex().flex_col().gap(px(8.))
        .child(div().text_sm().child(tr("ctl_mode_title")))
        .children(rows).into_any_element(), false);
    let field = div().text_size(px(13.5)).min_h(px(22.)).flex().items_center().overflow_hidden()
        .child(div().truncate().text_color(cx.theme().muted_foreground).child(tr("composer").replace("{agent}", "Kimi")));
    let pill = app::menu_pill("news-kimi-mode", labels[selected].clone(), false, cx).tab_stop(false);
    let send = app::send_button("news-kimi-send", false, tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field).child(app::composer_control_row([], [pill.into_any_element()], send));
    div().id("news-kimi-permissions").relative().w(width).overflow_hidden().p(px(14.)).rounded(px(9.))
        .border_1().border_color(theme::border().alpha(0.06)).bg(theme::chrome())
        .flex().flex_col().gap(px(10.)).child(menu).child(composer)
        .child(div().absolute().top_0().left_0().size_full().occlude()).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn normal_then_fast_then_ultra_fast() {
        assert_eq!([frame(0.5), frame(2.), frame(4.)], [0, 1, 2]);
        assert!((until_next_step(0.5) - 1.3).abs() < 1e-4);
        assert!((until_next_step(4.) - 2.).abs() < 1e-4);
    }
}
