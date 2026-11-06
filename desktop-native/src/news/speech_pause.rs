//! `pausar-fala`: o botão para a fala da assistente, e a retomada continua do ponto em que ela parou.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{component::ActiveTheme, *};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.;
const PAUSE: f32 = 2.5;
const RESUME: f32 = 5.;
/// As barras andam em amostras, como o nível do áudio real, e não a cada quadro.
const BAR_STEP: f32 = 0.08;
/// Quadro do movimento reduzido: a fala pausada, que é o que explica o recurso.
const STILL: f32 = 3.5;

fn paused(t: f32) -> bool { (PAUSE..RESUME).contains(&t) }

/// Voz de mentira: sílabas rápidas dentro de frases lentas, sempre igual para o laço não saltar.
fn level(t: f32) -> f32 {
    let sample = (t / BAR_STEP).floor();
    ((0.55 + 0.45 * (sample * 1.7).sin()) * (sample * 0.23).sin().abs()).max(0.35)
}

/// Segundos até o quadro mudar: pausada, só a retomada; falando, a próxima amostra.
fn until_next_step(t: f32) -> f32 {
    if paused(t) { return RESUME - t; }
    (((t / BAR_STEP).floor() + 1.) * BAR_STEP - t).max(0.001)
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
    let stopped = paused(t);
    let color = if stopped { theme::muted() } else { theme::success() };
    let status = if stopped { tr_shared("codex_voice_paused", &[]) } else { tr("voice_assistant_speaking") };
    let bars = app::voice_equalizer(if stopped { 0. } else { level(t) }, (t / BAR_STEP) as u64, 3., 18.)
        .map(|height| div().w(px(3.)).h(px(height)).rounded_full().bg(if stopped { theme::faint() } else { color }));
    let equalizer = div().h(px(18.)).flex().gap(px(2.)).items_center().children(bars);
    div().id("news-speech-pause").w(width).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06)).bg(theme::inset()).overflow_hidden()
        .child(app::voice_dock_card(equalizer, status, color, Some("00:42".to_owned()), None))
        .child(div().flex().justify_end().px(px(12.)).pb(px(10.)).child(app::speech_pause_button(stopped).tab_stop(false)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_speaks_pauses_and_goes_back() {
        assert!(!paused(1.) && paused(PAUSE) && paused(4.9) && !paused(RESUME) && !paused(CYCLE - 0.1));
        assert!(paused(STILL), "o quadro parado mostra a pausa");
    }

    #[test]
    fn paused_frames_wait_for_the_resume_and_speech_steps_by_sample() {
        assert!((until_next_step(3.) - 2.).abs() < 1e-3);
        assert!(until_next_step(1.) <= BAR_STEP);
    }
}
