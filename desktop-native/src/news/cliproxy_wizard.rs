//! `cliproxy-assistente`: o assistente do CLIProxyAPI instala item a item, a conta ChatGPT entra e o motor ganha nome e Fast.

use std::time::Instant;
use gpui_kit::{component::switch::Switch, *};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.;
/// Quando cada item da instalação fica pronto.
const ITEMS: [f32; 4] = [0.6, 1.2, 1.8, 2.4];
const CONNECT: f32 = 3.;
const NAME: f32 = 4.8;
/// Abaixo desta largura (o cartão deitado do "Atualizar?") só a prévia do nome cabe legível.
const COMPACT_BELOW: f32 = 240.;
const STAGE_HEIGHT: f32 = 360.;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { step: usize, done: usize }

fn frame(t: f32) -> Frame {
    let step = if t >= NAME { 2 } else if t >= CONNECT { 1 } else { 0 };
    Frame { step, done: ITEMS.iter().filter(|s| t >= **s).count() }
}

fn until_next_step(t: f32) -> f32 { ITEMS.into_iter().chain([CONNECT, NAME, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: o passo final, com o nome que aparece na sessão e o Fast disponível.
    let Frame { step, done } = if cx.reduce_motion() { frame(NAME) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let email = tr_shared("news_cliproxy_sample_email", &[]);
    let name = app::cliproxy_default_name(app::CliproxyBrand::ChatGpt, &email);
    let stage = div().id("news-cliproxy-wizard").w(width).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .text_color(theme::text()).overflow_hidden();
    if width < px(COMPACT_BELOW) {
        return stage.p(px(8.)).child(app::cliproxy_name_preview(&name)).into_any_element();
    }
    let lead = |key: &str| div().text_size(px(14.)).whitespace_normal().child(tr(key));
    let body = match step {
        0 => div().flex().flex_col().gap(px(9.)).child(lead("cliproxy_wizard_installing"))
            .children((0..app::CLIPROXY_INSTALL_ITEMS).map(|index| app::cliproxy_install_item(index, match index {
                i if i < done => app::CliproxyItem::Done,
                i if i == done => app::CliproxyItem::Running,
                _ => app::CliproxyItem::Waiting,
            }))),
        1 => div().flex().flex_col().gap(px(12.)).child(lead("cliproxy_wizard_connected_title"))
            .child(app::cliproxy_account_row(app::CliproxyBrand::ChatGpt, &email)),
        _ => div().flex().flex_col().gap(px(12.))
            .child(app::cliproxy_account_row(app::CliproxyBrand::ChatGpt, &email))
            .child(app::cliproxy_name_preview(&name))
            .child(app::cliproxy_fast_field(Switch::new("news-cliproxy-fast").checked(true).tab_stop(false))),
    };
    stage.h(px(STAGE_HEIGHT)).p(px(14.)).flex().flex_col().gap(px(14.))
        .child(app::cliproxy_steps("news-cliproxy-steps", step, false))
        .child(body)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn install_then_account_then_name() {
        assert_eq!(frame(0.3), Frame { step: 0, done: 0 });
        assert_eq!(frame(1.9), Frame { step: 0, done: 3 });
        assert_eq!(frame(3.5), Frame { step: 1, done: 4 });
        assert_eq!(frame(5.), Frame { step: 2, done: 4 });
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.3), 0.3));
        assert!(close(until_next_step(2.5), 0.5));
        assert!(close(until_next_step(5.), 2.));
    }
}
