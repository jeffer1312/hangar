//! `etiquetas-maquina-conta`: a aba mostra na borda de cima a máquina (quando é outra) e a conta, e o anel de contexto
//! em volta do agente.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app::{self, chrome, PlateMark, PlateSegment}, i18n::tr_shared, theme};

const CYCLE: f32 = 6.;
/// Quando a placa de cada aba aparece: a local (só a conta) e depois a de outra máquina.
const PLATE_LOCAL: f32 = 0.6;
const PLATE_REMOTE: f32 = 1.2;
/// As placas saem um pouco antes do fim, para o recomeço do ciclo se ver.
const HIDE: f32 = 5.6;
/// O anel da primeira aba enche em degraus de 5%, de `RING_FROM` a `RING_TO`.
const RING_FROM: f64 = 15.;
const RING_TO: f64 = 70.;
const RING_START: f32 = 1.;
const RING_STEP: f32 = 0.4;
const RING_STEPS: u32 = ((RING_TO - RING_FROM) / 5.) as u32;

/// Quadro do roteiro no instante `t` do ciclo: placa da aba local, placa da aba remota e o contexto da local, em %.
fn frame(t: f32) -> (bool, bool, f64) {
    let steps = if t < RING_START { 0 } else { (((t - RING_START) / RING_STEP).floor() as u32 + 1).min(RING_STEPS) };
    (t >= PLATE_LOCAL && t < HIDE, t >= PLATE_REMOTE && t < HIDE, RING_FROM + 5. * steps as f64)
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    let ring = (0..RING_STEPS).map(|k| RING_START + k as f32 * RING_STEP);
    ring.chain([PLATE_LOCAL, PLATE_REMOTE, HIDE, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

/// Abaixo desta largura (o cartão deitado do "Atualizar?") as duas abas não cabem lado a lado: vão uma sobre a outra.
const COMPACT_BELOW: f32 = 240.;

/// Um pedaço da placa com a etiqueta já escolhida: ícone, cor e nome.
fn segment(mark: PlateMark, color: &str, key: &str) -> PlateSegment {
    PlateSegment { mark, color: theme::tag_color(color).unwrap_or_else(theme::muted), name: tr_shared(key, &[]) }
}

/// A aba como a barra a desenha: caixa, avatar do agente com o anel, nome e a placa montada na borda de cima.
fn tab(name: &str, provider: &str, on: bool, pct: f64, plate: Option<Div>, width: f32) -> Div {
    let avatar = chrome::session_avatar(&chrome::AvatarLook::new(provider, None, "idle", false), 20., None).into_any_element();
    let card = app::tab_frame(on, false, false).w(px(width)).relative()
        .child(app::with_context_ring(avatar, Some(pct), 20.))
        .child(app::tab_title(tr_shared(name, &[]), None, false, false, None, None));
    div().relative().flex_shrink_0().child(card).children(plate)
}

pub(super) fn render(started: Instant, width: Pixels, animate: bool, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: as duas placas à vista e o anel já bem cheio.
    let (local, remote, pct) = if cx.reduce_motion() { (true, true, 60.) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        if animate { schedule_repaint(t, window, cx); }
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let work = segment(app::account_icon("id-card").map_or(PlateMark::Provider("claude".into()), PlateMark::Icon), "blue",
        "news_tags_sample_account_work");
    let personal = segment(app::account_icon("user-round").map_or(PlateMark::Provider("codex".into()), PlateMark::Icon), "pink",
        "news_tags_sample_account_personal");
    let machine = segment(PlateMark::Icon(app::machine_icon("cloud")), "violet", "news_tags_sample_machine");
    let (pad, gap) = if compact { (8., 14.) } else { (14., 10.) };
    let card_width = if compact { f32::from(width) - 2. * pad } else { (f32::from(width) - 2. * pad - gap) / 2. };
    let first = tab("news_tags_sample_session", "claude", true, pct, local.then(|| app::plate(None, Some(work), true)), card_width);
    let second = tab("news_tags_sample_session_other", "codex", false, 35., remote.then(|| app::plate(Some(machine), Some(personal), false)),
        card_width);
    // A folga de cima é a metade da placa que fica fora da aba.
    div().id("news-account-tags").w(width).p(px(pad)).pt(px(pad + 6.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().gap(px(gap)).when(compact, |el| el.flex_col())
        .child(first).child(second).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn plates_appear_and_the_ring_fills() {
        assert_eq!(frame(0.3), (false, false, 15.));
        assert_eq!(frame(0.8), (true, false, 15.));
        assert_eq!(frame(1.1), (true, false, 20.));
        assert_eq!(frame(1.3), (true, true, 20.));
        assert_eq!(frame(3.1), (true, true, 45.));
        assert_eq!(frame(5.2), (true, true, 70.));
        assert_eq!(frame(5.8), (false, false, 70.));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.4));
        assert!(close(until_next_step(0.7), 0.3));
        assert!(close(until_next_step(1.1), 0.1));
        assert!(close(until_next_step(1.3), 0.1));
        assert!(close(until_next_step(4.9), 0.1));
        assert!(close(until_next_step(5.1), 0.5));
        assert!(close(until_next_step(5.7), 0.3));
    }
}
