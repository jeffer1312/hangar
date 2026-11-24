//! Tela Agentes: uma tabela (um agente por coluna, o que o Hangar liga em cada um por linha) e o painel do agente
//! escolhido ao lado. Sem painel aberto, a tabela usa a largura toda.
use super::*;

/// Linhas da tabela: (chave do rótulo, ids de item que respondem por ela, na ordem de preferência).
const LINK_ROWS: [(&str, &[&str]); 3] =
    [("agents_row_wrapper", &["wrapper"]), ("agents_row_hooks", &["hooks", "extensoes"]), ("agents_row_logins", &["credenciais", "contas"])];
const CONFIG_ROWS: [(&str, &[&str]); 3] =
    [("agents_row_skills", &["skills"]), ("agents_row_mcp", &["mcp"]), ("agents_row_model", &["modelo"])];
/// Itens que o painel põe em "Ligação com o Hangar"; o resto é configuração do agente.
const LINK_ITEMS: [&str; 11] =
    ["wrapper", "hooks", "extensoes", "credenciais", "fullscreen", "statusline", "versao", "bloco", "default_terminal", "truecolor", "titulo"];
const LABEL_W: f32 = 170.;
/// Agentes com conta própria na tela Contas; os outros usam as chaves do app.
const WITH_ACCOUNT: [&str; 3] = ["claude", "codex", "antigravity"];

#[derive(Clone, Copy, PartialEq)]
enum Tone { Ok, Warn, Plain }

fn with(key: &str, params: &[(&str, String)]) -> String {
    params.iter().fold(tr(key), |text, (k, v)| text.replace(&format!("{{{k}}}"), v))
}

/// "1 pronto" / "N prontos" pela chave e a irmã terminada em `_1`.
fn plural(key: &str, n: usize) -> String {
    if n == 1 { tr(&format!("{key}_1")) } else { with(key, &[("n", n.to_string())]) }
}

fn listed(item: &Item, key: &str) -> usize {
    item.params.get(key).map_or(0, |l| l.split(", ").filter(|s| !s.is_empty()).count())
}

/// A célula curta da tabela; o texto inteiro do item fica no painel.
fn cell(cli: &str, ids: &[&str], items: &[Item]) -> (Tone, String) {
    let Some(item) = ids.iter().find_map(|id| items.iter().find(|i| i.id == *id)) else {
        return match (ids.first().copied(), cli) {
            (Some("skills"), "claude") => (Tone::Ok, tr("agents_cell_native")),
            (Some("skills"), "codex") => (Tone::Ok, tr("agents_cell_integration")),
            _ => (Tone::Plain, "—".to_owned()),
        };
    };
    let param = |k: &str| item.params.get(k).cloned().unwrap_or_default();
    let count = |k: &str| param(k).parse::<usize>().unwrap_or(0);
    let text = match item.codigo.as_str() {
        "wrapper_ok" | "credenciais_ok" => tr("agents_cell_on"),
        "wrapper_falta" | "faltam" | "faltam_n" | "extensoes_outra_fonte" => tr("agents_cell_missing"),
        "hooks_ok" | "hooks_codex" => with("agents_cell_hooks", &[("n", param("n"))]),
        "extensoes_ok" => with("agents_cell_extensions", &[("n", param("n"))]),
        "agy_hooks_sem_uso" => tr("agents_cell_first_run"),
        "credenciais_faltam" => with("agents_cell_missing_n", &[("n", listed(item, "faltam").to_string())]),
        "contas_ok" | "agy_contas" => plural("agents_cell_accounts", count("n")),
        "so_conta_padrao" => plural("agents_cell_accounts", 1),
        "skills_ok" => param("n"),
        "sem_ponte" => tr("agents_cell_native"),
        "agy_skills_desligadas" | "ponte_ausente" | "ponte_fora_da_config" => tr("agents_cell_off"),
        "links_pendurados" => with("agents_cell_broken", &[("n", param("n"))]),
        "mcp_ok" => plural("agents_cell_mcp", count("n")),
        "mcp_nenhum" => tr("agents_cell_none"),
        "modelo_padrao" => param("modelo"),
        "modelo_padrao_nenhum" => tr("agents_cell_cli_default"),
        "config_ilegivel" => tr("agents_cell_unreadable"),
        _ => item_text(item),
    };
    let tone = match item.ok { Some(false) => Tone::Warn, Some(true) if !item.info => Tone::Ok, _ => Tone::Plain };
    (tone, text)
}

/// Situação do agente numa frase curta: o que mais pede ação aparece pelo nome.
fn status(h: &Cli) -> (Hsla, String) {
    if !h.instalado { return (theme::faint(), tr("agents_status_off")); }
    let bad: Vec<&Item> = h.itens.iter().filter(|i| i.ok == Some(false)).collect();
    match bad.as_slice() {
        [] => (theme::success(), tr("agents_status_ready")),
        [one] if one.codigo == "credenciais_faltam" => (theme::warning(), with("agents_status_logins", &[("n", listed(one, "faltam").to_string())])),
        [one] if one.codigo == "agy_skills_desligadas" => (theme::warning(), tr("agents_status_skills")),
        many => (theme::warning(), plural("agents_status_fix", many.len())),
    }
}

fn agent_glyph(id: &str, size: f32) -> Div {
    if id == "tmux" {
        return div().size(px(size)).flex_shrink_0().rounded(px(size * 0.22)).bg(theme::raised()).flex().items_center().justify_center()
            .text_size(px(size * 0.5)).font_weight(FontWeight::BOLD).text_color(theme::muted()).child("⌗");
    }
    chrome::provider_glyph(id, size)
}

fn pill(tone: Tone, text: String) -> Div {
    let base = div().flex().items_center().gap(px(5.)).min_w_0().max_w_full().text_size(px(12.5));
    let mark = |glyph: &'static str, color: Hsla| div().flex_shrink_0().font_weight(FontWeight::BOLD).text_color(color).child(glyph);
    let label = div().min_w_0().truncate().child(text);
    match tone {
        Tone::Ok => base.px(px(8.)).py(px(2.)).rounded_full().bg(theme::success().opacity(0.12)).text_color(theme::success_text())
            .child(mark("✓", theme::success())).child(label),
        Tone::Warn => base.px(px(8.)).py(px(2.)).rounded_full().bg(theme::warning().opacity(0.14)).text_color(theme::warning_text())
            .child(mark("!", theme::warning())).child(label),
        Tone::Plain => base.text_color(theme::muted()).child(label),
    }
}

fn status_line(color: Hsla, text: String, warn: bool) -> Div {
    div().flex().items_center().gap(px(6.)).min_w_0().text_size(px(12.))
        .text_color(if warn { theme::warning_text() } else { theme::muted() })
        .child(div().size(px(7.)).flex_shrink_0().rounded_full().bg(color)).child(div().min_w_0().truncate().child(text))
}

fn panel_section(title: String) -> Div {
    div().flex().flex_col().gap_1().px(px(18.)).py(px(14.)).border_t_1().border_color(theme::border())
        .child(div().pb(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::faint()).child(title))
}

impl Hangar {
    pub(super) fn select_agent(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        self.harness.selected = if self.harness.selected == id { None } else { id };
        cx.notify();
    }

    /// Copia para cada agente os logins que o app tem e ele não, um conserto depois do outro.
    fn copy_all_logins(&mut self, cx: &mut Context<Self>) {
        let Some(list) = &self.harness.list else { return };
        let mut queue: std::collections::VecDeque<(String, String, String)> = list.iter()
            .flat_map(|h| h.itens.iter().filter_map(move |i| i.conserto.as_ref().filter(|c| c.starts_with("sync:"))
                .map(|c| (c.clone(), h.id.clone(), i.id.clone()))))
            .collect();
        let Some((id, cli, item)) = queue.pop_front() else { return };
        self.harness.bulk = queue;
        self.repair_harness(id, cli, item, cx);
    }

    /// Próximo conserto da fila do "Copiar para todos"; falha para a fila, e o erro fica à vista no agente dele.
    pub(super) fn continue_bulk(&mut self, ok: bool, cx: &mut Context<Self>) {
        if !ok { self.harness.bulk.clear(); return; }
        if let Some((id, cli, item)) = self.harness.bulk.pop_front() { self.repair_harness(id, cli, item, cx); }
    }

    pub(super) fn render_agents(&self, list: &[Cli], cx: &mut Context<Self>) -> Div {
        let agents: Vec<&Cli> = list.iter().filter(|h| h.id != "tmux").collect();
        let selected = self.harness.selected.as_deref().and_then(|id| list.iter().find(|h| h.id == id));
        let table = self.agents_table(&agents, selected.map(|h| h.id.as_str()), cx);
        let tmux = list.iter().find(|h| h.id == "tmux").map(|h| self.tmux_row(h, selected.is_some_and(|s| s.id == "tmux"), cx));
        let main = div().flex_1().min_w_0().flex().flex_col().gap_3().child(table).children(tmux)
            .child(div().px_1().text_size(px(12.)).text_color(theme::faint()).child(tr("agents_hint")));
        div().flex().flex_col().gap_4()
            .child(self.agents_summary(&agents, cx))
            .child(div().flex().items_start().gap_4().child(main).children(selected.map(|h| self.agent_panel(h, cx))))
    }

    fn agents_summary(&self, agents: &[&Cli], cx: &mut Context<Self>) -> Div {
        let off = agents.iter().filter(|h| !h.instalado).count();
        let ready = agents.iter().filter(|h| h.instalado && !h.itens.iter().any(|i| i.ok == Some(false))).count();
        let fix = agents.len() - ready - off;
        let bar = div().w(px(180.)).h(px(8.)).flex_shrink_0().flex().rounded_full().overflow_hidden().bg(theme::raised())
            .children([(ready, theme::success()), (fix, theme::warning()), (off, theme::faint())].into_iter().filter(|(n, _)| *n > 0)
                .map(|(n, color)| div().h_full().flex_basis(px(0.)).flex_grow(n as f32).bg(color)));
        let legend = div().flex().flex_wrap().gap_x_3().text_size(px(12.5)).text_color(theme::muted())
            .child(plural("agents_sum_ready", ready)).child(plural("agents_sum_fix", fix)).child(plural("agents_sum_off", off));
        let missing: Vec<&str> = agents.iter().filter(|h| h.itens.iter().any(|i| i.conserto.as_deref().is_some_and(|c| c.starts_with("sync:"))))
            .map(|h| h.nome.as_str()).collect();
        let busy = self.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none());
        let bulk_running = busy && (!self.harness.bulk.is_empty() || self.harness.repair.as_ref().is_some_and(|r| r.item == "credenciais"));
        settings_box().flex_row().items_center().gap_4().px(px(16.)).py(px(12.))
            .child(bar).child(legend)
            .when(!missing.is_empty(), |el| el.child(div().ml_auto().flex().items_center().gap_3()
                .child(div().text_size(px(12.5)).text_color(theme::warning_text()).whitespace_normal()
                    .child(with("agents_logins_missing", &[("lista", missing.join(", "))])))
                .child(Button::new("agents-copy-all").primary().small().flex_shrink_0().label(tr("agents_copy_all"))
                    .loading(bulk_running).disabled(busy)
                    .on_click(cx.listener(|this, _, _, cx| this.copy_all_logins(cx))))))
    }

    fn agents_table(&self, agents: &[&Cli], selected: Option<&str>, cx: &mut Context<Self>) -> Stateful<Div> {
        let column = |id: &str| div().flex_basis(px(0.)).flex_grow(1.).min_w_0().when(selected == Some(id), |el| el.bg(theme::hover()));
        let head = div().flex().items_stretch().child(div().w(px(LABEL_W)).flex_shrink_0())
            .children(agents.iter().map(|h| {
                let (color, text) = status(h);
                let warn = h.instalado && h.itens.iter().any(|i| i.ok == Some(false));
                let id = h.id.clone();
                column(&h.id).id(SharedString::from(format!("agents-col-{}", h.id))).role(Role::Button).aria_label(h.nome.clone())
                    .cursor_pointer().hover(|el| el.bg(theme::hover())).px(px(10.)).pt(px(14.)).pb(px(10.))
                    .when(selected == Some(h.id.as_str()), |el| el.border_t_2().border_color(theme::text()))
                    .when(!h.instalado, |el| el.opacity(0.7))
                    .flex().flex_col().gap(px(7.))
                    .child(agent_glyph(&h.id, 30.))
                    .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).truncate().child(h.nome.clone()))
                    .child(status_line(color, text, warn))
                    .on_click(cx.listener(move |this, _, _, cx| this.select_agent(Some(id.clone()), cx)))
            }));
        let group = |key: &str| div().px(px(16.)).pt(px(14.)).pb(px(4.)).border_t_1().border_color(theme::border())
            .text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(theme::faint()).child(tr(key));
        let mut rows: Vec<AnyElement> = Vec::new();
        for (title, block) in [("agents_group_link", &LINK_ROWS), ("agents_group_config", &CONFIG_ROWS)] {
            rows.push(group(title).into_any_element());
            for (label, ids) in block.iter() {
                let cells = agents.iter().map(|h| {
                    let inner = if h.instalado {
                        let (tone, text) = cell(&h.id, ids, &h.itens);
                        Some(pill(tone, text).into_any_element())
                    } else if ids.first() == Some(&"wrapper") {
                        self.install_offer_compact(h, cx)
                    } else { None };
                    column(&h.id).flex().items_center().px(px(10.)).py(px(9.)).children(inner)
                });
                rows.push(div().flex().items_center().border_t_1().border_color(theme::border())
                    .child(div().w(px(LABEL_W)).flex_shrink_0().px(px(16.)).py(px(9.)).text_size(px(13.)).text_color(theme::muted()).child(tr(label)))
                    .children(cells).into_any_element());
            }
        }
        settings_box().id("agents-table").child(head).children(rows)
    }

    /// O botão Instalar da célula: o servidor sem comando conferido para este sistema manda à página do fornecedor no painel.
    fn install_offer_compact(&self, h: &Cli, cx: &mut Context<Self>) -> Option<AnyElement> {
        let install = self.harness.install.as_ref()?;
        let busy = install.running() || self.harness.starting.is_some();
        let id = h.id.clone();
        if install.comandos.contains_key(&h.id) {
            let (cli, name) = (h.id.clone(), h.nome.clone());
            return Some(Button::new(SharedString::from(format!("agents-{}-install", h.id))).primary().small()
                .label(web("harness_inst_botao")).loading(self.harness.starting.as_deref() == Some(h.id.as_str())).disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.harness.selected = Some(cli.clone());
                    this.confirm_install(cli.clone(), name.clone(), window, cx);
                })).into_any_element());
        }
        Some(Button::new(SharedString::from(format!("agents-{}-how", h.id))).outline().small().label(tr("agents_how_install"))
            .on_click(cx.listener(move |this, _, _, cx| this.select_agent(Some(id.clone()), cx))).into_any_element())
    }

    fn tmux_row(&self, h: &Cli, open: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let (color, text) = status(h);
        let warn = h.instalado && h.itens.iter().any(|i| i.ok == Some(false));
        settings_box().id("agents-tmux").role(Role::Button).aria_label(h.nome.clone()).flex_row().items_center().gap_3().px(px(14.)).py(px(11.))
            .cursor_pointer().hover(|el| el.bg(theme::hover())).when(open, |el| el.border_color(theme::text().opacity(0.4)))
            .child(agent_glyph("tmux", 28.))
            .child(div().flex_1().min_w_0().flex().items_center().gap_2()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(h.nome.clone()))
                .child(div().min_w_0().truncate().text_size(px(12.5)).text_color(theme::muted()).child(tr("agents_tmux_sub"))))
            .child(status_line(color, text, warn))
            .child(chrome::small_icon(IconName::ChevronRight, 14., theme::faint()))
            .on_click(cx.listener(|this, _, _, cx| this.select_agent(Some("tmux".to_owned()), cx)))
    }

    fn agent_panel(&self, h: &Cli, cx: &mut Context<Self>) -> Div {
        let (color, text) = status(h);
        let warn = h.instalado && h.itens.iter().any(|i| i.ok == Some(false));
        let version = if h.instalado { h.versao.clone().filter(|v| !v.is_empty()).unwrap_or_else(|| tr("harness_installed")) }
            else { tr("harness_not_installed") };
        let head = div().flex().items_center().gap_3().px(px(18.)).py(px(16.))
            .child(agent_glyph(&h.id, 40.))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(3.))
                .child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).truncate().child(h.nome.clone()))
                .child(div().truncate().text_size(px(12.)).font_family(theme::MONO).text_color(theme::faint()).child(version)))
            .child(status_line(color, text, warn))
            .child(Button::new("agents-panel-close").ghost().small().icon(IconName::Close).accessibility_label(tr("close"))
                .on_click(cx.listener(|this, _, _, cx| { this.harness.selected = None; cx.notify(); })));
        let running = self.harness.repair.as_ref().is_some_and(|r| r.outcome.is_none());
        // O conserto mais útil vira um botão grande no topo; os outros ficam no item deles.
        let first_fix = h.itens.iter().find(|i| i.ok == Some(false) && i.conserto.is_some());
        let fix = first_fix.map(|item| {
            let id = item.conserto.clone().unwrap_or_default();
            let label = if id.starts_with("sync:") { tr("agents_fix_logins") } else if id == "skills-agy" { tr("agents_fix_skills") } else { tr("harness_fix") };
            let (cli, item_id) = (h.id.clone(), item.id.clone());
            let here = self.harness.repair.as_ref().is_some_and(|r| r.cli == h.id && r.item == item.id && r.outcome.is_none());
            div().flex().items_center().gap_3().px(px(18.)).py(px(12.)).border_t_1().border_color(theme::warning().opacity(0.3))
                .bg(theme::warning().opacity(0.08))
                .child(div().flex_1().min_w_0().text_size(px(13.)).text_color(theme::warning_text()).whitespace_normal().child(item_text(item)))
                .child(Button::new(SharedString::from(format!("agents-{}-fix", h.id))).primary().small().flex_shrink_0().label(label)
                    .loading(here).disabled(running)
                    .on_click(cx.listener(move |this, _, _, cx| this.repair_harness(id.clone(), cli.clone(), item_id.clone(), cx))))
        });
        let account = (h.instalado && h.id != "tmux").then(|| {
            let with_account = WITH_ACCOUNT.contains(&h.id.as_str());
            panel_section(tr(if with_account { "agents_account" } else { "agents_keys" }))
                .child(div().flex().items_center().gap_3()
                    .child(div().flex_1().min_w_0().text_size(px(13.)).text_color(theme::text()).whitespace_normal()
                        .child(tr(&format!("agents_account_{}", if with_account { h.id.as_str() } else { "keys" }))))
                    .when(with_account, |el| el.child(Button::new(SharedString::from(format!("agents-{}-accounts", h.id))).outline().small()
                        .flex_shrink_0().label(tr("agents_manage_accounts"))
                        .on_click(cx.listener(|this, _, window, cx| this.open_settings(Page::Accounts, window, cx))))))
        });
        let (link, config): (Vec<&Item>, Vec<&Item>) = h.itens.iter().partition(|i| LINK_ITEMS.contains(&i.id.as_str()));
        let items = |title: &str, list: Vec<&Item>, cx: &mut Context<Self>| (!list.is_empty()).then(|| panel_section(tr(title))
            .children(list.into_iter().map(|item| self.render_harness_item(&h.id, item, cx))));
        let link = items("agents_group_link", link, cx);
        let config = items("agents_group_config", config, cx);
        let orphan = self.harness.repair.as_ref().filter(|r| r.cli == h.id && r.outcome.is_some() && !h.itens.iter().any(|i| i.id == r.item));
        let options = match h.id.as_str() {
            "claude" if h.instalado => Some(panel_section(tr("agents_options")).child(self.claude_options(cx))),
            "codex" => Some(panel_section(tr("agents_options")).child(self.codex_integration(cx))
                .when(h.instalado, |el| el.child(self.codex_options(cx)))),
            _ => None,
        };
        let install = (!h.instalado).then(|| panel_section(web("harness_inst_botao"))
            .children(self.install_offer(h, cx))
            .children(self.harness.install_error.as_ref().filter(|(owner, _)| owner.as_deref() == Some(h.id.as_str()))
                .map(|(_, error)| install_alert(error)))
            .children(self.install_progress(&h.id)));
        // Instalação que acabou de dar certo: o andamento continua à vista no painel do agente já instalado.
        let finished = h.instalado.then(|| self.install_progress(&h.id)).flatten().map(|p| panel_section(web("harness_inst_botao")).child(p));
        let key = (h.id.clone(), "tech".to_owned());
        let open = self.harness.why.contains(&key);
        let this = cx.entity().downgrade();
        let tech = (h.instalado).then(|| panel_section(tr("agents_tech"))
            .child(Disclosure::new(format!("agents-{}-tech", h.id), open, tr("agents_tech_show"), true)
                .name(tr("agents_tech"))
                .on_change(move |open, cx| { let _ = this.update(cx, |this, cx| {
                    this.harness.why.retain(|k| *k != key);
                    if open { this.harness.why.push(key.clone()); }
                    cx.notify();
                }); }))
            .when(open, |el| el.child(div().pt(px(6.)).flex().flex_col().gap(px(4.)).text_size(px(12.5))
                .child(div().flex().gap_3().child(div().w(px(110.)).text_color(theme::faint()).child(tr("agents_tech_version")))
                    .child(div().min_w_0().font_family(theme::MONO).text_color(theme::muted()).child(h.versao.clone().unwrap_or_default())))
                .child(div().flex().gap_3().child(div().w(px(110.)).text_color(theme::faint()).child(tr("agents_tech_program")))
                    .child(div().min_w_0().font_family(theme::MONO).text_color(theme::muted()).child(program(&h.id).to_owned()))))));
        settings_box().w(px(440.)).flex_shrink_0()
            .child(head).children(fix).children(install).children(finished).children(account).children(link).children(config)
            .children(orphan.map(|r| div().px(px(18.)).pb(px(10.)).child(repair_line(r, &item_label(&r.item)))))
            .children(options).children(tech)
    }
}

/// O executável por trás do id do agente.
fn program(id: &str) -> &str { if id == "antigravity" { "agy" } else { id } }
