//! `modo-da-conversa`: na tela inicial, a pílula do terminal ao lado da máquina e da conta troca o modo só desta conversa.

use std::time::Instant;
use gpui_kit::{assets::IconName, base::Selectable, component::ActiveTheme, *};
use crate::{app::{self, chrome}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 6.;
/// O clique na pílula abre o menu do modo.
const OPEN: f32 = 1.2;
/// A escolha passa para "No terminal".
const PICK: f32 = 2.8;
/// O menu fecha e a pílula mostra a escolha.
const CLOSE: f32 = 3.4;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { menu: bool, terminal: bool }

fn frame(t: f32) -> Frame {
    Frame { menu: (OPEN..CLOSE).contains(&t), terminal: t >= PICK }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [OPEN, PICK, CLOSE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: quadro parado no menu aberto, a escolha ainda no padrão.
    let Frame { menu, terminal } = if cx.reduce_motion() { frame(OPEN) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        frame(t)
    };
    let field = div().text_size(px(13.5)).min_h(px(22.)).flex().items_center().overflow_hidden()
        .child(div().truncate().text_color(cx.theme().muted_foreground).child(tr("composer").replace("{agent}", "Claude")));
    let send = app::send_button("send", false, tr("send"), cx).tab_stop(false);
    let composer = app::composer_card(false).w_full().child(field).child(app::composer_control_row([], [], send));
    let mode = tr(if terminal { "create_mode_tmux" } else { "create_mode_headless" });
    let pills = div().flex().items_center().gap(px(2.)).pt(px(2.)).pl(px(6.))
        .child(app::quiet_button("news-mode-machine", IconName::Monitor, tr("news_login_browser_home"), true, cx).tab_stop(false))
        .child(app::quiet_button("news-mode-account", IconName::CircleUser, tr_shared("news_tags_sample_account_work", &[]), true, cx).tab_stop(false))
        .child(app::quiet_button("news-mode-pill", IconName::SquareTerminal, mode, true, cx).selected(menu).tab_stop(false));
    let list = menu.then(|| div().pl(px(6.)).pt(px(4.)).child(div().w_full().max_w(px(300.)).child(chrome::popover(div().p(px(4.)).flex().flex_col().gap(px(2.))
        .child(app::popup::title(tr("create_mode"), None))
        .child(app::mode_row("news-mode-tmux", terminal, false, false).tab_stop(false))
        .child(app::mode_row("news-mode-headless", !terminal, true, false).tab_stop(false)).into_any_element(), false))));
    // Altura do quadro com o menu aberto: o cartão não pula entre os passos.
    div().id("news-new-chat-mode").w(width).min_h(px(300.)).overflow_hidden().p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(composer).child(pills).children(list).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_menu_opens_the_choice_moves_and_the_pill_keeps_it() {
        assert_eq!(frame(0.5), Frame { menu: false, terminal: false });
        assert_eq!(frame(2.), Frame { menu: true, terminal: false });
        assert_eq!(frame(3.), Frame { menu: true, terminal: true });
        assert_eq!(frame(4.), Frame { menu: false, terminal: true });
    }
}
