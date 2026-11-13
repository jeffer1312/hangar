//! Mission Control: os tickets dos conectores do servidor ativo em três colunas, cada um com a sessão
//! aberta para ele. O vínculo ticket → sessão mora no servidor (`/api/connectors/{id}/links`).
use super::*;
use super::costs::{empty_state, error_state, loading_state, note_box, page_frame};
use super::linked_children::{state_label, tone};
use gpui_kit::component::progress::Progress;
use serde::Deserialize;
use std::collections::BTreeMap;

const PAGE: &str = "60";

#[derive(Clone, Deserialize)]
struct Connector {
    id: String,
    name: String,
    enabled: bool,
    #[serde(default)]
    capabilities: BTreeMap<String, u32>,
    #[serde(default)]
    links: BTreeMap<String, String>,
}

#[derive(Clone, Deserialize)]
struct Status { name: String, category: String }

#[derive(Clone, Deserialize)]
struct Person { name: String }

#[derive(Clone, Deserialize)]
struct Ticket {
    id: String,
    key: String,
    title: String,
    url: String,
    status: Status,
    assignee: Option<Person>,
    updated_at: Option<String>,
}

#[derive(Deserialize)]
struct TicketPage { items: Vec<Ticket>, next_cursor: Option<String>, total: Option<u64> }

#[derive(Deserialize)]
struct Detail {
    #[serde(flatten)]
    ticket: Ticket,
    description_markdown: String,
    start_prompt: Option<String>,
}

struct DetailView { detail: Detail, view: Entity<TextViewState> }

struct Board {
    connectors: Vec<Connector>,
    items: Vec<Ticket>,
    next_cursor: Option<String>,
    total: Option<u64>,
    /// Busca que gerou a página: o cursor só vale com ela.
    query: String,
}

/// Pedido de Iniciar à espera da sessão nova: só a criação seguinte, com a mensagem que começa pelo texto
/// posto no campo, liga o ticket.
struct PendingStart { server: String, connector: String, ticket: String, prompt: String }

#[derive(Default)]
pub(super) struct Mission {
    pub(super) open: bool,
    /// Servidor de onde veio o que está na tela.
    server: Option<String>,
    connector: Option<String>,
    board: Option<Result<Board, String>>,
    loading: bool,
    more: bool,
    seq: u64,
    selected: Option<String>,
    detail: Option<Result<DetailView, String>>,
    detail_seq: u64,
    busy: bool,
    pending: Option<PendingStart>,
    query: Option<Entity<InputState>>,
    _query_events: Option<Subscription>,
}

/// Coluna do ticket: concluído pelo sistema de origem vence; sessão viva põe em andamento.
fn column(ticket: &Ticket, session: Option<&SessionInfo>) -> usize {
    match ticket.status.category.as_str() {
        "done" | "canceled" => 2,
        _ if session.is_some_and(|s| s.state != "dead") => 1,
        "in_progress" => 1,
        _ => 0,
    }
}

fn failure_text(error: &Failure) -> String {
    match error.code.as_deref() {
        Some("auth_required" | "permission_denied") => tr("mission_auth_required"),
        Some("connectors_owner_only") => tr("mission_owner_only"),
        _ if error.status.is_some() && !error.detail.is_empty() => error.detail.clone(),
        _ => Hangar::fetch_failure(error),
    }
}

fn updated(at: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some(super::side::ago(super::side::now_seconds() - at.timestamp() as f64))
}

impl Hangar {
    pub(super) fn toggle_mission(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.mission.open { self.close_mission(window, cx) } else { self.open_mission(window, cx) }
    }

    pub(super) fn open_mission(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_dialog { return; }
        self.close_settings(window, cx);
        self.close_costs(window, cx);
        self.close_worktrees(window, cx);
        self.close_reviews(window, cx);
        self.close_popups();
        self.command_panel = false;
        self.mission.open = true;
        if self.mission.query.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(tr("mission_search")).clean_on_escape());
            self.mission._query_events = Some(cx.subscribe_in(&input, window, |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) { this.load_mission(false, window, cx); }
            }));
            self.mission.query = Some(input);
        }
        self.root_focus.focus(window, cx);
        self.load_mission(false, window, cx);
        cx.notify();
    }

    pub(super) fn close_mission(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if std::mem::take(&mut self.mission.open) {
            self.root_focus.focus(window, cx);
            cx.notify();
        }
    }

    fn mission_query(&self, cx: &App) -> String {
        self.mission.query.as_ref().map(|input| input.read(cx).value().trim().to_owned()).unwrap_or_default()
    }

    /// Lê o catálogo e a primeira página (ou a seguinte, com `more`) do conector escolhido.
    fn load_mission(&mut self, more: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let server = api.identity();
        let same = self.mission.server.as_deref() == Some(server.as_str());
        let page = more.then(|| self.mission.board.as_ref().and_then(|b| b.as_ref().ok())
            .and_then(|b| Some((b.next_cursor.clone()?, b.query.clone())))).flatten();
        if more && page.is_none() { return; }
        let (cursor, query) = match page { Some((cursor, query)) => (Some(cursor), query), None => (None, self.mission_query(cx)) };
        if !same { (self.mission.connector, self.mission.selected, self.mission.detail, self.mission.board) = (None, None, None, None); }
        self.mission.seq += 1;
        let seq = self.mission.seq;
        (self.mission.loading, self.mission.more, self.mission.server) = (!more, more, Some(server));
        let wanted = self.mission.connector.clone();
        let used = query.clone();
        let request = async move {
            let catalog = api.server_read(&["connectors"], &[], 15).await?;
            let connectors: Vec<Connector> = serde_json::from_value(catalog["connectors"].clone()).map_err(|_| Failure::local("invalid_response"))?;
            let usable = |c: &&Connector| c.enabled && c.capabilities.contains_key("work_tracking");
            let chosen = wanted.filter(|id| connectors.iter().filter(usable).any(|c| &c.id == id))
                .or_else(|| connectors.iter().find(usable).map(|c| c.id.clone()));
            let Some(id) = chosen else { return Ok((connectors, None)) };
            let mut params = vec![("assignee", "me".to_owned()), ("limit", PAGE.to_owned())];
            if !query.is_empty() { params.push(("query", query)); }
            if let Some(cursor) = cursor { params.push(("cursor", cursor)); }
            let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let page = api.server_read(&["connectors", &id, "tickets"], &params, 45).await?;
            let page: TicketPage = serde_json::from_value(page).map_err(|_| Failure::local("invalid_response"))?;
            Ok((connectors, Some((id, page))))
        };
        run(&self.runtime, request, window, cx, move |this: &mut Hangar, result, window, cx| {
            if this.mission.seq != seq { return; }
            (this.mission.loading, this.mission.more) = (false, false);
            match result {
                // A falha do "Carregar mais" não apaga o que já está na tela.
                Err(error) if more => window.push_notification(Notification::error(failure_text(&error)), cx),
                Err(error) => this.mission.board = Some(Err(failure_text(&error))),
                Ok((connectors, None)) => {
                    this.mission.connector = None;
                    this.mission.board = Some(Ok(Board { connectors, items: Vec::new(), next_cursor: None, total: None, query: used }));
                }
                Ok((connectors, Some((id, page)))) => {
                    let mut items = page.items;
                    if more && let Some(Ok(board)) = this.mission.board.take() {
                        let seen: HashSet<String> = board.items.iter().map(|t| t.id.clone()).collect();
                        items.retain(|t| !seen.contains(&t.id));
                        items = board.items.into_iter().chain(items).collect();
                    }
                    this.mission.connector = Some(id);
                    this.mission.board = Some(Ok(Board { connectors, items, next_cursor: page.next_cursor, total: page.total, query: used }));
                }
            }
        });
        cx.notify();
    }

    fn choose_connector(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        (self.mission.connector, self.mission.selected, self.mission.detail) = (Some(id), None, None);
        self.load_mission(false, window, cx);
    }

    fn select_ticket(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(connector)) = (self.api.clone(), self.mission.connector.clone()) else { return };
        (self.mission.selected, self.mission.detail) = (Some(id.clone()), None);
        self.mission.detail_seq += 1;
        let seq = self.mission.detail_seq;
        let request = async move {
            let value = api.server_read(&["connectors", &connector, "tickets", &id], &[], 45).await?;
            serde_json::from_value::<Detail>(value).map_err(|_| Failure::local("invalid_response"))
        };
        run(&self.runtime, request, window, cx, move |this: &mut Hangar, result, _, cx| {
            if this.mission.detail_seq != seq { return; }
            this.mission.detail = Some(result.map_err(|error| failure_text(&error)).map(|detail| {
                let source = safe_markdown(&detail.description_markdown);
                let view = cx.new(|cx| TextViewState::markdown(&source, cx));
                DetailView { detail, view }
            }));
        });
        cx.notify();
    }

    fn linked_session(&self, ticket: &str) -> Option<(&str, Option<&SessionInfo>)> {
        let board = self.mission.board.as_ref()?.as_ref().ok()?;
        let connector = board.connectors.iter().find(|c| Some(&c.id) == self.mission.connector.as_ref())?;
        let name = connector.links.get(ticket)?;
        Some((name.as_str(), self.sessions.iter().find(|s| &s.name == name)))
    }

    /// Iniciar: a tela de nova conversa com a primeira mensagem do conector no campo. A pessoa escolhe a pasta e envia;
    /// a sessão que nascer com essa mensagem fica ligada ao ticket.
    fn start_ticket(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(Ok(view)), Some(connector), Some(server)) = (&self.mission.detail, self.mission.connector.clone(), self.mission.server.clone())
            else { return };
        let ticket = &view.detail.ticket;
        let prompt = view.detail.start_prompt.clone()
            .unwrap_or_else(|| tr("mission_default_prompt").replace("{key}", &ticket.key).replace("{title}", &ticket.title).replace("{url}", &ticket.url));
        let ticket = ticket.id.clone();
        if self.create_blocked(window, cx) { return; }
        self.close_mission(window, cx);
        self.go_home(window, cx);
        // O rascunho que já estava na tela de nova conversa fica, depois do pedido do ticket.
        let draft = self.composer.read(cx).value().trim().to_owned();
        let text = if draft.is_empty() { prompt.clone() } else { format!("{prompt}\n\n{draft}") };
        self.composer.update(cx, |input, cx| { input.set_value(text, window, cx); input.focus(window, cx); });
        self.mission.pending = Some(PendingStart { server, connector, ticket, prompt });
        window.push_notification(Notification::info(tr("mission_start_hint")), cx);
    }

    /// Volta da criação: a sessão nascida do Iniciar, com a mensagem que cita o ticket, é ligada a ele no servidor.
    pub(super) fn mission_session_created(&mut self, server: &str, session: &str, first: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        // Qualquer criação encerra o pedido: só a primeira depois do Iniciar pode ser a dele.
        let Some(start) = self.mission.pending.take() else { return };
        if start.server != server || !first.is_some_and(|text| text.trim_start().starts_with(start.prompt.trim())) { return; }
        let Some(api) = self.api_for(server) else { return };
        let body = json!({"session": session});
        let request = async move {
            api.server_send(reqwest::Method::PUT, &["connectors", &start.connector, "links", &start.ticket], Some(body), 15).await
        };
        run(&self.runtime, request, window, cx, |_: &mut Hangar, result, window, cx| {
            if let Err(error) = result {
                window.push_notification(Notification::warning(tr("mission_link_failed").replace("{erro}", &failure_text(&error))), cx);
            }
        });
    }

    fn unlink_ticket(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(connector), Some(ticket)) = (self.api.clone(), self.mission.connector.clone(), self.mission.selected.clone()) else { return };
        self.mission.busy = true;
        let request = async move { api.server_send(reqwest::Method::DELETE, &["connectors", &connector, "links", &ticket], None, 15).await };
        run(&self.runtime, request, window, cx, |this: &mut Hangar, result, window, cx| {
            this.mission.busy = false;
            match result {
                Ok(_) => this.load_mission(false, window, cx),
                Err(error) => window.push_notification(Notification::error(failure_text(&error)), cx),
            }
        });
        cx.notify();
    }

    fn open_linked(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.iter().find(|s| s.name == name).cloned() else { return };
        let Some(server) = self.mission.server.clone() else { return };
        self.close_mission(window, cx);
        self.select_on(&servers::norm(&server), session, window, cx);
    }

    // ── Desenho ─────────────────────────────────────────────────────────────

    pub(super) fn render_mission(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Outro servidor ativo: o que está na tela era do anterior.
        if self.api.as_ref().map(|api| api.identity()) != self.mission.server {
            self.load_mission(false, window, cx);
        }
        let loading = self.mission.loading;
        let search = self.mission.query.clone().map(|input| Input::new(&input).small().w(px(260.)).cleanable(true)
            .aria_label(tr("mission_search")).prefix(chrome::small_icon(IconName::Search, 13., theme::faint())).into_any_element());
        let chooser = self.render_mission_connectors(cx);
        let header = self.costs_header(tr("mission_title"), |this, window, cx| this.close_mission(window, cx),
            search.into_iter().chain(chooser).chain([
                Button::new("mission-refresh").outline().small().label(tr_shared("custos_atualizar", &[])).loading(loading).disabled(loading)
                    .on_click(cx.listener(|this, _, window, cx| this.load_mission(false, window, cx))).into_any_element(),
            ]).collect(), cx);
        let body = self.render_mission_body(cx);
        let panel = self.mission.selected.clone().map(|id| self.render_mission_detail(&id, cx));
        page_frame(div().size_full().flex().flex_col().child(header)
            .child(div().flex_1().min_h_0().flex().child(body).children(panel)))
    }

    fn render_mission_connectors(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let board = self.mission.board.as_ref()?.as_ref().ok()?;
        let usable: Vec<&Connector> = board.connectors.iter().filter(|c| c.enabled && c.capabilities.contains_key("work_tracking")).collect();
        if usable.len() < 2 { return None; }
        Some(div().flex().gap(px(4.)).children(usable.into_iter().map(|c| {
            let id = c.id.clone();
            Button::new(SharedString::from(format!("mission-connector-{}", c.id))).small().ghost().label(c.name.clone())
                .selected(Some(&c.id) == self.mission.connector.as_ref())
                .on_click(cx.listener(move |this, _, window, cx| this.choose_connector(id.clone(), window, cx)))
        })).into_any_element())
    }

    fn render_mission_body(&self, cx: &mut Context<Self>) -> AnyElement {
        let frame = |content: AnyElement| div().flex_1().min_w_0().h_full().px(px(20.)).pt(px(16.)).pb(px(12.)).flex().flex_col().gap(px(12.))
            .child(content).into_any_element();
        let board = match &self.mission.board {
            None => return frame(loading_state().into_any_element()),
            Some(Err(error)) => return frame(error_state(error.clone(), cx.listener(|this, _, window, cx| this.load_mission(false, window, cx))).into_any_element()),
            Some(Ok(board)) => board,
        };
        if self.mission.connector.is_none() {
            let text = if board.connectors.is_empty() { tr("mission_no_connectors") } else { tr("mission_no_usable") };
            return frame(div().flex().flex_col().gap(px(10.)).pt(px(24.)).items_center()
                .child(chrome::small_icon(IconName::LayoutDashboard, 28., theme::faint()))
                .child(empty_state(text)).into_any_element());
        }
        let mut columns: [Vec<&Ticket>; 3] = Default::default();
        for ticket in &board.items {
            let session = self.linked_session(&ticket.id).and_then(|(_, s)| s);
            columns[column(ticket, session)].push(ticket);
        }
        let titles = [tr("mission_todo"), tr("mission_doing"), tr("mission_done")];
        let lanes = div().flex_1().min_h_0().flex().gap(px(12.)).children(columns.iter().zip(titles).enumerate().map(|(n, (items, title))| {
            let cards: Vec<AnyElement> = items.iter().map(|ticket| self.render_mission_card(ticket, cx)).collect();
            let empty = cards.is_empty();
            div().flex_1().min_w(px(220.)).h_full().flex().flex_col().rounded(px(12.)).bg(theme::inset())
                .child(div().px(px(12.)).py(px(10.)).flex().items_center().gap(px(8.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().text_size(px(12.)).text_color(theme::faint()).child(items.len().to_string())))
                .child(div().id(SharedString::from(format!("mission-lane-{n}"))).flex_1().min_h_0().overflow_y_scroll().px(px(8.)).pb(px(8.))
                    .flex().flex_col().gap(px(8.))
                    .when(empty, |el| el.child(empty_state(tr("mission_empty_column"))))
                    .children(cards))
        }));
        let shown = board.items.len().to_string();
        let total = board.total.map(|t| t.to_string()).unwrap_or_else(|| shown.clone());
        let footer = div().flex().items_center().gap(px(10.)).text_size(px(12.)).text_color(theme::muted())
            .child(tr("mission_shown").replace("{n}", &shown).replace("{total}", &total))
            .when(board.next_cursor.is_some(), |el| el.child(Button::new("mission-more").ghost().small().label(tr("mission_more"))
                .loading(self.mission.more).disabled(self.mission.more)
                .on_click(cx.listener(|this, _, window, cx| this.load_mission(true, window, cx)))));
        frame(div().flex_1().min_h_0().flex().flex_col().gap(px(10.)).child(lanes).child(footer).into_any_element())
    }

    fn render_mission_card(&self, ticket: &Ticket, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.mission.selected.as_deref() == Some(ticket.id.as_str());
        let link = self.linked_session(&ticket.id);
        let id = ticket.id.clone();
        let session = link.map(|(name, session)| match session {
            Some(session) => session_line(session),
            None => div().text_size(px(11.5)).text_color(theme::faint()).truncate().child(tr("mission_session_gone").replace("{name}", name)),
        });
        div().id(SharedString::from(format!("mission-card-{}", ticket.id))).w_full().p(px(10.)).rounded(px(10.)).border_1()
            .border_color(if selected { theme::accent() } else { theme::border() }).bg(theme::boxed()).cursor_pointer()
            .hover(|s| s.border_color(theme::accent().alpha(0.5)))
            .flex().flex_col().gap(px(6.))
            .child(div().flex().items_center().gap(px(6.))
                .child(div().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted()).child(ticket.key.clone()))
                .child(div().flex_1())
                .child(div().px(px(6.)).py(px(1.)).rounded(px(6.)).bg(theme::inset()).text_size(px(11.)).text_color(theme::muted()).truncate()
                    .child(ticket.status.name.clone())))
            .child(div().text_size(px(13.)).line_height(relative(1.35)).whitespace_normal().line_clamp(3).child(ticket.title.clone()))
            .children(session)
            .on_click(cx.listener(move |this, _, window, cx| this.select_ticket(id.clone(), window, cx)))
            .into_any_element()
    }

    fn render_mission_detail(&self, id: &str, cx: &mut Context<Self>) -> Stateful<Div> {
        let close = Button::new("mission-detail-close").ghost().small().icon(IconName::Close).accessibility_label(tr("close"))
            .on_click(cx.listener(|this, _, _, cx| { (this.mission.selected, this.mission.detail) = (None, None); cx.notify(); }));
        let panel = div().id("mission-detail").w(px(460.)).flex_shrink_0().h_full().overflow_y_scroll().border_l_1().border_color(theme::border())
            .p(px(16.)).flex().flex_col().gap(px(14.)).text_size(px(13.)).whitespace_normal();
        let view = match &self.mission.detail {
            None => return panel.child(div().flex().justify_end().child(close)).child(loading_state()),
            Some(Err(error)) => return panel.child(div().flex().justify_end().child(close)).child(note_box(error.clone(), theme::warning())),
            Some(Ok(view)) => view,
        };
        let ticket = &view.detail.ticket;
        let link = self.linked_session(id);
        let meta = [Some(ticket.status.name.clone()), ticket.assignee.as_ref().map(|a| a.name.clone()),
            ticket.updated_at.as_deref().and_then(updated)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let url = ticket.url.clone();
        let mut actions = div().flex().flex_wrap().gap(px(8.));
        match link {
            Some((name, Some(_))) => {
                let name = name.to_owned();
                actions = actions.child(Button::new("mission-open-session").primary().small().icon(IconName::MessageSquare).label(tr("mission_open_session"))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_linked(name.clone(), window, cx))));
            }
            _ => {
                actions = actions.child(Button::new("mission-start").primary().small().icon(IconName::Play).label(tr("mission_start"))
                    .on_click(cx.listener(|this, _, window, cx| this.start_ticket(window, cx))));
            }
        }
        actions = actions.child(Button::new("mission-source").outline().small().icon(IconName::ExternalLink).label(tr("mission_open_source"))
            .on_click(move |_, window, cx| super::links::open_link(&url, true, window, cx)));
        if link.is_some() {
            actions = actions.child(Button::new("mission-unlink").ghost().small().label(tr("mission_unlink")).loading(self.mission.busy)
                .disabled(self.mission.busy).on_click(cx.listener(|this, _, window, cx| this.unlink_ticket(window, cx))));
        }
        let session = link.map(|(name, session)| match session {
            Some(session) => div().p(px(10.)).rounded(px(10.)).bg(theme::inset()).flex().flex_col().gap(px(4.))
                .child(div().text_size(px(12.)).text_color(theme::muted()).child(tr("mission_session").replace("{name}", name)))
                .child(session_line(session)),
            None => note_box(tr("mission_session_gone").replace("{name}", name), theme::muted()),
        });
        let description = if view.detail.description_markdown.trim().is_empty() {
            div().text_color(theme::faint()).child(tr("mission_no_description")).into_any_element()
        } else {
            div().child(TextView::new(&view.view).selectable(true).scrollable(false)).into_any_element()
        };
        panel
            .child(div().flex().items_start().gap(px(8.))
                .child(div().flex_1().min_w_0().flex().flex_col().gap(px(4.))
                    .child(div().font_family(theme::MONO).text_size(px(12.)).text_color(theme::muted()).child(ticket.key.clone()))
                    .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(ticket.title.clone()))
                    .child(div().text_size(px(12.)).text_color(theme::muted()).child(meta)))
                .child(close))
            .child(actions)
            .children(session)
            .child(div().border_t_1().border_color(theme::border()).pt(px(12.)).child(description))
    }
}

/// Estado da sessão ligada e, com plano, a Task em andamento e a barra do que já foi feito.
fn session_line(session: &SessionInfo) -> Div {
    let tone = tone(session.display_state());
    let mut line = div().flex().flex_col().gap(px(4.))
        .child(div().flex().items_center().gap(px(6.)).text_size(px(11.5)).min_w_0()
            .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(theme::status(tone)))
            .child(div().text_color(theme::muted()).child(state_label(tone)))
            .child(div().text_color(theme::faint()).truncate().child(session.name.clone())));
    if let (Some(task), Some(total)) = (session.plan_task, session.plan_task_total) {
        line = line.child(div().text_size(px(11.5)).text_color(theme::faint())
            .child(tr("mission_plan_task").replace("{n}", &task.to_string()).replace("{total}", &total.to_string())));
    }
    if let (Some(done), Some(total)) = (session.plan_done, session.plan_total.filter(|t| *t > 0)) {
        line = line.child(Progress::new(SharedString::from(format!("mission-plan-{}", session.name))).xsmall()
            .accessibility_label(tr("mission_plan_progress")).value((done as f32 * 100. / total as f32).min(100.)));
    }
    line
}
