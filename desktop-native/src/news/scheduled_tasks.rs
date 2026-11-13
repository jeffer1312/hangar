//! A mesma linha da configuração recebe o pedido e abre uma conversa nova.
use std::time::Instant;

use gpui_kit::{component::switch::Switch, *};

use crate::{app, i18n::tr_shared, theme};

pub(super) fn render(
    started: Instant,
    width: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let elapsed = started.elapsed().as_secs_f32() % 6.;
    let sent = cx.reduce_motion() || elapsed >= 2.5;
    if !cx.reduce_motion() {
        super::repaint_in(
            if elapsed < 2.5 {
                2.5 - elapsed
            } else {
                6. - elapsed
            },
            window,
            cx,
        );
    }
    let t = |key| tr_shared(key, &[]);
    let schedule = tr_shared(
        "scheduled_schedule_weekly",
        &[("days", &t("scheduled_weekdays_only")), ("time", "09:00")],
    );
    let row = app::scheduled_row_frame("news-scheduled-row")
        .child(div().flex_1().min_w_0().child(app::scheduled_row_text(
            t("scheduled_news_name"),
            t("scheduled_news_prompt"),
            schedule,
            t("scheduled_news_next"),
            t(if sent {
                "scheduled_status_sent"
            } else {
                "scheduled_status_never"
            }),
            true,
        )))
        .child(
            Switch::new("news-scheduled-enabled")
                .checked(true)
                .tab_stop(false),
        );
    div()
        .id("news-scheduled-tasks")
        .relative()
        .w(width)
        .min_h(px(245.))
        .p_4()
        .rounded(px(9.))
        .border_1()
        .border_color(theme::border())
        .bg(theme::inset())
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::text())
                .child(t("scheduled_title")),
        )
        .child(row)
        .child(
            div()
                .text_xs()
                .text_color(theme::accent_text())
                .opacity(if sent { 1. } else { 0. })
                .child(t("scheduled_news_session")),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .block_mouse_except_scroll(),
        )
        .into_any_element()
}
