//! Estilo Claude Desktop das chamadas de ferramenta: chamadas seguidas viram uma linha cinza com o resumo, que abre num
//! cartão com uma linha por chamada; a linha aberta mostra o corpo do Clássico, com o código de saída acima da saída.
use super::*;
use crate::conversation::Family;

/// Família da chamada: a raiz das chaves (`ferr_desktop_<raiz>` no resumo, `ferr_desktop_linha_<raiz>` na linha) e, nas
/// outras ferramentas, o nome, que separa uma da outra. É a do modo Chips, com a web e o carregador de ferramentas à parte.
fn family(name: Option<&str>) -> (&'static str, String) {
    let root = match name {
        Some("WebSearch" | "WebFetch") => "web",
        Some("ToolSearch") => "ferramentas",
        _ => match conversation::family(name) {
            Family::Run => "rodou", Family::Read => "leu", Family::Edit => "editou", Family::Create => "criou", Family::Search => "buscou",
            Family::Other => return ("outra", name.map(conversation::tool_display_name).unwrap_or_else(|| tr("tool"))),
        },
    };
    (root, String::new())
}

/// Resumo do grupo: uma frase por família, na ordem em que cada uma aparece, as falhas na frase da família e as que
/// ainda rodam no fim ("Executou 4 comandos (2 falhas)", "Carregou ferramentas, Pesquisou na web 1 vez"). Cada chamada
/// vem como (chamada, falhou, rodando).
pub(crate) fn desktop_summary<'a>(calls: impl IntoIterator<Item = (&'a ChatEvent, bool, bool)>) -> String {
    let mut families: Vec<((&'static str, String), usize, usize)> = Vec::new();
    let mut live = 0;
    for (call, failed, running) in calls {
        let key = family(call.tool_name.as_deref());
        let failed = usize::from(failed);
        live += usize::from(running);
        match families.iter_mut().find(|(k, ..)| *k == key) {
            Some((_, n, f)) => { *n += 1; *f += failed; }
            None => families.push((key, 1, failed)),
        }
    }
    let mut parts: Vec<String> = families.iter().map(|((root, name), n, failed)| {
        let key = format!("ferr_desktop_{root}");
        let mut phrase = if *root == "ferramentas" { crate::i18n::tr_shared(&key, &[]) }
            else { super::terminal_look::count_with(&format!("{key}_1"), &key, *n, &[("nome", name.as_str())]) };
        if *failed > 0 { phrase.push(' '); phrase.push_str(&super::terminal_look::count_with("ferr_desktop_falhas_1", "ferr_desktop_falhas", *failed, &[])); }
        phrase
    }).collect();
    if live > 0 { parts.push(crate::i18n::tr_shared("ferr_desktop_executando", &[("n", &live.to_string())])); }
    parts.join(", ")
}

/// `Exit code N` na primeira linha não vazia do resultado, como o Claude Code escreve a saída de um comando que falhou.
fn exit_code(output: &str) -> Option<i32> {
    let number = output.lines().map(str::trim).find(|l| !l.is_empty())?.strip_prefix("Exit code ")?;
    // O `parse` aceita "+1"; o Claude Code nunca escreve o sinal.
    number.parse().ok().filter(|_| !number.starts_with('+'))
}

/// "Código de saída N" acima da saída aberta, em toda pele; saída sem o código, nada.
pub(super) fn exit_code_line(output: &str) -> Option<Div> {
    let code = exit_code(output)?;
    Some(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(theme::warning())
        .child(crate::i18n::tr_shared("ferr_desktop_codigo_saida", &[("n", &code.to_string())])))
}

/// O grupo: a linha do resumo e, aberto, o cartão com uma linha por chamada.
pub(crate) fn group_block(header: Button, open: bool, rows: Vec<AnyElement>) -> AnyElement {
    div().flex().flex_col().gap_1().child(div().flex().min_w_0().child(header))
        .when(open, |el| el.child(super::rows::chip_table(rows))).into_any_element()
}

/// Uma chamada do cartão: a linha e, aberta, o corpo.
pub(super) fn call_block(line: Button, body: Option<Div>) -> AnyElement {
    div().flex().flex_col().child(line).children(body.map(|body| body.px(px(12.)))).into_any_element()
}

fn chevron(open: bool) -> Icon { chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 13., theme::faint()) }

/// A linha cinza do grupo, com o chevron logo depois do texto. Quem chama liga o clique.
pub(crate) fn summary_button(id: String, summary: String, open: bool, cx: &App) -> Button {
    Button::new(SharedString::from(id))
        .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::muted()).hover(theme::hover()).active(theme::hover()))
        .h(px(28.)).px(px(6.)).toggled(open).accessibility_label(summary.clone())
        .child(div().min_w_0().flex().items_center().gap(px(4.)).text_size(px(13.5)).text_color(theme::muted())
            .child(div().min_w_0().truncate().child(summary))
            .child(chevron(open)))
}

/// Uma chamada no cartão: verbo cinza, alvo, "Falhou" e o chevron logo depois. Quem chama liga o clique.
pub(crate) fn call_button(id: String, call: &ChatEvent, failed: bool, open: bool, cx: &App) -> Button {
    let (root, name) = family(call.tool_name.as_deref());
    let verb = crate::i18n::tr_shared(&format!("ferr_desktop_linha_{root}"), &[]);
    let target = if root == "outra" { name } else { super::rows::chip_text(call) };
    let failed = failed.then(|| super::activity::web("formato_tool_falhou"));
    Button::new(SharedString::from(id))
        .custom(ButtonCustomVariant::new(cx).color(transparent_black()).foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
        .w_full().h(px(36.)).px(px(12.)).rounded(px(0.)).toggled(open)
        .accessibility_label(format!("{verb} {target}{}", failed.as_ref().map(|f| format!(". {f}")).unwrap_or_default()))
        .child(div().w_full().min_w_0().flex().items_center().gap(px(8.)).text_size(px(13.))
            .child(div().flex_shrink_0().text_color(theme::muted()).child(verb))
            .child(div().min_w_0().truncate().text_color(theme::text()).child(target))
            .when_some(failed, |el, failed| el.child(div().flex_shrink_0().text_color(theme::warning()).child(failed)))
            .child(chevron(open))
            .child(div().flex_1()))
}

impl Hangar {
    /// Grupo no estilo Claude Desktop: a linha do resumo e, aberto, o cartão. Nasce fechado: a chave ausente do
    /// `expanded` é o fechado.
    pub(super) fn render_desktop_group(&mut self, row: &str, tools: &[Tool], cx: &mut Context<Self>) -> AnyElement {
        let events = &self.chat.events;
        let summary = desktop_summary(tools.iter().map(|t| (&events[t.call], t.result.is_some_and(|i| events[i].is_error == Some(true)),
            t.result.is_none() && self.running(t.call))));
        let open = self.expanded.contains(row);
        let toggle_key = row.to_owned();
        let header = summary_button(format!("desktop-{row}"), summary, open, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let rows: Vec<AnyElement> = if open { tools.iter().map(|&tool| self.render_desktop_call(tool, row, cx)).collect() } else { Vec::new() };
        group_block(header, open, rows)
    }

    /// Chamada solta (dentro do pensamento): a mesma linha, num cartão de uma linha.
    pub(super) fn render_desktop_tool(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let line = self.render_desktop_call(tool, row, cx);
        super::rows::chip_box().child(line).into_any_element()
    }

    fn render_desktop_call(&mut self, tool: Tool, row: &str, cx: &mut Context<Self>) -> AnyElement {
        let events = &self.chat.events;
        let call = &events[tool.call];
        let key = call.id.clone();
        let open = self.expanded.contains(&key);
        let failed = tool.result.is_some_and(|i| events[i].is_error == Some(true));
        let toggle_key = key.clone();
        let line = call_button(format!("desktop-{key}"), call, failed, open, cx)
            .on_click(cx.listener(move |this, _, _, cx| this.toggle(toggle_key.clone(), cx)));
        let body = open.then(|| self.tool_body(tool, row, cx));
        call_block(line, body)
    }
}

#[cfg(test)]
mod tests {
    use super::{ChatEvent, Tool, desktop_summary, exit_code};

    fn call(id: &str, name: &str) -> ChatEvent {
        ChatEvent { kind: "tool_use".into(), id: id.into(), tool_name: Some(name.into()), tool_use_id: Some(id.into()), ..Default::default() }
    }

    fn result(id: &str, error: bool) -> ChatEvent {
        ChatEvent { kind: "tool_result".into(), id: format!("r-{id}"), tool_use_id: Some(id.into()), is_error: Some(error), ..Default::default() }
    }

    /// Cada par é (nome, desfecho): `Some(true)` falhou, `Some(false)` deu certo, `None` sem resultado.
    fn summary(calls: &[(&str, Option<bool>)]) -> String {
        crate::i18n::english_in_this_thread(false);
        let mut events = Vec::new();
        let mut tools = Vec::new();
        for (n, (name, outcome)) in calls.iter().enumerate() {
            let id = format!("c{n}");
            events.push(call(&id, name));
            let at = events.len() - 1;
            let done = outcome.map(|error| { events.push(result(&id, error)); events.len() - 1 });
            tools.push(Tool { call: at, result: done });
        }
        desktop_summary(tools.iter().map(|t| (&events[t.call], t.result.is_some_and(|i| events[i].is_error == Some(true)), t.result.is_none())))
    }

    #[test]
    fn desktop_summary_counts_failed_commands() {
        assert_eq!(summary(&[("Bash", Some(false)), ("Bash", Some(true)), ("Bash", Some(false)), ("Bash", Some(true))]),
            "Executou 4 comandos (2 falhas)");
    }

    #[test]
    fn desktop_summary_counts_powershell_as_command() {
        assert_eq!(summary(&[("PowerShell", Some(true))]), "Executou 1 comando (1 falha)");
    }

    #[test]
    fn desktop_summary_keeps_the_order_families_first_appear() {
        assert_eq!(summary(&[("ToolSearch", Some(false)), ("WebSearch", Some(false))]), "Carregou ferramentas, Pesquisou na web 1 vez");
    }

    #[test]
    fn desktop_summary_counts_each_family() {
        assert_eq!(summary(&[("Read", Some(false)), ("Read", Some(false)), ("Edit", Some(false))]), "Leu 2 arquivos, Editou 1 arquivo");
    }

    #[test]
    fn desktop_summary_adds_the_calls_still_running() {
        assert_eq!(summary(&[("Bash", Some(false)), ("Bash", None)]), "Executou 2 comandos, 1 em execução");
    }

    #[test]
    fn desktop_summary_names_other_tools() {
        assert_eq!(summary(&[("mcp__hangar__sessions", Some(false)), ("mcp__hangar__sessions", Some(false))]), "Usou hangar · sessions 2 vezes");
    }

    #[test]
    fn exit_code_reads_the_first_line() { assert_eq!(exit_code("Exit code 1\nerro"), Some(1)); }

    #[test]
    fn exit_code_trims_the_line() { assert_eq!(exit_code("  Exit code 127"), Some(127)); }

    #[test]
    fn exit_code_ignores_later_lines() { assert_eq!(exit_code("ok\nExit code 1"), None); }

    #[test]
    fn exit_code_needs_the_number() { assert_eq!(exit_code("Exit code"), None); }
}
