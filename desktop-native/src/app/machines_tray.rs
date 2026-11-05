//! Bandeja das abas: as listas já abertas de cada máquina, sem streams próprios.
use super::*;
use super::sidebar::Target;
use gpui_kit::component::switch::Switch;

pub(super) struct Tray {
    pub(super) open: bool,
    focus: FocusHandle,
    /// Máquina escolhida na trilha; `None` mostra todas.
    machine: Option<String>,
    filter: Filter,
    options: bool,
    collapsed: HashSet<String>,
    search: Option<(Entity<InputState>, Subscription)>,
}

impl Tray {
    pub(super) fn new(cx: &mut Context<Hangar>) -> Self {
        Self { open: false, focus: cx.focus_handle(), machine: None, filter: Filter::All, options: false, collapsed: HashSet::new(), search: None }
    }
}

impl Hangar {
    pub(super) fn ungrouped_tab_targets(&self) -> Vec<Target> {
        let active = self.active_key();
        let open = self.selected_target();
        let listed = |key: &str, s: &SessionInfo| !self.sidebar.is_hidden(key, &s.name) && sidebar::listed_on(s, &Target::new(key, &s.name), open.as_ref());
        let mut targets = self.sessions.iter().filter(|s| listed(&active, s))
            .map(|s| Target::new(&active, &s.name)).collect::<Vec<_>>();
        if appearance::get().show_remote_tabs {
            for entry in self.servers.iter().filter(|s| servers::has_section(s)) {
                let key = servers::norm(&entry.address);
                if key == active { continue; }
                targets.extend(self.sessions_of(&key).iter().filter(|s| listed(&key, s))
                    .map(|s| Target::new(&key, &s.name)));
            }
        }
        targets
    }

    fn other_machines_attention(&self) -> usize {
        let active = self.active_key();
        self.servers.iter().filter(|entry| servers::has_section(entry)).map(|entry| servers::norm(&entry.address))
            .filter(|key| key != &active).map(|key| self.sessions_of(&key).iter()
                .filter(|s| s.state == "awaiting_input" || s.pending_questions > 0).count()).sum()
    }

    fn tray_sessions<'a>(&'a self, key: &str) -> (Vec<&'a SessionInfo>, Vec<&'a SessionInfo>) {
        tray_columns(self.sessions_of(key), key, self.selected_target().as_ref(), |name| self.sidebar.is_hidden(key, name))
    }

    /// As máquinas da bandeja: as ligadas; sem nenhuma, a ativa.
    fn tray_keys(&self) -> Vec<String> {
        let keys: Vec<String> = self.servers.iter().filter(|s| !s.disabled).map(|s| servers::norm(&s.address)).collect();
        if keys.is_empty() { vec![self.active_key()] } else { keys }
    }

    /// O que a bandeja mostra somado: (todas as linhas, só as do grupo "Ocultas").
    pub(super) fn tray_counts(&self) -> (usize, usize) {
        self.tray_keys().iter().map(|key| self.tray_sessions(key))
            .fold((0, 0), |(all, hidden), (s, h)| (all + s.len() + h.len(), hidden + h.len()))
    }

    /// Abre a bandeja com o foco nela.
    pub(super) fn open_machines_tray(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_popups();
        self.machines_tray.open = true;
        if self.machines_tray.search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("machines_tray_search")));
            let changed = cx.subscribe(&input, |_: &mut Hangar, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) { cx.notify(); });
            self.machines_tray.search = Some((input, changed));
        }
        self.machines_tray.focus.focus(window, cx);
        cx.notify();
    }

    fn other_machines_have_sessions(&self) -> bool {
        let active = self.active_key();
        self.servers.iter().filter(|entry| !entry.disabled).any(|entry| {
            let key = servers::norm(&entry.address);
            (key != active || entry.invite) && self.sessions_of(&key).iter().any(|s| !self.sidebar.is_hidden(&key, &s.name))
        })
    }

    pub(super) fn render_machines_tray_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let occupied = self.other_machines_have_sessions();
        let attention = self.other_machines_attention();
        let tip = if attention > 0 { tr("machines_tray_attention").replace("{count}", &attention.to_string()) }
            else { tr(if occupied { "machines_tray_occupied" } else { "machines_tray_open" }) };
        let icon = div().relative().size(px(18.)).child(svg().path(crate::MACHINES_GLYPH).size_full()
            .text_color(if attention > 0 { theme::warning() } else { theme::muted() }))
            .when(occupied && attention == 0, |el| el.child(div().id("machines-tray-dot").absolute().right(px(-2.)).top(px(-2.))
                .size(px(5.)).rounded_full().bg(theme::faint())))
            .when(attention > 0, |el| el.child(div().id("machines-tray-attention").absolute().right(px(-6.)).top(px(-6.))
                .min_w(px(13.)).h(px(13.)).px(px(2.)).rounded_full().bg(theme::warning()).text_color(theme::chrome())
                .text_size(px(9.)).font_weight(FontWeight::BOLD).flex().items_center().justify_center()
                .child(if attention > 99 { "99+".to_owned() } else { attention.to_string() })));
        // O total que a bandeja mostra, ocultas incluídas.
        let total = self.tray_counts().0;
        let button = Button::new("machines-tray-toggle").ghost().h(px(28.)).px(px(6.)).rounded(px(6.))
            .child(div().flex().items_center().gap(px(6.)).child(icon)
                .child(div().font_family(theme::MONO).text_size(px(11.)).text_color(theme::faint()).child(total.to_string())))
            .selected(self.machines_tray.open).tooltip(tip.clone()).accessibility_label(tip)
            .on_click(cx.listener(|this, _, window, cx| {
                if this.machines_tray.open { this.close_popups(); cx.notify(); } else { this.open_machines_tray(window, cx); }
            }));
        popup::anchor(button.relative(), "machines-tray-toggle").into_any_element()
    }

    /// As máquinas da bandeja com o estado da lista de cada uma: as ligadas; sem nenhuma, a ativa.
    fn tray_machines(&self, cx: &App) -> Vec<Machine> {
        let active = self.active_key();
        let mut machines: Vec<Machine> = self.servers.iter().filter(|s| !s.disabled).map(|entry| {
            let key = servers::norm(&entry.address);
            let (loaded, online, error) = if key == active {
                (self.list_online || self.list_error.is_some(), self.list_online,
                    self.invite_ended.contains(&key).then(|| tr_shared("convite_encerrado", &[])).or(self.list_error.clone()))
            } else {
                self.remote.get(&key).map(|list| (list.loaded, list.online, list.error.clone())).unwrap_or_default()
            };
            Machine { key, label: entry.label.clone(), invite: entry.invite, loaded, online, error }
        }).collect();
        if machines.is_empty() {
            machines.push(Machine { key: active, label: self.server_label(cx), invite: self.active_invite(),
                loaded: self.list_online || self.list_error.is_some(), online: self.list_online, error: self.list_error.clone() });
        }
        machines
    }

    fn tray_query(&self, cx: &App) -> String {
        self.machines_tray.search.as_ref().map(|(input, _)| input.read(cx).value().trim().to_lowercase()).unwrap_or_default()
    }

    /// O que a seção da máquina mostra com a busca e o filtro: (no topo, à parte em "Ocultas"). O grupo das ocultas só
    /// existe sem busca e sem filtro; com eles, a oculta que casa vem junto das outras.
    fn tray_view<'a>(&'a self, key: &str, query: &str) -> (Vec<&'a SessionInfo>, Vec<&'a SessionInfo>) {
        let (shown, hidden) = self.tray_sessions(key);
        let filter = self.machines_tray.filter;
        let keep = |s: &&SessionInfo| matches_query(s, query) && filter.keeps(s);
        let mut shown: Vec<_> = match filter {
            Filter::Hidden => vec![],
            _ => shown.into_iter().filter(keep).collect(),
        };
        let hidden: Vec<_> = hidden.into_iter().filter(keep).collect();
        // Quem espera a pessoa primeiro, depois quem trabalha; dentro de cada faixa, a ordem por nome de antes.
        shown.sort_by_key(|s| state_rank(s));
        if filter == Filter::All && query.is_empty() { (shown, hidden) } else { (shown.into_iter().chain(hidden).collect(), vec![]) }
    }

    pub(super) fn render_machines_tray(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let machines = self.tray_machines(cx);
        let rail = machines.len() > 1;
        let picked = self.machines_tray.machine.as_ref().filter(|key| machines.iter().any(|m| &m.key == *key));
        let viewport = window.viewport_size();
        let width = px(if rail { 780. } else { 560. }).min(viewport.width - px(32.));
        let height = (viewport.height - px(250.)).clamp(px(160.), px(520.));
        let query = self.tray_query(cx);
        let scope: Vec<&Machine> = machines.iter().filter(|m| picked.is_none_or(|key| &m.key == key)).collect();
        // Os números dos filtros contam o escopo da trilha, sem a busca.
        let in_scope: Vec<&SessionInfo> = scope.iter().flat_map(|m| { let (s, h) = self.tray_sessions(&m.key); s.into_iter().chain(h) }).collect();
        let count = |filter: Filter| in_scope.iter().filter(|s| filter.keeps(s)).count();
        let hidden_total: usize = scope.iter().map(|m| self.tray_sessions(&m.key).1.len()).sum();
        let chips = [(Filter::All, "machines_tray_filter_all", in_scope.len()), (Filter::Working, "machines_tray_filter_working", count(Filter::Working)),
            (Filter::Waiting, "machines_tray_filter_waiting", count(Filter::Waiting)), (Filter::Hidden, "machines_tray_filter_hidden", hidden_total)]
            .map(|(filter, label, n)| {
                let on = self.machines_tray.filter == filter;
                div().id(SharedString::from(format!("machines-tray-filter-{label}"))).h(px(22.)).px(px(9.)).flex().items_center().gap(px(4.))
                    .rounded_full().border_1().cursor_pointer().text_size(px(11.5))
                    .border_color(if on { theme::accent() } else { theme::border() })
                    .when(on, |el| el.bg(theme::accent_dim()).text_color(theme::accent_text()))
                    .when(!on, |el| el.text_color(theme::muted()).hover(|el| el.bg(theme::hover())))
                    .role(Role::Button).aria_selected(on).aria_label(format!("{} · {n}", tr(label)))
                    .child(tr(label)).child(div().font_weight(FontWeight::SEMIBOLD).child(n.to_string()))
                    .on_click(cx.listener(move |this, _, _, cx| { this.machines_tray.filter = filter; cx.notify(); }))
            });
        let sections: Vec<AnyElement> = scope.iter().map(|m| self.render_machine_section(m, &query, window, cx)).collect();
        let compact = appearance::get().machines_tray_compact;
        let search = self.machines_tray.search.as_ref().map(|(input, _)| div().flex_1().min_w(px(180.))
            .child(Input::new(input).small().cleanable(true).prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))
                .aria_label(tr("machines_tray_search"))));
        div().id("machines-tray").relative().w(width).p(px(12.)).flex().flex_col().gap(px(10.)).track_focus(&self.machines_tray.focus)
            .role(Role::Dialog).aria_label(tr("machines_tray_title"))
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                let Some((input, _)) = this.machines_tray.search.as_ref() else { return };
                if input.read(cx).focus_handle(cx).is_focused(window) { return; }
                match event.keystroke.key.as_str() {
                    // Depois do evento: focado agora, o próprio "/" seria digitado na busca.
                    "/" => { let input = input.clone(); window.defer(cx, move |window, cx| input.update(cx, |state, cx| state.focus(window, cx))); }
                    "down" => window.focus_next(cx),
                    "up" => window.focus_prev(cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .child(div().flex().items_center().gap_2()
                .child(div().flex_1().text_sm().font_weight(FontWeight::SEMIBOLD).child(tr("machines_tray_title")))
                .child(super::settings::segments("machines-tray-density", &[tr("machines_tray_detailed"), tr("settings_sidebar_compact")],
                    compact as usize, 2, false, String::new(),
                    |this: &mut Hangar, index, _: &mut Window, cx| {
                        this.apply_appearance(appearance::Appearance { machines_tray_compact: index == 1, ..appearance::get() }, true, cx);
                    }, cx))
                .child(chrome::icon_button("machines-tray-options", IconName::Settings, tr("machines_tray_options"), cx)
                    .selected(self.machines_tray.options)
                    .on_click(cx.listener(|this, _, _, cx| { this.machines_tray.options = !this.machines_tray.options; cx.notify(); })))
                .child(chrome::icon_button("machines-tray-close", IconName::Close, tr("close"), cx)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.close_popups();
                        this.root_focus.focus(window, cx);
                        cx.notify();
                    }))))
            .child(div().flex().flex_wrap().items_center().gap(px(8.)).children(search).child(div().flex().flex_wrap().gap(px(4.)).children(chips)))
            .child(div().flex().gap(px(10.)).min_h_0()
                .when(rail, |el| el.child(self.render_tray_rail(&machines, picked.cloned(), cx)))
                .child(div().id("machines-tray-list").relative().flex_1().min_w_0().max_h(height).overflow_y_scroll().pr(px(4.))
                    .flex().flex_col().gap(px(12.)).children(sections)))
            .child(div().pt_2().border_t_1().border_color(theme::border()).flex().flex_wrap().gap(px(12.)).text_xs().text_color(theme::muted())
                .child(div().whitespace_normal().child(tr("machines_tray_pair_hint")))
                .child(div().text_color(theme::faint()).child(tr("machines_tray_keys_hint"))))
            // Adiado: na mesma camada, a lista rolável era pintada por cima do quadro.
            .when(self.machines_tray.options, |el| el.child(deferred(self.render_tray_options(cx)).with_priority(gpui_kit::base::POPUP_PRIORITY + 1)))
            .into_any_element()
    }

    /// As opções da barra de abas da tela de Aparência, à mão de quem olha as abas, num quadro sobre a lista.
    fn render_tray_options(&self, cx: &mut Context<Self>) -> AnyElement {
        let row = |label: &str| div().flex().items_center().gap_2().child(div().flex_1().min_w_0().text_xs().whitespace_normal().child(tr(label)));
        div().id("machines-tray-options-panel").absolute().top(px(46.)).right(px(12.)).w(px(310.)).p(px(12.)).flex().flex_col().gap(px(10.))
            // Fundo sólido: o quadro cobre a lista, e o vidro deixava ler as linhas por trás das opções.
            .rounded(px(10.)).border_1().border_color(theme::glass_border()).bg(theme::elevated()).shadow(theme::popover_shadow())
            .occlude().role(Role::Group).aria_label(tr("machines_tray_options"))
            .child(div().text_size(px(10.5)).text_color(theme::muted()).child(tr("machines_tray_tabs_heading")))
            .child(row("machines_tray_show_remote_tabs")
                .child(Switch::new("machines-tray-show-remote-tabs").checked(appearance::get().show_remote_tabs)
                    .accessibility_label(tr("machines_tray_show_remote_tabs"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.cancel_session_numbers(cx);
                        this.apply_appearance(appearance::Appearance { show_remote_tabs: *on, ..appearance::get() }, true, cx);
                        this.redraw(panes::Area::Nav, cx);
                    }))))
            .child(row("settings_tab_preview")
                .child(Switch::new("machines-tray-tab-preview").checked(appearance::get().tab_preview)
                    .accessibility_label(tr("settings_tab_preview"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.apply_appearance(appearance::Appearance { tab_preview: *on, ..appearance::get() }, true, cx);
                    }))))
            .child(row("settings_tabs_working_first")
                .child(Switch::new("machines-tray-tabs-working-first").checked(appearance::get().tabs_working_first)
                    .accessibility_label(tr("settings_tabs_working_first"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.apply_appearance(appearance::Appearance { tabs_working_first: *on, ..appearance::get() }, true, cx);
                        this.redraw(panes::Area::Nav, cx);
                    }))))
            .child(row("settings_tabs_recent_first")
                .child(Switch::new("machines-tray-tabs-recent-first").checked(appearance::get().tabs_recent_first)
                    .accessibility_label(tr("settings_tabs_recent_first"))
                    .on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.apply_appearance(appearance::Appearance { tabs_recent_first: *on, ..appearance::get() }, true, cx);
                        this.redraw(panes::Area::Nav, cx);
                    }))))
            .child(row("settings_tabs_detail")
                .child(super::settings::segments("machines-tray-tabs-detail", &[tr("settings_sidebar_compact"), tr("settings_tabs_full")],
                    appearance::get().tabs_full as usize, 2, false, String::new(),
                    |this: &mut Hangar, index, _: &mut Window, cx| {
                        this.apply_appearance(appearance::Appearance { tabs_full: index == 1, ..appearance::get() }, true, cx);
                        this.redraw(panes::Area::Nav, cx);
                    }, cx)))
            .into_any_element()
    }

    /// A trilha das máquinas: "Todas" e cada uma com o estado da lista e quantas trabalham ou esperam.
    fn render_tray_rail(&self, machines: &[Machine], picked: Option<String>, cx: &mut Context<Self>) -> AnyElement {
        let item = |id: String, pick: Option<String>, dot: Hsla, name: String, sub: (String, Hsla), total: usize, cx: &mut Context<Self>| {
            let on = picked == pick;
            div().id(SharedString::from(id)).w_full().px(px(8.)).py(px(6.)).flex().items_center().gap(px(7.)).rounded(px(7.)).cursor_pointer()
                .when(on, |el| el.bg(theme::accent_dim()))
                .when(!on, |el| el.hover(|el| el.bg(theme::hover())))
                .role(Role::Button).aria_selected(on).aria_label(format!("{name} · {}", sub.0))
                .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(dot))
                .child(div().flex_1().min_w_0().flex().flex_col()
                    .child(div().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(name))
                    .child(div().truncate().text_size(px(10.5)).text_color(sub.1).child(sub.0)))
                .child(div().flex_shrink_0().text_xs().text_color(theme::faint()).child(total.to_string()))
                .on_click(cx.listener(move |this, _, _, cx| { this.machines_tray.machine = pick.clone(); cx.notify(); }))
        };
        let mut all = 0;
        let mut rows = Vec::new();
        for m in machines {
            let (shown, hidden) = self.tray_sessions(&m.key);
            let total = shown.len() + hidden.len();
            all += total;
            let working = shown.iter().chain(&hidden).filter(|s| Filter::Working.keeps(s)).count();
            let waiting = shown.iter().chain(&hidden).filter(|s| Filter::Waiting.keeps(s)).count();
            let sub = if m.error.is_some() { (tr("machines_tray_offline"), theme::warning()) }
                else if !m.loaded { (tr("loading"), theme::muted()) }
                else if working + waiting == 0 { (tr("machines_tray_all_idle"), theme::muted()) }
                else {
                    let parts: Vec<String> = [(working, "machines_tray_working"), (waiting, "machines_tray_waiting")].into_iter()
                        .filter(|(n, _)| *n > 0).map(|(n, key)| tr_shared(key, &[("n", &n.to_string())])).collect();
                    (parts.join(" · "), if waiting > 0 { theme::warning() } else { theme::accent_text() })
                };
            rows.push(item(format!("machines-tray-rail-{}", m.key), Some(m.key.clone()), machine_dot(m), m.label.clone(), sub, total, cx));
        }
        let everyone = item("machines-tray-rail-all".to_owned(), None, theme::accent(), tr("machines_tray_all"),
            (tr("machines_tray_machines").replace("{n}", &machines.len().to_string()), theme::muted()), all, cx);
        div().id("machines-tray-rail").w(px(190.)).flex_shrink_0().flex().flex_col().gap(px(2.)).pr(px(10.)).border_r_1().border_color(theme::border())
            .role(Role::Group).aria_label(tr("machines_tray_title"))
            .child(everyone).children(rows)
            .into_any_element()
    }

    fn render_machine_section(&self, m: &Machine, query: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let key = m.key.as_str();
        let (sessions, hidden) = self.tray_view(key, query);
        let (all_shown, all_hidden) = self.tray_sessions(key);
        let total = all_shown.len() + all_hidden.len();
        let remote = (!self.is_active_key(key)).then_some(key);
        let filtered = !query.is_empty() || self.machines_tray.filter != Filter::All;
        // Com busca ou filtro a seção abre: recolhida, ela esconderia o que casou.
        let closed = !filtered && self.machines_tray.collapsed.contains(key);
        let mut rows = Vec::new();
        // O bloco do par junta só os membros desta máquina.
        let column: Vec<grouping::Row> = sessions.iter().map(|&session| grouping::Row { server: key, session, invite: m.invite }).collect();
        for row in grouping::cluster(&[column], &self.identities()).into_iter().flatten() {
            match row {
                grouping::ListRow::Header { key: block, gid, label, members } => rows.push(self.render_pair_header(&block, &gid, &label, &members, window, cx)),
                grouping::ListRow::Session { block: Some(b), .. } | grouping::ListRow::External { block: b, .. } if self.sidebar.is_collapsed(&b) => {}
                grouping::ListRow::Session { row, .. } => rows.push(self.render_machine_session(row.session, key, m.invite, false, window, cx)),
                grouping::ListRow::External { gid, pair, .. } =>
                    rows.push(self.render_external_pair_row(&gid, &pair.owner, &pair.session, &pair.alias, remote, window, cx)),
            }
        }
        // As ocultas no fim, recolhidas pelo mesmo interruptor do grupo da barra lateral.
        if !hidden.is_empty() {
            rows.push(self.render_hidden_header(key, hidden.len(), window, cx));
            if self.hidden_open() { rows.extend(hidden.iter().map(|s| self.render_machine_session(s, key, m.invite, true, window, cx))); }
        }
        let working = all_shown.iter().chain(&all_hidden).filter(|s| Filter::Working.keeps(s)).count();
        let waiting = all_shown.iter().chain(&all_hidden).filter(|s| Filter::Waiting.keeps(s)).count();
        let tally = |n: usize, key: &str, (bg, fg): (Hsla, Hsla)| (n > 0).then(|| div().flex_shrink_0().px(px(6.)).rounded_full().bg(bg).text_color(fg)
            .text_size(px(10.5)).child(tr_shared(key, &[("n", &n.to_string())])));
        let toggle = m.key.clone();
        let header = div().id(SharedString::from(format!("machines-tray-section-{key}"))).flex().items_center().gap(px(7.)).py(px(5.)).px(px(2.))
            .border_b_1().border_color(theme::border()).cursor_pointer()
            .role(Role::Button).aria_label(format!("{} · {total}", m.label))
            .child(chrome::small_icon(if closed { IconName::ChevronRight } else { IconName::ChevronDown }, 13., theme::faint()))
            .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(machine_dot(m)))
            .child(div().min_w_0().truncate().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).child(m.label.clone()))
            .when(m.invite, |el| el.child(div().flex_shrink_0().px(px(6.)).rounded_full().border_1().border_color(theme::border())
                .text_size(px(10.)).text_color(theme::muted()).child(tr("invite_badge"))))
            .children(tally(working, "machines_tray_working", theme::pill("working")))
            .children(tally(waiting, "machines_tray_waiting", theme::pill("awaiting_input")))
            .child(div().flex_1())
            .child(div().flex_shrink_0().text_xs().text_color(theme::faint()).child(total.to_string()))
            .on_click(cx.listener(move |this, _, _, cx| {
                let set = &mut this.machines_tray.collapsed;
                if !set.remove(&toggle) { set.insert(toggle.clone()); }
                cx.notify();
            }));
        let empty = rows.is_empty();
        let retrying = m.key.clone();
        let retry = remote.is_some().then(|| Button::new(SharedString::from(format!("machines-tray-retry-{key}"))).outline().xsmall()
            .label(tr("machines_tray_retry"))
            .on_click(cx.listener(move |this, _, _, cx| {
                // Volta a "Carregando" até a nova resposta: sem isso o clique não muda nada na tela.
                if let Some(list) = this.remote.get_mut(&retrying) { list.error = None; list.loaded = false; }
                this.start_remote_lists();
                this.redraw(panes::Area::Nav, cx);
                cx.notify();
            })));
        let body = div().id(SharedString::from(format!("machines-tray-rows-{key}"))).relative().w_full().flex().flex_col().gap(px(5.))
            .children(self.render_keyring_notice(key, false, cx))
            .when(!m.loaded && m.error.is_none(), |el| el.child(div().p_2().text_xs().text_color(theme::muted()).child(tr("loading"))))
            .when_some(m.error.clone(), |el, error| el.child(div().p(px(10.)).flex().items_center().gap(px(10.)).rounded(px(9.))
                .border_1().border_color(theme::border())
                .child(div().flex_1().min_w_0().text_xs().text_color(theme::warning()).whitespace_normal().child(error))
                .children(retry)))
            .when(empty && m.loaded && m.error.is_none(), |el| el.child(div().p(px(12.)).text_xs().text_color(theme::muted())
                .child(tr(if filtered { "machines_tray_no_match" } else { "empty_sessions" }))))
            .children(rows)
            .map(|el| self.drop_background(el, cx));
        div().id(SharedString::from(format!("machines-tray-machine-{key}"))).w_full().flex().flex_col().gap(px(6.))
            .role(Role::Group).aria_label(m.label.clone())
            .child(header)
            .when(!closed, |el| el.child(body))
            .into_any_element()
    }

    /// `outside_block`: a linha do grupo "Ocultas", sem a faixa do bloco do par.
    fn render_machine_session(&self, session: &SessionInfo, key: &str, invite: bool, outside_block: bool, window: &mut Window,
        cx: &mut Context<Self>) -> AnyElement {
        let target = Target::new(key, &session.name);
        let on = self.selected_target().as_ref() == Some(&target);
        let compact = appearance::get().machines_tray_compact;
        let row_id = format!("machines-tray-row-{}", target.id());
        let focus = window.use_keyed_state(SharedString::from(format!("{row_id}-focus")), cx, |_, cx| cx.focus_handle().tab_stop(true))
            .read(cx).clone();
        let state = session.display_state();
        let readonly = session.read_only();
        let asleep = session.asleep.then(|| tr_shared("session_asleep_badge", &[]));
        let mut tip = format!("{} · {}", session.name, tr(&format!("chip_{state}")));
        if let Some(text) = &asleep { tip.push_str(&format!(" · {text}")); }
        let pick = target.clone();
        let open = target.clone();
        let session_menu = sidebar::session_menu(cx.entity().downgrade(), key.to_owned(), session.clone());
        let (chip_bg, chip_fg) = theme::pill(state);
        let changed = (session.git_added.unwrap_or(0), session.git_removed.unwrap_or(0));
        // A pergunta em aberto vence a última resposta: é o que a pessoa precisa ler para responder.
        let line = session.question.as_deref().filter(|_| state == "awaiting_input")
            .map(|q| (format!("? {}", crate::conversation::one_line(q, 200)), theme::warning()))
            .or_else(|| session.last_reply.as_deref().map(|r| (crate::conversation::one_line(r, 200), theme::muted())))
            .filter(|(text, _)| !text.trim().is_empty() && !compact);
        let row = div().id(SharedString::from(row_id)).relative().w_full().min_h(px(44.))
            .px(px(9.)).py(px(8.)).flex().items_start().gap(px(9.)).rounded(px(8.)).border_1().border_color(theme::border()).cursor_pointer()
            .when(on, |el| el.bg(theme::accent_dim()).border_color(theme::accent()))
            .when(!on, |el| el.hover(|el| el.bg(theme::hover())))
            .track_focus(&focus).when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
            .role(Role::Button).aria_selected(on).aria_label(tip.clone())
            .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
            .child(div().pt(px(1.)).child(chrome::provider_glyph(&session.provider, 15.)))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                .child(div().min_w_0().flex().items_center().gap(px(7.))
                    .child(div().min_w_0().truncate().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).child(session.name.clone()))
                    .child(div().flex_shrink_0().px(px(6.)).rounded_full().bg(chip_bg).text_color(chip_fg).text_size(px(10.5))
                        .child(tr(&format!("chip_{state}"))))
                    .children(asleep.map(|text| div().flex_shrink_0().px(px(6.)).rounded_full().bg(theme::inset()).text_color(theme::muted())
                        .text_size(px(10.5)).child(text)))
                    .when(readonly, |el| el.child(div().flex_shrink_0().text_size(px(9.5)).text_color(theme::muted()).child(tr("machines_tray_read_only"))))
                    .when(session.pending_questions > 0, |el| el.child(div().flex_shrink_0().text_xs().text_color(theme::warning())
                        .child(format!("? {}", session.pending_questions))))
                    .child(div().flex_1())
                    .children(session.last_activity.map(|at| div().flex_shrink_0().text_size(px(10.5)).text_color(theme::faint())
                        .child(super::side::since(at)))))
                // A pasta (quando difere do nome), a branch e o ↑↓ de push e pull; no modo detalhado, as linhas mudadas e o modelo.
                .child(div().min_w_0().flex().flex_wrap().items_center().gap_x(px(8.)).gap_y(px(2.)).text_size(px(10.5)).text_color(theme::muted())
                    .children(folder_name(session).filter(|f| *f != session.name).map(|f| div().min_w_0().truncate().child(f)))
                    .children(shown_branch(session).map(|b| div().min_w_0().truncate().child(format!("⑂ {b}"))))
                    .children(session.git_ahead.filter(|n| *n > 0).map(|n| div().flex_shrink_0().text_color(theme::accent_text()).child(format!("↑{n}"))))
                    .children(session.git_behind.filter(|n| *n > 0).map(|n| div().flex_shrink_0().text_color(theme::warning()).child(format!("↓{n}"))))
                    .when(!compact && (changed.0 > 0 || changed.1 > 0), |el| el.child(div().flex_shrink_0().flex().gap(px(4.))
                        .child(div().text_color(theme::success_text()).child(format!("+{}", changed.0)))
                        .child(div().text_color(theme::danger()).child(format!("−{}", changed.1)))))
                    .when(!compact, |el| el.children(session.model.as_deref().filter(|m| !m.is_empty())
                        .map(|m| div().flex_shrink_0().child(crate::status::model_label(m))))))
                .children(line.map(|(text, color)| div().min_w_0().truncate().text_size(px(11.5)).text_color(color).child(text))))
            .child(div().flex_shrink_0().flex().flex_col().items_center().gap(px(4.))
                .when(!compact, |el| el.children(super::session_tabs::context_pct(session).map(|pct| chrome::ring_sized(Some(pct), 20., 18., 2.))))
                .child(chrome::icon_button(SharedString::from(format!("machines-tray-menu-{}", target.id())), IconName::Ellipsis,
                        tr("sidebar_options").replace("{n}", &session.name), cx).size(px(20.))
                    .dropdown_menu_with_anchor(Anchor::TopRight,
                        sidebar::session_menu(cx.entity().downgrade(), key.to_owned(), session.clone()))))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.close_popups();
                this.open_target(&pick, window, cx);
                cx.notify();
            }))
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                if !matches!(event.keystroke.key.as_str(), "enter" | "space") { return; }
                this.close_popups();
                this.open_target(&open, window, cx);
                cx.stop_propagation();
                cx.notify();
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, _, _, cx| this.start_menu(target.clone(), cx)))
            .when(!invite && !readonly, |row| self.group_row(row, session, (!self.is_active_key(key)).then_some(key), 8., outside_block, cx));
        row.context_menu(session_menu).into_any_element()
    }
}

struct Machine { key: String, label: String, invite: bool, loaded: bool, online: bool, error: Option<String> }

fn machine_dot(m: &Machine) -> Hsla {
    if m.online { theme::success() } else if m.error.is_some() { theme::faint() } else { theme::warning() }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Filter { All, Working, Waiting, Hidden }

impl Filter {
    fn keeps(self, s: &SessionInfo) -> bool {
        match self {
            // As ocultas são o grupo de baixo de cada máquina: quem as separa é o `tray_view`.
            Filter::All | Filter::Hidden => true,
            Filter::Working => s.display_state() == "working",
            Filter::Waiting => s.display_state() == "awaiting_input" || s.pending_questions > 0,
        }
    }
}

fn state_rank(s: &SessionInfo) -> u8 {
    if Filter::Waiting.keeps(s) { 0 } else if Filter::Working.keeps(s) { 1 } else { 2 }
}

/// Busca por nome, pasta ou branch; `query` já vem em minúsculas.
fn matches_query(s: &SessionInfo, query: &str) -> bool {
    query.is_empty() || [Some(s.name.clone()), folder_name(s), shown_branch(s)].into_iter().flatten()
        .any(|text| text.to_lowercase().contains(query))
}

/// Uma máquina da bandeja, por nome: as da lista e, à parte, as ocultas. A oculta aberta fica com as da lista, como nas abas; a
/// que está sendo fechada (`closing`) sai das duas.
fn tray_columns<'a>(sessions: &'a [SessionInfo], key: &str, open: Option<&Target>, closing: impl Fn(&str) -> bool)
    -> (Vec<&'a SessionInfo>, Vec<&'a SessionInfo>) {
    let mut sorted: Vec<&SessionInfo> = sessions.iter().filter(|s| !closing(&s.name)).collect();
    sorted.sort_by_cached_key(|s| sidebar::text_key(&s.name));
    sorted.into_iter().partition(|s| sidebar::listed_on(s, &Target::new(key, &s.name), open))
}

#[cfg(test)]
mod tests {
    use super::{Filter, SessionInfo, Target, matches_query, state_rank, tray_columns};

    #[test]
    fn tray_column_keeps_hidden_sessions_apart_and_the_open_one_listed() {
        let session = |name: &str, hidden: bool| SessionInfo { name: name.into(), hidden_from_list: hidden, ..Default::default() };
        let list = vec![session("zeta", true), session("api", false), session("Gama", true), session("beta", true), session("gone", true)];
        let names = |v: Vec<&SessionInfo>| v.into_iter().map(|s| s.name.clone()).collect::<Vec<_>>();
        let (shown, hidden) = tray_columns(&list, "nb", None, |name| name == "gone");
        assert_eq!(names(shown), ["api"]);
        assert_eq!(names(hidden), ["beta", "Gama", "zeta"], "a mesma ordem da barra lateral, sem caixa");
        let open = Target::new("nb", "zeta");
        let (shown, hidden) = tray_columns(&list, "nb", Some(&open), |_| false);
        assert_eq!(names(shown), ["api", "zeta"], "a oculta aberta fica na lista");
        assert_eq!(names(hidden), ["beta", "Gama", "gone"]);
        let only_hidden = vec![session("x", true)];
        assert_eq!(names(tray_columns(&only_hidden, "dp", None, |_| false).1), ["x"], "máquina só com ocultas ainda tem o que mostrar");
    }

    #[test]
    fn tray_filters_and_search_match_state_name_and_branch() {
        let session = |name: &str, state: &str, branch: Option<&str>, questions: u32| SessionInfo { name: name.into(), state: state.into(),
            branch: branch.map(Into::into), pending_questions: questions, ..Default::default() };
        let busy = session("hangar", "working", Some("main"), 0);
        let asking = session("onboarding", "idle", Some("feat/tour"), 1);
        let idle = session("conversa", "idle", None, 0);
        assert!(Filter::Working.keeps(&busy) && !Filter::Working.keeps(&idle));
        assert!(Filter::Waiting.keeps(&asking), "pergunta pendente conta como aguardando");
        assert!(state_rank(&asking) < state_rank(&busy) && state_rank(&busy) < state_rank(&idle));
        assert!(matches_query(&asking, "tour") && matches_query(&busy, "hang") && !matches_query(&idle, "main"));
        assert!(matches_query(&idle, ""));
    }
}
