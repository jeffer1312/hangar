//! `conversa-nova`: a mesma aba aberta, a aba que espera resposta e a sugestão de nome, primeiro no desenho atual e
//! depois na conversa nova (beta), alternando.

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app::{self, chrome}, appearance, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// Metade do ciclo em cada desenho.
const SWITCH: f32 = 3.;

/// Qual desenho o instante `t` do ciclo mostra: `true` é a conversa nova.
fn shows_new(t: f32) -> bool { t >= SWITCH }

/// Segundos até a próxima troca de desenho: o roteiro anda em dois degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 { if t < SWITCH { SWITCH - t } else { CYCLE - t } }

/// Abaixo desta largura (o cartão deitado do "Atualizar?") fica só a dupla de abas.
const COMPACT_BELOW: f32 = 240.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no desenho novo, que é o que a novidade apresenta.
    let new = if cx.reduce_motion() { true } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        shows_new(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let pad = if compact { 8. } else { 14. };
    appearance::with_conversation_beta(new, || {
        let label = tr(if new { "settings_conversation_beta_on" } else { "settings_conversation_beta_off" });
        let tab = |name: String, on: bool, waiting: bool, state: &str| {
            let avatar = chrome::session_avatar(&chrome::AvatarLook::new("claude", None, state, false), 20., None);
            app::tab_frame(on, false, false, waiting).w(px(if compact { f32::from(width) - 2. * pad } else { 170. }))
                .child(avatar).child(app::tab_title(name, None, false, false, None, None))
        };
        let tabs = div().flex().gap(px(8.)).flex_wrap()
            .child(tab(tr_shared("news_rename_sample_name", &[]), true, false, "working"))
            .when(!compact, |el| el.child(tab(tr("news_conversation_waiting_tab"), false, true, "awaiting_input")));
        let stage = div().id("news-new-conversation").w(width).p(px(pad)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
            .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
            .flex().flex_col().gap(px(12.)).text_color(theme::text())
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(if new { theme::accent_text() } else { theme::muted() })
                .child(format!("{} · {label}", tr("settings_conversation_beta"))))
            .child(tabs);
        if compact { return stage.into_any_element(); }
        // Miniatura só leitura: os botões da faixa ficam sem clique e fora do Tab.
        stage.child(app::rename_strip(&tr_shared("news_rename_sample_name", &[]), app::RenameStrip { weak: true, ..Default::default() },
                |button| button.tab_stop(false), |button| button.tab_stop(false), |button| button.tab_stop(false)))
            .into_any_element()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_alternates_current_and_new() {
        assert!(!shows_new(0.5));
        assert!(!shows_new(2.9));
        assert!(shows_new(3.1));
        assert!(shows_new(5.9));
    }

    #[test]
    fn repaint_waits_for_the_next_switch() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 2.5));
        assert!(close(until_next_step(4.0), 2.0));
    }
}
