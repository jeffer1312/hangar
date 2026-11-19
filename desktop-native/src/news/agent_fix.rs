//! `consertar-com-agente`: o servidor desta máquina parado, Consertar com agente, o Claude trabalhando passo a passo e
//! o servidor de volta com o resumo do que ele fez.

use std::time::{Duration, Instant};
use gpui_kit::{assets::IconName, *};
use hangar_local_server::service::{TASK, UNIT};
use crate::{app::{chrome, status_parts as parts, setup::agent::Agent}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.;
/// Consertar com agente é apertado: o Claude começa.
const START: f32 = 1.6;
/// Cada passo do Claude termina nestes instantes; o próximo começa junto.
const STEPS_DONE: [f32; 3] = [2.5, 3.0, 4.2];
/// O servidor responde: o resumo aparece.
const FIXED: f32 = 4.6;
/// O relógio do agente anda mais rápido que o da animação: 3 s viram uns 25 s de conserto.
const CLOCK_SPEED: f32 = 8.;
const VERSION: &str = "0.1.0.9160";
const STAGE_HEIGHT: f32 = 340.;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    Down,
    /// Quantos passos já terminaram.
    Working(usize),
    Fixed,
}

fn frame(t: f32) -> Phase {
    if t >= FIXED { Phase::Fixed } else if t >= START { Phase::Working(STEPS_DONE.iter().filter(|done| t >= **done).count()) } else { Phase::Down }
}

fn until_next_step(t: f32) -> f32 {
    [START, STEPS_DONE[0], STEPS_DONE[1], STEPS_DONE[2], FIXED, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

fn agent_time(t: f32) -> Duration { Duration::from_secs_f32(((t - START) * CLOCK_SPEED).max(0.)) }

/// Os três passos da amostra: ler o serviço, achar a causa no log e subir de novo.
fn sample_steps(t: f32) -> Vec<parts::Step> {
    let (status, log, restart) = if cfg!(windows) {
        (format!("schtasks /Query /TN \"{TASK}\" /V /FO LIST"), "Get-Content $HOME\\.hangar\\logs\\server.log -Tail 40".to_owned(),
            format!("Stop-Process -Id (Get-NetTCPConnection -LocalPort 8765).OwningProcess\nschtasks /Run /TN \"{TASK}\""))
    } else {
        (format!("systemctl --user status {UNIT}"), format!("journalctl --user -u {UNIT} -n 40"),
            format!("fuser -k 8765/tcp\nsystemctl --user restart {UNIT}"))
    };
    let plan = [
        ("news_agent_fix_step_status", status, vec!["Active: failed (Result: exit-code)".to_owned()]),
        ("news_agent_fix_step_log", log, vec!["bind 127.0.0.1:8765: address already in use".to_owned()]),
        ("news_agent_fix_step_restart", restart, Vec::new()),
    ];
    let started = [START, STEPS_DONE[0], STEPS_DONE[1]];
    plan.into_iter().enumerate().filter(|(i, _)| t >= started[*i]).map(|(i, (key, command, output))| {
        let done = t >= STEPS_DONE[i];
        parts::Step {
            id: format!("news-step-{i}"),
            action: parts::Action::Command { description: Some(tr_shared(key, &[])), command },
            output: if done { output } else { Vec::new() },
            state: if done { parts::StepState::Done } else { parts::StepState::Running },
            at: agent_time(started[i]),
            took: done.then(|| agent_time(STEPS_DONE[i]) - agent_time(started[i])),
        }
    }).collect()
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado com o servidor de volta e o resumo.
    let t = if cx.reduce_motion() { CYCLE - 0.1 } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        t
    };
    let agent = Agent::Claude;
    let name = agent.name();
    let cards: Vec<AnyElement> = match frame(t) {
        Phase::Down => {
            let account = tr("status_agent_account_default");
            let bar = parts::fix_bar(parts::fix_button(agent, &account).tab_stop(false), parts::single_agent(agent), account,
                Some(parts::start_button().outline().tab_stop(false)));
            vec![parts::down_head(chrome::small_icon(IconName::Close, 24., theme::danger()).into_any_element(), theme::danger(), false,
                tr("status_down_stopped_title"), Some(parts::silent_for(Duration::from_secs(150))), [bar]).into_any_element()]
        }
        Phase::Working(done) => {
            let steps = sample_steps(t);
            let sub = parts::working_sub(&parts::clock(agent_time(t)), done + 1);
            let mut out = vec![parts::working_head(name, sub, parts::stop_button().tab_stop(false), false).into_any_element()];
            out.extend(parts::working_steps(&steps, agent_time(t), None));
            out
        }
        Phase::Fixed => {
            let sub = parts::fixed_sub(name, &parts::clock(agent_time(FIXED)), Some(VERSION));
            let items = ["news_agent_fix_summary_port", "news_agent_fix_summary_restart"].map(|key| tr_shared(key, &[])).to_vec();
            vec![parts::fixed_head(sub, items, parts::see_status_button().tab_stop(false)).into_any_element()]
        }
    };
    div().id("news-agent-fix").relative().w(width).min_h(px(STAGE_HEIGHT)).p(px(14.)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(12.))
        .children(cards)
        // Miniatura só leitura: nada aqui dentro recebe o mouse.
        .child(div().absolute().top_0().left_0().size_full().block_mouse_except_scroll())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn the_agent_works_step_by_step_and_then_the_server_is_back() {
        assert_eq!(frame(0.5), Phase::Down);
        assert_eq!(frame(1.7), Phase::Working(0));
        assert_eq!(frame(2.6), Phase::Working(1));
        assert_eq!(frame(4.3), Phase::Working(3));
        assert_eq!(frame(5.), Phase::Fixed);
    }

    #[test]
    fn a_step_shows_its_output_only_when_it_ends() {
        let steps = sample_steps(2.6);
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].state, parts::StepState::Done);
        assert!(!steps[0].output.is_empty());
        assert_eq!(steps[1].state, parts::StepState::Running);
        assert!(steps[1].output.is_empty());
    }
}
