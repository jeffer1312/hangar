//! `codex-atividade`: na conversa do Codex, cada subagente aparece no mesmo cartão do Claude, rodando e depois concluído.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{base::TextView, *};
use crate::{app::{self, chrome}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 8.5;
/// Movimento reduzido: os dois cartões rodando.
const STILL: f32 = 3.5;
const TEXT_AT: f32 = 0.2;

struct Agent { name: &'static str, task: &'static str, starts: f32, ends: f32 }

const AGENTS: [Agent; 2] = [
    Agent { name: "explorer", task: "news_codex_agent_explore", starts: 0.8, ends: 4.8 },
    Agent { name: "worker", task: "news_codex_agent_task", starts: 1.6, ends: 6.6 },
];

#[derive(Debug, PartialEq)]
enum Card { Hidden, Running(u64), Done(u64) }

/// Quadro do roteiro no instante `t`: os segundos são os do cartão (rodando há, concluído em).
fn frame(t: f32) -> [Card; 2] {
    AGENTS.each_ref().map(|a| if t < a.starts { Card::Hidden }
        else if t < a.ends { Card::Running((t - a.starts) as u64) } else { Card::Done((a.ends - a.starts) as u64) })
}

/// Segundos até o próximo degrau: troca de cartão ou o segundo seguinte de quem está rodando.
fn until_next_step(t: f32) -> f32 {
    let ticks = AGENTS.iter().filter(|a| t >= a.starts && t < a.ends).map(|a| a.starts + (t - a.starts).floor() + 1.);
    AGENTS.iter().flat_map(|a| [a.starts, a.ends]).chain([TEXT_AT, CYCLE]).chain(ticks).filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

thread_local! {
    static NEXT_REPAINT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Só é marcado enquanto a animação é desenhada: fechou o modal, para.
fn schedule_repaint(t: f32, window: &mut Window, cx: &mut App) {
    // Margem para o timer não acordar um pouco antes do degrau e desenhar o quadro velho.
    let at = Instant::now() + Duration::from_secs_f32(until_next_step(t)) + Duration::from_millis(5);
    if NEXT_REPAINT.get().is_some_and(|next| next > Instant::now() && next <= at) { return; }
    NEXT_REPAINT.set(Some(at));
    window.spawn(cx, async move |cx| {
        cx.background_executor().timer(at.saturating_duration_since(Instant::now())).await;
        let _ = cx.update(|window, _| window.refresh());
    }).detach();
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") só os cartões cabem.
const COMPACT_BELOW: f32 = 240.;

pub(super) fn render(started: Instant, width: Pixels, animate: bool, window: &mut Window, cx: &mut App) -> AnyElement {
    let t = if cx.reduce_motion() { STILL } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        if animate { schedule_repaint(t, window, cx); }
        t
    };
    let cards = AGENTS.iter().zip(frame(t)).enumerate().filter_map(|(i, (agent, card))| {
        let (timer, mark) = match card {
            Card::Hidden => return None,
            Card::Running(secs) => (app::agent_clock().child(tr("agent_running_for")).child(chrome::format_elapsed(Duration::from_secs(secs))),
                Some(chrome::WorkingMark::new(SharedString::from(format!("news-agent-mark-{i}")), 12., theme::accent()).into_any_element())),
            Card::Done(secs) => (app::agent_clock().child(tr("agent_finished_in").replace("{time}", &chrome::format_elapsed(Duration::from_secs(secs)))), None),
        };
        // Miniatura só leitura: o cartão da conversa, sem ação nem foco.
        Some(app::agent_card(SharedString::from(format!("news-agent-{i}")), agent.name.into(), tr_shared(agent.task, &[]), None, false,
            Some(timer.into_any_element()), mark, None, cx).tab_stop(false).into_any_element())
    }).collect::<Vec<_>>();
    let stage = div().id("news-codex-activity").w_full().rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06)).bg(theme::inset())
        .overflow_hidden().flex().flex_col().gap_2();
    if width < px(COMPACT_BELOW) {
        return stage.h(px(74.)).p(px(8.)).children(cards).into_any_element();
    }
    let text = (t >= TEXT_AT).then(|| TextView::markdown("news-codex-text", tr_shared("news_codex_split", &[])).scrollable(false).style(theme::conversation_markdown(cx)));
    // Altura fixa no maior quadro: o cartão da novidade não pula quando os subagentes entram.
    stage.h(px(150.)).px_4().py(px(14.)).children(text).children(cards).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_opens_both_cards_and_finishes_them_in_order() {
        assert_eq!(frame(0.5), [Card::Hidden, Card::Hidden]);
        assert_eq!(frame(STILL), [Card::Running(2), Card::Running(1)]);
        assert_eq!(frame(5.), [Card::Done(4), Card::Running(3)]);
        assert_eq!(frame(8.), [Card::Done(4), Card::Done(5)]);
    }

    #[test]
    fn repaint_waits_for_the_next_second_or_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 0.3));
        assert!(close(until_next_step(1.0), 0.6));
        assert!(close(until_next_step(7.0), 1.5));
    }
}
