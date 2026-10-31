//! `sugestao-de-nome`: a sessão com o nome que o Hangar tirou da pasta ganha, depois da primeira mensagem, a faixa
//! "Renomear para X?" de um clique.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app::{self, chrome}, i18n::tr_shared, theme};

const CYCLE: f32 = 6.;
/// A primeira mensagem entra.
const BUBBLE: f32 = 0.8;
/// A faixa entra deslizando em `SLIDE_STEPS` degraus de `SLIDE_STEP`.
const STRIP: f32 = 1.8;
const SLIDE_STEP: f32 = 0.05;
const SLIDE_STEPS: u8 = 3;
/// O Renomear é apertado: o botão fica carregando, como na tela enquanto o servidor renomeia.
const PRESS: f32 = 3.4;
/// O nome novo chega na aba e a faixa some.
const RENAMED: f32 = 4.2;

/// Quadro do roteiro no instante `t` do ciclo: a bolha, o degrau da faixa (0 entrando, `SLIDE_STEPS` parada), o
/// Renomear apertado e se a aba já tem o nome novo.
fn frame(t: f32) -> (bool, Option<u8>, bool, bool) {
    let strip = (t >= STRIP && t < RENAMED).then(|| (((t - STRIP) / SLIDE_STEP).floor() as u8).min(SLIDE_STEPS));
    (t >= BUBBLE, strip, t >= PRESS && t < RENAMED, t >= RENAMED)
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    let slide = (1..=SLIDE_STEPS).map(|k| STRIP + k as f32 * SLIDE_STEP);
    slide.chain([BUBBLE, STRIP, PRESS, RENAMED, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

/// Abaixo desta largura (o cartão deitado do "Atualizar?") a faixa não cabe legível: fica só a aba trocando de nome.
const COMPACT_BELOW: f32 = 240.;
/// Alturas fixas da conversa e da faixa: a bolha não pula quando a faixa entra.
const CONVERSATION_HEIGHT: f32 = 76.;
const STRIP_HEIGHT: f32 = 44.;

pub(super) fn render(started: Instant, width: Pixels, animate: bool, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado com a faixa à vista, antes do clique.
    let (bubble, strip, renaming, renamed) = if cx.reduce_motion() { (true, Some(SLIDE_STEPS), false, false) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        if animate { schedule_repaint(t, window, cx); }
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let pad = if compact { 8. } else { 14. };
    let suggestion = tr_shared("news_rename_sample_name", &[]);
    let name = if renamed { suggestion.clone() } else { tr_shared("news_rename_sample_generic", &[]) };
    let avatar = chrome::session_avatar(&chrome::AvatarLook::new("claude", None, "idle", false), 20., None);
    let tab = app::tab_frame(true, false, false).when(compact, |el| el.w(px(f32::from(width) - 2. * pad)))
        .child(avatar).child(app::tab_title(name, None, false, false, None, None));
    let stage = div().id("news-rename-suggestion").w(width).p(px(pad)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(12.)).text_color(theme::text()).child(tab);
    if compact { return stage.into_any_element(); }
    let message = app::user_bubble(app::conversation_text(div().flex().flex_col().gap_2(), true).child(tr_shared("news_rename_sample_message", &[])));
    // Miniatura só leitura: os botões da faixa ficam sem clique e fora do Tab.
    let strip = strip.map(|step| div().relative().top(px(((SLIDE_STEPS - step) * 4) as f32)).opacity(0.25 + 0.25 * step as f32)
        .child(app::rename_strip(&suggestion, app::RenameStrip { renaming, ..Default::default() },
            |button| button.tab_stop(false), |button| button.tab_stop(false), |button| button.tab_stop(false))));
    stage.child(div().h(px(CONVERSATION_HEIGHT)).overflow_hidden().flex().flex_col().justify_end()
            .when(bubble, |el| el.child(div().flex().justify_end().child(message))))
        .child(div().h(px(STRIP_HEIGHT)).overflow_hidden().children(strip))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_shows_the_message_then_the_strip_then_the_new_name() {
        assert_eq!(frame(0.5), (false, None, false, false));
        assert_eq!(frame(1.0), (true, None, false, false));
        assert_eq!(frame(1.82), (true, Some(0), false, false));
        assert_eq!(frame(1.87), (true, Some(1), false, false));
        assert_eq!(frame(2.5), (true, Some(SLIDE_STEPS), false, false));
        assert_eq!(frame(3.6), (true, Some(SLIDE_STEPS), true, false));
        assert_eq!(frame(4.5), (true, None, false, true));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.6));
        assert!(close(until_next_step(1.0), 0.8));
        assert!(close(until_next_step(1.82), 0.03));
        assert!(close(until_next_step(2.5), 0.9));
        assert!(close(until_next_step(3.6), 0.6));
        assert!(close(until_next_step(4.5), 1.5));
    }
}
