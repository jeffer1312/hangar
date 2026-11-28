//! `filhas-ligadas`: fechar a conversa que abriu outras pergunta se fecha as filhas junto; "Fechar só esta" para o que
//! elas faziam e as deixa abertas, com a marca "sem pai" e o nome de quem as abriu.

use std::time::Instant;
use gpui_kit::{prelude::FluentBuilder, *};
use crate::{app::{self, chrome, RowSub}, i18n::{tr, tr_shared}, theme};

const CYCLE: f32 = 7.;
/// O Fechar da conversa que abriu as filhas: a pergunta aparece.
const ASK: f32 = 1.3;
/// "Fechar só esta": a principal sai, e as filhas param e ficam com a marca.
const CLOSE: f32 = 3.9;
/// Altura do quadro com a pergunta aberta: o cartão não pula entre os passos.
const STAGE_HEIGHT: f32 = 236.;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Step { Working, Asking, Orphaned }

fn step(t: f32) -> Step { if t >= CLOSE { Step::Orphaned } else if t >= ASK { Step::Asking } else { Step::Working } }

/// Segundos até o próximo instante em que o quadro muda: o roteiro anda em degraus, não a cada quadro.
fn until_next_step(t: f32) -> f32 { [ASK, CLOSE, CYCLE].into_iter().filter(|b| *b > t).fold(CYCLE, f32::min) - t }

/// A linha como a barra a desenha: marca do agente, nome, o que está fazendo e, embaixo, a marca de sem pai.
fn row(id: &str, name: String, working: Option<String>, orphan: Option<String>) -> Stateful<Div> {
    let avatar = chrome::session_avatar(&chrome::AvatarLook::new("claude", None, if working.is_some() { "working" } else { "idle" }, false), 30., None);
    app::session_row_frame(SharedString::from(format!("news-linked-row-{id}")), false)
        .child(avatar)
        .child(app::session_row_text()
            .child(div().flex().items_center().gap_2().min_w_0().child(app::session_row_name(name)))
            .when_some(working, |el, text| el.child(app::session_row_sub(text, RowSub::Working, theme::muted(), true)))
            .when_some(orphan, |el, orphan| el.child(div().flex().text_size(px(11.5)).child(app::badge(orphan, theme::muted())))))
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    // Movimento reduzido: o fim, com as filhas abertas e a marca de quem as abriu.
    let step = if cx.reduce_motion() { Step::Orphaned } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(until_next_step(t), window, cx);
        self::step(t)
    };
    let pad = if width < px(400.) { 6. } else { 14. };
    let list_width = (f32::from(width) - 2. * pad).min(340.);
    let parent = tr_shared("news_linked_sample_parent", &[]);
    let kids = [("tests", tr_shared("news_linked_sample_child_tests", &[]), tr_shared("news_linked_sample_task_tests", &[])),
        ("docs", tr_shared("news_linked_sample_child_docs", &[]), tr_shared("news_linked_sample_task_docs", &[]))];
    let n = kids.len().to_string();
    let orphaned = step == Step::Orphaned;
    let orphan = orphaned.then(|| tr_shared("session_orphaned_from", &[("name", &parent)]));
    let list = div().w(px(list_width)).flex().flex_col().gap(px(2.))
        .when(!orphaned, |el| el.child(row("parent", parent.clone(), Some(tr_shared("news_linked_sample_task_parent", &[])), None)))
        .children(kids.into_iter().map(|(id, name, task)| row(id, name, (!orphaned).then_some(task), orphan.clone())));
    // A pergunta do Fechar, com o texto e os botões do diálogo de verdade.
    let ask = (step == Step::Asking).then(|| {
        let question = tr_shared("session_close_children_question", &[("name", &parent), ("n", &n)]);
        let buttons = app::close_children_buttons(&n).map(|button| button.tab_stop(false));
        let card = div().w(px((f32::from(width) - 24.).min(420.))).p(px(18.)).flex().flex_col().gap(px(14.))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_size(px(15.)).child(tr("sidebar_close_title")))
            .child(app::close_children_text(parent.clone(), question))
            .child(app::close_children_footer(buttons));
        div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().bg(theme::background().alpha(0.45))
            .child(chrome::popover(card.into_any_element(), false))
    });
    div().id("news-linked-children").relative().w(width).h(px(STAGE_HEIGHT)).p(px(pad)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).overflow_hidden().text_color(theme::text())
        .bg(linear_gradient(180., linear_color_stop(theme::chrome(), 0.), linear_color_stop(theme::background(), 1.)))
        .child(list)
        .children(ask)
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
    fn closing_asks_then_only_the_parent_leaves() {
        assert_eq!(step(0.5), Step::Working);
        assert_eq!(step(2.), Step::Asking);
        assert_eq!(step(5.), Step::Orphaned);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-4;
        assert!(close(until_next_step(0.5), 0.8));
        assert!(close(until_next_step(2.), 1.9));
        assert!(close(until_next_step(5.), 2.));
    }
}
