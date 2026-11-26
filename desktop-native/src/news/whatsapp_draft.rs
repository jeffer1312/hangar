//! `whatsapp-draft`: o agente deixa um rascunho de WhatsApp, o cartão aparece na conversa dele com a foto e o número,
//! as últimas mensagens, o texto exato com o Editar e o anexo, e a mensagem só sai quando a pessoa toca em Enviar. O
//! cartão é o mesmo da tela.

use std::{sync::Arc, time::Instant};
use chrono::TimeZone;
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
/// Foto de exemplo: pessoa desenhada, não é foto real (JPEG 96×96).
const AVATAR: &str = "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";

thread_local! {
    static PHOTO: Option<Arc<RenderImage>> = app::whatsapp_avatar_image(Some(AVATAR));
}

/// Hora local de exemplo: o cartão mostra a hora local, e a animação diz 14:02 em qualquer fuso.
fn at(hour: u32, minute: u32) -> String {
    chrono::Local.with_ymd_and_hms(2026, 11, 24, hour, minute, 0).single().map(|t| t.to_rfc3339()).unwrap_or_default()
}

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
fn sample_value() -> serde_json::Value {
    let (file, contact) = (tr_shared("whatsapp_draft_news_file", &[]), tr_shared("whatsapp_draft_news_contact", &[]));
    json!({
        "id": "news", "origin": format!("{}::site", tr("vault_news_machine")), "chat_number": NUMBER, "chat_name": contact,
        "chat_name_source": "contact", "chat_avatar": AVATAR, "text": tr_shared("whatsapp_draft_news_text", &[]),
        "recent": [
            {"from_me": true, "at": at(14, 2), "kind": "text", "text": tr_shared("whatsapp_draft_news_recent_mine", &[])},
            {"from_me": false, "sender": contact, "at": at(14, 3), "kind": "image", "text": ""},
            {"from_me": false, "sender": contact, "at": at(14, 3), "kind": "text", "text": tr_shared("whatsapp_draft_news_recent_theirs", &[])},
        ],
        "file": {"name": file, "size": 2_516_582, "source": format!("/home/ana/site/{file}"), "outside_cwd": false},
        "expires_at": "2026-11-24T14:10:00Z", "answer_by": "2026-11-24T14:09:30Z", "state": "pending"
    })
}

fn sample() -> Option<app::WhatsAppDraft> { serde_json::from_value(sample_value()).ok() }

/// Abaixo desta largura (o cartão deitado do "Atualizar?") o cartão inteiro não cabe legível: ficam o título e o Enviar.
const COMPACT_BELOW: f32 = 240.;
/// Alturas fixas do palco: nada pula quando o cartão troca pelo aviso do envio. Cabe o cartão inteiro, com a foto e as três
/// mensagens.
const STAGE_HEIGHT: f32 = 500.;
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
                let text = app::whatsapp_mark_invisibles(&tr_shared("whatsapp_draft_news_text", &[]));
                // A máquina do exemplo aceita edição: o Editar aparece, sem clique.
                let edit = app::whatsapp_edit_button("edit", "news-whatsapp-draft", tr_shared("whatsapp_draft_edit", &[]), false).tab_stop(false);
                let block = app::WhatsAppTextBlock::Exact { text: &text, edit: Some(edit) };
                app::whatsapp_draft_card(&draft, block, &draft.bubbles(), PHOTO.with(Clone::clone), &tr("vault_news_machine"),
                    app::whatsapp_countdown_label("9:12"), send, decline, None)
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
            for key in ["whatsapp_draft_news_contact", "whatsapp_draft_news_agent", "whatsapp_draft_news_text", "whatsapp_draft_news_file",
                "whatsapp_draft_news_recent_mine", "whatsapp_draft_news_recent_theirs", "whatsapp_draft_edit", "whatsapp_draft_kind_image",
                "whatsapp_draft_avatar"] {
                assert_ne!(tr_shared(key, &[]), key, "frase sem tradução ({english})");
            }
            let value = sample_value();
            let recent = value["recent"].as_array().unwrap();
            assert_eq!(recent.iter().map(|r| (r["from_me"].as_bool(), r["kind"].as_str())).collect::<Vec<_>>(),
                [(Some(true), Some("text")), (Some(false), Some("image")), (Some(false), Some("text"))]);
            assert!(recent.iter().all(|r| chrono::DateTime::parse_from_rfc3339(r["at"].as_str().unwrap()).is_ok()), "hora ilegível ({english})");
        }
        crate::i18n::english_in_this_thread(false);
    }

    #[test]
    fn sample_photo_decodes_and_the_times_read_as_in_the_script() {
        assert!(PHOTO.with(Option::is_some), "a foto de exemplo tem que decodificar");
        let local = |s: String| chrono::DateTime::parse_from_rfc3339(&s).unwrap().with_timezone(&chrono::Local).format("%H:%M").to_string();
        assert_eq!((local(at(14, 2)), local(at(14, 3))), ("14:02".to_owned(), "14:03".to_owned()));
    }
}
