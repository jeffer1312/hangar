//! Arquivos sobre a conversa; cada aba conserva seu rascunho e seu pedido.
use super::*;
use gpui_kit::component::input::{Editor, EditorState, Position, RopeExt};

actions!(file_view, [CloseFile, NextFile, PreviousFile, SaveFile]);

pub(super) struct Files {
    pub path: Entity<InputState>,
    owner: (u64, u64),
    hidden: bool,
    tabs: Vec<FileTab>,
    active: usize,
    serial: u64,
    focus: FocusHandle,
    return_focus: Option<FocusHandle>,
    _focus_lost: Subscription,
}

struct FileTab {
    id: u64,
    path: String,
    line: Option<u32>,
    content: Option<Result<Document, String>>,
}

#[derive(serde::Deserialize)]
pub(super) struct Content { path: String, text: String, truncated: bool, digest: Option<String>, #[serde(skip)] external: bool }
impl Content {
    fn editable(&self) -> bool { !self.truncated && self.digest.as_ref().is_some_and(|digest| !digest.is_empty()) }
}
pub(super) enum FileReply {
    Read(u64, Result<Content, Failure>),
    Saved(u64, String, Result<Value, Failure>),
}

struct Document {
    editor: Entity<EditorState>,
    base: Content,
    saving: bool,
    dirty: bool,
    saved: Option<Instant>,
    error: Option<String>,
    _changed: Subscription,
}

impl Document {
    fn editable(&self) -> bool { self.base.editable() }
    fn dirty(&self) -> bool { self.dirty }
}

fn file_failure(error: &Failure) -> String {
    if error.detail.starts_with("erro_arq_") {
        return Hangar::fetch_failure(error);
    }
    match error.status {
        Some(409) => activity::web("erro_arq_mudou_no_disco"),
        Some(413) => activity::web("erro_arq_grande_demais"),
        Some(415) => activity::web("erro_arq_binario"),
        _ => Hangar::fetch_failure(error),
    }
}

impl Files {
    pub fn new(window: &mut Window, cx: &mut Context<Hangar>) -> Self {
        cx.bind_keys([
            KeyBinding::new("alt-w", CloseFile, Some("FileViewer")),
            KeyBinding::new("ctrl-pageup", PreviousFile, Some("FileViewer")),
            KeyBinding::new("ctrl-pagedown", NextFile, Some("FileViewer")),
            KeyBinding::new("ctrl-s", SaveFile, Some("FileViewer")),
        ]);
        let focus = cx.focus_handle();
        let lost = cx.on_focus_lost(window, |this, window, cx| this.files_focus_lost(window, cx));
        Self { path: cx.new(|cx| InputState::new(window, cx).placeholder(tr("file_path"))),
            owner: (0, 0), hidden: false, tabs: Vec::new(), active: 0, serial: 0,
            focus, return_focus: None, _focus_lost: lost }
    }
}

fn path_line(raw: &str) -> (String, Option<u32>) {
    let raw = raw.trim();
    match raw.rsplit_once(':').and_then(|(path, line)| line.parse::<u32>().ok().filter(|n| *n > 0).map(|n| (path, n))) {
        Some((path, line)) => (path.to_owned(), Some(line)),
        None => (raw.to_owned(), None),
    }
}

async fn read_file(api: Api, name: String, path: String) -> Result<Content, Failure> {
    let resolved = api.act(&name, &["files", "resolver"], Some(json!({"caminhos": [&path]})), false, 30).await?;
    let entry = resolved.get("ok").and_then(|v| v.get(&path)).ok_or_else(|| Failure::local("file_missing"))?;
    let (route, requested) = match entry.get("relativo") {
        Some(Value::String(relative)) => (["files", "read"], relative.as_str()),
        Some(Value::Null) => (["file", "text"], path.as_str()),
        _ => return Err(Failure::local("invalid_response")),
    };
    let mut content: Content = serde_json::from_value(api.read(&name, &route, &[("path", requested)], 30).await?)
        .map_err(|_| Failure::local("invalid_response"))?;
    content.external = route[0] == "file";
    Ok(content)
}

impl Hangar {
    fn files_visible(&self) -> bool {
        self.files.owner == (self.connection, self.selection) && !self.files.tabs.is_empty() && !self.files.hidden
            && (self.settings.is_none() || self.settings_live())
    }

    pub(super) fn open_file_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (path, line) = path_line(self.files.path.read(cx).value().as_ref());
        if !path.is_empty() { self.open_file(path, line, window, cx); }
    }

    pub(super) fn open_file(&mut self, path: String, line: Option<u32>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return };
        if !self.files_visible() {
            if self.files.owner != (self.connection, self.selection) { self.files.tabs.clear(); }
            self.files.owner = (self.connection, self.selection);
            self.files.hidden = false;
            self.files.return_focus = window.focused(cx);
        }
        if let Some(ix) = self.files.tabs.iter().position(|tab| tab.path == path) {
            self.files.active = ix;
            self.files.tabs[ix].line = line;
            self.focus_file(window, cx);
            return;
        }
        self.files.serial += 1;
        let id = self.files.serial;
        self.files.tabs.push(FileTab { id, path: path.clone(), line, content: None });
        self.files.active = self.files.tabs.len() - 1;
        self.focus_file(window, cx);
        let (connection, selection, tx) = (self.connection, Some(self.selection), self.tx.clone());
        self.runtime.spawn(async move {
            let result = read_file(api, key.name, path).await;
            let _ = tx.send(Envelope { connection, selection, payload: Payload::FileView(FileReply::Read(id, result)) }).await;
        });
    }

    pub(super) fn receive_file_view(&mut self, reply: FileReply, window: &mut Window, cx: &mut Context<Self>) {
        let (id, result) = match reply {
            FileReply::Read(id, result) => (id, result),
            FileReply::Saved(id, text, result) => { self.file_saved(id, text, result, cx); return; }
        };
        let Some(ix) = self.files.tabs.iter().position(|tab| tab.id == id) else { return };
        let path = &self.files.tabs[ix].path;
        self.files.tabs[ix].content = Some(result.map(|content| {
            let extension = std::path::Path::new(path).extension().and_then(|s| s.to_str()).unwrap_or("txt");
            let editor = cx.new(|cx| EditorState::new(window, cx).language(extension).default_value(content.text.clone()).soft_wrap(true));
            let changed = cx.subscribe_in(&editor, window, move |this: &mut Self, _, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::Change) {
                    if let Some(tab) = this.files.tabs.iter_mut().find(|tab| tab.id == id) {
                        if let Some(Ok(doc)) = &mut tab.content {
                            doc.dirty = doc.editor.read(cx).value().as_ref() != doc.base.text;
                            doc.saved = None;
                        }
                    }
                    cx.notify();
                }
            });
            let doc = Document { editor, base: content, saving: false, dirty: false, saved: None, error: None, _changed: changed };
            doc.editor.update(cx, |state, cx| state.set_readonly(!doc.editable(), cx));
            doc
        }).map_err(|error| match error.status {
            Some(415) => activity::web("erro_arq_binario"),
            Some(404) => activity::web("erro_arq_inexistente"),
            None if error.detail == "file_missing" => activity::web("erro_arq_inexistente"),
            _ => file_failure(&error),
        }));
        // A leitura não toma o foco de outra aba, diálogo ou campo aberto enquanto esperava.
        if ix == self.files.active && self.files.focus.contains_focused(window, cx) { self.focus_file(window, cx); }
        cx.notify();
    }

    fn save_file(&mut self, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let (Some(api), Some(key)) = (self.api.clone(), self.selected_key()) else { return };
        let tab = &mut self.files.tabs[self.files.active];
        let Some(Ok(doc)) = &mut tab.content else { return };
        if doc.saving || !doc.editable() || !doc.dirty() { return; }
        let text = doc.editor.read(cx).value().to_string();
        let body = json!({"path": doc.base.path, "text": text, "digest": doc.base.digest});
        let route = if doc.base.external { ["file", "text"] } else { ["files", "write"] };
        (doc.saving, doc.saved, doc.error) = (true, None, None);
        // A resposta não pode apagar uma edição feita depois do envio.
        doc.editor.update(cx, |state, cx| state.set_readonly(true, cx));
        let (id, connection, selection, tx) = (tab.id, self.connection, Some(self.selection), self.tx.clone());
        self.runtime.spawn(async move {
            let result = api.act(&key.name, &route, Some(body), false, 30).await;
            let _ = tx.send(Envelope { connection, selection, payload: Payload::FileView(FileReply::Saved(id, text, result)) }).await;
        });
        cx.notify();
    }

    fn file_saved(&mut self, id: u64, text: String, result: Result<Value, Failure>, cx: &mut Context<Self>) {
        let Some(tab) = self.files.tabs.iter_mut().find(|tab| tab.id == id) else { return };
        let Some(Ok(doc)) = &mut tab.content else { return };
        doc.saving = false;
        doc.editor.update(cx, |state, cx| state.set_readonly(!doc.editable(), cx));
        match result.and_then(|value| value.get("digest").and_then(Value::as_str).filter(|s| !s.is_empty())
            .map(str::to_owned).ok_or_else(|| Failure::local("invalid_response"))) {
            Ok(digest) => {
                (doc.base.text, doc.base.digest) = (text, Some(digest));
                doc.dirty = false;
                let saved = Instant::now();
                doc.saved = Some(saved);
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(Duration::from_secs(2)).await;
                    let _ = this.update(cx, |this, cx| {
                        if let Some(tab) = this.files.tabs.iter_mut().find(|tab| tab.id == id) {
                            if let Some(Ok(doc)) = &mut tab.content {
                                if doc.saved == Some(saved) { doc.saved = None; cx.notify(); }
                            }
                        }
                    });
                }).detach();
            }
            Err(error) => doc.error = Some(file_failure(&error)),
        }
        cx.notify();
    }

    fn discard_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let Some(Ok(doc)) = &mut self.files.tabs[self.files.active].content else { return };
        if doc.saving { return; }
        doc.editor.update(cx, |state, cx| state.set_value(doc.base.text.clone(), window, cx));
        doc.dirty = false;
        (doc.error, doc.saved) = (None, None);
        cx.notify();
    }

    fn focus_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let tab = &mut self.files.tabs[self.files.active];
        let id = tab.id;
        if let Some(Ok(doc)) = &tab.content {
            let line = tab.line.take();
            let row = doc.editor.update(cx, |state, cx| {
                if let Some(line) = line {
                    let row = line.saturating_sub(1).min(state.text().lines_len().saturating_sub(1) as u32);
                    state.set_cursor_position(Position::new(row, 0), window, cx);
                    Some(row)
                }
                else { state.focus(window, cx); None }
            });
            if let Some(row) = row {
                // O editor novo só tem medida depois do primeiro desenho.
                cx.on_next_frame(window, move |_, window, cx| cx.on_next_frame(window,
                    move |this, window, cx| this.reveal_file_line(id, row, window, cx)));
            }
        } else { self.files.focus.focus(window, cx); }
        cx.notify();
    }

    fn reveal_file_line(&mut self, id: u64, row: u32, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let tab = &self.files.tabs[self.files.active];
        if tab.id != id { return; }
        if let Some(Ok(doc)) = &tab.content {
            doc.editor.update(cx, |state, cx| {
                let position = Position::new(row, 0);
                if state.focus_handle(cx).is_focused(window) && state.cursor_position() == position {
                    state.set_cursor_position(position, window, cx);
                }
            });
        }
    }

    fn files_focus_lost(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        window.focus_lost_restore_target(cx).unwrap_or_else(|| self.root_focus.clone()).focus(window, cx);
    }

    fn step_file(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let n = self.files.tabs.len();
        self.files.active = (self.files.active + if forward { 1 } else { n - 1 }) % n;
        self.focus_file(window, cx);
    }

    fn close_file(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.files.tabs.iter().position(|tab| tab.id == id) else { return };
        self.files.tabs.remove(ix);
        if self.files.tabs.is_empty() { self.restore_file_focus(window, cx); }
        else {
            if ix < self.files.active { self.files.active -= 1; }
            self.files.active = self.files.active.min(self.files.tabs.len() - 1);
            self.focus_file(window, cx);
        }
        cx.notify();
    }

    fn restore_file_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.files.return_focus.take().filter(|focus| self.root_focus.contains(focus, window))
            .unwrap_or_else(|| self.root_focus.clone()).focus(window, cx);
    }

    pub(super) fn files_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.files_visible() { return false; }
        self.files.hidden = true;
        self.restore_file_focus(window, cx);
        cx.notify();
        true
    }

    pub(super) fn render_file_view(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.files_visible() { return None; }
        let tab = &self.files.tabs[self.files.active];
        let tabs = div().id("file-tabs").flex().gap_1().px_2().pt_1().flex_shrink_0().overflow_x_scroll()
            .children(self.files.tabs.iter().enumerate().map(|(ix, tab)| {
                let id = tab.id;
                let mark = match &tab.content {
                    Some(Ok(doc)) if doc.error.is_some() => Some(true),
                    Some(Ok(doc)) if doc.dirty() => Some(false),
                    _ => None,
                };
                div().id(("file-tab", id)).flex().items_center().flex_shrink_0().rounded_t_lg()
                    .when(ix == self.files.active, |el| el.bg(theme::elevated()))
                    .child(Button::new(("file-activate", id)).ghost().small().max_w(rems(12.5)).selected(ix == self.files.active)
                        .label(composer::basename(&tab.path).to_owned())
                        .tooltip(mark.map_or(tab.path.clone(), |failed| format!("{} · {}", tab.path,
                            activity::web(if failed { "arq_falhou_salvar" } else { "arq_nao_salvo" }))))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            if let Some(ix) = this.files.tabs.iter().position(|t| t.id == id) { this.files.active = ix; this.focus_file(window, cx); }
                        })))
                    .when_some(mark, |el, failed| el.child(div().size(px(6.)).flex_shrink_0().rounded_full()
                        .bg(if failed { theme::danger() } else { theme::accent() })))
                    .child(Button::new(("file-close", id)).ghost().small().icon(IconName::Close).accessibility_label(tr("file_close"))
                        .tooltip(tr("file_close")).on_click(cx.listener(move |this, _, window, cx| this.close_file(id, window, cx))))
            }));
        let content = match &tab.content {
            None => div().p_4().text_color(theme::muted()).child(tr("file_loading")).into_any_element(),
            Some(Err(error)) => div().p_4().text_color(theme::danger()).child(error.clone()).into_any_element(),
            Some(Ok(doc)) => div().flex().flex_col().size_full().min_h_0()
                .when(doc.base.truncated, |el| el.child(div().px_4().py_2().text_xs().text_color(theme::warning()).child(tr("file_truncated"))))
                .when_some(doc.error.as_ref(), |el, error| el.child(div().id("file-save-error").role(Role::Alert)
                    .flex_shrink_0().px_4().py_2().text_sm().text_color(theme::danger()).child(error.clone())))
                .child(div().flex_1().min_h_0().overflow_hidden()
                    .child(Editor::new(&doc.editor).readonly(!doc.editable() || doc.saving).bordered(false).h_full().font_family(theme::MONO).text_sm()
                        .line_height(relative(1.7)).aria_label(tab.path.clone())))
                .into_any_element(),
        };
        Some(div().id("file-viewer").absolute().inset_0().occlude().flex().flex_col().min_h_0().bg(theme::surface())
            .border_1().border_color(theme::border()).rounded_lg().overflow_hidden()
            .key_context("FileViewer").track_focus(&self.files.focus)
            .on_action(cx.listener(|this, _: &CloseFile, window, cx| {
                if let Some(tab) = this.files.tabs.get(this.files.active) { this.close_file(tab.id, window, cx); }
            }))
            .on_action(cx.listener(|this, _: &NextFile, window, cx| this.step_file(true, window, cx)))
            .on_action(cx.listener(|this, _: &PreviousFile, window, cx| this.step_file(false, window, cx)))
            .on_action(cx.listener(|this, _: &SaveFile, _, cx| this.save_file(cx)))
            .child(tabs)
            .child(div().flex().items_center().gap_4().px_4().py_2().flex_shrink_0().border_b_1().border_color(theme::border())
                .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_xs().text_color(theme::muted()).child(tab.path.clone()))
                .when_some(tab.content.as_ref().and_then(|result| result.as_ref().ok()), |el, doc| el
                    .when(doc.saved.is_some(), |el| el.child(div().text_sm().text_color(theme::success()).child(tr("file_saved"))))
                    .when(doc.editable() && doc.dirty(), |el| el
                        .child(Button::new("file-discard").ghost().small().label(tr("file_discard")).disabled(doc.saving)
                            .on_click(cx.listener(|this, _, window, cx| this.discard_file(window, cx))))
                        .child(Button::new("file-save").primary().small().label(tr(if doc.saving { "file_saving" } else { "file_save" }))
                            .tooltip(tr("file_save_shortcut")).disabled(doc.saving)
                            .on_click(cx.listener(|this, _, _, cx| this.save_file(cx))))))
                .child(Button::new("file-back").ghost().small().label(tr("file_back"))
                    .on_click(cx.listener(|this, _, window, cx| { this.files_escape(window, cx); }))))
            .child(div().flex_1().min_h_0().overflow_hidden().child(content)).into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::path_line;
    #[test]
    fn editing_requires_a_complete_read_and_digest() {
        let mut content: super::Content = serde_json::from_value(serde_json::json!({
            "path": "empty.txt", "text": "", "truncated": false, "digest": "read-digest"
        })).unwrap();
        assert!(content.editable());
        content.truncated = true;
        assert!(!content.editable());
        content.truncated = false;
        content.digest = None;
        assert!(!content.editable());
        content.digest = Some(String::new());
        assert!(!content.editable());
    }
    #[test]
    fn file_line_suffix_preserves_windows_drive() {
        assert_eq!(path_line("C:\\src\\main.rs:120"), ("C:\\src\\main.rs".into(), Some(120)));
        assert_eq!(path_line("C:\\src\\main.rs"), ("C:\\src\\main.rs".into(), None));
        assert_eq!(path_line("src/demo.rs:0"), ("src/demo.rs:0".into(), None));
    }
}
