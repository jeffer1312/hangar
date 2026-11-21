//! Busca na prévia e navegação do arquivo; o código usa a busca do próprio editor.
use super::*;
use gpui_kit::base::text::RangeHighlight;

pub(super) struct PreviewFind {
    pub(super) open: bool,
    pub(super) target: Option<u64>,
    input: Entity<InputState>,
    ranges: Vec<std::ops::Range<usize>>,
    active: usize,
    painted: Option<(String, String, usize)>,
    watch: Option<Subscription>,
}

impl PreviewFind {
    pub(super) fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("file_find")));
        cx.subscribe_in(&input, window, |this: &mut Hangar, _, event: &InputEvent, _, cx| match event {
            InputEvent::Change => { this.files.preview_find.active = 0; this.paint_preview_find(true, cx); }
            InputEvent::PressEnter { shift, .. } => this.step_preview_find(if *shift { -1 } else { 1 }, cx),
            _ => {},
        }).detach();
        Self { open: false, target: None, input, ranges: Vec::new(), active: 0, painted: None, watch: None }
    }
}

impl Hangar {
    pub(in crate::app) fn find_in_file(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if window.has_active_dialog(cx) { return false; }
        if !self.files_visible() { return false; }
        let tab = &self.files.tabs[self.files.active];
        let Some(Ok(doc)) = &tab.content else { return true; };
        // Na tabela a busca é a do código: troca para ele e abre a busca do editor.
        if tab.reader.mode.reading() && doc.table.is_some() {
            let editor = doc.editor.clone();
            self.files.tabs[self.files.active].reader.set_mode(reader::ReaderMode::Source);
            editor.update(cx, |state, cx| state.open_search(false, cx));
            cx.notify();
            return true;
        }
        if tab.reader.mode.reading() {
            let Some(view) = doc.markdown.clone() else { return true; };
            let id = tab.id;
            let find = &mut self.files.preview_find;
            if find.target != Some(id) {
                find.watch = Some(cx.observe(&view, |this, _, cx| this.paint_preview_find(false, cx)));
                find.target = Some(id);
                find.painted = None;
                find.active = 0;
            }
            find.open = true;
            find.input.update(cx, |state, cx| { state.focus(window, cx); state.select_all(window, cx); });
            self.paint_preview_find(true, cx);
        } else {
            doc.editor.update(cx, |state, cx| state.open_search(false, cx));
        }
        cx.notify();
        true
    }

    fn paint_preview_find(&mut self, reveal: bool, cx: &mut Context<Self>) {
        if !self.files.preview_find.open { return; }
        let Some(tab) = self.files.tabs.get(self.files.active).filter(|tab| Some(tab.id) == self.files.preview_find.target) else { return; };
        let Some(view) = tab.content.as_ref().and_then(|doc| doc.as_ref().ok()).and_then(|doc| doc.markdown.clone()) else { return; };
        let rendered = view.read(cx).rendered_text().as_str().to_owned();
        let find = &mut self.files.preview_find;
        let query = find.input.read(cx).value().to_lowercase();
        let stamp = (query.clone(), rendered.clone(), find.active);
        if !reveal && find.painted.as_ref() == Some(&stamp) { return; }
        find.ranges = super::super::find::occurrences(&rendered, &query);
        find.active = find.active.min(find.ranges.len().saturating_sub(1));
        find.painted = Some((query, rendered, find.active));
        view.update(cx, |state, cx| {
            if find.ranges.is_empty() { state.clear_range_highlights(cx); return; }
            let _ = state.set_range_highlights(find.ranges.iter().enumerate().map(|(ix, range)|
                RangeHighlight::new(range.clone(), theme::warning().opacity(if ix == find.active { 0.55 } else { 0.22 }))), cx);
            if reveal { let _ = state.reveal_range(find.ranges[find.active].clone(), cx); }
        });
        cx.notify();
    }

    fn step_preview_find(&mut self, delta: isize, cx: &mut Context<Self>) {
        let find = &mut self.files.preview_find;
        if find.ranges.is_empty() { return; }
        find.active = (find.active as isize + delta).rem_euclid(find.ranges.len() as isize) as usize;
        self.paint_preview_find(true, cx);
    }

    pub(super) fn close_preview_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let restore = self.files_visible() && self.files.focus.contains_focused(window, cx);
        if let Some(id) = self.files.preview_find.target {
            if let Some(view) = self.files.tabs.iter().find(|tab| tab.id == id)
                .and_then(|tab| tab.content.as_ref()?.as_ref().ok()?.markdown.clone()) {
                view.update(cx, |state, cx| state.clear_range_highlights(cx));
            }
        }
        let find = &mut self.files.preview_find;
        (find.open, find.target, find.watch, find.painted) = (false, None, None, None);
        find.ranges.clear();
        if restore { self.files.focus.focus(window, cx); }
        cx.notify();
    }

    pub(super) fn render_preview_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = &self.files.preview_find;
        if !find.open || !self.files.tabs.get(self.files.active).is_some_and(|tab| tab.reader.mode.reading() && find.target == Some(tab.id)) { return None; }
        let count = if find.ranges.is_empty() { tr("find_none") } else {
            tr("find_count").replace("{n}", &(find.active + 1).to_string()).replace("{total}", &find.ranges.len().to_string())
        };
        Some(div().id("file-preview-find").flex().items_center().flex_shrink_0().gap_2().px_3().py_2()
            .border_b_1().border_color(theme::border())
            .child(div().flex_1().min_w_0().child(Input::new(&find.input).small().aria_label(tr("file_find"))))
            .child(div().text_xs().text_color(theme::muted()).child(count))
            .child(chrome::icon_button("file-find-prev", IconName::ChevronUp, tr("find_prev"), cx).disabled(find.ranges.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.step_preview_find(-1, cx))))
            .child(chrome::icon_button("file-find-next", IconName::ChevronDown, tr("find_next"), cx).disabled(find.ranges.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.step_preview_find(1, cx))))
            .child(chrome::icon_button("file-find-close", IconName::Close, tr("find_close"), cx)
                .on_click(cx.listener(|this, _, window, cx| this.close_preview_find(window, cx)))).into_any_element())
    }

    pub(super) fn open_file_line(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let tab = &self.files.tabs[self.files.active];
        let Some(Ok(doc)) = &tab.content else { return; };
        let (id, owner, max) = (tab.id, self.files.owner.clone(), doc.editor.read(cx).text().lines_len());
        let input = cx.new(|cx| InputState::new(window, cx).default_value((doc.editor.read(cx).cursor_position().line + 1).to_string()));
        let enter_owner = owner.clone();
        cx.subscribe_in(&input, window, move |this: &mut Hangar, input, event: &InputEvent, window, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                if let Ok(line) = input.read(cx).value().trim().parse::<u32>() {
                    if line > 0 && line as usize <= max { this.go_to_file_line(id, &enter_owner, line, window, cx); }
                }
            }
            cx.notify();
        }).detach();
        let weak = cx.entity().downgrade();
        let focus = input.clone();
        cx.defer_in(window, move |_, window, cx| focus.update(cx, |state, cx| { state.focus(window, cx); state.select_all(window, cx); }));
        window.open_dialog(cx, move |dialog, _, cx| {
            let line = input.read(cx).value().trim().parse::<u32>().ok().filter(|line| *line > 0 && *line as usize <= max);
            let go = weak.clone();
            let go_owner = owner.clone();
            let run = move |window: &mut Window, cx: &mut App| {
                let Some(line) = line else { return; };
                let _ = go.update(cx, |this, cx| {
                    this.go_to_file_line(id, &go_owner, line, window, cx);
                });
            };
            popup::dialog(dialog).title(tr("file_goto_line"))
                .child(div().flex().flex_col().gap_2()
                    .child(div().text_sm().child(tr("file_line_range").replace("{max}", &max.to_string())))
                    .child(Input::new(&input).aria_label(tr("file_goto_line"))))
                .footer(div().flex().justify_end().gap_2()
                    .child(Button::new("file-line-cancel").label(tr("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("file-line-go").primary().label(tr("file_go")).disabled(line.is_none())
                        .on_click(move |_, window, cx| run(window, cx))))
                .on_ok(super::super::machines::enter_to_focused)
        });
    }

    fn go_to_file_line(&mut self, id: u64, owner: &Option<SessionOwner>, line: u32, window: &mut Window, cx: &mut Context<Self>) {
        if &self.files.owner != owner || self.files.owner != self.session_owner() { window.close_dialog(cx); return; }
        let Some(ix) = self.files.tabs.iter().position(|tab| tab.id == id) else { window.close_dialog(cx); return; };
        self.files.active = ix;
        self.files.tabs[ix].line = Some(line);
        window.close_dialog(cx);
        cx.defer_in(window, |this, window, cx| this.focus_file(window, cx));
    }
}
