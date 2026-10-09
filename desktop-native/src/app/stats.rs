//! Estatísticas de uso: página irmã de Custos (`Uso.svelte`, `/api/uso`), aberta pelo link do topo de Custos. Os mesmos
//! cartões, grupos, tabelas e detalhe do web; textos do web por `tr_web`. Soma as mesmas máquinas da página de Custos
//! (`mergeUso`), com a mesma escolha de quais entram.
use super::*;
use std::collections::BTreeMap;
use super::costs::{View, Period, web, web_with, dec, tok, money, money2, chart, project_label, card, card_plain, swatch, hint_text,
    note_box, loading_state, empty_state, error_state, page_frame, Machine, MachinePart, MachineRead, Part, Partial, Warming, set_warming, all_failed,
    warming_note, partial_note, WARM_TRIES};
use super::device::Remote;
use super::settings::segments;
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use serde::Deserialize;

/// Linhas da tabela do Avançado antes do "mostrar mais", e ferramentas no ranking.
const TOP_ROWS: usize = 20;
const TOP_TOOLS: usize = 12;

/// Um corte do relatório de uso (skill, tool, comando, MCP, agente, contexto, plugin, imagem, dia, conta…).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Item {
    key: String,
    label: Option<String>,
    plugin: String,
    sessions: f64,
    chamadas: f64,
    pedidas: f64,
    ctx_tokens_est: f64,
    ctx_chars: f64,
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
    cost: f64,
    cost_input: f64,
    cost_output: f64,
    cost_cache_write: f64,
    cost_cache_read: f64,
    ocupados_tokens_est: f64,
    ocupados_eq_tokens_est: f64,
    respostas: f64,
}

impl Item {
    fn no_cache(&self) -> f64 { self.input + self.output }
    fn ctx_per_call(&self) -> f64 { if self.chamadas > 0. { self.ctx_tokens_est / self.chamadas } else { 0. } }
    fn per_session(&self) -> f64 { if self.sessions > 0. { self.ctx_tokens_est / self.sessions } else { 0. } }
    fn name(&self) -> &str { self.label.as_deref().unwrap_or(&self.key) }
    fn add(&mut self, b: &Item) {
        self.chamadas += b.chamadas; self.sessions += b.sessions; self.input += b.input; self.output += b.output;
        self.cache_write += b.cache_write; self.cache_read += b.cache_read; self.cost += b.cost; self.cost_input += b.cost_input;
        self.cost_output += b.cost_output; self.cost_cache_write += b.cost_cache_write; self.cost_cache_read += b.cost_cache_read;
    }
    /// Soma de outra máquina: todos os números, inclusive os de contexto e presença.
    fn absorb(&mut self, b: &Item) {
        self.add(b);
        self.pedidas += b.pedidas; self.ctx_tokens_est += b.ctx_tokens_est; self.ctx_chars += b.ctx_chars;
        self.ocupados_tokens_est += b.ocupados_tokens_est; self.ocupados_eq_tokens_est += b.ocupados_eq_tokens_est; self.respostas += b.respostas;
    }
}

/// Junta pela chave; rótulo e plugin ficam com a primeira máquina que souber dizer.
fn join(dest: &mut Vec<Item>, list: &[Item]) {
    let mut at: HashMap<String, usize> = dest.iter().enumerate().map(|(i, b)| (b.key.clone(), i)).collect();
    for b in list {
        match at.get(&b.key) {
            Some(&i) => {
                let target = &mut dest[i];
                if target.label.is_none() { target.label = b.label.clone(); }
                if target.plugin.is_empty() { target.plugin = b.plugin.clone(); }
                target.absorb(b);
            }
            None => { at.insert(b.key.clone(), dest.len()); dest.push(b.clone()); }
        }
    }
}

/// O `mergeUso` do web: cada lista somada pela chave, só das máquinas que responderam no período pedido.
fn merge_usage(parts: &[MachinePart<Report>]) -> Report {
    let mut out = Report::default();
    for p in parts {
        let r = match &p.part {
            Part::Ok(r) => r,
            Part::Mismatched(rate) => { out.usd_brl = out.usd_brl.or(*rate); continue }
            Part::Failed(_) => continue,
        };
        out.usd_brl = out.usd_brl.or(r.usd_brl);
        out.totals.absorb(&r.totals);
        for (dest, list) in [(&mut out.by_skill, &r.by_skill), (&mut out.by_tool, &r.by_tool), (&mut out.by_bash, &r.by_bash),
            (&mut out.by_mcp, &r.by_mcp), (&mut out.by_agente, &r.by_agente), (&mut out.by_contexto, &r.by_contexto),
            (&mut out.by_plugin, &r.by_plugin), (&mut out.by_imagem, &r.by_imagem), (&mut out.by_conta, &r.by_conta),
            (&mut out.by_projeto, &r.by_projeto), (&mut out.by_modelo, &r.by_modelo), (&mut out.by_day, &r.by_day)] { join(dest, list); }
    }
    out.sorted()
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Report {
    totals: Item,
    by_skill: Vec<Item>,
    by_tool: Vec<Item>,
    by_bash: Vec<Item>,
    by_mcp: Vec<Item>,
    by_agente: Vec<Item>,
    by_contexto: Vec<Item>,
    by_plugin: Vec<Item>,
    by_imagem: Vec<Item>,
    by_conta: Vec<Item>,
    by_projeto: Vec<Item>,
    by_modelo: Vec<Item>,
    by_day: Vec<Item>,
    usd_brl: Option<f64>,
}

impl Report {
    /// A ordem do `mergeUso` do web: custo, contexto, chamadas e nome; os seletores por custo e chamadas; dias em ordem.
    fn sorted(mut self) -> Self {
        let by_use = |a: &Item, b: &Item| b.cost.total_cmp(&a.cost).then(b.ctx_chars.total_cmp(&a.ctx_chars))
            .then(b.chamadas.total_cmp(&a.chamadas)).then(a.key.cmp(&b.key));
        let by_pick = |a: &Item, b: &Item| b.cost.total_cmp(&a.cost).then(b.chamadas.total_cmp(&a.chamadas)).then(a.key.cmp(&b.key));
        for list in [&mut self.by_skill, &mut self.by_tool, &mut self.by_bash, &mut self.by_mcp, &mut self.by_agente, &mut self.by_contexto,
            &mut self.by_imagem, &mut self.by_plugin] { list.sort_by(by_use); }
        for list in [&mut self.by_conta, &mut self.by_projeto, &mut self.by_modelo] { list.sort_by(by_pick); }
        self.by_day.sort_by(|a, b| a.key.cmp(&b.key));
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Dim { Account, Project, Model, Plugin }

impl Dim {
    const ALL: [Dim; 4] = [Dim::Account, Dim::Project, Dim::Model, Dim::Plugin];
    fn query(self) -> &'static str { match self { Dim::Account => "conta", Dim::Project => "projeto", Dim::Model => "modelo", Dim::Plugin => "plugin" } }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
enum Tab { #[default] Skill, Agent, Plugin, Tool, Bash, Mcp, Context, Image }

/// A medida principal de cada aba, sempre em tokens: presença acumulada, tokens reais do subagente ou contexto injetado.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Measure { Held, Tokens, Ctx }

impl Tab {
    const ALL: [Tab; 8] = [Tab::Skill, Tab::Agent, Tab::Plugin, Tab::Tool, Tab::Bash, Tab::Mcp, Tab::Context, Tab::Image];
    fn label(self) -> String {
        web(match self { Tab::Skill => "uso_aba_skills", Tab::Agent => "uso_aba_agentes", Tab::Plugin => "uso_aba_plugins", Tab::Tool => "uso_aba_tools",
            Tab::Bash => "uso_aba_bash", Tab::Mcp => "uso_aba_mcp", Tab::Context => "uso_aba_contexto", Tab::Image => "uso_aba_imagens" })
    }
    fn measure(self) -> Measure {
        match self { Tab::Skill | Tab::Plugin => Measure::Held, Tab::Agent => Measure::Tokens, _ => Measure::Ctx }
    }
    fn list(self, r: &Report) -> &[Item] {
        match self { Tab::Skill => &r.by_skill, Tab::Agent => &r.by_agente, Tab::Plugin => &r.by_plugin, Tab::Tool => &r.by_tool,
            Tab::Bash => &r.by_bash, Tab::Mcp => &r.by_mcp, Tab::Context => &r.by_contexto, Tab::Image => &r.by_imagem }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Col { Name, Calls, Main, PerCall, Replies, Sessions }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum GroupOrder { #[default] Weight, Times }

fn main_of(b: &Item, m: Measure) -> f64 {
    match m { Measure::Held => b.ocupados_eq_tokens_est, Measure::Tokens => b.no_cache(), Measure::Ctx => b.ctx_tokens_est }
}

/// Por chamada: a skill mostra o tamanho de cada carga; o contexto, a média por sessão.
fn per_call(b: &Item, tab: Tab) -> f64 {
    let m = tab.measure();
    if tab == Tab::Context { b.per_session() } else if m == Measure::Held { b.ctx_per_call() }
    else if b.chamadas > 0. { main_of(b, m) / b.chamadas } else { 0. }
}

fn measure_label(m: Measure) -> String {
    web(match m { Measure::Held => "uso_col_ocupados_eq", Measure::Tokens => "uso_col_tokens_sem_cache", Measure::Ctx => "uso_col_ctx" })
}

/// Grupo que começa com `@` não é plugin: é a pasta onde o servidor achou a skill.
fn group_name(plugin: &str) -> String {
    match plugin { "@repo" => web("uso_grupo_repo"), "@pessoal" => web("uso_grupo_pessoal"), "@avulsa" => web("uso_grupo_avulsa"),
        "@embutida" => web("uso_grupo_embutida"), "@nativo" => web("uso_grupo_nativo"), "" => web("uso_grupo_sem"), other => other.to_owned() }
}

fn row_label(tab: Tab, b: &Item) -> String {
    match tab {
        Tab::Image if b.key == "enviada" => web("uso_img_enviada"),
        Tab::Image => web_with("uso_img_lida", &[("tool", b.key.trim_start_matches("lida:").to_owned())]),
        Tab::Plugin => group_name(&b.key),
        _ => b.name().to_owned(),
    }
}

/// Um item do grupo: o mesmo nome vindo do Claude e do Codex junta; `keys` guarda as chaves, a mais pesada primeiro.
#[derive(Clone, Debug)]
struct GroupItem { name: String, keys: Vec<String>, weight: f64, times: f64 }

#[derive(Clone, Debug)]
struct Group { plugin: String, weight: f64, times: f64, items: Vec<GroupItem> }

/// O `agruparPorPlugin` do web: grupo pelo `plugin` do servidor, senão pelo prefixo da chave, senão `none`.
fn group_by_plugin(list: &[Item], weight: impl Fn(&Item) -> f64, none: &str) -> Vec<Group> {
    let mut sorted: Vec<&Item> = list.iter().collect();
    sorted.sort_by(|x, y| weight(y).total_cmp(&weight(x)).then(y.chamadas.total_cmp(&x.chamadas)));
    let mut groups: Vec<Group> = Vec::new();
    for b in sorted {
        let colon = b.key.find(':').filter(|i| *i > 0);
        let plugin = if !b.plugin.is_empty() { b.plugin.clone() } else { colon.map_or_else(|| none.to_owned(), |i| b.key[..i].to_owned()) };
        let name = colon.map_or_else(|| b.key.clone(), |i| b.key[i + 1..].to_owned());
        let at = groups.iter().position(|g| g.plugin == plugin).unwrap_or_else(|| {
            groups.push(Group { plugin: plugin.clone(), weight: 0., times: 0., items: Vec::new() });
            groups.len() - 1
        });
        let g = &mut groups[at];
        let i = g.items.iter().position(|x| x.name == name).unwrap_or_else(|| {
            g.items.push(GroupItem { name: name.clone(), keys: Vec::new(), weight: 0., times: 0. });
            g.items.len() - 1
        });
        let w = weight(b);
        let item = &mut g.items[i];
        item.keys.push(b.key.clone());
        (item.weight, item.times) = (item.weight + w, item.times + b.chamadas);
        (g.weight, g.times) = (g.weight + w, g.times + b.chamadas);
    }
    groups.retain(|g| g.weight > 0. || g.times > 0.);
    for g in &mut groups { g.items.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(b.times.total_cmp(&a.times)).then(a.name.cmp(&b.name))); }
    groups.sort_by(|a, b| b.weight.total_cmp(&a.weight).then(b.times.total_cmp(&a.times)).then(a.plugin.cmp(&b.plugin)));
    groups
}

/// Categoria do contexto injetado, pela chave (`categoria` do web).
fn context_category(key: &str) -> usize {
    if key.starts_with("hook") { 2 }
    else if matches!(key, "instructions" | "nested_memory" | "session_context") { 0 }
    else if ["skill_listing", "agent_listing", "deferred_tools", "command_permissions"].iter().any(|p| key.starts_with(p)) { 1 }
    else if ["reminder", "output_style", "queued_command", "silent_turn"].iter().any(|p| key.contains(p)) { 3 }
    else { 4 }
}

fn short_day(key: &str) -> String { key.get(5..).unwrap_or(key).replacen('-', "/", 1) }

#[derive(Default)]
pub(super) struct UsageStats {
    period: Period,
    /// A soma das máquinas que já responderam; `loading` enquanto falta alguma.
    report: Remote<Report>,
    parts: Vec<MachinePart<Report>>,
    pending: usize,
    warming: Vec<Warming>,
    partial: Partial,
    filters: HashMap<Dim, Vec<String>>,
    /// Opções dos seletores: ficam as últimas que vieram, para um recorte vazio não esvaziar o seletor.
    lists: HashMap<Dim, Vec<Item>>,
    search: Option<Entity<InputState>>,
    _search_sub: Option<Subscription>,
    tab: Tab,
    order: HashMap<Tab, (Col, bool)>,
    expanded: bool,
    selected: Option<(Tab, String)>,
    series: Remote<Vec<Item>>,
    series_parts: Vec<MachinePart<Vec<Item>>>,
    series_pending: usize,
    series_partial: Partial,
    group_order: GroupOrder,
    /// Grupos de skills abertos; `None` = só o primeiro, até o primeiro clique.
    open_groups: Option<HashSet<String>>,
    advanced: bool,
    hover_day: Option<String>,
    hover_ctx: Option<usize>,
    pub(super) scroll: ScrollHandle,
}

impl UsageStats {
    fn query(&self, fresh: bool, focus: Option<&str>) -> Vec<(String, String)> {
        let mut q = vec![("period".to_owned(), self.period.key().to_owned())];
        for dim in Dim::ALL {
            q.extend(self.filters.get(&dim).into_iter().flatten().map(|v| (dim.query().to_owned(), v.clone())));
        }
        if fresh { q.push(("fresco".into(), "true".into())); }
        if let Some(f) = focus { q.push(("foco".into(), f.to_owned())); }
        q
    }
    fn filtering(&self) -> bool { self.filters.values().any(|v| !v.is_empty()) }
    fn search_text(&self, cx: &App) -> String {
        self.search.as_ref().map(|s| s.read(cx).value().trim().to_lowercase()).unwrap_or_default()
    }
}

impl Hangar {
    /// A página abriu (ou voltou a abrir): pede o relatório se ainda não há um.
    pub(super) fn usage_stats_opened(&mut self, cx: &mut Context<Self>) {
        let s = &self.usage_stats;
        if s.report.value.is_none() && !s.report.loading { self.load_usage(false, cx); }
    }

    /// A escolha de máquinas mudou com a página fechada: a próxima abertura relê.
    pub(super) fn usage_stale(&mut self) { self.usage_stats.report.reset(); }

    pub(super) fn load_usage(&mut self, fresh: bool, cx: &mut Context<Self>) {
        self.ensure_costs_prefs();
        let seq = self.usage_stats.report.start();
        let machines = self.chosen_machines();
        let s = &mut self.usage_stats;
        (s.parts, s.pending, s.warming, s.partial) = (Vec::new(), machines.len(), Vec::new(), Partial::default());
        if machines.is_empty() { s.report.finish(seq, Err(tr("connection_failed"))); }
        let query = s.query(fresh, None);
        let read = MachineRead { seq, alive: |this, seq| this.usage_stats.report.seq == seq,
            warm: |this, m, progress| set_warming(&mut this.usage_stats.warming, m, progress), done: Self::usage_part };
        for m in machines { self.read_machine(m, "uso", query.clone(), 0, read, cx); }
        self.load_usage_series(cx);
        cx.notify();
    }

    /// Uma máquina respondeu: a tela passa a mostrar a soma de quem já respondeu.
    fn usage_part(&mut self, seq: u64, m: &Machine, result: Result<Value, String>, cx: &mut Context<Self>) {
        let s = &mut self.usage_stats;
        if seq != s.report.seq { return; }
        let period = s.period.key();
        let parsed = result.and_then(|v| {
            let in_period = v.pointer("/applied/period").and_then(Value::as_str) == Some(period);
            serde_json::from_value::<Report>(v).map(|r| (in_period, r)).map_err(|_| tr("invalid_response"))
        });
        let part = match parsed { Ok((true, r)) => Part::Ok(r), Ok((false, r)) => Part::Mismatched(r.usd_brl), Err(error) => Part::Failed(error) };
        s.parts.push(MachinePart { id: m.id.clone(), label: m.label.clone(), part });
        s.pending = s.pending.saturating_sub(1);
        s.partial = Partial::of(&s.parts);
        let done = s.pending == 0;
        if done || s.parts.iter().any(|p| !matches!(p.part, Part::Failed(_))) {
            let value = all_failed(&s.parts).map_or_else(|| Ok(merge_usage(&s.parts)), Err);
            // Os seletores guardam as últimas opções: um recorte vazio não esvazia a escolha.
            if let Ok(r) = &value {
                for (dim, list) in [(Dim::Account, &r.by_conta), (Dim::Project, &r.by_projeto), (Dim::Model, &r.by_modelo)] {
                    if !list.is_empty() { s.lists.insert(dim, list.clone()); }
                }
                let plugin_filtered = s.filters.get(&Dim::Plugin).is_some_and(|v| !v.is_empty());
                if !plugin_filtered && !r.by_plugin.is_empty() { s.lists.insert(Dim::Plugin, r.by_plugin.clone()); }
            }
            s.report.value = Some(value);
        }
        s.report.loading = !done;
        cx.notify();
    }

    /// Série diária do item escolhido: consulta própria com `foco`, para o clique não refazer a tela.
    fn load_usage_series(&mut self, cx: &mut Context<Self>) {
        // `reset`, nunca `default`: o número do pedido não pode voltar, senão a resposta da série anterior entra nesta.
        let Some((_, key)) = self.usage_stats.selected.clone() else { self.usage_stats.series.reset(); return };
        let seq = self.usage_stats.series.start();
        let machines = self.chosen_machines();
        let s = &mut self.usage_stats;
        (s.series_parts, s.series_pending, s.series_partial) = (Vec::new(), machines.len(), Partial::default());
        if machines.is_empty() { s.series.finish(seq, Err(tr("connection_failed"))); }
        let query = s.query(false, Some(&key));
        let read = MachineRead { seq, alive: |this, seq| this.usage_stats.series.seq == seq, warm: |_, _, _| {}, done: Self::series_part };
        // Sem repetir enquanto a máquina aquece: a série é um detalhe, a máquina entra como "não respondeu".
        for m in machines { self.read_machine(m, "uso", query.clone(), WARM_TRIES, read, cx); }
    }

    fn series_part(&mut self, seq: u64, m: &Machine, result: Result<Value, String>, cx: &mut Context<Self>) {
        let s = &mut self.usage_stats;
        if seq != s.series.seq { return; }
        let period = s.period.key();
        let part = match result {
            Ok(v) if v.pointer("/applied/period").and_then(Value::as_str) != Some(period) => Part::Mismatched(None),
            Ok(v) => match serde_json::from_value::<Vec<Item>>(v.get("by_day").cloned().unwrap_or(Value::Array(Vec::new()))) {
                Ok(days) => Part::Ok(days),
                Err(_) => Part::Failed(tr("invalid_response")),
            },
            Err(error) => Part::Failed(error),
        };
        s.series_parts.push(MachinePart { id: m.id.clone(), label: m.label.clone(), part });
        s.series_pending = s.series_pending.saturating_sub(1);
        if s.series_pending > 0 { return; }
        s.series_partial = Partial::of(&s.series_parts);
        let value = all_failed(&s.series_parts).map_or_else(|| {
            let mut days = Vec::new();
            for p in &s.series_parts { if let Part::Ok(list) = &p.part { join(&mut days, list); } }
            days.sort_by(|a, b| a.key.cmp(&b.key));
            Ok(days)
        }, Err);
        s.series.finish(seq, value);
        cx.notify();
    }

    fn usage_select(&mut self, tab: Tab, key: String, cx: &mut Context<Self>) {
        let s = &mut self.usage_stats;
        s.selected = if s.selected.as_ref() == Some(&(tab, key.clone())) { None } else { Some((tab, key)) };
        self.load_usage_series(cx);
        cx.notify();
    }

    /// Clique num item fora da tabela (skill, ferramenta, agente): abre a aba dele e o detalhe.
    fn usage_open(&mut self, tab: Tab, key: String, cx: &mut Context<Self>) {
        if self.usage_stats.tab != tab { self.usage_stats.tab = tab; self.usage_stats.expanded = false; }
        self.usage_select(tab, key, cx);
    }

    fn usage_set_filter(&mut self, dim: Dim, values: Vec<String>, cx: &mut Context<Self>) {
        if values.is_empty() { self.usage_stats.filters.remove(&dim); } else { self.usage_stats.filters.insert(dim, values); }
        self.load_usage(false, cx);
    }

    fn usage_toggle_filter(&mut self, dim: Dim, key: String, cx: &mut Context<Self>) {
        let mut values = self.usage_stats.filters.get(&dim).cloned().unwrap_or_default();
        if let Some(at) = values.iter().position(|k| *k == key) { values.remove(at); } else { values.push(key); }
        self.usage_set_filter(dim, values, cx);
    }

    fn usage_filter_name(&self, dim: Dim, key: &str) -> String {
        match dim {
            Dim::Account => self.usage_stats.lists.get(&dim).and_then(|l| l.iter().find(|b| b.key == key)).map_or(key, |b| b.name()).to_owned(),
            Dim::Project => project_label(key),
            Dim::Model => key.to_owned(),
            Dim::Plugin => group_name(key),
        }
    }

    pub(super) fn render_usage_stats(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if self.usage_stats.search.is_none() {
            let input = cx.new(|cx| InputState::new(window, cx).placeholder(web("uso_busca")));
            self.usage_stats._search_sub = Some(cx.subscribe(&input, |_, _, event: &InputEvent, cx| if matches!(event, InputEvent::Change) { cx.notify() }));
            self.usage_stats.search = Some(input);
        }
        let loading = self.usage_stats.report.loading;
        let header = self.costs_header(web("nav_uso"), |this, window, cx| this.show_costs_view(View::Costs, window, cx), vec![
            Button::new("usage-costs").ghost().small().label(web("uso_ir_custos")).icon(IconName::ArrowLeft)
                .on_click(cx.listener(|this, _, window, cx| this.show_costs_view(View::Costs, window, cx))).into_any_element(),
            Button::new("usage-refresh").outline().small().label(web("custos_atualizar")).loading(loading).disabled(loading)
                .on_click(cx.listener(|this, _, _, cx| this.load_usage(true, cx))).into_any_element(),
        ], cx);
        let body = self.render_usage_body(cx);
        let scroll = div().id("usage-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.usage_stats.scroll)
            .child(div().w_full().flex().justify_center()
                .child(div().w_full().max_w(px(1120.)).px(px(24.)).pt(px(20.)).pb(px(48.)).flex().flex_col().gap(px(16.)).child(body)));
        page_frame(div().size_full().flex().flex_col().child(header).child(scroll))
    }

    fn render_usage_body(&mut self, cx: &mut Context<Self>) -> Div {
        let s = &self.usage_stats;
        let title = div().flex().items_center().gap(px(12.)).flex_wrap()
            .child(div().flex_1().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(web("uso_painel_titulo")))
            .child(segments("usage-period", &Period::labels(), s.period.index(), 5, false, String::new(),
                |this: &mut Hangar, n, _: &mut Window, cx| {
                    if this.usage_stats.period != Period::ALL[n] { this.usage_stats.period = Period::ALL[n]; this.load_usage(false, cx); }
                }, cx));
        let mut page = div().flex().flex_col().gap(px(16.)).child(title).child(self.render_usage_filters(cx)).children(self.machine_chips(cx));

        let s = &self.usage_stats;
        if s.report.loading && s.report.value.is_some() {
            page = page.child(div().text_size(px(12.5)).text_color(theme::muted()).child(web("uso_atualizando")));
        }
        page = page.children(s.warming.iter().map(warming_note));
        if !s.partial.is_empty() && s.report.ok().is_some() {
            page = page.child(partial_note("usage-partial-retry", &s.partial,cx.listener(|this, _, _, cx| this.load_usage(true, cx))));
        }
        let report = match &self.usage_stats.report.value {
            None => return page.child(loading_state()),
            Some(Err(error)) => return page.child(error_state(error.clone(), cx.listener(|this, _, _, cx| this.load_usage(true, cx)))),
            Some(Ok(r)) => r.clone(),
        };
        if !self.usage_stats.report.loading && report.totals.chamadas == 0. && report.totals.ctx_chars == 0. {
            return page.child(empty_state(web("uso_vazio")));
        }
        let rate = report.usd_brl;
        let skills = group_by_plugin(&report.by_skill, |b| b.ocupados_eq_tokens_est, "");
        page = page.child(self.render_usage_answers(&report, &skills, rate));
        let main = div().flex_1().min_w_0().flex().flex_col().gap(px(16.))
            .child(self.render_usage_skills(&skills, cx))
            // A tabela de agentes tem oito colunas: em meia largura o nome some; as duas seções ficam uma embaixo da outra.
            .child(self.render_usage_tools(&report, cx))
            .child(self.render_usage_agents(&report, rate, cx))
            .child(self.render_usage_advanced(&report, cx));
        let detail = self.usage_stats.selected.clone()
            .and_then(|(tab, key)| tab.list(&report).iter().find(|b| b.key == key).cloned().map(|b| (tab, b)))
            .map(|(tab, b)| self.render_usage_detail(tab, &b, rate, cx));
        page.child(div().flex().gap(px(16.)).items_start().child(main)
            .children(detail.map(|d| div().w(px(340.)).flex_shrink_0().child(d))))
    }

    fn render_usage_filters(&self, cx: &mut Context<Self>) -> Div {
        let s = &self.usage_stats;
        let mut row = div().flex().flex_wrap().items_center().gap(px(10.));
        for dim in Dim::ALL {
            let options = s.lists.get(&dim).cloned().unwrap_or_default();
            let selected = s.filters.get(&dim).cloned().unwrap_or_default();
            let all = web(if dim == Dim::Account { "uso_todas" } else { "uso_todos" });
            let value = match selected.len() {
                0 => all.clone(),
                1 => self.usage_filter_name(dim, &selected[0]),
                n => web_with("uso_n_de_m", &[("n", n.to_string()), ("m", options.len().to_string())]),
            };
            let shown = web_with(match dim { Dim::Account => "uso_filtro_conta", Dim::Project => "uso_filtro_projeto", Dim::Model => "uso_filtro_modelo",
                Dim::Plugin => "uso_filtro_plugin" }, &[("v", value)]);
            let items: Vec<(String, String, String, bool)> = options.iter().map(|b| (b.key.clone(), self.usage_filter_name(dim, &b.key),
                if dim == Dim::Plugin { dec(b.chamadas, 0) } else { tok(b.no_cache()) }, selected.contains(&b.key))).collect();
            let this = cx.entity().downgrade();
            let button = Button::new(SharedString::from(format!("usage-filter-{}", dim.query()))).outline().small().w(px(220.))
                .selected(!selected.is_empty())
                .child(div().w_full().flex().items_center().gap(px(6.))
                    .child(div().flex_1().min_w_0().truncate().text_left().child(shown))
                    .child(chrome::small_icon(IconName::ChevronDown, 14., theme::muted())))
                .accessibility_label(web(match dim { Dim::Account => "uso_conta", Dim::Project => "uso_projeto", Dim::Model => "uso_modelo", Dim::Plugin => "uso_plugin" }))
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let pick = |key: Option<String>| {
                        let this = this.clone();
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            let _ = this.update(cx, |this, cx| match key.clone() {
                                Some(key) => this.usage_toggle_filter(dim, key, cx),
                                None => this.usage_set_filter(dim, Vec::new(), cx),
                            });
                        }
                    };
                    let mut menu = sidebar::menu_style(menu).min_w(px(300.)).max_w(px(460.)).max_h(px(420.)).scrollable(true)
                        .item(PopupMenuItem::new(all.clone()).checked(items.iter().all(|i| !i.3)).on_click(pick(None)))
                        .separator();
                    for (key, name, hint, on) in items.clone() {
                        menu = menu.item(PopupMenuItem::element(move |_, _| div().w_full().flex().items_center().gap(px(12.))
                                .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                                .child(div().flex_shrink_0().text_color(theme::muted()).child(hint.clone())))
                            .checked(on).on_click(pick(Some(key))));
                    }
                    menu
                });
            row = row.child(button);
        }
        if let Some(search) = &s.search {
            row = row.child(div().w(px(220.)).child(Input::new(search).small().aria_label(web("uso_busca"))
                .prefix(chrome::small_icon(IconName::Search, 14., theme::faint()))));
        }
        row = row.children(self.machines_button("usage-machines", cx));
        if s.filtering() || !s.search_text(cx).is_empty() {
            row = row.child(Button::new("usage-clear").ghost().small().label(web("uso_limpar")).on_click(cx.listener(|this, _, window, cx| {
                if let Some(search) = this.usage_stats.search.clone() { search.update(cx, |i, cx| i.set_value("", window, cx)); }
                this.usage_stats.filters.clear();
                this.load_usage(false, cx);
            })));
        }
        row
    }

    fn render_usage_answers(&self, r: &Report, skills: &[Group], rate: Option<f64>) -> Div {
        let total_weight = skills.iter().map(|g| g.weight).sum::<f64>().max(1.);
        let best = |v: f64| v > 0.;
        let plugin_top = skills.iter().filter(|g| !g.plugin.is_empty() && !g.plugin.starts_with('@') && best(g.weight))
            .max_by(|a, b| a.weight.total_cmp(&b.weight));
        let items: Vec<(&GroupItem, &str)> = skills.iter().flat_map(|g| g.items.iter().map(move |i| (i, g.plugin.as_str()))).collect();
        let heaviest = items.iter().filter(|(i, _)| best(i.weight)).max_by(|a, b| a.0.weight.total_cmp(&b.0.weight));
        let most_used = items.iter().filter(|(i, _)| best(i.times)).max_by(|a, b| a.0.times.total_cmp(&b.0.times));
        let agents = agent_groups(r);
        let agent_plugin = agents.iter().filter(|g| !g.0.is_empty() && !g.0.starts_with('@') && best(g.1.cost)).max_by(|a, b| a.1.cost.total_cmp(&b.1.cost));
        let agent_top = agents.iter().flat_map(|g| g.2.iter()).filter(|i| best(i.1.cost)).max_by(|a, b| a.1.cost.total_cmp(&b.1.cost));
        let tools = tools_of(r);
        let tool_calls: f64 = tools.iter().map(|b| b.chamadas).sum();
        let answer = |q: &str, a: Option<(String, String)>| {
            let cell = div().flex_1().min_w_0().p(px(14.)).rounded(px(12.)).border_1().border_color(theme::border()).bg(theme::boxed())
                .flex().flex_col().gap(px(4.)).child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(web(q)));
            match a {
                Some((name, sub)) => cell.child(div().text_size(px(17.)).font_weight(FontWeight::SEMIBOLD).truncate().child(name))
                    .child(div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(sub)),
                None => cell.child(div().text_size(px(17.)).text_color(theme::faint()).child("—")),
            }
        };
        let cells = [
            answer("uso_resp_plugin_skills", plugin_top.map(|g| (group_name(&g.plugin), web_with("uso_resp_plugin_skills_sub",
                &[("n", tok(g.weight)), ("pct", dec(g.weight / total_weight * 100., 0))])))),
            answer("uso_resp_skill_contexto", heaviest.map(|(i, g)| (i.name.clone(), web_with("uso_resp_grupo_contexto", &[("grupo", group_name(g)), ("n", tok(i.weight))])))),
            answer("uso_resp_skill_usada", most_used.map(|(i, g)| (i.name.clone(), web_with("uso_resp_grupo_vezes", &[("grupo", group_name(g)), ("n", dec(i.times, 0))])))),
            answer("uso_resp_plugin_agentes", agent_plugin.map(|g| (group_name(&g.0), web_with("uso_resp_agente_custo_sub",
                &[("custo", money(g.1.cost, rate)), ("n", dec(g.1.chamadas, 0))])))),
            answer("uso_resp_agente_caro", agent_top.map(|i| (i.0.clone(), web_with("uso_resp_agente_custo_sub",
                &[("custo", money(i.1.cost, rate)), ("n", dec(i.1.chamadas, 0))])))),
            answer("uso_resp_ferramenta", tools.first().map(|t| (t.key.clone(), web_with("uso_resp_ferramenta_sub",
                &[("pct", dec(t.chamadas / tool_calls.max(1.) * 100., 0)), ("n", dec(t.chamadas, 0))])))),
        ];
        let mut cells = cells.into_iter();
        div().flex().flex_col().gap(px(12.))
            .child(div().flex().gap(px(12.)).children(cells.by_ref().take(3)))
            .child(div().flex().gap(px(12.)).children(cells))
    }

    fn render_usage_skills(&self, skills: &[Group], cx: &mut Context<Self>) -> Div {
        let s = &self.usage_stats;
        let term = s.search_text(cx);
        let order = s.group_order;
        let key = move |w: f64, t: f64| if order == GroupOrder::Weight { (w, t) } else { (t, w) };
        let mut shown: Vec<Group> = skills.iter().filter_map(|g| {
            let items: Vec<GroupItem> = g.items.iter().filter(|i| term.is_empty() || i.name.to_lowercase().contains(&term)
                || group_name(&g.plugin).to_lowercase().contains(&term)).cloned().collect();
            (!items.is_empty()).then(|| {
                let mut items = items;
                items.sort_by(|a, b| { let (ka, kb) = (key(a.weight, a.times), key(b.weight, b.times)); kb.0.total_cmp(&ka.0).then(kb.1.total_cmp(&ka.1)) });
                Group { weight: items.iter().map(|i| i.weight).sum(), times: items.iter().map(|i| i.times).sum(), plugin: g.plugin.clone(), items }
            })
        }).collect();
        shown.sort_by(|a, b| { let (ka, kb) = (key(a.weight, a.times), key(b.weight, b.times)); kb.0.total_cmp(&ka.0).then(kb.1.total_cmp(&ka.1)) });
        let total_weight = skills.iter().map(|g| g.weight).sum::<f64>().max(1.);
        let max_group = (shown.iter().map(|g| g.weight).fold(1., f64::max), shown.iter().map(|g| g.times).fold(1., f64::max));
        let max_item = (shown.iter().flat_map(|g| g.items.iter().map(|i| i.weight)).fold(1., f64::max),
            shown.iter().flat_map(|g| g.items.iter().map(|i| i.times)).fold(1., f64::max));
        let legend = div().flex().flex_wrap().items_center().gap(px(6.)).text_size(px(12.5)).text_color(theme::muted())
            .child(swatch(chart(1))).child(web("uso_legenda_peso")).child(swatch(chart(2))).child(web("uso_legenda_vezes"));
        let mut body = card(web("uso_sec_por_plugin"), None).child(legend);
        if shown.is_empty() { return body.child(empty_state(web("uso_vazio_secao"))); }
        let sort_button = |id: &'static str, label: String, which: GroupOrder, cx: &mut Context<Self>| {
            let on = order == which;
            Button::new(id).ghost().xsmall().selected(on).label(if on { format!("{label} ▾") } else { label })
                .on_click(cx.listener(move |this, _, _, cx| { this.usage_stats.group_order = which; cx.notify(); }))
        };
        body = body.child(div().flex().items_center().px(px(8.)).pb(px(4.)).border_b_1().border_color(theme::border())
            .text_size(px(12.)).text_color(theme::faint())
            .child(div().flex_1().child(web("uso_col_grupo")))
            .child(div().w(px(260.)).flex().justify_center().child(sort_button("usage-sort-weight", web("uso_col_peso"), GroupOrder::Weight, cx)))
            .child(div().w(px(260.)).flex().justify_center().child(sort_button("usage-sort-times", web("uso_col_vezes"), GroupOrder::Times, cx))));
        let first = shown.first().map(|g| g.plugin.clone());
        let selected = s.selected.clone();
        for g in &shown {
            let open = s.open_groups.as_ref().map_or(first.as_deref() == Some(g.plugin.as_str()), |set| set.contains(&g.plugin));
            let (plugin, first) = (g.plugin.clone(), first.clone());
            let share = dec(g.weight / total_weight * 100., 0);
            let meta = if g.items.len() == 1 { web_with("uso_grupo_1_skill", &[("pct", share)]) }
                else { web_with("uso_grupo_n_skills", &[("n", g.items.len().to_string()), ("pct", share)]) };
            body = body.child(Button::new(SharedString::from(format!("usage-group-{plugin}"))).ghost().w_full().h(px(40.)).px(px(8.))
                .child(div().w_full().flex().items_center()
                    .child(div().flex_1().min_w_0().flex().items_center().gap(px(8.))
                        .child(chrome::small_icon(if open { IconName::ChevronDown } else { IconName::ChevronRight }, 14., theme::muted()))
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(group_name(&g.plugin)))
                        .child(div().truncate().text_size(px(12.)).text_color(theme::muted()).child(meta)))
                    .child(bar_cell(g.weight, max_group.0, true)).child(bar_cell(g.times, max_group.1, false)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let s = &mut this.usage_stats;
                    let set = s.open_groups.get_or_insert_with(|| first.clone().into_iter().collect());
                    if !set.remove(&plugin) { set.insert(plugin.clone()); }
                    cx.notify();
                })));
            if !open { continue; }
            for i in &g.items {
                let key = i.keys[0].clone();
                let on = selected.as_ref().is_some_and(|(t, k)| *t == Tab::Skill && i.keys.contains(k));
                body = body.child(Button::new(SharedString::from(format!("usage-skill-{}-{}", g.plugin, i.name))).ghost().selected(on)
                    .w_full().h(px(36.)).pl(px(30.)).pr(px(8.)).tooltip(i.keys.join(", "))
                    .child(div().w_full().flex().items_center()
                        .child(div().flex_1().min_w_0().truncate().text_left().child(i.name.clone()))
                        .child(bar_cell(i.weight, max_item.0, true)).child(bar_cell(i.times, max_item.1, false)))
                    .on_click(cx.listener(move |this, _, _, cx| this.usage_open(Tab::Skill, key.clone(), cx))));
            }
        }
        body.child(hint_text(web("uso_grupo_nota")))
    }

    fn render_usage_tools(&self, r: &Report, cx: &mut Context<Self>) -> Div {
        let tools = tools_of(r);
        let body = card(web("uso_rank_ferramentas"), Some(web("uso_ferr_nota")));
        if tools.is_empty() { return body.child(empty_state(web("uso_vazio_secao"))); }
        let total: f64 = tools.iter().map(|b| b.chamadas).sum();
        let max = tools[0].chamadas.max(1.);
        let mut bash: Vec<&Item> = r.by_bash.iter().collect();
        bash.sort_by(|a, b| b.chamadas.total_cmp(&a.chamadas));
        let commands = bash.iter().take(5).map(|b| format!("{} {}", b.key, dec(b.chamadas, 0))).collect::<Vec<_>>().join(", ");
        let rows = tools.iter().take(TOP_TOOLS).map(|t| {
            let key = t.key.clone();
            div().flex().flex_col().gap(px(4.))
                .child(Button::new(SharedString::from(format!("usage-tool-{key}"))).ghost().w_full().h(px(28.)).px(px(4.))
                    .child(div().w_full().flex().items_center().gap(px(8.))
                        .child(div().flex_1().min_w_0().truncate().text_left().font_weight(FontWeight::SEMIBOLD).child(t.key.clone()))
                        .child(div().child(dec(t.chamadas, 0)))
                        .child(div().text_color(theme::faint()).child(format!("· {}%", dec(t.chamadas / total.max(1.) * 100., 0)))))
                    .on_click(cx.listener(move |this, _, _, cx| this.usage_open(Tab::Tool, key.clone(), cx))))
                .child(div().mx(px(4.)).h(px(5.)).rounded_full().bg(theme::inset())
                    .child(div().h_full().rounded_full().bg(chart(2)).w(relative((t.chamadas / max) as f32))))
                .when(t.key == "Bash" && !commands.is_empty(), |el| el.child(div().px(px(4.)).text_size(px(12.)).text_color(theme::muted())
                    .whitespace_normal().child(web_with("uso_ferr_bash", &[("lista", commands.clone())]))))
        }).collect::<Vec<_>>();
        body.child(div().flex().flex_col().gap(px(8.)).children(rows))
            .when(tools.len() > TOP_TOOLS, |el| el.child(hint_text(web_with("uso_mais_na_tabela", &[("n", (tools.len() - TOP_TOOLS).to_string())]))))
    }

    fn render_usage_agents(&self, r: &Report, rate: Option<f64>, cx: &mut Context<Self>) -> Div {
        let groups = agent_groups(r);
        let body = card(web("uso_sub_titulo"), Some(web("uso_sub_nota")));
        if groups.is_empty() { return body.child(empty_state(web("uso_vazio_secao"))); }
        let pair = |t: f64, c: f64| div().flex().flex_col().items_end().child(tok(t)).child(div().text_size(px(11.)).text_color(theme::faint()).child(money2(c, rate)));
        let cells = |b: &Item, sessions: Option<f64>| vec![
            div().child(money2(b.cost, rate)), div().child(dec(b.chamadas, 0)), pair(b.input, b.cost_input), pair(b.output, b.cost_output),
            pair(b.cache_write, b.cost_cache_write), pair(b.cache_read, b.cost_cache_read), div().child(sessions.map_or("—".into(), |n| dec(n, 0))),
        ];
        const W: [f32; 7] = [84., 60., 74., 74., 74., 74., 52.];
        let line = |first: Div, cells: Vec<Div>| div().w_full().flex().items_center().gap(px(4.)).text_size(px(12.5))
            .child(first.flex_1().min_w(px(140.)))
            .children(cells.into_iter().zip(W).map(|(c, w)| div().w(px(w)).flex_shrink_0().flex().justify_end().child(c)));
        let head = [web("uso_col_custo_real"), web("uso_col_chamadas"), web("custos_input_sem_cache"), web("custos_output_total"),
            web("custos_tipo_cache_escrito"), web("custos_tipo_cache_lido"), web("uso_col_sessoes")];
        let selected = self.usage_stats.selected.clone();
        let mut table = div().flex().flex_col()
            .child(line(div().child(web("uso_col_plugin_agente")), head.into_iter().map(|h| div().text_right().whitespace_normal().child(h)).collect())
                .px(px(6.)).pb(px(6.)).border_b_1().border_color(theme::border()).text_size(px(11.)).text_color(theme::faint()));
        for (plugin, total, items) in &groups {
            table = table.child(line(div().flex().items_center().gap(px(6.)).child(div().truncate().font_weight(FontWeight::SEMIBOLD).child(group_name(plugin)))
                    .child(div().text_color(theme::faint()).child(web_with("uso_agentes_n", &[("n", items.len().to_string())]))), cells(total, None))
                .px(px(6.)).py(px(6.)).border_t_1().border_color(theme::border()).bg(theme::inset()));
            for (name, b, keys) in items {
                let key = keys[0].clone();
                let on = selected.as_ref().is_some_and(|(t, k)| *t == Tab::Agent && keys.contains(k));
                table = table.child(Button::new(SharedString::from(format!("usage-agent-{plugin}-{name}"))).ghost().selected(on).w_full().h_auto().px(px(6.)).py(px(4.))
                    .child(line(div().truncate().text_left().child(name.clone()), cells(b, Some(b.sessions))))
                    .on_click(cx.listener(move |this, _, _, cx| this.usage_open(Tab::Agent, key.clone(), cx))));
            }
        }
        body.child(table).child(hint_text(web("uso_cache_legado_nota")))
    }

    fn render_usage_advanced(&self, r: &Report, cx: &mut Context<Self>) -> Div {
        let open = self.usage_stats.advanced;
        let toggle = Button::new("usage-advanced").ghost().small().icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
            .label(web("uso_avancado"))
            .on_click(cx.listener(|this, _, _, cx| { this.usage_stats.advanced = !this.usage_stats.advanced; cx.notify(); }));
        let mut body = div().flex().flex_col().gap(px(16.)).child(div().flex().child(toggle));
        if !open { return body; }
        body = body.child(self.render_usage_daily(r, cx)).child(self.render_usage_context(r, cx)).child(self.render_usage_table(r, cx));
        body
    }

    fn render_usage_daily(&self, r: &Report, cx: &mut Context<Self>) -> Div {
        let body = card(web("uso_graf_chamadas_dia"), Some(web("uso_graf_chamadas_dia_nota")));
        if r.by_day.is_empty() { return body.child(empty_state(web("uso_sem_dias"))); }
        let hover = self.usage_stats.hover_day.clone();
        let bars = day_bars("usage-day", &r.by_day, |b| b.chamadas, 180., hover.as_deref(), Some(cx));
        let caption = hover.as_ref().and_then(|h| r.by_day.iter().find(|b| &b.key == h))
            .map_or_else(String::new, |b| format!("{} {} · {}", dec(b.chamadas, 0), web("uso_graf_chamadas"), b.key));
        body.child(div().id("usage-daily").on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.usage_stats.hover_day.take().is_some() { cx.notify() }
            })).child(bars))
            .child(div().h(px(16.)).text_size(px(12.5)).text_color(theme::muted()).child(caption))
    }

    fn render_usage_context(&self, r: &Report, cx: &mut Context<Self>) -> Div {
        const CATS: [&str; 5] = ["uso_ctx_cat_instrucoes", "uso_ctx_cat_catalogo", "uso_ctx_cat_hooks", "uso_ctx_cat_lembretes", "uso_ctx_cat_outros"];
        let color = |i: usize| if i < 4 { chart(i + 1) } else { theme::muted() };
        let mut sums = [0f64; 5];
        for b in &r.by_contexto { sums[context_category(&b.key)] += b.ctx_tokens_est; }
        let sessions = r.totals.sessions.max(1.);
        let total = sums.iter().sum::<f64>().max(1.);
        let per_session_total = if r.totals.sessions > 0. { r.by_contexto.iter().map(|b| b.ctx_tokens_est).sum::<f64>() / r.totals.sessions } else { 0. };
        let hover = self.usage_stats.hover_ctx;
        let dim = move |i: usize| hover.is_some_and(|h| h != i);
        let head = div().flex().items_center()
            .child(div().flex_1().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(web("uso_graf_ctx")))
            .child(div().flex().gap(px(6.)).child(format!("≈ {}", tok(per_session_total))).child(div().text_color(theme::muted()).child(web("uso_por_sessao"))));
        let stack = div().h(px(22.)).w_full().flex().gap(px(2.)).rounded(px(6.)).overflow_hidden()
            .children((0..5).filter(|i| sums[*i] > 0.).map(|i| div().h_full().bg(color(i)).flex_basis(px(0.)).flex_grow(sums[i] as f32).when(dim(i), |el| el.opacity(0.35))));
        let legend = div().flex().flex_col().gap(px(4.)).children((0..5).map(|i| {
            div().id(SharedString::from(format!("usage-ctx-{i}"))).flex().items_center().gap(px(8.)).text_size(px(13.)).when(dim(i), |el| el.opacity(0.45))
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| { this.usage_stats.hover_ctx = hovered.then_some(i); cx.notify(); }))
                .child(swatch(color(i))).child(div().flex_1().child(web(CATS[i])))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(format!("≈ {}", tok(sums[i] / sessions))))
                .child(div().w(px(40.)).flex().justify_end().text_color(theme::muted()).child(format!("{}%", dec(sums[i] / total * 100., 0))))
        }));
        card_plain().child(head).child(hint_text(web("uso_graf_ctx_nota"))).child(stack).child(legend)
    }

    fn render_usage_table(&self, r: &Report, cx: &mut Context<Self>) -> Div {
        let s = &self.usage_stats;
        let tab = s.tab;
        let m = tab.measure();
        let tabs = div().flex().flex_wrap().gap(px(4.)).children(Tab::ALL.map(|t| {
            Button::new(SharedString::from(format!("usage-tab-{t:?}"))).ghost().small().selected(t == tab)
                .child(div().flex().gap(px(6.)).child(t.label()).child(div().text_color(theme::faint()).child(t.list(r).len().to_string())))
                .on_click(cx.listener(move |this, _, _, cx| { this.usage_stats.tab = t; this.usage_stats.expanded = false; cx.notify(); }))
        }));
        let mut body = card_plain().child(tabs);
        let term = s.search_text(cx);
        let (col, desc) = s.order.get(&tab).copied().unwrap_or((Col::Main, true));
        let value = |b: &Item, c: Col| match c { Col::Calls => b.chamadas, Col::Main => main_of(b, m), Col::PerCall => per_call(b, tab),
            Col::Replies => b.respostas, Col::Sessions => b.sessions, Col::Name => 0. };
        let mut rows: Vec<&Item> = tab.list(r).iter().filter(|b| term.is_empty() || row_label(tab, b).to_lowercase().contains(&term)
            || b.plugin.to_lowercase().contains(&term)).collect();
        rows.sort_by(|a, b| {
            let o = if col == Col::Name { a.name().cmp(b.name()) } else { value(a, col).total_cmp(&value(b, col)) };
            (if desc { o.reverse() } else { o }).then(a.key.cmp(&b.key))
        });
        if rows.is_empty() { return body.child(empty_state(web("uso_vazio_secao"))); }
        let max_calls = rows.iter().map(|b| b.chamadas).fold(1., f64::max);
        let max_main = rows.iter().map(|b| main_of(b, m)).fold(1., f64::max);
        let mut per: Vec<f64> = rows.iter().map(|b| per_call(b, tab)).filter(|v| *v > 0.).collect();
        per.sort_by(f64::total_cmp);
        let median = per.get(per.len() / 2).copied().unwrap_or(0.);
        let est = if m == Measure::Tokens { "" } else { "≈ " };
        let mut cols = vec![(Col::Name, web("uso_col_nome"), 0.),
            (Col::Calls, web(if m == Measure::Held { "uso_col_cargas" } else { "uso_col_chamadas" }), 120.),
            (Col::Main, measure_label(m), 150.),
            (Col::PerCall, web(if tab == Tab::Context { "uso_col_media" } else { match m { Measure::Held => "uso_col_tamanho", Measure::Tokens => "uso_col_tok_chamada", Measure::Ctx => "uso_col_ctx_chamada" } }), 150.)];
        if m == Measure::Held { cols.push((Col::Replies, web("uso_col_respostas"), 90.)); }
        cols.push((Col::Sessions, web("uso_col_sessoes"), 80.));
        let head = div().flex().items_center().px(px(8.)).pb(px(4.)).border_b_1().border_color(theme::border())
            .children(cols.iter().map(|(c, label, w)| {
                let c = *c;
                let arrow = if c == col { if desc { " ▾" } else { " ▴" } } else { "" };
                let button = Button::new(SharedString::from(format!("usage-col-{c:?}"))).ghost().xsmall().selected(c == col).label(format!("{label}{arrow}"))
                    .accessibility_label(web_with("uso_ordenar_por", &[("col", label.clone())]))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let s = &mut this.usage_stats;
                        let (cur, d) = s.order.get(&s.tab).copied().unwrap_or((Col::Main, true));
                        s.order.insert(s.tab, if cur == c { (c, !d) } else { (c, c != Col::Name) });
                        cx.notify();
                    }));
                if *w == 0. { div().flex_1().min_w_0().child(button) } else { div().w(px(*w)).flex_shrink_0().flex().justify_end().child(button) }
            }));
        body = body.child(head);
        let selected = s.selected.clone();
        let total = rows.len();
        for b in rows.into_iter().take(if s.expanded { usize::MAX } else { TOP_ROWS }) {
            let (v, pc) = (main_of(b, m), per_call(b, tab));
            let outlier = tab != Tab::Context && median > 0. && b.chamadas >= 1. && pc >= 3. * median;
            let key = b.key.clone();
            let on = selected.as_ref().is_some_and(|(t, k)| *t == tab && *k == key);
            let bar = |frac: f64, color: Hsla| div().w(px(40.)).h(px(4.)).rounded_full().bg(theme::inset())
                .child(div().h_full().rounded_full().bg(color).w(relative(frac as f32)));
            let mut cells = vec![
                (div().flex().items_center().gap(px(6.)).child(bar(b.chamadas / max_calls, chart(2))).child(dec(b.chamadas, 0)), 120.),
                (div().flex().items_center().gap(px(6.)).child(bar(v / max_main, chart(1))).child(if v > 0. { format!("{est}{}", tok(v)) } else { "—".into() }), 150.),
                (div().flex().items_center().gap(px(4.))
                    .when(outlier, |el| el.child(div().id(SharedString::from(format!("usage-outlier-{key}"))).text_color(theme::warning()).child("●")
                        .tooltip({ let t = web_with("uso_pesada_por_chamada", &[("x", dec(pc / median, 0))]); move |w, cx| gpui_kit::component::tooltip::Tooltip::new(t.clone()).build(w, cx) })))
                    .child(if pc > 0. { format!("{est}{}", tok(pc)) } else { "—".into() }), 150.),
            ];
            if m == Measure::Held { cells.push((div().child(dec(b.respostas, 0)), 90.)); }
            cells.push((div().child(dec(b.sessions, 0)), 80.));
            let name = div().flex().items_center().gap(px(6.)).child(div().truncate().child(row_label(tab, b)))
                .when(!b.plugin.is_empty() && tab != Tab::Plugin, |el| el.child(div().px(px(6.)).rounded(px(4.)).bg(theme::inset())
                    .text_size(px(11.)).text_color(theme::muted()).child(group_name(&b.plugin))));
            body = body.child(Button::new(SharedString::from(format!("usage-row-{tab:?}-{key}"))).ghost().selected(on).w_full().h(px(32.)).px(px(8.))
                .tooltip(b.key.clone())
                .child(div().w_full().flex().items_center().text_size(px(13.))
                    .child(name.flex_1().min_w_0())
                    .children(cells.into_iter().map(|(c, w)| div().w(px(w)).flex_shrink_0().flex().justify_end().child(c))))
                .on_click(cx.listener(move |this, _, _, cx| this.usage_select(tab, key.clone(), cx))));
        }
        if total > TOP_ROWS {
            let expanded = s.expanded;
            body = body.child(Button::new("usage-more").ghost().small()
                .label(if expanded { web("uso_mostrar_menos") } else { web_with("uso_mostrar_mais", &[("n", (total - TOP_ROWS).to_string())]) })
                .on_click(cx.listener(|this, _, _, cx| { this.usage_stats.expanded = !this.usage_stats.expanded; cx.notify(); })));
        }
        body
    }

    fn render_usage_detail(&self, tab: Tab, b: &Item, rate: Option<f64>, cx: &mut Context<Self>) -> Div {
        let m = tab.measure();
        let pair = |t: f64, c: f64| format!("{} · {}", tok(t), money2(c, rate));
        let mut stats = vec![(web("uso_col_chamadas"), dec(b.chamadas, 0)), (web("uso_col_sessoes"), dec(b.sessions, 0))];
        if matches!(tab, Tab::Skill | Tab::Agent) {
            stats.push((web(if tab == Tab::Skill { "uso_col_origem_skill" } else { "uso_col_origem_agente" }),
                format!("{} / {}", dec(b.pedidas, 0), dec(b.chamadas - b.pedidas, 0))));
        }
        if m == Measure::Tokens {
            stats.extend([(web("uso_col_custo_real"), money2(b.cost, rate)), (web("custos_input_sem_cache"), pair(b.input, b.cost_input)),
                (web("custos_output_total"), pair(b.output, b.cost_output)), (web("custos_tipo_cache_escrito"), pair(b.cache_write, b.cost_cache_write)),
                (web("custos_tipo_cache_lido"), pair(b.cache_read, b.cost_cache_read)),
                (web("uso_col_tok_sem_cache_chamada"), tok(if b.chamadas > 0. { b.no_cache() / b.chamadas } else { 0. }))]);
        }
        if m == Measure::Held {
            stats.extend([(web("uso_col_ocupados_eq"), format!("≈ {}", tok(b.ocupados_eq_tokens_est))),
                (web("uso_col_ocupados"), format!("≈ {}", tok(b.ocupados_tokens_est))), (web("uso_col_respostas"), dec(b.respostas, 0))]);
        }
        stats.extend([(web("uso_col_ctx"), format!("≈ {}", tok(b.ctx_tokens_est))), (web("uso_detalhe_ctx_chamada"), format!("≈ {}", tok(b.ctx_per_call()))),
            (web("uso_detalhe_media_sessao"), format!("≈ {}", tok(b.per_session())))]);
        let head = div().flex().items_start().gap(px(8.))
            .child(div().flex_1().min_w_0().flex().flex_col().gap(px(2.))
                .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(row_label(tab, b)))
                .child(div().text_size(px(12.)).text_color(theme::muted())
                    .child(if b.plugin.is_empty() { tab.label() } else { format!("{} · {}", tab.label(), group_name(&b.plugin)) })))
            .child(Button::new("usage-detail-close").ghost().xsmall().label(web("uso_detalhe_fechar"))
                .on_click(cx.listener(|this, _, _, cx| { this.usage_stats.selected = None; this.usage_stats.series.reset(); cx.notify(); })));
        let grid = div().flex().flex_col().gap(px(6.)).children(stats.into_iter().map(|(k, v)| div().flex().gap(px(8.)).text_size(px(12.5))
            .child(div().flex_1().text_color(theme::muted()).child(k)).child(div().font_weight(FontWeight::MEDIUM).child(v))));
        let series = &self.usage_stats.series;
        let chart_part = match &series.value {
            _ if series.loading => loading_state(),
            Some(Err(error)) => note_box(format!("⚠ {error}"), theme::warning()).child(div().mt(px(6.)).child(Button::new("usage-series-retry").outline().xsmall()
                .label(web("config_server_tentar_de_novo")).on_click(cx.listener(|this, _, _, cx| this.load_usage_series(cx))))),
            Some(Ok(days)) if !days.is_empty() => day_bars("usage-series", days, |d| main_of(d, m), 90., None, None),
            _ => empty_state(web("uso_detalhe_sem_serie")),
        };
        let partial = (!series.loading && series.ok().is_some() && !self.usage_stats.series_partial.is_empty())
            .then(|| partial_note("usage-series-partial-retry", &self.usage_stats.series_partial,cx.listener(|this, _, _, cx| this.load_usage_series(cx))));
        card_plain().child(head).child(grid)
            .child(div().flex().gap(px(6.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(web("uso_detalhe_por_dia"))
                .child(div().font_weight(FontWeight::NORMAL).text_color(theme::muted()).child(format!("({})", measure_label(m)))))
            .children(partial)
            .child(chart_part)
    }
}

/// Totais de um grupo de agentes e cada item somado pelas chaves (Claude e Codex juntos): (plugin, total, [(nome, soma, chaves)]).
fn agent_groups(r: &Report) -> Vec<(String, Item, Vec<(String, Item, Vec<String>)>)> {
    let by_key: HashMap<&str, &Item> = r.by_agente.iter().map(|b| (b.key.as_str(), b)).collect();
    let mut groups: Vec<_> = group_by_plugin(&r.by_agente, |b| b.cost, "@nativo").into_iter().map(|g| {
        let mut items: Vec<(String, Item, Vec<String>)> = g.items.into_iter().map(|i| {
            let mut sum = Item::default();
            for k in &i.keys { if let Some(b) = by_key.get(k.as_str()) { sum.add(b); } }
            (i.name, sum, i.keys)
        }).collect();
        items.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost).then(b.1.chamadas.total_cmp(&a.1.chamadas)).then(a.0.cmp(&b.0)));
        let mut total = Item::default();
        for (_, b, _) in &items { total.add(b); }
        (g.plugin, total, items)
    }).collect();
    groups.sort_by(|a, b| b.1.cost.total_cmp(&a.1.cost).then(b.1.chamadas.total_cmp(&a.1.chamadas)).then(a.0.cmp(&b.0)));
    groups
}

/// Ferramentas chamadas, sem `Skill` e `Agent` (repetem skills e subagentes), da mais chamada.
fn tools_of(r: &Report) -> Vec<Item> {
    let mut tools: Vec<Item> = r.by_tool.iter().filter(|b| b.key != "Skill" && b.key != "Agent" && b.chamadas > 0.).cloned().collect();
    tools.sort_by(|a, b| b.chamadas.total_cmp(&a.chamadas));
    tools
}

/// Barra horizontal de uma célula da tabela de skills (peso em azul, vezes em laranja) e o número.
fn bar_cell(v: f64, max: f64, weight: bool) -> Div {
    let frac = if v > 0. { (v / max).max(0.008) } else { 0. } as f32;
    div().w(px(260.)).flex_shrink_0().flex().items_center().gap(px(10.)).pl(px(12.))
        .child(div().flex_1().h(px(5.)).rounded_full().bg(theme::inset())
            .child(div().h_full().rounded_full().bg(chart(if weight { 1 } else { 2 })).w(relative(frac))))
        .child(div().w(px(70.)).flex().justify_end().text_size(px(13.))
            .child(if weight { if v > 0. { format!("≈ {}", tok(v)) } else { "—".into() } } else { dec(v, 0) }))
}

/// Colunas por dia com rótulos no início, meio e fim; com `cx`, passar o mouse marca o dia.
fn day_bars(id: &'static str, days: &[Item], value: impl Fn(&Item) -> f64, height: f32, hover: Option<&str>, cx: Option<&mut Context<Hangar>>) -> Div {
    if days.is_empty() { return div(); }
    let top = days.iter().map(&value).fold(0., f64::max);
    let top = if top > 0. { top } else { 1. };
    let n = days.len();
    let marks: BTreeMap<usize, String> = [0, n.saturating_sub(1) / 2, n.saturating_sub(1)].into_iter().map(|i| (i, short_day(&days[i].key))).collect();
    let listener = cx.map(|cx| cx.listener(|this: &mut Hangar, (key, hovered): &(String, bool), _: &mut Window, cx: &mut Context<Hangar>| {
        if *hovered { this.usage_stats.hover_day = Some(key.clone()); cx.notify(); }
    }));
    let listener = listener.map(std::rc::Rc::new);
    let columns = days.iter().map(|d| {
        let v = value(d);
        let h = if v > 0. { ((v / top) as f32 * height).max(2.) } else { 0. };
        let lit = hover.is_none_or(|k| k == d.key);
        let key = d.key.clone();
        let col = div().id(SharedString::from(format!("{id}-{}", d.key))).flex_1().h_full().flex().flex_col().justify_end().items_center()
            .child(div().w(relative(0.7)).max_w(px(22.)).min_w(px(2.)).h(px(h)).rounded(px(3.)).bg(chart(1)).when(!lit, |el| el.opacity(0.45)));
        match listener.clone() {
            Some(l) => col.on_hover(move |hovered: &bool, window, cx| l(&(key.clone(), *hovered), window, cx)),
            None => col,
        }
    });
    div().flex().flex_col().gap(px(4.))
        .child(div().h(px(height)).flex().items_end().border_b_1().border_color(theme::border_strong()).children(columns))
        .child(div().flex().children((0..n).map(|i| div().flex_1().flex().justify_center().text_size(px(10.5)).text_color(theme::faint())
            .child(marks.get(&i).cloned().unwrap_or_default()))))
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob traz o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    #[test]
    fn plugin_groups_merge_same_name_and_order_by_weight() {
        let item = |key: &str, plugin: &str, w: f64, calls: f64| Item { key: key.into(), plugin: plugin.into(), ocupados_eq_tokens_est: w, chamadas: calls, ..Default::default() };
        let list = [item("superpowers:brainstorming", "", 10., 1.), item("brainstorming", "superpowers", 5., 2.), item("solo", "", 30., 1.), item("x:zero", "", 0., 0.)];
        let groups = group_by_plugin(&list, |b| b.ocupados_eq_tokens_est, "@avulsa");
        assert_eq!(groups.iter().map(|g| g.plugin.as_str()).collect::<Vec<_>>(), ["@avulsa", "superpowers"]);
        let sp = &groups[1];
        assert_eq!((sp.items.len(), sp.weight, sp.times), (1, 15., 3.));
        assert_eq!(sp.items[0].keys[0], "superpowers:brainstorming");
        assert_eq!((context_category("hook_x"), context_category("skill_listing_a"), context_category("x_reminder")), (2, 1, 3));
    }
}
