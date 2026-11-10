//! `plugins-do-claude`: a página Plugins liga um plugin e a faixa oferece reabrir as sessões que ainda estão com o estado anterior.

use std::time::Instant;
use gpui_kit::*;
use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 6.5;
/// A chave do "humanizer" liga.
const FLIP: f32 = 1.5;
/// A faixa entra em `FADE_STEPS` degraus de `FADE_STEP`.
const BANNER: f32 = 2.2;
const FADE_STEP: f32 = 0.08;
const FADE_STEPS: u8 = 3;
/// Atualizado em: a mesma data das amostras, no formato do idioma de quem olha.
const SAMPLE_DATE: &str = "2026-11-07T10:00:00Z";
/// Sessões Claude abertas na amostra.
const OPEN_SESSIONS: usize = 3;
/// Altura do quadro com a faixa à vista: o cartão não pula quando ela entra.
const STAGE_HEIGHT: f32 = 290.;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame {
    on: bool,
    /// Degrau da entrada da faixa: 0 = escondida, `FADE_STEPS` = inteira.
    banner: u8,
}

fn frame(t: f32) -> Frame {
    let banner = if t < BANNER { 0 } else { (((t - BANNER) / FADE_STEP).floor() as u8 + 1).min(FADE_STEPS) };
    Frame { on: t >= FLIP, banner }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    let fade = (0..FADE_STEPS).map(|k| BANNER + k as f32 * FADE_STEP);
    fade.chain([FLIP, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado com a chave ligada e a faixa à vista.
    let f = if cx.reduce_motion() { Frame { on: true, banner: FADE_STEPS } } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let caveman = app::plugin_row_box("news-plugins-caveman")
        .child(app::plugin_text_block(app::plugin_name("caveman".into(), true, Some("4.2.0".into())),
            app::plugin_meta("4.1.0", SAMPLE_DATE, "caveman"), Some("Talk like caveman. Cut filler.".into())))
        .child(app::plugin_update_button("news-plugins-update", false).tab_stop(false))
        .child(app::plugin_switch("news-plugins-switch-caveman", "caveman", true, false).tab_stop(false));
    let humanizer = app::plugin_row_box("news-plugins-humanizer")
        .child(app::plugin_text_block(app::plugin_name("humanizer".into(), f.on, None),
            app::plugin_meta("3.1.0", SAMPLE_DATE, "humanizer"), Some("Remove signs of AI-generated writing.".into())))
        .child(app::plugin_switch("news-plugins-switch-humanizer", "humanizer", f.on, false).tab_stop(false));
    let banner = (f.banner > 0).then(|| {
        let shown = f.banner as f32 / FADE_STEPS as f32;
        let action = tr("plugins_action_enabled").replace("{name}", "humanizer");
        app::plugins_reopen_notice(&action, OPEN_SESSIONS, false, &[], |b| b.tab_stop(false), |b| b.tab_stop(false), cx)
            .relative().top(px(-6. * (1. - shown))).opacity(shown)
    });
    div().id("news-plugins").relative().w(width).min_h(px(STAGE_HEIGHT)).p(px(14.)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap_4()
        .children(banner)
        .child(app::settings_box().child(caveman).child(humanizer))
        // Miniatura só leitura: nada aqui dentro recebe o mouse.
        .child(div().absolute().top_0().left_0().size_full().block_mouse_except_scroll())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_switch_turns_on_and_then_the_banner_fades_in() {
        assert_eq!(frame(0.5), Frame { on: false, banner: 0 });
        assert_eq!(frame(1.6), Frame { on: true, banner: 0 });
        assert_eq!(frame(2.2), Frame { on: true, banner: 1 });
        assert_eq!(frame(2.3), Frame { on: true, banner: 2 });
        assert_eq!(frame(5.), Frame { on: true, banner: FADE_STEPS });
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 1.));
        assert!(close(until_next_step(1.6), 0.6));
        assert!(close(until_next_step(2.25), 0.03));
        assert!(close(until_next_step(3.), 3.5));
    }
}
