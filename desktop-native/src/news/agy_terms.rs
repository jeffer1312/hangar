//! `termos-antigravity`: o Aceitar os Termos do Antigravity abre os Termos do Google num painel da página de contas, com
//! os links e as duas escolhas; o Hangar só aperta a opção tocada e confirma no agy.

use std::time::Instant;
use gpui_kit::{base::Selectable, prelude::FluentBuilder, *};
use crate::{app::{self, AgyTermsAction, AgyTermsStage}, i18n::tr, theme};

const CYCLE: f32 = 7.;
/// O agy chega à tela dos Termos.
const TERMS: f32 = 1.2;
/// A pessoa toca em "Aceitar sem compartilhar": o botão acende.
const TAP: f32 = 3.6;
/// A resposta foi ao servidor, que aperta a opção no agy.
const ANSWER: f32 = 4.2;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step { Starting, Terms, Tap, Answering }

fn frame(t: f32) -> Step {
    if t >= ANSWER { Step::Answering } else if t >= TAP { Step::Tap } else if t >= TERMS { Step::Terms } else { Step::Starting }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [TERMS, TAP, ANSWER, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") o texto dos Termos não cabe: ficam as duas escolhas.
const COMPACT_BELOW: f32 = 360.;
/// Altura da etapa mais alta, a dos Termos: o cartão não pula entre as etapas.
const HEIGHT: f32 = 500.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado nos Termos com as duas escolhas, que é o que a novidade traz.
    let step = if cx.reduce_motion() { Step::Terms } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let stage = match step {
        Step::Starting => AgyTermsStage::Starting,
        Step::Terms | Step::Tap => AgyTermsStage::Terms(tr("news_agy_terms_sample")),
        Step::Answering => AgyTermsStage::Answering,
    };
    let lit = step == Step::Tap;
    let buttons = app::agy_terms_buttons(stage.actions(), false, true, |action, button|
        button.tab_stop(false).selected(lit && action == AgyTermsAction::Accept(2)));
    let compact = width < px(COMPACT_BELOW);
    let body = if compact {
        div().flex().flex_col().items_start().gap(px(6.)).children(buttons.into_iter().filter(|_| matches!(stage, AgyTermsStage::Terms(_))))
    } else {
        app::agy_terms_panel(&stage, None, &tr("news_login_browser_laptop"), buttons)
    };
    // Miniatura só leitura: a tampa por cima não deixa o mouse levar o foco para os botões nem abrir os links.
    div().id("news-agy-terms").relative().w(width).when(!compact, |el| el.h(px(HEIGHT))).p(px(if compact { 8. } else { 14. }))
        .rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(body).child(div().absolute().top_0().left_0().size_full().occlude()).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_opens_the_terms_lights_the_choice_then_confirms() {
        assert_eq!(frame(0.5), Step::Starting);
        assert_eq!(frame(2.), Step::Terms);
        assert_eq!(frame(3.8), Step::Tap);
        assert_eq!(frame(5.), Step::Answering);
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 1.));
        assert!(close(until_next_step(3.7), 0.5));
        assert!(close(until_next_step(5.), 2.));
    }
}
