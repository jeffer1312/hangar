//! Aba Atividade do painel da direita: o `ActivitySheet` docado do web no nível da lista (agentes rodando, subagentes do
//! disco, shells de fundo, processos vivos e tarefas) e o botão Atividade do cabeçalho. A lista de subagentes é lida por
//! esta view, em pedidos numerados, e a chegada redesenha só ela; a conta de subagentes no disco, que decide se a aba e o
//! botão existem, é do `Hangar`, como no `Chat.svelte`.
use super::*;
use crate::conversation::{Activity, TaskStatus};
use gpui_kit::component::tooltip::Tooltip;

/// Recontagem dos subagentes no disco enquanto a sessão trabalha.
const COUNT_EVERY: Duration = Duration::from_secs(5);
/// O "há N min" dos shells anda de 20 em 20 s, e só com a aba à vista.
const CLOCK_EVERY: Duration = Duration::from_secs(20);

pub(super) enum ActivityReply {
    /// Quantos subagentes a sessão tem no disco, amarrado à sessão e ao pedido.
    Count(SessionKey, u64, Result<usize, Failure>),
    /// A lista da aba, com o número do pedido.
    List(u64, Result<Vec<SubRun>, Failure>),
}

/// Um subagente do disco (`GET …/subagents`), só com o que a lista mostra e casa.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SubRun {
    agent_id: String, agent_type: Option<String>, prompt: Option<String>, calls: u64, last_tool: Option<String>,
    finished: bool, unreadable: bool,
}

fn parse_subs(value: &Value) -> Vec<SubRun> {
    let text = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
    value.as_array().map(|list| list.iter().filter_map(|item| Some(SubRun {
        agent_id: text(item, "agentId")?,
        agent_type: text(item, "agentType").filter(|t| !t.is_empty()),
        prompt: text(item, "prompt"),
        calls: item.get("toolCalls").and_then(Value::as_u64).unwrap_or(0),
        last_tool: item.get("recent").and_then(Value::as_array).and_then(|recent| recent.last()).and_then(|last| text(last, "name")),
        finished: item.get("finished").and_then(Value::as_bool).unwrap_or(false),
        unreadable: item.get("ilegivel").and_then(Value::as_bool).unwrap_or(false),
    })).collect()).unwrap_or_default()
}

fn web(key: &str) -> String { crate::i18n::tr_web(key, &HashMap::new()).unwrap_or_else(|| key.to_owned()) }
fn web_with(key: &str, name: &str, value: String) -> String {
    crate::i18n::tr_web(key, &HashMap::from([(name.to_owned(), value)])).unwrap_or_else(|| key.to_owned())
}

/// O `formatarIntervalo` do web: um minuto é o piso.
fn interval(seconds: f64) -> String {
    match seconds.max(0.) {
        s if s < 60. => "1 min".into(),
        s if s < 3600. => format!("{} min", (s / 60.).floor()),
        s if s < 86_400. => format!("{} h", (s / 3600.).floor()),
        s => format!("{} d", (s / 86_400.).floor()),
    }
}

fn now_seconds() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.) }

fn norm(text: &str) -> String { text.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase() }

/// O subagente do disco de um Agent: prompt normalizado inteiro, senão os 400 primeiros caracteres (o mais recente
/// vence, e a lista já vem do mais recém-escrito).
fn match_sub<'a>(subs: &'a [SubRun], prompt: Option<&str>) -> Option<&'a SubRun> {
    let full = norm(prompt?);
    subs.iter().find(|s| s.prompt.as_deref().is_some_and(|p| norm(p) == full)).or_else(|| {
        let head: String = full.chars().take(400).collect();
        subs.iter().find(|s| s.prompt.as_deref().is_some_and(|p| norm(p).starts_with(&head)))
    })
}

/// Primeira linha útil do prompt (pula o cabeçalho de skill e títulos), até 90 caracteres.
fn base_title(sub: &SubRun) -> String {
    let prompt = sub.prompt.as_deref().unwrap_or("");
    let lines: Vec<&str> = prompt.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let useful = lines.iter().find(|l| !l.to_lowercase().starts_with("base directory for this skill:") && !l.starts_with('#'));
    useful.or(lines.first()).copied().unwrap_or(&sub.agent_id).chars().take(90).collect()
}

/// Título repetido (os itens de um AgentSwarm abrem igual) leva o `agentId`.
fn sub_title(subs: &[SubRun], sub: &SubRun) -> String {
    let base = base_title(sub);
    if subs.iter().any(|o| o.agent_id != sub.agent_id && base_title(o) == base) { format!("{base} · {}", sub.agent_id) } else { base }
}

/// "N chamadas · última ferramenta".
fn calls_line(sub: &SubRun) -> String {
    let mut line = web_with("atividade_chamadas", "n", sub.calls.to_string());
    if let Some(tool) = &sub.last_tool { line.push_str(&format!(" · {tool}")); }
    line
}

#[derive(Clone, Debug, PartialEq)]
struct AgentLine { key: String, description: String, tags: Vec<(String, bool)>, now: Option<String> }
#[derive(Clone, Debug, PartialEq)]
struct OrphanLine { key: String, title: String, tag: Option<String>, now: String, done: bool }
#[derive(Clone, Debug, PartialEq)]
struct ShellLine { key: String, label: String, raw: String, now: String }
#[derive(Clone, Debug, PartialEq)]
struct TaskLine { mark: &'static str, text: String, status: TaskStatus }

/// O que a lista mostra, preparado quando um dado muda; o desenho só lê.
#[derive(Clone, Debug, Default, PartialEq)]
struct Lines { agents: Vec<AgentLine>, orphans: Vec<OrphanLine>, shells: Vec<ShellLine>, processes: Vec<ShellLine>, tasks: Vec<TaskLine>, timed: bool }

#[derive(Clone)]
pub(super) struct Link { pub api: Api, pub runtime: Arc<Runtime>, pub tx: async_channel::Sender<Envelope>, pub connection: u64 }

pub(super) struct ActivityPanel {
    link: Option<Link>,
    key: Option<SessionKey>,
    shown: bool,
    activity: Activity,
    processes: Vec<ShellAlive>,
    subs: Vec<SubRun>,
    seq: u64,
    loading: bool,
    failed: bool,
    lines: Lines,
    clock: Option<Task<()>>,
    scroll: ScrollHandle,
}

impl ActivityPanel {
    fn new() -> Self {
        Self { link: None, key: None, shown: false, activity: Activity::default(), processes: Vec::new(), subs: Vec::new(), seq: 0,
            loading: false, failed: false, lines: Lines::default(), clock: None, scroll: ScrollHandle::new() }
    }

    /// Refaz as linhas; `true` quando mudou algo que aparece.
    fn prepare(&mut self) -> bool {
        let now = now_seconds();
        let mut timed = false;
        let agents = self.activity.running_agents().map(|a| {
            let sub = match_sub(&self.subs, a.prompt.as_deref());
            let mut tags = Vec::new();
            if let Some(t) = &a.subagent_type { tags.push((t.clone(), false)); }
            if let Some(m) = &a.model { tags.push((m.clone(), true)); }
            AgentLine { key: a.id.clone(), description: a.description.clone(), tags, now: sub.map(calls_line) }
        }).collect();
        let listed: HashSet<&str> = self.activity.agents.iter()
            .filter_map(|a| match_sub(&self.subs, a.prompt.as_deref())).map(|s| s.agent_id.as_str()).collect();
        let orphans = self.subs.iter().filter(|s| !listed.contains(s.agent_id.as_str())).map(|s| OrphanLine {
            key: s.agent_id.clone(), title: sub_title(&self.subs, s), tag: s.agent_type.clone(), done: s.finished,
            // Ilegível não diz "0 chamadas": os campos vêm zerados porque o registro não foi lido.
            now: if s.unreadable { web("atividade_sub_ilegivel") }
                else if s.finished { format!("{} · {}", web("atividade_sub_concluido"), calls_line(s)) } else { calls_line(s) },
        }).collect();
        let shells = self.activity.running_shells().map(|s| {
            let mut now_text = s.description.clone().unwrap_or_default();
            if let Some(ts) = s.ts {
                timed = true;
                if !now_text.is_empty() { now_text.push_str(" · "); }
                now_text.push_str(&web_with("atividade_shell_ha", "t", interval(now - ts)));
            }
            ShellLine { key: s.id.clone(), label: s.label.clone(), raw: s.command.clone(), now: now_text }
        }).collect();
        let processes = self.processes.iter().map(|p| {
            let mut now_text = format!("pid {}", p.pid);
            if let Some(since) = p.desde {
                timed = true;
                now_text.push_str(&format!(" · {}", web_with("atividade_shell_ha", "t", interval(now - since))));
            }
            ShellLine { key: p.pid.to_string(), label: p.cmd.clone(), raw: p.cmd.clone(), now: now_text }
        }).collect();
        let tasks = self.activity.tasks.iter().map(|t| TaskLine {
            mark: match t.status { TaskStatus::Completed => "✓", TaskStatus::InProgress => "◐", TaskStatus::Pending => "○" },
            text: match (&t.status, &t.active_form) { (TaskStatus::InProgress, Some(form)) => form.clone(), _ => t.title.clone() },
            status: t.status.clone(),
        }).collect();
        let lines = Lines { agents, orphans, shells, processes, tasks, timed };
        if lines == self.lines { return false; }
        self.lines = lines;
        true
    }

    /// Conversa e estado novos da sessão aberta: a view só redesenha se a lista mudou.
    fn set_data(&mut self, activity: &Activity, processes: &[ShellAlive], cx: &mut Context<Self>) {
        if &self.activity == activity && self.processes == processes { return; }
        (self.activity, self.processes) = (activity.clone(), processes.to_vec());
        if self.prepare() { cx.notify(); }
    }

    /// A aba passou a aparecer (ou mudou de sessão): relê a lista. `None` = escondida, o relógio para.
    fn show(&mut self, target: Option<(SessionKey, Link)>, cx: &mut Context<Self>) {
        let Some((key, link)) = target else {
            self.shown = false;
            self.clock = None;
            return;
        };
        let other = self.key.as_ref() != Some(&key);
        if other || !self.shown {
            // Aba que volta a aparecer relê do zero, como o web, que desmonta a lista ao trocar de aba; resposta da
            // sessão anterior não entra na desta.
            self.seq += 1;
            (self.subs, self.failed, self.loading) = (Vec::new(), false, false);
            self.key = Some(key);
            self.link = Some(link);
            self.prepare();
            self.load(cx);
        }
        self.shown = true;
        if self.clock.is_none() {
            self.clock = Some(cx.spawn(async move |this, cx| loop {
                cx.background_executor().timer(CLOCK_EVERY).await;
                let alive = this.update(cx, |this, cx| if this.lines.timed && this.prepare() { cx.notify() });
                if alive.is_err() { break; }
            }));
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let (Some(link), Some(key)) = (self.link.clone(), self.key.clone()) else { return };
        self.seq += 1;
        self.loading = true;
        let seq = self.seq;
        link.runtime.spawn(async move {
            let result = link.api.read(&key.name, &["subagents"], &[], 15).await.map(|value| parse_subs(&value));
            let _ = link.tx.send(Envelope { connection: link.connection, selection: None, payload: Payload::Activity(ActivityReply::List(seq, result)) }).await;
        });
        cx.notify();
    }

    fn receive(&mut self, seq: u64, result: Result<Vec<SubRun>, Failure>, cx: &mut Context<Self>) {
        if seq != self.seq { return; }
        self.loading = false;
        // Falha mantém o que já se sabia e avisa em cima de tudo.
        match result { Ok(subs) => (self.subs, self.failed) = (subs, false), Err(_) => self.failed = true }
        self.prepare();
        cx.notify();
    }
}

fn spinning(key: String) -> AnyElement {
    div().mt(px(2.)).flex_shrink_0().child(chrome::Spinner::new(SharedString::from(key), IconName::LoaderCircle, px(13.), theme::accent())).into_any_element()
}

fn section(title: String, count: Option<usize>) -> Div {
    div().flex().flex_col().gap_2().child(chrome::section_label(title).flex().gap_1()
        .when_some(count, |el, n| el.child(div().text_color(theme::accent()).child(n.to_string()))))
}

fn tag(text: String, muted: bool) -> AnyElement {
    div().flex_shrink_0().px(px(6.)).py(px(1.)).rounded_full().bg(theme::hover()).font_family(theme::MONO)
        .text_size(px(10.)).text_color(if muted { theme::faint() } else { theme::muted() }).child(text).into_any_element()
}

fn now_text(text: String) -> AnyElement {
    div().font_family(theme::MONO).text_size(px(10.)).text_color(theme::faint()).child(text).into_any_element()
}

fn shell_row(prefix: &str, line: &ShellLine) -> AnyElement {
    let raw = line.raw.clone();
    div().id(SharedString::from(format!("{prefix}-{}", line.key))).flex().items_start().gap_2()
        .tooltip(move |window, cx| Tooltip::new(raw.clone()).build(window, cx))
        .child(spinning(format!("{prefix}-spin-{}", line.key)))
        .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
            .child(div().font_family(theme::MONO).text_sm().text_color(theme::text()).line_clamp(2).child(line.label.clone()))
            .when(!line.now.is_empty(), |el| el.child(now_text(line.now.clone()))))
        .into_any_element()
}

impl Render for ActivityPanel {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let lines = &self.lines;
        let mut body = div().px_4().py(px(14.)).flex().flex_col().gap_4();
        if self.failed {
            body = body.child(div().text_xs().text_color(theme::warning()).child(format!("⚠ {}", web("atividade_erro_subagentes"))));
        }
        if !lines.agents.is_empty() {
            body = body.child(section(web("atividade_rodando_agora"), None).children(lines.agents.iter().map(|a| {
                div().flex().items_start().gap_2().child(spinning(format!("act-agent-spin-{}", a.key)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                        .child(div().flex().items_center().gap_1().min_w_0()
                            .child(div().min_w_0().truncate().text_sm().text_color(theme::text()).child(a.description.clone()))
                            .children(a.tags.iter().map(|(t, muted)| tag(t.clone(), *muted))))
                        .when_some(a.now.clone(), |el, now| el.child(now_text(now))))
            })));
        }
        if !lines.orphans.is_empty() {
            body = body.child(section(web("atividade_subagentes"), None).children(lines.orphans.iter().map(|s| {
                div().flex().flex_col().gap(px(3.)).when(s.done, |el| el.opacity(0.6))
                    .child(div().flex().items_center().gap_1().min_w_0()
                        .child(div().min_w_0().truncate().text_sm().text_color(theme::text()).child(s.title.clone()))
                        .when_some(s.tag.clone(), |el, t| el.child(tag(t, false))))
                    .child(now_text(s.now.clone()))
            })));
        }
        if !lines.shells.is_empty() {
            body = body.child(section(web("atividade_shells"), Some(lines.shells.len()))
                .children(lines.shells.iter().map(|s| shell_row("act-shell", s))));
        }
        if !lines.processes.is_empty() {
            body = body.child(section(web("atividade_processos"), Some(lines.processes.len()))
                .children(lines.processes.iter().map(|p| shell_row("act-proc", p))));
        }
        if !lines.tasks.is_empty() {
            body = body.child(section(web("atividade_tarefas"), None).children(lines.tasks.iter().map(|t| {
                let (mark, text) = match t.status {
                    TaskStatus::InProgress => (theme::accent(), theme::text()),
                    TaskStatus::Completed => (theme::success(), theme::faint()),
                    TaskStatus::Pending => (theme::faint(), theme::muted()),
                };
                div().flex().items_baseline().gap_2()
                    .child(div().w(px(14.)).flex_shrink_0().text_center().text_sm().text_color(mark).child(t.mark))
                    .child(div().text_sm().text_color(text).when(t.status == TaskStatus::Completed, |el| el.line_through()).child(t.text.clone()))
            })));
        }
        let empty = lines.agents.is_empty() && lines.orphans.is_empty() && lines.shells.is_empty() && lines.processes.is_empty() && lines.tasks.is_empty();
        if empty {
            // Antes da primeira resposta não se afirma que não há nada.
            let text = if self.loading && self.subs.is_empty() && !self.failed { tr("activity_loading") } else { web("atividade_vazio") };
            body = body.child(div().py_4().text_center().text_sm().text_color(theme::faint()).child(text));
        }
        div().id("activity-scroll").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(body)
    }
}

/// Estado do `Hangar` para a aba: a view, a conta do disco e a aba escolhida por sessão.
pub(super) struct ActivityState {
    view: Entity<ActivityPanel>,
    /// Subagentes no disco da sessão aberta; falha mantém o número anterior.
    count: usize,
    count_seq: u64,
    count_busy: bool,
    count_timer: Option<Task<()>>,
    /// Sessões em que a aba escolhida é Atividade.
    chosen: HashSet<SessionKey>,
}

impl ActivityState {
    pub fn new(cx: &mut App) -> Self {
        Self { view: cx.new(|_| ActivityPanel::new()), count: 0, count_seq: 0, count_busy: false, count_timer: None, chosen: HashSet::new() }
    }
}

impl Hangar {
    /// Tarefas, agentes (mesmo os que terminaram), shells rodando, subagentes no disco ou processos vivos.
    pub(super) fn has_activity(&self) -> bool {
        let a = &self.activity;
        !a.tasks.is_empty() || !a.agents.is_empty() || a.running_shells().next().is_some() || self.act.count > 0 || !self.chat.state.shells.is_empty()
    }

    pub(super) fn activity_tab(&self) -> bool {
        self.selected_key().is_some_and(|key| self.act.chosen.contains(&key)) && self.has_activity()
    }

    fn activity_link(&self) -> Option<Link> {
        Some(Link { api: self.api.clone()?, runtime: self.runtime.clone(), tx: self.tx.clone(), connection: self.connection })
    }

    /// Depois de mudar conversa, estado, conta ou aba: a aba sem atividade volta para Contexto (e fica lá), a view recebe
    /// os dados e, se passou a aparecer, relê a lista. Antes de a conversa chegar não há como saber, e a escolha fica.
    pub(super) fn sync_activity(&mut self, cx: &mut Context<Self>) {
        let key = self.selected_key();
        if let Some(key) = &key { if self.history_installed && !self.has_activity() { self.act.chosen.remove(key); } }
        let target = key.filter(|_| self.side.open && self.activity_tab()).zip(self.activity_link());
        let (activity, processes) = (&self.activity, &self.chat.state.shells);
        self.act.view.update(cx, |view, cx| { view.set_data(activity, processes, cx); view.show(target, cx); });
    }

    pub(super) fn choose_side_tab(&mut self, activity: bool, cx: &mut Context<Self>) {
        let Some(key) = self.selected_key() else { return };
        if activity { self.act.chosen.insert(key); } else { self.act.chosen.remove(&key); }
        self.sync_activity(cx);
        cx.notify();
    }

    /// O botão do cabeçalho: abre o painel já na aba Atividade.
    fn open_activity(&mut self, cx: &mut Context<Self>) {
        if !self.side.open { self.toggle_side(cx); }
        self.choose_side_tab(true, cx);
    }

    /// Sessão nova: a conta recomeça e o pedido em voo da anterior é descartado.
    pub(super) fn reset_subagent_count(&mut self) {
        self.act.count = 0;
        self.act.count_seq += 1;
        self.act.count_busy = false;
        self.act.count_timer = None;
    }

    /// Conta uma vez agora e, enquanto a sessão trabalha, a cada 5 s; parar de trabalhar reconta uma vez. Só depois de a
    /// conversa carregar.
    pub(super) fn restart_subagent_count(&mut self, cx: &mut Context<Self>) {
        if !self.history_installed { return; }
        self.count_subagents();
        self.act.count_timer = (self.chat.state.state == "working").then(|| cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(COUNT_EVERY).await;
            if this.update(cx, |this, _| this.count_subagents()).is_err() { break; }
        }));
    }

    /// Um pedido por vez: com um em voo, a batida seguinte não pede outro.
    fn count_subagents(&mut self) {
        if self.act.count_busy { return; }
        let (Some(link), Some(key)) = (self.activity_link(), self.selected_key()) else { return };
        self.act.count_busy = true;
        let seq = self.act.count_seq;
        self.runtime.spawn(async move {
            let result = link.api.read(&key.name, &["subagents"], &[], 15).await.map(|value| value.as_array().map_or(0, Vec::len));
            let _ = link.tx.send(Envelope { connection: link.connection, selection: None, payload: Payload::Activity(ActivityReply::Count(key, seq, result)) }).await;
        });
    }

    pub(super) fn receive_activity(&mut self, reply: ActivityReply, cx: &mut Context<Self>) {
        match reply {
            ActivityReply::Count(key, seq, result) => {
                if seq != self.act.count_seq || self.selected_key().as_ref() != Some(&key) { return; }
                self.act.count_busy = false;
                let Ok(count) = result else { return };
                let before = self.has_activity();
                self.act.count = count;
                // Só a presença da aba e do botão depende da conta: o número dela não aparece.
                if self.has_activity() != before { self.sync_activity(cx); cx.notify(); }
            }
            ActivityReply::List(seq, result) => self.act.view.update(cx, |view, cx| view.receive(seq, result, cx)),
        }
    }

    /// A aba do painel: Atividade (a view própria) ou nada, e o Contexto segue como era.
    pub(super) fn activity_view(&self) -> AnyElement {
        self.act.view.clone().cached(StyleRefinement::default().size_full()).into_any_element()
    }

    /// Abas "Contexto | Atividade" no cabeçalho do painel; sem atividade, só o título de antes.
    pub(super) fn render_side_title(&self, cx: &mut Context<Self>) -> AnyElement {
        if !self.has_activity() { return div().font_weight(FontWeight::SEMIBOLD).child(tr("side_context")).into_any_element(); }
        let on_activity = self.activity_tab();
        let tab = |id: &'static str, label: String, selected: bool, activity: bool, cx: &mut Context<Self>| {
            div().h_full().flex().items_center().border_b_2().border_color(if selected { theme::accent() } else { transparent_black() })
                .child(Button::new(id).custom(ButtonCustomVariant::new(cx).color(transparent_black())
                        .foreground(if selected { theme::text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                    // Mesmo peso do título "Contexto" de antes: as abas chegam sem o texto mudar de lugar nem de peso.
                    .h(px(28.)).px(px(10.)).rounded(px(6.)).when(selected, |b| b.font_weight(FontWeight::SEMIBOLD)).label(label)
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_side_tab(activity, cx))))
        };
        div().h_full().flex().items_center().gap(px(2.)).ml(px(-10.))
            .child(tab("side-tab-context", tr("side_context"), !on_activity, false, cx))
            .child(tab("side-tab-activity", web("ctx_atividade"), on_activity, true, cx))
            .into_any_element()
    }

    /// O botão Atividade do cabeçalho: só com atividade e com o painel fora de vista (à vista, a entrada é a aba, como o
    /// web, que esconde a barra de cima com o painel aberto); número = tarefas em andamento + agentes e shells rodando;
    /// com agente rodando, cor de destaque e o ícone respirando.
    pub(super) fn render_activity_button(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.has_activity() || self.side_shown(window) { return None; }
        let running = self.activity.running_agents().next().is_some();
        let badge = self.activity.badge();
        let label = web("ctx_atividade");
        let color = if running { theme::accent() } else { theme::muted() };
        let icon: AnyElement = if running { chrome::Breathing::new("activity-breath", IconName::ListChecks, px(16.), color).into_any_element() }
            else { Icon::new(IconName::ListChecks).size(px(16.)).text_color(color).into_any_element() };
        Some(div().relative().flex_shrink_0()
            .child(Button::new("activity-open").custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(color)
                    .hover(theme::hover()).active(theme::hover()))
                .w(px(28.)).h(px(28.)).rounded(px(6.)).tooltip(label.clone()).accessibility_label(label).child(icon)
                .on_click(cx.listener(|this, _, _, cx| this.open_activity(cx))))
            // Selo no canto de fora do botão, sem cobrir as linhas do ícone.
            .when(badge > 0, |el| el.child(div().absolute().top(px(-6.)).right(px(-7.)).min_w(px(14.)).h(px(14.)).px(px(3.))
                .rounded_full().bg(theme::accent()).text_color(gpui::white()).text_size(px(9.5)).font_weight(FontWeight::SEMIBOLD)
                .flex().items_center().justify_center().child(badge.to_string())))
            .into_any_element())
    }
}

#[cfg(test)]
mod tests {
    use super::{SubRun, base_title, interval, match_sub, sub_title};

    fn sub(id: &str, prompt: &str) -> SubRun {
        SubRun { agent_id: id.into(), agent_type: None, prompt: Some(prompt.into()), calls: 0, last_tool: None, finished: false, unreadable: false }
    }

    #[test]
    fn subagents_match_by_whole_prompt_then_by_its_head() {
        let long = "x".repeat(500);
        let subs = vec![sub("a", "Pesquise  na web\nfontes"), sub("b", &format!("{long} fim")), sub("c", "outro")];
        assert_eq!(match_sub(&subs, Some("pesquise na web fontes")).map(|s| s.agent_id.as_str()), Some("a"));
        assert_eq!(match_sub(&subs, Some(&format!("{long} diferente"))).map(|s| s.agent_id.as_str()), Some("b"));
        assert_eq!(match_sub(&subs, Some("nada")), None);
        assert_eq!(match_sub(&subs, None), None);
    }

    #[test]
    fn titles_skip_skill_header_and_repeat_with_the_id() {
        assert_eq!(base_title(&sub("a", "Base directory for this skill: /x\n# Título\n\nFaça isto")), "Faça isto");
        assert_eq!(base_title(&sub("a", "")), "a");
        let subs = vec![sub("a", "Mesmo começo\nitem 1"), sub("b", "Mesmo começo\nitem 2"), sub("c", "Outro")];
        assert_eq!(sub_title(&subs, &subs[0]), "Mesmo começo · a");
        assert_eq!(sub_title(&subs, &subs[2]), "Outro");
        assert_eq!((interval(5.), interval(125.), interval(7200.), interval(90_000.)), ("1 min".into(), "2 min".into(), "2 h".into(), "1 d".into()));
    }
}
