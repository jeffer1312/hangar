//! Aba Atividade do painel da direita: o `ActivitySheet` docado do web no nível da lista (agentes rodando, subagentes do
//! disco, shells de fundo, processos vivos e tarefas) e o botão Atividade do cabeçalho. A lista de subagentes é lida por
//! esta view, em pedidos numerados, e a chegada redesenha só ela; a conta de subagentes no disco, que decide se a aba e o
//! botão existem, é do `Hangar`, como no `Chat.svelte`.
use super::*;
use super::subagent::SubConversation;
use crate::conversation::{Activity, AgentRun, TaskStatus};
use gpui_kit::component::tooltip::Tooltip;

/// Recontagem dos subagentes no disco enquanto a sessão trabalha.
const COUNT_EVERY: Duration = Duration::from_secs(5);
/// O "há N min" dos shells anda de 20 em 20 s, e só com a aba à vista.
const CLOCK_EVERY: Duration = Duration::from_secs(20);
/// Consulta do subagente aberto, como o `ActivitySheet`.
const DETAIL_EVERY: Duration = Duration::from_millis(2500);
/// Falhas seguidas que param a consulta; o 404 para na primeira.
const DETAIL_FAILS: u32 = 3;

pub(super) enum ActivityReply {
    /// Quantos subagentes a sessão tem no disco, amarrado à sessão e ao pedido.
    Count(SessionKey, u64, Result<usize, Failure>),
    /// A lista da aba, com o número do pedido.
    List(u64, Result<Vec<SubRun>, Failure>),
    /// O subagente aberto (`…/subagents/{id}?events=200`), com o número do pedido.
    Detail(u64, Result<Value, Failure>),
}

/// Um subagente do disco (`GET …/subagents`), só com o que a lista mostra e casa.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SubRun {
    agent_id: String, agent_type: Option<String>, prompt: Option<String>, calls: u64, last_tool: Option<String>,
    finished: bool, unreadable: bool, tools: Vec<(String, u64)>,
}

fn parse_sub(item: &Value) -> Option<SubRun> {
    let text = |item: &Value, key: &str| item.get(key).and_then(Value::as_str).map(str::to_owned);
    Some(SubRun {
        agent_id: text(item, "agentId")?,
        agent_type: text(item, "agentType").filter(|t| !t.is_empty()),
        prompt: text(item, "prompt"),
        calls: item.get("toolCalls").and_then(Value::as_u64).unwrap_or(0),
        last_tool: item.get("recent").and_then(Value::as_array).and_then(|recent| recent.last()).and_then(|last| text(last, "name")),
        finished: item.get("finished").and_then(Value::as_bool).unwrap_or(false),
        unreadable: item.get("ilegivel").and_then(Value::as_bool).unwrap_or(false),
        tools: item.get("tools").and_then(Value::as_array).map(|tools| tools.iter()
            .filter_map(|t| Some((text(t, "name")?, t.get("count").and_then(Value::as_u64).unwrap_or(0)))).collect()).unwrap_or_default(),
    })
}

fn parse_subs(value: &Value) -> Vec<SubRun> {
    value.as_array().map(|list| list.iter().filter_map(parse_sub).collect()).unwrap_or_default()
}

pub(super) fn web(key: &str) -> String { crate::i18n::tr_web(key, &HashMap::new()).unwrap_or_else(|| key.to_owned()) }
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

/// Terminou: o backend diz (Kimi, Pi) ou o Agent do pai que o lançou já teve o fim real — o registro do Claude não traz
/// `finished`. O Agent é achado pelo `agentId` do resultado; sem ele, pelo prompt, e só quando um Agent e um subagente
/// casam um com o outro e mais ninguém: ambíguo não afirma o fim.
fn sub_done(agents: &[AgentRun], subs: &[SubRun], run: &SubRun) -> bool {
    if run.finished { return true; }
    if let Some(agent) = agents.iter().find(|a| a.agent_id.as_deref() == Some(run.agent_id.as_str())) { return !agent.running; }
    let subs_of = |prompt: Option<&str>| subs.iter().filter(|s| match_sub(std::slice::from_ref(*s), prompt).is_some()).count();
    let mut by_prompt = agents.iter().filter(|a| a.agent_id.is_none() && match_sub(std::slice::from_ref(run), a.prompt.as_deref()).is_some());
    match (by_prompt.next(), by_prompt.next()) {
        (Some(agent), None) => !agent.running && subs_of(agent.prompt.as_deref()) == 1,
        _ => false,
    }
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
struct AgentLine { key: String, description: String, tags: Vec<(String, bool)>, now: Option<String>, sub: Option<String> }
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

/// O subagente aberto no detalhe: o que a lista sabia dele até a primeira resposta, depois o que a consulta traz.
/// `done` é o fim mostrado: o do backend ou o do Agent que o lançou.
struct Opened { run: SubRun, title: String, raw: Option<Value>, loaded: bool, has_events: bool, fails: u32, busy: bool, done: bool }

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
    opened: Option<Opened>,
    detail_seq: u64,
    poll: Option<Task<()>>,
    /// Aviso do detalhe ("parou", "não achei"), em cima de tudo, como o `subError` do web.
    notice: Option<String>,
    /// Clique num cartão Agent antes de a lista chegar: prompt e título.
    pending: Option<(Option<String>, String)>,
    conversation: Entity<SubConversation>,
    /// O ‹ do detalhe: quem abriu pelo teclado cai nele, e Enter volta.
    back_focus: FocusHandle,
}

impl ActivityPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self { link: None, key: None, shown: false, activity: Activity::default(), processes: Vec::new(), subs: Vec::new(), seq: 0,
            loading: false, failed: false, lines: Lines::default(), clock: None, scroll: ScrollHandle::new(), opened: None, detail_seq: 0,
            poll: None, notice: None, pending: None, conversation: cx.new(|_| SubConversation::new()), back_focus: cx.focus_handle().tab_stop(true) }
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
            AgentLine { key: a.id.clone(), description: a.description.clone(), tags, now: sub.map(calls_line), sub: sub.map(|s| s.agent_id.clone()) }
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
        // O Agent do pai terminou com o detalhe aberto: uma última leitura traz o fim, e a resposta dela para a consulta.
        if self.poll.is_some() && self.opened.as_ref().is_some_and(|o| !o.done && self.done(&o.run)) { self.fetch_detail(); }
    }

    fn done(&self, run: &SubRun) -> bool { sub_done(&self.activity.agents, &self.subs, run) }

    /// A aba passou a aparecer (ou mudou de sessão): relê a lista. `None` = escondida, o relógio para.
    fn show(&mut self, target: Option<(SessionKey, Link)>, cx: &mut Context<Self>) {
        let Some((key, link)) = target else {
            self.shown = false;
            self.clock = None;
            // Sair da aba mata o pedido do cartão e a consulta, e a volta começa na lista.
            self.close_detail(cx);
            (self.notice, self.pending) = (None, None);
            return;
        };
        let other = self.key.as_ref() != Some(&key);
        if other || !self.shown {
            // Aba que volta a aparecer relê do zero, como o web, que desmonta a lista ao trocar de aba; resposta da
            // sessão anterior não entra na desta.
            self.seq += 1;
            (self.subs, self.failed, self.loading) = (Vec::new(), false, false);
            if other { self.close_detail(cx); (self.notice, self.pending) = (None, None); }
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
        // Pedido do cartão que esperava a lista: com a lista falhando, o aviso dela já diz por quê.
        if let Some((prompt, title)) = self.pending.take().filter(|_| !self.failed) { self.resolve(prompt.as_deref(), title, cx); }
        cx.notify();
    }

    /// Clique no cartão Agent da conversa: abre a conversa daquele agente, casada pelo prompt. Sem a lista ainda,
    /// espera por ela (e a pede de novo se ela não está a caminho).
    pub(super) fn request_agent(&mut self, prompt: Option<String>, title: String, cx: &mut Context<Self>) {
        if self.subs.is_empty() {
            self.pending = Some((prompt, title));
            if !self.loading { self.load(cx); }
            return;
        }
        self.resolve(prompt.as_deref(), title, cx);
    }

    fn resolve(&mut self, prompt: Option<&str>, title: String, cx: &mut Context<Self>) {
        match match_sub(&self.subs, prompt).cloned() {
            Some(run) => self.open_sub(run, title, cx),
            None => {
                // Sem avisar, a tela ficaria no agente anterior: a conversa errada com cara de certa.
                self.close_detail(cx);
                self.notice = Some(web("atividade_agente_nao_achado"));
                cx.notify();
            }
        }
    }

    fn open_sub(&mut self, run: SubRun, title: String, cx: &mut Context<Self>) {
        self.close_detail(cx);
        self.notice = None;
        let done = self.done(&run);
        self.opened = Some(Opened { run, title, raw: None, loaded: false, has_events: false, fails: 0, busy: false, done });
        self.poll = Some(cx.spawn(async move |this, cx| loop {
            if this.update(cx, |this, _| this.fetch_detail()).is_err() { break; }
            cx.background_executor().timer(DETAIL_EVERY).await;
        }));
        cx.notify();
    }

    /// Volta para a lista e esquece o subagente: resposta dele que ainda chegar é descartada.
    fn close_detail(&mut self, cx: &mut Context<Self>) {
        self.detail_seq += 1;
        self.poll = None;
        if self.opened.take().is_some() { self.conversation.update(cx, |view, _| view.clear()); }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        self.close_detail(cx);
        cx.notify();
    }

    /// Uma batida da consulta: com um pedido em voo, a batida não pede outro.
    fn fetch_detail(&mut self) {
        let (Some(link), Some(key)) = (self.link.clone(), self.key.clone()) else { return };
        let Some(opened) = self.opened.as_mut().filter(|o| !o.busy) else { return };
        opened.busy = true;
        let (seq, id) = (self.detail_seq, opened.run.agent_id.clone());
        link.runtime.spawn(async move {
            let result = link.api.read(&key.name, &["subagents", &id], &[("events", "200")], 15).await;
            let _ = link.tx.send(Envelope { connection: link.connection, selection: None, payload: Payload::Activity(ActivityReply::Detail(seq, result)) }).await;
        });
    }

    fn receive_detail(&mut self, seq: u64, result: Result<Value, Failure>, cx: &mut Context<Self>) {
        if seq != self.detail_seq { return; }
        let Some(opened) = self.opened.as_mut() else { return };
        opened.busy = false;
        match result {
            Ok(value) => {
                let run = parse_sub(&value).unwrap_or_else(|| opened.run.clone());
                let done = self.done(&run);
                // Terminou: esta leitura já traz o fim, e a consulta para aqui.
                if done { self.poll = None; }
                let Some(opened) = self.opened.as_mut() else { return };
                opened.fails = 0;
                let first = !opened.loaded;
                opened.loaded = true;
                let cleared = self.notice.take().is_some();
                // Resposta igual à anterior não pede quadro.
                if opened.raw.as_ref() == Some(&value) && opened.done == done { if first || cleared { cx.notify(); } return; }
                let events: Vec<ChatEvent> = value.get("events").cloned().and_then(|e| serde_json::from_value(e).ok()).unwrap_or_default();
                let has_events = !events.is_empty();
                let changed = first || cleared || opened.run != run || opened.has_events != has_events || opened.done != done;
                (opened.run, opened.raw, opened.has_events, opened.done) = (run, Some(value), has_events, done);
                self.conversation.update(cx, |view, cx| view.set_events(events, done, cx));
                if changed { cx.notify(); }
            }
            Err(failure) => {
                // Falha isolada é normal (o arquivo some quando o agente termina) e mantém o último estado; o 404 é
                // definitivo (a sessão morreu por baixo do painel) e a terceira seguida é erro de verdade.
                opened.fails += 1;
                if failure.status == Some(404) || opened.fails >= DETAIL_FAILS {
                    opened.loaded = true;
                    self.poll = None;
                    self.notice = Some(web("atividade_erro_vivo"));
                    cx.notify();
                }
            }
        }
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

impl ActivityPanel {
    /// Views guardadas dentro desta, para quem guarda esta (`panes::cached_selectable`).
    pub(super) fn views(&self) -> Vec<EntityId> { vec![self.conversation.entity_id()] }
}

impl Render for ActivityPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::panes::rendered(cx.entity_id(), window, cx);
        if self.opened.is_some() { return self.render_detail(window, cx); }
        let lines = &self.lines;
        let mut body = div().px_4().py(px(14.)).flex().flex_col().gap_4();
        if self.failed {
            body = body.child(warning_line(web("atividade_erro_subagentes")));
        }
        if let Some(notice) = &self.notice { body = body.child(warning_line(notice.clone())); }
        if !lines.agents.is_empty() {
            let rows: Vec<AnyElement> = lines.agents.iter().map(|a| {
                let content = div().flex().items_start().gap_2().child(spinning(format!("act-agent-spin-{}", a.key)))
                    .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                        .child(div().flex().items_center().gap_1().min_w_0()
                            .child(div().min_w_0().truncate().text_sm().text_color(theme::text()).child(a.description.clone()))
                            .children(a.tags.iter().map(|(t, muted)| tag(t.clone(), *muted))))
                        .when_some(a.now.clone(), |el, now| el.child(now_text(now))));
                // Só abre quem casou com o disco; o resto é informação.
                match &a.sub {
                    Some(sub) => openable(format!("act-agent-{}", a.key), sub.clone(), a.description.clone(), content, window, cx),
                    None => content.into_any_element(),
                }
            }).collect();
            body = body.child(section(web("atividade_rodando_agora"), None).children(rows));
        }
        if !lines.orphans.is_empty() {
            let rows: Vec<AnyElement> = lines.orphans.iter().map(|s| {
                let content = div().flex().flex_col().gap(px(3.)).when(s.done, |el| el.opacity(0.6))
                    .child(div().flex().items_center().gap_1().min_w_0()
                        .child(div().min_w_0().truncate().text_sm().text_color(theme::text()).child(s.title.clone()))
                        .when_some(s.tag.clone(), |el, t| el.child(tag(t, false))))
                    .child(now_text(s.now.clone()));
                openable(format!("act-orphan-{}", s.key), s.key.clone(), s.title.clone(), content, window, cx)
            }).collect();
            body = body.child(section(web("atividade_subagentes"), None).children(rows));
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
        div().id("activity-scroll").size_full().overflow_y_scroll().track_scroll(&self.scroll).child(body).into_any_element()
    }
}

fn warning_line(text: String) -> AnyElement {
    div().text_xs().text_color(theme::warning()).child(format!("⚠ {text}")).into_any_element()
}

/// Linha da lista que abre a conversa do subagente: foco pelo Tab, Enter ou espaço abrem, › à direita como o web.
fn openable(id: String, agent_id: String, title: String, content: Div, window: &mut Window, cx: &mut Context<ActivityPanel>) -> AnyElement {
    let focus = window.use_keyed_state(SharedString::from(format!("{id}-focus")), cx, |_, cx| cx.focus_handle().tab_stop(true)).read(cx).clone();
    let label = format!("{}: {title}", web("tool_abrir_agente"));
    let (click_id, click_title) = (agent_id.clone(), title.clone());
    div().id(SharedString::from(id)).track_focus(&focus).flex().items_center().gap_2().mx(px(-8.)).px(px(8.)).py(px(4.)).rounded(px(8.))
        .cursor_pointer().hover(|el| el.bg(theme::hover()))
        .when(focus.is_focused(window), |el| el.focus_ring_style(window, cx))
        .role(Role::Button).aria_label(label)
        .child(content.flex_1().min_w_0())
        .child(chrome::small_icon(IconName::ChevronRight, 14., theme::faint()).flex_shrink_0())
        .on_click(cx.listener(move |this, _, _, cx| this.open_listed(&click_id, click_title.clone(), cx)))
        .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                this.open_listed(&agent_id, title.clone(), cx);
                // A linha focada some com a lista: o foco passa ao ‹, e o caminho de volta fica a um Enter.
                if this.opened.is_some() { this.back_focus.focus(window, cx); }
                cx.stop_propagation();
            }
        }))
        .into_any_element()
}

impl ActivityPanel {
    /// Linha da lista: o subagente já é conhecido, sem casar de novo pelo texto.
    fn open_listed(&mut self, agent_id: &str, title: String, cx: &mut Context<Self>) {
        if let Some(run) = self.subs.iter().find(|s| s.agent_id == agent_id).cloned() { self.open_sub(run, title, cx); }
    }

    /// Detalhe: ‹ e o título, o estado dele, a conversa (só ela rola), as ferramentas e o rodapé de só leitura.
    fn render_detail(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(opened) = &self.opened else { return div().into_any_element() };
        let run = &opened.run;
        let small = |text: String, color: Hsla| div().child(text).text_color(color).into_any_element();
        let meta: Vec<AnyElement> = if run.unreadable { vec![small(web("atividade_sub_ilegivel"), theme::muted())] } else {
            let mut meta = vec![
                if opened.done { small(format!("✓ {}", web("atividade_sub_concluido")), theme::success()) }
                else { small(format!("◐ {}", web("atividade_rodando")), theme::accent()) },
                small(web_with("atividade_chamadas", "n", run.calls.to_string()), theme::muted()),
            ];
            if let Some(t) = &run.agent_type { meta.push(small(t.clone(), theme::muted())); }
            if let Some(t) = &run.last_tool { meta.push(div().font_family(theme::MONO).text_color(theme::faint()).child(t.clone()).into_any_element()); }
            meta
        };
        let empty = |text: String| div().px_4().py_4().text_center().text_sm().text_color(theme::faint()).child(text).into_any_element();
        let body = if !opened.loaded { empty(tr("subagent_loading")) }
            else if opened.has_events {
                super::panes::cached_selectable(self.conversation.clone().into(), Vec::new(), StyleRefinement::default().size_full())
            }
            // Já chamou ferramentas (ou o registro não foi lido): é falha de leitura, não agente parado.
            else if run.unreadable || run.calls > 0 { empty(web("atividade_erro_transcript")) }
            else { empty(web("atividade_pensando")) };
        let title = if opened.title.trim().is_empty() { web("atividade_subagente") } else { opened.title.clone() };
        let back = web("comum_voltar");
        let session = self.key.as_ref().map(|k| k.name.clone()).unwrap_or_default();
        div().size_full().flex().flex_col()
            .child(div().flex_shrink_0().px(px(10.)).pt(px(10.)).pb(px(4.)).flex().items_center().gap_1()
                .child(div().id("act-back").track_focus(&self.back_focus).size(px(28.)).flex_shrink_0().rounded(px(6.))
                    .flex().items_center().justify_center().cursor_pointer().hover(|el| el.bg(theme::hover()))
                    .when(self.back_focus.is_focused(window), |el| el.focus_ring_style(window, cx))
                    .role(Role::Button).aria_label(back.clone())
                    .tooltip(move |window, cx| Tooltip::new(back.clone()).build(window, cx))
                    .child(chrome::small_icon(IconName::ChevronLeft, 16., theme::text()))
                    .on_click(cx.listener(|this, _, _, cx| this.back(cx)))
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if matches!(event.keystroke.key.as_str(), "enter" | "space") { this.back(cx); cx.stop_propagation(); }
                    })))
                .child(div().flex_1().min_w_0().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(title)))
            .when_some(self.notice.clone(), |el, notice| el.child(div().flex_shrink_0().px_4().pb_2().child(warning_line(notice))))
            .child(div().flex_shrink_0().px_4().pb(px(10.)).flex().flex_wrap().gap_x(px(10.)).gap_y(px(2.)).text_xs().children(meta))
            .child(div().flex_1().min_h_0().border_t_1().border_color(theme::border()).child(body))
            .when(!run.tools.is_empty(), |el| el.child(div().flex_shrink_0().px_4().py(px(10.)).border_t_1().border_color(theme::border())
                .flex().flex_col().gap_2()
                .child(chrome::section_label(web_with("atividade_ferramentas_chamadas", "n", run.calls.to_string())))
                .child(div().flex().flex_wrap().gap_1().children(run.tools.iter().map(|(name, n)| tag(format!("{name} ×{n}"), false))))))
            .child(div().flex_shrink_0().px_4().py(px(8.)).border_t_1().border_color(theme::border()).text_xs().text_color(theme::faint())
                .child(web_with("atividade_conversa_so_leitura", "nome", session)))
            .into_any_element()
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
        Self { view: cx.new(ActivityPanel::new), count: 0, count_seq: 0, count_busy: false, count_timer: None, chosen: HashSet::new() }
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
            ActivityReply::Detail(seq, result) => self.act.view.update(cx, |view, cx| view.receive_detail(seq, result, cx)),
        }
    }

    /// A conversa do subagente segue a Aparência, como a principal.
    pub(super) fn restyle_subagent(&mut self, cx: &mut Context<Self>) {
        let conversation = self.act.view.read(cx).conversation.clone();
        conversation.update(cx, |view, cx| view.restyle(cx));
    }

    /// Clique no cartão Agent da conversa: abre o painel na aba Atividade, já na conversa desse agente. Cada clique é um
    /// pedido novo, então clicar de novo no mesmo agente depois do ‹ reabre.
    pub(super) fn open_agent(&mut self, (prompt, title): (Option<String>, String), cx: &mut Context<Self>) {
        self.open_activity(cx);
        self.act.view.update(cx, |view, cx| view.request_agent(prompt, title, cx));
    }
}

/// O cartão Agent (só ele) abre a conversa do agente: prompt, que casa com o disco, e o título do detalhe.
pub(super) fn agent_request(call: &ChatEvent) -> Option<(Option<String>, String)> {
    if call.tool_name.as_deref() != Some("Agent") { return None; }
    let text = |key: &str| call.tool_input.as_ref().and_then(|i| i.get(key)).and_then(Value::as_str).map(str::to_owned);
    Some((text("prompt"), text("description").filter(|d| !d.trim().is_empty()).unwrap_or_else(|| web("atividade_subagente"))))
}

impl Hangar {

    /// A aba do painel: Atividade (a view própria) ou nada, e o Contexto segue como era.
    pub(super) fn activity_view(&self, cx: &App) -> AnyElement {
        let nested = self.act.view.read(cx).views();
        super::panes::cached_selectable(self.act.view.clone().into(), nested, StyleRefinement::default().size_full())
    }

    /// A aba Atividade e as views guardadas dentro dela, que o painel guardado leva junto.
    pub(super) fn activity_views(&self, cx: &App) -> Vec<EntityId> {
        let mut views = self.act.view.read(cx).views();
        views.push(self.act.view.entity_id());
        views
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
    use super::{AgentRun, SubRun, base_title, interval, match_sub, sub_done, sub_title};

    fn parent(id: &str, prompt: &str, agent_id: Option<&str>, running: bool) -> AgentRun {
        AgentRun { call: 0, id: id.into(), description: String::new(), subagent_type: None, model: None, prompt: Some(prompt.into()),
            running, agent_id: agent_id.map(Into::into) }
    }

    #[test]
    fn claude_subagent_ends_with_its_parent_agent_by_id_or_by_a_unique_prompt() {
        let subs = vec![sub("a", "tarefa um"), sub("b", "tarefa dois"), sub("c", "tarefa dois")];
        // Pelo agentId, mesmo com o prompt repetido.
        assert!(sub_done(&[parent("t1", "tarefa dois", Some("b"), false)], &subs, &subs[1]));
        assert!(!sub_done(&[parent("t1", "tarefa dois", Some("b"), true)], &subs, &subs[1]));
        // Pelo prompt só quando um Agent e um subagente casam entre si.
        assert!(sub_done(&[parent("t1", "tarefa um", None, false)], &subs, &subs[0]));
        assert!(!sub_done(&[parent("t1", "tarefa um", None, true)], &subs, &subs[0]));
        assert!(!sub_done(&[parent("t1", "tarefa dois", None, false)], &subs, &subs[1]));
        assert!(!sub_done(&[parent("t1", "tarefa um", None, false), parent("t2", "tarefa um", None, false)], &subs, &subs[0]));
        // Agent que já se sabe de outro subagente não fecha este; o `finished` do backend fecha sozinho.
        assert!(!sub_done(&[parent("t1", "tarefa um", Some("z"), false)], &subs, &subs[0]));
        assert!(sub_done(&[], &subs, &SubRun { finished: true, ..sub("k", "kimi") }));
    }

    fn sub(id: &str, prompt: &str) -> SubRun {
        SubRun { agent_id: id.into(), agent_type: None, prompt: Some(prompt.into()), calls: 0, last_tool: None, finished: false, unreadable: false, tools: Vec::new() }
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
