//! `antigravity`: o Antigravity (Google) entra no seletor de agente da conversa nova; na sessão, a pílula do modelo mostra
//! o modelo do agy e a do modo troca como nos outros agentes.

use std::time::Instant;
use gpui_kit::{base::Selectable, component::ActiveTheme, prelude::FluentBuilder, *};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// O ponteiro para sobre o segmento do Antigravity.
const HOVER: f32 = 0.6;
/// O Antigravity é escolhido: o compositor passa a ser dele e a pílula do modelo ainda não leu o modelo.
const AGENT: f32 = 1.2;
/// A pílula do modelo mostra o modelo do agy.
const MODEL: f32 = 2.4;
/// A pílula do modo troca para Automático.
const MODE: f32 = 3.8;
/// Quanto a pílula fica acesa depois da troca.
const LIT: f32 = 0.35;
const AGENTS: [&str; 3] = ["claude", "codex", "antigravity"];
const AGY_MODEL: &str = "Gemini 4 Neon (Low)";

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { hover: bool, agy: bool, model: bool, model_lit: bool, auto: bool, mode_lit: bool }

fn frame(t: f32) -> Frame {
    Frame { hover: (HOVER..AGENT).contains(&t), agy: t >= AGENT, model: t >= MODEL, model_lit: (MODEL..MODEL + LIT).contains(&t),
        auto: t >= MODE, mode_lit: (MODE..MODE + LIT).contains(&t) }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [HOVER, AGENT, MODEL, MODEL + LIT, MODE, MODE + LIT, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") a trilha dos agentes não cabe: ficam as duas pílulas.
const COMPACT_BELOW: f32 = 360.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no fim, com o Antigravity escolhido, o modelo dele e o modo trocado.
    let f = if cx.reduce_motion() { Frame { hover: false, agy: true, model: true, model_lit: false, auto: true, mode_lit: false } } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    // As pílulas da sessão: a do Claude tem o esforço ao lado; a do Antigravity, sem esforço separado, mostra "Modelo" até
    // ler o modelo, como na conversa de verdade.
    let model = if f.agy {
        let text = if f.model { AGY_MODEL.to_owned() } else { tr("ctl_model") };
        app::ctl_pill("ctl-model", text, Some("antigravity"), false, cx)
    } else {
        app::model_pill("ctl-model", "claude", "Opus 5.5".into(), Some(tr_shared("reasoning_level_high", &[])), None, cx)
    };
    let model = model.selected(f.model_lit).tab_stop(false);
    let mode = app::menu_pill("ctl-mode", tr(if f.auto { "mode_auto" } else { "mode_acceptEdits" }), false, cx).selected(f.mode_lit).tab_stop(false);
    let stage = div().id("news-antigravity").w(width).overflow_hidden().rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col();
    if width < px(COMPACT_BELOW) {
        return stage.p(px(8.)).gap(px(6.)).items_start().child(model).child(mode).into_any_element();
    }
    let agents = app::provider_track().children(AGENTS.into_iter().map(|p| {
        app::provider_segment(p, if p == "antigravity" { f.agy } else { !f.agy && p == "claude" }, cx).tab_stop(false)
            .when(f.hover && p == "antigravity", |el| el.bg(theme::hover()))
    }));
    let agent = if f.agy { "Antigravity" } else { "Claude" };
    let field = div().text_size(px(13.5)).min_h(px(22.)).flex().items_center().overflow_hidden()
        .child(div().truncate().text_color(cx.theme().muted_foreground).child(tr("composer").replace("{agent}", agent)));
    let send = app::send_button("send", false, tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field)
        .child(app::composer_control_row([], [model.into_any_element(), mode.into_any_element()], send));
    stage.p(px(14.)).gap(px(14.)).child(agents).child(composer).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_picks_antigravity_then_shows_the_model_then_the_mode() {
        let start = frame(0.3);
        assert!(!start.hover && !start.agy && !start.model && !start.auto);
        assert!(frame(0.8).hover && !frame(0.8).agy);
        let picked = frame(1.5);
        assert!(picked.agy && !picked.hover && !picked.model);
        assert!(frame(2.5).model && frame(2.5).model_lit);
        assert!(frame(3.).model && !frame(3.).model_lit && !frame(3.).auto);
        assert!(frame(3.9).auto && frame(3.9).mode_lit);
        assert!(frame(5.).auto && !frame(5.).mode_lit);
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.4));
        assert!(close(until_next_step(1.5), 0.9));
        assert!(close(until_next_step(2.5), 0.25));
        assert!(close(until_next_step(4.5), 1.5));
    }
}
