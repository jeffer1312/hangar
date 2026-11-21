//! Menu de contexto da árvore e dos citados, como o do explorador do VS Code: abrir, mostrar na pasta do sistema, citar
//! na conversa, copiar o caminho, renomear, criar e mandar para a lixeira. Renomear, criar e lixeira vão pelo servidor,
//! e só dentro da pasta da sessão; mostrar e abrir com o app padrão só quando o arquivo está no disco desta máquina.
use super::*;
use gpui_kit::component::{WindowExt, menu::PopupMenu};
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
                let cite = rel.clone().map_or_else(|| full.clone(), |rel| format!("@{rel}"));
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

/// Nome do item, para títulos e confirmações.
fn base(path: &str) -> &str { path.trim_end_matches('/').rsplit('/').next().unwrap_or(path) }

impl Hangar {
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
        let disk = self.tree.source.as_ref().and_then(FileSource::root).map(|root| root.join(rel));
        let full = disk.as_ref().map(|p| p.to_string_lossy().into_owned())
            .or_else(|| self.selected.as_ref().and_then(|s| s.cwd.clone()).map(|cwd| format!("{}/{rel}", cwd.trim_end_matches('/'))))
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
        let name = base(&rel).to_owned();
        let title = tr("tree_rename_title").replace("{name}", &name);
        self.tree_name_dialog(title, tr("sidebar_rename"), name, window, cx, move |this, new, window, cx| {
            let old = rel.clone();
            this.tree_request("rename", json!({"path": old, "name": new}), window, cx, move |this, value, _, cx| {
                let Some(path) = value.get("path").and_then(Value::as_str).map(str::to_owned) else { return };
                this.files_renamed(&old, &path);
                this.tree.selected = Some(path);
                this.redraw(Area::Side, cx);
            });
        });
    }

    /// Novo arquivo ou pasta em `parent` (`None` é a raiz da sessão).
    pub(super) fn tree_create_dialog(&mut self, parent: Option<String>, dir: bool, window: &mut Window, cx: &mut Context<Self>) {
        let folder = parent.as_deref().map_or_else(|| self.tree_root_label(), |p| base(p).to_owned());
        let title = tr(if dir { "tree_new_folder_title" } else { "tree_new_file_title" }).replace("{folder}", &folder);
        self.tree_name_dialog(title, tr("tree_create"), String::new(), window, cx, move |this, name, window, cx| {
            let parent = parent.clone();
            this.tree_request("create", json!({"path": parent, "name": name, "dir": dir}), window, cx, move |this, value, window, cx| {
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
        let weak = cx.entity().downgrade();
        chrome::confirm_alert(window, cx, tr("tree_trash_title").replace("{name}", base(&rel)), tr("tree_trash_msg"), tr("tree_trash_ok"),
            ButtonVariant::Danger, move |window, cx| {
                let rel = rel.clone();
                let _ = weak.update(cx, |this, cx| this.tree_request("trash", json!({"path": rel}), window, cx, |_, _, _, _| {}));
                true
            });
    }

    /// Manda a escrita ao servidor da sessão; deu certo, relê a árvore e os citados. Falha vira aviso com o motivo.
    fn tree_request(&mut self, route: &'static str, body: Value, window: &mut Window, cx: &mut Context<Self>,
        then: impl FnOnce(&mut Hangar, Value, &mut Window, &mut Context<Hangar>) + 'static) {
        let (Some(api), Some(session)) = (self.session_api(), self.selected.as_ref()) else { return };
        let (name, owner) = (session.name.clone(), self.session_owner());
        let job = self.runtime.spawn(async move { api.act(&name, &["files", route], Some(body), false, 30).await });
        cx.spawn_in(window, async move |this, cx| {
            let result = job.await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(value) => {
                    if this.session_owner() != owner { return; }
                    this.tree.cited.reset();
                    this.tree_reload(cx);
                    then(this, value, window, cx);
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
        base(&path).to_owned()
    }
}
