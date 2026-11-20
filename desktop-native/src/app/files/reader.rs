//! Estado e navegação do leitor de documentos, sem alterar o rascunho do editor.
use super::*;
use gpui_kit::base::text::MarkdownHeading;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum ReaderMode { #[default] Reading, Source, Split }

impl ReaderMode {
    pub(super) fn reading(self) -> bool { self != Self::Source }
    pub(super) fn source(self) -> bool { self != Self::Reading }
}

#[derive(Debug)]
pub(super) struct ReaderSettings { pub mode: ReaderMode, pub outline: bool, pub font_size: u8 }

impl Default for ReaderSettings {
    fn default() -> Self { Self { mode: ReaderMode::Reading, outline: true, font_size: theme::READER_BASE_SIZE } }
}

impl ReaderSettings {
    pub fn set_mode(&mut self, mode: ReaderMode) { self.mode = mode; }
    pub fn stacked(&self, width: f32) -> bool { self.mode == ReaderMode::Split && width < 760. }
}

#[derive(Debug, PartialEq)]
enum ReaderDestination { External(String), File(String, Option<String>), Anchor(String) }

impl ReaderDestination {
    fn parse(document: &str, href: &str) -> Option<Self> {
        if href.chars().any(char::is_control) { return None; }
        if let Some(anchor) = href.strip_prefix('#') { return Some(Self::Anchor(anchor.to_owned())); }
        if let Ok(url) = url::Url::parse(href) {
            return (matches!(url.scheme(), "http" | "https") && url.host_str().is_some()).then(|| Self::External(url.to_string()));
        }
        let path = link_target(document, href)?;
        Some(Self::File(path, href.split_once('#').map(|(_, anchor)| anchor.to_owned())))
    }
}

fn decode_url_part(value: &str) -> Option<String> {
    let mut decoded = Vec::new();
    let mut bytes = value.bytes();
    while let Some(byte) = bytes.next() {
        if byte != b'%' { decoded.push(byte); continue; }
        let hex = [bytes.next()?, bytes.next()?];
        decoded.push(u8::from_str_radix(std::str::from_utf8(&hex).ok()?, 16).ok()?);
    }
    String::from_utf8(decoded).ok()
}

fn anchor_index(headings: &[MarkdownHeading], anchor: &str) -> Option<usize> {
    let anchor = decode_url_part(anchor)?;
    let mut used = HashSet::new();
    headings.iter().enumerate().find_map(|(ix, heading)| {
        let base = heading.text.to_lowercase().chars().filter_map(|ch| {
            if ch.is_whitespace() { Some('-') }
            else if ch.is_alphanumeric() || matches!(ch, '-' | '_') { Some(ch) }
            else { None }
        }).collect::<String>();
        let mut slug = base.clone();
        let mut suffix = 0;
        while used.contains(&slug) { suffix += 1; slug = format!("{base}-{suffix}"); }
        used.insert(slug.clone());
        (slug == anchor).then_some(ix)
    })
}

impl Hangar {
    pub(super) fn navigate_reader_link(&mut self, id: u64, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let tab = &self.files.tabs[self.files.active];
        if tab.id != id { return; }
        let (path, anchor) = match ReaderDestination::parse(&tab.path, url) {
            Some(ReaderDestination::Anchor(anchor)) => (tab.path.clone(), Some(anchor)),
            Some(ReaderDestination::File(path, anchor)) => (path, anchor),
            _ => return,
        };
        self.open_file(path, None, window, cx);
        let tab = &mut self.files.tabs[self.files.active];
        if anchor.is_some() { tab.reader.set_mode(ReaderMode::Reading); }
        tab.pending_anchor = anchor;
        let id = tab.id;
        self.reveal_pending_anchor(id, window, cx);
    }

    pub(super) fn reveal_pending_anchor(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.files.tabs.iter_mut().find(|tab| tab.id == id) else { return };
        let Some(anchor) = tab.pending_anchor.as_ref() else { return };
        let Some(view) = tab.content.as_ref().and_then(|doc| doc.as_ref().ok()).and_then(|doc| doc.markdown.as_ref()) else { return };
        let rendered = view.read(cx).rendered_text();
        if rendered.is_empty() { return; }
        let target = if anchor.is_empty() { Some((0, 0..0)) }
            else { anchor_index(rendered.headings(), anchor).map(|ix| {
                let heading = &rendered.headings()[ix];
                (heading.block_index, heading.rendered_range.clone())
            }) };
        tab.pending_anchor = None;
        if let Some((block_index, range)) = target { view.update(cx, |view, cx| {
            view.list_state().scroll_to(ListOffset { item_ix: block_index, offset_in_item: px(0.) });
            let _ = view.reveal_range(range, cx);
        }); }
    }

    pub(super) fn reader_toolbar(&self, tab: &FileTab, cx: &mut Context<Self>) -> Div {
        div().flex().items_center().gap_1()
            .child(div().flex().items_center().gap(px(2.)).border_1().border_color(theme::border()).rounded_md().p(px(2.))
                .children([(ReaderMode::Reading, "file_reading", "file-reading"), (ReaderMode::Source, "file_source", "file-source"),
                    (ReaderMode::Split, "file_split", "file-split")].into_iter().map(|(mode, key, id)|
                    Button::new(id).ghost().xsmall().label(tr(key)).selected(tab.reader.mode == mode)
                        .on_click(cx.listener(move |this, _, window, cx| this.set_reader_mode(mode, window, cx))))))
            .child(chrome::icon_button("file-outline-toggle", IconName::List, tr("file_outline_toggle"), cx)
                .selected(tab.reader.outline).disabled(!tab.reader.mode.reading())
                .on_click(cx.listener(|this, _, _, cx| {
                    let reader = &mut this.files.tabs[this.files.active].reader;
                    reader.outline = !reader.outline;
                    cx.notify();
                })))
            .children([(-1i8, "file-smaller", "file_smaller", "A−"), (1, "file-larger", "file_larger", "A+")].into_iter().map(|(delta, id, key, label)|
                Button::new(id).ghost().xsmall().label(label).accessibility_label(tr(key)).tooltip(tr(key))
                    .disabled(!tab.reader.mode.reading() || (delta < 0 && tab.reader.font_size == 13) || (delta > 0 && tab.reader.font_size == 22))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let tab = &mut this.files.tabs[this.files.active];
                        tab.reader.font_size = (tab.reader.font_size as i8 + delta).clamp(13, 22) as u8;
                        if let Some(view) = tab.content.as_ref().and_then(|doc| doc.as_ref().ok()).and_then(|doc| doc.markdown.as_ref()) {
                            view.update(cx, |view, cx| view.invalidate_inline_layout(cx));
                        }
                        cx.notify();
                    }))))
    }

    pub(super) fn render_reader(&self, tab: &FileTab, doc: &Document, width: f32, cx: &mut Context<Self>) -> AnyElement {
        let Some(view) = doc.markdown.as_ref() else { return div().into_any_element() };
        let rendered = view.read(cx).rendered_text();
        let headings = rendered.headings();
        let top = view.read(cx).list_state().logical_scroll_top().item_ix;
        let active = headings.iter().rposition(|heading| heading.block_index <= top).unwrap_or(0);
        let stacked = tab.reader.stacked(width);
        let show_outline = tab.reader.outline && !headings.is_empty() && width >= if tab.reader.mode == ReaderMode::Split { 1100. } else { 760. };
        let (tx, connection, id, document) = (self.tx.clone(), self.connection, tab.id, tab.path.clone());
        let outline = div().id("file-outline").role(Role::Navigation).aria_label(tr("file_outline"))
            .w(px(204.)).h_full().flex_shrink_0().min_h_0().border_r_1().border_color(theme::border()).px_3().py_5().overflow_y_scroll()
            .child(div().px_2().mb_3().text_size(px(11.)).text_color(theme::faint()).child(tr("file_outline")))
            .children(headings.iter().enumerate().map(|(ix, heading)| {
                let range = heading.rendered_range.clone();
                let block_index = heading.block_index;
                let view = view.clone();
                Button::new(("file-heading", ix)).ghost().small().accessibility_label(heading.text.clone()).selected(ix == active)
                    .h_auto().w_full().py_2().pr_2().pl(px(8. + (heading.level.saturating_sub(1) as f32 * 10.)))
                    .border_l_2().border_color(if ix == active { theme::accent() } else { transparent_black() }).rounded_r_md()
                    .text_size(px(12.)).text_color(if ix == active { theme::accent_text() } else { theme::muted() })
                    .when(ix == active, |el| el.bg(theme::accent_dim())).cursor_pointer()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        view.update(cx, |view, cx| {
                            view.list_state().scroll_to(ListOffset { item_ix: block_index, offset_in_item: px(0.) });
                            let _ = view.reveal_range(range.clone(), cx);
                        });
                        this.files.focus.focus(window, cx);
                        cx.notify();
                    })).child(div().w_full().whitespace_normal().text_left().child(heading.text.clone()))
            }));
        let text = gpui_kit::base::TextView::new(view).selectable(true).scrollable(true)
            .style(theme::document_markdown(cx, tab.reader.font_size))
            .code_block_actions(|block, window, cx| div().pr_3().child(copy_code(block, window, cx)))
            .image_source_mapper(doc.reader_images.mapper(doc.base.path.clone(), doc.base.external, self.tree.local_root(&self.files.owner)
                    .or_else(|| self.selected.as_ref().and_then(|session| session.cwd.as_ref()).and_then(|cwd| self.local_dirs.get(cwd).cloned().flatten())),
                self.session_api(), self.selected_key().map(|key| key.name).unwrap_or_default(), self.runtime.clone(), self.tx.clone(), self.connection, tab.id))
            .on_link_click(move |url, event, window, cx| {
                match ReaderDestination::parse(&document, url) {
                    Some(ReaderDestination::External(url)) => open_web_link(&url.into(), event, window, cx),
                    Some(_) => { let _ = tx.try_send(Envelope { connection, selection: None, payload: Payload::FileView(FileReply::Link(id, url.to_string())) }); }
                    None => {},
                }
            });
        div().size_full().min_h_0().min_w_0().flex().flex_col()
            .when(doc.base.truncated, |el| el.child(div().px_4().py_2().text_xs().text_color(theme::warning())
                .child(tr("file_truncated").replace("{size}", &super::super::human_size(doc.large_size.unwrap_or(doc.base.text.len()) as u64)))))
            .when(doc.editor.read(cx).value().trim().is_empty(), |el| el.child(div().px_4().py_2().text_sm().text_color(theme::faint()).child(tr("file_empty"))))
            .child(div().flex_1().min_h_0().min_w_0().flex().when(stacked, |el| el.flex_col())
                .when(show_outline, |el| el.child(outline))
                .child(div().id("file-markdown").flex_1().min_w_0().min_h_0().when(!stacked, |el| el.h_full()).flex().px(px(if width < 760. { 16. } else { 26. })).py_6()
                    .child(div().w_full().h_full().font_family(theme::reading_font(cx))
                        .text_size(px(tab.reader.font_size as f32)).line_height(relative(22. / theme::READER_BASE_SIZE as f32)).child(text)))
                .when(tab.reader.mode.source(), |el| el.child(div().min_w_0().min_h_0().flex_shrink_0().border_color(theme::border())
                    .when(stacked, |el| el.w_full().h(relative(0.5)).border_t_1())
                    .when(!stacked, |el| el.w(relative(0.5)).h_full().border_l_1())
                    .child(self.reader_source(tab, doc, cx)))))
            .child(div().h(px(28.)).flex_shrink_0().px_3().border_t_1().border_color(theme::border()).flex().items_center().gap_3()
                .text_size(px(11.)).text_color(theme::faint())
                .child(tr(if tab.reader.mode == ReaderMode::Split { "file_split" } else { "file_reading" }))
                .child(format!("{}%", tab.reader.font_size as u32 * 100 / theme::READER_BASE_SIZE as u32))
                .child("Markdown").child(div().flex_1())
                .children(headings.get(active).map(|heading| div().max_w(relative(0.5)).truncate().child(heading.text.clone()))))
            .into_any_element()
    }

    pub(super) fn reader_source(&self, tab: &FileTab, doc: &Document, cx: &mut Context<Self>) -> Div {
        div().flex().flex_col().size_full().min_h_0()
            .when(doc.base.truncated, |el| el.child(div().px_4().py_2().text_xs().text_color(theme::warning())
                .child(tr("file_truncated").replace("{size}", &super::super::human_size(doc.large_size.unwrap_or(doc.base.text.len()) as u64)))))
            .when_some(doc.error.as_ref(), |el, error| el.child(div().id("file-save-error").role(Role::Alert)
                .flex_shrink_0().px_4().py_2().text_sm().text_color(theme::danger()).child(error.clone())))
            .child(div().flex_1().min_h_0().overflow_hidden()
                .child(self.commentable_editor(tab, doc, Editor::new(&doc.editor).readonly(!doc.editable() || doc.saving || doc.reloading)
                    .bordered(false).h_full().font_family(theme::MONO).text_sm().line_height(relative(1.7)).aria_label(tab.path.clone()).into_any_element(), cx)))
    }
}

#[cfg(test)]
mod render_tests {
    use gpui::{Context, Entity, IntoElement, Modifiers, Render, TestAppContext, Window, div, point, px, prelude::*};
    use gpui_kit::base::{TextView, TextViewState};
    use std::sync::{Arc, Mutex};

    struct ReferenceSurface { text: Entity<TextViewState>, opened: Arc<Mutex<Vec<String>>>, images: Arc<Mutex<Vec<String>>> }
    impl Render for ReferenceSurface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let (opened, images) = (self.opened.clone(), self.images.clone());
            div().w(px(400.)).child(gpui_kit::base::TextSelectionLayer)
                .child(TextView::new(&self.text).selectable(true)
                    .image_source_mapper(move |url| {
                        images.lock().unwrap().push(url.to_string());
                        Some(gpui::ImageSource::Custom(Arc::new(|_, _| None)))
                    })
                    .on_link_click(move |url, _, _, _| opened.lock().unwrap().push(url.to_string())))
        }
    }

    #[gpui::test]
    fn review_regression_duplicate_definitions_keep_the_first_image_and_link_destination(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (opened, images) = (Arc::new(Mutex::new(Vec::new())), Arc::new(Mutex::new(Vec::new())));
        let (_, cx) = cx.add_window_view(|_, cx| ReferenceSurface {
            text: cx.new(|cx| TextViewState::markdown("[fig]: https://example.org/first.png\n[fig]: https://example.org/second.png\n\n[Link][fig]\n\n![Figura][fig]", cx)),
            opened: opened.clone(), images: images.clone(),
        });
        cx.run_until_parked();
        cx.update(|window, cx| { let _ = window.draw(cx); });
        assert!(!images.lock().unwrap().is_empty());
        assert!(images.lock().unwrap().iter().all(|url| url == "https://example.org/first.png"));
        cx.simulate_click(point(px(12.), px(12.)), Modifiers::default());
        assert_eq!(*opened.lock().unwrap(), ["https://example.org/first.png"]);
    }

    struct LinkSurface { text: Entity<TextViewState>, opened: Arc<Mutex<Vec<String>>> }
    impl Render for LinkSurface {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let opened = self.opened.clone();
            div().w(px(400.)).child(gpui_kit::base::TextSelectionLayer)
                .child(TextView::new(&self.text).selectable(true).on_link_click(move |url, _, _, _| opened.lock().unwrap().push(url.to_string())))
        }
    }

    #[gpui::test]
    fn normal_click_on_reference_link_in_list_uses_resolved_url(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let opened = Arc::new(Mutex::new(Vec::new()));
        let (_, cx) = cx.add_window_view(|_, cx| LinkSurface {
            text: cx.new(|cx| TextViewState::markdown("- [Referência externa][target]\n\n[target]: https://example.org/reference#anchor", cx)),
            opened: opened.clone(),
        });
        cx.run_until_parked();
        cx.update(|window, cx| { let _ = window.draw(cx); });
        cx.simulate_click(point(px(60.), px(10.)), Modifiers::default());
        assert_eq!(*opened.lock().unwrap(), ["https://example.org/reference#anchor"]);
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::base::text::MarkdownExtensions;

    #[test]
    fn narrow_split_keeps_both_views_without_squeezing_them_into_columns() {
        use super::{ReaderMode, ReaderSettings};
        let mut reader = ReaderSettings::default();
        reader.set_mode(ReaderMode::Split);
        assert!(reader.stacked(416.));
        assert!(!reader.stacked(1000.));
        reader.set_mode(ReaderMode::Reading);
        assert!(!reader.stacked(416.));
        reader.set_mode(ReaderMode::Source);
        assert!(!reader.stacked(416.));
    }

    #[test]
    fn remote_windows_documents_keep_drive_and_resolve_relative_assets() {
        use super::ReaderDestination as Destination;
        let parse = |href| Destination::parse(r"C:\projeto\docs\guia.md", href);
        assert_eq!(parse(r"assets\exemplo.png"), Some(Destination::File(r"C:\projeto\docs\assets\exemplo.png".into(), None)));
        assert_eq!(parse("../outro.md#ação"), Some(Destination::File(r"C:\projeto\outro.md".into(), Some("ação".into()))));
        assert_eq!(parse("../../../../escape.md"), None);
        assert_eq!(parse(r"\\server\share\image.png"), None);
    }

    #[test]
    fn document_links_preserve_files_fragments_and_external_browser_boundary() {
        use super::ReaderDestination as Destination;
        let parse = |href| Destination::parse("docs/guide.md", href);
        assert_eq!(parse("../other%20guide.md#a%C3%A7%C3%A3o"), Some(Destination::File("other guide.md".into(), Some("a%C3%A7%C3%A3o".into()))));
        assert_eq!(parse("#seção"), Some(Destination::Anchor("seção".into())));
        assert_eq!(parse("https://example.com/a path?q=a b#section"), Some(Destination::External("https://example.com/a%20path?q=a%20b#section".into())));
        for href in ["javascript:alert(1)", "data:text/html,test", "file:///etc/passwd", "//outside.test/a", "../../escape.md", "%2Fetc/passwd", "%5Cserver/file", "%6Aavascript%3Aalert(1)", "bad%00.md", "https://example.com/\nunsafe"] {
            assert_eq!(parse(href), None, "{href}");
        }
    }

    #[test]
    fn anchors_locate_formatted_repeated_and_non_ascii_headings() {
        let headings = MarkdownExtensions::default().headings("# **Ação** com `código`\n\n## Repetido\n\n## Repetido\n\nTexto\n=====\n\n###### Último").unwrap();
        assert_eq!(super::anchor_index(&headings, "a%C3%A7%C3%A3o-com-c%C3%B3digo"), Some(0));
        assert_eq!(super::anchor_index(&headings, "repetido"), Some(1));
        assert_eq!(super::anchor_index(&headings, "repetido-1"), Some(2));
        assert_eq!(super::anchor_index(&headings, "texto"), Some(3));
        assert_eq!(super::anchor_index(&headings, "último"), Some(4));
        assert_eq!(super::anchor_index(&headings, "missing"), None);
        assert_eq!(headings[4].level, 6);
        assert!(headings[2].rendered_range.start > headings[1].rendered_range.end);
    }

    #[test]
    fn reader_modes_keep_the_source_and_reading_views_distinct() {
        use super::{ReaderMode, ReaderSettings};
        let mut reader = ReaderSettings::default();
        assert!(reader.mode.reading() && !reader.mode.source());
        reader.set_mode(ReaderMode::Source);
        assert!(!reader.mode.reading() && reader.mode.source());
        reader.set_mode(ReaderMode::Split);
        assert!(reader.mode.reading() && reader.mode.source());
        reader.set_mode(ReaderMode::Reading);
        assert!(reader.mode.reading() && !reader.mode.source());
    }

    #[test]
    fn outline_uses_real_headings_and_rendered_inline_text() {
        let source = "# **Título** com `código` e [link](https://example.com)\n\nSeção &amp; ação\n---\n\n```md\n# Não é título\n```\n\n## Repetido\n\n## Repetido\n";
        let headings = MarkdownExtensions::default().headings(source).unwrap();
        assert_eq!(headings.iter().map(|h| (h.level, h.text.as_str())).collect::<Vec<_>>(),
            vec![(1, "Título com código e link"), (2, "Seção & ação"), (2, "Repetido"), (2, "Repetido")]);
        assert_eq!(headings.iter().map(|h| &source[h.source_range.clone()]).collect::<Vec<_>>(),
            vec!["# **Título** com `código` e [link](https://example.com)", "Seção &amp; ação\n---", "## Repetido", "## Repetido"]);
    }
}
