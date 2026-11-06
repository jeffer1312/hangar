//! `pedido-na-sessao`: o agente pede uma senha e o cartão aparece na conversa dele, acima da caixa de mensagem;
//! guardada a senha, o cartão sai e o agente segue. Mesmo roteiro do `cofre-de-senhas`.

use std::time::Instant;
use gpui_kit::{component::button::*, prelude::FluentBuilder, *};
use super::password_vault::{CYCLE, SLIDE_STEPS, frame, schedule_repaint};
use crate::{app, i18n::tr, theme};

/// Abaixo desta largura (o cartão deitado do "Atualizar?") ficam só o título e o botão.
const COMPACT_BELOW: f32 = 240.;
/// Alturas fixas do palco: nada pula quando o cartão troca pela resposta do agente.
const STAGE_HEIGHT: f32 = 230.;
const COMPACT_HEIGHT: f32 = 58.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no cartão dentro da conversa, antes do clique.
    let (card, pressed, resolved) = if cx.reduce_motion() { (Some(SLIDE_STEPS), false, false) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let line = |text: String| div().text_xs().text_color(theme::text()).whitespace_normal().child(text);
    // Miniatura só leitura: os botões ficam sem clique e fora do Tab.
    let keep = app::request_button("keep", "news-session", tr("vault_request_keep"), false).primary().tab_stop(false)
        .when(pressed, |button| button.border_2().border_color(theme::text()));
    let title = tr("vault_request_entry_title").replace("{origin}", &tr("vault_session_news_origin"))
        .replace("{label}", &tr("vault_session_news_label"));
    let shown = card.map(|step| {
        let content = if compact {
            div().flex().flex_col().gap_1().child(div().truncate().text_xs().text_color(theme::text()).child(title)).child(keep).into_any_element()
        } else {
            let once = app::request_button("once", "news-session", tr("vault_request_once"), false).outline().tab_stop(false);
            app::request_card("news-session", &tr("vault_news_machine"), title, &tr("vault_session_news_reason"), app::request_pair(keep, once), None)
                .mx_0().my_0().into_any_element()
        };
        div().relative().top(px(((SLIDE_STEPS - step) * 4) as f32)).opacity(0.25 + 0.25 * step as f32).child(content)
    });
    let result = resolved.then(|| line(tr("vault_session_news_result")));
    let stage = if compact {
        div().h(px(COMPACT_HEIGHT)).flex().flex_col().justify_center().children(shown).children(result)
    } else {
        let composer = app::composer_card(false).p(px(6.)).pl(px(10.)).rounded(px(8.))
            .child(div().truncate().text_xs().text_color(theme::muted()).child(tr("composer").replace("{agent}", "Claude")));
        div().h(px(STAGE_HEIGHT)).flex().flex_col().justify_end().gap_2()
            .child(line(tr("vault_session_news_agent"))).children(shown).children(result).child(composer)
    };
    div().id("news-session-vault").w(width).p(px(if compact { 8. } else { 14. })).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .text_color(theme::text())
        .child(stage.overflow_hidden())
        .into_any_element()
}
