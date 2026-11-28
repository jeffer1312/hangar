//! `kimi-permissions`: a pílula de modo do Kimi passa pelas três permissões, no mesmo passo da novidade de velocidade.

use std::time::Instant;
use gpui_kit::{component::ActiveTheme, *};
use crate::{app::{self, chrome}, i18n::tr, theme};
use super::speed_choice::{CYCLE, frame, until_next_step};

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
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
