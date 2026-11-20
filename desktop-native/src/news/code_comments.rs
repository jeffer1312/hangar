//! `comentar-linhas`: arrastar sobre os números das linhas abre o comentário, e ele vai para o campo de mensagem.

use std::time::Instant;
use gpui_kit::{assets::IconName, component::{Disableable, Sizable, button::{Button, ButtonVariants}}, prelude::FluentBuilder, *};
use crate::{app, i18n::tr, theme};

const CYCLE: f32 = 7.5;
const DRAG: (f32, f32) = (1., 2.);
const TYPE: (f32, f32) = (2.3, 4.3);
const SENT: f32 = 5.;
const LINE: f32 = 20.;
/// Linhas marcadas pelo arrasto (1-based).
const FIRST: usize = 3;
const LAST: usize = 5;

/// O trecho de exemplo, já com o nome de destaque de cada pedaço (o mesmo nome que o tree-sitter dá).
const CODE: [&[(&str, &str)]; 8] = [
    &[("fn", "keyword"), (" ", ""), ("ler", "function"), ("(", "punctuation"), ("caminho", "variable"), (": &", "punctuation"), ("str", "type"), (") -> ", "punctuation"), ("Texto", "type"), (" {", "punctuation")],
    &[("    ", ""), ("let", "keyword"), (" ", ""), ("limite", "variable"), (" = ", "operator"), ("512", "number"), (" * ", "operator"), ("1024", "number"), (";", "punctuation")],
    &[("    ", ""), ("let", "keyword"), (" ", ""), ("dados", "variable"), (" = ", "operator"), ("abrir", "function"), ("(", "punctuation"), ("caminho", "variable"), (")?;", "punctuation")],
    &[("    ", ""), ("if", "keyword"), (" ", ""), ("dados", "variable"), (".", "punctuation"), ("len", "function"), ("() > ", "punctuation"), ("limite", "variable"), (" {", "punctuation")],
    &[("        ", ""), ("return", "keyword"), (" ", ""), ("cortar", "function"), ("(", "punctuation"), ("dados", "variable"), (", ", "punctuation"), ("\"fim\"", "string"), (");", "punctuation")],
    &[("    }", "punctuation")],
    &[("    ", ""), ("dados", "variable")],
    &[("}", "punctuation")],
];

/// Quadro no instante `t`: até qual linha o arrasto chegou, quantos caracteres do comentário já foram digitados e se
/// o comentário já foi para o campo de mensagem.
fn frame(t: f32, len: usize) -> (Option<usize>, Option<usize>, bool) {
    if t >= SENT { return (None, None, true); }
    if t < DRAG.0 { return (None, None, false); }
    let reached = (FIRST + ((t - DRAG.0) / (DRAG.1 - DRAG.0) * (LAST - FIRST + 1) as f32) as usize).min(LAST);
    let typed = (t >= TYPE.0).then(|| (((t - TYPE.0) / (TYPE.1 - TYPE.0)).min(1.) * len as f32) as usize);
    (Some(reached), (t >= DRAG.1).then(|| typed.unwrap_or(0)), false)
}

pub(super) fn render(started: Instant, width: Pixels, window: &mut Window, cx: &mut App) -> AnyElement {
    let comment = tr("news_code_comment_sample");
    let chars = comment.chars().count();
    let (reached, typed, sent) = if cx.reduce_motion() { (Some(LAST), Some(chars), false) } else {
        let t = started.elapsed().as_secs_f32() % CYCLE;
        super::repaint_in(if (DRAG.0..TYPE.1).contains(&t) { 0.08 } else { [DRAG.0, SENT, CYCLE].into_iter().find(|b| *b > t).unwrap_or(CYCLE) - t }, window, cx);
        frame(t, chars)
    };
    let palette = crate::code_theme::highlight_theme(theme::is_dark());
    let color = |name: &str| palette.style(name).and_then(|style| style.color).unwrap_or(theme::text());
    let selected = |n: usize| reached.is_some_and(|last| (FIRST..=last).contains(&n));
    let marked = |n: usize| sent && (FIRST..=LAST).contains(&n);
    let lines = CODE.iter().enumerate().map(|(ix, parts)| {
        let n = ix + 1;
        div().h(px(LINE)).flex().items_center()
            .when(selected(n), |el| el.bg(theme::accent().opacity(0.18)))
            .child(div().w(px(4.)).h_full().when(marked(n), |el| el.bg(theme::accent())))
            .child(div().w(px(30.)).pr(px(10.)).text_right().text_color(if selected(n) { theme::text() } else { theme::faint() }).child(n.to_string()))
            .children(parts.iter().map(|(text, name)| div().whitespace_nowrap().text_color(if name.is_empty() { theme::text() } else { color(name) }).child(*text)))
    });
    let label = tr("file_comment_on").replace("{lines}", &crate::context_refs::lines_label(FIRST as u32, LAST as u32));
    let card = typed.map(|typed| {
        let text: String = comment.chars().take(typed).collect();
        let field = div().min_h(px(38.)).px_2().py_1().rounded_md().border_1().border_color(theme::accent()).bg(theme::inset())
            .text_size(px(12.5)).text_color(if text.is_empty() { theme::faint() } else { theme::text() })
            .child(if text.is_empty() { tr("file_comment_placeholder") } else { text });
        let actions = div().flex().items_center().gap_1()
            .child(Button::new("news-comment-cancel").ghost().xsmall().label(tr("file_comment_cancel")).tab_stop(false))
            .child(Button::new("news-comment-send").primary().xsmall().label(tr("file_comment_send")).disabled(typed == 0).tab_stop(false));
        div().absolute().top(px(10. + LAST as f32 * LINE + 4.)).left(px(44.)).right(px(10.))
            .child(app::comment_card(label.clone(), field, actions))
    });
    let chip = sent.then(|| app::ref_chip("news-comment-chip".into(), IconName::Code,
        format!("ler.rs {}", crate::context_refs::lines_label(FIRST as u32, LAST as u32)), false));
    div().id("news-code-comments").relative().w(width).h(px(250.)).p(px(10.)).rounded(px(9.)).border_1()
        .border_color(theme::border().alpha(0.06)).bg(theme::surface()).overflow_hidden()
        .child(div().font_family(theme::MONO).text_size(px(12.)).children(lines))
        .children(card)
        .children(chip.map(|chip| div().absolute().left(px(10.)).bottom(px(10.)).child(chip)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn script_drags_types_and_sends() {
        assert_eq!(frame(0.5, 10), (None, None, false));
        assert_eq!(frame(1.1, 10).0, Some(FIRST));
        assert_eq!(frame(1.9, 10).0, Some(LAST));
        assert_eq!(frame(2.1, 10).1, Some(0));
        assert_eq!(frame(4.5, 10).1, Some(10));
        assert_eq!(frame(5.5, 10), (None, None, true));
    }
}
