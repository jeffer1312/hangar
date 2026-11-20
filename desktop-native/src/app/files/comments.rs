//! Comentário em linhas, como no T3: arrastar sobre os números das linhas marca o trecho e abre o cartão embaixo da
//! última linha; o comentário vai para o campo de mensagem como bloco `code`, e as linhas ficam marcadas na margem
//! enquanto o bloco estiver lá.
use super::*;
use gpui_kit::component::input::{GutterDecoration, GutterDecorationCollection, RangeDecoration, RangeDecorationCollection, RangeDecorationStyle};
use std::{cell::Cell, rc::Rc};

/// Faixa da margem junto do texto que fica com as setas de dobrar o código (`FOLD_ICON_HITBOX_WIDTH` do kit).
const FOLD_STRIP: f32 = 18.;

pub(super) struct LineComments {
    drag: Option<u32>,
    draft: Option<Draft>,
    marks: GutterDecorationCollection,
    /// As linhas do rascunho, pintadas enquanto o cartão está aberto.
    selected: RangeDecorationCollection,
    /// Onde o editor está na janela, medido no quadro anterior: posiciona o cartão sob a linha.
    area: Rc<Cell<Bounds<Pixels>>>,
}

struct Draft { start: u32, end: u32, input: Entity<TextareaState>, _keys: Subscription }

impl LineComments {
    pub(super) fn new(editor: &Entity<EditorState>, cx: &mut App) -> Self {
        let (marks, selected) = editor.update(cx, |state, cx| {
            state.set_gutter_decorations_width(px(4.), cx);
            (state.create_gutter_decorations_collection(Vec::new(), cx), state.create_range_decorations_collection(Vec::new(), cx))
        });
        Self { drag: None, draft: None, marks, selected, area: Rc::default() }
    }
}

/// Linha do arquivo (0-based) na altura `y` da janela, pelas linhas que o editor desenhou.
fn line_at(editor: &EditorState, y: Pixels) -> Option<u32> {
    let bounds = editor.text_bounds()?;
    let height = editor.line_height()?;
    if height <= px(0.) { return None; }
    let row = ((y - bounds.origin.y) / height).floor().max(0.) as usize;
    let last = editor.text().lines_len().saturating_sub(1);
    Some(editor.display_map().display_row_to_buffer_line(row).min(last) as u32)
}

/// O ponto está nos números da linha, fora da faixa das setas de dobrar.
fn in_gutter(editor: &EditorState, position: Point<Pixels>, line: u32) -> bool {
    let Some(bounds) = editor.text_bounds() else { return false };
    let start = editor.text().line_start_offset(line as usize);
    let Some(text_x) = editor.range_to_bounds(&(start..start)).map(|b| b.origin.x) else { return false };
    position.x >= bounds.origin.x && position.x < text_x - px(FOLD_STRIP)
}

/// Do começo da primeira linha ao fim da última, sem a quebra final.
fn lines_range(editor: &EditorState, a: u32, b: u32) -> std::ops::Range<usize> {
    let (first, last) = (a.min(b) as usize, a.max(b) as usize);
    let text = editor.text();
    let start = text.line_start_offset(first);
    let end = text.line_start_offset(last) + text.line_len(last);
    start..end.max(start)
}

fn select_lines(editor: &Entity<EditorState>, a: u32, b: u32, cx: &mut App) {
    editor.update(cx, |state, cx| {
        let range = lines_range(state, a, b);
        state.set_selected_range(range, cx);
    });
}

/// Cartão do comentário: rótulo, campo e a linha com o atalho e as ações. A tela e a animação da novidade usam o mesmo
/// desenho.
pub(crate) fn comment_card(label: String, field: impl IntoElement, actions: impl IntoElement) -> Div {
    div().px_3().py_2().rounded_md().border_1().border_color(theme::border()).bg(theme::elevated()).shadow_md()
        .child(div().mb_1().text_size(px(12.)).text_color(theme::muted()).child(label))
        .child(field)
        .child(div().mt(px(6.)).flex().items_center().gap_1()
            .child(div().mr_auto().text_size(px(12.)).text_color(theme::muted()).child(tr("file_comment_shortcut")))
            .child(actions))
}

/// Rótulo das linhas (0-based aqui) igual ao do bloco na bandeja.
fn range_label(start: u32, end: u32) -> String { crate::context_refs::lines_label(start + 1, end + 1) }

impl Hangar {
    fn comment_doc(&mut self, id: u64) -> Option<&mut Document> {
        self.files.tabs.iter_mut().find(|tab| tab.id == id).and_then(|tab| tab.content.as_mut()?.as_mut().ok())
    }

    pub(super) fn line_comment_press(&mut self, id: u64, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Left || event.modifiers.shift { return; }
        let Some(doc) = self.comment_doc(id) else { return };
        let editor = doc.editor.clone();
        let line = { let state = editor.read(cx); line_at(state, event.position.y).filter(|line| in_gutter(state, event.position, *line)) };
        let Some(line) = line else { return };
        cx.stop_propagation();
        let doc = self.comment_doc(id).expect("documento conferido acima");
        doc.comment.drag = Some(line);
        doc.comment.draft = None;
        select_lines(&editor, line, line, cx);
        window.prevent_default();
        cx.notify();
    }

    pub(super) fn line_comment_move(&mut self, id: u64, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.comment_doc(id) else { return };
        let Some(anchor) = doc.comment.drag else { return };
        if event.pressed_button != Some(MouseButton::Left) { return; }
        let editor = doc.editor.clone();
        let Some(line) = line_at(editor.read(cx), event.position.y) else { return };
        select_lines(&editor, anchor, line, cx);
    }

    pub(super) fn line_comment_release(&mut self, id: u64, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(doc) = self.comment_doc(id) else { return };
        let Some(anchor) = doc.comment.drag.take() else { return };
        let editor = doc.editor.clone();
        let line = line_at(editor.read(cx), position.y).unwrap_or(anchor);
        let (start, end) = (anchor.min(line), anchor.max(line));
        select_lines(&editor, start, end, cx);
        let range = lines_range(editor.read(cx), start, end);
        if let Some(doc) = self.comment_doc(id) {
            doc.comment.selected.set(vec![RangeDecoration::new(range).with_style(RangeDecorationStyle::Fill).with_color(theme::accent().opacity(0.18))], cx);
        }
        let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 6).placeholder(tr("file_comment_placeholder")));
        let keys = cx.subscribe_in(&input, window, move |this: &mut Self, _, event: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { secondary: true, .. } = event { this.submit_line_comment(id, window, cx); }
        });
        input.update(cx, |state, cx| state.focus(window, cx));
        if let Some(doc) = self.comment_doc(id) { doc.comment.draft = Some(Draft { start, end, input, _keys: keys }); }
        cx.notify();
    }

    /// Fecha o cartão aberto do arquivo à vista. Diz se havia um, para o Esc parar aqui.
    pub(super) fn cancel_line_comment(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.files.tabs.get(self.files.active).map(|tab| tab.id) else { return false };
        let Some(doc) = self.comment_doc(id) else { return false };
        if doc.comment.draft.take().is_none() { return false; }
        doc.comment.selected.clear(cx);
        let editor = doc.editor.clone();
        editor.update(cx, |state, cx| state.unselect(window, cx));
        self.focus_file(window, cx);
        cx.notify();
        true
    }

    fn submit_line_comment(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return };
        let Some(tab) = self.files.tabs.iter_mut().find(|tab| tab.id == id) else { return };
        let path = tab.path.clone();
        let Some(Ok(doc)) = tab.content.as_mut() else { return };
        let Some(draft) = doc.comment.draft.as_ref() else { return };
        let comment = draft.input.read(cx).value().trim().to_owned();
        if comment.is_empty() { return; }
        let (start, end) = (draft.start, draft.end);
        let text = { let state = doc.editor.read(cx); state.text().slice(lines_range(state, start, end)).to_string() };
        doc.comment.draft = None;
        doc.comment.selected.clear(cx);
        let editor = doc.editor.clone();
        editor.update(cx, |state, cx| state.unselect(window, cx));
        let lang = Some(file_language(&path)).filter(|lang| *lang != "text").unwrap_or_default().to_owned();
        self.add_context_ref(&key, crate::context_refs::ContextRef::Code(crate::context_refs::Code { path, start: start + 1, end: end + 1, lang, text, comment }), cx);
        self.focus_file(window, cx);
        cx.notify();
    }

    /// Margem: as linhas que já foram para o campo de mensagem deste arquivo, enquanto o bloco estiver lá.
    pub(super) fn sync_comment_marks(&self, tab: &FileTab, doc: &Document, cx: &mut App) {
        let Some(key) = self.selected_key() else { return };
        let marks = self.code_ref_lines(&key, &tab.path).into_iter().filter(|(start, _)| *start >= 1)
            .map(|(start, end)| (start - 1, end.max(start) - 1)).collect::<Vec<_>>();
        let state = doc.editor.read(cx);
        let last = state.text().lines_len().saturating_sub(1) as u32;
        let decorations = marks.into_iter().filter(|(start, _)| *start <= last)
            .map(|(start, end)| GutterDecoration::new(lines_range(state, start, end.min(last))).with_color(theme::accent()))
            .collect::<Vec<_>>();
        doc.comment.marks.set(decorations, cx);
    }

    /// O editor de código com a seleção pela margem e o cartão do comentário sob a última linha.
    pub(super) fn commentable_editor(&self, tab: &FileTab, doc: &Document, editor: AnyElement, cx: &mut Context<Self>) -> Div {
        self.sync_comment_marks(tab, doc, cx);
        let id = tab.id;
        let area = doc.comment.area.clone();
        let measure = canvas(move |bounds, _, _| area.set(bounds), |_, _, _, _| {}).absolute().inset_0();
        let card = doc.comment.draft.as_ref().and_then(|draft| {
            let state = doc.editor.read(cx);
            let start = state.text().line_start_offset(draft.end as usize);
            let line = state.range_to_bounds(&(start..start))?;
            let area = doc.comment.area.get();
            let top = line.bottom() - area.origin.y;
            (top >= px(0.) && top < area.size.height).then_some((top, draft))
        }).map(|(top, draft)| {
            let empty = draft.input.read(cx).value().trim().is_empty();
            let label = tr("file_comment_on").replace("{lines}", &range_label(draft.start, draft.end));
            let actions = div().flex().items_center().gap_1()
                .child(Button::new("file-comment-cancel").ghost().xsmall().label(tr("file_comment_cancel"))
                    .on_click(cx.listener(|this, _, window, cx| { this.cancel_line_comment(window, cx); })))
                .child(Button::new("file-comment-send").primary().xsmall().label(tr("file_comment_send")).disabled(empty)
                    .on_click(cx.listener(move |this, _, window, cx| this.submit_line_comment(id, window, cx))));
            // Cobre também os números da margem: as linhas sob o cartão não ficam numeradas ao lado dele.
            div().id("file-comment").absolute().top(top).left(px(8.)).right(px(12.)).mt_1()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(comment_card(label.clone(), Textarea::new(&draft.input).aria_label(label), actions))
        });
        div().relative().size_full()
            .capture_any_mouse_down(cx.listener(move |this, event: &MouseDownEvent, window, cx| this.line_comment_press(id, event, window, cx)))
            .on_mouse_move(cx.listener(move |this, event: &MouseMoveEvent, window, cx| this.line_comment_move(id, event, window, cx)))
            .capture_any_mouse_up(cx.listener(move |this, event: &MouseUpEvent, window, cx| this.line_comment_release(id, event.position, window, cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(move |this, event: &MouseUpEvent, window, cx| this.line_comment_release(id, event.position, window, cx)))
            .child(measure)
            .child(editor)
            .children(card)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn labels_one_line_or_a_range_like_t3() {
        assert_eq!(super::range_label(11, 11), "L12");
        assert_eq!(super::range_label(11, 13), "L12–L14");
    }
}
