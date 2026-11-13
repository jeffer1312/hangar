//! Mission Control: os tickets dos conectores do servidor ativo em colunas por etapa do trabalho, cada um com a
//! sessão que o trabalha, o portão do dono, as evidências de validação e o histórico (`/api/mission`).
use super::*;
use super::costs::{empty_state, error_state, loading_state, note_box, page_frame};
use super::device::size_text;
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

#[derive(Clone, Deserialize)]
struct Stage {
    id: String,
    name: String,
    #[serde(default)]
    gate: bool,
    #[serde(default)]
    statuses: Vec<String>,
}

#[derive(Clone, Deserialize)]
struct Evidence { id: String, name: String, mime: String, size: u64, #[serde(default)] caption: String, by: String }

#[derive(Clone, Deserialize)]
struct Event { at: String, kind: String, by: String, stage: Option<String>, note: Option<String> }

#[derive(Clone, Deserialize)]
struct Item {
    connector: String,
    ticket_id: String,
    session: Option<String>,
    stage: Option<String>,
    #[serde(default)]
    approved: Vec<String>,
    #[serde(default)]
    history: Vec<Event>,
    #[serde(default)]
    evidence: Vec<Evidence>,
    delivered_at: Option<String>,
}

#[derive(Clone, Deserialize)]
struct Work { stages: Vec<Stage>, items: Vec<Item> }

struct DetailView { detail: Detail, view: Entity<TextViewState> }

struct Board {
    connectors: Vec<Connector>,
    work: Work,
    tickets: Vec<Ticket>,
    next_cursor: Option<String>,
    total: Option<u64>,
    /// Busca que gerou a página: o cursor só vale com ela.
    query: String,
}

/// Pedido de Iniciar à espera da sessão nova: só a criação seguinte, com a mensagem que começa pelo texto
/// posto no campo, liga o ticket.
struct PendingStart { server: String, connector: String, ticket: Ticket, prompt: String }

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lane { Inbox, Stage(usize), Done }

/// De onde veio a etapa do ticket, para a tela dizer.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source { Reported, Status, Plan, None }

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
    /// Releituras das etapas e registros: a resposta mais nova vence.
    work_seq: u64,
    busy: bool,
    pending: Option<PendingStart>,
    query: Option<Entity<InputState>>,
    _query_events: Option<Subscription>,
    /// Miniaturas das evidências de imagem: `None` enquanto chega ou quando não decodificou.
    thumbs: HashMap<String, Option<Arc<RenderImage>>>,
    /// Campo do "Pedir ajustes", aberto no detalhe.
    changes: Option<Entity<TextareaState>>,
    /// Editor das etapas, aberto no lugar das colunas.
    stages_editor: Option<Entity<TextareaState>>,
}

fn failure_text(error: &Failure) -> String {
    match error.code.as_deref() {
        Some("auth_required" | "permission_denied") => tr("mission_auth_required"),
        Some("connectors_owner_only" | "mission_owner_only") => tr("mission_owner_only"),
        _ if error.status.is_some() && !error.detail.is_empty() => error.detail.clone(),
        _ => Hangar::fetch_failure(error),
    }
}

fn ago(at: &str) -> Option<String> {
    let at = chrono::DateTime::parse_from_rfc3339(at).ok()?;
    Some(super::side::ago(super::side::now_seconds() - at.timestamp() as f64))
}

/// A sessão é da PM quando a branch é a chave ou o nome começa por ela (`PM-1`, `PM-1-Coordenadora`), como o
/// plugin Promedico identifica a PM; `PM-1` não casa com `PM-12`.
fn named_after(session: &SessionInfo, key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    let starts = |text: &str| {
        let text = text.to_ascii_lowercase();
        text.strip_prefix(&key).is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '_', '.', ' ']))
    };
    session.branch.as_deref().is_some_and(|branch| branch.eq_ignore_ascii_case(&key)) || starts(&session.name)
}

/// Texto das etapas para editar: uma por linha, `Nome | portão | Status, Status`.
fn stages_text(stages: &[Stage]) -> String {
    stages.iter().map(|stage| {
        let gate = if stage.gate { tr("mission_gate_word") } else { String::new() };
        let mut line = stage.name.clone();
        if stage.gate || !stage.statuses.is_empty() { line.push_str(&format!(" | {gate}")); }
        if !stage.statuses.is_empty() { line.push_str(&format!(" | {}", stage.statuses.join(", "))); }
        line
    }).collect::<Vec<_>>().join("\n")
}

/// `id` de etapa a partir do nome: a mesma dobra de acentos das worktrees, começando por letra e com até 32 bytes.
fn slug(name: &str) -> String {
    let mut id = super::worktrees::slug(name);
    if !id.starts_with(|c: char| c.is_ascii_lowercase()) { id = format!("e-{id}"); }
    id.chars().take(32).collect::<String>().trim_end_matches('-').to_owned()
}

/// Lê o texto do editor; o nome igual ao de uma etapa existente mantém o `id` dela (e o que está nos tickets).
fn parse_stages(text: &str, current: &[Stage]) -> Result<Vec<Value>, String> {
    let gate_words = ["portão", "portao", "gate"];
    let mut used = HashSet::new();
    let mut stages = Vec::new();
    for line in text.lines().map(str::trim).filter(|line| !line.is_empty()) {
        let mut parts = line.split('|').map(str::trim);
        let name = parts.next().unwrap_or_default().to_owned();
        if name.is_empty() { return Err(tr("mission_stages_empty_name")); }
        let gate = parts.next().is_some_and(|flag| gate_words.iter().any(|word| flag.eq_ignore_ascii_case(word)));
        let statuses: Vec<String> = parts.next().map(|list| list.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect())
            .unwrap_or_default();
        let base = current.iter().find(|stage| stage.name.eq_ignore_ascii_case(&name)).map(|stage| stage.id.clone()).unwrap_or_else(|| slug(&name));
        let mut id = base.clone();
        let mut n = 2;
        while !used.insert(id.clone()) { id = format!("{}-{n}", base.chars().take(29).collect::<String>()); n += 1; }
        stages.push(json!({"id": id, "name": name, "gate": gate, "statuses": statuses}));
    }
    if stages.is_empty() { return Err(tr("mission_stages_none")); }
    Ok(stages)
}

struct Linked<'a> { name: &'a str, session: Option<&'a SessionInfo>, stored: bool }

struct TicketView<'a> {
    item: Option<&'a Item>,
    linked: Option<Linked<'a>>,
    lane: Lane,
    source: Source,
    /// Etapa da coluna, quando é uma etapa (não Entrada nem Concluído).
    stage: Option<&'a Stage>,
    /// Etapa com portão em que o ticket espera o dono.
    waiting: Option<&'a Stage>,
}

/// Coluna do ticket e de onde veio: concluído na origem ou entregue vence; depois a etapa informada, a que o status de
/// origem indica e, com sessão viva, a dedução pelo plano dela.
fn lane_of(stages: &[Stage], ticket: &Ticket, item: Option<&Item>, session: Option<&SessionInfo>) -> (Lane, Source) {
    if matches!(ticket.status.category.as_str(), "done" | "canceled") || item.is_some_and(|i| i.delivered_at.is_some()) {
        return (Lane::Done, Source::Status);
    }
    if let Some(index) = item.and_then(|i| i.stage.as_deref()).and_then(|id| stages.iter().position(|s| s.id == id)) {
        return (Lane::Stage(index), Source::Reported);
    }
    if let Some(index) = stages.iter().position(|s| s.statuses.iter().any(|status| status.eq_ignore_ascii_case(&ticket.status.name))) {
        return (Lane::Stage(index), Source::Status);
    }
    let Some(session) = session.filter(|s| s.state != "dead") else { return (Lane::Inbox, Source::None) };
    // Sem plano, plano em andamento e plano concluído: as etapas padrão, ou a posição delas numa lista própria.
    let (id, position) = match (session.plan_done, session.plan_total.filter(|t| *t > 0)) {
        (Some(done), Some(total)) if done >= total => ("revisao", 2),
        (_, Some(_)) => ("execucao", 1),
        _ => ("planejamento", 0),
    };
    let index = stages.iter().position(|s| s.id == id).unwrap_or(position.min(stages.len().saturating_sub(1)));
    if stages.is_empty() { (Lane::Inbox, Source::None) } else { (Lane::Stage(index), Source::Plan) }
}

impl Board {
    fn item(&self, connector: &str, ticket: &str) -> Option<&Item> {
        self.work.items.iter().find(|item| item.connector == connector && item.ticket_id == ticket)
    }
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
            (self.mission.changes, self.mission.stages_editor) = (None, None);
            self.root_focus.focus(window, cx);
            cx.notify();
        }
    }

    fn mission_query(&self, cx: &App) -> String {
        self.mission.query.as_ref().map(|input| input.read(cx).value().trim().to_owned()).unwrap_or_default()
    }

    fn mission_board(&self) -> Option<&Board> { self.mission.board.as_ref()?.as_ref().ok() }

    /// Lê o catálogo, as etapas com os registros e a primeira página (ou a seguinte, com `more`) do conector escolhido.
    fn load_mission(&mut self, more: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let server = api.identity();
        let same = self.mission.server.as_deref() == Some(server.as_str());
        let page = more.then(|| self.mission_board().and_then(|b| Some((b.next_cursor.clone()?, b.query.clone())))).flatten();
        if more && page.is_none() { return; }
        let (cursor, query) = match page { Some((cursor, query)) => (Some(cursor), query), None => (None, self.mission_query(cx)) };
        if !same {
            (self.mission.connector, self.mission.selected, self.mission.detail, self.mission.board) = (None, None, None, None);
            for image in self.mission.thumbs.drain().filter_map(|(_, image)| image) { cx.drop_image(image, Some(window)); }
        }
        self.mission.seq += 1;
        let seq = self.mission.seq;
        (self.mission.loading, self.mission.more, self.mission.server) = (!more, more, Some(server));
        let wanted = self.mission.connector.clone();
        let used = query.clone();
        let request = async move {
            let (catalog, work) = tokio::join!(api.server_read(&["connectors"], &[], 15), api.server_read(&["mission"], &[], 15));
            let connectors: Vec<Connector> = serde_json::from_value(catalog?["connectors"].clone()).map_err(|_| Failure::local("invalid_response"))?;
            let work: Work = serde_json::from_value(work?).map_err(|_| Failure::local("invalid_response"))?;
            let usable = |c: &&Connector| c.enabled && c.capabilities.contains_key("work_tracking");
            let chosen = wanted.filter(|id| connectors.iter().filter(usable).any(|c| &c.id == id))
                .or_else(|| connectors.iter().find(usable).map(|c| c.id.clone()));
            let Some(id) = chosen else { return Ok((connectors, work, None)) };
            let mut params = vec![("assignee", "me".to_owned()), ("limit", PAGE.to_owned())];
            if !query.is_empty() { params.push(("query", query)); }
            if let Some(cursor) = cursor { params.push(("cursor", cursor)); }
            let params: Vec<(&str, &str)> = params.iter().map(|(k, v)| (*k, v.as_str())).collect();
            let page = api.server_read(&["connectors", &id, "tickets"], &params, 45).await?;
            let page: TicketPage = serde_json::from_value(page).map_err(|_| Failure::local("invalid_response"))?;
            Ok((connectors, work, Some((id, page))))
        };
        run(&self.runtime, request, window, cx, move |this: &mut Hangar, result, window, cx| {
            if this.mission.seq != seq { return; }
            (this.mission.loading, this.mission.more) = (false, false);
            match result {
                // A falha do "Carregar mais" não apaga o que já está na tela.
                Err(error) if more => window.push_notification(Notification::error(failure_text(&error)), cx),
                Err(error) => this.mission.board = Some(Err(failure_text(&error))),
                Ok((connectors, work, None)) => {
                    this.mission.connector = None;
                    this.mission.board = Some(Ok(Board { connectors, work, tickets: Vec::new(), next_cursor: None, total: None, query: used }));
                }
                Ok((connectors, work, Some((id, page)))) => {
                    let mut tickets = page.items;
                    if more && let Some(Ok(board)) = this.mission.board.take() {
                        let seen: HashSet<String> = board.tickets.iter().map(|t| t.id.clone()).collect();
                        tickets.retain(|t| !seen.contains(&t.id));
                        tickets = board.tickets.into_iter().chain(tickets).collect();
                    }
                    this.mission.connector = Some(id);
                    this.mission.board = Some(Ok(Board { connectors, work, tickets, next_cursor: page.next_cursor, total: page.total, query: used }));
                    this.load_thumbs(window, cx);
                }
            }
        });
        cx.notify();
    }

    /// Relê só as etapas e os registros (depois de aprovar, mover ou anexar), sem pedir os tickets de novo.
    fn reload_work(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        let (seq, work_seq) = (self.mission.seq, self.mission.work_seq + 1);
        self.mission.work_seq = work_seq;
        let request = async move {
            let work = api.server_read(&["mission"], &[], 15).await?;
            serde_json::from_value::<Work>(work).map_err(|_| Failure::local("invalid_response"))
        };
        run(&self.runtime, request, window, cx, move |this: &mut Hangar, result, window, cx| {
            if this.mission.seq != seq || this.mission.work_seq != work_seq { return; }
            match result {
                Ok(work) => if let Some(Ok(board)) = this.mission.board.as_mut() { board.work = work; this.load_thumbs(window, cx); },
                Err(error) => window.push_notification(Notification::error(failure_text(&error)), cx),
            }
        });
    }

    /// Miniaturas que faltam das evidências de imagem do ticket aberto.
    fn load_thumbs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(ticket), Some(connector)) = (self.api.clone(), self.mission.selected.clone(), self.mission.connector.clone()) else { return };
        let Some(item) = self.mission_board().and_then(|b| b.item(&connector, &ticket)) else { return };
        let wanted: Vec<String> = item.evidence.iter().filter(|e| e.mime.starts_with("image/") && !self.mission.thumbs.contains_key(&e.id))
            .map(|e| e.id.clone()).collect();
        for id in wanted {
            self.mission.thumbs.insert(id.clone(), None);
            let api = api.clone();
            let fetch = id.clone();
            let request = async move {
                let bytes = api.server_bytes(&["mission", "evidence", &fetch], 60).await?;
                Ok(tokio::task::spawn_blocking(move || crate::media::thumbnail(&bytes)).await.ok().flatten())
            };
            run(&self.runtime, request, window, cx, move |this: &mut Hangar, result: Result<Option<Arc<RenderImage>>, Failure>, _, _| {
                this.mission.thumbs.insert(id, result.ok().flatten());
            });
        }
    }

    fn choose_connector(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        (self.mission.connector, self.mission.selected, self.mission.detail, self.mission.changes) = (Some(id), None, None, None);
        self.load_mission(false, window, cx);
    }

    fn select_ticket(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(connector)) = (self.api.clone(), self.mission.connector.clone()) else { return };
        (self.mission.selected, self.mission.detail, self.mission.changes) = (Some(id.clone()), None, None);
        self.mission.detail_seq += 1;
        let seq = self.mission.detail_seq;
        self.load_thumbs(window, cx);
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

    fn mission_item(&self, ticket: &str) -> Option<&Item> {
        let connector = self.mission.connector.as_deref()?;
        self.mission_board()?.item(connector, ticket)
    }

    /// Sessão do ticket: a ligada pelo Iniciar ou, sem ela, a viva que leva a chave na branch ou no nome.
    fn ticket_session<'a>(&'a self, ticket: &'a Ticket, item: Option<&'a Item>) -> Option<Linked<'a>> {
        if let Some(name) = item.and_then(|item| item.session.as_deref()) {
            return Some(Linked { name, session: self.sessions.iter().find(|s| s.name == name), stored: true });
        }
        self.sessions.iter().find(|s| s.state != "dead" && named_after(s, &ticket.key))
            .map(|s| Linked { name: s.name.as_str(), session: Some(s), stored: false })
    }

    /// Tudo o que a tela mostra de um ticket, calculado uma vez por quadro.
    fn ticket_view<'a>(&'a self, ticket: &'a Ticket) -> TicketView<'a> {
        let item = self.mission_item(&ticket.id);
        let linked = self.ticket_session(ticket, item);
        let stages = self.mission_board().map_or(&[][..], |b| b.work.stages.as_slice());
        let (lane, source) = lane_of(stages, ticket, item, linked.as_ref().and_then(|l| l.session));
        let stage = match lane { Lane::Stage(index) => stages.get(index), _ => None };
        let waiting = stage.filter(|stage| stage.gate && !item.is_some_and(|item| item.approved.contains(&stage.id)));
        TicketView { item, linked, lane, source, stage, waiting }
    }

    /// Iniciar: a tela de nova conversa com a primeira mensagem do conector no campo. A pessoa escolhe a pasta e envia;
    /// a sessão que nascer com essa mensagem fica ligada ao ticket.
    fn start_ticket(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(Ok(view)), Some(connector), Some(server)) = (&self.mission.detail, self.mission.connector.clone(), self.mission.server.clone())
            else { return };
        let ticket = view.detail.ticket.clone();
        let prompt = view.detail.start_prompt.clone()
            .unwrap_or_else(|| tr("mission_default_prompt").replace("{key}", &ticket.key).replace("{title}", &ticket.title).replace("{url}", &ticket.url));
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

    /// Volta da criação: a sessão nascida do Iniciar, com a mensagem do ticket, fica ligada a ele no servidor, que a
    /// avisa da tool `mission`.
    pub(super) fn mission_session_created(&mut self, server: &str, session: &str, first: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        // Qualquer criação encerra o pedido: só a primeira depois do Iniciar pode ser a dele.
        let Some(start) = self.mission.pending.take() else { return };
        if start.server != server || !first.is_some_and(|text| text.trim_start().starts_with(start.prompt.trim())) { return; }
        let Some(api) = self.api_for(server) else { return };
        self.link_session(api, start.connector, start.ticket, session.to_owned(), window, cx);
    }

    fn link_session(&mut self, api: Api, connector: String, ticket: Ticket, session: String, window: &mut Window, cx: &mut Context<Self>) {
        let body = json!({"key": ticket.key, "title": ticket.title, "url": ticket.url, "session": session});
        let request = async move {
            api.server_send(reqwest::Method::PUT, &["mission", "items", &connector, &ticket.id], Some(body), 15).await.map(|_| ())
        };
        run(&self.runtime, request, window, cx, |this: &mut Hangar, result: Result<(), Failure>, window, cx| match result {
            Ok(()) => if this.mission.open { this.reload_work(window, cx) },
            Err(error) => window.push_notification(Notification::warning(tr("mission_link_failed").replace("{erro}", &failure_text(&error))), cx),
        });
    }

    /// Pedido do dono ao Mission Control, com o botão ocupado enquanto vai; no sucesso, `done` e a releitura.
    fn mission_send(&mut self, method: reqwest::Method, path: Vec<String>, body: Option<Value>,
        done: impl FnOnce(&mut Hangar) + 'static, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        self.mission.busy = true;
        let request = async move {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            api.server_send(method, &path, body, 15).await
        };
        run(&self.runtime, request, window, cx, move |this: &mut Hangar, result, window, cx| {
            this.mission.busy = false;
            match result {
                Ok(_) => { done(this); this.reload_work(window, cx); }
                Err(error) => window.push_notification(Notification::error(failure_text(&error)), cx),
            }
        });
        cx.notify();
    }

    /// Caminho do registro do ticket aberto: `mission/items/{conector}/{ticket}`.
    fn item_path(&self) -> Option<Vec<String>> {
        Some(vec!["mission".into(), "items".into(), self.mission.connector.clone()?, self.mission.selected.clone()?])
    }

    /// Mudança do dono no registro do ticket aberto (`PATCH`).
    fn mission_change(&mut self, body: Value, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.item_path() else { return };
        self.mission_send(reqwest::Method::PATCH, path, Some(body), |_| {}, window, cx);
    }

    /// Ligar à tela uma sessão detectada pela chave: a partir daí ela usa a tool e aparece mesmo com outro nome.
    fn adopt_session(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(connector), Some(Ok(view))) = (self.api.clone(), self.mission.connector.clone(), &self.mission.detail) else { return };
        let ticket = view.detail.ticket.clone();
        self.link_session(api, connector, ticket, name, window, cx);
    }

    /// Esquecer o ticket: vínculo, etapa e evidências saem.
    fn forget_ticket(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.item_path() else { return };
        self.mission_send(reqwest::Method::DELETE, path, None, |_| {}, window, cx);
    }

    /// Pedir ajustes: a etapa volta, e o servidor manda o motivo à sessão ligada. O texto só sai do campo gravado.
    fn send_changes(&mut self, stage: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(input), Some(path)) = (self.mission.changes.clone(), self.item_path()) else { return };
        let note = input.read(cx).value().trim().to_owned();
        if note.is_empty() { return; }
        self.mission_send(reqwest::Method::PATCH, path, Some(json!({"action": "changes", "note": note, "stage": stage})),
            |this| this.mission.changes = None, window, cx);
    }

    /// Anexar uma evidência do disco desta máquina ao ticket aberto.
    fn pick_evidence(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(api), Some(connector), Some(ticket)) = (self.api.clone(), self.mission.connector.clone(), self.mission.selected.clone()) else { return };
        let prompt = cx.prompt_for_paths(PathPromptOptions { files: true, directories: false, multiple: false, prompt: None });
        let runtime = self.runtime.clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = prompt.await else { return };
            let Some(path) = paths.into_iter().next() else { return };
            let result = runtime.spawn(async move {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("evidencia").to_owned();
                if tokio::fs::metadata(&path).await.map(|m| m.len()).unwrap_or(0) > crate::api::MAX_BYTES { return Err(Failure::local("attach_too_big")); }
                let bytes = tokio::fs::read(&path).await.map_err(|_| Failure::local("picker_failed"))?;
                api.server_upload(&["mission", "items", &connector, &ticket, "evidence"], &[("name", name.as_str())], bytes).await
            }).await.unwrap_or_else(|_| Err(Failure::local("invalid_response")));
            let _ = this.update_in(cx, |this, window, cx| match result {
                Ok(_) => this.reload_work(window, cx),
                Err(error) => window.push_notification(Notification::error(failure_text(&error)), cx),
            });
        }).detach();
    }

    /// Abre a evidência no programa do sistema: baixa para a pasta privada e entrega o caminho.
    fn open_evidence(&mut self, id: String, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(api) = self.api.clone() else { return };
        // O nome vem do servidor: o cliente confere sozinho, como no abrir de anexo, antes de entregar ao sistema.
        let safe = composer::safe_name(&name);
        if !composer::openable(&safe) {
            window.push_notification(Notification::warning(tr("mission_evidence_not_openable")), cx);
            return;
        }
        let request = async move {
            let bytes = api.server_bytes(&["mission", "evidence", &id], 300).await?;
            let path = private_copy(&safe).map_err(|_| Failure::local("save_failed"))?;
            tokio::fs::write(&path, bytes).await.map_err(|_| Failure::local("save_failed"))?;
            Ok(path)
        };
        run(&self.runtime, request, window, cx, |_: &mut Hangar, result: Result<PathBuf, Failure>, window, cx| match result {
            Ok(path) => cx.open_with_system(&path),
            Err(error) => window.push_notification(Notification::error(failure_text(&error)), cx),
        });
    }

    fn open_linked(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.sessions.iter().find(|s| s.name == name).cloned() else { return };
        let Some(server) = self.mission.server.clone() else { return };
        self.close_mission(window, cx);
        self.select_on(&servers::norm(&server), session, window, cx);
    }

    fn edit_stages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(board) = self.mission_board() else { return };
        let text = stages_text(&board.work.stages);
        let editor = cx.new(|cx| TextareaState::new(window, cx).auto_grow(6, 16));
        editor.update(cx, |input, cx| input.set_value(text, window, cx));
        self.mission.stages_editor = Some(editor);
        cx.notify();
    }

    fn save_stages(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(editor), Some(board)) = (self.mission.stages_editor.clone(), self.mission_board()) else { return };
        let stages = match parse_stages(&editor.read(cx).value(), &board.work.stages) {
            Ok(stages) => stages,
            Err(error) => { window.push_notification(Notification::warning(error), cx); return; }
        };
        self.mission_send(reqwest::Method::PUT, vec!["mission".into(), "stages".into()], Some(json!({"stages": stages})),
            |this| this.mission.stages_editor = None, window, cx);
    }

    // ── Desenho ─────────────────────────────────────────────────────────────

    pub(super) fn render_mission(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Outro servidor ativo: o que está na tela era do anterior.
        if self.api.as_ref().map(|api| api.identity()) != self.mission.server {
            self.load_mission(false, window, cx);
        }
        let loading = self.mission.loading;
        let search = self.mission.query.clone().map(|input| Input::new(&input).small().w(px(240.)).cleanable(true)
            .aria_label(tr("mission_search")).prefix(chrome::small_icon(IconName::Search, 13., theme::faint())).into_any_element());
        let chooser = self.render_mission_connectors(cx);
        let stages = Button::new("mission-stages").ghost().small().icon(IconName::Settings2).label(tr("mission_stages"))
            .selected(self.mission.stages_editor.is_some()).disabled(self.mission_board().is_none())
            .on_click(cx.listener(|this, _, window, cx| if this.mission.stages_editor.is_some() {
                this.mission.stages_editor = None; cx.notify();
            } else { this.edit_stages(window, cx) }));
        let header = self.costs_header(tr("mission_title"), |this, window, cx| this.close_mission(window, cx),
            search.into_iter().chain(chooser).chain([
                stages.into_any_element(),
                Button::new("mission-refresh").outline().small().label(tr_shared("custos_atualizar", &[])).loading(loading).disabled(loading)
                    .on_click(cx.listener(|this, _, window, cx| this.load_mission(false, window, cx))).into_any_element(),
            ]).collect(), cx);
        let body = if self.mission.stages_editor.is_some() { self.render_stages_editor(cx) } else { self.render_mission_body(cx) };
        let panel = self.mission.selected.clone().filter(|_| self.mission.stages_editor.is_none()).map(|_| self.render_mission_detail(cx));
        page_frame(div().size_full().flex().flex_col().child(header)
            .child(div().flex_1().min_h_0().flex().child(body).children(panel)))
    }

    fn render_mission_connectors(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let board = self.mission_board()?;
        let usable: Vec<&Connector> = board.connectors.iter().filter(|c| c.enabled && c.capabilities.contains_key("work_tracking")).collect();
        if usable.len() < 2 { return None; }
        Some(div().flex().gap(px(4.)).children(usable.into_iter().map(|c| {
            let id = c.id.clone();
            Button::new(SharedString::from(format!("mission-connector-{}", c.id))).small().ghost().label(c.name.clone())
                .selected(Some(&c.id) == self.mission.connector.as_ref())
                .on_click(cx.listener(move |this, _, window, cx| this.choose_connector(id.clone(), window, cx)))
        })).into_any_element())
    }

    fn render_stages_editor(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(editor) = self.mission.stages_editor.clone() else { return div().into_any_element() };
        div().flex_1().min_w_0().h_full().overflow_hidden().flex().justify_center().child(
            div().id("mission-stages-editor").w_full().max_w(px(760.)).h_full().overflow_y_scroll().px(px(24.)).pt(px(20.)).pb(px(40.))
                .flex().flex_col().gap(px(14.))
                .child(div().text_size(px(18.)).font_weight(FontWeight::SEMIBOLD).child(tr("mission_stages_title")))
                .child(div().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(tr("mission_stages_hint")))
                .child(div().font_family(theme::MONO).child(Textarea::new(&editor).aria_label(tr("mission_stages_title"))))
                .child(div().flex().gap(px(8.))
                    .child(Button::new("mission-stages-save").primary().small().label(tr("mission_stages_save")).loading(self.mission.busy)
                        .disabled(self.mission.busy).on_click(cx.listener(|this, _, window, cx| this.save_stages(window, cx))))
                    .child(Button::new("mission-stages-cancel").ghost().small().label(tr("cancel"))
                        .on_click(cx.listener(|this, _, _, cx| { this.mission.stages_editor = None; cx.notify(); })))))
            .into_any_element()
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
        let stages = &board.work.stages;
        let mut lanes: Vec<(String, Lane, Vec<(&Ticket, TicketView)>)> = std::iter::once((tr("mission_inbox"), Lane::Inbox, Vec::new()))
            .chain(stages.iter().enumerate().map(|(n, s)| (s.name.clone(), Lane::Stage(n), Vec::new())))
            .chain(std::iter::once((tr("mission_done"), Lane::Done, Vec::new()))).collect();
        for ticket in &board.tickets {
            let view = self.ticket_view(ticket);
            if let Some((_, _, list)) = lanes.iter_mut().find(|(_, l, _)| *l == view.lane) { list.push((ticket, view)); }
        }
        let gates: HashSet<usize> = stages.iter().enumerate().filter(|(_, s)| s.gate).map(|(n, _)| n).collect();
        let columns = div().id("mission-lanes").flex_1().min_h_0().overflow_x_scroll().flex().gap(px(12.))
            .children(lanes.into_iter().enumerate().map(|(n, (title, lane, items))| {
                let count = items.len();
                let cards: Vec<AnyElement> = items.into_iter().map(|(ticket, view)| self.render_mission_card(ticket, view, cx)).collect();
                let gate = matches!(lane, Lane::Stage(index) if gates.contains(&index));
                div().w(px(250.)).flex_shrink_0().h_full().flex().flex_col().rounded(px(12.)).bg(theme::inset())
                    .child(div().px(px(12.)).py(px(10.)).flex().items_center().gap(px(6.))
                        .when(gate, |el| el.child(chrome::small_icon(IconName::ShieldCheck, 13., theme::warning())))
                        .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                        .child(div().text_size(px(12.)).text_color(theme::faint()).child(count.to_string())))
                    .child(div().id(SharedString::from(format!("mission-lane-{n}"))).flex_1().min_h_0().overflow_y_scroll().px(px(8.)).pb(px(8.))
                        .flex().flex_col().gap(px(8.))
                        .when(cards.is_empty(), |el| el.child(empty_state(tr("mission_empty_column"))))
                        .children(cards))
            }));
        let shown = board.tickets.len().to_string();
        let total = board.total.map(|t| t.to_string()).unwrap_or_else(|| shown.clone());
        let footer = div().flex().items_center().gap(px(10.)).text_size(px(12.)).text_color(theme::muted())
            .child(tr("mission_shown").replace("{n}", &shown).replace("{total}", &total))
            .when(board.next_cursor.is_some(), |el| el.child(Button::new("mission-more").ghost().small().label(tr("mission_more"))
                .loading(self.mission.more).disabled(self.mission.more)
                .on_click(cx.listener(|this, _, window, cx| this.load_mission(true, window, cx)))));
        frame(div().flex_1().min_h_0().flex().flex_col().gap(px(10.)).child(columns).child(footer).into_any_element())
    }

    fn render_mission_card(&self, ticket: &Ticket, view: TicketView, cx: &mut Context<Self>) -> AnyElement {
        let selected = self.mission.selected.as_deref() == Some(ticket.id.as_str());
        let id = ticket.id.clone();
        let session = view.linked.as_ref().map(|linked| match linked.session {
            Some(session) => session_line(session),
            None => div().text_size(px(11.5)).text_color(theme::faint()).truncate().child(tr("mission_session_gone").replace("{name}", linked.name)),
        });
        let evidence = view.item.map_or(0, |item| item.evidence.len());
        let waiting = view.waiting.map(|stage| stage.name.clone());
        div().id(SharedString::from(format!("mission-card-{}", ticket.id))).w_full().p(px(10.)).rounded(px(10.)).border_1()
            .border_color(if selected { theme::accent() } else if waiting.is_some() { theme::warning().alpha(0.6) } else { theme::border() })
            .bg(theme::boxed()).cursor_pointer()
            .hover(|s| s.border_color(theme::accent().alpha(0.5)))
            .flex().flex_col().gap(px(6.))
            .child(div().flex().items_center().gap(px(6.))
                .child(div().font_family(theme::MONO).text_size(px(11.5)).text_color(theme::muted()).child(ticket.key.clone()))
                .child(div().flex_1())
                .when(evidence > 0, |el| el.child(div().flex().items_center().gap(px(3.)).text_size(px(11.)).text_color(theme::muted())
                    .child(chrome::small_icon(IconName::Image, 12., theme::muted())).child(evidence.to_string())))
                .child(div().px(px(6.)).py(px(1.)).rounded(px(6.)).bg(theme::inset()).text_size(px(11.)).text_color(theme::muted()).truncate()
                    .max_w(px(120.)).child(ticket.status.name.clone())))
            .child(div().text_size(px(13.)).line_height(relative(1.35)).whitespace_normal().line_clamp(3).child(ticket.title.clone()))
            .children(session)
            .children(waiting.map(|stage| div().text_size(px(11.5)).text_color(theme::warning()).child(tr("mission_waiting_you").replace("{etapa}", &stage))))
            .on_click(cx.listener(move |this, _, window, cx| this.select_ticket(id.clone(), window, cx)))
            .into_any_element()
    }

    fn render_mission_detail(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let close = Button::new("mission-detail-close").ghost().small().icon(IconName::Close).accessibility_label(tr("close"))
            .on_click(cx.listener(|this, _, _, cx| { (this.mission.selected, this.mission.detail, this.mission.changes) = (None, None, None); cx.notify(); }));
        let panel = div().id("mission-detail").w(px(480.)).flex_shrink_0().h_full().overflow_y_scroll().border_l_1().border_color(theme::border())
            .p(px(16.)).flex().flex_col().gap(px(14.)).text_size(px(13.)).whitespace_normal();
        let view = match &self.mission.detail {
            None => return panel.child(div().flex().justify_end().child(close)).child(loading_state()),
            Some(Err(error)) => return panel.child(div().flex().justify_end().child(close)).child(note_box(error.clone(), theme::warning())),
            Some(Ok(view)) => view,
        };
        let ticket = &view.detail.ticket;
        let tv = self.ticket_view(ticket);
        let item = tv.item;
        let stages = self.mission_board().map_or(&[][..], |b| b.work.stages.as_slice());
        let meta = [Some(ticket.status.name.clone()), ticket.assignee.as_ref().map(|a| a.name.clone()),
            ticket.updated_at.as_deref().and_then(ago)].into_iter().flatten().collect::<Vec<_>>().join(" · ");
        let url = ticket.url.clone();
        let busy = self.mission.busy;

        let mut actions = div().flex().flex_wrap().gap(px(8.));
        match &tv.linked {
            Some(Linked { name, session: Some(_), stored }) => {
                let open = (*name).to_owned();
                actions = actions.child(Button::new("mission-open-session").primary().small().icon(IconName::MessageSquare).label(tr("mission_open_session"))
                    .on_click(cx.listener(move |this, _, window, cx| this.open_linked(open.clone(), window, cx))));
                if !stored {
                    let adopt = (*name).to_owned();
                    actions = actions.child(Button::new("mission-adopt").outline().small().icon(IconName::Link).label(tr("mission_adopt"))
                        .disabled(busy).on_click(cx.listener(move |this, _, window, cx| this.adopt_session(adopt.clone(), window, cx))));
                }
            }
            _ => {
                actions = actions.child(Button::new("mission-start").primary().small().icon(IconName::Play).label(tr("mission_start"))
                    .on_click(cx.listener(|this, _, window, cx| this.start_ticket(window, cx))));
            }
        }
        actions = actions.child(Button::new("mission-source").outline().small().icon(IconName::ExternalLink).label(tr("mission_open_source"))
            .on_click(move |_, window, cx| super::links::open_link(&url, true, window, cx)));
        if item.is_some() {
            let weak = cx.entity().downgrade();
            let menu_stages = stages.to_vec();
            actions = actions.child(Button::new("mission-move").ghost().small().label(tr("mission_move")).icon(IconName::ChevronDown).disabled(busy)
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let pick = |body: Value| {
                        let weak = weak.clone();
                        move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                            let _ = weak.update(cx, |this, cx| this.mission_change(body.clone(), window, cx));
                        }
                    };
                    let mut menu = sidebar::menu_style(menu).min_w(px(220.));
                    for stage in &menu_stages {
                        menu = menu.item(PopupMenuItem::new(stage.name.clone()).on_click(pick(json!({"action": "stage", "stage": stage.id}))));
                    }
                    menu.separator()
                        .item(PopupMenuItem::new(tr("mission_delivered")).on_click(pick(json!({"action": "delivered"}))))
                        .item(PopupMenuItem::new(tr("mission_reset")).on_click(pick(json!({"action": "reset"}))))
                }));
            actions = actions.child(Button::new("mission-forget").ghost().small().label(tr("mission_forget")).disabled(busy)
                .on_click(cx.listener(|this, _, window, cx| this.forget_ticket(window, cx))));
        }

        let stage_name = match (tv.lane, tv.stage) {
            (_, Some(stage)) => stage.name.clone(),
            (Lane::Done, _) => tr("mission_done"),
            _ => tr("mission_inbox"),
        };
        let source_text = match tv.source {
            Source::Reported => tr("mission_source_reported"),
            Source::Status => tr("mission_source_status"),
            Source::Plan => tr("mission_source_plan"),
            Source::None => String::new(),
        };
        let stage_line = div().flex().items_center().gap(px(6.)).text_size(px(12.5))
            .child(div().text_color(theme::muted()).child(tr("mission_stage_label")))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(stage_name))
            .when(!source_text.is_empty(), |el| el.child(div().text_color(theme::faint()).child(format!("· {source_text}"))));

        // A aprovação e o pedido de ajustes levam a etapa que a tela mostra: deduzida, o servidor ainda não a tem.
        let shown = tv.stage.map(|stage| stage.id.clone());
        let gate = tv.waiting.filter(|_| item.is_some()).map(|stage| {
            let stage = &stage.name;
            let mut block = div().p(px(12.)).rounded(px(10.)).border_1().border_color(theme::warning().alpha(0.5)).flex().flex_col().gap(px(8.))
                .child(div().flex().items_center().gap(px(6.)).text_color(theme::warning())
                    .child(chrome::small_icon(IconName::ShieldCheck, 14., theme::warning()))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("mission_gate_title").replace("{etapa}", &stage))))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("mission_gate_hint")));
            match self.mission.changes.clone() {
                Some(input) => {
                    block = block.child(Textarea::new(&input).aria_label(tr("mission_changes")))
                        .child(div().flex().gap(px(8.))
                            .child(Button::new("mission-changes-send").primary().small().label(tr("mission_changes_send")).disabled(busy)
                                .on_click(cx.listener({ let shown = shown.clone(); move |this, _, window, cx| this.send_changes(shown.clone(), window, cx) })))
                            .child(Button::new("mission-changes-cancel").ghost().small().label(tr("cancel"))
                                .on_click(cx.listener(|this, _, _, cx| { this.mission.changes = None; cx.notify(); }))));
                }
                None => {
                    block = block.child(div().flex().gap(px(8.))
                        .child(Button::new("mission-approve").primary().small().icon(IconName::Check).label(tr("mission_approve")).loading(busy).disabled(busy)
                            .on_click(cx.listener({ let shown = shown.clone(); move |this, _, window, cx| this.mission_change(json!({"action": "approve", "stage": shown}), window, cx) })))
                        .child(Button::new("mission-changes").outline().small().label(tr("mission_changes")).disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                let input = cx.new(|cx| TextareaState::new(window, cx).auto_grow(2, 6).placeholder(tr("mission_changes_placeholder")));
                                input.update(cx, |input, cx| input.focus(window, cx));
                                this.mission.changes = Some(input);
                                cx.notify();
                            }))));
                }
            }
            block
        });

        let session = tv.linked.as_ref().map(|&Linked { name, session, stored }| match session {
            Some(session) => div().p(px(10.)).rounded(px(10.)).bg(theme::inset()).flex().flex_col().gap(px(4.))
                .child(div().text_size(px(12.)).text_color(theme::muted())
                    .child(tr(if stored { "mission_session" } else { "mission_session_detected" }).replace("{name}", name)))
                .child(session_line(session)),
            None => note_box(tr("mission_session_gone").replace("{name}", name), theme::muted()),
        });

        let evidence = self.render_evidence(item, cx);
        let history = item.filter(|i| !i.history.is_empty()).map(|i| render_history(i, stages));
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
            .child(stage_line)
            .child(actions)
            .children(gate)
            .children(session)
            .child(evidence)
            .children(history)
            .child(div().border_t_1().border_color(theme::border()).pt(px(12.)).child(description))
    }

    fn render_evidence(&self, item: Option<&Item>, cx: &mut Context<Self>) -> AnyElement {
        let list = item.map_or(&[][..], |item| item.evidence.as_slice());
        let head = div().flex().items_center().gap(px(8.))
            .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("mission_evidence")))
            .child(div().text_size(px(12.)).text_color(theme::faint()).child(list.len().to_string()))
            .child(div().flex_1())
            .when(item.is_some(), |el| el.child(Button::new("mission-evidence-add").ghost().small().icon(IconName::Plus).label(tr("mission_evidence_add"))
                .on_click(cx.listener(|this, _, window, cx| this.pick_evidence(window, cx)))));
        let mut section = div().flex().flex_col().gap(px(8.)).child(head);
        if item.is_none() {
            return section.child(div().text_size(px(12.5)).text_color(theme::faint()).child(tr("mission_evidence_needs_link"))).into_any_element();
        }
        if list.is_empty() {
            return section.child(div().text_size(px(12.5)).text_color(theme::faint()).child(tr("mission_evidence_empty"))).into_any_element();
        }
        let tiles = list.iter().rev().map(|evidence| {
            let (id, name) = (evidence.id.clone(), evidence.name.clone());
            let thumb = self.mission.thumbs.get(&evidence.id).cloned().flatten();
            let icon = if evidence.mime.starts_with("video/") { IconName::Video } else if evidence.mime.starts_with("image/") { IconName::Image } else { IconName::File };
            let preview = match thumb {
                Some(image) => div().w_full().h(px(120.)).rounded(px(8.)).overflow_hidden().bg(theme::inset())
                    .child(img(image).size_full().object_fit(ObjectFit::Cover)),
                None => div().w_full().h(px(72.)).rounded(px(8.)).bg(theme::inset()).flex().items_center().justify_center()
                    .child(chrome::small_icon(icon, 22., theme::muted())),
            };
            let caption = if evidence.caption.is_empty() { evidence.name.clone() } else { evidence.caption.clone() };
            div().id(SharedString::from(format!("mission-evidence-{}", evidence.id))).w(px(210.)).p(px(6.)).rounded(px(10.)).border_1()
                .border_color(theme::border()).cursor_pointer().hover(|s| s.border_color(theme::accent().alpha(0.5)))
                .flex().flex_col().gap(px(4.))
                .child(preview)
                .child(div().text_size(px(12.)).line_clamp(2).child(caption))
                .child(div().text_size(px(11.)).text_color(theme::faint()).truncate()
                    .child(format!("{} · {} · {}", evidence.by, size_text(evidence.size), evidence.name)))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tr("mission_evidence_open")).build(window, cx))
                .on_click(cx.listener(move |this, _, window, cx| this.open_evidence(id.clone(), name.clone(), window, cx)))
        });
        section = section.child(div().flex().flex_wrap().gap(px(8.)).children(tiles));
        section.into_any_element()
    }
}

fn render_history(item: &Item, stages: &[Stage]) -> Div {
    let name = |id: &str| stages.iter().find(|s| s.id == id).map_or_else(|| id.to_owned(), |s| s.name.clone());
    let rows = item.history.iter().rev().take(30).map(|event| {
        let stage = event.stage.as_deref().map(name).unwrap_or_default();
        let what = match event.kind.as_str() {
            "linked" => tr("mission_event_linked").replace("{nota}", event.note.as_deref().unwrap_or_default()),
            "stage" => tr("mission_event_stage").replace("{etapa}", &stage),
            "approved" => tr("mission_event_approved").replace("{etapa}", &stage),
            "changes" => tr("mission_event_changes").replace("{etapa}", &stage),
            "evidence" => tr("mission_event_evidence").replace("{nota}", event.note.as_deref().unwrap_or_default()),
            "delivered" => tr("mission_event_delivered"),
            _ => tr("mission_event_note"),
        };
        let note = event.note.as_deref().filter(|_| matches!(event.kind.as_str(), "stage" | "changes" | "note" | "delivered")).map(str::to_owned);
        div().flex().flex_col().gap(px(2.)).py(px(4.)).border_b_1().border_color(theme::border())
            .child(div().flex().gap(px(6.)).text_size(px(12.))
                .child(div().child(what))
                .child(div().flex_1())
                .child(div().text_color(theme::faint()).flex_shrink_0().child(format!("{} · {}", event.by, ago(&event.at).unwrap_or_default()))))
            .children(note.map(|note| div().text_size(px(12.)).text_color(theme::muted()).child(note)))
    });
    div().flex().flex_col().gap(px(4.))
        .child(div().font_weight(FontWeight::SEMIBOLD).child(tr("mission_history")))
        .children(rows)
}

/// Estado da sessão e, com plano, a Task em andamento e a barra do que já foi feito.
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

#[cfg(test)]
mod tests {
    use super::*;
    // O glob traz o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    fn stage(id: &str, name: &str) -> Stage { Stage { id: id.into(), name: name.into(), gate: false, statuses: Vec::new() } }

    #[test]
    fn session_named_after_the_key_but_not_a_longer_one() {
        let session = |name: &str, branch: Option<&str>| SessionInfo { name: name.into(), branch: branch.map(Into::into), ..Default::default() };
        assert!(named_after(&session("PM-19816", None), "PM-19816"));
        assert!(named_after(&session("pm-19816-Coordenadora", None), "PM-19816"));
        assert!(named_after(&session("hangar", Some("PM-19816")), "PM-19816"));
        assert!(!named_after(&session("PM-198160", None), "PM-19816"));
        assert!(!named_after(&session("hangar", Some("PM-198160")), "PM-19816"));
    }

    #[test]
    fn stages_text_round_trips_and_keeps_known_ids() {
        let current = vec![Stage { gate: true, statuses: vec!["Revisão de Código".into()], ..stage("revisao", "Revisão") }, stage("x", "Teste")];
        let text = stages_text(&current);
        let parsed = parse_stages(&format!("{text}\nConferência do cliente | | Em Testes, Resolvido\nÚltima etapa"), &current).unwrap();
        assert_eq!((parsed[0]["id"].as_str(), parsed[0]["gate"].as_bool(), parsed[0]["statuses"][0].as_str()),
            (Some("revisao"), Some(true), Some("Revisão de Código")));
        assert_eq!(parsed[1]["id"], "x");
        assert_eq!((parsed[2]["id"].as_str(), parsed[2]["gate"].as_bool(), parsed[2]["statuses"][1].as_str()),
            (Some("conferencia-do-cliente"), Some(false), Some("Resolvido")));
        assert_eq!(parsed[3]["id"], "ultima-etapa");
        assert!(parse_stages("  \n ", &current).is_err());
        assert_eq!(parse_stages("A\na", &[]).unwrap()[1]["id"], "a-2", "nome repetido ganha id próprio");
    }
}
