//! Menu de contexto da árvore e dos citados, como o do explorador do VS Code: abrir, mostrar na pasta do sistema, citar
//! na conversa, copiar o caminho, renomear, criar e mandar para a lixeira. Renomear, criar e lixeira vão pelo servidor,
//! e só dentro da pasta da sessão; mostrar e abrir com o app padrão só quando o arquivo está no disco desta máquina.
use super::*;
use gpui_kit::component::{WindowExt, menu::PopupMenu};
use crate::app::files::renamed_path;
use std::rc::Rc;

/// O que o menu sabe do item: o caminho na pasta da sessão (sem ele, nada de renomear, criar nem lixeira), o caminho
/// que o visor abre, o do disco desta máquina e o caminho inteiro para copiar.
#[derive(Clone)]
pub(super) struct ItemTarget { pub session: Option<String>, pub open: String, pub disk: Option<PathBuf>, pub full: String, pub dir: bool }

fn item(hangar: &WeakEntity<Hangar>, label: String, icon: IconName, run: impl Fn(&mut Hangar, &mut Window, &mut Context<Hangar>) + 'static) -> PopupMenuItem {
    let hangar = hangar.clone();
    PopupMenuItem::new(label).icon(icon).on_click(move |_, window, cx| { let _ = hangar.update(cx, |this, cx| run(this, window, cx)); })
}

pub(super) fn item_menu(hangar: WeakEntity<Hangar>, target: ItemTarget) -> impl Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu + 'static {
    move |menu, _, cx| {
        let Some(entity) = hangar.upgrade() else { return menu };
        let ops = entity.read(cx).tree_writable() && target.session.is_some();
        let t = target.clone();
        let (open, full, rel, disk, dir) = (t.open.clone(), t.full.clone(), t.session.clone(), t.disk.clone(), t.dir);
        let trash_label = tr("tree_trash");
        let trash_target = t.clone();
        let weak = hangar.clone();
        sidebar::menu_style(menu).min_w(px(240.))
            .when(!dir, |m| m.item(item(&hangar, tr("tree_open"), IconName::FileText, move |this, window, cx| this.open_file(open.clone(), None, window, cx))))
            .when(dir && ops, |m| {
                let (a, b) = (rel.clone(), rel.clone());
                m.item(item(&hangar, tr("tree_new_file"), IconName::FilePlus, move |this, window, cx| this.tree_create_dialog(a.clone(), false, window, cx)))
                    .item(item(&hangar, tr("tree_new_folder"), IconName::Folder, move |this, window, cx| this.tree_create_dialog(b.clone(), true, window, cx)))
            })
            .when_some(disk, |m, disk| {
                let shown = disk.clone();
                m.when(!dir, |m| { let disk = disk.clone(); m.item(item(&hangar, tr("tree_open_system"), IconName::ExternalLink, move |_, _, cx| cx.open_with_system(&disk))) })
                    .item(item(&hangar, tr("tree_reveal"), IconName::FolderOpen, move |_, _, cx| cx.reveal_path(&shown)))
            })
            .separator()
            .item({
                let cite = rel.clone().map_or_else(|| full.clone(), |rel| crate::app::mention::at_mention(&rel));
                item(&hangar, tr("tree_cite"), IconName::AtSign, move |this, window, cx| this.tree_cite(cite.clone(), window, cx))
            })
            .item({ let full = full.clone(); item(&hangar, tr("tree_copy_path"), IconName::Copy, move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(full.clone()))) })
            .when_some(rel.clone(), |m, rel| m.item(item(&hangar, tr("tree_copy_relative"), IconName::Copy, move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(rel.clone())))))
            .when(ops, |m| {
                let renamed = t.clone();
                m.separator()
                    .item(item(&hangar, tr("tree_rename"), IconName::Pencil, move |this, window, cx| this.tree_rename_dialog(renamed.clone(), window, cx)))
                    .item(PopupMenuItem::element(move |_, _| div().text_color(theme::danger()).child(trash_label.clone()))
                        .icon(chrome::small_icon(IconName::Trash, 14., theme::danger()))
                        .on_click(move |_, window, cx| { let _ = weak.update(cx, |this, cx| this.tree_confirm_trash(trash_target.clone(), window, cx)); }))
            })
    }
}

/// Troca nas pastas abertas o caminho renomeado (e o de dentro dele); devolve as que mudaram.
fn move_expanded(expanded: &mut HashSet<String>, old: &str, new: &str, windows: bool) -> Vec<String> {
    let moved: Vec<String> = expanded.iter().filter_map(|dir| renamed_path(dir, old, new, windows)).collect();
    expanded.retain(|dir| renamed_path(dir, old, new, windows).is_none());
    expanded.extend(moved.iter().cloned());
    moved
}

/// Nome do item, para títulos e confirmações. `\` só separa pastas nos caminhos de uma máquina Windows; nos outros
/// sistemas é parte do nome.
fn base(path: &str, windows: bool) -> &str {
    let seps: &[char] = if windows { &['/', '\\'] } else { &['/'] };
    let trimmed = path.trim_end_matches(seps);
    trimmed.rsplit(seps).next().unwrap_or(trimmed)
}

impl Hangar {
    /// Os caminhos da árvore são da máquina dos arquivos: a do app na pasta local, a do servidor na da sessão.
    fn tree_windows(&self) -> bool {
        match self.tree.source.as_ref().and_then(FileSource::root) {
            Some(_) => cfg!(windows),
            None => self.selected.as_ref().and_then(|s| s.cwd.as_deref()).is_some_and(|cwd| cwd.contains('\\')),
        }
    }

    /// Escrever pelo servidor pede sessão com escrita (o convidado só de leitura não tem).
    pub(super) fn tree_writable(&self) -> bool {
        self.session_api().is_some() && self.selected.as_ref().is_some_and(|s| !s.read_only())
    }

    /// Criar na raiz da árvore: só quando a árvore é a pasta da sessão, não outra pasta escolhida no painel.
    pub(super) fn tree_root_writable(&self) -> bool {
        self.tree_writable() && self.tree.custom_root(&self.tree.owner).is_none()
    }

    /// O alvo de uma linha da árvore. Com outra pasta escolhida no painel, o item não é da sessão: sem as escritas.
    pub(super) fn tree_item(&self, rel: &str, dir: bool) -> ItemTarget {
        let disk = self.tree.source.as_ref().and_then(FileSource::root)
            .map(|root| root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)));
        // A pasta da sessão é de outra máquina: o separador é o do caminho dela.
        let remote = |cwd: String| {
            let sep = if cwd.contains('\\') { "\\" } else { "/" };
            format!("{}{sep}{}", cwd.trim_end_matches(['/', '\\']), rel.replace('/', sep))
        };
        let full = disk.as_ref().map(|p| p.to_string_lossy().into_owned())
            .or_else(|| self.selected.as_ref().and_then(|s| s.cwd.clone()).map(remote))
            .unwrap_or_else(|| rel.to_owned());
        let session = self.tree.custom_root(&self.tree.owner).is_none().then(|| rel.to_owned());
        ItemTarget { session, open: self.tree.file_path(rel.to_owned()), disk, full, dir }
    }

    /// Põe o caminho no fim do rascunho e leva o foco para lá.
    fn tree_cite(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.composer.update(cx, |input, cx| {
            let draft = input.value().to_string();
            let gap = if draft.is_empty() || draft.ends_with(char::is_whitespace) { "" } else { " " };
            input.set_value(format!("{draft}{gap}{text} "), window, cx);
            input.focus(window, cx);
        });
    }

    /// Diálogo curto com um campo de nome. Enter e o botão confirmam; vazio ou igual ao de antes não confirma.
    fn tree_name_dialog(&mut self, title: String, ok: String, initial: String, window: &mut Window, cx: &mut Context<Self>,
        submit: impl Fn(&mut Hangar, String, &mut Window, &mut Context<Hangar>) + 'static) {
        let input = cx.new(|cx| InputState::new(window, cx).default_value(initial.clone()));
        let submit = Rc::new(submit);
        let accept = |value: &str, initial: &str| !value.is_empty() && value != initial;
        let (enter_submit, enter_initial) = (submit.clone(), initial.clone());
        self.tree.name_dialog = Some(cx.subscribe_in(&input, window, move |this, state, event: &InputEvent, window, cx| match event {
            InputEvent::PressEnter { .. } => {
                let value = state.read(cx).value().trim().to_owned();
                if !accept(&value, &enter_initial) { return; }
                window.close_dialog(cx);
                this.tree.name_dialog = None;
                enter_submit(this, value, window, cx);
            }
            InputEvent::Change => cx.notify(),
            _ => {}
        }));
        let field = input.clone();
        cx.defer_in(window, move |_, window, cx| field.update(cx, |s, cx| { s.focus(window, cx); s.select_all(window, cx); }));
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            let value = input.read(cx).value().trim().to_owned();
            let (go, submit, close) = (weak.clone(), submit.clone(), weak.clone());
            let typed = value.clone();
            let confirm = Button::new("tree-name-ok").primary().label(ok.clone()).disabled(!accept(&value, &initial))
                .on_click(move |_, window, cx| {
                    window.close_dialog(cx);
                    let _ = go.update(cx, |this, cx| { this.tree.name_dialog = None; submit(this, typed.clone(), window, cx); });
                });
            popup::dialog(dialog).w(px(420.)).margin_top(popup::centered_top(window.viewport_size().height, px(170.)))
                .title(title.clone()).child(Input::new(&input).aria_label(tr("tree_name")))
                .footer(div().flex().justify_end().gap_2()
                    .child(Button::new("tree-name-cancel").label(tr("cancel")).on_click(|_, window, cx| window.close_dialog(cx)))
                    .child(confirm))
                .on_ok(machines::enter_to_focused)
                .on_close(move |_, _, cx| { let _ = close.update(cx, |this, _| this.tree.name_dialog = None); })
        });
    }

    pub(super) fn tree_rename_dialog(&mut self, target: ItemTarget, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rel) = target.session.clone() else { return };
        let windows = self.tree_windows();
        let (name, owner) = (base(&rel, windows).to_owned(), self.session_owner());
        let title = tr("tree_rename_title").replace("{name}", &name);
        self.tree_name_dialog(title, tr("sidebar_rename"), name, window, cx, move |this, new, window, cx| {
            let old = rel.clone();
            this.tree_request(owner.clone(), "rename", json!({"path": old, "name": new}), window, cx, move |this, value, same_root, _, cx| {
                let Some(path) = value.get("path").and_then(Value::as_str).map(str::to_owned) else { return };
                this.files_renamed(&old, &path, windows);
                if same_root {
                    // Pasta renomeada: o que estava aberto nela segue aberto, lido no caminho novo.
                    let moved = move_expanded(&mut this.tree.expanded, &old, &path, windows);
                    this.tree.dirs.retain(|dir, _| renamed_path(dir, &old, &path, windows).is_none());
                    if let Some(reveal) = this.tree.reveal.as_mut() && let Some(new) = renamed_path(reveal, &old, &path, windows) { *reveal = new; }
                    if !moved.is_empty() && let Some(source) = this.tree.source.clone() { this.tree_fetch(source, moved, false, cx); }
                    this.tree.selected = Some(path);
                }
                this.redraw(Area::Side, cx);
            });
        });
    }

    /// Novo arquivo ou pasta em `parent` (`None` é a raiz da sessão).
    pub(super) fn tree_create_dialog(&mut self, parent: Option<String>, dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let folder = parent.as_deref().map_or_else(|| self.tree_root_label(), |p| base(p, self.tree_windows()).to_owned());
        let title = tr(if dir { "tree_new_folder_title" } else { "tree_new_file_title" }).replace("{folder}", &folder);
        let owner = self.session_owner();
        self.tree_name_dialog(title, tr("tree_create"), String::new(), window, cx, move |this, name, window, cx| {
            let parent = parent.clone();
            this.tree_request(owner.clone(), "create", json!({"path": parent, "name": name, "dir": dir}), window, cx, move |this, value, same_root, window, cx| {
                // A árvore passou a mostrar outra pasta: o caminho criado não é dela, e abri-lo pegaria o de mesmo nome lá.
                if !same_root { return; }
                let Some(path) = value.get("path").and_then(Value::as_str).map(str::to_owned) else { return };
                if let Some(parent) = parent.clone() { this.tree.expanded.insert(parent); }
                this.tree.reveal = Some(path.clone());
                this.tree_apply_reveal(cx);
                if !dir { this.open_file(this.tree.file_path(path), None, window, cx); }
            });
        });
    }

    pub(super) fn tree_confirm_trash(&mut self, target: ItemTarget, window: &mut Window, cx: &mut Context<Self>) {
        let Some(rel) = target.session.clone() else { return };
        let (weak, owner) = (cx.entity().downgrade(), self.session_owner());
        chrome::confirm_alert(window, cx, tr("tree_trash_title").replace("{name}", base(&rel, self.tree_windows())), tr("tree_trash_msg"), tr("tree_trash_ok"),
            ButtonVariant::Danger, move |window, cx| {
                let rel = rel.clone();
                let _ = weak.update(cx, |this, cx| this.tree_request(owner.clone(), "trash", json!({"path": rel}), window, cx, |_, _, _, _, _| {}));
                true
            });
    }

    /// Manda a escrita ao servidor da sessão em que o diálogo abriu (`owner`); outra sessão escolhida nesse meio recusa
    /// com aviso, porque o mesmo caminho lá é outro arquivo. Deu certo, relê a árvore, os citados e a busca; `then` sabe se
    /// a árvore ainda mostra a pasta em que a escrita foi feita. Falha vira aviso com o motivo.
    fn tree_request(&mut self, owner: Option<SessionOwner>, route: &'static str, body: Value, window: &mut Window, cx: &mut Context<Self>,
        then: impl FnOnce(&mut Hangar, Value, bool, &mut Window, &mut Context<Hangar>) + 'static) {
        if owner.is_none() || self.session_owner() != owner {
            window.push_notification(Notification::warning(tr("tree_session_changed")), cx);
            return;
        }
        let (Some(api), Some(session)) = (self.session_api(), self.selected.as_ref()) else { return };
        let name = session.name.clone();
        let root = self.tree.custom_root(&owner).map(Path::to_path_buf);
        let job = self.runtime.spawn(async move { api.act(&name, &["files", route], Some(body), false, 30).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(value) => {
                    if this.session_owner() != owner { return; }
                    let same_root = this.tree.custom_root(&owner).map(Path::to_path_buf) == root;
                    this.tree.cited.reset();
                    this.tree_reload(cx);
                    this.tree_begin_search(cx);
                    then(this, value, same_root, window, cx);
                }
                Err(error) => {
                    // A frase do código do servidor (já existe, não sabe de nome…); sem ela, o motivo de sempre.
                    let code = error.code.clone().unwrap_or_else(|| error.detail.clone());
                    let reason = crate::i18n::tr_web(&code, &Default::default()).unwrap_or_else(|| Self::fetch_failure(&error));
                    window.push_notification(Notification::error(tr("tree_op_failed").replace("{reason}", &reason)), cx);
                }
            });
        }).detach();
    }

    /// Nome da pasta mostrada na árvore: a escolhida no painel ou a da sessão.
    pub(super) fn tree_root_label(&self) -> String {
        let path = self.tree.custom_root(&self.tree.owner).map(|p| p.to_string_lossy().into_owned())
            .or_else(|| self.selected.as_ref().and_then(|s| s.cwd.clone())).unwrap_or_default();
        base(&path, self.tree_windows()).to_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    #[test]
    fn renamed_folder_stays_open_with_its_subfolders() {
        let mut expanded: HashSet<String> = ["src", "src/x", "src/x/y", "srcx", "docs"].map(str::to_owned).into();
        let mut moved = super::move_expanded(&mut expanded, "src/x", "src/z", false);
        moved.sort();
        assert_eq!(moved, ["src/z", "src/z/y"]);
        let want: HashSet<String> = ["src", "src/z", "src/z/y", "srcx", "docs"].map(str::to_owned).into();
        assert_eq!(expanded, want);
    }
}
