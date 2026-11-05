//! `sem-projeto`: um clique em "Sem projeto" tira a pasta do caminho e a conversa abre na pasta do usuário.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{assets::IconName, base::Selectable, component::ActiveTheme, prelude::FluentBuilder, *};
use crate::{app, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// O clique: a pílula acende por um instante.
const PRESS: f32 = 1.8;
/// A pasta vira "Sem projeto" e o botão sai.
const PICKED: f32 = 2.2;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { lit: bool, picked: bool }

fn frame(t: f32) -> Frame {
    Frame { lit: (PRESS..PICKED).contains(&t), picked: t >= PICKED }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [PRESS, PICKED, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
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

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no botão, antes do clique.
    let Frame { lit, picked } = if cx.reduce_motion() { frame(0.) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let folder = if picked { tr("new_chat_no_project") } else { tr_shared("news_tags_sample_session", &[]) };
    let pills = div().flex().justify_end().items_center().gap(px(2.)).pb(px(6.))
        .child(app::quiet_button("news-no-project-machine", IconName::Monitor, tr("news_login_browser_home"), true, cx).tab_stop(false))
        .child(app::quiet_button("news-no-project-folder", IconName::Folder, folder, true, cx).tab_stop(false))
        .when(!picked, |el| el.child(app::quiet_button("news-no-project", IconName::House, tr("new_chat_no_project"), false, cx).selected(lit).tab_stop(false)));
    let field = div().text_size(px(13.5)).min_h(px(22.)).flex().items_center().overflow_hidden()
        .child(div().truncate().text_color(cx.theme().muted_foreground).child(tr("composer").replace("{agent}", "Claude")));
    let send = app::send_button("send", false, tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field).child(app::composer_control_row([], [], send));
    div().id("news-no-project").w(width).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(pills).child(composer).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_button_lights_up_and_leaves_when_the_folder_changes() {
        assert_eq!(frame(0.5), Frame { lit: false, picked: false });
        assert_eq!(frame(1.9), Frame { lit: true, picked: false });
        assert_eq!(frame(3.), Frame { lit: false, picked: true });
    }
}
