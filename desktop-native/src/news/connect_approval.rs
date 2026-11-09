//! `hangar-connect`: o celular pede para entrar pela conta, o cartão de aprovação aparece no computador, a pessoa
//! aprova e o celular passa a abrir a máquina de qualquer rede. Mesmo roteiro do `cofre-de-senhas`.

use std::time::Instant;
use gpui_kit::{component::{Disableable, Sizable, button::*}, *};
use super::password_vault::{CYCLE, SLIDE_STEPS, frame, schedule_repaint};
use crate::{app, i18n::tr_shared, theme};

/// Abaixo desta largura (o cartão deitado do "Atualizar?") fica só a frase do pedido.
const COMPACT_BELOW: f32 = 240.;
/// Alturas fixas do palco: nada pula quando o cartão troca pelo resultado.
const STAGE_HEIGHT: f32 = 150.;
const COMPACT_HEIGHT: f32 = 40.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no cartão de aprovação, antes do clique.
    let (card, pressed, resolved) = if cx.reduce_motion() { (Some(SLIDE_STEPS), false, false) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let (device, machine) = (tr_shared("news_connect_sample_device", &[]), tr_shared("news_connect_sample_machine", &[]));
    let line = |text: String| div().text_xs().text_color(theme::text()).whitespace_normal().child(text);
    let shown = card.map(|step| {
        let content = if compact {
            line(tr_shared("connect_device_pending", &[("machine", &machine)])).into_any_element()
        } else {
            // Aprovar apertado = o cartão enviando a resposta.
            approval_card(&device, &machine, pressed).into_any_element()
        };
        div().relative().top(px(((SLIDE_STEPS - step) * 4) as f32)).opacity(0.25 + 0.25 * step as f32).child(content)
    });
    let result = resolved.then(|| line(tr_shared("connect_news_result", &[("device", &device), ("machine", &machine)])));
    let stage = div().h(px(if compact { COMPACT_HEIGHT } else { STAGE_HEIGHT })).flex().flex_col().justify_center()
        .children(shown).children(result);
    div().id("news-connect-approval").w(width).p(px(if compact { 8. } else { 14. })).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .text_color(theme::text())
        .child(stage.overflow_hidden())
        .into_any_element()
}

/// O cartão de aprovação por máquina como era nesta versão; a miniatura não entra no Tab.
fn approval_card(name: &str, machine: &str, sending: bool) -> Div {
    let button = |action: &str, key: &str| Button::new(SharedString::from(format!("news-connect-{action}"))).xsmall().w_full()
        .label(tr_shared(key, &[])).disabled(sending).tab_stop(false);
    div().p_2().rounded(px(8.)).bg(theme::inset()).flex().flex_col().gap_1()
        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning()).whitespace_normal()
            .child(tr_shared("connect_device_pending", &[("machine", machine)])))
        .child(div().text_sm().text_color(theme::text()).whitespace_normal().child(name.to_owned()))
        .child(app::request_pair(button("approve", "connect_device_approve").primary(), button("deny", "connect_device_deny").outline()))
}
