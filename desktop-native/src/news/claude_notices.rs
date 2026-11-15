//! `avisos-do-claude`: o hook que falha vira uma linha com o total, que abre o Ignorar; o erro da API fica na caixa âmbar.

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.;
/// O hook falha de novo e o total sobe.
const AGAIN: f32 = 1.5;
/// A linha abre com o detalhe e o Ignorar.
const OPEN: f32 = 3.;
/// Ignorado: fica o Desfazer.
const IGNORED: f32 = 5.;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { count: u32, open: bool, ignored: bool }

fn frame(t: f32) -> Frame { Frame { count: if t >= AGAIN { 4 } else { 3 }, open: (OPEN..IGNORED).contains(&t), ignored: t >= IGNORED } }

fn until_next_step(t: f32) -> f32 { [AGAIN, OPEN, IGNORED, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: a linha aberta, que mostra o detalhe e o Ignorar.
    let Frame { count, open, ignored } = if cx.reduce_motion() { frame(OPEN) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let title = tr_shared("notice_hook_failed_line", &[("name", "PostToolUse:Bash")]);
    let count = tr_shared("notice_count", &[("count", &count.to_string())]);
    let line = if ignored {
        app::notice_rule_line(app::ignored_text().child(app::undo_button("news-notice-undo".into()).tab_stop(false)))
    } else {
        app::notice_rule_line(app::notice_head("news-notice-head".into(), title, Some(count), Some(open)))
    };
    // Altura do quadro mais alto (aberto): o cartão não pula entre os passos.
    div().id("news-claude-notices").w(width).min_h(px(250.)).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(line)
        .when(open, |el| el.child(app::notice_body(Some(tr("news_notice_sample_detail")))
            .child(app::ignore_button("news-notice-ignore".into()).tab_stop(false))))
        .child(app::api_error_box(tr("news_notice_sample_api_error")))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_total_rises_then_the_line_opens_and_then_it_is_ignored() {
        assert_eq!(frame(0.5), Frame { count: 3, open: false, ignored: false });
        assert_eq!(frame(2.), Frame { count: 4, open: false, ignored: false });
        assert_eq!(frame(4.), Frame { count: 4, open: true, ignored: false });
        assert_eq!(frame(6.), Frame { count: 4, open: false, ignored: true });
    }
}
