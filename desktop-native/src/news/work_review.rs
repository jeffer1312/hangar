//! `revisao-do-trabalho`: a aba Revisão mostra uma decisão por vez, com zoom no mock e na tela; cada escolha baixa a conta.

use std::time::Instant;
use gpui_kit::*;
use crate::{api::dto::{WorkReview, WorkReviewChoice, WorkReviewDecision, WorkReviewHeadline, WorkReviewSide, WorkReviewState, WorkReviewTone},
    app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.;
/// "Igualar ao mock" escolhido na primeira e na segunda decisão.
const PICKS: [f32; 2] = [1.6, 4.];
/// O cartão passa à decisão seguinte, depois do respiro da tela real.
const NEXTS: [f32; 2] = [2.4, 4.8];
const TOTAL: usize = 3;
/// Abaixo desta largura (o cartão deitado do "Atualizar?") só a faixa com a conta cabe legível.
const COMPACT_BELOW: f32 = 240.;
/// Tamanho da captura desenhada: o trecho (`box`) é medido nele, como o do conector.
const SHOT: (u32, u32) = (1600, 900);

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { index: usize, picked: usize }

fn frame(t: f32) -> Frame {
    Frame { index: NEXTS.iter().filter(|n| t >= **n).count(), picked: PICKS.iter().filter(|p| t >= **p).count() }
}

fn until_next_step(t: f32) -> f32 { PICKS.into_iter().chain(NEXTS).chain([CYCLE]).filter(|b| *b > t).fold(CYCLE, f32::min) - t }

/// O trecho de cada lado e a cor do elemento nele: o mock e a tela diferem no que a decisão aponta.
struct Sample { title: &'static str, left: ([u32; 4], u32, &'static str), right: ([u32; 4], u32, &'static str) }

const SAMPLES: [Sample; TOTAL] = [
    Sample { title: "news_work_review_sample_title_1", left: ([560, 380, 480, 140], 0x4472c4, "~480 px"), right: ([600, 380, 400, 140], 0x4472c4, "~400 px") },
    Sample { title: "news_work_review_sample_title_2", left: ([520, 300, 560, 120], 0x2f6fde, "#2f6fde"), right: ([520, 300, 560, 120], 0xc8783c, "#c8783c") },
    Sample { title: "news_work_review_sample_title_3", left: ([480, 320, 640, 200], 0x5a9e6f, "24 px"), right: ([480, 300, 640, 240], 0x5a9e6f, "40 px") },
];

fn side(label: String, (area, _, value): ([u32; 4], u32, &'static str)) -> WorkReviewSide {
    WorkReviewSide { label, value: Some(value.into()), asset: String::new(), width: SHOT.0, height: SHOT.1, frame: Some(area) }
}

fn decision(n: usize) -> WorkReviewDecision {
    let sample = &SAMPLES[n];
    let choice = |id: &str, label: &str, hint: &str, requires_reason| WorkReviewChoice { id: id.into(), label: tr(label), hint: Some(tr(hint)),
        requires_reason, default_reason: None, reason_prompt: None };
    WorkReviewDecision {
        id: n.to_string(), tag: Some(tr("news_work_review_sample_tag").replace("{n}", &(n + 1).to_string())), title: tr(sample.title),
        reason: Some(tr("news_work_review_sample_reason")), reason_label: Some(tr("news_work_review_sample_reason_label")),
        left: Some(side(tr("news_work_review_sample_left"), sample.left)), right: Some(side(tr("news_work_review_sample_right"), sample.right)),
        choices: vec![choice("match", "news_work_review_sample_match", "news_work_review_sample_match_hint", false),
            choice("keep", "news_work_review_sample_keep", "news_work_review_sample_keep_hint", true)],
    }
}

/// A captura desenhada: fundo da tela, duas linhas de conteúdo e o elemento da decisão no trecho, na cor do lado.
fn shot((area, color, _): ([u32; 4], u32, &'static str)) -> AnyElement {
    let [x, y, w, h] = area.map(|n| n as f32);
    let (sw, sh) = (SHOT.0 as f32, SHOT.1 as f32);
    let line = |top: f32, width: f32| div().absolute().left(relative(0.06)).top(relative(top)).w(relative(width)).h(relative(0.03)).rounded(px(2.)).bg(rgb(0x9a9ca3));
    div().relative().size_full().bg(rgb(0xb8bac0))
        .child(line(0.12, 0.4)).child(line(0.2, 0.26))
        .child(div().absolute().left(relative(x / sw)).top(relative(y / sh)).w(relative(w / sw)).h(relative(h / sh)).rounded(px(3.)).bg(rgb(color)))
        .into_any_element()
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: a primeira decisão já escolhida, com a conta baixando para 2.
    let Frame { index, picked } = if cx.reduce_motion() { frame(PICKS[0]) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let pending = TOTAL - picked;
    let decisions = (0..TOTAL).map(decision).collect::<Vec<_>>();
    let review = WorkReview {
        id: "news".into(), title: String::new(), subject: None, updated_at: String::new(), state: WorkReviewState::Waiting, steps: None,
        headline: Some(WorkReviewHeadline { value: TOTAL.to_string(), text: tr_shared("work_review_waiting", &[]), detail: Some(tr("news_work_review_sample_detail")) }),
        progress: None, badge: None, decisions, resolved: None, kept: None, links: Vec::new(),
    };
    let band = app::review_band(&review, pending);
    let stage = div().id("news-work-review").w(width).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06)).overflow_hidden();
    if width < px(COMPACT_BELOW) {
        return stage.h(px(74.)).p(px(6.)).bg(theme::inset()).children(band).into_any_element();
    }
    let decision = &review.decisions[index];
    let inner = f32::from(width) - 28.;
    let w = app::review_frame_width(inner, true);
    let h = (w * 0.5).clamp(70., 120.);
    let sample = &SAMPLES[index];
    let quadro = |slot: &str| div().id(SharedString::from(format!("news-work-review-frame-{slot}")));
    let frames = app::review_frames_row(
        app::review_side_frame(quadro("left"), decision.left.as_ref().unwrap(), w, h, Some(shot(sample.left))),
        Some(app::review_side_frame(quadro("right"), decision.right.as_ref().unwrap(), w, h, Some(shot(sample.right)))));
    let chosen = picked > index;
    let choices = decision.choices.iter().map(|c| app::review_choice_button(&decision.id, c, chosen && c.id == "match", cx).tab_stop(false)).collect();
    let done = (0..TOTAL).map(|n| n < picked).collect::<Vec<_>>();
    let (previous, next, send) = app::review_decision_nav(index, TOTAL, pending, pending == 0, false);
    let card = app::review_decision_card(decision, &done, index, Some(frames), choices)
        .child(app::review_decision_nav_row(previous.tab_stop(false), next.tab_stop(false), send.tab_stop(false)));
    let tabs = div().h(px(30.)).flex().gap(px(2.)).border_b_1().border_color(theme::border())
        .child(app::side_tab_slot(false, app::side_tab_button("news-work-review-tab-context", tr("side_context"), false, None, cx).tab_stop(false)))
        .child(app::side_tab_slot(true, app::side_tab_button("news-work-review-tab", tr_shared("work_review_tab", &[]), true,
            Some(app::review_tab_pill(pending.to_string(), WorkReviewTone::Warning)), cx).tab_stop(false)));
    stage.p(px(14.)).bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(tabs).children(band).child(card)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn each_pick_lowers_the_count_and_then_the_card_moves_on() {
        assert_eq!(frame(0.5), Frame { index: 0, picked: 0 });
        assert_eq!(frame(2.), Frame { index: 0, picked: 1 });
        assert_eq!(frame(3.), Frame { index: 1, picked: 1 });
        assert_eq!(frame(4.2), Frame { index: 1, picked: 2 });
        assert_eq!(frame(6.), Frame { index: 2, picked: 2 });
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 1.1));
        assert!(close(until_next_step(2.), 0.4));
        assert!(close(until_next_step(5.), 2.));
    }
}
