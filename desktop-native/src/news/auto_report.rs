//! `relatorio-automatico`: com o interruptor ligado, a máquina cuja atualização falha manda o relatório sozinha e a
//! linha dela mostra o código.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{assets::IconName, component::switch::Switch, *};
use crate::{app, i18n::tr_shared, theme, update::status_line};

const CYCLE: f32 = 6.5;
/// O interruptor é ligado.
const ON: f32 = 0.7;
/// A atualização, que vinha andando, falha.
const FAILED: f32 = 2.0;
/// O relatório sai sozinho.
const SENDING: f32 = 2.9;
/// O código chega na linha da máquina.
const SENT: f32 = 4.0;
/// Formato real de código de relatório, sem ser o de ninguém.
const SAMPLE_CODE: &str = "k7m2q4xa";

#[derive(Clone, Copy, Debug, PartialEq)]
enum Line { Running, Failed, Sending, Sent }

/// Quadro do roteiro no instante `t` do ciclo: o interruptor e a linha da máquina.
fn frame(t: f32) -> (bool, Line) {
    let line = if t >= SENT { Line::Sent } else if t >= SENDING { Line::Sending } else if t >= FAILED { Line::Failed } else { Line::Running };
    (t >= ON, line)
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [ON, FAILED, SENDING, SENT, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

thread_local! {
    /// O redesenho já marcado: cada render só marca outro se o dele cair antes.
    static NEXT_REPAINT: Cell<Option<Instant>> = const { Cell::new(None) };
}

/// Redesenha a janela no próximo degrau. Só é marcado enquanto a animação é desenhada: fechou o modal, para.
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

/// O texto da linha da máquina. No cartão estreito o relatório substitui a falha, que não caberia junto.
fn line_text(line: Line, compact: bool) -> String {
    let failed = tr_shared("update_all_failed", &[("erro", &tr_shared("news_auto_report_sample_error", &[]))]);
    let report = match line {
        Line::Running => return tr_shared("update_all_running", &[("texto", &tr_shared("news_auto_report_sample_step", &[])), ("passo", "2"), ("total", "3")]),
        Line::Failed => return failed,
        Line::Sending => tr_shared("update_report_pending", &[]),
        Line::Sent => tr_shared("update_report_sent", &[("code", SAMPLE_CODE)]),
    };
    if compact { report } else { format!("{failed} · {report}") }
}

/// Abaixo desta largura (o cartão deitado do "Atualizar?") a linha de configuração não cabe: fica só a da máquina.
const COMPACT_BELOW: f32 = 240.;
/// Altura da linha da máquina com o texto mais longo: o cartão não pula quando o relatório entra.
const STATUS_HEIGHT: f32 = 62.;
const COMPACT_STATUS_HEIGHT: f32 = 86.;

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no resultado.
    let (on, line) = if cx.reduce_motion() { (true, Line::Sent) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let compact = width < px(COMPACT_BELOW);
    let color = if line == Line::Running { theme::muted() } else { theme::danger() };
    let status = div().h(px(if compact { COMPACT_STATUS_HEIGHT } else { STATUS_HEIGHT })).overflow_hidden().text_sm()
        .child(status_line(tr_shared("news_auto_report_sample_machine", &[]), line_text(line, compact), color));
    let stage = div().id("news-auto-report").w(width).p(px(if compact { 8. } else { 14. })).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(12.)).text_color(theme::text());
    if compact { return stage.child(status).into_any_element(); }
    // Miniatura só leitura: o interruptor fica fora do Tab, e a tampa por cima não deixa o mouse levar o foco para ele.
    let toggle = Switch::new("news-auto-report-switch").checked(on).tab_stop(false).accessibility_label(tr_shared("native_auto_report_title", &[]));
    let hint = tr_shared(if on { "auto_report_on" } else { "auto_report_off" }, &[]);
    stage.child(div().relative().child(app::settings_box().child(app::setting_row(false, IconName::FileText, tr_shared("native_auto_report_title", &[]),
                div().whitespace_normal().child(hint), true, toggle.into_any_element())))
            .child(div().absolute().top_0().left_0().size_full().block_mouse_except_scroll()))
        .child(status)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn script_turns_on_then_fails_then_shows_the_code() {
        assert_eq!(frame(0.3), (false, Line::Running));
        assert_eq!(frame(1.0), (true, Line::Running));
        assert_eq!(frame(2.2), (true, Line::Failed));
        assert_eq!(frame(3.2), (true, Line::Sending));
        assert_eq!(frame(4.5), (true, Line::Sent));
    }

    #[test]
    fn repaint_waits_for_the_next_step() {
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.2), 0.5));
        assert!(close(until_next_step(1.0), 1.0));
        assert!(close(until_next_step(4.5), 2.0));
    }
}
