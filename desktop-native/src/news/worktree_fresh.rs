//! `worktree-atualizada`: a branch está atrás, e "Parte de" passa para a remota, com o aviso do commit que fica de fora.

use std::{cell::Cell, time::{Duration, Instant}};
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app, i18n::tr_shared, theme};

const CYCLE: f32 = 7.;
/// O fetch mostra a local atrás: a base vai para a remota.
const FETCHED: f32 = 1.6;
/// A lista abre com as duas.
const LIST: f32 = 3.8;

#[derive(Clone, Copy, Debug, PartialEq)]
struct Frame { fresh: bool, list: bool }

fn frame(t: f32) -> Frame {
    Frame { fresh: t >= FETCHED, list: t >= LIST }
}

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 {
    [FETCHED, LIST, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t
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
    // Movimento reduzido: quadro parado na remota já marcada, com o aviso.
    let Frame { fresh, list } = if cx.reduce_motion() { frame(FETCHED) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        schedule_repaint(t, window, cx);
        frame(t)
    };
    let (main, upstream) = (tr_shared("news_branch_sample_main", &[]), tr_shared("news_branch_sample_upstream", &[]));
    let base = if fresh { upstream.clone() } else { main.clone() };
    let warning = fresh.then(|| app::worktree_warning(tr_shared("worktree_menu_commits_left_out",&[("branch", &main), ("n", "1")])));
    let field = div().flex().items_center().gap_3().child(app::worktree_field_label("worktree_menu_parte_de"))
        .child(div().flex_1().min_w_0().child(app::base_button("news-worktree-base", base).tab_stop(false)));
    let rows = div().flex().flex_col()
        .child(app::base_row("news-worktree-upstream", fresh, upstream, 0).tab_stop(false))
        .child(app::base_row("news-worktree-local", !fresh, main, 112).tab_stop(false));
    // Altura do quadro mais alto (a lista): o cartão não pula entre os passos.
    div().id("news-worktree-fresh").w(width).min_h(px(190.)).p(px(14.)).rounded(px(9.)).border_1().border_color(theme::border().alpha(0.06))
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .flex().flex_col().gap(px(10.))
        .child(field).when(list, |el| el.child(rows)).children(warning).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob pode trazer o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn the_base_moves_to_the_upstream_and_then_the_list_opens() {
        assert_eq!(frame(0.5), Frame { fresh: false, list: false });
        assert_eq!(frame(2.), Frame { fresh: true, list: false });
        assert_eq!(frame(4.), Frame { fresh: true, list: true });
    }
}
