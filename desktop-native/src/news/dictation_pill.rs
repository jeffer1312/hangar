//! `pilula-do-ditado`: Ctrl+Espaço grava, a pílula mostra a voz acima do compositor e o texto cai nele ao parar.

use std::{cell::Cell, collections::VecDeque, time::{Duration, Instant}};
use gpui_kit::{component::ActiveTheme, prelude::FluentBuilder, *};
use crate::{app::{self, dictation}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 8.;
const START: f32 = 1.;
const STOP: f32 = 4.2;
const TEXT: f32 = 5.;
const CLEAR: f32 = 7.7;
/// Quanto a tecla fica acesa depois de apertada.
const LIT: f32 = 0.2;
/// As barras andam em amostras, como na gravação real, e não a cada quadro.
const BAR_STEP: f32 = 0.08;
/// Abaixo desta largura (o cartão deitado do "Atualizar?") a barra do compositor não cabe legível: fica só o campo.
const COMPACT_BELOW: f32 = 240.;
/// Quadro do movimento reduzido: a pílula aberta, que é o que explica o recurso.
const STILL: f32 = 2.;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase { Idle, Recording, Transcribing, Text }

fn phase(t: f32) -> Phase {
    if t < START || t >= CLEAR { Phase::Idle } else if t < STOP { Phase::Recording } else if t < TEXT { Phase::Transcribing } else { Phase::Text }
}

fn key_lit(t: f32) -> bool { [START, STOP].iter().any(|press| t >= *press && t < press + LIT) }

/// Voz de mentira: sílabas rápidas dentro de frases lentas, sempre igual para o laço não saltar.
fn voice(sample: f32) -> f32 { (0.55 + 0.45 * (sample * 1.7).sin()) * (sample * 0.23).sin().abs() }

/// As amostras do instante `t`, a mais nova no fim, como na gravação real.
fn voice_bars(t: f32) -> VecDeque<f32> {
    let newest = (t / BAR_STEP).floor();
    (0..12).rev().map(|age| voice(newest - age as f32)).collect()
}

/// Segundos até o quadro mudar: as barras só pedem redesenho enquanto a pílula está aberta.
fn until_next_step(t: f32) -> f32 {
    let bar = (phase(t) == Phase::Recording).then(|| ((t / BAR_STEP).floor() + 1.) * BAR_STEP);
    [START, START + LIT, STOP, STOP + LIT, TEXT, CLEAR, CYCLE].into_iter().chain(bar).filter(|step| *step > t).fold(CYCLE, f32::min) - t
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

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = if cx.reduce_motion() { STILL } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        t
    };
    let (phase, lit) = (phase(t), key_lit(t));
    let compact = width < px(COMPACT_BELOW);
    let size = if compact { 10.5 } else { 13.5 };
    let caret = div().flex_shrink_0().w(px(1.5)).h(px(size + 2.5)).bg(cx.theme().caret);
    // O campo tem as cores do `Textarea` do compositor: cursor antes do placeholder, depois do texto.
    let field = div().text_size(px(size)).min_h(px(if compact { 16. } else { 22. })).flex().items_center().gap(px(1.)).overflow_hidden()
        .map(|el| if phase == Phase::Text {
            el.child(div().truncate().text_color(theme::text()).child(tr("news_history_sample_3"))).child(caret)
        } else {
            el.child(caret).child(div().truncate().text_color(cx.theme().muted_foreground).child(tr("composer").replace("{agent}", "Claude")))
        });
    let bars = voice_bars(t);
    // Em 160 px a tecla e a pílula não cabem lado a lado: a tecla acende e dá a vez a ela.
    let pill = (phase == Phase::Recording && !(compact && lit)).then(|| div().w(px(dictation::PILL_WIDTH)).h(px(dictation::PILL_HEIGHT))
        .child(dictation::recording_pill(bars.back().copied().unwrap_or(0.), &bars)));
    let pill = pill.map(|pill| div().absolute().top(px(if compact { 6. } else { 11. })).left_0().right_0().flex().justify_center().child(pill));
    let key = div().absolute().font_family(theme::MONO).py(px(1.)).border_1().child(tr("dictation_shortcut"))
        .map(|el| if lit { el.border_color(theme::accent()).bg(theme::accent()).text_color(theme::on_accent()) }
            else { el.border_color(theme::text().alpha(0.22)).bg(theme::raised()).text_color(theme::body_text()) });
    if compact {
        let composer = app::composer_card(false).p(px(4.)).pl(px(8.)).rounded(px(7.)).gap_0().child(field);
        return div().id("news-dictation-pill").relative().w(width).h(px(74.)).rounded(px(8.)).border_1().border_color(theme::border().alpha(0.07))
            .bg(theme::inset()).overflow_hidden()
            .map(|el| match pill {
                Some(pill) => el.child(pill),
                None => el.child(key.top(px(9.)).right(px(9.)).text_size(px(10.5)).px(px(6.)).rounded(px(5.))),
            })
            .child(div().absolute().left(px(8.)).right(px(8.)).bottom(px(8.)).child(composer)).into_any_element();
    }
    let mic = match phase {
        Phase::Recording => dictation::stop_button(),
        Phase::Transcribing => app::composer_mic(tr("dictation_working"), cx).loading(true),
        Phase::Idle | Phase::Text => app::composer_mic(tr("dictation_start"), cx),
    };
    let [commands, attach, recent] = app::composer_icons(cx);
    // Gravando, a pílula do estilo sai, como no compositor de verdade.
    let style = (phase != Phase::Recording).then(|| app::menu_pill("dictation-style", tr("voice_style_limpar"), false, cx));
    let left = [commands, attach, recent, mic].map(|button| button.w(px(26.)).h(px(26.))).into_iter().chain(style)
        .map(|button| button.tab_stop(false).into_any_element());
    let right = [app::model_pill("ctl-model", "Opus 5.5".into(), Some(tr_shared("reasoning_level_high", &[])), None, cx),
        app::menu_pill("ctl-mode", tr("mode_bypassPermissions"), false, cx)].map(|button| button.tab_stop(false).into_any_element());
    let send = app::send_button("send", phase == Phase::Text, tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field).child(app::composer_control_row(left, right, send));
    div().id("news-dictation-pill").relative().w_full().p(px(14.)).pt(px(48.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(key.top(px(13.)).right(px(16.)).text_size(px(11.)).px(px(7.)).rounded(px(6.)))
        .children(pill).child(composer).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_records_then_drops_the_text_and_clears() {
        assert_eq!(phase(0.5), Phase::Idle);
        assert_eq!(phase(STILL), Phase::Recording);
        assert_eq!(phase(STOP), Phase::Transcribing);
        assert_eq!(phase(6.), Phase::Text);
        assert_eq!(phase(CLEAR), Phase::Idle);
        assert!(key_lit(START) && key_lit(STOP) && !key_lit(STILL));
    }

    #[test]
    fn bars_fit_the_pill_and_move() {
        let (before, after) = (voice_bars(2.), voice_bars(2. + BAR_STEP));
        assert_eq!(before.len(), 12);
        assert!(before.iter().all(|level| (0.0..=1.).contains(level)));
        assert_ne!(before, after);
    }

    #[test]
    fn repaint_is_fast_only_while_recording() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 0.5));
        assert!(until_next_step(2.01) <= BAR_STEP);
        assert!(close(until_next_step(6.), CLEAR - 6.));
    }
}
