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

fn chip_box() -> Div {
    div().flex().flex_col().rounded(px(10.)).border_1().border_color(theme::border()).overflow_hidden()
}

impl Hangar {
    /// Fim da linha no modo Chips: rodando, erro, linhas postas e tiradas, ou o desfecho por ferramenta.
    fn chip_ending(&self, tool: Tool) -> AnyElement {
        let events = &self.chat.events;
        let call = &events[tool.call];
        let name = call.tool_name.as_deref();
        let text = |t: String, color: Hsla| div().flex_shrink_0().max_w(px(260.)).truncate().text_size(px(12.5)).text_color(color).child(t);
        let Some(result) = tool.result.map(|i| &events[i]) else {
            return if self.running(tool.call) { text(tr("chip_running"), theme::accent()).into_any_element() }
                else { text(tr("tool_no_result"), theme::faint()).into_any_element() };
        };
        let raw = result.result.as_deref().unwrap_or("").trim();
        if result.is_error == Some(true) {
            let first = raw.lines().map(str::trim).find(|l| !l.is_empty()).map(|l| conversation::one_line(l, 72)).unwrap_or_else(|| tr("tool_failed"));
            return div().flex_shrink_0().flex().items_center().gap_1().child(chrome::small_icon(IconName::CircleX, 13., theme::warning()))
                .child(text(first, theme::warning())).into_any_element();
        }
        if let Some((added, removed)) = conversation::edit_counts(name, call.tool_input.as_ref()) {
            return div().flex_shrink_0().flex().gap(px(6.)).font_family(theme::MONO).text_size(px(12.))
                .child(div().text_color(theme::success()).child(format!("+{added}")))
                .when(removed > 0, |el| el.child(div().text_color(theme::removed()).child(format!("−{removed}"))))
                .into_any_element();
        }
        let lines = self.result_lines(result);
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

    /// Uma chamada no modo Chips: linha da tabela com verbo, resumo em mono e desfecho à direita.
    fn render_chip(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let call = &self.chat.events[tool.call];
        let key = call.id.clone();
        let name = call.tool_name.clone();
        let verb = verb(name.as_deref());
        let chip = chip_text(call);
        let open = self.expanded.contains(&key);
        let ending = self.chip_ending(tool);
        let toggle_key = key.clone();
        let label = format!("{verb} {chip}");
        let button = Button::new(SharedString::from(format!("chip-{key}")))
            .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
            .w_full().h(px(34.)).px(px(12.)).rounded(px(0.))
            .toggled(open).accessibility_label(label)
            .child(div().w_full().min_w_0().flex().items_center().gap(px(8.)).text_size(px(13.5))
                // Tabela do mock: verbo numa coluna, o resumo em mono, o desfecho alinhado à direita.
                // Coluna mínima, não fixa: nome de ferramenta sem verbo (ToolSearch, MCP) empurra o resumo.
                .child(div().min_w(px(64.)).flex_shrink_0().text_color(theme::muted()).child(verb))
                .child(div().flex_1().min_w_0().truncate().font_family(theme::MONO).text_size(px(12.5)).child(chip))
                .child(ending)
                .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 14., theme::faint())))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
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
        let events = &self.chat.events;
        let text = conversation::family_counts(events, tools).into_iter().map(|(family, n)| counted(match family {
            Family::Read => "chip_title_read", Family::Search => "chip_title_search", Family::Edit => "chip_title_edit",
            Family::Create => "chip_title_create", Family::Run => "chip_title_run", Family::Other => "chip_title_other",
        }, n)).collect::<Vec<_>>().join(" · ");
        let mut chars = text.chars();
        let title: String = chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or_default();
        let errors = tools.iter().filter(|t| t.result.is_some_and(|i| events[i].is_error == Some(true))).count();
        let running = tools.iter().any(|t| t.result.is_none() && self.running(t.call));
        let last = tools.last().map(|t| conversation::family(events[t.call].tool_name.as_deref())).unwrap_or(Family::Other);
        let calls = counted("chip_calls", tools.len());
        let open = (tools.len() <= CHIPS_OPEN_UP_TO) != self.expanded.contains(row);
        let toggle_key = row.to_owned();
        let header = self.disclosure(row, open)
            .accessibility_label(format!("{title} · {calls}"))
            .child(chrome::small_icon(family_icon(last), 14., if errors > 0 { theme::warning() } else { theme::muted() }))
            .child(div().min_w_0().truncate().text_color(if errors > 0 { theme::warning() } else { theme::text() }).child(title))
            .child(div().flex_shrink_0().text_color(theme::faint()).child(format!("· {calls}")))
            .child(div().flex_1())
            .when(running, |el| el.child(div().flex_shrink_0().text_color(theme::accent()).child(tr("chip_running"))))
            .when(errors > 0, |el| el.child(div().flex_shrink_0().text_color(theme::warning()).child(tr("tools_errors").replace("{n}", &errors.to_string()))))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let rows: Vec<AnyElement> = if open { tools.iter().map(|&tool| self.render_chip(tool, row, cx)).collect() } else { Vec::new() };
        div().flex().flex_col().gap_1().child(header)
            .when(open, |el| el.child(chip_box()
                .children(rows.into_iter().enumerate().map(|(n, row)| div().when(n > 0, |el| el.border_t_1().border_color(theme::border())).child(row)))))
            .into_any_element()
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
            out.push(TextView::new(&view).selectable(true).scrollable(false).on_link_click(open_web_link).into_any_element());
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
