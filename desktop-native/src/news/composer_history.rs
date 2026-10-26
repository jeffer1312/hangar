//! `historico-compositor`: a seta ↑ com a caixa vazia traz de volta as mensagens já mandadas.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{component::ActiveTheme, prelude::FluentBuilder, *};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.5;
/// Quando cada ↑ é apertado dentro do ciclo; cada um traz a mensagem seguinte.
const PRESSES: [f32; 3] = [1., 3., 5.];
/// Quanto a tecla fica acesa depois de apertada.
const LIT: f32 = 0.2;
/// Depois disto o campo volta a ficar vazio até o ciclo recomeçar.
const CLEAR: f32 = 7.2;

/// Quadro do roteiro no instante `t` do ciclo: qual mensagem está no campo, se a tecla está acesa e se o cursor aparece.
fn frame(t: f32) -> (Option<usize>, bool, bool) {
    let shown = if t >= CLEAR { None } else { PRESSES.iter().rposition(|p| t >= *p) };
    let lit = PRESSES.iter().any(|p| t >= *p && t < p + LIT);
    (shown, lit, (t * 2.) as u32 % 2 == 0)
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    let caret = ((t * 2.).floor() + 1.) / 2.;
    PRESSES.iter().flat_map(|p| [*p, p + LIT]).chain([CLEAR, CYCLE, caret]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

/// Abaixo desta largura (o cartão deitado do "Atualizar?") a barra do compositor não cabe legível: fica só o campo.
const COMPACT_BELOW: f32 = 240.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado na primeira mensagem, com a tecla à vista.
    let (shown, lit, caret) = if cx.reduce_motion() { (Some(0), false, true) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let text = shown.map(|n| tr(&format!("news_history_sample_{}", n + 1)));
    let size = if compact { 10.5 } else { 13.5 };
    let caret = div().flex_shrink_0().w(px(1.5)).h(px(size + 2.5)).when(caret, |el| el.bg(cx.theme().caret));
    // O campo tem as cores do `Textarea` do compositor: cursor antes do placeholder, depois do texto.
    let field = div().text_size(px(size)).min_h(px(if compact { 16. } else { 22. })).flex().items_center().gap(px(1.)).overflow_hidden()
        .map(|el| match text.clone() {
            Some(text) => el.child(div().truncate().text_color(theme::text()).child(text)).child(caret),
            None => el.child(caret).child(div().truncate().text_color(cx.theme().muted_foreground).child(tr("composer").replace("{agent}", "Claude"))),
        });
    let key = div().font_family(theme::MONO).py(px(1.)).border_1().child("↑");
    if compact {
        let key = key.absolute().top(px(9.)).right(px(9.)).text_size(px(10.5)).px(px(6.)).rounded(px(5.))
            .map(|el| if lit { el.mt(px(1.)).border_color(theme::accent().alpha(0.5)).bg(theme::accent().alpha(0.35)).text_color(theme::text()) }
                else { el.border_color(theme::text().alpha(0.18)).text_color(theme::body_text()) });
        let composer = app::composer_card(false).p(px(4.)).pl(px(8.)).rounded(px(7.)).gap_0().child(field);
        return div().id("news-composer-history").relative().w(width).h(px(74.)).rounded(px(8.)).border_1().border_color(theme::border().alpha(0.07))
            .bg(theme::inset()).overflow_hidden()
            .child(key).child(div().absolute().left(px(8.)).right(px(8.)).bottom(px(8.)).child(composer)).into_any_element();
    }
    let [commands, attach, recent] = app::composer_icons(cx);
    let left = [commands, attach, recent, app::composer_mic(tr("dictation_start"), cx)].map(|button| button.w(px(26.)).h(px(26.)))
        .into_iter().chain([app::menu_pill("dictation-style", tr("voice_style_limpar"), false, cx)])
        .map(|button| button.tab_stop(false).into_any_element());
    let right = [app::model_pill("ctl-model", "Opus 5.5".into(), Some(tr_shared("reasoning_level_high", &[])), None, cx),
        app::menu_pill("ctl-mode", tr("mode_bypassPermissions"), false, cx)].map(|button| button.tab_stop(false).into_any_element());
    let send = app::send_button("send", text.is_some(), tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field).child(app::composer_control_row(left, right, send));
    // A tecla só aparece do primeiro ↑ até o campo esvaziar, montada na borda de cima do palco.
    let key = key.absolute().top(px(-9.)).right(px(20.)).text_size(px(11.)).px(px(7.)).rounded(px(6.))
        .map(|el| if lit { el.border_color(theme::accent()).bg(theme::accent()).text_color(theme::on_accent()) }
            else { el.border_color(theme::text().alpha(0.22)).bg(theme::raised()).text_color(theme::text()) });
    div().id("news-composer-history").relative().w_full().p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(composer).when(shown.is_some(), |el| el.child(key)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_walks_the_three_messages_and_clears() {
        assert_eq!(frame(0.5).0, None);
        assert_eq!(frame(1.1), (Some(0), true, frame(1.1).2));
        assert_eq!(frame(2.0).0, Some(0));
        assert!(!frame(2.0).1);
        assert_eq!(frame(3.1).0, Some(1));
        assert_eq!(frame(6.0).0, Some(2));
        assert_eq!(frame(7.3).0, None);
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.3));
        assert!(close(until_next_step(0.9), 0.1));
        assert!(close(until_next_step(1.1), 0.1));
        assert!(close(until_next_step(1.25), 0.25));
        assert!(close(until_next_step(7.3), 0.2));
    }
}
