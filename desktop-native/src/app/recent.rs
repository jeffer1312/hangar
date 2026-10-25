//! Conversas fechadas no modo Conversas da barra (o "Recentes" do app do Claude): a lista vem de `archive/recent` da máquina
//! ativa, e abrir uma mostra o histórico com o compositor. A sessão só é retomada no Enviar, e a mensagem vai para ela.
use super::*;
use super::costs::{web, web_with};
use crate::api::Resumed;
use super::device::Remote;
use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub(super) struct ArchiveEntry {
    pub(super) project: String,
    pub(super) session_id: String,
    #[serde(default)] pub(super) cwd: String,
    #[serde(default)] pub(super) mtime: f64,
    #[serde(default)] pub(super) preview: String,
    #[serde(default)] pub(super) ultima: String,
    #[serde(default)] pub(super) live: bool,
    pub(super) config_dir: Option<String>,
    #[serde(default)] pub(super) conta: String,
    #[serde(default)] pub(super) provider: String,
    pub(super) codex_account: Option<String>,
}

impl ArchiveEntry {
    /// A última mensagem é o que identifica a conversa meses depois.
    pub(super) fn title(&self) -> String { Self::first_text([&self.ultima, &self.preview]) }
    /// Na barra, o assunto: a primeira mensagem, como a lista de conversas do app do Claude.
    fn heading(&self) -> String { Self::first_text([&self.preview, &self.ultima]) }
    /// Com citação ou elemento, cada bloco vira o rótulo curto dele, como na fila e no trilho.
    fn first_text(texts: [&String; 2]) -> String {
        texts.into_iter().find(|t| !t.trim().is_empty()).map(|t| super::quotes::summary(t)).unwrap_or_else(|| tr("create_no_messages"))
    }
    fn query(&self) -> Vec<(String, String)> {
        let mut q = Vec::new();
        if self.provider != "claude" && !self.provider.is_empty() { q.push(("provider".into(), self.provider.clone())); }
        if self.provider == "codex" && let Some(a) = &self.codex_account { q.push(("codex_account".into(), a.clone())); }
        q
    }
    /// O pedido de histórico da conversa, na conta dona dela.
    pub(super) fn history_query(&self, tail: usize) -> Vec<(String, String)> {
        let mut query = self.query();
        query.push(("tail".into(), tail.to_string()));
        if let Some(dir) = self.config_dir.clone() { query.push(("config_dir".into(), dir)); }
        query
    }
    fn meta(&self) -> String {
        let folder = self.cwd.trim_end_matches(['/', '\\']).rsplit(['/', '\\']).next().filter(|s| !s.is_empty()).map(str::to_owned);
        [folder, Some(self.conta.clone()).filter(|s| !s.is_empty()), Some(super::side::ago(chrono::Local::now().timestamp() as f64 - self.mtime))]
            .into_iter().flatten().collect::<Vec<_>>().join(" · ")
    }
}

/// Uma mensagem da prévia: a do usuário vem realçada.
pub(super) struct PreviewLine { mine: bool, view: Entity<TextViewState> }

/// Só as falas de usuário e assistente, com texto, de uma resposta de `archive/<p>/<id>/history`.
pub(super) fn preview_lines(result: Result<Value, Failure>, cx: &mut App) -> Result<Vec<PreviewLine>, String> {
    let events = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| serde_json::from_value::<Vec<ChatEvent>>(v).map_err(|_| tr("invalid_response")))?;
    Ok(events.into_iter().filter(|e| matches!(e.kind.as_str(), "user_msg" | "assistant_msg"))
        .filter_map(|e| e.text.filter(|t| !t.trim().is_empty()).map(|t| (e.kind == "user_msg", t)))
        .map(|(mine, text)| PreviewLine { mine, view: cx.new(|cx| TextViewState::markdown(&safe_markdown(&super::quotes::summary(&text)), cx)) }).collect())
}

/// Os balões somente leitura da prévia.
pub(super) fn render_preview_lines(lines: &[PreviewLine], cx: &App) -> Div {
    div().flex().flex_col().gap(px(8.)).children(lines.iter().map(|line| div().p(px(10.)).rounded(px(8.)).text_sm()
        .when(line.mine, |el| el.bg(theme::accent_dim()).ml(px(32.))).when(!line.mine, |el| el.bg(theme::inset()).mr(px(32.)))
        .child(TextView::new(&line.view).selectable(true).scrollable(false).style({
            let (font, size) = theme::original_code_typography(cx);
            gpui_kit::component::text::TextViewStyle::default()
                .code_block(StyleRefinement::default().font_family(font.clone()).text_size(size))
                .inline_code_font_family(font)
        }))))
}

#[derive(Default)]
pub(super) struct Recents {
    connection: Option<u64>,
    list: Remote<Vec<ArchiveEntry>>,
    /// Cada linha entra no Tab pela vida da entrada.
    focus: HashMap<String, FocusHandle>,
    history: Remote<Vec<PreviewLine>>,
    scroll: ScrollHandle,
    sending: bool,
    note: Option<String>,
}

const HISTORY_TAIL: usize = 100;

/// O rascunho da conversa fechada `session_id` na máquina `server`, à parte do da tela sem sessão. O nome com `\0` não
/// casa com nenhuma sessão de verdade.
fn reopen_key(server: &str, session_id: &str) -> SessionKey {
    SessionKey { server: server.to_owned(), name: "\0reopen".into(), jsonl: session_id.to_owned() }
}

impl Hangar {
    fn recents_shown(&self) -> bool {
        appearance::get().navigation == appearance::Navigation::Conversations && self.api.is_some() && !self.active_invite()
    }

    pub(super) fn reopen_sending(&self) -> bool { self.recents.sending }

    /// A lista de sessões da máquina ativa chegou: conexão nova ou sessão que fechou relê a lista; troca de estado, não.
    pub(super) fn recents_sessions_changed(&mut self, closed: bool, cx: &mut Context<Self>) {
        if !self.recents_shown() { return; }
        let fresh = self.recents.connection != Some(self.connection);
        if fresh || closed || (self.recents.list.value.is_none() && !self.recents.list.loading) { self.load_recents(cx); }
    }

    fn load_recents(&mut self, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        if self.recents.connection != Some(self.connection) {
            self.recents.connection = Some(self.connection);
            self.recents.list.reset();
            self.recents.focus.clear();
        }
        let (seq, connection) = (self.recents.list.start(), self.connection);
        let task = self.runtime.spawn(async move { api.server_read(&["archive", "recent"], &[("cap", "40")], 15).await });
        cx.spawn(async move |this, cx| {
            let joined = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.connection != connection || this.recents.list.seq != seq { return; }
                let list = match joined {
                    Ok(result) => result.map_err(|e| Self::fetch_failure(&e))
                        .and_then(|v| serde_json::from_value::<Vec<ArchiveEntry>>(v).map_err(|_| tr("invalid_response"))),
                    Err(_) => Err(web("conversas_recentes_falhou")),
                }.map(|list| list.into_iter().filter(|e| !e.live).collect::<Vec<_>>());
                if let Ok(list) = &list {
                    this.recents.focus.retain(|id, _| list.iter().any(|e| &e.session_id == id));
                    for entry in list {
                        this.recents.focus.entry(entry.session_id.clone()).or_insert_with(|| cx.focus_handle().tab_stop(true));
                    }
                }
                this.recents.list.finish(seq, list);
                cx.notify();
            });
        }).detach();
    }

    /// Clicar numa fechada: a sessão aberta sai, a área principal mostra o histórico dela com o compositor.
    fn open_closed(&mut self, entry: ArchiveEntry, window: &mut Window, cx: &mut Context<Self>) {
        if self.recents.sending { return; }
        let Some(api) = self.api.clone() else { return };
        self.stash_view_draft(cx);
        self.close_open_session(window, cx);
        self.recents.note = None;
        // A conta vem pré-escolhida na dona da conversa; a pílula usa as contas e cotas que a tela sem sessão já lê, e a
        // cota é relida: a leitura da conexão pode ser de antes da renovação.
        let claude = entry.provider == "claude" || entry.provider.is_empty();
        let owner = claude.then(|| entry.config_dir.clone());
        let unlisted = claude && entry.config_dir.is_none();
        self.ensure_new_chat(api, window, cx).update(cx, |view, cx| {
            (view.reopen_config, view.reopen_default) = (owner, unlisted);
            if claude { view.load_quotas(cx); }
        });
        self.reopen = Some(entry);
        self.load_view_draft(window, cx);
        self.load_reopen_history(cx);
        self.composer.update(cx, |input, cx| input.focus(window, cx));
        cx.notify();
    }

    fn load_reopen_history(&mut self, cx: &mut Context<Self>) {
        let (Some(api), Some(entry)) = (self.api.clone(), self.reopen.clone()) else { return };
        let (seq, connection) = (self.recents.history.start(), self.connection);
        let task = self.runtime.spawn(async move {
            let query = entry.history_query(HISTORY_TAIL);
            let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            api.server_read(&["archive", &entry.project, &entry.session_id, "history"], &query, 15).await
        });
        cx.spawn(async move |this, cx| {
            let joined = task.await;
            let _ = this.update(cx, |this, cx| {
                if this.connection != connection || this.recents.history.seq != seq { return; }
                let lines = match joined { Ok(result) => preview_lines(result, cx), Err(_) => Err(tr("create_preview_failed")) };
                // Abre no fim: é onde a conversa parou e onde ela continua.
                if this.recents.history.finish(seq, lines) { this.recents.scroll.scroll_to_bottom(); }
                cx.notify();
            });
        }).detach();
    }

    /// Enviar na conversa fechada: retoma na conta dela e manda o texto à sessão nova. Falhar no retomar deixa o texto no
    /// campo com o aviso; falhar só no envio abre a sessão nova com o aviso da entrega.
    pub(super) fn send_reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.reopen.clone() else { return };
        if self.recents.sending { return; }
        let text = self.composer.read(cx).value().to_string();
        if text.trim().is_empty() { return; }
        let Some(api) = self.api.clone() else { return };
        if let Some(block) = self.reopen_quota_block(cx) { self.recents.note = Some(block); cx.notify(); return; }
        let claude = entry.provider == "claude" || entry.provider.is_empty();
        // Claude pode retomar em outra conta: o servidor move a conversa para a escolhida, como no "Continuar" do diálogo.
        let config = self.new_chat.as_ref().and_then(|view| view.read(cx).reopen_config.clone()).unwrap_or_else(|| entry.config_dir.clone());
        let mut body = json!({"config_dir": if claude { json!(config) } else { Value::Null }, "provider": entry.provider});
        if entry.provider == "codex" && let Some(account) = &entry.codex_account { body["codex_account"] = json!(account); }
        (self.recents.sending, self.recents.note) = (true, None);
        let identity = api.identity();
        let message = text.clone();
        let (project, id) = (entry.project.clone(), entry.session_id.clone());
        let task = self.runtime.spawn(async move {
            match api.resume_archive(&project, &id, body).await.map_err(|e| Self::fetch_failure(&e))? {
                Resumed::Live(name) => Ok::<_, String>(Err(name)),
                Resumed::New(session) => {
                    let sent = api.send(&session.name, &message).await;
                    Ok(Ok((session, sent)))
                }
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let joined = task.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let current = this.reopen.as_ref().is_some_and(|r| r.session_id == entry.session_id);
                let back = reopen_key(&identity, &entry.session_id);
                // `sending` só cai depois: a seleção abaixo não pode guardar o texto em voo como rascunho desta tela.
                match joined.unwrap_or_else(|_| Err(tr("search_interrupted"))) {
                    Err(error) if current => this.recents.note = Some(error),
                    Err(error) => {
                        // Fora da tela, o texto volta ao rascunho da conversa fechada: reabri-la o devolve ao campo.
                        this.append_draft(back, &text);
                        window.push_notification(Notification::warning(error), cx);
                    }
                    Ok(Ok((session, sent))) => {
                        this.drafts.remove(&back);
                        this.forget_recent(&entry.session_id);
                        this.reopened(identity, session, text, sent, current, window, cx);
                    }
                    Ok(Err(live)) => this.reopen_live(identity, back, live, text, current, window, cx),
                }
                this.recents.sending = false;
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    /// A conversa já estava aberta numa sessão viva: abre essa, sem enviar, com o texto digitado somado ao rascunho dela.
    /// `identity` é a máquina do envio, não a ativa de agora; `back` é o rascunho da conversa fechada.
    fn reopen_live(&mut self, identity: String, back: SessionKey, name: String, text: String, current: bool, window: &mut Window, cx: &mut Context<Self>) {
        let target = super::servers::norm(&identity);
        let list = if self.is_active_key(&target) { Some(&self.sessions) } else { self.remote.get(&target).map(|l| &l.sessions) };
        let found = list.and_then(|l| l.iter().find(|s| s.name == name)).cloned();
        let Some((session, key)) = found.and_then(|s| SessionKey::new(&identity, &s).map(|key| (s, key))) else {
            // Sem a sessão na lista (ou sem conversa legível), o texto não tem onde esperar nela: fica na conversa fechada.
            let note = web_with("conversa_ja_aberta", &[("sessao", name)]);
            if current { self.recents.note = Some(note); } else {
                self.append_draft(back, &text);
                window.push_notification(Notification::warning(note), cx);
            }
            return;
        };
        self.drafts.remove(&back);
        self.append_draft(key.clone(), &text);
        if !current {
            // Ela pode ser a aberta agora: o campo mostra o rascunho somado.
            if self.selected_key().as_ref() == Some(&key) && let Some(draft) = self.drafts.get(&key).cloned() {
                self.composer.update(cx, |input, cx| input.set_value(draft, window, cx));
            }
            return;
        }
        if self.select_on(&target, session, window, cx) { self.composer.update(cx, |input, cx| input.focus(window, cx)); }
    }

    /// Soma `text` ao rascunho de `key`, com uma linha em branco entre os dois: nenhum se perde.
    fn append_draft(&mut self, key: SessionKey, text: &str) {
        let draft = self.drafts.entry(key).or_default();
        *draft = if draft.trim().is_empty() { text.to_owned() } else { format!("{draft}\n\n{text}") };
    }

    /// Guarda o campo da tela sem sessão em que se está (nova conversa ou conversa fechada) antes de sair dela: cada uma tem
    /// o próprio texto. O texto em voo da retomada não fica em nenhuma: o resultado dela decide onde ele vai parar.
    pub(super) fn stash_view_draft(&mut self, cx: &mut Context<Self>) {
        if self.selected.is_some() { return; }
        let key = match &self.reopen {
            Some(_) if self.recents.sending => return,
            Some(entry) => reopen_key(self.server.as_deref().unwrap_or_default(), &entry.session_id),
            None => super::create::new_chat_key(),
        };
        let text = self.composer.read(cx).value().to_string();
        if text.is_empty() { self.drafts.remove(&key); } else { self.drafts.insert(key, text); }
    }

    /// O campo passa a mostrar o rascunho da tela sem sessão que acabou de abrir.
    pub(super) fn load_view_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let key = match &self.reopen {
            Some(entry) => reopen_key(self.server.as_deref().unwrap_or_default(), &entry.session_id),
            None => super::create::new_chat_key(),
        };
        let text = self.drafts.get(&key).cloned().unwrap_or_default();
        self.composer.update(cx, |input, cx| input.set_value(text, window, cx));
    }

    /// A retomada virou sessão viva: sai da lista já, sem esperar a releitura.
    fn forget_recent(&mut self, session_id: &str) {
        if let Some(Ok(list)) = self.recents.list.value.as_mut() { list.retain(|e| e.session_id != session_id); }
        self.recents.focus.remove(session_id);
    }

    /// O aviso de conta sem cota que impede o Enviar na conversa fechada.
    fn reopen_quota_block(&self, cx: &App) -> Option<String> {
        self.new_chat.as_ref().filter(|_| self.reopen.is_some()).and_then(|view| view.read(cx).reopen_quota_block())
    }

    pub(super) fn reopen_blocked(&self, cx: &App) -> bool { self.reopen_quota_block(cx).is_some() }

    /// A ordem é a da primeira mensagem da tela sem sessão (`receive_create`): rascunho, entrega, e só então a seleção,
    /// que põe no campo o rascunho que sobrou (o texto, se a entrega falhou).
    fn reopened(&mut self, identity: String, session: SessionInfo, text: String, sent: Result<Delivery, Failure>, current: bool,
        window: &mut Window, cx: &mut Context<Self>) {
        let target = super::servers::norm(&identity);
        // Nasceu numa máquina que não é mais a ativa: entra na lista guardada dela, senão a leitura que chega primeiro a fecharia.
        if let Some(list) = self.remote.get_mut(&target).filter(|l| l.loaded && !l.sessions.iter().any(|s| s.name == session.name)) {
            list.sessions.push(session.clone());
        }
        let readable = session.readable();
        match SessionKey::new(&identity, &session) {
            Some(key) => {
                if sent.is_ok() { self.input_history.entry(key.clone()).or_default().record(&text); }
                self.drafts.entry(key.clone()).or_insert_with(|| text.clone());
                self.delivery.begin(key.clone(), text.clone(), HashSet::new());
                self.receive_sent(key, text.clone(), text.clone(), sent.clone(), window, cx);
            }
            None => if let Err(error) = &sent {
                window.push_notification(Notification::warning(tr("first_message_not_sent").replace("{erro}", &Self::failure(error))).autohide(false), cx);
            },
        }
        if !current { return; }
        // A seleção tira a conversa fechada da tela; com `sending` ainda ligado, o texto enviado não vira rascunho dela.
        if self.select_on(&target, session.clone(), window, cx) {
            if sent.is_err() && self.composer.read(cx).value().is_empty() { self.composer.update(cx, |input, cx| input.set_value(text, window, cx)); }
            if readable { self.composer.update(cx, |input, cx| input.focus(window, cx)); }
        } else {
            self.reopen = None;
            self.load_view_draft(window, cx);
        }
    }

    /// A seção "Recentes" da barra no modo Conversas, depois das sessões vivas. Vazia, não aparece.
    pub(super) fn render_recents(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        if !self.recents_shown() { return Vec::new(); }
        let header = || div().flex().items_center().justify_between().px(px(8.)).pt(px(12.)).pb(px(6.))
            .child(chrome::section_label(web("conversas_recentes"))).into_any_element();
        let list = &self.recents.list;
        let entries = match &list.value {
            None if list.loading => return vec![header(), div().id("recents-loading").role(Role::Status).aria_label(tr("loading"))
                .flex().flex_col().gap(px(2.)).children((0..3).map(|row| div().h(px(29.)).px(px(8.)).flex().items_center()
                    .child(chrome::Skeleton::new(("recents-skeleton", row)).row(row).w(relative(0.7)).h(px(12.)).rounded(px(4.)))))
                .into_any_element()],
            None => return Vec::new(),
            Some(Err(error)) => return vec![header(), div().px(px(8.)).py(px(4.)).flex().flex_col().items_start().gap(px(6.))
                .child(div().id("recents-error").role(Role::Alert).text_xs().text_color(theme::warning()).whitespace_normal().child(error.clone()))
                .child(Button::new("recents-retry").outline().xsmall().label(tr("retry")).disabled(list.loading)
                    .on_click(cx.listener(|this, _, _, cx| { this.load_recents(cx); cx.notify(); })))
                .into_any_element()],
            Some(Ok(entries)) if entries.is_empty() => return Vec::new(),
            Some(Ok(entries)) => entries,
        };
        let (_, selection, hover, _) = theme::conversation_sidebar();
        let mut rows = vec![header()];
        for entry in entries {
            let on = self.reopen.as_ref().is_some_and(|r| r.session_id == entry.session_id);
            let focus = self.recents.focus.get(&entry.session_id);
            let title = entry.heading();
            let label = format!("{title} · {}", entry.meta());
            let provider = if entry.provider.is_empty() { "claude" } else { entry.provider.as_str() };
            let (click, key) = (entry.clone(), entry.clone());
            rows.push(div().id(SharedString::from(format!("recent-row-{}", entry.session_id))).flex_shrink_0().h(px(29.))
                .px(px(8.)).py(px(6.)).flex().items_center().gap(px(4.)).rounded(px(8.)).text_color(theme::text()).cursor_pointer()
                .when_some(focus, |el, focus| el.track_focus(focus))
                .when(focus.is_some_and(|f| f.is_focused(window)), |el| el.focus_ring_style(window, cx))
                .when(on, |el| el.bg(selection))
                .when(!on, |el| el.hover(|el| el.bg(hover)))
                .role(Role::Button).aria_selected(on).aria_label(label.clone())
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(label.clone()).build(window, cx))
                // O balão no lugar do ponto de estado: conversa sem sessão viva não tem estado.
                .child(div().size(px(13.)).flex_shrink_0().flex().items_center().justify_center()
                    .child(chrome::small_icon(IconName::MessageCircle, 12., theme::muted())))
                .child(badge(agent_name(provider).to_owned(), theme::muted()))
                .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).line_height(px(17.)).child(title))
                .child(div().flex_shrink_0().text_size(px(11.)).line_height(px(14.)).text_color(theme::muted())
                    .child(super::side::ago(chrono::Local::now().timestamp() as f64 - entry.mtime)))
                .on_click(cx.listener(move |this, _, window, cx| this.open_closed(click.clone(), window, cx)))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                    if !matches!(event.keystroke.key.as_str(), "enter" | "space") { return; }
                    this.open_closed(key.clone(), window, cx);
                    cx.stop_propagation();
                }))
                .into_any_element());
        }
        rows
    }

    /// A conversa fechada aberta: o histórico somente leitura e o compositor normal embaixo.
    pub(super) fn render_reopen(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(entry) = self.reopen.clone() else { return div().into_any_element() };
        let history = &self.recents.history;
        let body = match history.value.as_ref().filter(|_| !history.loading) {
            None => div().id("reopen-loading").role(Role::Status).text_sm().text_color(theme::muted()).child(tr("loading")).into_any_element(),
            Some(Err(error)) => div().flex().flex_col().items_start().gap(px(6.))
                .child(div().id("reopen-error").role(Role::Alert).text_sm().text_color(theme::warning()).whitespace_normal().child(error.clone()))
                .child(Button::new("reopen-retry").outline().xsmall().label(tr("retry"))
                    .on_click(cx.listener(|this, _, _, cx| { this.load_reopen_history(cx); cx.notify(); })))
                .into_any_element(),
            Some(Ok(lines)) if lines.is_empty() => div().text_sm().text_color(theme::muted()).child(tr("create_no_messages")).into_any_element(),
            Some(Ok(lines)) => render_preview_lines(lines, cx).into_any_element(),
        };
        let composer = self.render_composer(false, false, false, 0, false, false, window, cx);
        // Mesma view da tela sem sessão: a pílula de conta e o menu dela (com a cota) vêm de lá.
        let account = self.api.clone().and_then(|api| {
            let codex = [&entry.conta, entry.codex_account.as_ref().unwrap_or(&String::new())].into_iter()
                .find(|s| !s.is_empty()).cloned().unwrap_or_else(|| tr("create_default"));
            self.ensure_new_chat(api, window, cx).update(cx, |view, cx| view.render_reopen_account(&entry.provider, codex, cx))
        });
        let block = self.reopen_quota_block(cx);
        let (note, warning) = match (&self.recents.note, self.recents.sending, block) {
            (Some(error), ..) => (error.clone(), true),
            (None, true, _) => (web("conversa_retomando"), false),
            (None, false, Some(block)) => (block, true),
            (None, false, None) => (web("conversa_reabrir_dica"), false),
        };
        div().id("reopen").size_full().flex().flex_col()
            .child(div().id("reopen-history").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.recents.scroll).pt(px(20.)).pb(px(12.))
                .child(landing_column(div().flex().flex_col().gap(px(12.))
                    .child(div().flex().flex_col().gap(px(2.))
                        .child(div().text_base().font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(entry.heading()))
                        .child(div().text_xs().text_color(theme::faint()).child(entry.meta())))
                    .child(body))))
            .child(composer)
            .children(account.map(landing_column))
            .child(landing_column(div().id("reopen-note").role(if warning { Role::Alert } else { Role::Status }).px(px(14.)).pb(px(10.))
                .text_sm().whitespace_normal().text_color(if warning { theme::warning() } else { theme::muted() }).child(note)))
            .into_any_element()
    }
}
