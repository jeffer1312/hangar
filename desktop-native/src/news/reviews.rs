//! `revisoes-pr-mr`: a revisão na lista termina o pipeline e a thread aberta aparece com "Mandar para a sessão".

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 7.;
/// O pipeline termina.
const PASSED: f32 = 2.2;
/// A thread aberta aparece com os botões.
const THREAD: f32 = 4.;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { passed: bool, thread: bool }

fn frame(t: f32) -> Frame { Frame { passed: t >= PASSED, thread: t >= THREAD } }

fn until_next_step(t: f32) -> f32 { [PASSED, THREAD, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: o quadro com o pipeline verde e a thread que se manda para a sessão.
    let Frame { passed, thread } = if cx.reduce_motion() { frame(THREAD) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let item = app::ReviewSummary {
        provider: "gitlab".into(), host: "gitlab.com".into(), repo: "acme/billing-api".into(), number: 214, state: "open".into(),
        title: tr("news_review_sample_title"), author: "ana".into(), mine: true, source: "feat/export-batch".into(), target: "main".into(),
        pipeline: Some(if passed { "success" } else { "running" }.into()), open_threads: Some(1), conflicts: Some(false),
        sessions: vec!["billing-export".into()], ..Default::default()
    };
    let at = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let open = app::ReviewThread {
        id: "t1".into(), file: Some("src/export/batch.ts".into()), line: Some(48), resolved: false,
        code: " for (const invoice of invoices)\n-  await storage.upload(invoice);\n+  await limit(() => storage.upload(invoice));".into(),
        comments: vec![app::ReviewComment { author: "coderabbitai".into(), body: tr("news_review_sample_comment"), at, bot: true, ..Default::default() }],
    };
    let (send, reply, resolve) = app::review_thread_buttons("news-review-thread", tr("review_send"));
    // Altura do quadro mais alto (com a thread): o cartão não pula entre os passos.
    div().id("news-reviews").w(width).min_h(px(300.)).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(app::review_row_view(&item).bg(theme::hover()))
        .when(thread, |el| el.child(app::review_thread_box("news-review-thread", &open, cx)
            .child(app::review_thread_actions().child(send.tab_stop(false)).child(reply.tab_stop(false)).child(resolve.tab_stop(false)))))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_pipeline_passes_and_then_the_thread_opens() {
        assert_eq!(frame(0.5), Frame { passed: false, thread: false });
        assert_eq!(frame(3.), Frame { passed: true, thread: false });
        assert_eq!(frame(5.), Frame { passed: true, thread: true });
    }
}
