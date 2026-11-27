//! Avisos da CLI do Claude na conversa: uma linha discreta entre fios, que abre o detalhe, e os avisos que o dono
//! mandou ignorar em cada máquina (`/api/notice-ignores`, evento `notice_ignores` da lista).
use super::*;
use std::collections::BTreeSet;
use std::rc::Rc;

/// O que a pessoa fez num aviso desta conversa: ele segue na tela (com Desfazer ou o erro) até ela ser recarregada.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Acted {
    /// Ignorado aqui, com a chave: voltar a mostrar essa chave desfaz todas as linhas dela.
    Ignored(String),
    Failed(String),
    Reinstalling,
    Reinstalled,
    ReinstallFailed(String),
    Reloading,
    /// Buscando a prévia ou abrindo a sessão de conserto.
    FixStarting,
    /// Aberta, com o estado de antes: até a nota "consertando" chegar a linha não volta ao botão.
    FixOpened(Option<FixState>),
    FixFailed(String),
    /// Cancelar da nota do continuar sozinho: pedido e aceito, até a nota trocada pelo desfecho chegar.
    Cancelling,
    CancelFailed(String),
}

/// O botão que o servidor deu ao aviso.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum NoticeAction {
    Reinstall(String),
    Fix,
}

/// Como está o conserto de um aviso, pela nota mais nova da conversa.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FixState {
    Running { child: String },
    Done,
    Failed,
}

/// Evento que o servidor marcou como aviso: o mapa presente, mesmo vazio.
pub(super) fn is_notice_line(event: &ChatEvent) -> bool { event.notice_meta.is_some() }

fn meta<'a>(event: &'a ChatEvent, field: &str) -> Option<&'a str> {
    event.notice_meta.as_ref()?.get(field)?.as_str()
}

pub(super) fn notice_action(event: &ChatEvent) -> Option<NoticeAction> {
    match meta(event, "action")? {
        "reinstall_plugin" => meta(event, "plugin").map(|plugin| NoticeAction::Reinstall(plugin.to_owned())),
        "fix_hook" => Some(NoticeAction::Fix),
        _ => None,
    }
}

/// Nota de conserto: a de andamento muda a linha do aviso e não tem linha própria; a do resultado tem.
pub(super) fn fix_of(event: &ChatEvent) -> Option<&str> { meta(event, "fix_of") }

pub(super) fn is_fix_running_note(event: &ChatEvent) -> bool { fix_of(event).is_some() && meta(event, "state") == Some("running") }

fn attempt(event: &ChatEvent) -> f64 {
    event.notice_meta.as_ref().and_then(|meta| meta.get("attempt")).and_then(Value::as_f64).unwrap_or(0.)
}

/// A tentativa vigente é a de maior `attempt`; entre as notas dela, o resultado vence o andamento.
pub(super) fn fix_state(events: &[ChatEvent], notice_id: &str) -> Option<FixState> {
    let notes: Vec<&ChatEvent> = events.iter().filter(|event| fix_of(event) == Some(notice_id)).collect();
    let latest = notes.iter().map(|event| attempt(event)).fold(f64::NEG_INFINITY, f64::max);
    let current: Vec<&&ChatEvent> = notes.iter().filter(|event| attempt(event) == latest).collect();
    let state = |wanted: &str| current.iter().find(|event| meta(event, "state") == Some(wanted));
    if state("done").is_some() { return Some(FixState::Done); }
    if state("failed").is_some() { return Some(FixState::Failed); }
    state("running").map(|event| FixState::Running { child: meta(event, "child").unwrap_or_default().to_owned() })
}

/// A chave de ignorar; aviso de bloqueio real não tem.
pub(super) fn notice_key(event: &ChatEvent) -> Option<&str> { meta(event, "key") }

/// Resposta que a CLI gravou no lugar da do modelo quando a API falhou: o turno parou.
pub(super) fn is_api_error(event: &ChatEvent) -> bool { meta(event, "level") == Some("error") }

/// Nota do continuar sozinho (`kind: "auto_continue"`): a espera pela rede e o desfecho, trocados no lugar.
pub(super) fn is_auto_continue(event: &ChatEvent) -> bool { meta(event, "kind") == Some("auto_continue") }

/// Horário local (HH:MM) de um campo em segundos do meta.
fn meta_clock(event: &ChatEvent, field: &str) -> Option<String> {
    let at = event.notice_meta.as_ref()?.get(field)?.as_f64()?;
    chrono::DateTime::from_timestamp(at as i64, 0).map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
}

/// O texto da nota pelas chaves, com os horários do meta; estado que este app não conhece fica com o texto do servidor.
pub(super) fn auto_continue_text(event: &ChatEvent) -> String {
    let code = meta(event, "code").unwrap_or_default();
    let text = if meta(event, "cause") == Some(REBOOT_CAUSE) {
        match meta(event, "state") {
            Some("continued") => meta_clock(event, "continued_at").map(|continued| tr_shared("auto_continue_reboot_line", &[("continued", &continued)])),
            Some("failed") => Some(tr_shared("auto_continue_reboot_failed_line", &[("code", code)])),
            _ => None,
        }
    } else {
        meta_clock(event, "stopped_at").and_then(|stopped| match meta(event, "state")? {
            "waiting" => Some(tr_shared("auto_continue_waiting_line", &[("stopped", &stopped)])),
            "continued" => meta_clock(event, "continued_at")
                .map(|continued| tr_shared("auto_continue_line", &[("stopped", &stopped), ("continued", &continued)])),
            "failed" => Some(tr_shared("auto_continue_failed_line", &[("stopped", &stopped), ("code", code)])),
            "cancelled" => Some(tr_shared("auto_continue_cancelled_line", &[("stopped", &stopped)])),
            "skipped" => Some(tr_shared("auto_continue_skipped_line", &[("stopped", &stopped)])),
            _ => None,
        })
    };
    text.unwrap_or_else(|| plain(event).trim().to_owned())
}

/// O `cause` da nota do turno cortado pelo reboot (`hangar_auto_continue::CAUSE_REBOOT`).
const REBOOT_CAUSE: &str = "reboot";

/// Só a espera tem Cancelar: o desfecho já saiu, e a nota do reboot nasce com ele.
pub(super) fn auto_continue_cancellable(event: &ChatEvent) -> bool {
    is_auto_continue(event) && meta(event, "state") == Some("waiting") && meta(event, "cause").is_none()
}

/// O texto do aviso sem o `⚙️` que o servidor põe para o cliente antigo.
fn plain(event: &ChatEvent) -> &str {
    let text = event.text.as_deref().unwrap_or("");
    text.strip_prefix("⚙️").map(str::trim_start).unwrap_or(text)
}

pub(super) fn notice_title(event: &ChatEvent) -> String {
    match meta(event, "subject") {
        Some(hook) => tr_shared("notice_hook_failed_line", &[("name", hook)]),
        None => plain(event).lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or_default().to_owned(),
    }
}

/// "12 vezes", só quando o hook falhou mais de uma.
pub(super) fn notice_count(event: &ChatEvent) -> Option<String> {
    let count = event.notice_meta.as_ref()?.get("count")?.as_u64().filter(|count| *count > 1)?;
    Some(tr_shared("notice_count", &[("count", &count.to_string())]))
}

/// O que abre ao clicar: o detalhe do hook, ou o aviso inteiro quando ele não cabe na linha.
pub(super) fn notice_detail(event: &ChatEvent) -> Option<String> {
    let detail = event.hook_error.as_deref().unwrap_or_else(|| plain(event)).trim();
    (!detail.is_empty() && (event.hook_error.is_some() || detail != notice_title(event))).then(|| detail.to_owned())
}

pub(super) fn notice_hidden(event: &ChatEvent, ignored: &BTreeSet<String>) -> bool {
    notice_key(event).is_some_and(|key| ignored.contains(key))
}

/// Como uma chave ignorada aparece na lista das Configurações; a que este app não conhece aparece crua.
pub(super) fn notice_key_label(key: &str) -> String {
    if let Some(hook) = key.strip_prefix("hook:") { return tr_shared("notice_key_hook", &[("name", hook)]); }
    if let Some(name) = key.strip_prefix("note:notification:") { return tr_shared("notice_key_notification", &[("name", name)]); }
    if key.starts_with("note:startup:") { return tr_shared("notice_key_startup", &[]); }
    if let Some(plugin) = key.strip_prefix("note:plugin:") { return tr_shared("notice_key_plugin", &[("name", plugin)]); }
    if key == "note:retry" { return tr_shared("notice_key_retry", &[]); }
    key.to_owned()
}

/// `{"keys":[…]}` da rota e do evento.
pub(super) fn parse_keys(data: &Value) -> Option<BTreeSet<String>> {
    data.get("keys")?.as_array()?.iter().map(|key| key.as_str().map(str::to_owned)).collect()
}

/// Guarda a lista de uma máquina; `Some(true)` quando mudou, `None` quando o evento não é uma lista.
pub(super) fn store(map: &mut HashMap<String, BTreeSet<String>>, server: &str, data: &Value) -> Option<bool> {
    let keys = parse_keys(data)?;
    if map.get(server) == Some(&keys) { return Some(false); }
    map.insert(server.to_owned(), keys);
    Some(true)
}

impl Hangar {
    fn ignored_here(&self) -> Option<&BTreeSet<String>> { self.notice_ignores.get(&self.open_server()) }

    /// Aviso ignorado nesta máquina some da conversa, menos o que a pessoa acabou de ignorar aqui (fica com Desfazer).
    pub(super) fn notice_row_hidden(&self, event: &ChatEvent) -> bool {
        if is_fix_running_note(event) { return true; }
        is_notice_line(event) && !self.notice_acted.contains_key(&event.id)
            && self.ignored_here().is_some_and(|ignored| notice_hidden(event, ignored))
    }

    /// Chegou a lista da máquina (evento da lista, a fonte em ordem): o "ignorado · Desfazer" de uma chave que outro
    /// aparelho voltou a mostrar sai, e as linhas da conversa aberta mudam de altura.
    pub(super) fn notice_ignores_changed(&mut self, server: &str, cx: &mut Context<Self>) {
        if server != self.open_server() { return; }
        if let Some(keys) = self.notice_ignores.get(server).cloned() {
            self.notice_acted.retain(|_, acted| !matches!(acted, Acted::Ignored(key) if !keys.contains(key)));
        }
        self.redraw_notice_lines(cx);
    }

    /// Ações locais valem até recarregar a conversa; o erro da lista das Configurações fica.
    pub(super) fn forget_notice_actions(&mut self) {
        self.notice_acted.retain(|_, acted| matches!(acted, Acted::Failed(_)));
    }

    /// Reinstala pela rota de plugins da máquina da conversa; a linha mostra o andamento.
    fn reinstall_plugin(&mut self, event_id: String, plugin: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.session_api() else { return };
        self.notice_acted.insert(event_id.clone(), Acted::Reinstalling);
        self.redraw_notice_lines(cx);
        run(&self.runtime, async move { api.server_send(reqwest::Method::POST, &["plugins", &plugin, "install"], None, 240).await }, window, cx,
            move |this, result, _, cx| {
                let acted = match result {
                    Ok(body) if body.pointer("/result/outcome").and_then(Value::as_str) != Some("failed") => Acted::Reinstalled,
                    Ok(body) => Acted::ReinstallFailed(body.pointer("/result/message").and_then(Value::as_str).unwrap_or_default().to_owned()),
                    Err(error) => Acted::ReinstallFailed(Hangar::fetch_failure(&error)),
                };
                this.notice_acted.insert(event_id, acted);
                this.redraw_notice_lines(cx);
            });
    }

    fn reload_after_reinstall(&mut self, event_id: String, cx: &mut Context<Self>) {
        self.notice_acted.insert(event_id, Acted::Reloading);
        self.reload(cx);
        self.redraw_notice_lines(cx);
    }

    /// "Consertar…": busca a prévia (o que a filha vai receber) e abre a janela de confirmação.
    /// O aviso como o servidor precisa: o hook, o erro e se ele bloqueia (sem chave de ignorar).
    fn fix_request(&self, event_id: &str) -> Option<Value> {
        let event = self.chat.events.iter().find(|event| event.id == event_id)?;
        Some(json!({"notice_id": event_id, "subject": meta(event, "subject")?, "detail": event.hook_error.as_deref().unwrap_or_default(),
            "blocking": notice_key(event).is_none()}))
    }

    fn fix_preview(&mut self, event_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(name), Some(body)) = (self.session_api(), self.selected.as_ref().map(|s| s.name.clone()), self.fix_request(&event_id)) else { return };
        self.notice_acted.insert(event_id.clone(), Acted::FixStarting);
        self.redraw_notice_lines(cx);
        run(&self.runtime, async move { api.server_send(reqwest::Method::POST, &["sessions", &name, "notice", "fix", "preview"], Some(body), 30).await },
            window, cx, move |this, result, window, cx| {
                match result {
                    Ok(preview) => { this.notice_acted.remove(&event_id); this.open_fix_dialog(event_id, preview, window, cx); }
                    Err(error) => { this.notice_acted.insert(event_id, Acted::FixFailed(Hangar::fetch_failure(&error))); }
                }
                this.redraw_notice_lines(cx);
            });
    }

    fn open_fix_dialog(&mut self, event_id: String, preview: Value, window: &mut Window, cx: &mut Context<Self>) {
        let subject = preview["subject"].as_str().unwrap_or_default().to_owned();
        let detail = preview["detail"].as_str().unwrap_or_default().to_owned();
        let files: Vec<String> = preview["files"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect();
        let footer = tr_shared(if preview["no_hooks"].as_bool() == Some(true) { "notice_fix_footer_no_hooks" } else { "notice_fix_footer" }, &[]);
        let body = move |_: &mut Window, _: &mut App| fix_dialog_body(&detail, &files, &footer).into_any_element();
        let this = cx.entity().downgrade();
        chrome::confirm_dialog(window, cx, px(560.), px(420.), Some(tr_shared("notice_fix_title", &[("name", &subject)])), Rc::new(body),
            Rc::new(|| true), tr_shared("notice_fix_open", &[]), None, |d| d,
            move |window, cx| { let _ = this.update(cx, |this, cx| this.start_fix(event_id.clone(), window, cx)); true });
    }

    fn start_fix(&mut self, event_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(name), Some(body)) = (self.session_api(), self.selected.as_ref().map(|s| s.name.clone()), self.fix_request(&event_id)) else { return };
        self.notice_acted.insert(event_id.clone(), Acted::FixStarting);
        self.redraw_notice_lines(cx);
        run(&self.runtime, async move { api.server_send(reqwest::Method::POST, &["sessions", &name, "notice", "fix"], Some(body), 120).await },
            window, cx, move |this, result, _, cx| {
                // O andamento chega pela nota do servidor; aqui só o erro fica.
                match result {
                    Ok(_) => {
                        let before = fix_state(&this.chat.events, &event_id);
                        this.notice_acted.insert(event_id, Acted::FixOpened(before));
                    }
                    Err(error) => { this.notice_acted.insert(event_id, Acted::FixFailed(Hangar::fetch_failure(&error))); }
                }
                this.redraw_notice_lines(cx);
            });
    }

    /// Cancelar da nota que espera a rede: a nota trocada por "não continuei (cancelado)" chega pela conversa.
    fn cancel_auto_continue(&mut self, event_id: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(name)) = (self.session_api(), self.selected.as_ref().map(|s| s.name.clone())) else { return };
        self.notice_acted.insert(event_id.clone(), Acted::Cancelling);
        self.redraw_notice_lines(cx);
        run(&self.runtime, async move { api.server_send(reqwest::Method::DELETE, &["sessions", &name, "auto-continue"], None, 15).await },
            window, cx, move |this, result, _, cx| {
                match result {
                    // Cancelou: o giro fica até a nota trocada chegar, e o botão sai com ela.
                    Ok(body) if body["cancelled"].as_bool() == Some(true) => {}
                    // Nada esperando (o desfecho já saiu): a nota chega trocada sozinha.
                    Ok(_) => { this.notice_acted.remove(&event_id); }
                    Err(error) => { this.notice_acted.insert(event_id, Acted::CancelFailed(Hangar::fetch_failure(&error))); }
                }
                this.redraw_notice_lines(cx);
            });
    }

    /// A nota do continuar sozinho: linha neutra entre fios, com o Cancelar enquanto espera a rede.
    fn render_auto_continue(&self, event: &ChatEvent, cx: &mut Context<Self>) -> AnyElement {
        let id = event.id.clone();
        let cancel = auto_continue_cancellable(event).then(|| match self.notice_acted.get(&id) {
            Some(Acted::Cancelling) => notice_spin(&id, small(tr_shared("auto_continue_cancel", &[]), theme::muted())).into_any_element(),
            acted => div().flex().items_center().gap(px(7.))
                .when_some(match acted { Some(Acted::CancelFailed(why)) => Some(why.clone()), _ => None },
                    |el, why| el.child(small(tr_shared("auto_continue_cancel_failed", &[("why", &why)]), theme::warning())))
                .child(notice_action_button(&id, tr_shared("auto_continue_cancel", &[]))
                    .on_click(cx.listener({ let id = id.clone(); move |this, _, window, cx| this.cancel_auto_continue(id.clone(), window, cx) })))
                .into_any_element(),
        });
        notice_rule_line(auto_continue_row(auto_continue_text(event), cancel)).into_any_element()
    }

    fn open_fix_child(&mut self, child: String, window: &mut Window, cx: &mut Context<Self>) {
        let target = super::sidebar::Target::new(&self.open_server(), &child);
        let _ = self.select_target(&target, window, cx);
    }

    fn redraw_notice_lines(&mut self, cx: &mut Context<Self>) {
        self.follow_content_changed(cx);
        self.list_state.remeasure();
        self.redraw(panes::Area::Conversation, cx);
    }

    fn set_notice_ignored(&mut self, event_id: String, key: String, ignored: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.session_api() else { return };
        self.put_notice_ignore(api, self.open_server(), event_id, key, ignored, window, cx);
    }

    /// "Voltar a mostrar" das Configurações, na máquina ativa.
    fn show_notice_again(&mut self, key: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let server = self.active_key();
        // O erro fica com a máquina: em outra, a mesma chave não mostra a falha desta.
        self.put_notice_ignore(api, server.clone(), format!("settings:{server}:{key}"), key, false, window, cx);
    }

    /// `owner`: quem mostra o resultado, o id do evento na conversa ou `settings:{máquina}:{chave}` na lista das
    /// Configurações.
    fn put_notice_ignore(&mut self, api: Api, server: String, event_id: String, key: String, ignored: bool, window: &mut Window, cx: &mut Context<Self>) {
        let body = json!({"key": key, "ignored": ignored});
        run(&self.runtime, async move { api.server_send(reqwest::Method::PUT, &["notice-ignores"], Some(body), 15).await }, window, cx,
            move |this, result, _, cx| {
                // A lista em si chega pelo evento `notice_ignores`, em ordem: a resposta de um PUT antigo que chegasse
                // depois traria de volta uma lista velha.
                match result {
                    Ok(_) => {
                        this.notice_acted.remove(&event_id);
                        if ignored { this.notice_acted.insert(event_id, Acted::Ignored(key)); }
                        // Voltou a mostrar (aqui ou nas Configurações): as linhas "ignorado · Desfazer" dessa chave saem.
                        else { this.notice_acted.retain(|_, acted| *acted != Acted::Ignored(key.clone())); }
                    }
                    Err(error) => {
                        let why = format!("{} · {}", tr_shared("notice_ignore_failed", &[]), Hangar::fetch_failure(&error));
                        this.notice_acted.insert(event_id, Acted::Failed(why));
                    }
                }
                if server == this.open_server() { this.redraw_notice_lines(cx); }
            });
    }

    /// Avisos ignorados da máquina ativa, para as Configurações. `None` em servidor que não manda a lista.
    pub(super) fn render_ignored_notices(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let server = self.active_key();
        let keys = self.notice_ignores.get(&server)?;
        let rows: Vec<Div> = keys.iter().map(|key| {
            let failed = match self.notice_acted.get(&format!("settings:{server}:{key}")) { Some(Acted::Failed(why)) => Some(why.clone()), _ => None };
            let put = key.clone();
            div().flex().flex_col().gap(px(2.))
                .child(div().min_h(px(30.)).flex().items_center().gap(px(8.))
                    .child(div().flex_1().min_w_0().truncate().text_sm().child(notice_key_label(key)))
                    .child(Button::new(SharedString::from(format!("notice-show-{key}"))).ghost().xsmall().label(tr_shared("notice_show_again", &[]))
                        .on_click(cx.listener(move |this, _, window, cx| this.show_notice_again(put.clone(), window, cx)))))
                .when_some(failed, |el, why| el.child(div().text_xs().text_color(theme::warning()).child(why)))
        }).collect();
        Some(div().pt(px(10.)).flex().flex_col().gap(px(4.))
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr_shared("notice_ignored_title", &[])))
            .map(|el| if rows.is_empty() { el.child(div().text_sm().text_color(theme::faint()).child(tr_shared("notice_ignored_empty", &[]))) }
                else { el.children(rows) })
            .into_any_element())
    }

    /// O resultado do conserto que voltou da sessão filha: uma linha com o link para ela.
    fn render_fix_result(&self, event: &ChatEvent, cx: &mut Context<Self>) -> AnyElement {
        let done = meta(event, "state") == Some("done");
        let child = meta(event, "child").unwrap_or_default().to_owned();
        let open = fix_open_button(&event.id)
            .on_click(cx.listener(move |this, _, window, cx| this.open_fix_child(child.clone(), window, cx)));
        notice_rule_line(fix_result_row(done, plain(event).trim().to_owned(), open)).into_any_element()
    }

    /// O que vai entre o título e a seta: o andamento (reinstalando, consertando…) ou o botão da ação.
    /// `fix`: o estado do conserto deste aviso, lido uma vez por linha.
    fn notice_action_part(&self, event: &ChatEvent, fix: Option<FixState>, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = event.id.clone();
        let spin = |label: String| notice_spin(&id, small(label, theme::muted())).into_any_element();
        let button = |label: String| notice_action_button(&id, label);
        match (notice_action(event)?, self.notice_acted.get(&id)) {
            (NoticeAction::Reinstall(plugin), acted) => Some(match acted {
                Some(Acted::Reinstalling) => spin(tr_shared("notice_reinstalling", &[("name", &plugin)])),
                Some(Acted::Reloading) => spin(tr_shared("notice_reloading", &[])),
                Some(Acted::Reinstalled) => notice_reinstalled(&plugin, true)
                    .child(button(tr_shared("notice_reload", &[])).on_click(cx.listener({ let id = id.clone(); move |this, _, _, cx| this.reload_after_reinstall(id.clone(), cx) })))
                    .into_any_element(),
                failed => div().flex().items_center().gap(px(7.))
                    .when_some(match failed { Some(Acted::ReinstallFailed(why)) => Some(why.clone()), _ => None },
                        |el, why| el.child(small(tr_shared("notice_reinstall_failed", &[("why", &why)]), theme::warning())))
                    .child(button(tr_shared(if matches!(failed, Some(Acted::ReinstallFailed(_))) { "notice_retry" } else { "notice_reinstall" }, &[]))
                        .on_click(cx.listener({ let id = id.clone(); move |this, _, window, cx| this.reinstall_plugin(id.clone(), plugin.clone(), window, cx) })))
                    .into_any_element(),
            }),
            (NoticeAction::Fix, acted) => {
                // A nota "consertando" pode chegar antes da resposta do pedido: andamento e resultado vencem o giro.
                let opened = matches!(acted, Some(Acted::FixOpened(before)) if *before == fix)
                    && !matches!(fix, Some(FixState::Running { .. } | FixState::Done));
                if opened || matches!(acted, Some(Acted::FixStarting)) { return Some(spin(tr_shared("notice_fix", &[]))); }
                match fix {
                    Some(FixState::Running { child }) => Some(notice_spin(&id, notice_fixing(&id, &child).cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| this.open_fix_child(child.clone(), window, cx))))
                        .into_any_element()),
                    Some(FixState::Done) => Some(notice_fixed().into_any_element()),
                    state => Some(div().flex().items_center().gap(px(7.))
                        .when(state == Some(FixState::Failed), |el| el.child(small(format!("· {}", tr_shared("notice_fix_failed", &[])), theme::warning())))
                        .when_some(match acted { Some(Acted::FixFailed(why)) => Some(why.clone()), _ => None },
                            |el, why| el.child(small(tr_shared("notice_fix_start_failed", &[("why", &why)]), theme::warning())))
                        .child(button(tr_shared("notice_fix", &[]))
                            .on_click(cx.listener(move |this, _, window, cx| this.fix_preview(id.clone(), window, cx))))
                        .into_any_element()),
                }
            }
        }
    }

    pub(super) fn render_notice_line(&self, event_index: usize, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let Some(event) = self.chat.events.get(event_index) else { return div().into_any_element(); };
        if is_auto_continue(event) { return self.render_auto_continue(event, cx); }
        if is_api_error(event) { return api_error_box(plain(event).trim().to_owned()).into_any_element(); }
        if fix_of(event).is_some() { return self.render_fix_result(event, cx); }
        let (id, key) = (event.id.clone(), notice_key(event).map(str::to_owned));
        // Acabou de ignorar: a linha fica, com o caminho de volta, até a conversa ser recarregada.
        if matches!(self.notice_acted.get(&id), Some(Acted::Ignored(_))) {
            let undo = key.map(|key| {
                let event_id = id.clone();
                undo_button(SharedString::from(format!("notice-undo-{id}")))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_notice_ignored(event_id.clone(), key.clone(), false, window, cx)))
            });
            return notice_rule_line(ignored_text().children(undo)).into_any_element();
        }
        let (title, count, detail) = (notice_title(event), notice_count(event), notice_detail(event));
        let failed = match self.notice_acted.get(&id) { Some(Acted::Failed(why)) => Some(why.clone()), _ => None };
        let expandable = detail.is_some() || key.is_some();
        let open = expandable && self.expanded.contains(row);
        let toggle_key = row.to_owned();
        let fix = (notice_action(event) == Some(NoticeAction::Fix)).then(|| fix_state(&self.chat.events, &id)).flatten();
        let fixed = fix == Some(FixState::Done);
        let action = self.notice_action_part(event, fix, cx);
        // Com ação, a seta sai do título: o botão fica entre os dois e o clique nele não abre o detalhe.
        let head = notice_head_marked(SharedString::from(format!("notice-{id}")), title, count, expandable.then_some(open).filter(|_| action.is_none()), fixed)
            .when(expandable, |el| el.cursor_pointer().on_click(cx.listener({ let key = toggle_key.clone(); move |this, _, _, cx| this.toggle(key.clone(), cx) })));
        let head = match action {
            None => head.into_any_element(),
            Some(action) => div().flex().items_center().gap(px(7.)).min_w_0().child(head).child(action)
                .when(expandable, |el| el.child(notice_chevron(&id, open).cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)))))
                .into_any_element(),
        };
        let body = open.then(|| {
            let ignore = key.map(|key| {
                let event_id = id.clone();
                ignore_button(SharedString::from(format!("notice-ignore-{id}")))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_notice_ignored(event_id.clone(), key.clone(), true, window, cx)))
            });
            notice_body(detail).when_some(failed, |el, why| el.child(small(why, theme::warning()))).children(ignore)
        });
        div().w_full().flex().flex_col().child(notice_rule_line(head)).children(body).into_any_element()
    }
}

fn small(text: String, color: Hsla) -> Div { div().flex_shrink_0().text_size(px(12.5)).text_color(color).child(text) }

/// O conteúdo centrado entre dois fios: a linha de aviso fechada.
pub(crate) fn notice_rule_line(content: impl IntoElement) -> Div {
    let rule = || div().flex_1().h(px(1.)).bg(theme::border());
    div().w_full().min_h(px(27.)).flex().items_center().gap(px(8.)).child(rule()).child(content).child(rule())
}

/// Ícone, título e total; `open` é `None` quando não há o que abrir.
pub(crate) fn notice_head(id: SharedString, title: String, count: Option<String>, open: Option<bool>) -> Stateful<Div> {
    notice_head_marked(id, title, count, open, false)
}

/// Peças da ação na linha, sem estado: a conversa e a animação da novidade usam as mesmas.
pub(crate) fn notice_action_button(id: &str, label: String) -> Button {
    Button::new(SharedString::from(format!("notice-act-{id}"))).outline().xsmall().label(label)
}

/// A linha da nota do continuar sozinho: o texto neutro e, enquanto espera a rede, o Cancelar.
pub(crate) fn auto_continue_row(text: String, cancel: Option<AnyElement>) -> Div {
    div().flex().items_center().gap(px(7.)).min_w_0()
        .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(theme::muted()).child(text))
        .children(cancel)
}

/// Andamento: o indicador girando e o texto.
pub(crate) fn notice_spin(id: &str, label: impl IntoElement) -> Div {
    div().flex().items_center().gap(px(6.))
        .child(chrome::Spinner::new(SharedString::from(format!("notice-spin-{id}")), IconName::LoaderCircle, px(13.), theme::accent()))
        .child(label)
}

/// "consertando em <sessão>", no tom de link.
pub(crate) fn notice_fixing(id: &str, child: &str) -> Stateful<Div> {
    div().id(SharedString::from(format!("notice-child-{id}"))).text_size(px(12.5)).text_color(theme::accent())
        .child(tr_shared("notice_fixing", &[("name", child)]))
}

pub(crate) fn notice_fixed() -> Div { small(format!("· {}", tr_shared("notice_fixed", &[])), theme::faint()) }

/// "Plugin x reinstalado"; `hint` acrescenta que a conversa só usa depois de recarregar. O Recarregar entra depois.
pub(crate) fn notice_reinstalled(plugin: &str, hint: bool) -> Div {
    let text = tr_shared("notice_reinstalled", &[("name", plugin)]);
    let text = if hint { format!("{text} · {}", tr_shared("notice_reinstalled_hint", &[])) } else { text };
    div().flex().items_center().gap(px(7.))
        .child(chrome::small_icon(IconName::Check, 14., theme::success()))
        .child(small(text, theme::muted()))
}

pub(crate) fn notice_chevron(id: &str, open: bool) -> Stateful<Div> {
    div().id(SharedString::from(format!("notice-chevron-{id}")))
        .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 14., theme::faint()))
}

pub(crate) fn fix_open_button(id: &str) -> Button {
    Button::new(SharedString::from(format!("notice-fix-open-{id}"))).ghost().xsmall().label(tr_shared("notice_fix_open", &[]))
}

/// ✓ do resolvido ou o triângulo âmbar do aviso.
fn mark_icon(done: bool) -> impl IntoElement {
    if done { chrome::small_icon(IconName::Check, 14., theme::success()) } else { chrome::small_icon(IconName::TriangleAlert, 14., theme::warning()) }
}

/// A linha do resultado do conserto: feito (✓) ou falha (âmbar), o texto e o Abrir.
pub(crate) fn fix_result_row(done: bool, text: String, open: Button) -> Div {
    div().flex().items_center().gap(px(7.)).min_w_0().child(mark_icon(done))
        .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(theme::muted()).child(text))
        .child(open)
}

/// `done`: o problema foi consertado, o ícone vira o de feito.
pub(crate) fn notice_head_marked(id: SharedString, title: String, count: Option<String>, open: Option<bool>, done: bool) -> Stateful<Div> {
    div().id(id).flex().items_center().gap(px(7.)).min_w_0()
        .child(mark_icon(done))
        .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(theme::muted()).child(title))
        .when_some(count, |el, count| el.child(small(format!("· {count}"), theme::faint())))
        .when_some(open, |el, open| el.child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 14., theme::faint())))
}

/// O que abre embaixo da linha: o detalhe em fonte fixa; o Ignorar entra depois.
pub(crate) fn notice_body(detail: Option<String>) -> Div {
    div().w_full().flex().flex_col().items_center().gap(px(7.)).pb(px(6.))
        .when_some(detail, |el, detail| el.child(div().max_w(px(900.)).px(px(12.)).py(px(9.)).rounded(px(8.)).bg(theme::inset())
            .font_family(theme::MONO).text_size(px(12.5)).text_color(theme::muted()).child(detail)))
}

pub(crate) fn ignore_button(id: SharedString) -> Button { Button::new(id).ghost().xsmall().label(tr_shared("notice_ignore", &[])) }

pub(crate) fn undo_button(id: SharedString) -> Button { Button::new(id).ghost().xsmall().label(tr_shared("comum_desfazer", &[])) }

/// "Aviso ignorado nesta máquina"; o Desfazer entra depois.
pub(crate) fn ignored_text() -> Div {
    div().flex().items_center().gap(px(6.)).child(small(tr_shared("notice_ignored_line", &[]), theme::faint()))
}

/// O corpo da janela do "Consertar…": o que a sessão filha vai receber.
pub(crate) fn fix_dialog_body(detail: &str, files: &[String], footer: &str) -> Div {
    let label = |key: &str| div().pt(px(10.)).pb(px(4.)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::faint()).child(tr_shared(key, &[]));
    let mono = |text: String| div().px(px(9.)).py(px(7.)).rounded(px(6.)).bg(theme::inset()).font_family(theme::MONO).text_size(px(12.))
        .text_color(theme::muted()).whitespace_normal().child(text);
    let files = if files.is_empty() { tr_shared("notice_fix_no_files", &[]) } else { files.join("\n") };
    div().flex().flex_col()
        .child(div().text_size(px(13.)).line_height(px(19.)).text_color(theme::muted()).whitespace_normal().child(tr_shared("notice_fix_intro", &[])))
        .child(label("notice_fix_error_label")).child(mono(detail.to_owned()))
        .child(label("notice_fix_files_label")).child(mono(files))
        .child(div().pt(px(12.)).text_xs().text_color(theme::faint()).child(footer.to_owned()))
}

/// Erro da API que parou o turno: caixa âmbar, sem Ignorar.
pub(crate) fn api_error_box(text: String) -> Div {
    div().w_full().px(px(12.)).py(px(9.)).rounded(px(8.)).bg(theme::warning().opacity(0.08))
        .border_1().border_color(theme::warning().opacity(0.4)).flex().flex_col().gap(px(4.))
        .child(div().flex().items_center().gap(px(7.))
            .child(chrome::small_icon(IconName::TriangleAlert, 14., theme::warning()))
            .child(div().text_size(px(13.)).text_color(theme::warning_text()).child(tr_shared("notice_api_error_title", &[]))))
        .child(div().text_size(px(12.5)).text_color(theme::muted()).child(text))
}

#[cfg(test)]
mod tests {
    use super::*;
    // O `super::*` traz o `test` do gpui, que não é o dos testes comuns.
    use core::prelude::v1::test;

    fn notice(text: &str, meta: Value) -> ChatEvent {
        ChatEvent { kind: "assistant_msg".into(), id: "local-1".into(), text: Some(text.into()), notice_meta: meta.as_object().cloned(), ..Default::default() }
    }

    fn hook(detail: &str, meta: Value) -> ChatEvent {
        ChatEvent { kind: "notice".into(), text: Some("hook_failed".into()), hook_error: Some(detail.into()), ..notice("", meta) }
    }

    #[test]
    fn notice_title_for_hook_uses_subject() {
        let event = hook("SessionStart:compact: sem pasta (falhou 12 vezes)", json!({"key":"hook:SessionStart:compact","subject":"SessionStart:compact","count":12}));
        assert_eq!(notice_title(&event), tr_shared("notice_hook_failed_line", &[("name", "SessionStart:compact")]));
        assert_eq!(notice_count(&event), Some(tr_shared("notice_count", &[("count", "12")])));
        assert_eq!(notice_detail(&event).as_deref(), Some("SessionStart:compact: sem pasta (falhou 12 vezes)"));
        assert_eq!(notice_count(&hook("Stop: x", json!({"key":"hook:Stop","subject":"Stop"}))), None, "uma falha só não tem total");
    }

    #[test]
    fn notice_title_for_cli_note_strips_the_gear_and_keeps_the_first_line() {
        let event = notice("⚙️ Servidor MCP que precisa de login: `slack`\nO plugin `x` não carregou: sem pasta", json!({"key":"note:startup:ab12cd34"}));
        assert_eq!(notice_title(&event), "Servidor MCP que precisa de login: `slack`");
        assert_eq!(notice_detail(&event).as_deref(), Some("Servidor MCP que precisa de login: `slack`\nO plugin `x` não carregou: sem pasta"));
        let short = notice("⚙️ O modelo recusou responder", json!({}));
        assert_eq!(notice_title(&short), "O modelo recusou responder");
        assert_eq!(notice_detail(&short), None, "aviso de uma linha não tem o que abrir");
    }

    #[test]
    fn notice_hidden_only_for_ignored_key() {
        let ignored: BTreeSet<String> = ["hook:Stop".to_owned()].into();
        assert!(notice_hidden(&hook("Stop: x", json!({"key":"hook:Stop","subject":"Stop"})), &ignored));
        assert!(!notice_hidden(&hook("Start: x", json!({"key":"hook:Start","subject":"Start"})), &ignored));
    }

    #[test]
    fn api_error_reply_is_a_notice_without_key_and_other_notices_are_not_api_errors() {
        let error = notice("Failed to refresh OAuth token", json!({"level":"error"}));
        assert!(is_notice_line(&error) && is_api_error(&error) && notice_key(&error).is_none());
        assert!(!is_api_error(&notice("⚙️ Servidor MCP que precisa de login: `x`", json!({"key":"note:startup:ab12cd34"}))));
    }

    #[test]
    fn notice_without_key_is_never_hidden_and_has_no_ignore() {
        let event = notice("⚙️ Turno parado: a conversa chegou ao limite do contexto", json!({}));
        assert!(is_notice_line(&event) && notice_key(&event).is_none());
        assert!(!notice_hidden(&event, &["hook:Stop".to_owned(), String::new()].into()));
    }

    #[test]
    fn event_without_notice_meta_is_not_a_notice_line() {
        let answer = ChatEvent { kind: "assistant_msg".into(), id: "a1".into(), text: Some("⚙️ resposta comum".into()), ..Default::default() };
        assert!(!is_notice_line(&answer));
        // Aviso de hook do Codex, sem o mapa: segue no desenho antigo.
        let codex = ChatEvent { kind: "notice".into(), text: Some("hook_failed".into()), hook_error: Some("x".into()), ..Default::default() };
        assert!(!is_notice_line(&codex));
    }

    #[test]
    fn notice_key_label_names_each_kind_and_keeps_an_unknown_key() {
        assert_eq!(notice_key_label("hook:Stop"), tr_shared("notice_key_hook", &[("name", "Stop")]));
        assert_eq!(notice_key_label("hook:PostToolUse:Bash"), tr_shared("notice_key_hook", &[("name", "PostToolUse:Bash")]));
        assert_eq!(notice_key_label("note:startup:ab12cd34"), tr_shared("notice_key_startup", &[]));
        assert_eq!(notice_key_label("note:retry"), tr_shared("notice_key_retry", &[]));
        assert_eq!(notice_key_label("note:notification:update"), tr_shared("notice_key_notification", &[("name", "update")]));
        assert_eq!(notice_key_label("note:de-uma-versao-futura"), "note:de-uma-versao-futura");
        assert_ne!(notice_key_label("hook:Stop"), "notice_key_hook", "a chave de texto existe nas mensagens");
    }

    #[test]
    fn notice_action_comes_from_the_meta() {
        let reinstall = hook("x", json!({"key":"hook:S","subject":"S","action":"reinstall_plugin","plugin":"pl@mk"}));
        assert_eq!(notice_action(&reinstall), Some(NoticeAction::Reinstall("pl@mk".into())));
        assert_eq!(notice_action(&hook("x", json!({"subject":"S","action":"fix_hook"}))), Some(NoticeAction::Fix));
        assert_eq!(notice_action(&hook("x", json!({"subject":"S","action":"reinstall_plugin"}))), None, "sem o plugin não há o que reinstalar");
        assert_eq!(notice_action(&notice("⚙️ x", json!({}))), None);
        assert_eq!(notice_key_label("note:plugin:pl@mk"), tr_shared("notice_key_plugin", &[("name", "pl@mk")]));
    }

    fn fix_note(id: &str, attempt: u64, state: &str) -> ChatEvent {
        ChatEvent { id: id.into(), ..notice("x", json!({"fix_of":"hookfail:a","attempt":attempt,"state":state,"child":"conserto-s"})) }
    }

    #[test]
    fn fix_state_follows_the_latest_attempt() {
        assert_eq!(fix_state(&[], "hookfail:a"), None);
        let running = fix_note("local-fix:hookfail:a:1", 1, "running");
        assert_eq!(fix_state(std::slice::from_ref(&running), "hookfail:a"), Some(FixState::Running { child: "conserto-s".into() }));
        let done = fix_note("local-fixdone:hookfail:a:1", 1, "done");
        assert_eq!(fix_state(&[running.clone(), done.clone()], "hookfail:a"), Some(FixState::Done));
        let again = fix_note("local-fix:hookfail:a:2", 2, "running");
        assert_eq!(fix_state(&[running, done, again], "hookfail:a"), Some(FixState::Running { child: "conserto-s".into() }), "nova tentativa depois do resultado");
        assert_eq!(fix_state(&[fix_note("f", 1, "failed")], "hookfail:a"), Some(FixState::Failed));
        assert_eq!(fix_state(&[fix_note("f", 1, "done")], "hookfail:b"), None, "nota de outro aviso");
    }

    #[test]
    fn running_fix_note_has_no_row_and_the_result_has() {
        assert!(is_fix_running_note(&fix_note("f", 1, "running")));
        assert!(!is_fix_running_note(&fix_note("f", 1, "done")));
        assert!(fix_of(&fix_note("f", 1, "done")).is_some());
    }

    fn auto_note(state: &str, extra: Value) -> ChatEvent {
        let mut meta = json!({"kind":"auto_continue","state":state,"stopped_at":at(10, 5),"count":1});
        meta.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        notice("texto do servidor", meta)
    }

    fn at(hour: u32, minute: u32) -> f64 {
        use chrono::TimeZone;
        chrono::Local.with_ymd_and_hms(2026, 11, 27, hour, minute, 0).single().unwrap().timestamp() as f64
    }

    #[test]
    fn auto_continue_note_reads_its_times_from_the_meta() {
        crate::i18n::english_in_this_thread(false);
        assert!(is_auto_continue(&auto_note("waiting", json!({}))));
        assert_eq!(auto_continue_text(&auto_note("waiting", json!({}))), "A conexão caiu às 10:05; continuo quando a rede voltar.");
        assert_eq!(auto_continue_text(&auto_note("continued", json!({"continued_at": at(10, 20), "count": 2}))),
            "A conexão caiu às 10:05; continuei às 10:20.");
        assert_eq!(auto_continue_text(&auto_note("failed", json!({"code": "erro_envio_falhou"}))),
            "A conexão caiu às 10:05; não consegui continuar (erro_envio_falhou).");
        assert_eq!(auto_continue_text(&auto_note("cancelled", json!({}))), "A conexão caiu às 10:05; não continuei (cancelado).");
        assert_eq!(auto_continue_text(&auto_note("skipped", json!({"code": "user_after_stop"}))), "A conexão caiu às 10:05; não continuei.");
        // Reboot: `stopped_at` é a hora do boot, o texto diz só quando continuou.
        assert_eq!(auto_continue_text(&auto_note("continued", json!({"cause": "reboot", "continued_at": at(10, 20)}))),
            "O computador reiniciou no meio do turno; continuei às 10:20.");
        assert_eq!(auto_continue_text(&auto_note("de-uma-versao-futura", json!({}))), "texto do servidor", "estado desconhecido fica com o texto do servidor");
        assert!(!is_auto_continue(&notice("⚙️ x", json!({"state":"waiting"}))));
    }

    #[test]
    fn auto_continue_only_the_waiting_note_offers_cancel() {
        assert!(auto_continue_cancellable(&auto_note("waiting", json!({}))));
        for state in ["continued", "failed", "cancelled", "skipped"] {
            assert!(!auto_continue_cancellable(&auto_note(state, json!({}))), "{state}: o desfecho já saiu");
        }
        assert!(!auto_continue_cancellable(&auto_note("waiting", json!({"cause": "reboot"}))), "a nota do reboot nasce com o desfecho");
        assert!(!auto_continue_cancellable(&fix_note("f", 1, "waiting")), "nota de outro tipo");
    }

    #[test]
    fn notice_ignores_store_reports_change_per_server() {
        let mut map = HashMap::new();
        assert_eq!(store(&mut map, "casa", &json!({"keys":["hook:Stop"]})), Some(true));
        assert_eq!(store(&mut map, "casa", &json!({"keys":["hook:Stop"]})), Some(false));
        assert_eq!(store(&mut map, "vps", &json!({"keys":[]})), Some(true));
        assert_eq!(store(&mut map, "casa", &json!({"machine":null})), None, "evento de outro formato não apaga a lista");
        assert_eq!(map["casa"], ["hook:Stop".to_owned()].into());
    }
}
