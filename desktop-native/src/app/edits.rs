//! O diff das chamadas que editam arquivo, dentro da conversa: as linhas da aba Git, com quebra, as duas numerações
//! e o realce do tree-sitter que os blocos de código já usam.
use super::*;
use super::git::{Kind, line_row};
use crate::editdiff::{self, Edit, Op};
use gpui_kit::component::highlighter::{HighlightTheme, SyntaxHighlighter};
use std::{cell::RefCell, ops::Range, rc::Rc};

/// Linhas desenhadas por chamada: um Write de milhares de linhas travaria a conversa. O resto vai pelo Copiar.
pub(super) const SHOWN_MAX: usize = 400;

type Marks = Vec<(Range<usize>, HighlightStyle)>;
/// Realce de cada lado (antigo, novo) de cada edição, por linha.
pub(super) type Painted = Rc<Vec<(Vec<Marks>, Vec<Marks>)>>;

thread_local! {
    /// Por chamada e tema: o parse roda uma vez, não a cada quadro.
    static PAINTED: RefCell<HashMap<(String, usize), Painted>> = RefCell::new(HashMap::new());
    /// Um por linguagem: criar um compila a consulta de realce, e isto roda na thread da tela, a cada lado de cada edição.
    static HIGHLIGHTERS: RefCell<HashMap<String, SyntaxHighlighter>> = RefCell::new(HashMap::new());
}

/// "+5 −2" da chamada, na mesma forma do fim da linha nos Chips e na Árvore.
pub(super) fn totals(call: &ChatEvent) -> Option<AnyElement> {
    let (added, removed) = editdiff::totals(&editdiff::of(call)?);
    Some(div().flex_shrink_0().flex().gap(px(6.)).font_family(theme::MONO).text_size(px(12.))
        .child(div().text_color(theme::success()).child(format!("+{added}")))
        .when(removed > 0, |el| el.child(div().text_color(theme::removed()).child(format!("−{removed}"))))
        .into_any_element())
}

/// Realce das `upto` primeiras linhas de um lado. Linguagem sem gramática não pinta nada.
fn paint(lang: &str, text: &str, upto: usize, theme: &HighlightTheme) -> Vec<Marks> {
    if upto == 0 || lang == "text" { return Vec::new(); }
    let starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let code = &text[..starts.get(upto).map_or(text.len(), |s| s - 1)];
    let mut out = vec![Vec::new(); upto.min(starts.len())];
    HIGHLIGHTERS.with(|highlighters| {
        let mut highlighters = highlighters.borrow_mut();
        let highlighter = highlighters.entry(lang.to_owned()).or_insert_with(|| SyntaxHighlighter::new(lang));
        highlighter.update(None, &Rope::from_str(code), None);
        for (range, style) in highlighter.styles(&(0..code.len()), theme) {
            // Um nó pode atravessar linhas (comentário de bloco, string longa): cada linha leva o seu pedaço.
            let mut line = starts.partition_point(|&s| s <= range.start).saturating_sub(1);
            let mut start = range.start;
            while start < range.end && line < out.len() {
                let end = range.end.min(starts.get(line + 1).map_or(code.len(), |s| s - 1));
                if end > start { out[line].push((start - starts[line]..end - starts[line], style)); }
                line += 1;
                start = starts.get(line).copied().unwrap_or(usize::MAX);
            }
        }
    });
    out
}

pub(super) fn painted(call: &ChatEvent, edits: &[Edit], cx: &App) -> Painted {
    let theme = cx.theme().highlight_theme.clone();
    let patched = edits.first().is_some_and(|e| e.patched);
    let key = (format!("{}#{}", call.id, patched as u8), Arc::as_ptr(&theme) as usize);
    if let Some(found) = PAINTED.with(|p| p.borrow().get(&key).cloned()) { return found; }
    let mut left = SHOWN_MAX;
    let sides = edits.iter().map(|edit| {
        let lang = super::files::file_language(&edit.path);
        let shown = &edit.lines[..edit.lines.len().min(left)];
        left -= shown.len();
        let upto = |number: fn(&editdiff::Line) -> Option<u32>, start: u32| shown.iter().filter_map(number).max()
            .map_or(0, |last| last.saturating_add(1).saturating_sub(start) as usize);
        (paint(lang, &edit.old, upto(|l| l.old, edit.start.0), &theme), paint(lang, &edit.new, upto(|l| l.new, edit.start.1), &theme))
    }).collect::<Vec<_>>();
    let sides = Rc::new(sides);
    PAINTED.with(|p| {
        let mut p = p.borrow_mut();
        // ponytail: esvazia tudo ao passar de 128; um LRU entra se reabrir diffs antigos ficar lento.
        if p.len() >= 128 { p.clear(); }
        p.insert(key, sides.clone());
    });
    sides
}

/// O caminho com o nome do arquivo em destaque; quem encolhe é a pasta.
fn path_label(path: &str) -> Div {
    let (dir, base) = path.rsplit_once('/').map_or(("", path), |(dir, base)| (dir, base));
    div().flex_1().min_w_0().flex().items_center().gap(px(6.)).font_family(theme::MONO).text_size(px(12.))
        .child(crate::fileicons::citation_icon(base))
        .when(!dir.is_empty(), |el| el.child(div().min_w_0().truncate().text_color(theme::faint()).child(format!("{dir}/"))))
        .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(base.to_owned()))
}

/// O diff da chamada, um cartão por edição; `None` quando a chamada não edita arquivo (fica a entrada crua).
pub(super) fn card(call: &ChatEvent, result: Option<&ChatEvent>, cx: &App) -> Option<AnyElement> {
    let edits = editdiff::shown(call, result)?;
    let marks = painted(call, &edits, cx);
    let total: usize = edits.iter().map(|e| e.lines.len()).sum();
    let mut left = SHOWN_MAX;
    let cards = edits.iter().zip(marks.iter()).enumerate().map(|(n, (edit, (old_marks, new_marks)))| {
        let shown = &edit.lines[..edit.lines.len().min(left)];
        left -= shown.len();
        let full = edit.new.clone();
        let counts = if edit.added + edit.removed == 0 {
            div().text_size(px(12.)).text_color(theme::muted()).child(activity::web("editdiff_sem_mudanca"))
        } else {
            div().flex().gap(px(6.)).font_family(theme::MONO).text_size(px(12.))
                .child(div().text_color(theme::success()).child(format!("+{}", edit.added)))
                .child(div().text_color(theme::removed()).child(format!("−{}", edit.removed)))
        };
        let step = (edits.len() > 1).then(|| crate::i18n::tr_web("editdiff_edicao", &HashMap::from([
            ("n".to_owned(), (n + 1).to_string()), ("total".to_owned(), edits.len().to_string())]))).flatten();
        let header = div().flex().items_center().gap_2().pl_3().pr_1().py(px(4.)).border_b_1().border_color(theme::border())
            .child(path_label(&edit.path))
            .when_some(step, |el, step| el.child(div().flex_shrink_0().text_size(px(11.)).text_color(theme::muted()).child(step)))
            .child(counts.flex_shrink_0())
            .child(Button::new(SharedString::from(format!("copy-diff-{}-{n}", call.id))).ghost().xsmall().icon(IconName::Copy)
                .tooltip(tr("copy_new_text")).accessibility_label(tr("copy_new_text"))
                .on_click(move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(full.clone()))));
        let rows = shown.iter().zip(&edit.spans).map(|(line, spans)| {
            let (kind, side, number, start, tint) = match line.op {
                Op::Add => (Kind::Add, new_marks, line.new, edit.start.1, Some(theme::success())),
                Op::Del => (Kind::Del, old_marks, line.old, edit.start.0, Some(theme::removed())),
                Op::Same => (Kind::Context, new_marks, line.new, edit.start.1, None),
            };
            let len = line.text.len();
            // O realce foi medido no texto do lado; a linha do diff perde o `\r` do fim, então nada passa do tamanho dela.
            let syntax = number.and_then(|n| side.get(n.saturating_sub(start) as usize)).into_iter().flatten()
                .filter(|(r, _)| r.start < len).map(|(r, s)| (r.start..r.end.min(len), *s));
            let words = tint.into_iter().flat_map(|tint| spans.iter().filter(|r| r.start < len).map(move |r| (r.start..r.end.min(len),
                HighlightStyle { background_color: Some(tint.opacity(0.28)), ..Default::default() })));
            let highlights = gpui::combine_highlights(syntax, words).collect::<Vec<_>>();
            line_row(kind, line.old, line.new, StyledText::new(line.text.clone()).with_highlights(highlights), true)
        }).collect::<Vec<_>>();
        div().flex().flex_col().rounded(px(8.)).border_1().border_color(theme::border()).bg(theme::inset()).overflow_hidden()
            .child(header)
            .child(div().flex().flex_col().py_1().children(rows))
            .into_any_element()
    }).collect::<Vec<_>>();
    let note = (total > SHOWN_MAX).then(|| tr("diff_clipped").replace("{shown}", &SHOWN_MAX.to_string()).replace("{total}", &total.to_string()));
    Some(div().flex().flex_col().gap_2().children(cards)
        .when_some(note, |el, note| el.child(div().text_xs().text_color(theme::muted()).child(note)))
        .into_any_element())
}

#[cfg(test)]
mod tests {
    use gpui_kit::component::highlighter::HighlightTheme;

    #[test]
    fn reused_highlighter_paints_like_a_new_one() {
        let theme = HighlightTheme::default_dark();
        let (long, short) = ("fn main() {\n    let answer = 42;\n    println!(\"{answer}\");\n}", "struct Point { x: i32 }");
        let fresh = super::paint("rust", short, 1, &theme);
        assert!(!fresh[0].is_empty());
        super::paint("rust", long, 4, &theme);
        assert_eq!(super::paint("rust", short, 1, &theme), fresh);
    }
}
