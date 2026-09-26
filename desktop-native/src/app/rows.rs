//! Linhas da conversa que dependem da Aparência: chamadas no modo Chips, o bloco da lista de tarefas
//! e o gráfico das tabelas das respostas.
use super::*;
use crate::{conversation::{Family, Task, TaskStatus}, tables::{self, Table}};
use gpui_kit::component::{chart::BarChart, progress::ProgressCircle};

/// Os grupos pequenos nascem abertos no modo Chips, como no web; nos maiores, a pessoa abre.
const CHIPS_OPEN_UP_TO: usize = 5;

fn family_icon(family: Family) -> IconName {
    match family {
        Family::Read => IconName::FileText, Family::Search => IconName::Search, Family::Edit => IconName::SquarePen,
        Family::Create => IconName::FilePlus, Family::Run => IconName::SquareTerminal, Family::Other => IconName::Wrench,
    }
}

fn verb(name: Option<&str>) -> String {
    match conversation::family(name) {
        Family::Read => tr("chip_read"), Family::Search => tr("chip_search"), Family::Edit => tr("chip_edit"),
        Family::Create => tr("chip_create"), Family::Run => tr("chip_run"),
        Family::Other => name.map(conversation::tool_display_name).unwrap_or_else(|| tr("tool")),
    }
}

/// Plural de uma contagem pela chave `<key>_1` / `<key>` com `{n}`.
fn counted(key: &str, n: usize) -> String {
    if n == 1 { tr(&format!("{key}_1")) } else { tr(key).replace("{n}", &n.to_string()) }
}

/// O que vai no chip: o nome do arquivo nas ferramentas de arquivo, o resumo da entrada nas demais.
fn chip_text(event: &ChatEvent) -> String {
    let summary = conversation::summarize_input(event.tool_name.as_deref(), event.tool_input.as_ref());
    match conversation::family(event.tool_name.as_deref()) {
        Family::Read | Family::Edit | Family::Create => summary.rsplit('/').next().unwrap_or(&summary).to_owned(),
        _ => summary,
    }
}

pub(super) fn chip_box() -> Div {
    div().flex().flex_col().rounded(px(10.)).border_1().border_color(theme::border()).overflow_hidden()
}

/// Fim da linha no modo Chips: rodando, erro, linhas postas e tiradas, ou o desfecho por ferramenta. As linhas do
/// resultado vêm de quem chama, porque a conversa principal as guarda prontas.
pub(super) fn chip_ending(call: &ChatEvent, result: Option<&ChatEvent>, running: bool, lines: impl FnOnce(&ChatEvent) -> usize) -> AnyElement {
    let name = call.tool_name.as_deref();
    let text = |t: String, color: Hsla| div().flex_shrink_0().max_w(px(260.)).truncate().text_size(px(12.5)).text_color(color).child(t);
    let Some(result) = result else {
        return if running { text(tr("chip_running"), theme::accent()).into_any_element() }
            else { text(tr("tool_no_result"), theme::faint()).into_any_element() };
    };
    let raw = result.result.as_deref().unwrap_or("").trim();
    if result.is_error == Some(true) {
        let first = raw.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| conversation::one_line(l, 72)).unwrap_or_else(|| tr("tool_failed"));
        return warning_ending(first);
    }
    if let Some((added, removed)) = conversation::edit_counts(name, call.tool_input.as_ref()) {
        return div().flex_shrink_0().flex().gap(px(6.)).font_family(theme::MONO).text_size(px(12.))
            .child(div().text_color(theme::success()).child(format!("+{added}")))
            .when(removed > 0, |el| el.child(div().text_color(theme::removed()).child(format!("−{removed}"))))
            .into_any_element();
    }
    let lines = lines(result);
    let outcome = match (raw.is_empty(), conversation::family(name)) {
        (true, _) => tr("chip_done"),
        (false, Family::Run) => format!("{} ({})", tr("chip_done"), counted("chip_lines", lines)),
        (false, Family::Read) => counted("chip_lines_loaded", lines),
        _ => counted("chip_lines_returned", lines),
    };
    // Pronto tem sinal próprio além da cor: o ✓ distingue do "rodando" e do erro.
    div().flex_shrink_0().flex().items_center().gap_1().child(chrome::small_icon(IconName::Check, 13., theme::success()))
        .child(text(outcome, theme::muted())).into_any_element()
}

fn warning_ending(text: String) -> AnyElement {
    div().flex_shrink_0().flex().items_center().gap_1().child(chrome::small_icon(IconName::CircleX, 13., theme::warning()))
        .child(div().flex_shrink_0().max_w(px(260.)).truncate().text_size(px(12.5)).text_color(theme::warning()).child(text))
        .into_any_element()
}

/// Fim da linha de um subagente que acabou em erro de API.
fn failed_ending() -> AnyElement { warning_ending(tr("tool_failed")) }

/// Uma chamada no modo Chips: linha da tabela com verbo, resumo em mono e desfecho à direita. Quem chama liga o clique;
/// `label` troca o rótulo acessível quando o clique faz outra coisa que abrir.
pub(super) fn chip_button(id: String, call: &ChatEvent, ending: AnyElement, open: bool, label: Option<String>, cx: &App) -> Button {
    let verb = verb(call.tool_name.as_deref());
    let chip = chip_text(call);
    let label = label.unwrap_or_else(|| format!("{verb} {chip}"));
    // O Agent abre a conversa noutro lugar, não expande aqui.
    let trailing = if super::activity::agent_request(call).is_some() { IconName::ExternalLink }
        else if open { IconName::ChevronDown } else { IconName::ChevronRight };
    Button::new(SharedString::from(id))
        .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
        .w_full().h(px(34.)).px(px(12.)).rounded(px(0.))
        .toggled(open).accessibility_label(label)
        .child(div().w_full().min_w_0().flex().items_center().gap(px(8.)).text_size(px(13.5))
            // Tabela do mock: verbo numa coluna, o resumo em mono, o desfecho alinhado à direita.
            // Coluna mínima, não fixa: nome de ferramenta sem verbo (ToolSearch, MCP) empurra o resumo.
            .child(div().min_w(px(64.)).flex_shrink_0().text_color(theme::muted()).child(verb))
            .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(12.5)).child(chip))
            .child(ending)
            .child(chrome::small_icon(trailing, 14., theme::faint())))
}

/// Grupo pequeno nasce aberto nos Chips; o clique inverte.
pub(super) fn chip_group_open(tools: usize, toggled: bool) -> bool { (tools <= CHIPS_OPEN_UP_TO) != toggled }

/// Resumo do grupo pela contagem por família, com a primeira letra maiúscula: "Rodou 2 comandos · leu 1 arquivo".
pub(super) fn family_title(events: &[ChatEvent], tools: &[Tool]) -> String {
    let text = conversation::family_counts(events, tools).into_iter().map(|(family, n)| counted(match family {
        Family::Read => "chip_title_read", Family::Search => "chip_title_search", Family::Edit => "chip_title_edit",
        Family::Create => "chip_title_create", Family::Run => "chip_title_run", Family::Other => "chip_title_other",
    }, n)).collect::<Vec<_>>().join(" · ");
    let mut chars = text.chars();
    chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default()
}

/// "1 falhou" / "N falharam".
pub(super) fn failed_count(n: usize) -> String { counted("tools_failed", n) }

/// Cabeçalho do grupo nos Chips, no botão de abrir de quem chama: título pela contagem por família, chamadas, rodando
/// e erros.
/// `failed` diz se a chamada falhou além do resultado marcado como erro (o Agent cujo subagente falhou).
pub(super) fn chip_group_header(button: Button, events: &[ChatEvent], tools: &[Tool], running: bool, failed: impl Fn(usize) -> bool) -> Button {
    let title = family_title(events, tools);
    let errors = tools.iter().filter(|t| t.result.is_some_and(|i| events[i].is_error == Some(true)) || failed(t.call)).count();
    let last = tools.last().map(|t| conversation::family(events[t.call].tool_name.as_deref())).unwrap_or(Family::Other);
    let calls = counted("chip_calls", tools.len());
    button.accessibility_label(format!("{title} · {calls}"))
        .child(chrome::small_icon(family_icon(last), 14., if errors > 0 { theme::warning() } else { theme::muted() }))
        .child(div().min_w_0().truncate().text_color(if errors > 0 { theme::warning() } else { theme::text() }).child(title))
        .child(div().flex_shrink_0().text_color(theme::faint()).child(format!("· {calls}")))
        .child(div().flex_1())
        .when(running, |el| el.child(div().flex_shrink_0().text_color(theme::accent()).child(tr("chip_running"))))
        .when(errors > 0, |el| el.child(div().flex_shrink_0().text_color(theme::warning()).child(tr("tools_errors").replace("{n}", &errors.to_string()))))
}

// Árvore, nas medidas do Zeron: o tronco sai do centro do chevron do cabeçalho (botão small: 8 de recuo + ícone de 14),
// curva de raio 6 até o fim do ramo, onde começa a linha de 32.
const TREE_TRUNK: f32 = 14.5;
const TREE_BRANCH_END: f32 = 30.;
const TREE_LINE: f32 = 32.;
/// Do fim do ramo até o rótulo: recuo do botão, ícone de 16 e o vão de 8. O texto sob a linha começa ali.
const TREE_TEXT: f32 = 28.;

/// Título do grupo da Árvore: o raciocínio primeiro, depois a contagem por família ("Raciocínio · fez 1 busca").
pub(super) fn tree_title(events: &[ChatEvent], parts: &[Tool]) -> String {
    let calls: Vec<Tool> = parts.iter().copied().filter(|t| events[t.call].kind != "thinking").collect();
    let thought = match parts.len() - calls.len() { 0 => None, 1 => Some(tr("thinking")), n => Some(tr("tree_thoughts").replace("{n}", &n.to_string())) };
    let counts = (!calls.is_empty()).then(|| family_title(events, &calls));
    match (thought, counts) {
        // A contagem vem com maiúscula; depois do raciocínio volta à minúscula da chave.
        (Some(thought), Some(counts)) => {
            let mut chars = counts.chars();
            format!("{thought} · {}", chars.next().map(|c| c.to_lowercase().chain(chars).collect::<String>()).unwrap_or_default())
        }
        (thought, counts) => thought.or(counts).unwrap_or_default(),
    }
}

/// Cabeçalho do grupo da Árvore no botão de abrir de quem chama: título sem caixa, falhas e "rodando" em texto.
pub(super) fn tree_header(button: Button, title: String, failed: usize, running: bool) -> Button {
    let failed = (failed > 0).then(|| failed_count(failed));
    button.accessibility_label(failed.as_ref().map_or_else(|| title.clone(), |f| format!("{title} · {f}")))
        .child(div().min_w_0().truncate().text_color(theme::muted()).child(title))
        .when_some(failed, |el, failed| el.child(div().flex_shrink_0().text_color(theme::warning()).child(format!("· {failed}"))))
        .when(running, |el| el.child(div().flex_shrink_0().text_color(theme::accent()).child(tr("chip_running"))))
        .child(div().flex_1())
}

/// Uma linha da árvore: ícone, rótulo e detalhe; o fim só aparece para falha e "rodando". Quem chama liga o clique.
fn tree_line(id: String, icon: IconName, label: String, detail: String, ending: Option<AnyElement>, failed: bool, cx: &App) -> Button {
    let color = if failed { theme::warning() } else { theme::muted() };
    Button::new(SharedString::from(id))
        .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
        .w_full().h(px(TREE_LINE)).px(px(4.)).rounded(px(6.))
        .accessibility_label(format!("{label} {detail}"))
        .child(div().w_full().min_w_0().flex().items_center().gap(px(8.)).text_size(px(13.))
            .child(chrome::small_icon(icon, 16., color))
            .child(div().flex_shrink_0().text_color(if failed { theme::warning() } else { theme::text() }).child(label))
            .child(div().flex_1().min_w_0().truncate().text_color(color).child(detail))
            .children(ending))
}

/// Linha de chamada da árvore: ícone da família, verbo e o resumo do chip.
pub(super) fn tree_call_line(id: String, call: &ChatEvent, ending: Option<AnyElement>, failed: bool, cx: &App) -> Button {
    let name = call.tool_name.as_deref();
    tree_line(id, family_icon(conversation::family(name)), verb(name), chip_text(call), ending, failed, cx)
}

/// Linha de raciocínio da árvore; o texto vai embaixo, pelo `tree_row`.
pub(super) fn tree_thought_line(id: String, cx: &App) -> Button {
    tree_line(id, IconName::MessageCircle, tr("thinking"), String::new(), None, false, cx)
}

/// Raciocínio fechado sob a linha: texto corrido cortado em três linhas, como no Zeron.
pub(super) fn thought_preview(text: &str) -> AnyElement {
    // Três linhas cabem em bem menos que 120 palavras; o resto nem é juntado.
    let flat = text.split_whitespace().take(120).collect::<Vec<_>>().join(" ");
    // Sem o `text_ellipsis` a gpui não corta a terceira linha: ela passa da coluna.
    div().text_size(px(13.)).line_height(px(20.)).text_color(theme::muted()).line_clamp(3).text_ellipsis().child(flat).into_any_element()
}

/// Linha pendurada no tronco: a curva até ela e, fora a última, o tronco seguindo até a próxima. `below` (texto do
/// raciocínio, entrada e resultado) fica alinhado ao rótulo.
pub(super) fn tree_row(last: bool, line: Button, below: Option<AnyElement>) -> Div {
    // A borda comum some no fundo; a forte fica perto do traço do Zeron.
    let stroke = theme::border_strong();
    div().relative().pl(px(TREE_BRANCH_END))
        .when(!last, |el| el.child(div().absolute().top_0().bottom_0().left(px(TREE_TRUNK)).w(px(1.)).bg(stroke)))
        .child(div().absolute().top_0().left(px(TREE_TRUNK)).w(px(TREE_BRANCH_END - TREE_TRUNK)).h(px(TREE_LINE / 2.))
            .border_l_1().border_b_1().border_color(stroke).rounded_bl(px(6.)))
        .child(line)
        .when_some(below, |el, below| el.child(div().pl(px(TREE_TEXT)).pb(px(6.)).child(below)))
}

/// As linhas do grupo aberto numa tabela com borda.
pub(super) fn chip_table(rows: Vec<AnyElement>) -> Div {
    chip_box().children(rows.into_iter().enumerate().map(|(n, row)| div().when(n > 0, |el| el.border_t_1().border_color(theme::border())).child(row)))
}

impl Hangar {
    /// Uma chamada no modo Chips da conversa principal.
    fn render_chip(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let events = &self.chat.events;
        let call = &events[tool.call];
        let key = call.id.clone();
        let open = self.expanded.contains(&key);
        // O cartão Agent abre a conversa dele na aba Atividade em vez de expandir; o subagente que acabou em erro de API
        // fica em aviso, e o que roda leva a marca animada.
        let agent = super::activity::agent_request(call);
        let failed = agent.is_some() && self.agent_failed(tool.call);
        let running = self.running(tool.call);
        let ending = if failed { failed_ending() } else { chip_ending(call, tool.result.map(|i| &events[i]), running, |result| self.result_lines(result)) };
        let ending = if agent.is_some() && running && tool.result.is_none() && !failed {
            div().flex_shrink_0().flex().items_center().gap(px(6.))
                .child(self.working_mark_slot(super::panes::Area::Conversation, format!("agent-{key}"), 12., theme::accent())).child(ending)
                .into_any_element()
        } else { ending };
        let toggle_key = key.clone();
        let label = agent.is_some().then(|| format!("{}: {}", super::activity::web("tool_abrir_agente"), chip_text(call)));
        let button = chip_button(format!("chip-{key}"), call, ending, open, label, cx)
            .on_click(cx.listener(move |this, _, _, cx| match &agent {
                Some(request) => this.open_agent(request.clone(), cx),
                None => this.toggle(toggle_key.clone(), cx),
            }));
        let body = open.then(|| self.tool_body(tool, row, cx).px(px(12.)));
        div().flex().flex_col().child(button).children(body).into_any_element()
    }

    /// Chamada solta nos Chips: a mesma linha da tabela do grupo, numa caixa de uma linha. Forma única para toda chamada.
    pub(super) fn render_single_chip(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let line = self.render_chip(tool, row, cx);
        chip_box().child(line).into_any_element()
    }

    /// Grupo no modo Chips: título pela contagem por família e as chamadas numa tabela com borda.
    pub(super) fn render_chip_group(&mut self, row: &str, tools: &[Tool], cx: &mut Context<Self>) -> AnyElement {
        let running = tools.iter().any(|t| t.result.is_none() && self.running(t.call));
        let open = chip_group_open(tools.len(), self.expanded.contains(row));
        let toggle_key = row.to_owned();
        let header = chip_group_header(self.disclosure(row, open), &self.chat.events, tools, running, |call| self.agent_failed(call))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let rows: Vec<AnyElement> = if open { tools.iter().map(|&tool| self.render_chip(tool, row, cx)).collect() } else { Vec::new() };
        div().flex().flex_col().gap_1().child(header).when(open, |el| el.child(chip_table(rows))).into_any_element()
    }

    /// Grupo da Árvore: título e as linhas no tronco. Aberto enquanto é a cauda do turno, como no Zeron, mesmo entre uma
    /// chamada e a próxima; o clique inverte.
    pub(super) fn render_tree_group(&mut self, row: &str, parts: &[Tool], cx: &mut Context<Self>) -> AnyElement {
        let events = &self.chat.events;
        let calls = parts.iter().filter(|t| events[t.call].kind != "thinking");
        let failed = calls.clone().filter(|t| t.result.is_some_and(|i| events[i].is_error == Some(true))).count();
        let running = calls.clone().any(|t| t.result.is_none() && self.running(t.call));
        // Texto fecha grupo no `build`: a primeira parte "roda" enquanto nenhuma mensagem veio depois e a sessão trabalha.
        let live = parts.first().is_some_and(|t| self.running(t.call));
        let open = live != self.expanded.contains(row);
        let toggle_key = row.to_owned();
        let header = tree_header(self.disclosure(row, open), tree_title(events, parts), failed, running)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let lines: Vec<AnyElement> = if open {
            parts.iter().enumerate().map(|(n, &tool)| self.render_tree_part(tool, row, n + 1 == parts.len(), cx)).collect()
        } else { Vec::new() };
        div().flex().flex_col().child(header).children(lines).into_any_element()
    }

    /// Uma linha do grupo da Árvore: raciocínio (texto cortado embaixo, inteiro ao abrir) ou chamada (entrada e resultado).
    fn render_tree_part(&mut self, tool: Tool, row: &str, last: bool, cx: &mut Context<Self>) -> AnyElement {
        let events = &self.chat.events;
        let event = &events[tool.call];
        let key = event.id.clone();
        let open = self.expanded.contains(&key);
        let toggle_key = key.clone();
        let toggle = cx.listener(move |this, _: &ClickEvent, _, cx| this.toggle(toggle_key.clone(), cx));
        if event.kind == "thinking" {
            let text = event.text.clone().unwrap_or_default();
            let below = if open {
                let view = self.text_view(&format!("{key}:thought"), row, safe_markdown(&text), cx);
                chat_text(&view, cx).text_color(theme::muted()).into_any_element()
            } else { thought_preview(&text) };
            return tree_row(last, tree_thought_line(format!("tree-{key}"), cx).on_click(toggle), Some(below)).into_any_element();
        }
        let failed = tool.result.is_some_and(|i| events[i].is_error == Some(true));
        let running = tool.result.is_none() && self.running(tool.call);
        let ending = (failed || running).then(|| chip_ending(event, tool.result.map(|i| &events[i]), running, |result| self.result_lines(result)));
        let line = tree_call_line(format!("tree-{key}"), event, ending, failed, cx).on_click(toggle);
        let body = open.then(|| self.tool_body(tool, row, cx).into_any_element());
        tree_row(last, line, body).into_any_element()
    }

    /// Lista de tarefas do agente, no lugar das chamadas TaskCreate/TaskUpdate: anel de progresso, quanto falta
    /// e os passos. O passo em andamento mostra o que o agente diz que está fazendo.
    pub(super) fn render_tasks(&mut self, row: &str, tasks: &[Task], cx: &mut Context<Self>) -> AnyElement {
        let done = tasks.iter().filter(|t| t.status == TaskStatus::Completed).count();
        let left = tasks.len() - done;
        let label = if left == 0 { tr("tasks_all_done") } else { counted("tasks_left", left) };
        let minimized_key = format!("{row}#min");
        let minimized = self.expanded.contains(&minimized_key);
        let toggle_min = minimized_key.clone();
        let header = div().flex().items_center().gap(px(10.))
            .child(ProgressCircle::new(SharedString::from(format!("{row}-ring"))).size(px(16.))
                .color(if left == 0 { theme::success() } else { theme::accent() }).value(100. * done as f32 / tasks.len().max(1) as f32)
                .accessibility_label(format!("{done}/{}", tasks.len())))
            .child(div().flex_1().min_w_0().truncate().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).child(label))
            .child(Button::new(SharedString::from(format!("{row}-minimize"))).ghost().xsmall()
                .icon(chrome::small_icon(if minimized { IconName::ChevronDown } else { IconName::Minus }, 14., theme::muted()))
                .tooltip(tr(if minimized { "tasks_expand" } else { "tasks_minimize" }))
                .accessibility_label(tr(if minimized { "tasks_expand" } else { "tasks_minimize" }))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_min.clone(), cx))));
        let steps: Vec<AnyElement> = if minimized { Vec::new() } else { tasks.iter().map(|task| {
            // Identidade pela chave da tarefa, nunca pela posição: uma tarefa que sai não passa o estado aberto para a vizinha.
            let key = format!("{row}#{}", task.key);
            let open = self.expanded.contains(&key);
            // Em andamento gira, parado quando o sistema pede movimento reduzido.
            let glyph = match task.status {
                TaskStatus::Pending => chrome::small_icon(IconName::CircleDashed, 14., theme::faint()).into_any_element(),
                TaskStatus::InProgress => chrome::Spinner::new(SharedString::from(format!("{key}-spin")), IconName::LoaderCircle, px(14.), theme::accent()).into_any_element(),
                TaskStatus::Completed => chrome::small_icon(IconName::CircleCheck, 14., theme::success()).into_any_element(),
            };
            let active = task.status == TaskStatus::InProgress;
            let subject = if task.subject.is_empty() { tr("tasks_untitled") } else { task.subject.clone() };
            let shown = if active && !task.active_form.is_empty() { task.active_form.clone() } else { subject.clone() };
            let has_description = !task.description.is_empty();
            let state = tr(match task.status { TaskStatus::Pending => "tasks_pending", TaskStatus::InProgress => "tasks_in_progress", TaskStatus::Completed => "tasks_completed" });
            let toggle = key.clone();
            div().flex().flex_col()
                .child(Button::new(SharedString::from(format!("{row}-step-{}", task.key)))
                    .custom(ButtonCustomVariant::new(cx).color(if active { theme::accent_dim() } else { transparent_black() })
                        .foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                    .w_full().h(px(28.)).px(px(6.)).rounded(px(6.)).when(active, |el| el.bg(theme::accent_dim()))
                    .accessibility_label(format!("{subject}: {state}"))
                    .child(div().w_full().min_w_0().flex().items_center().gap(px(8.)).text_size(px(13.))
                        .child(glyph)
                        .child(div().flex_1().min_w_0().truncate()
                            .text_color(match task.status { TaskStatus::Completed => theme::muted(), TaskStatus::InProgress => theme::accent_text(), _ => theme::text() })
                            .child(shown)))
                    .when(has_description, |el| el.on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx)))))
                .when(open && has_description, |el| el.child(div().pl(px(28.)).pr(px(6.)).pb(px(4.)).text_size(px(12.5))
                    .text_color(theme::muted()).whitespace_normal().child(task.description.clone())))
                .into_any_element()
        }).collect() };
        // Respiro próprio: vizinho de uma tabela de chamadas, a borda de um encostava na do outro.
        div().w_full().max_w(px(380.)).my(px(6.)).flex().flex_col().gap(px(6.)).px(px(12.)).py(px(10.)).rounded(px(12.))
            .border_1().border_color(theme::border()).bg(theme::boxed())
            .child(header)
            .when(!steps.is_empty(), |el| el.child(div().flex().flex_col().gap(px(2.)).children(steps)))
            .into_any_element()
    }

    /// Resposta com tabela numérica: o texto em trechos e, em cada tabela, o botão Gráfico/Tabela e, com o
    /// gráfico aberto e mais de uma coluna numérica, a escolha da coluna.
    pub(super) fn render_charted(&mut self, id: &str, markdown: &str, tables: &[Table], cx: &mut Context<Self>) -> Vec<AnyElement> {
        let lines: Vec<&str> = markdown.lines().collect();
        let mut out = Vec::new();
        let mut cursor = 0;
        let decimal = tr("decimal").chars().next().unwrap_or(',');
        fn text(this: &mut Hangar, key: String, row: &str, source: String, out: &mut Vec<AnyElement>, cx: &mut Context<Hangar>) {
            if source.trim().is_empty() { return; }
            let view = this.text_view(&key, row, source, cx);
            out.push(chat_text(&view, cx).on_link_click(open_web_link)
                .markdown_extensions(citation_extensions(&key, cx.weak_entity())).into_any_element());
        }
        for (n, table) in tables.iter().enumerate() {
            text(self, format!("{id}#s{n}"), id, lines[cursor..table.start].join("\n"), &mut out, cx);
            cursor = table.end;
            let key = format!("{id}#t{n}");
            let chart = self.expanded.contains(&key);
            let column = self.table_column.get(&key).copied().unwrap_or(0).min(table.columns.len() - 1);
            let toggle = key.clone();
            let toolbar = div().flex().flex_wrap().items_center().gap(px(6.))
                .child(Button::new(SharedString::from(format!("{key}-toggle"))).outline().xsmall()
                    .icon(chrome::small_icon(if chart { IconName::Table } else { IconName::ChartColumn }, 13., theme::muted()))
                    .label(tr(if chart { "table_show_table" } else { "table_show_chart" }))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle.clone(), cx))))
                .when(chart && table.columns.len() > 1, |el| el.children(table.columns.iter().enumerate().map(|(c, col)| {
                    let key = key.clone();
                    let on = c == column;
                    Button::new(SharedString::from(format!("{key}-col-{c}")))
                        .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
                            .foreground(if on { theme::accent_text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
                        .xsmall().rounded(px(6.)).selected(on).label(col.title.clone())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.table_column.insert(key.clone(), c);
                            this.toggle_redraw(&key, cx);
                        }))
                })));
            out.push(toolbar.into_any_element());
            if chart {
                let values = &table.columns[column].values;
                // A banda do gráfico é chave: rótulo repetido ganha o número da linha para não fundir barras.
                let mut seen = HashSet::new();
                let data: Vec<(String, f64)> = table.labels.iter().zip(values).enumerate()
                    .map(|(r, (label, &v))| (if seen.insert(label.clone()) { label.clone() } else { format!("{label} ({})", r + 1) }, v)).collect();
                let accent = theme::accent();
                let percent = table.columns[column].percent;
                // Sem `id`: a dica de passar o mouse do kit escreve o valor cru (46900000, 35 sem "%"); o valor com
                // a unidade da coluna fica no rótulo de cada barra.
                out.push(div().h(px(200.)).w_full().px(px(12.)).pt(px(14.)).pb(px(8.)).rounded(px(10.)).border_1().border_color(theme::border())
                    .child(BarChart::new(data)
                        .band(|d: &(String, f64)| d.0.clone()).value(|d: &(String, f64)| d.1)
                        .label(move |d: &(String, f64)| tables::short(d.1, decimal, percent))
                        .fill(move |_, _, _, _| accent).corner_radii(Corners::all(px(4.))))
                    .into_any_element());
            } else {
                text(self, key.clone(), id, lines[table.start..table.end].join("\n"), &mut out, cx);
            }
        }
        text(self, format!("{id}#tail"), id, lines[cursor.min(lines.len())..].join("\n"), &mut out, cx);
        out
    }

    /// Mudança dentro de uma linha que não abre nem fecha nada (a coluna do gráfico): só remede e redesenha.
    fn toggle_redraw(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(row) = self.row_ids.iter().position(|id| key.strip_prefix(id.as_str()).is_some_and(|rest| rest.starts_with('#'))) {
            self.list_state.remeasure_items(row..row + 1);
        }
        cx.notify();
    }
}
