//! `sonho-da-memoria`: Sonhar agora num projeto, o sonho roda e a linha mostra quando foi, quanto custou e o que mudou.

use std::time::Instant;
use gpui_kit::{component::{Disableable, button::*}, *};
use crate::{app::{self, memory_dream as dream}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// Sonhar agora é apertado: o botão vira Sonhando….
const RUN: f32 = 1.5;
/// O sonho termina: a linha ganha o último sonho e a lista do que mudou aparece.
const DONE: f32 = 3.5;
/// Amostra: o projeto, as máquinas dele, as conversas novas antes do sonho e o custo da rodada.
const PROJECT: &str = "hangar";
const ID: &str = "news-dream";
const MACHINES: &str = "notebook, vps";
const NEW_SESSIONS: u64 = 12;
const COST_USD: f64 = 0.30;
const FILES: [(&str, &str); 2] = [("deploy.md", "dream_file_added"), ("MEMORY.md", "dream_file_changed")];
/// Altura com a barra da linha quebrada em duas (Desfazer entra no fim): o cartão não pula.
const STAGE_HEIGHT: f32 = 350.;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Idle,
    Running,
    Done,
}

fn frame(t: f32) -> Phase {
    if t >= DONE { Phase::Done } else if t >= RUN { Phase::Running } else { Phase::Idle }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [RUN, DONE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado com o sonho feito e a lista do que mudou.
    let phase = if cx.reduce_motion() { Phase::Done } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let (running, done) = (phase == Phase::Running, phase == Phase::Done);
    // Depois do sonho as conversas novas zeram: elas contam a partir da última rodada.
    let facts = dream::facts(MACHINES.into(), if done { 0 } else { NEW_SESSIONS }, done.then(|| (tr("since_now"), COST_USD)));
    let picker = dream::mode_picker(ID, "key", false, None::<fn(&'static str, &mut Window, &mut App)>, cx);
    let run = dream::run_button(SharedString::from(format!("dream-run-{ID}")), false).outline().disabled(running).tab_stop(false);
    let row = dream::row_frame(ID)
        .child(dream::row_head(PROJECT.into(), true, facts, dream::project_switch(ID, PROJECT, true, false).tab_stop(false)))
        .child(dream::row_bar(picker, None, run, done.then(|| dream::changes_button(ID).tab_stop(false)),
            done.then(|| dream::undo_button(ID, false).tab_stop(false))));
    let head = dream::projects_head(dream::refresh_button(false).tab_stop(false),
        dream::run_button("news-dream-run-all", running).primary().disabled(running).tab_stop(false));
    // A lista abre num diálogo na página; aqui fica embaixo da linha, com a altura já reservada.
    let changes = div().flex().flex_col().gap(px(4.)).opacity(if done { 1. } else { 0. })
        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(tr_shared("dream_changes", &[])))
        .children(FILES.map(|(file, key)| {
            dream::file_button(SharedString::from(format!("news-dream-file-{file}")), file.into(), key, false).tab_stop(false)
        }));
    div().id("news-memory-dream").relative().w(width).min_h(px(STAGE_HEIGHT)).p(px(14.)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap_3()
        .child(div().flex().flex_col().child(head).child(app::settings_box().child(row)))
        .child(changes)
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
    fn the_button_runs_and_then_the_row_shows_the_dream() {
        assert_eq!(frame(0.5), Phase::Idle);
        assert_eq!(frame(1.6), Phase::Running);
        assert_eq!(frame(3.5), Phase::Done);
        assert_eq!(frame(5.9), Phase::Done);
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 1.));
        assert!(close(until_next_step(2.), 1.5));
        assert!(close(until_next_step(4.), 2.));
    }
}
