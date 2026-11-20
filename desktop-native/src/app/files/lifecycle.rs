use super::*;

#[derive(Clone, Copy)]
enum Change { Close, Discard, Reload }

pub(super) fn watch_files(_: &mut Window, cx: &mut Context<Hangar>) -> Task<()> {
    cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            if this.update(cx, |this, cx| this.poll_file_changes(cx)).is_err() { break; }
        }
    })
}

impl Hangar {
    pub(in crate::app) fn files_connection_dropped(&mut self, cx: &mut Context<Self>) {
        if let Some(owner) = &mut self.files.owner { owner.0 = self.connection; }
        let active = self.files.tabs.get(self.files.active).map(|tab| tab.id);
        // Essas leituras perderam a resposta da conexão anterior e ainda não têm rascunho.
        self.files.tabs.retain(|tab| !(tab.picture.is_none() && tab.content.is_none()) && !matches!(tab.picture, Some(Picture::Loading)));
        self.files.active = active.and_then(|id| self.files.tabs.iter().position(|tab| tab.id == id))
            .unwrap_or_else(|| self.files.active.min(self.files.tabs.len().saturating_sub(1)));
        for tab in &mut self.files.tabs {
            let Some(Ok(doc)) = &mut tab.content else { continue; };
            if doc.saving || doc.reloading { doc.error = Some(tr("file_operation_unknown")); }
            doc.read_seq = doc.read_seq.wrapping_add(1);
            (doc.saving, doc.reloading, doc.checking, doc.close_after_save) = (false, false, false, false);
            doc.poll_error = None;
            doc.editor.update(cx, |state, cx| state.set_readonly(!doc.editable(), cx));
        }
    }

    pub(super) fn guard_file_owner_change(&mut self, path: String, line: Option<u32>, window: &mut Window,
        cx: &mut Context<Self>) -> bool {
        let owner = self.files.owner.clone();
        let target = self.session_owner();
        if owner.as_ref().zip(target.as_ref()).is_some_and(|(owner, target)| owner.1 == target.1 && owner.2 == target.2) {
            self.files.owner = target;
            return false;
        }
        let protected = self.files.tabs.iter().any(|tab| tab.content.as_ref().is_some_and(|content|
            content.as_ref().is_ok_and(|doc| doc.dirty() || doc.saving || doc.reloading)));
        if !protected { return false; }
        let saving = self.files.tabs.iter().any(|tab| tab.content.as_ref().is_some_and(|content|
            content.as_ref().is_ok_and(|doc| doc.saving)));
        let busy = saving || self.files.tabs.iter().any(|tab| tab.content.as_ref().is_some_and(|content|
            content.as_ref().is_ok_and(|doc| doc.reloading)));
        let session = owner.as_ref().map(|owner| owner.2.as_str()).unwrap_or("");
        let title = tr("file_other_session_title").replace("{session}", session);
        let body = tr("file_other_session_body");
        let focus = window.focused(cx);
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, _| {
            let (weak, owner, target, path, focus) = (weak.clone(), owner.clone(), target.clone(), path.clone(), focus.clone());
            popup::dialog(dialog).w(rems(32.).to_pixels(window.rem_size())).title(title.clone())
                .child(div().v_flex().gap_2().text_sm().child(body.clone())
                    .when(busy, |body| body.child(tr(if saving { "file_saving" } else { "file_loading" }))))
                .footer(div().h_flex().justify_end().gap_2()
                    .child(Button::new("file-owner-cancel").label(tr("cancel"))
                        .on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(Button::new("file-owner-discard").danger().label(tr("file_discard_open")).disabled(busy)
                        .on_click(move |_, window, cx| {
                            window.close_dialog(cx);
                            let _ = weak.update(cx, |this, cx| {
                                if this.files.owner != owner || this.session_owner() != target { return; }
                                if this.files.tabs.iter().any(|tab| tab.content.as_ref().is_some_and(|content|
                                    content.as_ref().is_ok_and(|doc| doc.saving || doc.reloading))) { return; }
                                if this.files.preview_find.target.is_some() { this.close_preview_find(window, cx); }
                                for tab in std::mem::take(&mut this.files.tabs) {
                                    this.stop_audio(&format!("file:{}", tab.id));
                                    release(tab, window, cx);
                                }
                                this.files.owner = None;
                                this.files.active = 0;
                                focus.clone().filter(|focus| this.root_focus.contains(focus, window))
                                    .unwrap_or_else(|| this.root_focus.clone()).focus(window, cx);
                                this.open_file(path.clone(), line, window, cx);
                            });
                        })))
                .on_ok(super::super::machines::enter_to_focused)
        });
        true
    }

    pub(super) fn request_close_file(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(tab) = self.files.tabs.iter().find(|tab| tab.id == id) else { return true };
        let Some(Ok(doc)) = &tab.content else { return false };
        if doc.saving || doc.reloading { return true; }
        if !doc.dirty() { return false; }
        self.confirm_file_change(id, Change::Close, window, cx);
        true
    }

    pub(super) fn request_discard_file(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if !self.files_visible() { return true; }
        let tab = &self.files.tabs[self.files.active];
        let Some(Ok(doc)) = &tab.content else { return true };
        if doc.saving || doc.reloading { return true; }
        if !doc.dirty() { return false; }
        self.confirm_file_change(tab.id, Change::Discard, window, cx);
        true
    }

    pub(super) fn reload_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let tab = &self.files.tabs[self.files.active];
        let Some(Ok(doc)) = &tab.content else { return };
        if doc.saving || doc.reloading { return; }
        let id = tab.id;
        if doc.dirty() { self.confirm_file_change(id, Change::Reload, window, cx); }
        else { self.read_file_refresh(id, true, cx); }
    }

    fn confirm_file_change(&mut self, id: u64, change: Change, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.files.tabs.iter().find(|tab| tab.id == id) else { return };
        let path = tab.path.clone();
        let owner = self.files.owner.clone();
        let weak = cx.entity().downgrade();
        let (title, body, action) = match change {
            Change::Close => ("file_unsaved_title", "file_unsaved_body", "file_discard_close"),
            Change::Discard => ("file_discard_title", "file_discard_body", "file_discard"),
            Change::Reload => ("file_reload_title", "file_reload_body", "file_reload"),
        };
        window.open_dialog(cx, move |dialog, window, _| {
            let run = |button_id: &'static str, label: &'static str, save: bool| {
                let (weak, owner) = (weak.clone(), owner.clone());
                Button::new(button_id).label(tr(label)).when(save, |button| button.primary()).when(!save, |button| button.danger())
                    .on_click(move |_, window, cx| {
                        window.close_dialog(cx);
                        let _ = weak.update(cx, |this, cx| {
                            if this.files.owner != owner || this.session_owner() != owner { return; }
                            this.apply_file_change(id, change, save, window, cx);
                        });
                    })
            };
            popup::dialog(dialog).w(rems(32.).to_pixels(window.rem_size())).title(tr(title))
                .child(div().v_flex().gap_2().text_sm()
                    .child(div().font_family(theme::MONO).child(path.clone()))
                    .child(tr(body)))
                .footer(div().h_flex().justify_end().gap_2()
                    .child(Button::new("file-change-cancel").label(tr("cancel"))
                        .on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(run("file-change-discard", action, false))
                    .when(matches!(change, Change::Close), |footer| footer.child(run("file-change-save", "file_save_close", true))))
                .on_ok(super::super::machines::enter_to_focused)
        });
    }

    fn apply_file_change(&mut self, id: u64, change: Change, save: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.files.tabs.iter().position(|tab| tab.id == id) else { return };
        let Some(Ok(doc)) = &mut self.files.tabs[ix].content else { return };
        if doc.saving || doc.reloading { return; }
        match change {
            Change::Close if save => {
                if !doc.dirty() { self.finish_close_file(id, window, cx); return; }
                if !doc.editable() { return; }
                doc.close_after_save = true;
                self.files.active = ix;
                self.focus_file(window, cx);
                self.save_file(cx);
                // Sem API disponível, o pedido de fechar não pode ficar armado para um salvamento futuro.
                if let Some(Ok(doc)) = &mut self.files.tabs[ix].content {
                    if !doc.saving { doc.close_after_save = false; }
                }
            }
            Change::Close => self.finish_close_file(id, window, cx),
            Change::Discard => {
                self.files.active = ix;
                self.finish_discard_file(window, cx);
            }
            Change::Reload => self.read_file_refresh(id, true, cx),
        }
    }

    fn poll_file_changes(&mut self, cx: &mut Context<Self>) {
        if !self.files_visible() { return; }
        let Some(tab) = self.files.tabs.get(self.files.active) else { return };
        let Some(Ok(doc)) = &tab.content else { return };
        // Arquivo grande não é relido a cada 5 s: Recarregar continua valendo.
        if doc.saving || doc.reloading || doc.checking || doc.large() { return; }
        self.read_file_refresh(tab.id, false, cx);
    }

    fn read_file_refresh(&mut self, id: u64, reload: bool, cx: &mut Context<Self>) {
        let (Some(api), Some(key)) = (self.session_api(), self.selected_key()) else { return };
        let local = self.tree.local_root(&self.session_owner());
        let cwd = self.selected.as_ref().and_then(|session| session.cwd.clone());
        let Some(tab) = self.files.tabs.iter_mut().find(|tab| tab.id == id) else { return };
        let Some(Ok(doc)) = &mut tab.content else { return };
        if doc.saving || doc.reloading || (!reload && doc.checking) { return; }
        doc.read_seq = doc.read_seq.wrapping_add(1);
        let seq = doc.read_seq;
        let path = doc.base.path.clone();
        let external = doc.base.external;
        doc.checking = !reload;
        doc.reloading = reload;
        if reload {
            doc.error = None;
            doc.editor.update(cx, |state, cx| state.set_readonly(true, cx));
        }
        let (connection, tx) = (self.connection, self.tx.clone());
        self.runtime.spawn(async move {
            let here = local.is_some() || (api.is_loopback() && cwd.is_some_and(|cwd| std::path::Path::new(&cwd).is_dir()));
            let result = read_open_file(api, key.name, path, external, local, here, reload).await;
            let _ = tx.send(Envelope { connection, selection: None,
                payload: Payload::FileView(FileReply::Refresh(id, seq, reload, result)) }).await;
        });
        if reload { cx.notify(); }
    }

    pub(super) fn file_refreshed(&mut self, id: u64, seq: u64, reload: bool, result: Result<Content, Failure>, window: &mut Window,
        cx: &mut Context<Self>) {
        let restore_cursor = self.files_visible() && self.files.tabs.get(self.files.active).is_some_and(|tab| tab.id == id)
            && self.files.focus.contains_focused(window, cx);
        let word_wrap = self.files.word_wrap;
        let Some(tab) = self.files.tabs.iter_mut().find(|tab| tab.id == id) else { return };
        let Some(Ok(doc)) = &mut tab.content else { return };
        if doc.read_seq != seq || doc.saving { return; }
        let previous = (doc.disk_changed, doc.error.clone(), doc.poll_error.clone());
        (doc.checking, doc.reloading) = (false, false);
        match result {
            Ok(mut content) if reload => {
                // A classificação de arquivo grande vale para o texto relido: ele pode ter crescido ou diminuído. Grande,
                // o texto vai para o editor sem ficar uma cópia em `base`.
                let large = content.text.len() > hangar_workspace::files::MAX_BYTES;
                let reclassified = large != doc.large();
                doc.large_size = large.then_some(content.text.len());
                if let Some(markdown) = doc.markdown.as_ref().filter(|_| !large) { markdown.update(cx, |state, cx| state.set_text(&content.text, cx)); }
                // Cresceu além do teto: a prévia de Markdown sai e a aba mostra o código. O caminho inverso não cria a
                // prévia: como a linguagem de destaque, ela volta ao reabrir a aba.
                if large && doc.markdown.is_some() {
                    (doc.markdown, doc._markdown_changed) = (None, None);
                    tab.reader.set_mode(super::reader::ReaderMode::Source);
                }
                if doc.table.is_some() { doc.table = content.table.take(); doc.table_stale = false; }
                let text = if large { std::mem::take(&mut content.text) } else { content.text.clone() };
                doc.editor.update(cx, |state, cx| {
                    let cursor = state.cursor_position();
                    state.set_value(text, window, cx);
                    state.set_soft_wrap(word_wrap && !large, window, cx);
                    if reclassified { state.set_highlighter(if large { "text" } else { super::file_language(&tab.path) }, cx); }
                    if restore_cursor {
                        let row = cursor.line.min(state.text().lines_len().saturating_sub(1) as u32);
                        state.set_cursor_position(Position::new(row, cursor.character), window, cx);
                    }
                });
                doc.base = content;
                (doc.dirty, doc.disk_changed, doc.close_after_save) = (false, false, false);
                (doc.error, doc.saved) = (None, None);
                doc.poll_error = None;
            }
            Ok(content) => {
                doc.disk_changed = content.digest != doc.base.digest || content.text != doc.base.text;
                doc.poll_error = None;
            }
            Err(error) if reload => doc.error = Some(file_failure(&error)),
            Err(error) => doc.poll_error = Some(file_failure(&error)),
        }
        if reload { doc.editor.update(cx, |state, cx| state.set_readonly(!doc.editable(), cx)); }
        if reload || previous != (doc.disk_changed, doc.error.clone(), doc.poll_error.clone()) { cx.notify(); }
    }
}

/// A tabela do CSV só é remontada no Recarregar; a verificação a cada 5 s não precisa dela.
fn table_on_reload(content: Content, reload: bool) -> Content { if reload { super::with_table(content) } else { content } }

/// `large` só no Recarregar: a verificação a cada 5 s lê até 512 KB, e um arquivo que cresceu além disso aparece como
/// mudado no disco em vez de ser relido inteiro.
async fn read_open_file(api: Api, name: String, path: String, external: bool, local: Option<PathBuf>, here: bool, large: bool) -> Result<Content, Failure> {
    if !external {
        if let Some(root) = local {
            return tokio::task::spawn_blocking(move || super::super::tree::read_local(&root, &path, large).map(|read|
                table_on_reload(Content { path, text: read.text, truncated: read.truncated, digest: read.digest, external: false, table: None }, large)))
                .await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
        }
    } else if here {
        let file = path.strip_prefix("~/").and_then(|rest| super::super::tree::home_dir().map(|home| home.join(rest)))
            .unwrap_or_else(|| PathBuf::from(&path));
        if file.is_absolute() {
            return tokio::task::spawn_blocking(move || {
                let real = std::fs::canonicalize(file).map_err(|_| Failure::local("erro_arq_inexistente"))?;
                if real.components().any(|part| part.as_os_str() == ".git") { return Err(Failure::local("erro_arq_area_do_git")); }
                let (Some(folder), Some(file_name)) = (real.parent(), real.file_name().and_then(|name| name.to_str()))
                    else { return Err(Failure::local("erro_arq_inexistente")) };
                super::super::tree::read_local(folder, file_name, large).map(|read|
                    table_on_reload(Content { path, text: read.text, truncated: read.truncated, digest: read.digest, external: true, table: None }, large))
            }).await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
        }
    }
    let route = if external { ["file", "text"] } else { ["files", "read"] };
    let large = if large { "1" } else { "0" };
    let mut content: Content = serde_json::from_value(api.read(&name, &route, &[("path", path.as_str()), ("large", large)], if large == "1" { LARGE_READ_SECONDS } else { 30 }).await?)
        .map_err(|_| Failure::local("invalid_response"))?;
    if content.path != path { return Err(Failure::local("invalid_response")); }
    content.external = external;
    Ok(table_on_reload(content, large == "1"))
}
