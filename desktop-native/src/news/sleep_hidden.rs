//! `adormecer-ocultas`: a sessão oculta e parada ganha o selo "Adormecida"; a mensagem seguinte a reabre e ela volta a trabalhar.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app::{self, chrome, RowSub}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.5;
/// O prazo passou: o processo fecha e a linha ganha o selo.
const SLEEP: f32 = 1.6;
/// A mensagem chegou: a sessão reabre na mesma conversa e trabalha.
const WAKE: f32 = 4.2;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { asleep: bool, working: bool }

fn frame(t: f32) -> Frame {
    Frame { asleep: (SLEEP..WAKE).contains(&t), working: t >= WAKE }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [SLEEP, WAKE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

/// Altura fixa: o cartão não pula quando a segunda linha da sessão aparece.
const STAGE_HEIGHT: f32 = 228.;

/// A linha como a barra a desenha: marca do agente, nome com o selo e a segunda linha.
fn row(id: &str, name: String, provider: &'static str, state: &'static str, asleep: bool, sub: Option<(String, RowSub)>) -> Stateful<Div> {
    let avatar = chrome::session_avatar(&chrome::AvatarLook::new(provider, None, state, false), 30., None);
    let italic = state == "working";
    app::session_row_frame(SharedString::from(format!("news-sleep-row-{id}")), false)
        .child(avatar)
        .child(app::session_row_text()
            .child(div().flex().items_center().gap_2().child(app::session_row_name(name))
                .when(asleep, |el| el.child(app::badge(tr_shared("session_asleep_badge", &[]), theme::muted()))))
            .when_some(sub, |el, (text, kind)| el.child(app::session_row_sub(text, kind, theme::muted(), italic))))
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado com a oculta já adormecida.
    let f = if cx.reduce_motion() { Frame { asleep: true, working: false } } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let pad = if width < px(400.) { 6. } else { 14. };
    let list_width = (f32::from(width) - 2. * pad).min(300.);
    let (state, sub) = if f.working { ("working", Some((tr_shared("news_codex_agent_task", &[]), RowSub::Working))) } else { ("idle", None) };
    let list = div().w(px(list_width)).flex().flex_col().gap(px(2.))
        .child(row("a", tr_shared("news_tags_sample_session", &[]), "claude", "idle", false, None))
        .child(row("b", tr_shared("news_rename_sample_generic", &[]), "claude", "idle", false, None))
        .child(app::group_header_look(div(), tr("sidebar_hidden_group").replace("{n}", "1"), true, None, 0))
        .child(row("hidden", tr_shared("news_tags_sample_session_other", &[]), "codex", state, f.asleep, sub));
    div().id("news-sleep-hidden").relative().w(width).h(px(STAGE_HEIGHT)).p(px(pad)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(list)
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
    fn script_sleeps_the_hidden_row_and_wakes_it_working() {
        assert_eq!(frame(0.5), Frame { asleep: false, working: false });
        assert_eq!(frame(2.0), Frame { asleep: true, working: false });
        assert_eq!(frame(5.0), Frame { asleep: false, working: true });
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 1.1));
        assert!(close(until_next_step(2.0), 2.2));
        assert!(close(until_next_step(5.0), 1.5));
    }
}
