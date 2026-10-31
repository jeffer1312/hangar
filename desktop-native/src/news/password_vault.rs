//! `cofre-de-senhas`: o agente pede para usar uma senha do cofre, a pessoa permite no cartão, e o que volta para a
//! conversa traz `••••` no lugar do valor.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{component::button::*, prelude::FluentBuilder, *};
use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 6.;
/// O cartão entra deslizando em `SLIDE_STEPS` degraus de `SLIDE_STEP`.
const CARD: f32 = 0.6;
const SLIDE_STEP: f32 = 0.05;
const SLIDE_STEPS: u8 = 3;
/// O Permitir é apertado e fica em destaque.
const PRESS: f32 = 2.8;
/// O cartão some e o resultado aparece.
const RESOLVED: f32 = 3.6;

/// Quadro do roteiro no instante `t` do ciclo: o degrau do cartão (0 entrando, `SLIDE_STEPS` parado), o Permitir em
/// destaque e se o resultado já apareceu.
fn frame(t: f32) -> (Option<u8>, bool, bool) {
    let card = (t >= CARD && t < RESOLVED).then(|| (((t - CARD) / SLIDE_STEP).floor() as u8).min(SLIDE_STEPS));
    (card, t >= PRESS && t < RESOLVED, t >= RESOLVED)
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    let slide = (1..=SLIDE_STEPS).map(|k| CARD + k as f32 * SLIDE_STEP);
    slide.chain([CARD, PRESS, RESOLVED, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

thread_local! {
    /// O redesenho já marcado: cada render só marca outro se o dele cair antes.
    static NEXT_REPAINT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Redesenha a janela no próximo degrau. Só é marcado enquanto a animação é desenhada: fechou o modal, para.
fn schedule_repaint(t: f32, window: &mut Window, cx: &mut App) {
    // Margem para o timer não acordar um pouco antes do degrau e desenhar o quadro velho.
    let at = Instant::now() + Duration::from_secs_f32(until_next_step(t)) + Duration::from_millis(5);
    if NEXT_REPAINT.get().is_some_and(|next| next > Instant::now() && next <= at) { return; }
    NEXT_REPAINT.set(Some(at));
    window.spawn(cx, async move |cx| {
        cx.background_executor().timer(at.saturating_duration_since(Instant::now())).await;
        let _ = cx.update(|window, _| window.refresh());
    }).detach();
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") o cartão inteiro não cabe legível: ficam o título e os botões.
const COMPACT_BELOW: f32 = 240.;
/// Alturas fixas do palco: nada pula quando o cartão troca pelo resultado.
const STAGE_HEIGHT: f32 = 250.;
const COMPACT_HEIGHT: f32 = 58.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no cartão de aprovação, antes do clique.
    let (card, pressed, resolved) = if cx.reduce_motion() { (Some(SLIDE_STEPS), false, false) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let pad = if compact { 8. } else { 14. };
    let height = if compact { COMPACT_HEIGHT } else { STAGE_HEIGHT };
    // Miniatura só leitura: os botões ficam sem clique e fora do Tab.
    let allow = app::request_button("allow", "news", tr("vault_request_allow"), false).primary().tab_stop(false)
        .when(pressed, |button| button.border_2().border_color(theme::text()));
    let deny = app::request_button("deny", "news", tr("vault_request_deny"), false).outline().tab_stop(false);
    let title = app::approve_title(&tr("vault_news_origin"), &tr("vault_news_entry"));
    let shown = card.map(|step| {
        let content = if compact {
            div().flex().flex_col().gap_1()
                .child(div().truncate().text_xs().text_color(theme::text()).child(title))
                .child(app::request_pair(allow, deny))
                .into_any_element()
        } else {
            let body = app::approve_body(Some("exec"), &tr("vault_news_command"), allow, deny);
            app::request_card("news", &tr("vault_news_machine"), title, &tr("vault_news_reason"), body, None).mx_0().my_0().into_any_element()
        };
        div().relative().top(px(((SLIDE_STEPS - step) * 4) as f32)).opacity(0.25 + 0.25 * step as f32).child(content)
    });
    let result = resolved.then(|| div().id("news-vault-result").flex().flex_col().gap_1()
        .when(!compact, |el| el.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("vault_news_result_caption"))))
        .child(app::request_command(tr("vault_news_result"))));
    div().id("news-password-vault").w(width).p(px(pad)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .text_color(theme::text())
        .child(div().h(px(height)).overflow_hidden().flex().flex_col().justify_center().children(shown).children(result))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_shows_the_card_then_the_press_then_the_masked_result() {
        assert_eq!(frame(0.3), (None, false, false));
        assert_eq!(frame(0.62), (Some(0), false, false));
        assert_eq!(frame(1.5), (Some(SLIDE_STEPS), false, false));
        assert_eq!(frame(3.0), (Some(SLIDE_STEPS), true, false));
        assert_eq!(frame(4.0), (None, false, true));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.4));
        assert!(close(until_next_step(0.62), 0.03));
        assert!(close(until_next_step(1.5), 1.3));
        assert!(close(until_next_step(3.0), 0.6));
        assert!(close(until_next_step(4.0), 2.));
    }
}
