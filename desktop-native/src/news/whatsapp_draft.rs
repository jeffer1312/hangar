//! `whatsapp-rascunho`: o agente deixa um rascunho de WhatsApp, o cartão aparece na conversa dele com o número, o texto
//! exato e o anexo, e a mensagem só sai quando a pessoa toca em Enviar. O cartão é o mesmo da tela.

use std::time::Instant;
use gpui_kit::{component::button::*, prelude::FluentBuilder, *};
use serde_json::json;
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 8.;
/// O cartão entra deslizando em `SLIDE_STEPS` degraus de `SLIDE_STEP`.
const CARD: f32 = 0.6;
const SLIDE_STEP: f32 = 0.05;
const SLIDE_STEPS: u8 = 3;
/// Tempo de ler o cartão; depois o Enviar é tocado e fica em destaque.
const PRESS: f32 = 5.;
/// O cartão some e o aviso do envio aparece.
const SENT: f32 = 5.6;
/// Número de mentira: o DDD 00 não existe.
const NUMBER: &str = "5500912345678";

/// Quadro do roteiro no instante `t` do ciclo: o degrau do cartão (0 entrando, `SLIDE_STEPS` parado), o Enviar em
/// destaque e se a mensagem já saiu.
fn frame(t: f32) -> (Option<u8>, bool, bool) {
    let card = (t >= CARD && t < SENT).then(|| (((t - CARD) / SLIDE_STEP).floor() as u8).min(SLIDE_STEPS));
    (card, t >= PRESS && t < SENT, t >= SENT)
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    let slide = (1..=SLIDE_STEPS).map(|k| CARD + k as f32 * SLIDE_STEP);
    slide.chain([CARD, PRESS, SENT, CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

/// O rascunho de exemplo no formato do evento `whatsapp_outbox`: o cartão lê dele como lê o de verdade.
fn sample() -> Option<app::WhatsAppDraft> {
    let file = tr_shared("whatsapp_draft_news_file", &[]);
    serde_json::from_value(json!({
        "id": "news", "origin": format!("{}::site", tr("vault_news_machine")), "chat_number": NUMBER, "chat_name": "Marina",
        "chat_name_source": "contact", "text": tr_shared("whatsapp_draft_news_text", &[]),
        "file": {"name": file, "size": 2_516_582, "source": format!("/home/ana/site/{file}"), "outside_cwd": false},
        "expires_at": "2026-11-24T14:10:00Z", "state": "pending"
    })).ok()
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") o cartão inteiro não cabe legível: ficam o título e o Enviar.
const COMPACT_BELOW: f32 = 240.;
/// Alturas fixas do palco: nada pula quando o cartão troca pelo aviso do envio.
const STAGE_HEIGHT: f32 = 330.;
const COMPACT_HEIGHT: f32 = 58.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no cartão inteiro, antes do toque.
    let (card, pressed, sent) = if cx.reduce_motion() { (Some(SLIDE_STEPS), false, false) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let line = |text: String| div().text_xs().text_color(theme::text()).whitespace_normal().child(text);
    // Miniatura só leitura: os botões ficam sem clique e fora do Tab.
    let send = app::request_button("whatsapp-send", "news-whatsapp-draft", tr_shared("whatsapp_draft_send", &[]), false).primary().tab_stop(false)
        .when(pressed, |button| button.border_2().border_color(theme::text()));
    let shown = card.map(|step| {
        let content = match (!compact).then(sample).flatten() {
            Some(draft) => {
                let decline = app::request_button("whatsapp-decline", "news-whatsapp-draft", tr_shared("whatsapp_draft_decline", &[]), false)
                    .outline().tab_stop(false);
                app::whatsapp_draft_card(&draft, &tr("vault_news_machine"), app::whatsapp_countdown_label("9:12"), send, decline, None)
                    .mx_0().my_0().into_any_element()
            }
            None => div().flex().flex_col().gap_1()
                .child(div().truncate().text_xs().text_color(theme::text()).child(tr_shared("whatsapp_draft_title", &[]))).child(send).into_any_element(),
        };
        div().relative().top(px(((SLIDE_STEPS - step) * 4) as f32)).opacity(0.25 + 0.25 * step as f32).child(content)
    });
    let result = sent.then(|| line(tr_shared("whatsapp_draft_sending", &[])));
    let stage = if compact {
        div().h(px(COMPACT_HEIGHT)).flex().flex_col().justify_center().children(shown).children(result)
    } else {
        let composer = app::composer_card(false).p(px(6.)).pl(px(10.)).rounded(px(8.))
            .child(div().truncate().text_xs().text_color(theme::muted()).child(tr("composer").replace("{agent}", "Claude")));
        div().h(px(STAGE_HEIGHT)).flex().flex_col().justify_end().gap_2()
            .child(line(tr_shared("whatsapp_draft_news_agent", &[]))).children(shown).children(result).child(composer)
    };
    div().id("news-whatsapp-draft").w(width).p(px(if compact { 8. } else { 14. })).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .text_color(theme::text())
        .child(stage.overflow_hidden())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn script_slides_the_card_in_presses_send_and_clears() {
        assert_eq!(frame(0.3), (None, false, false));
        assert_eq!(frame(0.62).0, Some(0));
        assert_eq!(frame(1.).0, Some(SLIDE_STEPS));
        assert_eq!(frame(5.2), (Some(SLIDE_STEPS), true, false));
        assert_eq!(frame(6.), (None, false, true));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.4));
        assert!(close(until_next_step(1.), 4.));
        assert!(close(until_next_step(5.2), 0.4));
        assert!(close(until_next_step(6.), 2.));
    }

    #[test]
    fn sample_draft_reads_like_the_event_in_both_languages() {
        for english in [false, true] {
            crate::i18n::english_in_this_thread(english);
            assert!(sample().is_some(), "rascunho de exemplo ilegível ({english})");
            for key in ["whatsapp_draft_news_agent", "whatsapp_draft_news_text", "whatsapp_draft_news_file"] {
                assert_ne!(tr_shared(key, &[]), key, "frase sem tradução ({english})");
            }
        }
        crate::i18n::english_in_this_thread(false);
    }
}
