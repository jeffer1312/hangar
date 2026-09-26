//! Conversa só leitura de um subagente, no detalhe da aba Atividade: as linhas da conversa (bolha do usuário, resposta
//! em markdown, cabeçalho recolhível de ferramenta e pensamento) montadas quando a consulta traz novidade. É uma view
//! própria: a chegada de uma resposta igual não pede quadro, e uma diferente só remede as linhas que mudaram.
use super::*;

fn chips() -> bool { appearance::get().tool_look == appearance::ToolLook::Chips }
fn tree() -> bool { appearance::get().tool_look == appearance::ToolLook::Tree }

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tone { Muted, Accent, Warning }

impl Tone {
    fn color(self) -> Hsla {
        match self { Tone::Muted => theme::muted(), Tone::Accent => theme::accent(), Tone::Warning => theme::warning() }
    }
}

/// Uma chamada (ou um resultado sem chamada) com o cabeçalho já escrito.
#[derive(Clone, Debug, PartialEq)]
struct ToolRow { key: String, call: usize, result: Option<usize>, orphan: bool, name: String, summary: String, status: String, tone: Tone }

#[derive(Clone, Debug, PartialEq)]
enum Part { Thought(String, String), Tool(ToolRow) }

/// Uma linha preparada quando a conversa muda; o desenho só lê.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Message { id: String, markdown: String, user: bool, label: Option<(String, bool)> },
    Tool(ToolRow),
    Group { id: String, label: String, summary: String, status: String, tone: Tone, tools: Vec<ToolRow> },
    Thinking { id: String, summary: String, count: Option<String>, parts: Vec<Part> },
}

impl Row {
    fn id(&self) -> &str {
        match self { Row::Message { id, .. } | Row::Group { id, .. } | Row::Thinking { id, .. } => id, Row::Tool(tool) => &tool.key }
    }
}

struct Rich { source: String, view: Entity<TextViewState>, _observer: Subscription }

pub(super) struct SubConversation {
    events: Vec<ChatEvent>,
    finished: bool,
    rows: Vec<Row>,
    expanded: HashSet<String>,
    rich: HashMap<String, Rich>,
    list: ListState,
}

/// Rodando só enquanto o subagente não terminou e nenhuma mensagem veio depois da chamada, como na conversa.
fn tool_row(events: &[ChatEvent], call: usize, result: Option<usize>, finished: bool, last: Option<usize>) -> ToolRow {
    let event = &events[call];
    let name = event.tool_name.clone().unwrap_or_else(|| tr("tool"));
    let summary = conversation::summarize_input(event.tool_name.as_deref(), event.tool_input.as_ref());
    let (status, tone) = match result.map(|i| &events[i]) {
        Some(r) if r.is_error == Some(true) => {
            let line = r.result.as_deref().and_then(|t| t.lines().map(str::trim).find(|l| !l.is_empty()));
            (line.map(|l| conversation::one_line(l, 72)).unwrap_or_else(|| tr("tool_failed")), Tone::Warning)
        }
        Some(r) => (match count_lines(r) { 0 => tr("tool_done"), 1 => tr("tool_line"), n => tr("tool_lines").replace("{n}", &n.to_string()) }, Tone::Muted),
        None if !finished && last.is_none_or(|last| call > last) => (tr("tool_running"), Tone::Accent),
        None => (tr("tool_no_result"), Tone::Muted),
    };
    ToolRow { key: event.id.clone(), call, result, orphan: false, name, summary, status, tone }
}

fn prepare(events: &[ChatEvent], finished: bool) -> Vec<Row> {
    let view = conversation::View { thinking: appearance::get().thinking_tools, tasks: false, merge_thinking: tree() };
    let items = conversation::build(events, view, &HashSet::new());
    let paired = conversation::pair_results(events).0;
    let last = events.iter().rposition(|e| e.kind == "assistant_msg" || e.kind == "user_msg");
    let tool = |t: &Tool| tool_row(events, t.call, t.result, finished, last);
    items.iter().filter_map(|item| Some(match item {
        Item::Event(i) => {
            let event = &events[*i];
            let error = event.is_error == Some(true);
            let user = event.kind == "user_msg";
            let label = (!user && (event.kind != "assistant_msg" || error)).then(|| (match event.kind.as_str() {
                "assistant_msg" => tr("assistant"), "notice" => tr("notice"), _ => tr("unknown"),
            }, error));
            Row::Message { id: event.id.clone(), markdown: render_source(event), user, label }
        }
        Item::Tool(t) => Row::Tool(tool(t)),
        Item::Orphan(i) => {
            let event = &events[*i];
            let first = event.result.as_deref().unwrap_or("").lines().map(str::trim).find(|l| !l.is_empty()).map(|l| conversation::one_line(l, 96)).unwrap_or_default();
            let tone = if event.is_error == Some(true) { Tone::Warning } else { Tone::Muted };
            Row::Tool(ToolRow { key: event.id.clone(), call: *i, result: Some(*i), orphan: true, name: tr("tool_orphan"), summary: first, status: String::new(), tone })
        }
        Item::Group { id, tools } => {
            let rows: Vec<ToolRow> = tools.iter().map(tool).collect();
            // A Árvore põe o raciocínio no grupo: título, resumo e estado guardados contam só as chamadas.
            let calls: Vec<Tool> = tools.iter().copied().filter(|t| events[t.call].kind != "thinking").collect();
            let counted: Vec<&ToolRow> = rows.iter().filter(|r| events[r.call].kind != "thinking").collect();
            let mut distinct: Vec<&str> = counted.iter().map(|r| r.name.as_str()).collect();
            distinct.dedup();
            // O título conta por família, como o dos Chips: "Rodou 2 comandos · leu 1 arquivo".
            let label = super::rows::family_title(events, &calls);
            let summary = if distinct.len() == 1 { counted.last().map(|r| r.summary.clone()).unwrap_or_default() }
                else { conversation::one_line(&distinct.join(", "), 96) };
            let errors = counted.iter().filter(|r| r.tone == Tone::Warning).count();
            let (status, tone) = if errors > 0 { (super::rows::failed_count(errors), Tone::Warning) }
                else if counted.iter().any(|r| r.tone == Tone::Accent) { (tr("tool_running"), Tone::Accent) } else { (String::new(), Tone::Muted) };
            Row::Group { id: id.clone(), label, summary, status, tone, tools: rows }
        }
        Item::Thinking { id, parts } => {
            let thoughts: Vec<&ChatEvent> = parts.iter().map(|&i| &events[i]).filter(|e| e.kind == "thinking").collect();
            let summary = conversation::thought_summary(thoughts.first().and_then(|e| e.text.as_deref()).unwrap_or(""));
            let calls: Vec<&ChatEvent> = parts.iter().map(|&i| &events[i]).filter(|e| e.kind != "thinking" && e.tool_name.as_deref() != Some("ToolSearch")).collect();
            let searches = calls.iter().all(|e| conversation::is_search(e.tool_name.as_deref()));
            let count = match (calls.len(), searches) {
                (0, _) => None,
                (1, true) => Some(tr("thinking_search")),
                (n, true) => Some(tr("thinking_searches").replace("{n}", &n.to_string())),
                (1, false) => Some(tr("thinking_call")),
                (n, false) => Some(tr("thinking_calls").replace("{n}", &n.to_string())),
            };
            let parts = parts.iter().filter(|&&i| events[i].tool_name.as_deref() != Some("ToolSearch")).map(|&i| {
                let event = &events[i];
                if event.kind == "thinking" { Part::Thought(format!("{}:thought", event.id), safe_markdown(event.text.as_deref().unwrap_or(""))) }
                else { Part::Tool(tool_row(events, i, paired.get(&i).copied(), finished, last)) }
            }).collect();
            Row::Thinking { id: id.clone(), summary, count, parts }
        }
        Item::Tasks { .. } => return None,
    })).collect()
}

/// A linha dona da chave: id igual, senão o id mais longo seguido de `:` — o backend dá `U`, `U:1`, `U:2`… às partes da
/// mesma mensagem, e `U:1:input` é da linha `U:1`, não da `U`.
fn owner(rows: &[Row], key: &str) -> Option<usize> {
    rows.iter().enumerate().filter(|(_, r)| key.strip_prefix(r.id()).is_some_and(|rest| rest.is_empty() || rest.starts_with(':')))
        .max_by_key(|(_, r)| r.id().len()).map(|(i, _)| i)
}

impl SubConversation {
    pub fn new() -> Self {
        Self { events: Vec::new(), finished: false, rows: Vec::new(), expanded: HashSet::new(), rich: HashMap::new(),
            list: ListState::new(0, ListAlignment::Bottom, px(200.)) }
    }

    /// A Aparência mudou: refaz as linhas (pensamento com ou sem ferramentas) e remede todas (Clássico ou Chips).
    pub fn restyle(&mut self, cx: &mut Context<Self>) {
        let events = std::mem::take(&mut self.events);
        self.set_events(events, self.finished, cx);
        self.list.remeasure();
        cx.notify();
    }

    /// Outro subagente: nada do anterior fica.
    pub fn clear(&mut self) {
        (self.events, self.rows, self.finished) = (Vec::new(), Vec::new(), false);
        self.expanded.clear();
        self.rich.clear();
        self.list.reset(0);
    }

    /// Conversa nova do mesmo subagente: só o trecho que mudou entra na lista, e linha igual não é remedida.
    pub fn set_events(&mut self, events: Vec<ChatEvent>, finished: bool, cx: &mut Context<Self>) {
        let rows = prepare(&events, finished);
        (self.events, self.finished) = (events, finished);
        // Só o estado do subagente mudou (chamadas, fim): as linhas são as mesmas e nada se redesenha.
        if rows == self.rows { return; }
        let old: HashMap<&str, &Row> = self.rows.iter().map(|r| (r.id(), r)).collect();
        let resized: Vec<usize> = rows.iter().enumerate().filter(|(_, r)| old.get(r.id()).is_some_and(|o| *o != *r)).map(|(i, _)| i).collect();
        let prefix = self.rows.iter().zip(&rows).take_while(|(a, b)| a.id() == b.id()).count();
        let suffix = self.rows[prefix..].iter().rev().zip(rows[prefix..].iter().rev()).take_while(|(a, b)| a.id() == b.id()).count();
        if prefix + suffix < self.rows.len() || prefix + suffix < rows.len() {
            self.list.splice(prefix..self.rows.len() - suffix, rows.len() - prefix - suffix);
        }
        for i in resized { self.list.remeasure_items(i..i + 1); }
        self.rows = rows;
        let ids: HashSet<&str> = self.rows.iter().map(Row::id).collect();
        self.rich.retain(|key, _| ids.iter().any(|id| key.starts_with(id)));
        self.expanded.retain(|key| self.events.iter().any(|e| &e.id == key)
            || ids.contains(key.strip_suffix(":more").unwrap_or(key)));
        cx.notify();
    }

    fn remeasure(&mut self, key: &str) {
        if let Some(i) = owner(&self.rows, key) { self.list.remeasure_items(i..i + 1); }
    }

    fn text(&mut self, key: String, source: String, cx: &mut Context<Self>) -> Entity<TextViewState> {
        if let Some(rich) = self.rich.get_mut(&key) {
            if rich.source != source {
                rich.view.update(cx, |view, cx| view.set_text(&source, cx));
                rich.source = source;
            }
            return rich.view.clone();
        }
        let view = cx.new(|cx| TextViewState::markdown(&source, cx));
        let owner = key.clone();
        // O parse que termina muda a altura da linha dona do texto.
        let observer = cx.observe(&view, move |this, _, _| this.remeasure(&owner));
        self.rich.insert(key, Rich { source, view: view.clone(), _observer: observer });
        view
    }

    fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.expanded.remove(&key) { self.expanded.insert(key.clone()); }
        self.remeasure(&key);
        // Chamada aberta dentro de um grupo ou pensamento: a linha é a do bloco.
        if let Some(i) = self.rows.iter().position(|r| match r {
            Row::Group { tools, .. } => tools.iter().any(|t| t.key == key),
            Row::Thinking { parts, .. } => parts.iter().any(|p| matches!(p, Part::Tool(t) if t.key == key)),
            _ => false,
        }) { self.list.remeasure_items(i..i + 1); }
        cx.notify();
    }

    fn header(&self, key: &str, cx: &mut Context<Self>) -> Button {
        let open = self.expanded.contains(key);
        let toggle = key.to_owned();
        Button::new(SharedString::from(format!("sub-toggle-{key}"))).ghost().small().w_full().toggled(open)
            .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx)))
    }

    fn detail(&mut self, key: String, full: String, label: String, error: bool, cx: &mut Context<Self>) -> AnyElement {
        let Prepared::Detail { fenced, total, clipped, .. } = prepare_detail(full) else { return div().into_any_element() };
        let view = self.text(key, fenced, cx);
        let note = clipped.then(|| tr("clipped").replace("{shown}", &DETAIL_MAX.to_string()).replace("{total}", &total.to_string()));
        div().flex().flex_col().gap_1()
            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::muted() }).child(label))
            .child(TextView::new(&view).selectable(true).scrollable(false))
            .when_some(note, |el, note| el.child(div().text_xs().text_color(theme::muted()).child(note)))
            .into_any_element()
    }

    fn render_tool(&mut self, tool: &ToolRow, cx: &mut Context<Self>) -> AnyElement {
        // Nos Chips, a conversa do subagente segue o desenho da principal, como o `MessageList` do detalhe no web.
        if chips() && !tool.orphan { return super::rows::chip_box().child(self.render_chip(tool, cx)).into_any_element(); }
        let error = tool.tone == Tone::Warning;
        let header = self.header(&tool.key, cx)
            .accessibility_label(format!("{}: {}. {}", tool.name, tool.summary, tool.status))
            .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(if error { theme::warning() } else { theme::text() }).child(tool.name.clone()))
            .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(tool.summary.clone()))
            .when(!tool.status.is_empty(), |el| el.child(div().flex_shrink_0().max_w(px(140.)).truncate().text_color(tool.tone.color()).child(tool.status.clone())));
        if !self.expanded.contains(&tool.key) { return header.into_any_element(); }
        let body = self.tool_body(tool, cx).pl_6();
        div().flex().flex_col().child(header).child(body).into_any_element()
    }

    /// Uma chamada nos Chips: a linha da tabela e, aberta, a entrada e o resultado.
    fn render_chip(&mut self, tool: &ToolRow, cx: &mut Context<Self>) -> AnyElement {
        let open = self.expanded.contains(&tool.key);
        let running = tool.result.is_none() && tool.tone == Tone::Accent;
        let call = &self.events[tool.call];
        let ending = super::rows::chip_ending(call, tool.result.map(|i| &self.events[i]), running, count_lines);
        let key = tool.key.clone();
        let button = super::rows::chip_button(format!("sub-chip-{}", tool.key), call, ending, open, None, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(key.clone(), cx)));
        let body = open.then(|| self.tool_body(tool, cx).px(px(12.)));
        div().flex().flex_col().child(button).children(body).into_any_element()
    }

    /// Uma linha do grupo da Árvore, como na conversa principal: raciocínio com o texto embaixo, ou chamada.
    fn render_tree_part(&mut self, tool: &ToolRow, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let open = self.expanded.contains(&tool.key);
        let key = tool.key.clone();
        let toggle = cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle(key.clone(), cx));
        let event = &self.events[tool.call];
        let id = format!("sub-tree-{}", tool.key);
        if event.kind == "thinking" {
            let text = event.text.clone().unwrap_or_default();
            let below = if open {
                let view = self.text(format!("{}:thought", tool.key), safe_markdown(&text), cx);
                chat_text(&view, cx).text_color(theme::muted()).into_any_element()
            } else { super::rows::thought_preview(&text) };
            return super::rows::tree_row(last, super::rows::tree_thought_line(id, cx).on_click(toggle), Some(below)).into_any_element();
        }
        let failed = tool.tone == Tone::Warning;
        let running = tool.result.is_none() && tool.tone == Tone::Accent;
        let ending = (failed || running).then(|| super::rows::chip_ending(event, tool.result.map(|i| &self.events[i]), running, count_lines));
        let line = super::rows::tree_call_line(id, event, ending, failed, cx).on_click(toggle);
        let body = open.then(|| self.tool_body(tool, cx).into_any_element());
        super::rows::tree_row(last, line, body).into_any_element()
    }

    /// Entrada e resultado da chamada aberta, o mesmo no Clássico e nos Chips.
    fn tool_body(&mut self, tool: &ToolRow, cx: &mut Context<Self>) -> Div {
        let error = tool.tone == Tone::Warning;
        let mut body = div().flex().flex_col().gap_2().pt_1().pb_2();
        if !tool.orphan {
            let input = conversation::pretty_input(self.events[tool.call].tool_input.as_ref());
            if !input.is_empty() { body = body.child(self.detail(format!("{}:input", tool.key), input, tr("tool_input"), false, cx)); }
        }
        body = match tool.result {
            Some(i) => {
                let output = self.events[i].result.clone().unwrap_or_default();
                body.child(self.detail(format!("{}:result", tool.key), output, tr("tool_output"), error, cx))
            }
            None => body.child(div().text_sm().text_color(theme::muted()).child(tool.status.clone())),
        };
        body
    }

    fn render_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(index).cloned() else { return div().into_any_element() };
        let message = matches!(row, Row::Message { .. });
        let row_id = SharedString::from(format!("sub-row-{}", row.id()));
        let inner = match row {
            Row::Message { id, markdown, user, label } => {
                let (long, more_key) = (user && long_message(&markdown), format!("{id}:more"));
                let open = self.expanded.contains(&more_key);
                let view = self.text(id.clone(), markdown, cx);
                let content = conversation_text(div().flex().flex_col().gap_2())
                    .when_some(label, |el, (label, error)| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD)
                        .text_color(if error { theme::warning() } else { theme::muted() }).child(label)))
                    .child(collapse(chat_text(&view, cx).on_link_click(open_web_link), long, open))
                    .when(long, |el| el.child(more_button(SharedString::from(format!("sub-more-{id}")), open)
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(more_key.clone(), cx)))));
                div().w_full().flex().flex_col().map(|el| if user { el.items_end().child(user_bubble(content)) } else { el.child(content) })
                    .into_any_element()
            }
            Row::Tool(tool) => self.render_tool(&tool, cx),
            // A conversa do subagente é lida depois: o grupo da Árvore abre só no clique.
            Row::Group { id, tools, .. } if tree() => {
                let calls: Vec<Tool> = tools.iter().map(|t| Tool { call: t.call, result: t.result }).collect();
                let thinking = |t: &ToolRow| self.events[t.call].kind == "thinking";
                let failed = tools.iter().filter(|t| !thinking(t) && t.tone == Tone::Warning).count();
                let running = tools.iter().any(|t| !thinking(t) && t.result.is_none() && t.tone == Tone::Accent);
                let header = super::rows::tree_header(self.header(&id, cx), super::rows::tree_title(&self.events, &calls), failed, running);
                let lines: Vec<AnyElement> = if self.expanded.contains(&id) {
                    tools.iter().enumerate().map(|(n, t)| self.render_tree_part(t, n + 1 == tools.len(), cx)).collect()
                } else { Vec::new() };
                div().flex().flex_col().child(header).children(lines).into_any_element()
            }
            Row::Group { id, tools, .. } if chips() => {
                let open = super::rows::chip_group_open(tools.len(), self.expanded.contains(&id));
                let calls: Vec<Tool> = tools.iter().map(|t| Tool { call: t.call, result: t.result }).collect();
                let running = tools.iter().any(|t| t.result.is_none() && t.tone == Tone::Accent);
                let toggle = id.clone();
                let button = Button::new(SharedString::from(format!("sub-toggle-{id}"))).ghost().small().w_full().toggled(open)
                    .icon(if open { IconName::ChevronDown } else { IconName::ChevronRight });
                let header = super::rows::chip_group_header(button, &self.events, &calls, running, |_| false)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx)));
                let rows: Vec<AnyElement> = if open { tools.iter().map(|t| self.render_chip(t, cx)).collect() } else { Vec::new() };
                div().flex().flex_col().gap_1().child(header).when(open, |el| el.child(super::rows::chip_table(rows))).into_any_element()
            }
            Row::Group { id, label, summary, status, tone, tools } => {
                let open = self.expanded.contains(&id);
                let header = self.header(&id, cx).accessibility_label(format!("{label}: {summary}. {status}"))
                    .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(label))
                    .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(summary))
                    .child(div().flex_shrink_0().text_color(tone.color()).child(status));
                let children: Vec<AnyElement> = if open { tools.iter().map(|t| self.render_tool(t, cx)).collect() } else { Vec::new() };
                div().flex().flex_col().child(header).when(open, |el| el.child(div().flex().flex_col().pl_5().children(children))).into_any_element()
            }
            Row::Thinking { id, summary, count, parts } => {
                let open = self.expanded.contains(&id);
                let header = self.header(&id, cx).accessibility_label(format!("{}: {summary}", tr("thinking")))
                    .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::muted()).child(tr("thinking")))
                    .child(div().flex_1().min_w_0().truncate().italic().text_color(theme::muted()).child(summary))
                    .when_some(count, |el, count| el.child(div().flex_shrink_0().text_color(theme::muted()).child(count)));
                let body: Vec<AnyElement> = if open { parts.iter().map(|part| match part {
                    Part::Thought(key, source) => {
                        let view = self.text(key.clone(), source.clone(), cx);
                        chat_text(&view, cx).text_color(theme::muted()).into_any_element()
                    }
                    Part::Tool(tool) => self.render_tool(tool, cx),
                }).collect() } else { Vec::new() };
                div().flex().flex_col().child(header)
                    .when(open, |el| el.child(div().flex().flex_col().gap_2().pl_6().pt_1().pb_2().children(body))).into_any_element()
            }
        };
        div().id(row_id).w_full().px_4().when(message, |el| el.py(px(8.))).when(!message, |el| el.py(px(2.)))
            .child(inner).into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatEvent, Row, Tone, owner, prepare};

    #[test]
    fn a_key_belongs_to_the_row_with_the_same_id_or_the_longest_one_before_a_colon() {
        let rows: Vec<Row> = ["U", "U:1", "U:10"].iter().map(|id| Row::Message { id: (*id).into(), markdown: String::new(), user: false, label: None }).collect();
        assert_eq!((owner(&rows, "U"), owner(&rows, "U:1"), owner(&rows, "U:1:input"), owner(&rows, "U:10:result")), (Some(0), Some(1), Some(1), Some(2)));
        assert_eq!((owner(&rows, "U:2:input"), owner(&rows, "V")), (Some(0), None));
    }

    fn event(kind: &str, id: &str, tool: Option<&str>, key: Option<&str>) -> ChatEvent {
        ChatEvent { kind: kind.into(), id: id.into(), tool_name: tool.map(Into::into), tool_use_id: key.map(Into::into),
            text: Some("x".into()), ..Default::default() }
    }

    #[test]
    fn open_calls_run_only_after_the_last_message_and_until_the_agent_finishes() {
        let events = vec![event("tool_use", "a", Some("Read"), Some("u1")), event("assistant_msg", "m", None, None),
            event("tool_use", "b", Some("Grep"), Some("u2"))];
        let tone = |rows: &[Row], id: &str| rows.iter().find_map(|r| match r { Row::Tool(t) if t.key == id => Some(t.tone), _ => None });
        let rows = prepare(&events, false);
        assert_eq!((tone(&rows, "a"), tone(&rows, "b")), (Some(Tone::Muted), Some(Tone::Accent)));
        assert_eq!(tone(&prepare(&events, true), "b"), Some(Tone::Muted));
    }
}

impl Render for SubConversation {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        super::panes::rendered(cx.entity_id(), window, cx);
        let view = cx.entity().downgrade();
        list(self.list.clone(), move |i, _, cx| view.update(cx, |this, cx| this.render_row(i, cx)).unwrap_or_else(|_| div().into_any_element()))
            .size_full()
    }
}
