//! Continuar uma conversa antiga da pasta escolhida (`CreateSessionSheet.svelte`): a lista de `archive-por-cwd`, a prévia das
//! últimas mensagens no lugar da lista de pastas e o retomar pela rota do arquivo. Escolher uma conversa leva a conta à dona dela.
use super::*;

#[derive(Clone, Debug, Deserialize)]
pub(super) struct ArchiveEntry {
    project: String,
    session_id: String,
    #[serde(default)] mtime: f64,
    #[serde(default)] preview: String,
    #[serde(default)] ultima: String,
    #[serde(default)] live: bool,
    config_dir: Option<String>,
    #[serde(default)] conta: String,
    #[serde(default)] provider: String,
    codex_account: Option<String>,
}

impl ArchiveEntry {
    /// A última mensagem é o que identifica a conversa meses depois.
    fn title(&self) -> String {
        [&self.ultima, &self.preview].into_iter().find(|t| !t.trim().is_empty()).cloned().unwrap_or_else(|| tr("create_no_messages"))
    }
    fn query(&self) -> Vec<(String, String)> {
        let mut q = Vec::new();
        if self.provider != "claude" && !self.provider.is_empty() { q.push(("provider".into(), self.provider.clone())); }
        if self.provider == "codex" && let Some(a) = &self.codex_account { q.push(("codex_account".into(), a.clone())); }
        q
    }
}

/// Uma mensagem da prévia: a do usuário vem realçada.
pub(super) struct PreviewLine { mine: bool, view: Entity<TextViewState> }

impl NewSession {
    /// A lista da pasta e do agente escolhidos; a escolha anterior não vale para outra pasta ou conta.
    pub(super) fn load_archive(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.archive.start();
        self.want_resume = false;
        self.conversation.clear();
        self.target_changed(window, cx);
        // O bastão fica de fora: retomar reabre uma conversa antiga, e o bastão é o oposto, uma sessão nova com o resumo.
        let Some(cwd) = self.picked.clone().filter(|_| self.baton.is_none() && !(self.provider == "codex" && self.codex_account.is_empty())) else {
            self.archive.finish(seq, Ok(Vec::new()));
            return;
        };
        let mut query = vec![("cwd".to_owned(), cwd)];
        if self.provider != "claude" { query.push(("provider".into(), self.provider.to_owned())); }
        if self.provider == "codex" { query.push(("codex_account".into(), self.codex_account.clone())); }
        self.request(cx, move |api, send| Box::pin(async move {
            let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            send(CreateReply::Archive(seq, api.server_read(&["archive-por-cwd"], &query, 15).await)).await
        }));
    }

    /// A conversa que o botão vai continuar: só com o "continuar" ligado.
    pub(super) fn target(&self) -> Option<&ArchiveEntry> {
        self.want_resume.then(|| self.archive.ok()?.iter().find(|c| c.session_id == self.conversation)).flatten()
    }

    fn choose(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating { return; }
        if self.conversation == id { self.conversation.clear(); } else { self.conversation = id; }
        if let Some(c) = self.target().cloned() {
            let known = self.configs.ok().is_some_and(|l| l.iter().any(|k| Some(&k.path) == c.config_dir.as_ref()));
            if c.provider == "claude" && c.config_dir.is_some() && c.config_dir != self.config && known {
                if self.before.is_none() { self.before = Some(self.config.clone()); }
                self.config = c.config_dir.clone();
                self.build_config_pick(window, cx);
                self.load_models(window, cx);
            }
        }
        self.target_changed(window, cx);
        cx.notify();
    }

    fn set_want(&mut self, want: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.creating { return; }
        self.want_resume = want;
        self.target_changed(window, cx);
        cx.notify();
    }

    /// A escolha mudou: a prévia é refeita, e sem conversa a conta volta à que estava antes de a escolha puxá-la.
    fn target_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let seq = self.preview.start();
        let Some(c) = self.target().cloned() else {
            self.preview = Remote::default();
            self.preview.seq = seq;
            if let Some(back) = self.before.take() {
                let known = back.is_none() || self.configs.ok().is_some_and(|l| l.iter().any(|k| Some(&k.path) == back.as_ref()));
                if back != self.config && known {
                    self.config = back;
                    self.build_config_pick(window, cx);
                    self.load_models(window, cx);
                }
            }
            return;
        };
        let mut query = c.query();
        query.push(("tail".into(), "30".into()));
        if let Some(dir) = c.config_dir.clone() { query.push(("config_dir".into(), dir)); }
        self.request(cx, move |api, send| Box::pin(async move {
            let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            send(CreateReply::Preview(seq, api.server_read(&["archive", &c.project, &c.session_id, "history"], &query, 15).await)).await
        }));
    }

    pub(super) fn receive_archive(&mut self, reply: CreateReply, cx: &mut Context<Self>) {
        match reply {
            CreateReply::Archive(seq, result) => {
                let list = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| serde_json::from_value::<Vec<ArchiveEntry>>(v).map_err(|_| tr("invalid_response")));
                // Falhar esconde o atalho, como no web; o motivo fica no log para não confundir "sem conversa" com "não consegui perguntar".
                if let Err(error) = &list { eprintln!("lista de conversas retomáveis falhou: {error}"); }
                self.archive.finish(seq, list.map(|l| l.into_iter().filter(|c| !c.live).collect()));
            }
            CreateReply::Preview(seq, result) => {
                let events = result.map_err(|e| Hangar::fetch_failure(&e)).and_then(|v| serde_json::from_value::<Vec<ChatEvent>>(v).map_err(|_| tr("invalid_response")));
                if let Err(error) = &events { eprintln!("prévia da conversa falhou: {error}"); }
                let lines = events.map(|events| events.into_iter().filter(|e| matches!(e.kind.as_str(), "user_msg" | "assistant_msg"))
                    .filter_map(|e| e.text.filter(|t| !t.trim().is_empty()).map(|t| (e.kind == "user_msg", t)))
                    .map(|(mine, text)| PreviewLine { mine, view: cx.new(|cx| TextViewState::markdown(&safe_markdown(&text), cx)) }).collect());
                // A prévia abre no fim: ela existe para mostrar onde a conversa parou.
                if self.preview.finish(seq, lines) { self.preview_scroll.scroll_to_bottom(); }
            }
            _ => {}
        }
    }

    /// Retoma pela rota do web; motor e conta só existem no Claude.
    pub(super) fn resume(&mut self, cx: &mut Context<Self>) {
        let Some(c) = self.target().cloned() else { return };
        if self.creating { return; }
        let claude = c.provider == "claude";
        let mut body = json!({"engine": if claude && !self.engine.is_empty() { json!(self.engine) } else { Value::Null },
            "config_dir": if claude { json!(self.config) } else { Value::Null }, "provider": c.provider});
        if c.provider == "codex" && let Some(account) = &c.codex_account { body["codex_account"] = json!(account); }
        self.create_seq += 1;
        let seq = self.create_seq;
        (self.creating, self.resuming, self.error) = (true, true, None);
        self.request(cx, move |api, send| Box::pin(async move {
            let result = api.server_send(reqwest::Method::POST, &["archive", &c.project, &c.session_id, "resume"], Some(body), 120).await;
            let opened = result.map_err(|e| Hangar::fetch_failure(&e))
                .and_then(|v| serde_json::from_value(v).map_err(|_| tr("invalid_response"))).map(|session| Opened { session, notes: Vec::new(), warning: None });
            send(CreateReply::Created(seq, opened)).await
        }));
        cx.notify();
    }

    /// O rótulo do botão com uma conversa escolhida: continuar, continuar na conta dela, ou mover para a conta do seletor.
    pub(super) fn resume_label(&self, c: &ArchiveEntry) -> String {
        if self.creating { return tr("create_creating"); }
        let moving = c.provider == "claude" && c.config_dir.is_some() && self.config.is_some() && c.config_dir != self.config;
        if moving {
            let account = self.configs.ok().and_then(|l| l.iter().find(|k| Some(&k.path) == self.config.as_ref())).map(|k| k.label.clone()).unwrap_or_default();
            return tr("create_resume_move").replace("{conta}", &account);
        }
        if c.provider == "claude" && !c.conta.is_empty() { return tr("create_resume_in").replace("{conta}", &c.conta); }
        tr("create_resume_action")
    }

    pub(super) fn render_resume(&self, cx: &mut Context<Self>) -> Option<Div> {
        let list = self.archive.ok().filter(|l| !l.is_empty())?;
        Some(div().flex().flex_col().gap(px(8.))
            .child(Checkbox::new("create-resume").label(tr("create_resume")).checked(self.want_resume).disabled(self.creating)
                .on_click(cx.listener(|this, checked: &bool, window, cx| this.set_want(*checked, window, cx))))
            .when(self.want_resume, |el| el.child(div().id("create-conversations").role(Role::Group).aria_label(tr("create_resume_choose"))
                .max_h(px(208.)).overflow_y_scroll().rounded(px(8.)).border_1().border_color(theme::border())
                .children(list.iter().enumerate().map(|(n, c)| {
                    let on = self.conversation == c.session_id;
                    let id = c.session_id.clone();
                    let meta = [Some(c.conta.clone()).filter(|s| !s.is_empty()), Some(super::super::side::ago(chrono::Local::now().timestamp() as f64 - c.mtime))]
                        .into_iter().flatten().collect::<Vec<_>>().join(" · ");
                    choice(SharedString::from(format!("create-conversation-{}", c.session_id)), on, cx).w_full().h_auto().py(px(8.)).px(px(10.))
                        .rounded(px(0.)).when(n > 0, |b| b.border_t_1()).border_l_0().border_r_0().when(n == 0, |b| b.border_t_0()).border_b_0()
                        .selected(on).disabled(self.creating).accessibility_label(c.title())
                        .child(div().w_full().flex().items_center().gap(px(8.))
                            .child(div().flex_1().min_w_0().flex().flex_col().items_start().gap(px(2.))
                                .child(div().w_full().truncate().text_sm().child(c.title()))
                                .child(div().text_size(px(11.)).text_color(theme::faint()).child(meta)))
                            .child(div().w(px(14.)).flex_shrink_0().text_color(theme::accent()).child(if on { "✓" } else { "" })))
                        .on_click(cx.listener(move |this, _, window, cx| this.choose(id.clone(), window, cx)))
                })))))
    }

    /// A leitura da conversa escolhida, no lugar da lista de pastas; o ✕ volta à lista.
    pub(super) fn render_preview(&self, c: &ArchiveEntry, cx: &mut Context<Self>) -> Div {
        let body = match self.preview.value.as_ref().filter(|_| !self.preview.loading) {
            None => muted(tr("loading")).into_any_element(),
            Some(Err(_)) => alert("create-preview-error", tr("create_preview_failed")).into_any_element(),
            Some(Ok(lines)) if lines.is_empty() => muted(tr("create_no_messages")).into_any_element(),
            Some(Ok(lines)) => div().flex().flex_col().gap(px(8.)).children(lines.iter().map(|line| div().p(px(10.)).rounded(px(8.)).text_sm()
                .when(line.mine, |el| el.bg(theme::accent_dim()).ml(px(32.))).when(!line.mine, |el| el.bg(theme::inset()).mr(px(32.)))
                .child(TextView::new(&line.view).selectable(true).scrollable(false)))).into_any_element(),
        };
        div().flex_1().min_h_0().flex().flex_col().gap(px(8.))
            .child(div().flex().items_center().gap(px(8.))
                .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).child(c.title()))
                .child(Button::new("create-preview-close").ghost().small().icon(IconName::Close).accessibility_label(tr("create_preview_close"))
                    .tooltip(tr("create_preview_close")).disabled(self.creating)
                    .on_click(cx.listener(|this, _, window, cx| { this.conversation.clear(); this.target_changed(window, cx); cx.notify(); }))))
            .child(div().id("create-preview").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.preview_scroll).child(body))
    }
}
