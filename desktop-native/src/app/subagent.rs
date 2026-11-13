//! Conversa só leitura de um subagente, no detalhe da aba Atividade: as linhas da conversa (bolha do usuário, resposta
//! em markdown, cabeçalho recolhível de ferramenta e pensamento) montadas quando a consulta traz novidade. É uma view
//! própria: a chegada de uma resposta igual não pede quadro, e uma diferente só remede as linhas que mudaram.
use super::*;

fn terminal() -> bool { appearance::get().tool_look == appearance::ToolLook::Terminal }

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
    /// `at`: data e hora sob o prompt (a primeira mensagem do usuário), do começo do subagente.
    Message { id: String, markdown: String, user: bool, label: Option<(String, bool)>, at: Option<String> },
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
    /// `startedAt` do subagente, em segundos desde a época.
    started: Option<f64>,
    rows: Vec<Row>,
    expanded: HashSet<String>,
    rich: HashMap<String, Rich>,
    list: ListState,
    /// Servidores de plugin da sessão aberta: mudam quais chamadas ficam fora da dobra.
    plugin_servers: Vec<String>,
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

fn prepare(events: &[ChatEvent], finished: bool, started: Option<f64>) -> Vec<Row> {
    let prompt = events.iter().position(|e| e.kind == "user_msg");
    // ponytail: sem o cartão de tarefas (`task_list`); as chamadas de tarefa aparecem como chamadas comuns.
    let a = appearance::get();
    let view = conversation::View::for_look(a.tool_look, a.thinking_tools, false);
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
            let at = (prompt == Some(*i)).then(|| stamp(started.or(event.ts))).flatten();
            Row::Message { id: event.id.clone(), markdown: render_source(event), user, label, at }
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

/// Eventos mais antigos entraram na frente: o id do primeiro de antes, onde a rolagem fica presa.
fn prepended_anchor<'a>(old: &'a [ChatEvent], new: &[ChatEvent]) -> Option<&'a str> {
    let first = old.first()?;
    new.iter().skip(1).any(|e| e.id == first.id).then_some(first.id.as_str())
}

impl SubConversation {
    pub fn new() -> Self {
        Self { events: Vec::new(), finished: false, started: None, rows: Vec::new(), expanded: HashSet::new(), rich: HashMap::new(),
            list: ListState::new(0, ListAlignment::Bottom, px(200.)), plugin_servers: Vec::new() }
    }

    /// O conjunto muda a altura das linhas da pele Terminal: remede todas.
    pub fn set_plugin_servers(&mut self, servers: &[String], cx: &mut Context<Self>) {
        if self.plugin_servers == servers { return; }
        self.plugin_servers = servers.to_vec();
        self.list.remeasure();
        cx.notify();
    }

    /// A Aparência mudou: refaz as linhas (pensamento com ou sem ferramentas) e remede todas (Clássico ou Chips).
    pub fn restyle(&mut self, cx: &mut Context<Self>) {
        let events = std::mem::take(&mut self.events);
        self.set_events(events, self.finished, self.started, cx);
        self.list.remeasure();
        cx.notify();
    }

    /// Outro subagente: nada do anterior fica.
    pub fn clear(&mut self) {
        (self.events, self.rows, self.finished, self.started) = (Vec::new(), Vec::new(), false, None);
        self.expanded.clear();
        self.rich.clear();
        self.list.reset(0);
    }

    /// Conversa nova do mesmo subagente: só o trecho que mudou entra na lista, e linha igual não é remedida.
    pub fn set_events(&mut self, events: Vec<ChatEvent>, finished: bool, started: Option<f64>, cx: &mut Context<Self>) {
        let rows = prepare(&events, finished, started);
        let anchor = prepended_anchor(&self.events, &events).map(str::to_owned);
        (self.events, self.finished, self.started) = (events, finished, started);
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
        // Página anterior carregada no topo: a primeira linha de antes continua onde estava, como na conversa principal.
        // O splice já desloca a rolagem; isto cobre a linha do topo que mudou de forma ao juntar os eventos novos.
        let top = self.list.logical_scroll_top();
        if let (0, Some(i)) = (top.item_ix, anchor.and_then(|id| owner(&rows, &id)).filter(|&i| i > 0)) {
            self.list.scroll_to(ListOffset { item_ix: i, offset_in_item: top.offset_in_item });
        }
        self.rows = rows;
        let ids: HashSet<&str> = self.rows.iter().map(Row::id).collect();
        self.rich.retain(|key, _| ids.iter().any(|id| key.starts_with(id)));
        self.expanded.retain(|key| {
            let base = key.strip_suffix(":more").or_else(|| key.strip_suffix(":fold")).unwrap_or(key);
            self.events.iter().any(|e| &e.id == key || e.id == base) || ids.contains(base)
        });
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
        let probe = key.strip_suffix(":fold").unwrap_or(&key);
        if let Some(i) = self.rows.iter().position(|r| match r {
            Row::Group { tools, .. } => tools.iter().any(|t| t.key == probe),
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
        if !tool.orphan {
            match appearance::get().tool_look {
                appearance::ToolLook::Chips => return super::rows::chip_box().child(self.render_chip(tool, cx)).into_any_element(),
                appearance::ToolLook::Terminal => return self.render_terminal_tool(tool, cx),
                appearance::ToolLook::ClaudeDesktop => return super::rows::chip_box().child(self.render_desktop_call(tool, cx)).into_any_element(),
                appearance::ToolLook::Classic | appearance::ToolLook::Tree => {}
            }
        }
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

    /// Uma chamada no estilo Claude Desktop, como `Hangar::render_desktop_call`.
    fn render_desktop_call(&mut self, tool: &ToolRow, cx: &mut Context<Self>) -> AnyElement {
        let open = self.expanded.contains(&tool.key);
        let key = tool.key.clone();
        let line = super::desktop_look::call_button(format!("sub-desktop-{}", tool.key), &self.events[tool.call], tool.tone == Tone::Warning, open, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(key.clone(), cx)));
        let body = open.then(|| self.tool_body(tool, cx));
        super::desktop_look::call_block(line, body)
    }

    /// Grupo no estilo Claude Desktop, como `Hangar::render_desktop_group`: a linha do resumo e, aberto, o cartão.
    fn render_desktop_group(&mut self, id: &str, tools: &[ToolRow], cx: &mut Context<Self>) -> AnyElement {
        let events = &self.events;
        let summary = super::desktop_look::desktop_summary(tools.iter().map(|t| (&events[t.call],
            t.result.is_some_and(|i| events[i].is_error == Some(true)), t.result.is_none() && t.tone == Tone::Accent)));
        let open = self.expanded.contains(id);
        let toggle = id.to_owned();
        let header = super::desktop_look::summary_button(format!("sub-desktop-{id}"), summary, open, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx)));
        let rows: Vec<AnyElement> = if open { tools.iter().map(|t| self.render_desktop_call(t, cx)).collect() } else { Vec::new() };
        super::desktop_look::group_block(header, open, rows)
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
            let below = open.then(|| {
                let view = self.text(format!("{}:thought", tool.key), safe_markdown(&text), cx);
                chat_text(&view, cx).text_color(theme::muted()).into_any_element()
            });
            return super::rows::tree_row(last, super::rows::tree_thought_line(id, cx).on_click(toggle), below).into_any_element();
        }
        let failed = tool.tone == Tone::Warning;
        let ending = if failed { None } else { super::edits::totals(event) };
        let line = super::rows::tree_call_line(id, event, ending, failed, cx).on_click(toggle);
        let body = open.then(|| self.tool_body(tool, cx).into_any_element());
        super::rows::tree_row(last, line, body).into_any_element()
    }

    /// Entrada e resultado da chamada aberta, o mesmo no Clássico e nos Chips.
    fn tool_body(&mut self, tool: &ToolRow, cx: &mut Context<Self>) -> Div {
        let error = tool.tone == Tone::Warning;
        let mut body = div().flex().flex_col().gap_2().pt_1().pb_2();
        // Edição de arquivo mostra o diff no lugar da entrada crua; o resultado só aparece se falhou.
        let result = tool.result.map(|i| &self.events[i]);
        let diff = (!tool.orphan).then(|| super::edits::card(&self.events[tool.call], result, cx)).flatten();
        let has_diff = diff.is_some();
        if let Some(diff) = diff { body = body.child(diff); }
        else if !tool.orphan {
            let input = conversation::pretty_input(self.events[tool.call].tool_input.as_ref());
            if !input.is_empty() { body = body.child(self.detail(format!("{}:input", tool.key), input, tr("tool_input"), false, cx)); }
        }
        body = match tool.result {
            Some(_) if has_diff && !error => body,
            Some(i) => {
                let output = self.events[i].result.clone().unwrap_or_default();
                body.children(super::desktop_look::exit_code_line(&output))
                    .child(self.detail(format!("{}:result", tool.key), output, tr("tool_output"), error, cx))
            }
            None => body.child(div().text_sm().text_color(theme::muted()).child(tool.status.clone())),
        };
        body
    }

    /// Uma chamada na pele Terminal, como `Hangar::render_terminal_tool`: "● Nome(arg)", o desfecho com o cotovelo e, na
    /// edição, o diff. ponytail: sem o cartão de agente e os arquivos enviados da conversa principal.
    fn render_terminal_tool(&mut self, tool: &ToolRow, cx: &mut Context<Self>) -> AnyElement {
        use super::terminal_look as term;
        let call = &self.events[tool.call];
        let result = tool.result.map(|i| &self.events[i]);
        let key = tool.key.clone();
        let error = tool.tone == Tone::Warning;
        let name = term::display_name(call.tool_name.as_deref().unwrap_or_default());
        let edits = (!error).then(|| crate::editdiff::shown(call, result)).flatten();
        let (arg, path) = term::header_arg(call.tool_name.as_deref(), call.tool_input.as_ref());
        let dot = if error { theme::danger() } else if result.is_none() { theme::accent() } else { theme::success() };
        let created = call.tool_name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case("write"))
            && edits.as_deref().is_some_and(|e| e.iter().all(|e| !e.patched && e.removed == 0));
        let shown = if created { term::WRITE_SHOWN } else { term::EDIT_SHOWN };
        let expandable = edits.as_deref().is_some_and(|e| e.iter().map(|e| e.lines.len()).sum::<usize>() > shown);
        let outcome = match &edits { Some(edits) if result.is_some() => term::edit_summary(edits, created), _ => tool.status.clone() };
        let open = self.expanded.contains(&key);
        let inner = div().w_full().flex().items_start().justify_start().gap(px(8.)).font_family(theme::MONO).text_size(px(12.5)).line_height(px(19.))
            .child(term::bullet(dot, 19.))
            .child(div().min_w_0().flex().flex_wrap()
                .child(div().font_weight(FontWeight::BOLD).text_color(theme::text()).child(name.clone()))
                .child(div().min_w_0().text_color(theme::text()).child(format!("({arg})"))));
        let header: AnyElement = if edits.is_some() && !expandable {
            div().id(SharedString::from(format!("sub-term-{key}"))).w_full()
                .when_some(path.clone(), |el, path| el.tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(path.clone()).build(window, cx)))
                .child(inner).into_any_element()
        } else {
            let toggle = key.clone();
            Button::new(SharedString::from(format!("sub-term-{key}"))).ghost().w_full().h_auto().p_0().justify_start().toggled(open)
                .accessibility_label(format!("{name}: {}. {outcome}", path.as_deref().unwrap_or(&arg)))
                .when_some(path.clone(), |el, path| el.tooltip(path))
                .child(inner)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx))).into_any_element()
        };
        let outcome_line = div().flex().gap(px(8.)).pl(px(term::INDENT - 8.)).font_family(theme::MONO).text_size(px(12.5)).line_height(px(19.))
            .child(div().flex_none().text_color(theme::faint()).child("⎿"))
            .child(div().min_w_0().text_color(if error { theme::danger() } else if edits.is_some() { theme::text() } else { tool.tone.color() }).child(outcome));
        let body = match &edits {
            Some(edits) => {
                let (rows, hidden, total) = term::diff_body(&self.events[tool.call], edits, (!open).then_some(shown), cx);
                let faint = |text: String| div().text_size(px(12.)).font_family(theme::MONO).text_color(theme::faint()).child(text);
                let more = (!open && hidden > 0).then(|| faint(crate::i18n::tr_shared("term_mais_linhas", &[("n", &hidden.to_string())])));
                let clipped = (more.is_none() && total > super::edits::SHOWN_MAX).then(|| faint(tr("diff_clipped")
                    .replace("{shown}", &super::edits::SHOWN_MAX.to_string()).replace("{total}", &total.to_string())));
                let collapse = (open && expandable).then(|| faint(crate::i18n::tr_shared("term_recolher", &[])));
                Some(div().pl(px(term::INDENT + 16.)).flex().flex_col().children(rows).children(more).children(clipped).children(collapse))
            }
            None => open.then(|| self.tool_body(tool, cx).pl(px(term::INDENT + 16.))),
        };
        div().flex().flex_col().child(header).child(outcome_line).children(body).into_any_element()
    }

    /// Chamadas seguidas na pele Terminal: buscas, leituras e comandos seguidos somam numa linha que abre no clique, como
    /// `Hangar::render_terminal_group`.
    fn render_terminal_group(&mut self, tools: &[ToolRow], cx: &mut Context<Self>) -> AnyElement {
        use super::terminal_look as term;
        let kinds: Vec<Option<term::FoldKind>> = tools.iter().map(|t| {
            let call = &self.events[t.call];
            if call.kind == "thinking" { None } else { term::fold_kind(call.tool_name.as_deref(), call.tool_input.as_ref(), &self.plugin_servers) }
        }).collect();
        let running = |t: &ToolRow| t.result.is_none() && t.tone == Tone::Accent;
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut i = 0;
        while i < tools.len() {
            if kinds[i].is_none() { rows.push(self.render_tool(&tools[i], cx)); i += 1; continue; }
            let run = kinds[i..].iter().take_while(|k| k.is_some()).count();
            let folded = &tools[i..i + run];
            let calls: Vec<Tool> = folded.iter().map(|t| Tool { call: t.call, result: t.result }).collect();
            let folded_kinds: Vec<term::FoldKind> = kinds[i..i + run].iter().flatten().copied().collect();
            let key = format!("{}:fold", folded[0].key);
            let open = self.expanded.contains(&key);
            let live = folded.iter().rev().find(|t| running(t));
            let title = term::fold_title(&self.events, &calls, &folded_kinds, live.is_some());
            let toggle = key.clone();
            let mut block = vec![term::fold_header(SharedString::from(format!("sub-{key}")), title, open, live.is_some(),
                cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx)))];
            if !open {
                if let Some(tool) = live {
                    let call = &self.events[tool.call];
                    let shell = call.tool_name.as_deref().is_some_and(|name| term::is_any(name, term::SHELL_TOOLS));
                    block.push(term::fold_sub(if shell { format!("$ {}", tool.summary) } else { tool.summary.clone() }, theme::muted()));
                }
                for tool in folded.iter().filter(|t| t.tone == Tone::Warning) { block.push(term::fold_sub(tool.status.clone(), theme::danger())); }
            }
            rows.push(div().flex().flex_col().children(block).into_any_element());
            if open { for tool in folded { rows.push(self.render_terminal_tool(tool, cx)); } }
            i += run;
        }
        div().flex().flex_col().gap(px(10.)).children(rows).into_any_element()
    }

    fn render_row(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(row) = self.rows.get(index).cloned() else { return div().into_any_element() };
        let message = matches!(row, Row::Message { .. });
        let row_id = SharedString::from(format!("sub-row-{}", row.id()));
        let inner = match row {
            Row::Message { id, markdown, user, label, at } => {
                let dotted = terminal() && !user && label.is_none();
                let (long, more_key) = (user && long_message(&markdown), format!("{id}:more"));
                let open = self.expanded.contains(&more_key);
                let view = self.text(id.clone(), markdown, cx);
                let content = conversation_text(div().flex().flex_col().gap_2(), user)
                    .when_some(label, |el, (label, error)| el.child(div().text_xs().font_weight(FontWeight::SEMIBOLD)
                        .text_color(if error { theme::warning() } else { theme::muted() }).child(label)))
                    .child(collapse(chat_text(&view, cx).on_link_click(open_web_link), long, open))
                    .when(long, |el| el.child(more_button(SharedString::from(format!("sub-more-{id}")), open)
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(more_key.clone(), cx)))));
                div().w_full().flex().flex_col().map(|el| if user { el.items_end().child(user_bubble(content)) }
                    else if dotted { el.child(super::terminal_look::assistant_text_row(content, super::conversation_line(false))) }
                    else { el.child(content) })
                    .when_some(at, |el, at| el.child(div().pt(px(4.)).text_xs().text_color(theme::faint()).child(at)))
                    .into_any_element()
            }
            Row::Tool(tool) => self.render_tool(&tool, cx),
            Row::Group { id, label, summary, status, tone, tools } => match appearance::get().tool_look {
                // A conversa do subagente é lida depois: o grupo da Árvore abre só no clique.
                appearance::ToolLook::Tree => {
                    let calls: Vec<Tool> = tools.iter().map(|t| Tool { call: t.call, result: t.result }).collect();
                    let thinking = |t: &ToolRow| self.events[t.call].kind == "thinking";
                    let failed = tools.iter().filter(|t| !thinking(t) && t.tone == Tone::Warning).count();
                    let running = tools.iter().any(|t| !thinking(t) && t.result.is_none() && t.tone == Tone::Accent);
                    let running = running.then(|| div().flex_shrink_0().text_color(theme::faint()).child(tr("chip_running")).into_any_element());
                    let header = super::rows::tree_header(self.header(&id, cx), super::rows::tree_title(&self.events, &calls), failed, running, None);
                    let lines: Vec<AnyElement> = if self.expanded.contains(&id) {
                        tools.iter().enumerate().map(|(n, t)| self.render_tree_part(t, n + 1 == tools.len(), cx)).collect()
                    } else { Vec::new() };
                    div().flex().flex_col().child(header).children(lines).into_any_element()
                }
                appearance::ToolLook::Terminal => self.render_terminal_group(&tools, cx),
                appearance::ToolLook::ClaudeDesktop => self.render_desktop_group(&id, &tools, cx),
                appearance::ToolLook::Chips => {
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
                appearance::ToolLook::Classic => {
                    let open = self.expanded.contains(&id);
                    let header = self.header(&id, cx).accessibility_label(format!("{label}: {summary}. {status}"))
                        .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(label))
                        .child(div().flex_1().min_w_0().truncate().text_color(theme::muted()).child(summary))
                        .child(div().flex_shrink_0().text_color(tone.color()).child(status));
                    let children: Vec<AnyElement> = if open { tools.iter().map(|t| self.render_tool(t, cx)).collect() } else { Vec::new() };
                    div().flex().flex_col().child(header).when(open, |el| el.child(div().flex().flex_col().pl_5().children(children))).into_any_element()
                }
            },
            Row::Thinking { id, summary, count, parts } => {
                let open = self.expanded.contains(&id);
                let header = self.header(&id, cx).accessibility_label(format!("{}: {summary}", tr("thinking")))
                    .child(super::chrome::small_icon(IconName::Brain, 14., theme::muted()))
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
    use super::{ChatEvent, Row, Tone, owner, prepare, prepended_anchor};

    #[test]
    fn older_events_in_front_anchor_the_previous_first_event_and_a_sliding_window_does_not() {
        let evs = |ids: &[&str]| ids.iter().map(|id| event("user_msg", id, None, None)).collect::<Vec<_>>();
        assert_eq!(prepended_anchor(&evs(&["c", "d"]), &evs(&["a", "b", "c", "d"])), Some("c"));
        assert_eq!(prepended_anchor(&evs(&["c", "d"]), &evs(&["d", "e"])), None);
        assert_eq!(prepended_anchor(&evs(&["c", "d"]), &evs(&["c", "d", "e"])), None);
        assert_eq!(prepended_anchor(&[], &evs(&["a"])), None);
    }

    #[test]
    fn a_key_belongs_to_the_row_with_the_same_id_or_the_longest_one_before_a_colon() {
        let rows: Vec<Row> = ["U", "U:1", "U:10"].iter().map(|id| Row::Message { id: (*id).into(), markdown: String::new(), user: false, label: None, at: None }).collect();
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
        let rows = prepare(&events, false, None);
        assert_eq!((tone(&rows, "a"), tone(&rows, "b")), (Some(Tone::Muted), Some(Tone::Accent)));
        assert_eq!(tone(&prepare(&events, true, None), "b"), Some(Tone::Muted));
    }

    /// O código de saída aparece acima da saída aberta em toda pele (os testes rodam no Clássico).
    #[gpui::test]
    fn classic_open_call_shows_the_exit_code(cx: &mut gpui::TestAppContext) {
        crate::i18n::english_in_this_thread(false);
        cx.update(gpui_kit::init);
        let (view, cx) = cx.add_window_view(|window, _| { window.retain_a11y_tree(true); super::SubConversation::new() });
        view.update(cx, |view, cx| {
            let mut result = event("tool_result", "r1", None, Some("u1"));
            (result.is_error, result.result) = (Some(true), Some("Exit code 1\nerro".into()));
            view.set_events(vec![event("tool_use", "u1", Some("Bash"), Some("u1")), result], true, None, cx);
            view.expanded.insert("u1".into());
        });
        cx.run_until_parked();
        let shown = cx.update(|window, cx| { let _ = window.draw(cx); window.a11y_snapshot(None).unwrap() });
        assert!(shown.contains("erro"), "a saída aberta precisa aparecer: {shown}");
        assert!(shown.contains(&crate::i18n::tr_shared("ferr_desktop_codigo_saida", &[("n", "1")])), "{shown}");
    }
}

impl Render for SubConversation {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        list(self.list.clone(), move |i, _, cx| view.update(cx, |this, cx| this.render_row(i, cx)).unwrap_or_else(|_| div().into_any_element()))
            .size_full()
    }
}
