//! `atualizacoes-e-manual`: a lista de Atualizações por dia, a linha nova é aberta e vira a página da novidade.

use std::time::Instant;
use gpui_kit::{assets::IconName, component::{Sizable, button::*}, prelude::FluentBuilder, *};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.;
/// A linha da novidade nova fica destacada, como sob o ponteiro.
const PRESS: f32 = 1.8;
/// A página da novidade substitui a lista.
const PAGE: f32 = 2.6;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { pressed: bool, page: bool }

fn frame(t: f32) -> Frame { Frame { pressed: t >= PRESS, page: t >= PAGE } }

fn until_next_step(t: f32) -> f32 { [PRESS, PAGE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

/// A linha como a de `updates_page`: bolinha de não vista, título, selo do tipo e, se abre, o "Ver ›".
fn row(id: &'static str, kind: &str, title: String, unread: bool, opens: bool, pressed: bool, cx: &App) -> Div {
    let title = div().flex().flex_wrap().items_center().gap_x(px(8.)).gap_y(px(2.))
        .when(unread, |el| el.child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(theme::accent())))
        .child(div().min_w_0().whitespace_normal().child(title))
        .children(app::news_kind_pill(kind, cx));
    let control = if opens { Button::new(id).ghost().small().label(tr("updates_view")).tab_stop(false).into_any_element() } else { div().into_any_element() };
    app::setting_row(false, app::news_kind_icon(kind), title, div(), true, control).when(pressed, |el| el.bg(theme::hover()))
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: a lista por dia, com a novidade ainda não vista.
    let Frame { pressed, page } = if cx.reduce_motion() { frame(0.) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let t = |key| tr_shared(key, &[]);
    let content = if page {
        div().flex().flex_col()
            .child(div().flex().ml(px(-8.)).child(Button::new("news-guide-back").ghost().small().icon(IconName::ArrowLeft)
                .label(tr("settings_page_updates")).tab_stop(false)))
            .child(div().mt(px(12.)).text_size(px(13.)).text_color(theme::muted()).child(t("news_guide_sample_date")))
            .child(div().mt(px(4.)).flex().flex_wrap().items_center().gap_2()
                .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(t("news_guide_sample_new")))
                .children(app::news_kind_pill("new", cx)))
            .child(app::news_markdown_block("news-guide-text".into(), t("news_guide_sample_text")).mt(px(14.)))
            .child(div().mt(px(14.)).flex().child(Button::new("news-guide-manual").outline().small().icon(IconName::BookOpen)
                .label(tr("updates_read_manual")).tab_stop(false)))
    } else {
        div().flex().flex_col()
            .child(app::news_group_heading(t("news_guide_sample_day"), Some(t("news_guide_sample_weekday"))).mt_0())
            .child(app::settings_box()
                .child(row("news-guide-view-new", "new", t("news_guide_sample_new"), true, true, pressed, cx))
                .child(row("news-guide-view-improvement", "improvement", t("news_guide_sample_improvement"), false, false, false, cx))
                .child(row("news-guide-view-fix", "fix", t("news_guide_sample_fix"), false, false, false, cx)))
    };
    // Altura do quadro mais alto (a página): o cartão não pula entre os passos.
    div().id("news-updates-manual").relative().w(width).min_h(px(300.)).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border())
        .bg(theme::inset()).child(content)
        .child(div().absolute().top_0().left_0().size_full().block_mouse_except_scroll())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_row_is_pressed_and_then_the_page_opens() {
        assert_eq!(frame(0.5), Frame { pressed: false, page: false });
        assert_eq!(frame(2.), Frame { pressed: true, page: false });
        assert_eq!(frame(4.), Frame { pressed: true, page: true });
        assert!((until_next_step(2.) - 0.6).abs() < 1e-4);
    }
}
