//! `compactar-parada`: a conversa parada com o cache quase vencendo é compactada, e o prazo do cache recomeça.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{base::TextView, component::ActiveTheme, prelude::FluentBuilder, *};
use crate::{app::{self, chrome}, chat::LastCache, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.5;
/// O chip desce de "5min" para "4min".
const TICK: f32 = 1.2;
const COMPACTING: f32 = 2.4;
const DONE: f32 = 4.2;
/// Movimento reduzido: o aviso de compactado com o prazo de volta a 1 hora.
const STILL: f32 = 5.;
const TTL: u64 = 3600;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage { Idle, Compacting, Done }

/// Quadro do roteiro no instante `t`: segundos que restam no cache e a etapa da conversa.
fn frame(t: f32) -> (u64, Stage) {
    if t >= DONE { (TTL, Stage::Done) }
    else if t >= COMPACTING { (240, Stage::Compacting) }
    else if t >= TICK { (240, Stage::Idle) }
    else { (300, Stage::Idle) }
}

/// Segundos até o próximo degrau: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [TICK, COMPACTING, DONE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

/// Abaixo desta largura (o cartão deitado do "Atualizar?") o compositor não cabe legível: ficam a linha e o chip.
const COMPACT_BELOW: f32 = 240.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = if cx.reduce_motion() { STILL } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        t
    };
    let (rest, stage) = frame(t);
    // O chip real num relógio de mentira: a âncora fica no zero e o "agora" anda até sobrar `rest`.
    let chip = app::cache_chip_at(LastCache { ts: 0., ttl: TTL }, (TTL - rest) as f64);
    // A conversa por cima do compositor: a linha de trabalho enquanto compacta, o aviso depois.
    let tail = match stage {
        Stage::Idle => None,
        Stage::Compacting => Some(app::working_line(chrome::WorkingMark::new("news-compact-mark", 14., theme::accent()).into_any_element(),
            tr_shared("news_compact_idle_working", &[])).into_any_element()),
        Stage::Done => Some(app::conversation_text(div().flex().flex_col().gap_2(), false)
            .child(app::message_label(tr("notice"), false))
            .child(TextView::markdown("news-compact-notice", tr("compacted")).scrollable(false).style(theme::conversation_markdown(cx)))
            .into_any_element()),
    };
    let stage_box = div().id("news-compact-idle").relative().w_full().rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .overflow_hidden().flex().flex_col();
    if width < px(COMPACT_BELOW) {
        return stage_box.h(px(74.)).p(px(8.)).bg(theme::inset())
            .child(div().flex_1().min_h_0().overflow_hidden().children(tail))
            .child(div().flex().justify_end().child(chip)).into_any_element();
    }
    let field = div().text_size(px(13.5)).min_h(px(22.)).flex().items_center().overflow_hidden()
        .child(div().truncate().text_color(cx.theme().muted_foreground).child(tr("composer").replace("{agent}", "Claude")));
    let [commands, attach, recent] = app::composer_icons(cx);
    let left = [commands, attach, recent, app::composer_mic(tr("dictation_start"), cx)].map(|button| button.w(px(26.)).h(px(26.)))
        .map(|button| button.tab_stop(false).into_any_element());
    let right = [app::model_pill("ctl-model", "Opus 5.5".into(), Some(tr_shared("reasoning_level_high", &[])), None, cx)
        .tab_stop(false).into_any_element()];
    let send = app::send_button("send", false, tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field).child(app::composer_control_row(left, right, send));
    // A faixa de baixo do compositor, só com o chip do cache no canto, onde ele mora na conversa.
    let strip = div().pt(px(7.)).px(px(6.)).flex().items_center().child(div().flex_1()).child(chip);
    // Altura fixa no maior quadro: o cartão da novidade não pula quando a linha troca pelo aviso.
    stage_box.p(px(14.)).gap(px(10.))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(div().h(px(64.)).flex().flex_col().justify_end().when_some(tail, |el, tail| el.child(tail)))
        .child(div().flex().flex_col().child(composer).child(strip)).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_counts_down_compacts_and_restarts_the_cache() {
        let label = |t: f32| crate::chat::cache_left(LastCache { ts: 0., ttl: TTL }, (TTL - frame(t).0) as f64);
        assert_eq!(label(0.5), (300, true, Some("5min".into())));
        assert_eq!(label(1.5), (240, true, Some("4min".into())));
        assert_eq!(frame(3.).1, Stage::Compacting);
        assert_eq!(frame(STILL), (TTL, Stage::Done));
        assert_eq!(label(STILL), (3600, false, Some("1h00".into())));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 1.));
        assert!(close(until_next_step(3.), 1.2));
        assert!(close(until_next_step(5.), 1.5));
    }
}
