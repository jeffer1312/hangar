//! Exportar e importar os atalhos sem credencial: o backend (`/api/shortcuts/export` e `/import`, crate `hangar-shortcut-transfer`)
//! troca cada senha por `⟦SEGREDO:<nome>⟧` na saída e, na entrada, preenche os valores que a pessoa digitou. Aqui fica
//! a seleção do pacote, a prévia dos arquivos e o formulário mascarado dos marcadores.
use super::*;
use super::shortcuts::ShortcutsReply;
use super::sidebar::menu_style;
use gpui_kit::component::spinner::Spinner;

/// Importação conferida pelo backend, esperando a pessoa confirmar.
pub(super) struct ImportDraft {
    api: Api,
    connection: u64,
    data: Value,
    added: u64,
    replaced: u64,
    /// (id do atalho, rótulo, nome do marcador, campo mascarado)
    fields: Vec<(String, String, String, Entity<InputState>)>,
    files: Vec<Value>,
    verify_commands: Vec<(String, String)>,
    expanded: HashSet<String>,
    _changes: Vec<Subscription>,
    pub(super) applying: bool,
    /// Sessão aberta na hora de importar: recebe o pedido de correção quando um `verify` falha.
    target: Option<crate::delivery::SessionKey>,
}

/// Um `verify` rodado pelo backend depois de importar.
pub(super) struct Check { label: String, ok: bool, output: String }

/// As verificações e o pedido de correção: `fix` diz se foi entregue à sessão aberta (nome) ou o erro do envio;
/// `None` com falha = não havia sessão desta máquina aberta. `skipped`: (rótulo ou id, motivo) dos que o backend não rodou.
pub(super) struct Checks { list: Vec<Check>, skipped: Vec<(String, String)>, prompt: String, fix: Option<Result<String, String>> }

fn parse_checks(value: &Value) -> (Vec<Check>, Vec<(String, String)>, String) {
    let (mut list, mut prompts) = (Vec::new(), Vec::new());
    for check in value.get("checks").and_then(Value::as_array).into_iter().flatten() {
        let text = |key: &str| check.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        let ok = check.get("code").and_then(Value::as_i64) == Some(0);
        if !ok && !text("prompt").is_empty() { prompts.push(text("prompt")); }
        list.push(Check { label: text("label"), ok, output: text("output") });
    }
    let skipped = value.get("skipped").and_then(Value::as_array).into_iter().flatten().map(|item| {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        (Some(text("label")).filter(|label| !label.is_empty()).unwrap_or_else(|| text("id")), text("motivo"))
    }).collect();
    (list, skipped, prompts.join("\n\n---\n\n"))
}

/// (rótulo ou id, comando) de cada `verify` que roda logo depois de importar.
fn verify_commands(preview: &Value) -> Vec<(String, String)> {
    preview.get("verify_commands").and_then(Value::as_array).into_iter().flatten().filter_map(|item| {
        let text = |key: &str| item.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        let label = Some(text("label")).filter(|label| !label.is_empty()).unwrap_or_else(|| text("id"));
        Some((label, text("command"))).filter(|(_, command)| !command.is_empty())
    }).collect()
}

pub(super) struct ExportDraft {
    api: Api,
    connection: u64,
    items: Vec<Value>,
    selected: HashSet<String>,
    pub(super) loading: bool,
    pub(super) saving: bool,
}

/// Nome da primeira credencial em branco (`⟦SEGREDO:<nome>⟧`) no comando ou texto do atalho.
pub(super) fn missing_secret(text: &str) -> Option<String> {
    let start = text.find("⟦SEGREDO:")? + "⟦SEGREDO:".len();
    let end = text[start..].find('⟧')?;
    let name = &text[start..start + end];
    name.chars().all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c)).then(|| name.to_owned()).filter(|n| !n.is_empty())
}

/// Mantém o pacote completo; a contagem de credenciais removidas fica só no aviso.
fn export_file(value: &Value) -> (String, u64) {
    let mut body = value.clone();
    if let Some(object) = body.as_object_mut() { object.remove("removed"); }
    (serde_json::to_string_pretty(&body).unwrap_or_default() + "\n", value.get("removed").and_then(Value::as_u64).unwrap_or(0))
}

fn warnings(value: &Value) -> Vec<String> {
    value.get("warnings").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
}

fn export_query(ids: &[String]) -> Vec<(&str, &str)> {
    let mut query: Vec<_> = ids.iter().map(|id| ("ids", id.as_str())).collect();
    query.push(("include_scripts", "true"));
    query
}

fn export_supported(value: &Value, include_scripts: bool) -> bool {
    value.get("version").and_then(Value::as_u64) == Some(2)
        && (!include_scripts || value.get("scripts").is_some_and(Value::is_array))
}

fn import_supported(data: &Value, preview: &Value) -> bool {
    data.get("version").and_then(Value::as_u64) != Some(2)
        || preview.get("files").is_some_and(Value::is_array)
}

impl Hangar {
    fn transfer_note(&mut self, text: String, error: bool, cx: &mut Context<Self>) {
        self.shortcuts.transfer_note = Some((text, error));
        cx.notify();
    }

    /// Só busca os scripts depois que a pessoa escolhe quais atalhos levar.
    pub(super) fn export_shortcuts(&mut self, cx: &mut Context<Self>) {
        if self.shortcuts.busy() || self.shortcuts.import.is_some() || self.shortcuts.export.is_some() { return; }
        let Some(api) = self.api.clone() else { return };
        self.shortcuts.transfer_seq += 1;
        let seq = self.shortcuts.transfer_seq;
        self.shortcuts.transfer_note = None;
        self.shortcuts.transfer_warnings.clear();
        self.shortcuts.export = Some(ExportDraft { api: api.clone(), connection: self.connection,
            items: Vec::new(), selected: HashSet::new(), loading: true, saving: false });
        let done = self.shortcuts_send_later();
        self.runtime.spawn(async move {
            let result = api.server_read(&["shortcuts", "export"], &[("include_scripts", "false")], 10).await;
            done(ShortcutsReply::ExportCandidates(seq, result.map_err(|e| Hangar::fetch_failure(&e)))).await;
        });
        cx.notify();
    }

    fn save_shortcut_export(&mut self, cx: &mut Context<Self>) {
        if self.shortcuts.busy() { return; }
        let Some(draft) = self.shortcuts.export.as_mut() else { return };
        if draft.connection != self.connection || draft.selected.is_empty() { return; }
        let api = draft.api.clone();
        let ids: Vec<_> = draft.items.iter().filter_map(|item| item.get("id").and_then(Value::as_str))
            .filter(|id| draft.selected.contains(*id)).map(str::to_owned).collect();
        if ids.is_empty() { return; }
        draft.saving = true;
        let prompt = cx.prompt_for_new_path(&downloads_folder(), Some("hangar-atalhos.json"));
        let (tx, connection, runtime, seq) = (self.tx.clone(), self.connection, self.runtime.handle().clone(), self.shortcuts.transfer_seq);
        cx.spawn(async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(path))) => path,
                other => {
                    let _ = this.update(cx, |this, cx| {
                        if this.connection != connection || this.shortcuts.transfer_seq != seq { return; }
                        if let Some(draft) = this.shortcuts.export.as_mut() { draft.saving = false; }
                        if !matches!(other, Ok(Ok(None))) { this.transfer_note(tr("save_dialog_failed"), true, cx); }
                        cx.notify();
                    });
                    return;
                }
            };
            if !this.update(cx, |this, _| this.connection == connection && this.shortcuts.transfer_seq == seq).unwrap_or(false) { return; }
            let result = runtime.spawn(async move {
                api.server_read(&["shortcuts", "export"], &export_query(&ids), 30).await.map_err(|e| Hangar::fetch_failure(&e))
                    .and_then(|value| if export_supported(&value, true) { Ok(value) }
                        else { Err(tr_shared("shortcut_transfer_update_required", &[])) })
            }).await.unwrap_or_else(|e| Err(e.to_string()));
            if !this.update(cx, |this, _| this.connection == connection && this.shortcuts.transfer_seq == seq).unwrap_or(false) { return; }
            runtime.spawn(async move {
                let result = match result {
                    Err(error) => Err(error),
                    Ok(value) => {
                        let (text, removed) = export_file(&value);
                        let warnings = warnings(&value);
                        tokio::task::spawn_blocking(move || std::fs::write(&path, text).map(|()| (path, removed, warnings))).await
                            .map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| format!("{}: {e}", tr("save_failed"))))
                    }
                };
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Shortcuts(ShortcutsReply::Exported(seq, result)) }).await;
            });
        }).detach();
        cx.notify();
    }

    /// Abre o arquivo e pede ao backend a conferência (contagem e marcadores); nada é gravado ainda.
    pub(super) fn import_shortcuts(&mut self, cx: &mut Context<Self>) {
        if self.shortcuts.busy() || self.shortcuts.import.is_some() || self.shortcuts.export.is_some() { return; }
        let Some(api) = self.api.clone() else { return };
        self.shortcuts.transfer_seq += 1;
        self.shortcuts.import_loading = true;
        self.shortcuts.transfer_note = None;
        self.shortcuts.transfer_warnings.clear();
        if !self.shortcuts.verifying { self.shortcuts.checks = None; }
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        let (tx, connection, runtime, seq) = (self.tx.clone(), self.connection, self.runtime.handle().clone(), self.shortcuts.transfer_seq);
        cx.spawn(async move |this, cx| {
            let path = match prompt.await {
                Ok(Ok(Some(mut paths))) if !paths.is_empty() => paths.remove(0),
                other => {
                    let _ = this.update(cx, |this, cx| {
                        if this.connection != connection || this.shortcuts.transfer_seq != seq { return; }
                        this.shortcuts.import_loading = false;
                        if !matches!(other, Ok(Ok(_))) { this.transfer_note(tr("picker_failed"), true, cx); }
                        cx.notify();
                    });
                    return;
                }
            };
            if !this.update(cx, |this, _| this.connection == connection && this.shortcuts.transfer_seq == seq).unwrap_or(false) { return; }
            runtime.spawn(async move {
                let parsed = tokio::task::spawn_blocking(move || std::fs::read_to_string(&path)).await
                    .map_err(|e| e.to_string()).and_then(|r| r.map_err(|e| e.to_string()))
                    .and_then(|text| serde_json::from_str::<Value>(&text).map_err(|e| e.to_string()));
                let reply = match parsed {
                    Err(error) => ShortcutsReply::Previewed(seq, Value::Null, Err(error)),
                    Ok(data) => {
                        let result = api.server_send(reqwest::Method::POST, &["shortcuts", "import"], Some(json!({"data": data.clone()})), 10).await;
                        ShortcutsReply::Previewed(seq, data, result.map_err(|e| Hangar::fetch_failure(&e)))
                    }
                };
                let _ = tx.send(Envelope { connection, selection: None, payload: Payload::Shortcuts(reply) }).await;
            });
        }).detach();
        cx.notify();
    }

    fn apply_import(&mut self, cx: &mut Context<Self>) {
        // Uma gravação da lista em voo e a importação não se cruzam: a segunda apagaria a primeira.
        if self.shortcuts.busy() { return; }
        let target = self.selected_key();
        let Some(draft) = self.shortcuts.import.as_mut() else { return };
        if draft.applying || draft.connection != self.connection { return; }
        draft.target = target;
        if draft.fields.iter().any(|(id, _, _, input)| id.starts_with("script:") && input.read(cx).value().is_empty()) { return; }
        let api = draft.api.clone();
        let seq = self.shortcuts.transfer_seq;
        draft.applying = true;
        let mut secrets = serde_json::Map::new();
        for (id, _, name, input) in &draft.fields {
            let value = input.read(cx).value().to_string();
            let entry = secrets.entry(id.clone()).or_insert_with(|| json!({}));
            entry[name.as_str()] = json!(value);
        }
        let body = json!({"data": draft.data.clone(), "apply": true, "secrets": secrets});
        let done = self.shortcuts_send_later();
        self.runtime.spawn(async move {
            let result = api.server_send(reqwest::Method::POST, &["shortcuts", "import"], Some(body), 10).await;
            done(ShortcutsReply::Imported(seq, result.map_err(|e| Hangar::fetch_failure(&e)))).await
        });
        cx.notify();
    }

    /// Roda o `verify` dos atalhos recém-importados e, se algum falhar, entrega o pedido de correção à sessão aberta.
    fn verify_imported(&mut self, draft: ImportDraft, ids: Vec<String>, cx: &mut Context<Self>) {
        let scripts: Vec<String> = draft.files.iter().filter_map(|f| f.get("path").and_then(Value::as_str)).map(str::to_owned).collect();
        // Os scripts foram instalados na máquina da importação: sessão de outra máquina não os alcança.
        let identity = draft.api.identity();
        let target = draft.target.filter(|key| key.server == identity)
            .and_then(|key| self.api_for(&key.server).map(|api| (api, key.name)));
        let (api, seconds) = (draft.api, 60 * ids.len().min(20) as u64 + 15);
        self.shortcuts.verifying = true;
        self.shortcuts.checks = None;
        let done = self.shortcuts_send_later();
        self.runtime.spawn(async move {
            let body = json!({"ids": ids, "scripts": scripts});
            let value = match api.server_send(reqwest::Method::POST, &["shortcuts", "verify"], Some(body), seconds).await {
                Ok(value) => value,
                Err(error) => return done(ShortcutsReply::Verified(Err(Hangar::fetch_failure(&error)))).await,
            };
            let (list, skipped, prompt) = parse_checks(&value);
            let fix = match target {
                Some((api, name)) if !prompt.is_empty() =>
                    Some(api.send(&name, &prompt).await.map(|_| name).map_err(|e| Hangar::fetch_failure(&e))),
                _ => None,
            };
            done(ShortcutsReply::Verified(Ok(Checks { list, skipped, prompt, fix }))).await
        });
        cx.notify();
    }

    /// Sem sessão desta máquina aberta: o pedido vai para a área de transferência e abre a criação de sessão.
    fn open_fix_session(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(checks) = &self.shortcuts.checks { cx.write_to_clipboard(ClipboardItem::new_string(checks.prompt.clone())); }
        self.open_new_session(None, window, cx);
    }

    pub(super) fn receive_transfer(&mut self, reply: ShortcutsReply, window: &mut Window, cx: &mut Context<Self>) {
        let seq = match &reply {
            ShortcutsReply::ExportCandidates(seq, _) | ShortcutsReply::Exported(seq, _)
                | ShortcutsReply::Previewed(seq, ..) | ShortcutsReply::Imported(seq, _) => *seq,
            // Resultado de atalhos já gravados: vale mesmo com outro exportar/importar começado depois.
            ShortcutsReply::Verified(..) => self.shortcuts.transfer_seq,
            _ => return,
        };
        if seq != self.shortcuts.transfer_seq { return; }
        match reply {
            ShortcutsReply::ExportCandidates(_, result) => {
                let Some(draft) = self.shortcuts.export.as_mut() else { return };
                draft.loading = false;
                match result {
                    Ok(value) if export_supported(&value, false) => {
                        draft.items = value.get("shortcuts").and_then(Value::as_array).cloned().unwrap_or_default();
                        self.shortcuts.transfer_warnings = warnings(&value);
                    }
                    Ok(_) => {
                        self.shortcuts.export = None;
                        self.transfer_note(tr_shared("shortcut_transfer_update_required", &[]), true, cx);
                    }
                    Err(error) => { self.shortcuts.export = None; self.transfer_note(error, true, cx); }
                }
                cx.notify();
            }
            ShortcutsReply::Exported(_, Ok((path, removed, warnings))) => {
                self.shortcuts.export = None;
                self.shortcuts.transfer_warnings = warnings;
                let name = path.display().to_string();
                let text = if removed > 0 { tr("shortcuts_exported_secrets").replace("{path}", &name).replace("{n}", &removed.to_string()) }
                    else { tr("shortcuts_exported").replace("{path}", &name) };
                self.transfer_note(text, false, cx);
            }
            ShortcutsReply::Exported(_, Err(error)) => {
                if let Some(draft) = self.shortcuts.export.as_mut() { draft.saving = false; }
                self.transfer_note(error, true, cx);
            }
            ShortcutsReply::Previewed(_, data, Ok(preview)) => {
                self.shortcuts.import_loading = false;
                if !import_supported(&data, &preview) {
                    self.transfer_note(tr_shared("shortcut_transfer_update_required", &[]), true, cx);
                    return;
                }
                let Some(api) = self.api.clone() else { return };
                let mut fields = Vec::new();
                let mut changes = Vec::new();
                for p in preview.get("placeholders").and_then(Value::as_array).into_iter().flatten() {
                    let (Some(id), label) = (p.get("id").and_then(Value::as_str), p.get("label").and_then(Value::as_str).unwrap_or("")) else { continue };
                    for name in p.get("names").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                        let input = cx.new(|cx| InputState::new(window, cx).masked(true));
                        changes.push(cx.subscribe(&input, |_, _, _: &InputEvent, cx| cx.notify()));
                        fields.push((id.to_owned(), label.to_owned(), name.to_owned(), input));
                    }
                }
                self.shortcuts.transfer_note = None;
                self.shortcuts.transfer_warnings = warnings(&preview);
                self.shortcuts.import = Some(ImportDraft { api, connection: self.connection, data,
                    files: preview.get("files").and_then(Value::as_array).cloned().unwrap_or_default(),
                    verify_commands: verify_commands(&preview), expanded: HashSet::new(), _changes: changes,
                    added: preview.get("added").and_then(Value::as_u64).unwrap_or(0),
                    replaced: preview.get("replaced").and_then(Value::as_u64).unwrap_or(0), fields, applying: false, target: None });
                cx.notify();
            }
            ShortcutsReply::Previewed(_, _, Err(error)) => {
                self.shortcuts.import_loading = false;
                self.transfer_note(tr("shortcuts_import_invalid").replace("{error}", &error), true, cx);
            }
            ShortcutsReply::Imported(_, Ok(value)) => {
                let draft = self.shortcuts.import.take();
                self.transfer_note(tr("shortcuts_imported"), false, cx);
                self.reload_shortcuts(cx);
                let ids: Vec<String> = value.get("verify").and_then(Value::as_array).into_iter().flatten()
                    .filter_map(Value::as_str).map(str::to_owned).collect();
                if let (false, Some(draft)) = (ids.is_empty(), draft) { self.verify_imported(draft, ids, cx); }
            }
            ShortcutsReply::Verified(result) => {
                self.shortcuts.verifying = false;
                match result {
                    Ok(checks) => self.shortcuts.checks = Some(checks),
                    Err(error) => self.transfer_note(tr("shortcuts_verify_failed").replace("{error}", &error), true, cx),
                }
                cx.notify();
            }
            ShortcutsReply::Imported(_, Err(error)) => {
                if let Some(draft) = self.shortcuts.import.as_mut() { draft.applying = false; }
                self.transfer_note(tr("shortcuts_import_invalid").replace("{error}", &error), true, cx);
            }
            _ => {}
        }
    }

    /// Os dois caminhos abrem a página onde a seleção e a conferência aparecem.
    pub(super) fn transfer_menu_button(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let weak = cx.weak_entity();
        Button::new("side-shortcut-transfer").ghost().xsmall().icon(IconName::Ellipsis)
            .tooltip(tr("shortcuts_transfer")).accessibility_label(tr("shortcuts_transfer"))
            .dropdown_menu_with_anchor(Anchor::TopRight, move |menu, _, _| {
                let (import, export) = (weak.clone(), weak.clone());
                menu_style(menu)
                    .item(PopupMenuItem::new(tr("shortcuts_import")).on_click(move |_, window, cx| {
                        let _ = import.update(cx, |this, cx| {
                            if this.shortcuts.busy() { return; }
                            this.open_settings(super::settings::Page::Shortcuts, window, cx);
                            this.import_shortcuts(cx);
                        });
                    }))
                    .item(PopupMenuItem::new(tr("shortcuts_export")).on_click(move |_, window, cx| {
                        let _ = export.update(cx, |this, cx| {
                            if this.shortcuts.busy() { return; }
                            this.open_settings(super::settings::Page::Shortcuts, window, cx);
                            this.export_shortcuts(cx);
                        });
                    }))
            })
    }

    /// Aviso do último exportar/importar (painel lateral e página de Atalhos).
    pub(super) fn transfer_note_element(&self) -> Option<Div> {
        if self.shortcuts.transfer_note.is_none() && self.shortcuts.transfer_warnings.is_empty() { return None; }
        Some(div().w_full().flex().flex_col().gap_2().text_xs().whitespace_normal()
            .children(self.shortcuts.transfer_note.clone().map(|(text, error)| div()
                .text_color(if error { theme::danger() } else { theme::muted() }).child(text)))
            .when(!self.shortcuts.transfer_warnings.is_empty(), |el| el
                .child(div().font_weight(FontWeight::SEMIBOLD).child(tr_shared("shortcut_bundle_warnings", &[])))
                .children(self.shortcuts.transfer_warnings.iter().map(|warning| div().text_color(theme::warning_text()).child(warning.clone())))))
    }

    /// Resultado da verificação dos atalhos importados e o caminho da correção.
    pub(super) fn render_checks(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.shortcuts.verifying {
            return Some(div().w_full().flex().items_center().gap_2().text_xs().text_color(theme::muted())
                .child(Spinner::new().small()).child(tr("shortcuts_verifying")));
        }
        let checks = self.shortcuts.checks.as_ref()?;
        let mut body = div().w_full().flex().flex_col().gap_2().text_xs().whitespace_normal()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("shortcuts_check_title")));
        let total = checks.list.len() + checks.skipped.len();
        if total > 0 {
            body = body.child(div().text_color(theme::muted()).child(tr("shortcuts_check_count")
                .replace("{n}", &checks.list.len().to_string()).replace("{m}", &total.to_string())));
        }
        for (label, reason) in &checks.skipped {
            body = body.child(div().text_color(theme::warning_text())
                .child(tr("shortcuts_check_skipped").replace("{label}", label).replace("{reason}", reason)));
        }
        for check in &checks.list {
            let (text, color) = if check.ok { (tr("shortcuts_check_ok"), theme::success_text()) } else { (tr("shortcuts_check_failed"), theme::danger()) };
            body = body.child(div().text_color(color).child(text.replace("{label}", &check.label)))
                .when(!check.ok && !check.output.is_empty(), |el| el.child(div().p_2().rounded(px(6.)).bg(theme::inset())
                    .font_family(theme::MONO).text_color(theme::muted()).child(check.output.clone())));
        }
        if checks.list.iter().any(|check| !check.ok) {
            body = match &checks.fix {
                Some(Ok(name)) => body.child(div().text_color(theme::muted()).child(tr("shortcuts_fix_sent").replace("{name}", name))),
                Some(Err(error)) => body.child(div().text_color(theme::danger()).child(tr("shortcuts_fix_failed").replace("{error}", error))),
                None => body.child(div().text_color(theme::muted()).child(tr("shortcuts_fix_no_session")))
                    .child(div().child(Button::new("shortcuts-fix-session").outline().small().label(tr("shortcuts_open_session"))
                        .on_click(cx.listener(|this, _, window, cx| this.open_fix_session(window, cx))))),
            };
        }
        Some(body)
    }

    pub(super) fn render_export_draft(&self, cx: &mut Context<Self>) -> Option<Div> {
        let draft = self.shortcuts.export.as_ref()?;
        let mut body = div().mt_4().p_4().rounded_lg().border_1().border_color(theme::border()).bg(theme::inset())
            .flex().flex_col().gap_3()
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr_shared("shortcut_export_title", &[])))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr_shared("shortcut_export_help", &[])))
            .child(div().text_xs().text_color(theme::muted()).child(tr_shared("shortcut_source_server", &[("nome", &draft.api.identity())])))
            .child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr_shared("shortcut_bundle_help", &[])));
        if draft.loading { body = body.child(Spinner::new().small()); }
        else if draft.items.is_empty() { body = body.child(div().text_sm().child(tr_shared("shortcut_export_empty", &[]))); }
        else {
            body = body.child(div().flex().flex_wrap().gap_2()
                .child(Button::new("shortcuts-export-all").ghost().small().label(tr_shared("shortcut_select_all", &[])).disabled(draft.saving)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(draft) = this.shortcuts.export.as_mut() {
                            draft.selected = draft.items.iter().filter_map(|item| item.get("id").and_then(Value::as_str)).map(str::to_owned).collect();
                        }
                        cx.notify();
                    })))
                .child(Button::new("shortcuts-export-none").ghost().small().label(tr_shared("shortcut_select_none", &[])).disabled(draft.saving)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Some(draft) = this.shortcuts.export.as_mut() { draft.selected.clear(); }
                        cx.notify();
                    }))));
            for item in &draft.items {
                let Some(id) = item.get("id").and_then(Value::as_str) else { continue };
                let label = item.get("label").and_then(Value::as_str).unwrap_or(id).to_owned();
                let selected = draft.selected.contains(id);
                let id = id.to_owned();
                body = body.child(Checkbox::new(SharedString::from(format!("shortcut-export-{id}"))).label(label)
                    .checked(selected).disabled(draft.saving).on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        if let Some(draft) = this.shortcuts.export.as_mut() {
                            if *checked { draft.selected.insert(id.clone()); } else { draft.selected.remove(&id); }
                        }
                        cx.notify();
                    })));
            }
        }
        Some(body.child(div().flex().justify_end().gap_2()
            .child(Button::new("shortcuts-export-cancel").outline().small().label(tr("cancel")).disabled(draft.saving)
                .on_click(cx.listener(|this, _, _, cx| {
                    this.shortcuts.transfer_seq += 1;
                    this.shortcuts.export = None;
                    cx.notify();
                })))
            .child(Button::new("shortcuts-export-save").primary().small()
                .label(tr_shared("shortcut_export_selected", &[("n", &draft.selected.len().to_string())])).loading(draft.saving)
                .disabled(self.shortcuts.busy() || draft.selected.is_empty())
                .on_click(cx.listener(|this, _, _, cx| this.save_shortcut_export(cx))))))
    }

    /// Conferência da importação: contagem, um campo mascarado por credencial e confirmar/cancelar.
    pub(super) fn render_import_draft(&self, cx: &mut Context<Self>) -> Option<Div> {
        let draft = self.shortcuts.import.as_ref()?;
        let summary = tr("shortcuts_import_summary").replace("{added}", &draft.added.to_string()).replace("{replaced}", &draft.replaced.to_string());
        let mut body = div().mt(px(16.)).p_4().rounded(px(12.)).border_1().border_color(theme::border()).bg(theme::inset())
            .flex().flex_col().gap(px(10.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("shortcuts_import_title")))
            .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(summary))
            .child(div().text_xs().text_color(theme::muted()).child(tr_shared("shortcut_source_server", &[("nome", &draft.api.identity())])));
        if !draft.files.is_empty() {
            body = body.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr_shared("shortcut_import_files", &[])));
            for file in &draft.files {
                let path = file.get("path").and_then(Value::as_str).unwrap_or("").to_owned();
                let expanded = draft.expanded.contains(&path);
                let status = match file.get("status").and_then(Value::as_str) {
                    Some("create") => tr_shared("shortcut_file_create", &[]),
                    Some("replace") => tr_shared("shortcut_file_replace", &[]),
                    Some("same") => tr_shared("shortcut_file_same", &[]),
                    _ => tr("invalid_response"),
                };
                let content = file.get("content").and_then(Value::as_str).unwrap_or("").to_owned();
                body = body.child(div().flex().flex_col().gap_1()
                    .child(Button::new(SharedString::from(format!("shortcut-import-file-{path}"))).ghost().small().h_auto().w_full()
                        .icon(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                        .accessibility_label(format!("{status} · {path}"))
                        .child(div().flex_1().min_w_0().text_left().whitespace_normal().child(format!("{status} · {path}")))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = this.shortcuts.import.as_mut() {
                                if !draft.expanded.remove(&path) { draft.expanded.insert(path.clone()); }
                            }
                            cx.notify();
                        })))
                    .when(expanded, |el| el.child(div().p_2().font_family(theme::MONO).text_xs().whitespace_normal().child(content))));
            }
        }
        if !draft.verify_commands.is_empty() {
            body = body.child(div().text_sm().font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(tr("shortcuts_import_verify")))
                .children(draft.verify_commands.iter().map(|(label, command)| div().p_2().rounded(px(6.)).bg(theme::elevated())
                    .font_family(theme::MONO).text_xs().whitespace_normal().child(format!("{label}: {command}"))));
        }
        if !draft.fields.is_empty() {
            if draft.fields.iter().any(|(id, ..)| !id.starts_with("script:")) {
                body = body.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr("shortcuts_import_secrets")));
            }
            if draft.fields.iter().any(|(id, ..)| id.starts_with("script:")) {
                body = body.child(div().text_xs().text_color(theme::muted()).whitespace_normal().child(tr_shared("shortcut_script_secrets_required", &[])));
            }
            for (_, label, name, input) in &draft.fields {
                body = body.child(div().flex().flex_col().gap(px(4.))
                    .child(div().text_size(px(12.5)).child(format!("{label}: {name}")))
                    .child(Input::new(input).small().aria_label(format!("{label}: {name}"))));
            }
        }
        Some(body.child(div().flex().justify_end().gap(px(8.))
            .child(Button::new("shortcuts-import-cancel").outline().small().label(tr("cancel"))
                .disabled(draft.applying)
                .on_click(cx.listener(|this, _, _, cx| { this.shortcuts.transfer_seq += 1; this.shortcuts.import = None; cx.notify(); })))
            .child(Button::new("shortcuts-import-apply").primary().small().label(tr("shortcuts_import")).loading(draft.applying)
                .disabled(self.shortcuts.busy() || draft.fields.iter().any(|(id, _, _, input)| id.starts_with("script:") && input.read(cx).value().is_empty()))
                .on_click(cx.listener(|this, _, _, cx| this.apply_import(cx))))))
    }
}

#[cfg(test)]
mod tests {
    // Sem glob: o `test` da gpui colide com o atributo padrão.
    use super::{export_file, export_query, export_supported, import_supported, missing_secret, parse_checks, verify_commands, warnings};
    use serde_json::json;

    #[test]
    fn verify_reports_skipped_and_preview_lists_commands() {
        let (list, skipped, prompt) = parse_checks(&json!({
            "checks": [{"label": "rdp", "code": null, "error": "timeout", "prompt": "conserta"}],
            "skipped": [{"id": "vm", "motivo": "sem verify"}, {"id": "x", "label": "X", "motivo": "limite"}]}));
        assert_eq!((list.len(), list[0].ok, prompt.as_str()), (1, false, "conserta"));
        assert_eq!(skipped, [("vm".to_owned(), "sem verify".to_owned()), ("X".to_owned(), "limite".to_owned())]);
        let commands = verify_commands(&json!({"verify_commands": [{"id": "a", "label": "", "command": "which x"}, {"id": "b", "command": ""}]}));
        assert_eq!(commands, [("a".to_owned(), "which x".to_owned())]);
    }

    #[test]
    fn missing_secret_names_the_first_blank_credential() {
        assert_eq!(missing_secret("xfreerdp /p:⟦SEGREDO:senha⟧ /v:h").as_deref(), Some("senha"));
        assert_eq!(missing_secret("delphi-vm ide"), None);
        assert_eq!(missing_secret("⟦SEGREDO:⟧"), None);
    }

    #[test]
    fn export_file_keeps_version_and_list_but_not_the_count() {
        let (text, removed) = export_file(&json!({"version": 1, "shortcuts": [{"id": "a"}], "removed": 2}));
        assert_eq!(removed, 2);
        let written: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(written, json!({"version": 1, "shortcuts": [{"id": "a"}]}));
    }

    #[test]
    fn selected_bundle_preserves_script_permissions_and_warnings() {
        let bundle = json!({"version": 2, "shortcuts": [{"id": "selected"}], "removed": 1,
            "scripts": [{"path": ".local/bin/run", "content": "echo ⟦SEGREDO:token⟧", "executable": true}],
            "warnings": ["not copied"]});
        let (text, removed) = export_file(&bundle);
        let written: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert_eq!(written["scripts"], bundle["scripts"]);
        assert_eq!(written["warnings"], bundle["warnings"]);
        assert_eq!(warnings(&bundle), ["not copied"]);
        assert_eq!(removed, 1);
        let selected = vec!["selected".to_owned(), "second".to_owned()];
        assert_eq!(export_query(&selected), [("ids", "selected"), ("ids", "second"), ("include_scripts", "true")]);
    }

    #[test]
    fn export_refuses_legacy_servers_and_incomplete_bundles() {
        assert!(!export_supported(&json!({"version": 1, "shortcuts": []}), false));
        assert!(!export_supported(&json!({"shortcuts": [], "scripts": []}), true));
        assert!(export_supported(&json!({"version": 2, "shortcuts": []}), false));
        assert!(!export_supported(&json!({"version": 2, "shortcuts": []}), true));
        assert!(!export_supported(&json!({"version": 2, "scripts": null}), true));
        assert!(export_supported(&json!({"version": 2, "shortcuts": [], "scripts": []}), true));
    }

    #[test]
    fn import_requires_bundle_preview_support_but_keeps_legacy_files() {
        let old = json!({"added": 1, "replaced": 0, "placeholders": []});
        assert!(!import_supported(&json!({"version": 2, "scripts": []}), &old));
        assert!(!import_supported(&json!({"version": 2}), &json!({"files": null})));
        assert!(import_supported(&json!({"version": 2}), &json!({"files": []})));
        assert!(import_supported(&json!({"version": 1}), &old));
        assert!(import_supported(&json!([]), &old));
    }
}
