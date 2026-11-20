//! `seletor-de-modelos`: o seletor com trilho de agentes, favoritos e raciocínio ao lado; escolher um modelo do
//! Codex numa conversa do Claude leva a conversa para ele, mesmo com o turno rodando.

use std::time::Instant;
use gpui_kit::{assets::IconName, prelude::FluentBuilder, *};
use crate::{app::{self, model_picker as picker}, i18n::tr_shared, theme};

const CYCLE: f32 = 8.;
/// Clique na aba do Codex.
const CODEX: f32 = 2.6;
/// A linha do Codex escolhida: a troca fica pendente.
const PICKED: f32 = 5.2;
const LIT: f32 = 0.25;

/// O que o quadro mostra no instante `t`: a aba do Codex à vista, a aba acesa pelo clique e se o seletor já fechou.
fn frame(t: f32) -> (bool, bool, bool) { (t >= CODEX, (CODEX..CODEX + LIT).contains(&t), t >= PICKED) }

fn until_next_step(t: f32) -> f32 { [CODEX, CODEX + LIT, PICKED, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

/// Abaixo desta largura (o cartão deitado do "Atualizar?") a coluna de raciocínio não cabe: fica a lista.
const COMPACT_BELOW: f32 = 400.;

fn name(text: &str) -> AnyElement { div().truncate().text_sm().font_weight(FontWeight::MEDIUM).text_color(theme::text()).child(text.to_owned()).into_any_element() }

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado na aba do Codex, que é o que a novidade traz.
    let (codex, lit, picked) = if cx.reduce_motion() { (true, false, false) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let stage = div().id("news-model-handoff").relative().w_full().p(px(12.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)));
    if picked {
        // Seletor fechado: a faixa da troca pendente sobre o compositor e o divisor que a conversa ganha depois.
        let route = || picker::route_view("claude", "Opus 5.5".into(), "codex", "GPT-6.2-Sol".into(), 12.5);
        let band = picker::band_view(IconName::Hourglass, tr_shared("handoff_waiting_turn", &[]), theme::muted(), route(), None);
        return stage.child(div().flex().flex_col().gap(px(10.)).child(picker::divider_view(route())).child(band)).into_any_element();
    }
    let star = |on: bool| Some(picker::star_icon(on).into_any_element());
    let rows: Vec<AnyElement> = if codex {
        [("GPT-6.2-Sol", true), ("GPT-6.2-Astra", false), ("GPT-6.1 Terra", false)].into_iter().enumerate().map(|(n, (label, hot))| {
            picker::row_view(SharedString::from(format!("news-handoff-row-{n}")), "codex", name(label), false, hot, star(false), Some(n + 1)).into_any_element()
        }).collect()
    } else {
        [("Opus 5.5", true, true), ("Sonnet 5.5", false, true), ("Haiku 5.5", false, false)].into_iter().enumerate().map(|(n, (label, current, fav))| {
            picker::row_view(SharedString::from(format!("news-handoff-row-{n}")), "claude", name(label), current, false, star(fav), Some(n + 1)).into_any_element()
        }).collect()
    };
    let tab = |id: &str, selected: bool, glyph: AnyElement| picker::rail_tab(picker::rail_button(id, selected, id.into(), glyph)
        .when(lit && id == "codex", |b| b.bg(theme::accent().alpha(0.3))), selected).into_any_element();
    let rail = picker::rail_view(vec![
        tab("favorites", false, app::chrome::small_icon(IconName::Star, 16., theme::accent()).into_any_element()),
        tab("claude", !codex, app::chrome::provider_glyph("claude", 20.).into_any_element()),
        tab("codex", codex, app::chrome::provider_glyph("codex", 20.).into_any_element()),
    ]);
    let efforts: Vec<AnyElement> = ["low", "medium", "high", "xhigh"].into_iter()
        .map(|e| picker::effort_row(e, e == if codex { "low" } else { "high" }).into_any_element()).collect();
    let reasoning = (width >= px(COMPACT_BELOW)).then(|| picker::reasoning_view(if codex { "GPT-6.2-Sol" } else { "Opus 5.5" }.into(), efforts));
    let panel = div().rounded(px(12.)).border_1().border_color(theme::glass_border()).bg(theme::popup_fill(theme::raised())).p(px(4.))
        .child(div().flex().child(rail).child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.)).px(px(4.)).children(rows)).children(reasoning));
    stage.child(panel).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_opens_on_claude_moves_to_codex_and_ends_on_the_pending_handoff() {
        assert_eq!(frame(1.0), (false, false, false));
        assert_eq!(frame(2.7), (true, true, false));
        assert_eq!(frame(4.0), (true, false, false));
        assert_eq!(frame(6.0), (true, false, true));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(1.0), 1.6));
        assert!(close(until_next_step(2.7), 0.15));
        assert!(close(until_next_step(6.0), 2.0));
    }
}
