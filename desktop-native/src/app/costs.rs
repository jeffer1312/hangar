//! Custos: página própria, fora das Configurações, com o relatório do web (`Costs.svelte`, `/api/costs`) — os mesmos
//! números, cortes e filtros, no desenho do nativo. A página irmã Estatísticas de uso (`stats.rs`) abre pelo link do topo.
//! Os textos são os do web (`tr_web`): a tela é a mesma, e copiá-los como `native_*` seria manter dois dicionários.
//! Como no web, o relatório soma as máquinas próprias e ligadas (`mergeReports`): cada uma é lida à parte, a que não
//! responde ou devolve outro período fica fora da soma e nomeada no aviso, e a escolha de quais entram é guardada.
use super::*;
use std::collections::BTreeMap;
use super::device::Remote;
use super::settings::segments;
use crate::appearance::{self, Currency};
use gpui_kit::component::menu::{DropdownMenu, PopupMenuItem};
use serde::Deserialize;

/// Quantas vezes a página repergunta enquanto o servidor ainda lê o histórico (202 `aquecendo`), a cada 3 s: ~5 min.
pub(super) const WARM_TRIES: u32 = 100;
const WARM_EVERY: Duration = Duration::from_secs(3);
/// Máximo de entidades no Comparar: são quatro cores de gráfico.
const COMPARE_MAX: usize = 4;

// ── Textos e números ────────────────────────────────────────────────────────

/// Texto do web pela chave dele.
pub(super) fn web(key: &str) -> String { crate::i18n::tr_web(key, &HashMap::new()).unwrap_or_else(|| key.to_owned()) }

/// Texto do web com os `{nome}` trocados.
pub(super) fn web_with(key: &str, params: &[(&str, String)]) -> String {
    let map = params.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
    crate::i18n::tr_web(key, &map).unwrap_or_else(|| key.to_owned())
}

/// Número com `places` casas, vírgula decimal e ponto de milhar no português (o `dec` do web).
pub(super) fn dec(n: f64, places: usize) -> String {
    let comma = tr("decimal") == ",";
    let raw = format!("{:.*}", places, n.abs());
    let (int, frac) = raw.split_once('.').map_or((raw.as_str(), ""), |(i, f)| (i, f));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 { grouped.push(if comma { '.' } else { ',' }); }
        grouped.push(c);
    }
    let sign = if n < 0. && raw.chars().any(|c| c.is_ascii_digit() && c != '0') { "-" } else { "" };
    if frac.is_empty() { format!("{sign}{grouped}") } else { format!("{sign}{grouped}{}{frac}", if comma { ',' } else { '.' }) }
}

/// Tokens na escala: os cortes olham o valor já arredondado, como o `tok` do web ("999,5 mil" sobe para "1,0 Mi").
pub(super) fn tok(n: f64) -> String {
    let en = crate::i18n::english();
    if n >= 999.95e6 { format!("{} {}", dec(n / 1e9, 2), if en { "B" } else { "Bi" }) }
    else if n >= 999.5e3 { format!("{} {}", dec(n / 1e6, 1), if en { "M" } else { "Mi" }) }
    else if n >= 1e3 { format!("{}{}", dec(n / 1e3, 0), if en { "k" } else { " mil" }) }
    else { dec(n, 0) }
}

fn currency_value(usd: f64, rate: Option<f64>) -> (&'static str, f64) {
    match (appearance::get().currency, rate) { (Currency::Brl, Some(rate)) => ("R$", usd * rate), _ => ("US$", usd) }
}

/// Valor exato, com centavos: tabelas e o que se soma ou compara (o `money2` do web). Sem cotação, fica em dólar.
pub(super) fn money2(usd: f64, rate: Option<f64>) -> String {
    let (symbol, value) = currency_value(usd, rate);
    format!("{symbol} {}{}", if value < 0. { "-" } else { "" }, dec(value.abs(), 2))
}

/// Grandeza, compacta a partir de mil: só nos números grandes (o `money` do web).
pub(super) fn money(usd: f64, rate: Option<f64>) -> String {
    let (symbol, value) = currency_value(usd, rate);
    if value.abs() < 1000. { return money2(usd, rate); }
    let en = crate::i18n::english();
    let (scaled, unit) = if value.abs() >= 1e9 { (value / 1e9, if en { "B" } else { " bi" }) }
        else if value.abs() >= 1e6 { (value / 1e6, if en { "M" } else { " mi" }) } else { (value / 1e3, if en { "K" } else { " mil" }) };
    let text = dec(scaled, 1);
    let text = text.strip_suffix(if tr("decimal") == "," { ",0" } else { ".0" }).unwrap_or(&text).to_owned();
    format!("{symbol} {text}{unit}")
}

fn pct(n: f64, total: f64) -> String { if total > 0. { format!("{}%", dec(n / total * 100., 1)) } else { "—".into() } }

/// Rótulo do dia no eixo: "26/09".
fn day_label(key: &str) -> String { if key.len() >= 10 { format!("{}/{}", &key[8..10], &key[5..7]) } else { key.to_owned() } }

/// A paleta de gráfico do web (`--chart-1..4` do `app.css`), no tom do tema da tela; a 1 é o destaque.
pub(super) fn chart(slot: usize) -> Hsla {
    let dark = theme::is_dark();
    match slot {
        1 => theme::accent(),
        2 => rgb(if dark { 0xd95926 } else { 0xeb6834 }).into(),
        3 => rgb(if dark { 0x199e70 } else { 0x1baf7a }).into(),
        _ => rgb(if dark { 0xc98500 } else { 0xeda100 }).into(),
    }
}

fn source_name(key: &str) -> String {
    match key { "claude" => web("custos_claude_code"), "codex" => "Codex".into(), "pi" => "Pi".into(), "omp" => "oh-my-pi".into(),
        "kimi" => "Kimi".into(), other => other.to_owned() }
}

/// Cor por fonte, não por posição: um dia em que o Codex passasse o Claude não troca as duas cores.
fn source_color(key: &str) -> Hsla {
    match key { "claude" => chart(1), "codex" => chart(2), "pi" => chart(3), "kimi" => theme::muted(), _ => chart(4) }
}

/// Nome curto do projeto: a última pasta do caminho.
pub(super) fn project_label(path: &str) -> String {
    let name = path.trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if name.is_empty() { web("formato_sem_projeto") } else { name.to_owned() }
}

/// Modelo grátis pelo id (`free`, `-free`, `:free`, `free:thinking`): preço zero de verdade, não "sem tarifa".
fn is_free(model: &str) -> bool {
    let lower = model.to_lowercase();
    lower.match_indices("free").any(|(at, _)| {
        let before = lower[..at].chars().next_back();
        let after = lower[at + 4..].chars().next();
        before.is_none_or(|c| matches!(c, '/' | ':' | '-')) && after.is_none_or(|c| c == ':')
    })
}

// ── Dados do servidor ───────────────────────────────────────────────────────

/// Um corte qualquer (dia, provedor, fonte, projeto, modelo). Tokens em `f64` para a conta não virar conversão.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct Bucket {
    pub(super) key: String,
    pub(super) label: Option<String>,
    pub(super) sessions: f64,
    pub(super) subagentes: f64,
    pub(super) input: f64,
    pub(super) output: f64,
    pub(super) cache_write: f64,
    pub(super) cache_read: f64,
    pub(super) cost: f64,
    pub(super) cost_input: f64,
    pub(super) cost_output: f64,
    pub(super) cost_cache_write: f64,
    pub(super) cost_cache_read: f64,
    pub(super) cache_write_1h: f64,
    pub(super) regravado: f64,
    pub(super) custo_regravado: f64,
}

impl Bucket {
    fn zero(key: &str) -> Self { Bucket { key: key.to_owned(), ..Default::default() } }
    /// Os quatro tipos somados (o `brutos` do web).
    pub(super) fn raw(&self) -> f64 { self.input + self.output + self.cache_write + self.cache_read }
    /// Tokens que o modelo viu pela primeira vez: o cache lido fica de fora.
    fn fresh(&self) -> f64 { self.input + self.cache_write + self.output }
    /// Volume com custo zero só acontece sem tarifa: "não sei o preço", não "de graça".
    fn unknown(&self) -> bool { self.raw() > 0. && self.cost == 0. }
    fn tokens_of(&self, kind: Kind) -> f64 {
        match kind { Kind::Input => self.input, Kind::Output => self.output, Kind::CacheWrite => self.cache_write, Kind::CacheRead => self.cache_read }
    }
    fn cost_of(&self, kind: Kind) -> f64 {
        match kind { Kind::Input => self.cost_input, Kind::Output => self.cost_output, Kind::CacheWrite => self.cost_cache_write,
            Kind::CacheRead => self.cost_cache_read }
    }
    fn value(&self, metric: Metric) -> f64 { if metric == Metric::Cost { self.cost } else { self.raw() } }
}

/// Uma combinação que aconteceu (dia × provedor × fonte × projeto × modelo × subagente): todo recorte vira uma soma.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Combo {
    dia: String,
    provider: String,
    source: String,
    project: String,
    model: String,
    subagente: bool,
    sessions: f64,
    session_ids: Option<Vec<String>>,
    custo_sem_cache: Option<f64>,
    equivalente_cobrado: Option<f64>,
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
    cost: f64,
    cost_input: f64,
    cost_output: f64,
    cost_cache_write: f64,
    cost_cache_read: f64,
    cache_write_1h: f64,
    regravado: f64,
    custo_regravado: f64,
    /// Id da máquina que mandou a linha, carimbado na soma.
    #[serde(skip)]
    machine: String,
}

impl Combo {
    fn field(&self, dim: Dim) -> &str {
        match dim { Dim::Provider => &self.provider, Dim::Source => &self.source, Dim::Project => &self.project, Dim::Model => &self.model,
            Dim::Machine => &self.machine }
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Session {
    session_id: String,
    source: String,
    provider: String,
    project: String,
    model: String,
    inicio: String,
    fim: String,
    subagentes: f64,
    input: f64,
    output: f64,
    cache_write: f64,
    cache_read: f64,
    cost: f64,
    custo_regravado: f64,
    #[serde(skip)]
    machine: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Rate { provider: String, model: String, input: f64, output: f64, cache_read: f64, cache_write: f64, origin: String, cache_estimado: bool }

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub(super) struct Report {
    pub(super) totals: Bucket,
    pub(super) by_day: Vec<Bucket>,
    by_provider: Vec<Bucket>,
    by_source: Vec<Bucket>,
    by_project: Vec<Bucket>,
    pub(super) by_model: Vec<Bucket>,
    rates: Vec<Rate>,
    pub(super) sem_tarifa: Vec<String>,
    custo_sem_cache: f64,
    equivalente_cobrado: f64,
    anterior: Option<Bucket>,
    pub(super) usd_brl: Option<f64>,
    combos: Vec<Combo>,
    sessoes: Vec<Session>,
    /// Falso quando o servidor não manda cache de 1 h nem cache perdido (versão antiga).
    #[serde(skip)]
    cache_detailed: bool,
    /// Total de cada máquina somada, chave = id da máquina e rótulo = nome dela.
    #[serde(skip)]
    by_machine: Vec<Bucket>,
}

impl Report {
    fn parse(value: &Value) -> Result<Report, String> {
        let mut report: Report = serde_json::from_value(value.clone()).map_err(|_| tr("invalid_response"))?;
        report.cache_detailed = value.pointer("/totals/regravado").is_some() || report.totals.cache_write == 0.;
        Ok(report)
    }
    fn list(&self, dim: Dim) -> &[Bucket] {
        match dim { Dim::Provider => &self.by_provider, Dim::Source => &self.by_source, Dim::Project => &self.by_project, Dim::Model => &self.by_model,
            Dim::Machine => &self.by_machine }
    }
    /// Tarifa por modelo: `None` dentro do mapa quando o mesmo modelo tem preço em mais de um provedor.
    fn rates_by_model(&self) -> HashMap<String, Option<Rate>> {
        let mut out: HashMap<String, Option<Rate>> = HashMap::new();
        for rate in &self.rates {
            out.entry(rate.model.clone()).and_modify(|slot| *slot = None).or_insert_with(|| Some(rate.clone()));
        }
        out
    }
}

impl Bucket {
    fn absorb(&mut self, b: &Bucket) {
        self.sessions += b.sessions; self.subagentes += b.subagentes;
        self.input += b.input; self.output += b.output; self.cache_write += b.cache_write; self.cache_read += b.cache_read;
        self.cost += b.cost; self.cost_input += b.cost_input; self.cost_output += b.cost_output;
        self.cost_cache_write += b.cost_cache_write; self.cost_cache_read += b.cost_cache_read;
        self.cache_write_1h += b.cache_write_1h; self.regravado += b.regravado; self.custo_regravado += b.custo_regravado;
    }
}

/// O rótulo é do primeiro servidor que souber dizer: máquina antiga manda a linha sem ele.
fn join(dest: &mut HashMap<String, Bucket>, list: &[Bucket]) {
    for b in list {
        let target = dest.entry(b.key.clone()).or_insert_with(|| Bucket::zero(&b.key));
        if target.label.is_none() { target.label = b.label.clone(); }
        target.absorb(b);
    }
}

fn by_cost(map: HashMap<String, Bucket>) -> Vec<Bucket> {
    let mut list: Vec<Bucket> = map.into_values().collect();
    list.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| a.key.cmp(&b.key)));
    list
}

/// O `mergeReports` do web: soma por chave, linhas e sessões carimbadas com a máquina, tarifas por provedor e modelo.
fn merge_costs(parts: &[MachinePart<Report>]) -> Report {
    let mut out = Report { totals: Bucket::zero("totals"), cache_detailed: true, ..Default::default() };
    let mut dims: [HashMap<String, Bucket>; 5] = Default::default();
    let mut rates: HashMap<(String, String), Rate> = HashMap::new();
    let mut unpriced = std::collections::BTreeSet::new();
    let (mut entered, mut with_previous, mut detailed) = (0, 0, true);
    let mut previous = Bucket::zero("anterior");
    for p in parts {
        let r = match &p.part {
            Part::Ok(r) => r,
            Part::Mismatched(rate) => { out.usd_brl = out.usd_brl.or(*rate); continue }
            Part::Failed(_) => continue,
        };
        out.usd_brl = out.usd_brl.or(r.usd_brl);
        entered += 1;
        out.totals.absorb(&r.totals);
        let mut own = Bucket { key: p.id.clone(), label: Some(p.label.clone()), ..Default::default() };
        own.absorb(&r.totals);
        // Máquina com volume e sem detalhamento: cruzar só as outras tiraria o consumo dela de todo recorte.
        if r.combos.is_empty() && own.raw() + own.sessions + own.cost > 0. { detailed = false; }
        out.by_machine.push(own);
        for (dim, list) in dims.iter_mut().zip([&r.by_day, &r.by_provider, &r.by_source, &r.by_project, &r.by_model]) { join(dim, list); }
        for rate in &r.rates { rates.insert((rate.provider.clone(), rate.model.clone()), rate.clone()); }
        unpriced.extend(r.sem_tarifa.iter().cloned());
        out.combos.extend(r.combos.iter().map(|c| Combo { machine: p.id.clone(), ..c.clone() }));
        out.sessoes.extend(r.sessoes.iter().map(|s| Session { machine: p.id.clone(), ..s.clone() }));
        out.cache_detailed &= r.cache_detailed;
        out.custo_sem_cache += r.custo_sem_cache;
        out.equivalente_cobrado += r.equivalente_cobrado;
        if let Some(a) = &r.anterior { previous.absorb(a); with_previous += 1; }
    }
    let [day, provider, source, project, model] = dims;
    out.by_day = day.into_values().collect();
    out.by_day.sort_by(|a, b| b.key.cmp(&a.key));
    (out.by_provider, out.by_source, out.by_project, out.by_model) = (by_cost(provider), by_cost(source), by_cost(project), by_cost(model));
    out.rates = rates.into_values().collect();
    out.rates.sort_by(|a, b| a.model.cmp(&b.model));
    out.sem_tarifa = unpriced.into_iter().collect();
    // Anterior de só parte das máquinas mostraria uma alta que não existe.
    out.anterior = (entered > 0 && with_previous == entered).then_some(previous);
    if !detailed { out.combos.clear(); }
    out.sessoes.sort_by(|a, b| b.cost.total_cmp(&a.cost));
    out.by_machine.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| a.key.cmp(&b.key)));
    out
}

// ── Várias máquinas ─────────────────────────────────────────────────────────

/// O que uma máquina devolveu. Outro período é servidor antigo que ignora `?period=`: fica fora da soma, declarado, mas a
/// cotação dele ainda vale (USD/BRL não depende de período).
pub(super) enum Part<R> { Ok(R), Mismatched(Option<f64>), Failed(String) }

pub(super) struct MachinePart<R> { pub(super) id: String, pub(super) label: String, pub(super) part: Part<R> }

impl<R> MachinePart<R> {
    fn answered(&self) -> bool { !matches!(self.part, Part::Failed(_)) }
}

/// Todas falharam: a tela mostra o erro em vez de zeros que parecem relatório. Uma só leva o motivo dela; várias, cada uma o seu.
pub(super) fn all_failed<R>(parts: &[MachinePart<R>]) -> Option<String> {
    let errors = parts.iter().map(|p| match &p.part { Part::Failed(e) => Some((p.label.as_str(), e.as_str())), _ => None }).collect::<Option<Vec<_>>>()?;
    match errors.as_slice() {
        [] => None,
        [(_, error)] => Some((*error).to_owned()),
        many => Some(many.iter().map(|(label, error)| format!("{label}: {error}")).collect::<Vec<_>>().join("\n")),
    }
}

/// Quem ficou fora da soma, cada causa com os nomes das máquinas.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Partial { failed: Vec<String>, mismatched: Vec<String> }

impl Partial {
    pub(super) fn of<R>(parts: &[MachinePart<R>]) -> Partial {
        let mut out = Partial::default();
        for p in parts {
            match p.part { Part::Failed(_) => out.failed.push(p.label.clone()), Part::Mismatched(_) => out.mismatched.push(p.label.clone()), Part::Ok(_) => {} }
        }
        out
    }
    pub(super) fn is_empty(&self) -> bool { self.failed.is_empty() && self.mismatched.is_empty() }
    pub(super) fn text(&self) -> String {
        let mut text = format!("⚠ {}", web("custos_total_parcial"));
        for (names, one, many) in [(&self.failed, "custos_servidor_nao_respondeu_1", "custos_servidor_nao_respondeu"),
            (&self.mismatched, "custos_fora_periodo_1", "custos_fora_periodo")] {
            if names.is_empty() { continue; }
            let phrase = if names.len() == 1 { web(one) } else { web_with(many, &[("n", names.len().to_string())]) };
            text.push_str(&format!(" {phrase} ({}).", names.join(", ")));
        }
        text
    }
}

/// Uma máquina do relatório: o id é a chave do corte e da escolha; o rótulo é editável e pode repetir.
#[derive(Clone)]
pub(super) struct Machine { pub(super) id: String, pub(super) label: String, key: String }

/// Máquina na primeira leitura do histórico (202 `aquecendo`): lidos e total.
pub(super) struct Warming { id: String, label: String, read: u64, total: u64 }

pub(super) fn set_warming(list: &mut Vec<Warming>, m: &Machine, progress: Option<(u64, u64)>) {
    list.retain(|w| w.id != m.id);
    if let Some((read, total)) = progress { list.push(Warming { id: m.id.clone(), label: m.label.clone(), read, total }); }
}

pub(super) fn warming_note(w: &Warming) -> Div {
    let text = if w.total > 0 { web_with("custos_aquecendo_progresso", &[("maquina", w.label.clone()), ("lidos", w.read.to_string()), ("total", w.total.to_string())]) }
        else { web_with("custos_aquecendo", &[("maquina", w.label.clone())]) };
    let done = if w.total > 0 { (w.read as f32 / w.total as f32).clamp(0., 1.) } else { 0.1 };
    note_box(text, theme::muted()).child(div().mt(px(8.)).h(px(4.)).rounded_full().bg(theme::border_strong())
        .child(div().h_full().rounded_full().bg(theme::accent()).w(relative(done))))
}

pub(super) fn partial_note(id: &'static str, partial: &Partial, retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Div {
    note_box(partial.text(), theme::warning()).child(div().mt(px(6.)).child(Button::new(id).outline().xsmall()
        .label(web("config_server_tentar_de_novo")).on_click(retry)))
}

/// Ganchos de uma leitura por máquina: se o pedido ainda vale, o progresso da primeira leitura e a resposta.
#[derive(Clone, Copy)]
pub(super) struct MachineRead {
    pub(super) seq: u64,
    pub(super) alive: fn(&Hangar, u64) -> bool,
    pub(super) warm: fn(&mut Hangar, &Machine, Option<(u64, u64)>),
    pub(super) done: fn(&mut Hangar, u64, &Machine, Result<Value, String>, &mut Context<Hangar>),
}

/// Área do código do relatório de uso (`/api/uso`, `by_area`).
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Area { key: String, input: f64, output: f64, cache_write: f64, cache_read: f64, cost: f64 }

impl Area { fn tokens(&self) -> f64 { self.input + self.output + self.cache_write + self.cache_read } }

/// Escolhas guardadas da página: projetos tirados da lista e máquinas tiradas da soma.
fn prefs_path() -> Option<PathBuf> { Some(saved_connection_path()?.with_file_name("costs.json")) }

fn load_prefs() -> (HashSet<String>, HashSet<String>) {
    let saved = prefs_path().and_then(|p| std::fs::read(p).ok()).and_then(|b| serde_json::from_slice::<Value>(&b).ok());
    let list = |key: &str| saved.as_ref().and_then(|v| v.get(key)).and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default();
    (list("hidden_projects"), list("machines_off"))
}

fn save_prefs(hidden: &HashSet<String>, off: &HashSet<String>) -> std::io::Result<()> {
    let path = prefs_path().ok_or_else(|| std::io::Error::other("sem pasta de configuração"))?;
    let sorted = |set: &HashSet<String>| { let mut list: Vec<String> = set.iter().cloned().collect(); list.sort(); list };
    std::fs::create_dir_all(path.parent().ok_or_else(|| std::io::Error::other("caminho sem pasta"))?)?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, json!({ "hidden_projects": sorted(hidden), "machines_off": sorted(off) }).to_string())?;
    std::fs::rename(tmp, path)
}

fn merge_areas(parts: &[Option<Vec<Area>>]) -> Vec<Area> {
    let mut out: Vec<Area> = Vec::new();
    for a in parts.iter().flatten().flatten() {
        match out.iter_mut().find(|x| x.key == a.key) {
            Some(x) => { x.input += a.input; x.output += a.output; x.cache_write += a.cache_write; x.cache_read += a.cache_read; x.cost += a.cost; }
            None => out.push(a.clone()),
        }
    }
    out
}

// ── Estado da página ────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum View { #[default] Costs, Usage }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Period { Day, Week, #[default] Month, Quarter, All }

impl Period {
    pub(super) const ALL: [Period; 5] = [Period::Day, Period::Week, Period::Month, Period::Quarter, Period::All];
    pub(super) fn key(self) -> &'static str {
        match self { Period::Day => "1d", Period::Week => "7d", Period::Month => "30d", Period::Quarter => "90d", Period::All => "all" }
    }
    pub(super) fn label(self) -> String {
        web(match self { Period::Day => "custos_periodo_1d", Period::Week => "custos_periodo_7d", Period::Month => "custos_periodo_30d",
            Period::Quarter => "custos_periodo_90d", Period::All => "custos_periodo_tudo" })
    }
    fn days(self) -> f64 { match self { Period::Day => 1., Period::Week => 7., Period::Month => 30., Period::Quarter => 90., Period::All => 0. } }
    pub(super) fn labels() -> Vec<String> { Self::ALL.map(Period::label).to_vec() }
    pub(super) fn index(self) -> usize { Self::ALL.iter().position(|p| *p == self).unwrap_or(2) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Dim { Provider, Source, Project, Model, Machine }

impl Dim {
    const ALL: [Dim; 5] = [Dim::Provider, Dim::Source, Dim::Project, Dim::Model, Dim::Machine];
    fn name(self) -> String {
        web(match self { Dim::Provider => "custos_dim_provedor", Dim::Source => "custos_dim_fonte", Dim::Project => "custos_dim_projeto",
            Dim::Model => "custos_dim_modelo", Dim::Machine => "custos_dim_maquina" })
    }
    fn id(self) -> &'static str {
        match self { Dim::Provider => "provider", Dim::Source => "source", Dim::Project => "project", Dim::Model => "model", Dim::Machine => "machine" }
    }
    /// Com uma máquina só, "qual máquina" não é escolha.
    fn shown(multi: bool) -> Vec<Dim> { Dim::ALL.into_iter().filter(|d| multi || *d != Dim::Machine).collect() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind { Input, Output, CacheWrite, CacheRead }

impl Kind {
    const ALL: [Kind; 4] = [Kind::Input, Kind::Output, Kind::CacheWrite, Kind::CacheRead];
    fn id(self) -> &'static str { match self { Kind::Input => "input", Kind::Output => "output", Kind::CacheWrite => "cache_write", Kind::CacheRead => "cache_read" } }
    fn label(self) -> String {
        web(match self { Kind::Input => "custos_input_sem_cache", Kind::Output => "custos_output_total", Kind::CacheWrite => "custos_tipo_cache_escrito",
            Kind::CacheRead => "custos_tipo_cache_lido" })
    }
    fn color(self) -> Hsla { chart(match self { Kind::Input => 1, Kind::Output => 2, Kind::CacheWrite => 3, Kind::CacheRead => 4 }) }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Metric { #[default] Tokens, Cost }

/// Recorte cruzado: cada dimensão é uma lista (vazia = todas) e o subagente tem três estados.
#[derive(Clone, Debug, Default, PartialEq)]
struct Filter { lists: HashMap<Dim, Vec<String>>, sub: Option<bool> }

impl Filter {
    fn get(&self, dim: Dim) -> &[String] { self.lists.get(&dim).map_or(&[], Vec::as_slice) }
    fn has(&self, dim: Dim, key: &str) -> bool { self.get(dim).iter().any(|k| k == key) }
    fn active(&self) -> bool { self.sub.is_some() || Dim::ALL.iter().any(|d| !self.get(*d).is_empty()) }
    fn without(&self, dim: Dim) -> Filter { let mut f = self.clone(); f.lists.remove(&dim); f }
    /// Escrita (o `aplicar` do web): sem detalhamento só vale uma dimensão e um valor, o que os `by_*` sabem somar.
    fn set(&mut self, dim: Dim, values: Vec<String>, cross: bool) {
        if !cross { self.lists.clear(); self.sub = None; }
        let values = if cross { values } else { values.into_iter().take(1).collect() };
        if values.is_empty() { self.lists.remove(&dim); } else { self.lists.insert(dim, values); }
    }
}

fn matches(values: &[String], x: &str) -> bool { values.is_empty() || values.iter().any(|v| v == x) }

fn filtered<'a>(combos: &'a [Combo], f: &Filter) -> Vec<&'a Combo> {
    combos.iter().filter(|c| Dim::ALL.iter().all(|d| matches(f.get(*d), c.field(*d))) && f.sub.is_none_or(|s| c.subagente == s)).collect()
}

/// Soma de linhas; a sessão conta uma vez pelo id (o do subagente já carrega a marca dele).
fn add(target: &mut Bucket, c: &Combo, seen: &mut HashSet<String>) {
    match &c.session_ids {
        Some(ids) => for id in ids {
            if seen.insert(id.clone()) {
                target.sessions += 1.;
                if c.subagente { target.subagentes += 1.; }
            }
        },
        None => { target.sessions += c.sessions; if c.subagente { target.subagentes += c.sessions; } }
    }
    target.input += c.input; target.output += c.output; target.cache_write += c.cache_write; target.cache_read += c.cache_read;
    target.cost += c.cost; target.cost_input += c.cost_input; target.cost_output += c.cost_output;
    target.cost_cache_write += c.cost_cache_write; target.cost_cache_read += c.cost_cache_read;
    target.cache_write_1h += c.cache_write_1h; target.regravado += c.regravado; target.custo_regravado += c.custo_regravado;
}

fn sum(combos: &[&Combo]) -> Bucket {
    let (mut total, mut seen) = (Bucket::zero("totals"), HashSet::new());
    for c in combos { add(&mut total, c, &mut seen); }
    total
}

fn group_by(combos: &[&Combo], key: impl Fn(&Combo) -> &str) -> Vec<Bucket> {
    let mut groups: HashMap<String, (Bucket, HashSet<String>)> = HashMap::new();
    for c in combos {
        let k = key(c);
        let (bucket, seen) = groups.entry(k.to_owned()).or_insert_with(|| (Bucket::zero(k), HashSet::new()));
        add(bucket, c, seen);
    }
    let mut out: Vec<Bucket> = groups.into_values().map(|(b, _)| b).collect();
    out.sort_by(|a, b| b.cost.total_cmp(&a.cost).then_with(|| a.key.cmp(&b.key)));
    out
}

fn group(combos: &[&Combo], dim: Dim) -> Vec<Bucket> { group_by(combos, |c| c.field(dim)) }

/// Custo "se nenhum token fosse cache" de um recorte, com a tarifa do modelo (linha sem tarifa não entra).
fn without_cache(combos: &[&Combo], rates: &HashMap<String, Option<Rate>>) -> f64 {
    combos.iter().map(|c| c.custo_sem_cache.unwrap_or_else(|| rates.get(&c.model).cloned().flatten()
        .map_or(0., |t| (c.input + c.cache_write + c.cache_read) / 1e6 * t.input + c.output / 1e6 * t.output))).sum()
}

/// Tokens em equivalente de entrada: cada tipo pesado pela própria tarifa.
fn equivalent(combos: &[&Combo], rates: &HashMap<String, Option<Rate>>) -> f64 {
    combos.iter().map(|c| c.equivalente_cobrado.unwrap_or_else(|| match rates.get(&c.model).cloned().flatten() {
        Some(t) if t.input > 0. => c.input + c.output * (t.output / t.input) + c.cache_write * (t.cache_write / t.input)
            + c.cache_read * (t.cache_read / t.input),
        _ => 0.,
    })).sum()
}

/// Dias de `from` a `to` (inclusive), para o eixo não pular dia parado.
fn days_between(from: &str, to: &str, step: i64) -> Vec<String> {
    let parse = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok();
    let (Some(mut day), Some(end)) = (parse(from), parse(to)) else { return vec![from.to_owned()] };
    let mut out = Vec::new();
    // O teto guarda contra data torta virar laço sem fim.
    while day <= end && out.len() < 800 {
        out.push(day.format("%Y-%m-%d").to_string());
        day += chrono::Duration::days(step);
    }
    out
}

/// Segunda-feira da semana do dia, para o Comparar semanal.
fn monday(day: &str) -> String {
    use chrono::Datelike;
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map_or_else(|_| day.to_owned(),
        |d| (d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64)).format("%Y-%m-%d").to_string())
}

/// Página de custos: o relatório do período e tudo o que a pessoa escolheu nela.
#[derive(Default)]
pub(super) struct Costs {
    /// Aberta por cima da janela; `None` é a janela da conversa.
    pub(super) view: Option<View>,
    period: Period,
    /// A soma das máquinas que já responderam; `loading` enquanto falta alguma.
    report: Remote<Report>,
    parts: Vec<MachinePart<Report>>,
    pending: usize,
    warming: Vec<Warming>,
    partial: Partial,
    /// Máquinas tiradas da soma, pelo id; lidas do disco junto dos projetos escondidos e valem também nas Estatísticas.
    pub(super) off: Option<HashSet<String>>,
    show_machines: bool,
    filter: Filter,
    daily: Metric,
    layers_off: HashSet<String>,
    layers_mode: &'static str,
    hover_day: Option<String>,
    /// Projetos tirados da lista (continuam somando); lidos do disco na primeira abertura.
    hidden: Option<HashSet<String>>,
    all_projects: bool,
    more_sessions: bool,
    areas: Remote<Vec<Area>>,
    area_parts: Vec<Option<Vec<Area>>>,
    area_pending: usize,
    areas_key: String,
    compare_dim: Option<Dim>,
    compare_metric: Metric,
    compared: Vec<String>,
    method_open: bool,
    scroll: ScrollHandle,
}

/// Tudo o que a tela mostra, derivado do relatório e do recorte uma vez por desenho.
struct Derived {
    has_combos: bool,
    filtering: bool,
    focus: Bucket,
    sub_cost: f64,
    free_only: bool,
    unknown: bool,
    empty_cut: bool,
    without_cache: f64,
    equivalent: f64,
    cache_source: Bucket,
    rates: HashMap<String, Option<Rate>>,
}

impl Hangar {
    /// Leitura do servidor que volta à janela; resposta de outra conexão é descartada.
    pub(super) fn server_get(&mut self, path: Vec<String>, query: Vec<(String, String)>, seconds: u64, cx: &mut Context<Self>,
        done: impl FnOnce(&mut Self, Result<Value, Failure>, &mut Context<Self>) + 'static) {
        let Some(api) = self.api.clone() else { return };
        let connection = self.connection;
        let task = self.runtime.spawn(async move {
            let path: Vec<&str> = path.iter().map(String::as_str).collect();
            let query: Vec<(&str, &str)> = query.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            api.server_read(&path, &query, seconds).await
        });
        cx.spawn(async move |this, cx| {
            let Ok(result) = task.await else { return };
            let _ = this.update(cx, |this, cx| if this.connection == connection { done(this, result, cx) });
        }).detach();
    }

    /// As máquinas do relatório, o `listOwnServers` do web: as ligadas e sem convite. Sem lista guardada, a conexão ativa.
    pub(super) fn report_machines(&self) -> Vec<Machine> {
        let own: Vec<Machine> = self.servers.iter().filter(|s| !s.disabled && !s.invite).map(|s| Machine {
            id: s.id.clone(), key: servers::norm(&s.address),
            label: if s.label.is_empty() { servers::default_label(&s.address) } else { s.label.clone() },
        }).collect();
        if !own.is_empty() || self.active_invite() { return own; }
        self.server.as_deref().map(|a| vec![Machine { id: servers::norm(a), label: servers::default_label(a), key: servers::norm(a) }]).unwrap_or_default()
    }

    /// As que entram na soma: as marcadas, ou todas quando nenhuma ficou (relatório vazio pareceria "sem dados").
    pub(super) fn chosen_machines(&self) -> Vec<Machine> {
        let all = self.report_machines();
        let on: Vec<Machine> = all.iter().filter(|m| !self.costs.off.as_ref().is_some_and(|off| off.contains(&m.id))).cloned().collect();
        if on.is_empty() { all } else { on }
    }

    pub(super) fn ensure_costs_prefs(&mut self) {
        if self.costs.hidden.is_some() && self.costs.off.is_some() { return; }
        let (hidden, off) = load_prefs();
        self.costs.hidden.get_or_insert(hidden);
        self.costs.off.get_or_insert(off);
    }

    fn save_costs_prefs(&mut self, failed_key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_costs_prefs();
        let (Some(hidden), Some(off)) = (&self.costs.hidden, &self.costs.off) else { return };
        if let Err(error) = save_prefs(hidden, off) {
            window.push_notification(Notification::warning(tr(failed_key).replace("{reason}", &error.to_string())), cx);
        }
    }

    /// Marca ou tira uma máquina da soma; a última marcada não sai. As duas páginas releem.
    pub(super) fn toggle_machine(&mut self, id: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_costs_prefs();
        let off = self.costs.off.get_or_insert_default();
        match id { Some(id) => { if !off.remove(&id) { off.insert(id); } } None => off.clear() }
        self.save_costs_prefs("costs_machines_not_saved", window, cx);
        if self.costs.view == Some(View::Usage) {
            self.costs.report.reset();
            self.load_usage(false, cx);
        } else {
            self.usage_stale();
            self.load_costs(false, cx);
        }
    }

    /// O botão "N de M" que abre a escolha de máquinas; nenhum com uma máquina só.
    pub(super) fn machines_button(&self, id: &'static str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let all = self.report_machines();
        if all.len() < 2 { return None; }
        let on = self.chosen_machines().len();
        Some(Button::new(id).outline().small().selected(self.costs.show_machines)
            .label(web_with("custos_de_servidores", &[("n", on.to_string()), ("m", all.len().to_string())]))
            .on_click(cx.listener(|this, _, _, cx| { this.costs.show_machines = !this.costs.show_machines; cx.notify(); }))
            .into_any_element())
    }

    /// Uma pílula por máquina: acesa entra na soma.
    pub(super) fn machine_chips(&self, cx: &mut Context<Self>) -> Option<Div> {
        let all = self.report_machines();
        if !self.costs.show_machines || all.len() < 2 { return None; }
        // Nenhuma marcada vale como todas (`chosen_machines`): as pílulas mostram o mesmo.
        let mut off = self.costs.off.clone().unwrap_or_default();
        if all.iter().all(|m| off.contains(&m.id)) { off.clear(); }
        let on_count = all.iter().filter(|m| !off.contains(&m.id)).count();
        let chips = all.iter().map(|m| {
            let (on, id) = (!off.contains(&m.id), m.id.clone());
            Button::new(SharedString::from(format!("costs-machine-{id}"))).outline().small().selected(on).disabled(on && on_count == 1)
                .label(m.label.clone())
                .on_click(cx.listener(move |this, _, window, cx| this.toggle_machine(Some(id.clone()), window, cx)))
        }).collect::<Vec<_>>();
        Some(div().flex().flex_wrap().items_center().gap(px(6.)).child(div().text_size(px(12.)).text_color(theme::muted()).child(web("custos_servidores_relatorio")))
            .children(chips)
            .when(on_count < all.len(), |el| el.child(Button::new("costs-machine-all").ghost().small().label(web("custos_todos"))
                .on_click(cx.listener(|this, _, window, cx| this.toggle_machine(None, window, cx))))))
    }

    /// Lê `path` de uma máquina, repetindo a cada 3 s enquanto ela ainda lê o histórico (até ~5 min e só com a página aberta).
    /// Resposta de outra conexão ou de pedido já trocado é descartada.
    pub(super) fn read_machine(&mut self, m: Machine, path: &'static str, query: Vec<(String, String)>, tries: u32, read: MachineRead,
        cx: &mut Context<Self>) {
        let Some(api) = self.machine_api(&m.key) else {
            let error = self.machine_error(&m.key);
            (read.done)(self, read.seq, &m, Err(error), cx);
            return;
        };
        let (connection, sent) = (self.connection, query.clone());
        let task = self.runtime.spawn(async move {
            let sent: Vec<(&str, &str)> = sent.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            api.server_read(&[path], &sent, 120).await
        });
        cx.spawn(async move |this, cx| {
            // Tarefa que morreu ainda conta como resposta: sem ela a página ficaria carregando para sempre.
            let result = match task.await { Ok(result) => result.map_err(|e| Self::failure(&e)), Err(_) => Err(tr("connection_failed")) };
            let _ = this.update(cx, |this, cx| {
                if this.connection != connection || !(read.alive)(this, read.seq) { return; }
                match result {
                    Ok(v) if v.get("aquecendo") == Some(&Value::Bool(true)) => {
                        if tries >= WARM_TRIES || this.costs.view.is_none() {
                            (read.warm)(this, &m, None);
                            let error = web_with("custos_aquecendo", &[("maquina", m.label.clone())]);
                            (read.done)(this, read.seq, &m, Err(error), cx);
                        } else {
                            let count = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0);
                            (read.warm)(this, &m, Some((count("lidos"), count("total"))));
                            cx.spawn(async move |this, cx| {
                                cx.background_executor().timer(WARM_EVERY).await;
                                // A troca de servidor zera os números de pedido: só o `connection` separa esta espera da leitura nova.
                                let _ = this.update(cx, |this, cx| if this.connection == connection && (read.alive)(this, read.seq) {
                                    this.read_machine(m, path, query, tries + 1, read, cx)
                                });
                            }).detach();
                        }
                    }
                    Ok(v) => { (read.warm)(this, &m, None); (read.done)(this, read.seq, &m, Ok(v), cx); }
                    Err(error) => { (read.warm)(this, &m, None); (read.done)(this, read.seq, &m, Err(error), cx); }
                }
                cx.notify();
            });
        }).detach();
    }

    /// Abre a página de custos (atalho ou botão); chamada de novo com ela aberta, fecha.
    pub(super) fn toggle_costs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.costs.view.is_some() { self.close_costs(window, cx) } else { self.open_costs(window, cx) }
    }

    pub(super) fn open_costs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection_dialog { return; }
        self.close_settings(window, cx);
        self.close_worktrees(window, cx);
        self.close_popups();
        self.command_panel = false;
        self.show_costs_view(View::Costs, window, cx);
    }

    /// Troca entre Custos e Estatísticas de uso sem sair da página.
    pub(super) fn show_costs_view(&mut self, view: View, window: &mut Window, cx: &mut Context<Self>) {
        self.costs.view = Some(view);
        self.costs.scroll.set_offset(point(px(0.), px(0.)));
        self.root_focus.focus(window, cx);
        match view {
            View::Costs => if self.costs.report.value.is_none() && !self.costs.report.loading { self.load_costs(false, cx) },
            View::Usage => self.usage_stats_opened(cx),
        }
        cx.notify();
    }

    pub(super) fn close_costs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.costs.view.take().is_some() {
            self.root_focus.focus(window, cx);
            cx.notify();
        }
    }

    /// Esc: das Estatísticas volta a Custos; de Custos, à conversa.
    pub(super) fn costs_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        match self.costs.view {
            Some(View::Usage) => { self.show_costs_view(View::Costs, window, cx); true }
            Some(View::Costs) => { self.close_costs(window, cx); true }
            None => false,
        }
    }

    /// Page Up/Down, Home e End rolam a página aberta sem o ponteiro, como numa página de documento.
    pub(super) fn costs_page_key(&mut self, event: &KeyDownEvent, window: &mut Window) -> bool {
        let Some(view) = self.costs.view else { return false };
        let m = &event.keystroke.modifiers;
        if m.control || m.alt || m.platform || m.shift || self.search.open { return false; }
        let scroll = match view { View::Costs => &self.costs.scroll, View::Usage => &self.usage_stats.scroll };
        let (offset, max) = (scroll.offset(), scroll.max_offset());
        let page = (window.viewport_size().height - px(140.)).max(px(120.));
        let y = match event.keystroke.key.as_str() {
            "pagedown" => offset.y - page, "pageup" => offset.y + page, "home" => px(0.), "end" => -max.y,
            _ => return false,
        };
        scroll.set_offset(point(offset.x, y.clamp(-max.y, px(0.))));
        window.refresh();
        true
    }

    /// Outra conexão: as conexões das máquinas mudaram. Com a página aberta, relê.
    pub(super) fn costs_reconnected(&mut self, cx: &mut Context<Self>) {
        let (view, hidden, off) = (self.costs.view, self.costs.hidden.take(), self.costs.off.take());
        self.costs = Costs { view, hidden, off, ..Default::default() };
        self.usage_stats = Default::default();
        match view {
            Some(View::Costs) => self.load_costs(false, cx),
            Some(View::Usage) => self.usage_stats_opened(cx),
            None => {}
        }
    }

    fn load_costs(&mut self, fresh: bool, cx: &mut Context<Self>) {
        self.ensure_costs_prefs();
        let seq = self.costs.report.start();
        let machines = self.chosen_machines();
        (self.costs.parts, self.costs.pending, self.costs.warming, self.costs.partial) = (Vec::new(), machines.len(), Vec::new(), Partial::default());
        if machines.is_empty() { self.costs.report.finish(seq, Err(tr("connection_failed"))); }
        let mut query = vec![("period".to_owned(), self.costs.period.key().to_owned())];
        if fresh { query.push(("fresco".into(), "true".into())); }
        let read = MachineRead { seq, alive: |this, seq| this.costs.report.seq == seq,
            warm: |this, m, progress| set_warming(&mut this.costs.warming, m, progress), done: Self::costs_part };
        for m in machines { self.read_machine(m, "costs", query.clone(), 0, read, cx); }
        // "Atualizar" e "Tentar de novo" releem as áreas também: a chave delas não muda numa nova tentativa.
        if fresh { self.costs.areas_key.clear(); }
        self.refresh_areas(cx);
        cx.notify();
    }

    /// Uma máquina respondeu: a tela passa a mostrar a soma de quem já respondeu, sem esperar a mais lenta.
    fn costs_part(&mut self, seq: u64, m: &Machine, result: Result<Value, String>, cx: &mut Context<Self>) {
        if seq != self.costs.report.seq { return; }
        let period = self.costs.period.key();
        let part = match result.and_then(|v| Report::parse(&v).map(|r| (v.pointer("/applied/period").and_then(Value::as_str) == Some(period), r))) {
            Ok((true, r)) => Part::Ok(r),
            Ok((false, r)) => Part::Mismatched(r.usd_brl),
            Err(error) => Part::Failed(error),
        };
        let c = &mut self.costs;
        c.parts.push(MachinePart { id: m.id.clone(), label: m.label.clone(), part });
        c.pending = c.pending.saturating_sub(1);
        c.partial = Partial::of(&c.parts);
        let done = c.pending == 0;
        if done || c.parts.iter().any(MachinePart::answered) {
            c.report.value = Some(all_failed(&c.parts).map_or_else(|| Ok(merge_costs(&c.parts)), Err));
        }
        c.report.loading = !done;
        // Com máquina fora do ar, o recorte dela não some: os projetos dela só não chegaram.
        if done && c.partial.failed.is_empty() { self.prune_filter(); }
        if done { self.refresh_areas(cx); }
        cx.notify();
    }

    /// O recorte só vale para chaves que ainda existem no período carregado: trocar de 30 para 7 dias pode apagar o projeto.
    fn prune_filter(&mut self) {
        let Some(report) = self.costs.report.ok() else { return };
        let has_combos = !report.combos.is_empty();
        let exists = |dim: Dim, key: &str| if has_combos { report.combos.iter().any(|c| c.field(dim) == key) }
            else { report.list(dim).iter().any(|b| b.key == key) };
        let mut filter = self.costs.filter.clone();
        for dim in Dim::ALL {
            let kept: Vec<String> = filter.get(dim).iter().filter(|k| exists(dim, k)).cloned().collect();
            if kept.is_empty() { filter.lists.remove(&dim); } else { filter.lists.insert(dim, kept); }
        }
        if !has_combos { filter.sub = None; }
        self.costs.filter = filter;
        let dim = self.compare_dim(report);
        self.costs.compared.retain(|k| exists(dim, k));
    }

    fn compare_dim(&self, _report: &Report) -> Dim {
        match self.costs.compare_dim {
            Some(Dim::Machine) if self.report_machines().len() < 2 => Dim::Provider,
            dim => dim.unwrap_or(Dim::Provider),
        }
    }

    /// Áreas do código: o relatório de uso só recorta por projeto e modelo, e é pedido de novo quando esses mudam.
    fn refresh_areas(&mut self, cx: &mut Context<Self>) {
        let machines = self.chosen_machines();
        let (projects, models) = (self.costs.filter.get(Dim::Project).to_vec(), self.costs.filter.get(Dim::Model).to_vec());
        let ids: Vec<&str> = machines.iter().map(|m| m.id.as_str()).collect();
        let key = format!("{}|{}|{}|{}", self.costs.period.key(), projects.join("\n"), models.join("\n"), ids.join("\n"));
        if key == self.costs.areas_key && (self.costs.areas.loading || self.costs.areas.value.is_some()) { return; }
        self.costs.areas_key = key;
        let seq = self.costs.areas.start();
        (self.costs.area_parts, self.costs.area_pending) = (Vec::new(), machines.len());
        if machines.is_empty() { self.costs.areas.finish(seq, Err(web("custos_areas_erro"))); }
        let mut query = vec![("period".to_owned(), self.costs.period.key().to_owned())];
        query.extend(projects.into_iter().map(|p| ("projeto".to_owned(), p)));
        query.extend(models.into_iter().map(|m| ("modelo".to_owned(), m)));
        let read = MachineRead { seq, alive: |this, seq| this.costs.areas.seq == seq, warm: |_, _, _| {}, done: Self::areas_part };
        for m in machines { self.read_machine(m, "uso", query.clone(), 0, read, cx); }
    }

    /// Áreas só saem quando todas as máquinas responderam; erro só se nenhuma respondeu.
    fn areas_part(&mut self, seq: u64, _: &Machine, result: Result<Value, String>, cx: &mut Context<Self>) {
        if seq != self.costs.areas.seq { return; }
        let period = self.costs.period.key();
        let part = result.ok().and_then(|v| match v.pointer("/applied/period").and_then(Value::as_str) == Some(period) {
            // Fora do período: respondeu, mas não soma.
            false => Some(Vec::new()),
            true => v.get("by_area").map_or(Some(Vec::new()), |list| serde_json::from_value::<Vec<Area>>(list.clone()).ok()),
        });
        let c = &mut self.costs;
        c.area_parts.push(part);
        c.area_pending = c.area_pending.saturating_sub(1);
        if c.area_pending == 0 {
            let value = if c.area_parts.iter().all(Option::is_none) { Err(web("custos_areas_erro")) } else { Ok(merge_areas(&c.area_parts)) };
            c.areas.finish(seq, value);
        }
        cx.notify();
    }

    fn set_period(&mut self, period: Period, cx: &mut Context<Self>) {
        if self.costs.period == period && self.costs.report.value.is_some() { return; }
        self.costs.period = period;
        self.costs.hover_day = None;
        self.load_costs(false, cx);
    }

    fn set_cut(&mut self, dim: Dim, values: Vec<String>, cx: &mut Context<Self>) {
        let cross = self.costs.report.ok().is_some_and(|r| !r.combos.is_empty());
        self.costs.filter.set(dim, values, cross);
        self.refresh_areas(cx);
        cx.notify();
    }

    /// Clique numa linha, aba ou item: marca ou desmarca a chave na lista da dimensão.
    fn toggle_cut(&mut self, dim: Dim, key: String, cx: &mut Context<Self>) {
        let mut values = self.costs.filter.get(dim).to_vec();
        if let Some(at) = values.iter().position(|k| *k == key) { values.remove(at); } else { values.push(key); }
        self.set_cut(dim, values, cx);
    }

    fn clear_cut(&mut self, layers_too: bool, cx: &mut Context<Self>) {
        self.costs.filter = Filter::default();
        if layers_too { self.costs.layers_mode = ""; }
        self.refresh_areas(cx);
        cx.notify();
    }

    fn hidden_projects(&mut self) -> &mut HashSet<String> {
        self.ensure_costs_prefs();
        self.costs.hidden.get_or_insert_default()
    }

    fn set_hidden(&mut self, key: Option<String>, hide: bool, window: &mut Window, cx: &mut Context<Self>) {
        let hidden = self.hidden_projects();
        match key { Some(key) => { if hide { hidden.insert(key); } else { hidden.remove(&key); } } None => hidden.clear() }
        self.save_costs_prefs("costs_hidden_not_saved", window, cx);
        cx.notify();
    }

    // ── Desenho ─────────────────────────────────────────────────────────────

    pub(super) fn render_costs(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        match self.costs.view {
            Some(View::Usage) => self.render_usage_stats(window, cx),
            _ => self.render_costs_page(window, cx),
        }
    }

    /// Cabeçalho das duas páginas: voltar, título e as ações dele à direita.
    pub(super) fn costs_header(&self, title: String, back: impl Fn(&mut Hangar, &mut Window, &mut Context<Hangar>) + 'static,
        actions: Vec<AnyElement>, cx: &mut Context<Self>) -> Div {
        div().h(px(52.)).flex_shrink_0().px(px(16.)).flex().items_center().gap(px(10.)).border_b_1().border_color(theme::border())
            .child(Button::new("costs-back").ghost().small().icon(IconName::ArrowLeft).accessibility_label(tr("costs_back"))
                .tooltip(tr("costs_back"))
                .on_click(cx.listener(move |this, _, window, cx| back(this, window, cx))))
            .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title))
            .child(div().flex_1())
            .children(actions)
            .child(chrome::kbd("Esc"))
    }

    fn render_costs_page(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let loading = self.costs.report.loading;
        let header = self.costs_header(web("nav_custos"), |this, window, cx| this.close_costs(window, cx), vec![
            Button::new("costs-usage").ghost().small().label(web("custos_ir_uso")).icon(IconName::ChartPie)
                .on_click(cx.listener(|this, _, window, cx| this.show_costs_view(View::Usage, window, cx))).into_any_element(),
            Button::new("costs-refresh").outline().small().label(web("custos_atualizar")).loading(loading).disabled(loading)
                .on_click(cx.listener(|this, _, _, cx| this.load_costs(true, cx))).into_any_element(),
        ], cx);
        let body = self.render_costs_body(window, cx);
        let scroll = div().id("costs-scroll").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.costs.scroll)
            .child(div().w_full().flex().justify_center()
                .child(div().w_full().max_w(px(1120.)).px(px(24.)).pt(px(20.)).pb(px(48.)).flex().flex_col().gap(px(16.)).child(body)));
        page_frame(div().size_full().flex().flex_col().child(header).child(scroll))
    }

    fn render_costs_body(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let rate = self.costs.report.ok().and_then(|r| r.usd_brl);
        let intro = div().flex().flex_col().gap(px(4.))
            .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD).child(web("custos_consumo_estimativa")))
            .child(div().max_w(px(640.)).text_size(px(13.5)).text_color(theme::muted()).whitespace_normal().child(web("custos_aviso_estimativa")));
        let period = segments("costs-period", &Period::labels(), self.costs.period.index(), 5, false, String::new(),
            |this: &mut Hangar, n, _: &mut Window, cx| this.set_period(Period::ALL[n], cx), cx);
        let brl = appearance::get().currency == Currency::Brl && rate.is_some();
        let currency = segments("costs-currency", &["US$".into(), "R$".into()], brl as usize, if rate.is_some() { 2 } else { 1 }, false,
            web("custos_cotacao_indisponivel"),
            |this: &mut Hangar, n, _: &mut Window, cx| {
                let mut next = appearance::get();
                next.currency = if n == 1 { Currency::Brl } else { Currency::Usd };
                this.apply_appearance(next, true, cx);
            }, cx);
        let toolbar = div().flex().flex_wrap().items_center().gap(px(8.)).child(period).child(currency);
        let mut page = div().flex().flex_col().gap(px(16.)).child(intro).child(toolbar);
        let warming: Vec<Div> = self.costs.warming.iter().map(warming_note).collect();

        let report = match &self.costs.report.value {
            None => return page.children(warming).child(loading_state()),
            Some(Err(error)) => return page.children(warming).child(error_state(error.clone(), cx.listener(|this, _, _, cx| this.load_costs(true, cx)))),
            Some(Ok(report)) => report.clone(),
        };
        let multi = self.report_machines().len() > 1;
        page = page.child(self.render_filters(&report, multi, cx)).children(self.machine_chips(cx));
        if !self.costs.partial.is_empty() {
            page = page.child(partial_note("costs-partial-retry", &self.costs.partial,cx.listener(|this, _, _, cx| this.load_costs(true, cx))));
        }
        let d = self.derive(&report);
        if d.filtering { page = page.child(self.render_cut_line(&report, &d, cx)); }
        page = page.children(warming);
        if self.costs.pending > 0 && multi {
            page = page.child(hint_text(web_with("custos_carregando_maquinas", &[("n", self.costs.pending.to_string())])));
        }
        if report.totals.sessions == 0. {
            return page.child(empty_state(web("custos_sem_dados_periodo")));
        }
        page.child(self.render_source_tabs(&report, &d, cx))
            .children(self.unknown_note(&report, &d))
            .child(self.render_kpis(&report, &d, rate))
            .child(self.render_daily(&report, &d, rate, window, cx))
            .child(two_cols(self.render_dollar(&d, rate), self.render_cache(&d, rate)))
            .child(two_cols(self.render_rank(&report, &d, Dim::Provider, rate, cx), self.render_rank(&report, &d, Dim::Source, rate, cx)))
            .children(multi.then(|| self.render_costs_by_machine(&report, rate, cx)))
            .child(self.render_areas(&d, rate))
            .child(self.render_projects(&report, &d, rate, cx))
            .child(self.render_sessions(&report, &d, rate, cx))
            .child(self.render_models(&report, &d, rate, cx))
            .child(self.render_compare(&report, &d, rate, multi, cx))
            .child(self.render_method(&report, &d, rate, cx))
    }

    fn derive(&self, report: &Report) -> Derived {
        let filter = &self.costs.filter;
        let has_combos = !report.combos.is_empty();
        let filtering = filter.active();
        let cut = filtered(&report.combos, filter);
        let focus = if has_combos { sum(&cut) } else {
            Dim::ALL.iter().find(|d| !filter.get(**d).is_empty())
                .and_then(|d| report.list(*d).iter().find(|b| b.key == filter.get(*d)[0]).cloned())
                .unwrap_or_else(|| report.totals.clone())
        };
        let sub_cost = if has_combos && filter.sub.is_none() { sum(&cut.iter().copied().filter(|c| c.subagente).collect::<Vec<_>>()).cost } else { 0. };
        let free_only = has_combos && !cut.is_empty() && cut.iter().all(|c| is_free(&c.model));
        let rates = report.rates_by_model();
        let (without_cache, equivalent) = if has_combos && filtering { (without_cache(&cut, &rates), equivalent(&cut, &rates)) }
            else { (report.custo_sem_cache, report.equivalente_cobrado) };
        Derived {
            unknown: focus.unknown() && !free_only,
            empty_cut: has_combos && filtering && cut.is_empty(),
            cache_source: if has_combos { focus.clone() } else { report.totals.clone() },
            has_combos, filtering, focus, sub_cost, free_only, without_cache, equivalent, rates,
        }
    }

    /// A lista de uma dimensão já no recorte completo (com o filtro dela): é o que os painéis mostram.
    fn list_of(&self, report: &Report, dim: Dim) -> Vec<Bucket> {
        if report.combos.is_empty() { report.list(dim).to_vec() } else { group(&filtered(&report.combos, &self.costs.filter), dim) }
    }

    /// O que o seletor de uma dimensão oferece: cruzado com os outros filtros, nunca com o próprio, e o que está
    /// marcado continua na lista mesmo sem sobrar nada dele.
    fn options_of(&self, report: &Report, dim: Dim) -> Vec<Bucket> {
        // Máquina sai do total de cada uma, não do cruzamento: a que respondeu sem detalhamento ficaria impossível de escolher.
        if report.combos.is_empty() || dim == Dim::Machine { return report.list(dim).to_vec(); }
        let mut list = group(&filtered(&report.combos, &self.costs.filter.without(dim)), dim);
        for key in self.costs.filter.get(dim) {
            if !list.iter().any(|b| &b.key == key) { list.push(Bucket::zero(key)); }
        }
        list
    }

    fn provider_name(report: &Report, key: &str) -> String {
        let name = report.by_provider.iter().find(|b| b.key == key).and_then(|b| b.label.clone()).unwrap_or_else(|| key.to_owned());
        let cli = if key.starts_with("anthropic:") { source_name("claude") } else if key.starts_with("codex:") { source_name("codex") } else { String::new() };
        if !cli.is_empty() && !name.starts_with(&format!("{cli} · ")) { format!("{cli} · {name}") } else { name }
    }

    fn name_of(report: &Report, dim: Dim, key: &str) -> String {
        match dim { Dim::Provider => Self::provider_name(report, key), Dim::Project => project_label(key), Dim::Source => source_name(key),
            Dim::Model => key.to_owned(),
            Dim::Machine => report.by_machine.iter().find(|b| b.key == key).and_then(|b| b.label.clone()).unwrap_or_else(|| key.to_owned()) }
    }

    /// "N sessões · M subagentes", seguindo os três estados do filtro de subagente.
    fn sessions_text(&self, has_combos: bool, b: &Bucket) -> String {
        let (n, word) = (b.sessions, |n: f64, one: &str, many: &str| format!("{} {}", dec(n, 0), web(if n == 1. { one } else { many })));
        match (has_combos, self.costs.filter.sub) {
            (true, Some(false)) => word(n, "sessao_singular", "lista_sessoes_plural"),
            (true, Some(true)) => word(n, "custos_subagente", "custos_subagentes"),
            (true, None) if b.subagentes > 0. => format!("{} · {}", word(n - b.subagentes, "sessao_singular", "lista_sessoes_plural"),
                word(b.subagentes, "custos_subagente", "custos_subagentes")),
            _ => word(n, "custos_sessao_ou_subagente", "custos_sessoes_e_subagentes"),
        }
    }

    fn render_filters(&self, report: &Report, multi: bool, cx: &mut Context<Self>) -> Div {
        let has_combos = !report.combos.is_empty();
        let rate = report.usd_brl;
        let mut row = div().flex().flex_wrap().items_end().gap(px(12.));
        for dim in Dim::shown(multi) {
            let options = self.options_of(report, dim);
            let selected = self.costs.filter.get(dim).to_vec();
            let all = web_with(if matches!(dim, Dim::Source | Dim::Machine) { "custos_todas_n" } else { "custos_todos_n" }, &[("n", options.len().to_string())]);
            let shown = match selected.len() {
                0 => all.clone(),
                1 => Self::name_of(report, dim, &selected[0]),
                n => web_with("uso_n_de_m", &[("n", n.to_string()), ("m", options.len().to_string())]),
            };
            let items: Vec<(String, String, String, bool)> = options.iter().map(|b| {
                let hint = if dim == Dim::Model && !report.rates.iter().any(|r| r.model == b.key) {
                    web(if is_free(&b.key) { "custos_gratis" } else { "custos_sem_tarifa" })
                } else if b.unknown() { "—".into() } else { money(b.cost, rate) };
                (b.key.clone(), Self::name_of(report, dim, &b.key), hint, selected.contains(&b.key))
            }).collect();
            let this = cx.entity().downgrade();
            let button = Button::new(SharedString::from(format!("costs-filter-{}", dim.id()))).outline().small().w(px(230.))
                .child(div().w_full().flex().items_center().gap(px(6.))
                    .child(div().flex_1().min_w_0().truncate().text_left().child(shown))
                    .child(chrome::small_icon(IconName::ChevronDown, 14., theme::muted())))
                .accessibility_label(dim.name())
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let pick = |values: Option<String>| {
                        let this = this.clone();
                        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                            let _ = this.update(cx, |this, cx| match values.clone() {
                                Some(key) => this.toggle_cut(dim, key, cx),
                                None => this.set_cut(dim, Vec::new(), cx),
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
            row = row.child(field(dim.name(), button.into_any_element()));
        }
        if let Some(button) = self.machines_button("costs-machines", cx) { row = row.child(field(web("custos_servidores"), button)); }
        if has_combos {
            let sub = segments("costs-sub", &[web("custos_tudo"), web("custos_so_conversa"), web("custos_so_subagente")],
                match self.costs.filter.sub { None => 0, Some(false) => 1, Some(true) => 2 }, 3, false, String::new(),
                |this: &mut Hangar, n, _: &mut Window, cx| { this.costs.filter.sub = [None, Some(false), Some(true)][n]; cx.notify(); }, cx);
            row = row.child(field(web("custos_subagente"), sub));
        }
        let default_layers = self.costs.layers_off == default_layers(self.costs.layers_mode);
        row = row.child(Button::new("costs-clear").ghost().small().label(web("custos_limpar_filtros"))
            .disabled(!self.costs.filter.active() && default_layers)
            .on_click(cx.listener(|this, _, _, cx| this.clear_cut(true, cx))));
        card(web("custos_filtros"), None).child(row)
    }

    fn cut_description(&self, report: &Report) -> String {
        let filter = &self.costs.filter;
        Dim::ALL.iter().filter(|d| !filter.get(**d).is_empty())
            .map(|d| format!("{} {}", d.name(), filter.get(*d).iter().map(|k| Self::name_of(report, *d, k)).collect::<Vec<_>>().join(", ")))
            .chain(filter.sub.map(|s| web(if s { "custos_so_subagente" } else { "custos_so_conversa" })))
            .collect::<Vec<_>>().join(" · ")
    }

    fn render_cut_line(&self, report: &Report, d: &Derived, cx: &mut Context<Self>) -> Div {
        div().flex().flex_wrap().items_center().gap(px(6.)).text_size(px(13.)).text_color(theme::muted())
            .child(web("custos_recorte_inicio"))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(self.cut_description(report)))
            .child(format!("{}{}", web("custos_recorte_fim"), web(if d.has_combos { "custos_recorte_inteiro" } else { "custos_recorte_parcial" })))
            .child(Button::new("costs-cut-clear").ghost().xsmall().label(web("custos_tirar_recorte"))
                .on_click(cx.listener(|this, _, _, cx| this.clear_cut(false, cx))))
    }

    fn render_source_tabs(&self, report: &Report, _d: &Derived, cx: &mut Context<Self>) -> Div {
        let selected = self.costs.filter.get(Dim::Source).to_vec();
        let chip = |id: String, on: bool, label: Div, cx: &mut Context<Self>| Button::new(SharedString::from(id))
            .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { theme::boxed() })
                .foreground(if on { theme::text() } else { theme::muted() }).hover(theme::hover()).active(theme::hover()))
            .h(px(34.)).px(px(12.)).rounded(px(8.)).border_1().border_color(if on { theme::accent().alpha(0.5) } else { theme::border() })
            .when(on, |el| el.bg(theme::accent_dim())).child(label);
        div().flex().flex_wrap().gap(px(8.))
            .child(chip("costs-src-all".into(), selected.is_empty(), div().child(web("custos_todas_fontes")), cx)
                .on_click(cx.listener(|this, _, _, cx| this.set_cut(Dim::Source, Vec::new(), cx))))
            .children(self.options_of(report, Dim::Source).into_iter().map(|b| {
                let key = b.key.clone();
                chip(format!("costs-src-{key}"), selected.contains(&key), div().flex().items_center().gap(px(8.))
                        .child(swatch(source_color(&key)))
                        .child(source_name(&key))
                        .child(div().text_color(theme::faint()).child(tok(b.fresh()))), cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_cut(Dim::Source, key.clone(), cx)))
            }))
    }

    fn unknown_models(&self, report: &Report) -> Vec<String> {
        let cut = filtered(&report.combos, &self.costs.filter);
        report.sem_tarifa.iter().filter(|m| !is_free(m)).filter(|m| report.combos.is_empty() || cut.iter().any(|c| &c.model == *m)).cloned().collect()
    }

    fn unknown_note(&self, report: &Report, _d: &Derived) -> Option<Div> {
        let unknown = self.unknown_models(report);
        (!unknown.is_empty()).then(|| note_box(web_with("custos_estimativa_incompleta", &[("n", unknown.len().to_string())]), theme::warning()))
    }

    fn render_kpis(&self, report: &Report, d: &Derived, rate: Option<f64>) -> Div {
        let f = &d.focus;
        let dash = d.unknown || d.empty_cut;
        let m1 = |v: f64| if dash { "—".to_owned() } else { money(v, rate) };
        let m2 = |v: f64| if dash { "—".to_owned() } else { money2(v, rate) };
        let days = if self.costs.period.days() > 0. { self.costs.period.days() } else { report.by_day.len().max(1) as f64 };
        let mut cost_foot = vec![format!("{} · {} · {}{}", m2(f.cost), self.sessions_text(d.has_combos, f), m2(f.cost / days), web("custos_por_dia"))];
        if d.unknown { cost_foot.push(web("custos_sem_tarifa_volume")); }
        else if d.free_only { cost_foot.push(web("custos_modelo_gratis_nada")); }
        else if d.empty_cut { cost_foot.push(web("custos_recorte_vazio")); }
        if d.sub_cost > 0. && f.cost > 0. { cost_foot.push(web_with("custos_veio_subagente", &[("pct", pct(d.sub_cost, f.cost))])); }
        if let Some(prev) = report.anterior.as_ref().filter(|p| !d.filtering && p.cost > 0.) {
            let delta = (report.totals.cost - prev.cost) / prev.cost * 100.;
            cost_foot.push(if delta.abs() < 1. { web("custos_delta_igual") } else {
                format!("{} {}", if delta > 0. { "▲" } else { "▼" }, web_with("custos_delta_vs", &[("n", dec(delta.abs(), 0)), ("rot", self.costs.period.label())]))
            });
        }
        // Máquina desmarcada sai da conta: sem esta linha, "parte do gasto" passaria por "o gasto".
        let (chosen, all) = (self.chosen_machines().len(), self.report_machines().len());
        if chosen < all { cost_foot.push(web_with("custos_somando_maquinas", &[("n", chosen.to_string()), ("m", all.to_string())])); }
        let raw = f.raw();
        let entry = f.input + f.cache_read + f.cache_write;
        let saved = d.without_cache - f.cost;
        let economy = if d.filtering && !d.has_combos { ("—".to_owned(), web("custos_so_total_periodo")) }
            else if d.empty_cut { ("—".to_owned(), web("custos_sem_dados_recorte_short")) }
            else { (m1(saved), format!("{} · {}", m2(saved), web_with("custos_abaixo_preco_cheio", &[("n",
                if d.without_cache > 0. { dec(100. - f.cost / d.without_cache * 100., 0) } else { "0".into() })]))) };
        let kpis = vec![
            kpi(web("custos_custo_periodo"), m1(f.cost), cost_foot, false, dash),
            kpi(web("custos_tokens_novos"), tok(f.fresh()), vec![web("custos_tokens_novos_pe")], false, false),
            kpi(web("custos_tokens_processados"), tok(raw), vec![web_with("custos_tokens_processados_pe", &[("relidos", tok(f.cache_read)), ("pct", pct(f.cache_read, raw))])], true, false),
            kpi(web("custos_cache_na_entrada"), pct(f.cache_read, entry),
                if f.cache_write > 0. { vec![web_with("custos_cada_gravado_lido", &[("x", dec(f.cache_read / f.cache_write, 1))])] } else { vec![] }, false, false),
            kpi(web("custos_economia_cache"), economy.0, vec![economy.1], false, false),
            kpi(web("custos_custo_por_milhao"), if raw > 0. { m1(f.cost / raw * 1e6) } else { "—".into() },
                if f.fresh() > 0. { vec![web_with("custos_por_milhao_novos", &[("valor", m1(f.cost / f.fresh() * 1e6))])] } else { vec![] }, false, false),
            if report.cache_detailed { kpi(web("custos_cache_perdido"), m1(f.custo_regravado), vec![web_with("custos_cache_perdido_pe", &[("tokens", tok(f.regravado))])], false, false) }
                else { kpi(web("custos_cache_perdido"), "—".into(), vec![web("custos_cache_sem_dado")], false, false) },
            kpi(web("custos_cache_1h"), if report.cache_detailed { pct(f.cache_write_1h, f.cache_write) } else { "—".into() },
                if !report.cache_detailed { vec![web("custos_cache_sem_dado")] }
                else if f.cache_write > 0. { vec![web_with("custos_cache_1h_pe", &[("h1", tok(f.cache_write_1h)), ("total", tok(f.cache_write))])] } else { vec![] }, false, false),
        ];
        let mut rows = div().flex().flex_col().gap(px(12.));
        let mut kpis = kpis.into_iter();
        loop {
            let chunk: Vec<Div> = kpis.by_ref().take(4).collect();
            if chunk.is_empty() { break; }
            rows = rows.child(div().flex().gap(px(12.)).children(chunk));
        }
        rows
    }

    /// Camadas do gráfico diário: por tipo de token (tokens, ou sem detalhamento) ou por fonte (custo com detalhamento).
    fn daily_layers(&self, report: &Report, d: &Derived) -> (bool, Vec<(String, String, Hsla)>) {
        let by_kind = !d.has_combos || self.costs.daily == Metric::Tokens;
        let layers = if by_kind { Kind::ALL.iter().map(|k| (k.id().to_owned(), k.label(), k.color())).collect() }
            else { group(&filtered(&report.combos, &self.costs.filter), Dim::Source).into_iter().map(|b| (b.key.clone(), source_name(&b.key), source_color(&b.key))).collect() };
        (by_kind, layers)
    }

    /// Série diária: um balde por dia, com o valor de cada camada; dia parado entra zerado.
    fn daily_series(&self, report: &Report, d: &Derived, by_kind: bool) -> Vec<(String, Bucket, HashMap<String, f64>)> {
        let metric = self.costs.daily;
        let per_kind = |b: &Bucket| Kind::ALL.iter().map(|k| (k.id().to_owned(), if metric == Metric::Tokens { b.tokens_of(*k) } else { b.cost_of(*k) })).collect();
        let mut days: BTreeMap<String, (Bucket, HashMap<String, f64>)> = BTreeMap::new();
        if d.has_combos {
            let cut = filtered(&report.combos, &self.costs.filter);
            let mut by_day: BTreeMap<&str, Vec<&Combo>> = BTreeMap::new();
            for c in cut { by_day.entry(c.dia.as_str()).or_default().push(c); }
            for (day, rows) in by_day {
                let bucket = Bucket { key: day.to_owned(), ..sum(&rows) };
                let values = if by_kind { per_kind(&bucket) } else { group(&rows, Dim::Source).into_iter().map(|b| (b.key.clone(), b.value(metric))).collect() };
                days.insert(day.to_owned(), (bucket, values));
            }
        } else {
            for b in &report.by_day { days.insert(b.key.clone(), (b.clone(), per_kind(b))); }
        }
        let (Some(first), Some(last)) = (days.keys().next().cloned(), days.keys().next_back().cloned()) else { return Vec::new() };
        days_between(&first, &last, 1).into_iter().map(|day| {
            let (bucket, values) = days.remove(&day).unwrap_or_else(|| (Bucket::zero(&day), HashMap::new()));
            (day, bucket, values)
        }).collect()
    }

    fn render_daily(&mut self, report: &Report, d: &Derived, rate: Option<f64>, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let (by_kind, layers) = self.daily_layers(report, d);
        let mode = if by_kind { if self.costs.daily == Metric::Tokens { "kind-tokens" } else { "kind-cost" } } else { "source" };
        // Trocou o que a legenda lista: volta ao padrão do modo (o cache lido nasce desligado em tokens).
        if self.costs.layers_mode != mode { self.costs.layers_mode = mode; self.costs.layers_off = default_layers(mode); }
        let metric = self.costs.daily;
        let fmt = move |v: f64| if metric == Metric::Tokens { tok(v) } else { money(v, rate) };
        let fmt2 = move |v: f64| if metric == Metric::Tokens { tok(v) } else { money2(v, rate) };
        let visible: Vec<(String, String, Hsla)> = layers.iter().filter(|l| !self.costs.layers_off.contains(&l.0)).cloned().collect();
        let series = self.daily_series(report, d, by_kind);
        let columns: Vec<(String, Bucket, Vec<(String, f64, Hsla)>, f64)> = series.into_iter().map(|(day, bucket, values)| {
            let segs: Vec<(String, f64, Hsla)> = visible.iter().map(|(id, label, color)| (label.clone(), values.get(id).copied().unwrap_or(0.), *color))
                .filter(|s| s.1 > 0.).collect();
            let total = segs.iter().map(|s| s.1).sum();
            (day, bucket, segs, total)
        }).collect();

        let metric_seg = segments("costs-daily-metric", &[web("ctx_tokens"), web("custos_estimativa_api")], (metric == Metric::Cost) as usize, 2, false,
            String::new(), |this: &mut Hangar, n, _: &mut Window, cx| {
                this.costs.daily = if n == 1 { Metric::Cost } else { Metric::Tokens };
                this.costs.hover_day = None;
                cx.notify();
            }, cx);
        let mut hint = web_with("custos_empilhado_por", &[("modo", web(if by_kind { "custos_tipo_token" } else { "custos_dim_fonte" }))]);
        if metric == Metric::Tokens { hint.push(' '); hint.push_str(&web("custos_cache_lido_desligado")); }
        if d.filtering && !d.has_combos { hint.push(' '); hint.push_str(&web("custos_ressalva_periodo_inteiro")); }
        let layer_count = layers.len();
        let legend = div().flex().flex_wrap().gap(px(6.)).children(layers.iter().map(|(id, label, color)| {
            let (id, on) = (id.clone(), !self.costs.layers_off.contains(id));
            Button::new(SharedString::from(format!("costs-layer-{id}"))).ghost().xsmall().selected(on)
                .child(div().flex().items_center().gap(px(6.)).when(!on, |el| el.opacity(0.45)).child(swatch(*color)).child(label.clone()))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let off = &mut this.costs.layers_off;
                    if !off.remove(&id) { off.insert(id.clone()); }
                    if off.len() == layer_count { off.clear(); }
                    cx.notify();
                }))
        }));

        let head = div().flex().items_center().gap(px(12.))
            .child(div().flex_1().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(web("custos_uso_por_dia"))).child(metric_seg);
        let mut body = card_plain().child(head)
            .child(div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(hint)).child(legend);
        if columns.is_empty() { return body.child(empty_state(web("custos_sem_dados_recorte"))); }

        // Régua "bonita": passo de 1, 2 ou 5 vezes uma potência de dez; tudo zerado não divide por zero.
        let max = columns.iter().map(|c| c.3).fold(0., f64::max);
        let max = if max > 0. { max } else { 1. };
        let nice = 10f64.powf(max.log10().floor());
        let step = if max / nice > 5. { nice * 2. } else if max / nice > 2. { nice } else { nice / 2. };
        let top = (max / step).ceil() * step;
        const H: f32 = 200.;
        let lines: Vec<f64> = (0..).map(|i| i as f64 * step).take_while(|g| *g <= top + 1e-9).collect();
        let plot_w = (f32::from(window.viewport_size().width).min(1120.) - 48. - 32. - 56.).max(280.);
        let every = (columns.len() as f32 / (plot_w / 52.).floor().max(3.)).ceil().max(1.) as usize;
        let hover = self.costs.hover_day.clone();
        let grid = div().absolute().inset_0().children(lines.iter().map(|g| {
            let y = H * (1. - (*g / top) as f32);
            div().absolute().left_0().right_0().top(px(y)).h(px(1.)).bg(if *g == 0. { theme::border_strong() } else { theme::border() })
        }));
        let axis = div().relative().w(px(56.)).h(px(H)).flex_shrink_0().children(lines.iter().map(|g| {
            let y = H * (1. - (*g / top) as f32);
            div().absolute().right(px(8.)).top(px(y - 7.)).text_size(px(10.5)).text_color(theme::faint()).child(fmt(*g))
        }));
        let n = columns.len();
        let bars = div().relative().flex_1().h(px(H)).child(grid).child(div().absolute().inset_0().flex().items_end()
            .children(columns.iter().map(|(day, _, segs, total)| {
                let key = day.clone();
                let lit = hover.as_deref() == Some(day.as_str());
                div().id(SharedString::from(format!("costs-day-{day}"))).flex_1().h_full().flex().flex_col().justify_end().items_center()
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered { this.costs.hover_day = Some(key.clone()); cx.notify(); }
                    }))
                    .child(div().w(relative(0.72)).max_w(px(26.)).min_w(px(2.)).flex().flex_col_reverse().gap(px(2.))
                        .when(hover.is_some() && !lit, |el| el.opacity(0.55))
                        .children(segs.iter().map(|(_, v, color)| div().w_full().rounded(px(2.)).bg(*color)
                            .h(px(((*v / top) as f32 * H - 2.).max(1.)))))
                        .when(*total <= 0., |el| el.h(px(0.))))
            })));
        let labels = div().flex().pl(px(56.)).children(columns.iter().enumerate().map(|(i, (day, ..))| {
            div().flex_1().flex().justify_center().text_size(px(10.5)).text_color(theme::faint())
                .child(if i % every == 0 || (n <= 14) { day_label(day) } else { String::new() })
        }));
        let caption = match hover.as_ref().and_then(|h| columns.iter().find(|c| &c.0 == h)) {
            Some((day, bucket, segs, total)) => {
                let entry = bucket.input + bucket.cache_read + bucket.cache_write;
                div().flex().flex_wrap().items_center().gap(px(10.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).text_color(theme::text()).child(day_label(day)))
                    .child(format!("{} · {}", fmt2(*total), self.sessions_text(d.has_combos, bucket)))
                    .when(entry > 0., |el| el.child(web_with("custos_dia_reuso", &[("pct", pct(bucket.cache_read, entry))])))
                    .children(segs.iter().map(|(label, v, color)| div().flex().items_center().gap(px(5.)).child(swatch(*color)).child(format!("{label} {}", fmt2(*v)))))
            }
            None => div().child(web("custos_hover_detalhe")),
        };
        body = body.child(div().id("costs-daily").flex().flex_col().gap(px(6.))
                .on_hover(cx.listener(|this, hovered: &bool, _, cx| if !*hovered && this.costs.hover_day.take().is_some() { cx.notify() }))
                .child(div().flex().child(axis).child(bars)).child(labels))
            .child(div().text_size(px(12.5)).text_color(theme::muted()).child(caption));
        body
    }

    fn render_dollar(&self, d: &Derived, rate: Option<f64>) -> Div {
        let f = &d.focus;
        let m2f = |v: f64| if d.unknown || d.empty_cut { "—".to_owned() } else { money2(v, rate) };
        let mut body = card(web("custos_para_onde_dolar"), Some(web("custos_tipo_token_conta")));
        body = if d.unknown {
            let what = if d.filtering { self.cut_description(self.costs.report.ok().unwrap_or(&Report::default())) } else { web("custos_nenhum_modelo_periodo") };
            body.child(hint_text(format!("{}{} {}", web("custos_sem_tarifa_para"), what, web("custos_sem_tarifa_aqui"))))
        } else if d.free_only { body.child(hint_text(web("custos_modelo_gratis_nenhum_dolar"))) }
        else if d.empty_cut { body.child(hint_text(web("custos_recorte_vazio"))) }
        else { body.child(stack_bar(Kind::ALL.iter().map(|k| (f.cost_of(*k), k.color())).collect())) };
        let header = table_row(vec![(String::new(), None), (web("ctx_tokens"), Some(90.)), (web("custos_custo"), Some(110.)), (web("custos_pct_conta"), Some(110.))], true);
        body.child(header).children(Kind::ALL.iter().map(|k| {
            table_row_el(div().flex().items_center().gap(px(8.)).child(swatch(k.color())).child(k.label()), vec![
                (tok(f.tokens_of(*k)), 90., false), (m2f(f.cost_of(*k)), 110., false),
                (if d.unknown { "—".into() } else { pct(f.cost_of(*k), f.cost) }, 110., true)])
        }))
    }

    fn render_cache(&self, d: &Derived, rate: Option<f64>) -> Div {
        let source = &d.cache_source;
        let dash = if d.has_combos { d.unknown || d.empty_cut } else { source.unknown() };
        let m1 = |v: f64| if dash { "—".to_owned() } else { money(v, rate) };
        let m2 = |v: f64| if dash { "—".to_owned() } else { money2(v, rate) };
        let saved = d.without_cache - source.cost;
        let reuse = if source.raw() > 0. { source.cache_read / source.raw() } else { 0. };
        let mut hint = web("custos_mesmos_tokens");
        if !d.has_combos && d.filtering { hint.push(' '); hint.push_str(&web("custos_ressalva_cache")); }
        let line = |label: String, value: String| div().flex().items_center().text_size(px(13.5))
            .child(div().flex_1().text_color(theme::muted()).child(label)).child(div().font_weight(FontWeight::SEMIBOLD).child(value));
        let fill = if d.without_cache > 0. { (source.cost / d.without_cache).clamp(0.02, 1.) as f32 } else { 0. };
        card(web("custos_o_que_cache"), Some(hint))
            .child(line(web("custos_pago_de_verdade"), m2(source.cost)))
            .child(div().h(px(10.)).rounded_full().bg(theme::inset()).child(div().h_full().rounded_full().bg(chart(1)).w(relative(fill))))
            .child(line(web("custos_se_nada_cache"), m2(d.without_cache)))
            .child(div().flex().gap(px(12.))
                .child(mini_kpi(web("custos_economizado"), m1(saved), Some(theme::success())))
                .child(mini_kpi(web("custos_do_volume_cache_lido"), format!("{}%", dec(reuse * 100., 0)), None)))
    }

    /// Ranking de uma dimensão: nome, custo e a barra com a forma do gasto (os quatro tipos), clicável para recortar.
    fn rank_rows(&self, report: &Report, dim: Dim, list: &[Bucket], rate: Option<f64>, hide: bool, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let peak = list.iter().map(|b| b.cost).fold(1., f64::max);
        list.iter().map(|b| {
            let key = b.key.clone();
            let on = self.costs.filter.has(dim, &key);
            let width = (b.cost / peak * 100.).max(1.5) as f32 / 100.;
            let row = Button::new(SharedString::from(format!("costs-rank-{}-{key}", dim.id())))
                .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() })
                    .foreground(theme::text()).hover(theme::hover()).active(theme::hover()))
                .when(on, |el| el.bg(theme::accent_dim()))
                .flex_1().min_w_0().h_auto().px(px(8.)).py(px(7.)).rounded(px(7.))
                .when(dim == Dim::Project, |el| el.tooltip(format!("{key}\n{}", web("custos_clique_recortar"))))
                .child(div().w_full().flex().flex_col().gap(px(6.))
                    .child(div().w_full().flex().items_center().gap(px(10.)).text_size(px(13.5))
                        .child(div().flex_1().min_w_0().truncate().text_left().child(Self::name_of(report, dim, &b.key)))
                        .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).child(if b.unknown() { "—".into() } else { money2(b.cost, rate) })))
                    .child(div().w(relative(width)).h(px(6.)).flex().gap(px(2.)).children(Kind::ALL.iter().filter(|k| b.cost_of(**k) > 0.)
                        .map(|k| div().h_full().rounded(px(2.)).bg(k.color()).flex_basis(px(0.)).flex_grow(b.cost_of(*k) as f32)))))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_cut(dim, key.clone(), cx)));
            let hide_key = b.key.clone();
            div().flex().items_center().gap(px(4.)).child(row)
                .when(hide, |el| el.child(Button::new(SharedString::from(format!("costs-hide-{hide_key}"))).ghost().xsmall().icon(IconName::Close)
                    .accessibility_label(web_with("custos_tirar_da_lista", &[("nome", hide_key.clone())])).tooltip(web("custos_tirar_lista_conta"))
                    .on_click(cx.listener(move |this, _, window, cx| this.set_hidden(Some(hide_key.clone()), true, window, cx)))))
                .into_any_element()
        }).collect()
    }

    fn render_rank(&self, report: &Report, d: &Derived, dim: Dim, rate: Option<f64>, cx: &mut Context<Self>) -> Div {
        let (title, hint) = match dim { Dim::Provider => ("custos_por_provedor", "custos_quem_cobra"), _ => ("custos_por_fonte", "custos_qual_agente") };
        let mut hint = web(hint);
        if d.filtering && !d.has_combos { hint.push(' '); hint.push_str(&web("custos_ressalva_periodo_inteiro")); }
        let list = self.list_of(report, dim);
        let rows = self.rank_rows(report, dim, &list, rate, false, cx);
        card(web(title), Some(hint)).child(if rows.is_empty() { empty_state(web("custos_sem_dados_no_periodo")) } else { div().flex().flex_col().gap(px(2.)).children(rows) })
    }

    /// Total de cada máquina no período inteiro, clicável para recortar o resto da tela.
    fn render_costs_by_machine(&self, report: &Report, rate: Option<f64>, cx: &mut Context<Self>) -> Div {
        let rows = self.rank_rows(report, Dim::Machine, &report.by_machine, rate, false, cx);
        card(web("custos_por_maquina"), Some(web("custos_onde_sessao")))
            .child(if rows.is_empty() { empty_state(web("custos_sem_dados_no_periodo")) } else { div().flex().flex_col().gap(px(2.)).children(rows) })
    }

    fn render_areas(&self, d: &Derived, rate: Option<f64>) -> Div {
        let filter = &self.costs.filter;
        let mut hint = web("uso_graf_areas_nota");
        if [Dim::Provider, Dim::Source, Dim::Machine].iter().any(|d| !filter.get(*d).is_empty()) { hint.push(' '); hint.push_str(&web("custos_areas_sem_recorte")); }
        let body = card(web("uso_graf_areas"), Some(hint));
        let _ = d;
        match &self.costs.areas.value {
            None => body.child(empty_state(web("custos_areas_carregando"))),
            Some(Err(error)) => body.child(empty_state(error.clone())),
            Some(Ok(list)) => {
                const ORDER: [&str; 7] = ["front", "back", "banco", "infra", "docs", "outros", "conversa"];
                let pos = |k: &str| ORDER.iter().position(|o| *o == k).unwrap_or(ORDER.len());
                let mut items: Vec<&Area> = list.iter().filter(|a| a.tokens() > 0.).collect();
                items.sort_by(|a, b| pos(&a.key).cmp(&pos(&b.key)).then(b.tokens().total_cmp(&a.tokens())));
                if items.is_empty() { return body.child(empty_state(web("custos_sem_dados_no_periodo"))); }
                let total: f64 = items.iter().map(|a| a.tokens()).sum();
                let color = |k: &str| match k { "front" => chart(1), "back" => chart(2), "banco" => chart(3), "infra" => chart(4), "conversa" => theme::muted(),
                    _ => theme::border_strong() };
                let name = |k: &str| match k { "front" | "back" | "banco" | "infra" | "docs" | "outros" | "conversa" => web(&format!("uso_area_{k}")), other => other.to_owned() };
                body.child(stack_bar(items.iter().map(|a| (a.tokens(), color(&a.key))).collect()))
                    .child(table_row(vec![(String::new(), None), (web("ctx_tokens"), Some(110.)), (web("custos_custo"), Some(130.)), (web("custos_areas_pct"), Some(110.))], true))
                    .children(items.iter().map(|a| table_row_el(div().flex().items_center().gap(px(8.)).child(swatch(color(&a.key))).child(name(&a.key)),
                        vec![(tok(a.tokens()), 110., false), (money2(a.cost, rate), 130., false), (pct(a.tokens(), total), 110., true)])))
            }
        }
    }

    fn render_projects(&mut self, report: &Report, d: &Derived, rate: Option<f64>, cx: &mut Context<Self>) -> Div {
        let hidden = self.hidden_projects().clone();
        let list = self.list_of(report, Dim::Project);
        let (visible, gone): (Vec<Bucket>, Vec<Bucket>) = list.into_iter().partition(|b| !hidden.contains(&b.key));
        let shown: Vec<Bucket> = if self.costs.all_projects { visible.clone() } else { visible.iter().take(12).cloned().collect() };
        let mut hint = web("custos_pasta_sessao");
        if d.filtering && !d.has_combos { hint.push(' '); hint.push_str(&web("custos_ressalva_periodo_inteiro")); }
        let rows = self.rank_rows(report, Dim::Project, &shown, rate, true, cx);
        let mut body = card(web("custos_por_projeto"), Some(hint))
            .child(if rows.is_empty() { empty_state(web("custos_sem_dados_no_periodo")) } else { div().flex().flex_col().gap(px(2.)).children(rows) });
        if visible.len() > 12 {
            let all = self.costs.all_projects;
            body = body.child(Button::new("costs-projects-more").ghost().small()
                .label(if all { web("custos_mostrar_menos") } else { web_with("custos_ver_projetos", &[("n", visible.len().to_string())]) })
                .on_click(cx.listener(|this, _, _, cx| { this.costs.all_projects = !this.costs.all_projects; cx.notify(); })));
        }
        if !gone.is_empty() {
            let total: f64 = gone.iter().map(|b| b.cost).sum();
            body = body.child(div().flex().flex_wrap().items_center().gap(px(6.)).pt(px(8.)).border_t_1().border_color(theme::border())
                .text_size(px(12.5)).text_color(theme::muted())
                .child(web_with("custos_fora_da_lista", &[("n", gone.len().to_string()), ("valor", money2(total, rate))]))
                .children(gone.iter().map(|b| {
                    let key = b.key.clone();
                    Button::new(SharedString::from(format!("costs-unhide-{key}"))).outline().xsmall().label(format!("{} ✕", project_label(&key))).tooltip(key.clone())
                        .on_click(cx.listener(move |this, _, window, cx| this.set_hidden(Some(key.clone()), false, window, cx)))
                }))
                .child(Button::new("costs-unhide-all").ghost().xsmall().label(web("custos_mostrar_todos"))
                    .on_click(cx.listener(|this, _, window, cx| this.set_hidden(None, false, window, cx)))));
        }
        body
    }

    fn render_sessions(&self, report: &Report, d: &Derived, rate: Option<f64>, cx: &mut Context<Self>) -> Div {
        let f = &self.costs.filter;
        let cut: Vec<&Session> = report.sessoes.iter().filter(|s| matches(f.get(Dim::Provider), &s.provider) && matches(f.get(Dim::Source), &s.source)
            && matches(f.get(Dim::Project), &s.project) && matches(f.get(Dim::Model), &s.model) && matches(f.get(Dim::Machine), &s.machine)).collect();
        let body = card(web("custos_sessoes_caras"), Some(web("custos_sessoes_hint")));
        let _ = d;
        if report.sessoes.is_empty() && report.totals.sessions > 0. { return body.child(empty_state(web("custos_sessoes_atualize"))); }
        if cut.is_empty() { return body.child(empty_state(web("custos_sessoes_vazio"))); }
        let days = |s: &Session| if s.inicio == s.fim { day_label(&s.inicio) } else { format!("{}–{}", day_label(&s.inicio), day_label(&s.fim)) };
        let more = self.costs.more_sessions;
        let rows = cut.iter().take(if more { 30 } else { 10 }).map(|s| {
            let raw = s.input + s.output + s.cache_write + s.cache_read;
            let cost = if s.cost == 0. && raw > 0. && !is_free(&s.model) { "—".into() } else { money2(s.cost, rate) };
            let mut meta = vec![source_name(&s.source), s.model.clone(), days(s),
                web_with("custos_sessao_tokens_novos", &[("tokens", tok(s.input + s.cache_write + s.output))])];
            if s.custo_regravado > 0. { meta.push(web_with("custos_sessao_cache_perdido", &[("valor", money2(s.custo_regravado, rate))])); }
            if s.subagentes > 0. { meta.push(format!("+{} {}", dec(s.subagentes, 0), web(if s.subagentes == 1. { "custos_subagente" } else { "custos_subagentes" }))); }
            div().id(SharedString::from(format!("costs-session-{}-{}", s.machine, s.session_id))).px(px(8.)).py(px(8.)).border_t_1().border_color(theme::border())
                .flex().flex_col().gap(px(3.))
                .child(div().flex().items_center().gap(px(10.))
                    .child(div().flex_1().min_w_0().truncate().text_size(px(13.5)).child(project_label(&s.project)))
                    .child(div().flex_shrink_0().font_weight(FontWeight::SEMIBOLD).child(cost)))
                .child(div().flex().flex_wrap().gap(px(10.)).text_size(px(12.)).text_color(theme::muted()).children(meta))
                .tooltip({ let project = s.project.clone(); move |window, cx| gpui_kit::component::tooltip::Tooltip::new(project.clone()).build(window, cx) })
        });
        let mut body = body.child(div().flex().flex_col().children(rows));
        if cut.len() > 10 {
            body = body.child(Button::new("costs-sessions-more").ghost().small()
                .label(if more { web("custos_mostrar_menos") } else { web_with("custos_ver_sessoes", &[("n", cut.len().min(30).to_string())]) })
                .on_click(cx.listener(|this, _, _, cx| { this.costs.more_sessions = !this.costs.more_sessions; cx.notify(); })));
        }
        body
    }

    fn render_models(&self, report: &Report, d: &Derived, rate: Option<f64>, cx: &mut Context<Self>) -> Div {
        let list = self.list_of(report, Dim::Model);
        let total: f64 = list.iter().map(|b| b.cost).sum();
        let peak = list.iter().map(|b| b.cost).fold(1., f64::max);
        let mut hint = web("custos_com_tarifa");
        if d.filtering && !d.has_combos { hint.push(' '); hint.push_str(&web("custos_ressalva_periodo_inteiro")); }
        const COLS: [f32; 7] = [150., 80., 80., 96., 110., 90., 100.];
        let head = table_row(vec![(web("custos_dim_modelo"), None), (web("custos_col_entrada"), Some(COLS[0])), ("out".into(), Some(COLS[1])),
            ("cache R".into(), Some(COLS[2])), (web("custos_tarifa_in_out"), Some(COLS[3])), (web("custos_origem"), Some(COLS[4])),
            (web("custos_col_pct_conta"), Some(COLS[5])), (web("custos_custo"), Some(COLS[6])), (String::new(), Some(60.))], true);
        let mut body = card(web("custos_por_modelo"), Some(hint)).child(head);
        if list.is_empty() { return body.child(empty_state(web("custos_sem_dados_no_periodo"))); }
        for b in &list {
            let priced = d.rates.contains_key(&b.key);
            let shown_rate = d.rates.get(&b.key).cloned().flatten();
            let partial = priced && report.sem_tarifa.contains(&b.key);
            let key = b.key.clone();
            let on = self.costs.filter.has(Dim::Model, &key);
            let tag = |text: String| div().px(px(6.)).py(px(1.)).rounded(px(4.)).bg(theme::inset()).text_size(px(11.)).text_color(theme::muted()).child(text);
            let name = div().flex().items_center().gap(px(6.)).child(div().truncate().child(b.key.clone()))
                .when(!priced, |el| el.child(tag(web(if is_free(&b.key) { "custos_gratis" } else { "custos_sem_tarifa" }))))
                .when(partial, |el| el.child(tag(web("custos_preco_parcial"))));
            let entry = div().flex().justify_end().child(tok(b.input + b.cache_write + b.cache_read))
                .child(div().text_color(theme::faint()).child(format!("/{}", tok(b.input))))
                .when(b.cache_write == 0. && b.cache_read > 0., |el| el.child(div().text_color(theme::faint()).child("*")));
            let origin = div().flex().justify_end().gap(px(4.)).child(shown_rate.as_ref().map_or("—".into(), |r| r.origin.clone()))
                .when(shown_rate.as_ref().is_some_and(|r| r.cache_estimado), |el| el.child(tag(web("custos_cache_estimado"))));
            let cells: Vec<(AnyElement, f32)> = vec![
                (entry.into_any_element(), COLS[0]),
                (div().child(tok(b.output)).into_any_element(), COLS[1]),
                (div().child(tok(b.cache_read)).into_any_element(), COLS[2]),
                (div().text_color(theme::muted()).child(shown_rate.as_ref().map_or("—".into(), |r| format!("{}/{}", dec(r.input, 2), dec(r.output, 2)))).into_any_element(), COLS[3]),
                (origin.text_color(theme::muted()).into_any_element(), COLS[4]),
                (div().text_color(theme::muted()).child(if priced { pct(b.cost, total) } else { "—".into() }).into_any_element(), COLS[5]),
                (div().font_weight(FontWeight::SEMIBOLD).text_color(if priced { theme::accent() } else { theme::faint() })
                    .child(if priced { money2(b.cost, rate) } else { "—".into() }).into_any_element(), COLS[6]),
            ];
            let bar = div().w(px(60.)).flex_shrink_0().pl(px(10.)).child(div().h(px(5.)).rounded_full().bg(theme::inset())
                .when(priced, |el| el.child(div().h_full().rounded_full().bg(chart(1)).w(relative(((b.cost / peak) as f32).max(0.03))))));
            body = body.child(Button::new(SharedString::from(format!("costs-model-{key}")))
                .custom(ButtonCustomVariant::new(cx).color(if on { theme::accent_dim() } else { transparent_black() }).foreground(theme::text())
                    .hover(theme::hover()).active(theme::hover()))
                .when(on, |el| el.bg(theme::accent_dim())).w_full().h(px(34.)).px(px(8.)).rounded(px(6.))
                .child(div().w_full().flex().items_center().text_size(px(13.))
                    .child(div().flex_1().min_w_0().child(name))
                    .children(cells.into_iter().map(|(el, w)| div().w(px(w)).flex_shrink_0().flex().justify_end().child(el)))
                    .child(bar))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_cut(Dim::Model, key.clone(), cx))));
        }
        if list.iter().any(|b| b.cache_write == 0. && b.cache_read > 0.) { body = body.child(hint_text(web("custos_sem_cache_escrito_ajuda"))); }
        body
    }

    fn render_compare(&self, report: &Report, d: &Derived, rate: Option<f64>, multi: bool, cx: &mut Context<Self>) -> Div {
        let dim = self.compare_dim(report);
        let metric = self.costs.compare_metric;
        let marked = self.costs.compared.clone();
        let base = filtered(&report.combos, &self.costs.filter.without(dim));
        let all = if d.has_combos { group(&base, dim) } else { report.list(dim).to_vec() };
        let mut candidates: Vec<Bucket> = all.iter().take(8).cloned().collect();
        candidates.extend(all.iter().skip(8).filter(|b| marked.contains(&b.key)).cloned());
        let rest: Vec<Bucket> = all.iter().filter(|b| !candidates.iter().any(|c| c.key == b.key)).cloned().collect();
        let fmt = move |v: f64| if metric == Metric::Cost { money(v, rate) } else { tok(v) };

        let shown = Dim::shown(multi);
        let labels: Vec<String> = shown.iter().map(|d| d.name()).collect();
        let dims = segments("costs-compare-dim", &labels, shown.iter().position(|x| *x == dim).unwrap_or(0), shown.len(), false, String::new(),
            move |this: &mut Hangar, n, _: &mut Window, cx| { this.costs.compare_dim = Some(shown[n]); this.costs.compared.clear(); cx.notify(); }, cx);
        let metrics = segments("costs-compare-metric", &[web("ctx_tokens"), web("custos_custo")], (metric == Metric::Cost) as usize, 2, false, String::new(),
            |this: &mut Hangar, n, _: &mut Window, cx| { this.costs.compare_metric = if n == 1 { Metric::Cost } else { Metric::Tokens }; cx.notify(); }, cx);
        let full = marked.len() >= COMPARE_MAX;
        let chips = div().flex().flex_wrap().gap(px(6.)).children(candidates.iter().map(|b| {
            let key = b.key.clone();
            let on = marked.contains(&key);
            Button::new(SharedString::from(format!("costs-cmp-{key}"))).outline().small().selected(on).disabled(!on && full)
                .label(Self::name_of(report, dim, &key))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_compared(key.clone(), cx)))
        }));
        let mut body = card(web("lista_comparar"), Some(web_with("custos_comparar_hint", &[("n", COMPARE_MAX.to_string())])))
            .child(div().flex().flex_wrap().items_center().gap(px(10.))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(web("custos_comparar_por"))).child(dims).child(metrics))
            .child(chips);
        if !rest.is_empty() {
            let this = cx.entity().downgrade();
            let items: Vec<(String, String, String)> = rest.iter().map(|b| (b.key.clone(), Self::name_of(report, dim, &b.key),
                if b.unknown() { "—".into() } else { money(b.cost, rate) })).collect();
            body = body.child(Button::new("costs-cmp-add").ghost().small().disabled(full)
                .label(format!("{} — {}", web("custos_adicionar"), web_with("custos_adicionar_fora", &[("n", rest.len().to_string())])))
                .dropdown_menu_with_anchor(Anchor::TopLeft, move |menu, _, _| {
                    let mut menu = sidebar::menu_style(menu).min_w(px(320.)).max_w(px(480.)).max_h(px(420.)).scrollable(true);
                    for (key, name, hint) in items.clone() {
                        let this = this.clone();
                        menu = menu.item(PopupMenuItem::element(move |_, _| div().w_full().flex().items_center().gap(px(12.))
                                .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                                .child(div().flex_shrink_0().text_color(theme::muted()).child(hint.clone())))
                            .on_click(move |_, _, cx| { let _ = this.update(cx, |this, cx| this.toggle_compared(key.clone(), cx)); }));
                    }
                    menu
                }));
        }
        if marked.len() < 2 {
            body = body.child(empty_state(web("custos_marque_duas")));
        } else {
            let cards: Vec<Bucket> = if d.has_combos {
                let groups = group(&base, dim);
                marked.iter().map(|k| groups.iter().find(|b| &b.key == k).cloned().unwrap_or_else(|| Bucket::zero(k))).collect()
            } else { marked.iter().map(|k| report.list(dim).iter().find(|b| &b.key == k).cloned().unwrap_or_else(|| Bucket::zero(k))).collect() };
            let total: f64 = cards.iter().map(|b| b.value(metric)).sum();
            let days = if self.costs.period.days() > 0. { self.costs.period.days() } else { report.by_day.len().max(1) as f64 };
            let unknown_cost = |b: &Bucket| metric == Metric::Cost && b.unknown();
            body = body.child(div().flex().flex_wrap().gap(px(10.)).children(cards.iter().enumerate().map(|(i, b)| {
                div().flex_1().min_w(px(200.)).p(px(12.)).rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
                    .flex().flex_col().gap(px(3.))
                    .child(div().flex().items_center().gap(px(6.)).text_size(px(12.5)).text_color(theme::muted())
                        .child(swatch(chart(i + 1))).child(div().truncate().child(Self::name_of(report, dim, &b.key))))
                    .child(div().text_size(px(20.)).font_weight(FontWeight::SEMIBOLD)
                        .child(if metric == Metric::Cost { if b.unknown() { "—".into() } else { money2(b.cost, rate) } } else { tok(b.raw()) }))
                    .children([
                        if metric == Metric::Cost { tok(b.raw()) } else if b.unknown() { "—".into() } else { money2(b.cost, rate) },
                        self.sessions_text(d.has_combos, b),
                        if unknown_cost(b) { "—".into() } else { web_with("custos_do_comparado", &[("pct", pct(b.value(metric), total))]) },
                        format!("{}{}", if unknown_cost(b) { "—".into() } else { fmt(b.value(metric) / days) }, web("custos_por_dia")),
                    ].map(|t| div().text_size(px(12.)).text_color(theme::muted()).child(t)))
            })));
            let readable: Vec<&Bucket> = cards.iter().filter(|b| !unknown_cost(b)).collect();
            if readable.len() >= 2 {
                let mut sorted = readable.clone();
                sorted.sort_by(|a, b| b.value(metric).total_cmp(&a.value(metric)));
                let (big, small) = (sorted[0], sorted[sorted.len() - 1]);
                if big.value(metric) > 0. {
                    body = body.child(hint_text(web_with("custos_leitura", &[("menor", Self::name_of(report, dim, &small.key)), ("vMenor", fmt(small.value(metric))),
                        ("pct", pct(small.value(metric), big.value(metric))), ("maior", Self::name_of(report, dim, &big.key)), ("vMaior", fmt(big.value(metric)))])));
                }
            }
            if !d.has_combos { body = body.child(hint_text(web("custos_sem_detalhamento"))); }
            else {
                let weekly = matches!(self.costs.period, Period::Quarter | Period::All);
                let keys: Vec<String> = marked.iter().filter(|k| metric == Metric::Tokens || !cards.iter().find(|b| &b.key == *k).is_some_and(|b| b.unknown())).cloned().collect();
                let points = compare_series(&base, dim, &keys, metric, weekly);
                if !points.is_empty() {
                    let colors: Vec<Hsla> = keys.iter().map(|k| chart(marked.iter().position(|m| m == k).unwrap_or(0) + 1)).collect();
                    body = body.child(line_chart(points, colors, fmt)).child(hint_text(web(if weekly { "custos_ponto_semana" } else { "custos_ponto_dia" })));
                }
            }
        }
        if metric == Metric::Cost { body = body.child(hint_text(web("custos_preco_tabela"))); }
        if dim == Dim::Provider {
            let mut inflating: Vec<String> = base.iter().filter(|c| c.source == "claude" && c.provider.starts_with("anthropic:") && report.sem_tarifa.contains(&c.model))
                .map(|c| c.model.clone()).collect::<HashSet<_>>().into_iter().collect();
            inflating.sort();
            if !inflating.is_empty() {
                body = body.child(hint_text(format!("⚠ {}", web_with(if inflating.len() == 1 { "custos_inflando_1" } else { "custos_inflando" }, &[("nomes", inflating.join(", "))]))));
            }
        }
        body
    }

    fn toggle_compared(&mut self, key: String, cx: &mut Context<Self>) {
        let list = &mut self.costs.compared;
        if let Some(at) = list.iter().position(|k| *k == key) { list.remove(at); } else if list.len() < COMPARE_MAX { list.push(key); }
        cx.notify();
    }

    fn render_method(&self, report: &Report, d: &Derived, rate: Option<f64>, cx: &mut Context<Self>) -> Div {
        let open = self.costs.method_open;
        let toggle = Button::new("costs-method").ghost().small().icon(if open { IconName::ChevronDown } else { IconName::ChevronRight })
            .label(web("custos_como_calculamos"))
            .on_click(cx.listener(|this, _, _, cx| { this.costs.method_open = !this.costs.method_open; cx.notify(); }));
        let mut body = card_plain().child(div().flex().child(toggle));
        if !open { return body; }
        let equivalent = if d.filtering && (!d.has_combos || d.unknown || d.empty_cut) { "—".into() } else { tok(d.equivalent) };
        let mut notes = vec![web("custos_metodo_codex"), web("custos_metodo_standard"),
            format!("{}: {}. {}", web("custos_equivalente_cobrado"), equivalent, web("custos_equivalente_metodo")),
            format!("{} {}.", web("custos_fontes"), ["claude", "codex", "pi", "omp", "kimi"].map(source_name).join(" · ")),
            format!("{}{}", web("custos_tarifas"), web("custos_tarifas_models"))];
        if let Some(rate) = rate.filter(|_| appearance::get().currency == Currency::Brl) {
            notes.push(format!("{} {}", web("custos_cotacao"), web_with("custos_usd_brl", &[("taxa", dec(rate, 2))])));
        }
        notes.push(format!("{}{}{}", web("custos_custo_tabela"), web("custos_nao_fatura"), web("custos_assinatura")));
        let free: Vec<&String> = report.sem_tarifa.iter().filter(|m| is_free(m)).collect();
        if !free.is_empty() {
            notes.push(format!("{} {} ({}).", free.len(), web(if free.len() == 1 { "custos_modelo_gratis" } else { "custos_modelos_gratis" }),
                free.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")));
        }
        let unknown: Vec<&String> = report.sem_tarifa.iter().filter(|m| !is_free(m)).collect();
        if !unknown.is_empty() {
            notes.push(format!("{} {} {}", unknown.len(), web(if unknown.len() == 1 { "custos_modelo_sem_tarifa" } else { "custos_modelos_sem_tarifa" }),
                web_with("custos_aparecem_traco", &[("lista", unknown.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "))])));
        }
        for note in notes { body = body.child(hint_text(note)); }
        body
    }
}

fn default_layers(mode: &str) -> HashSet<String> {
    if mode == "kind-tokens" { HashSet::from(["cache_read".to_owned()]) } else { HashSet::new() }
}

/// Série do Comparar: um ponto por dia (ou semana) com o valor de cada entidade; dia sem ninguém entra zerado.
fn compare_series(combos: &[&Combo], dim: Dim, keys: &[String], metric: Metric, weekly: bool) -> Vec<(String, Vec<f64>)> {
    if keys.is_empty() { return Vec::new(); }
    let mut acc: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for c in combos {
        let Some(i) = keys.iter().position(|k| k == c.field(dim)) else { continue };
        let x = if weekly { monday(&c.dia) } else { c.dia.clone() };
        acc.entry(x).or_insert_with(|| vec![0.; keys.len()])[i] += if metric == Metric::Cost { c.cost } else { c.input + c.output + c.cache_write + c.cache_read };
    }
    let (Some(first), Some(last)) = (acc.keys().next().cloned(), acc.keys().next_back().cloned()) else { return Vec::new() };
    days_between(&first, &last, if weekly { 7 } else { 1 }).into_iter().map(|x| { let v = acc.remove(&x).unwrap_or_else(|| vec![0.; keys.len()]); (x, v) }).collect()
}

/// Linhas do Comparar: o `PathBuilder` desenha cada série e um ponto em cada valor (com um ponto só, a linha não aparece).
fn line_chart(points: Vec<(String, Vec<f64>)>, colors: Vec<Hsla>, fmt: impl Fn(f64) -> String) -> Div {
    const H: f32 = 170.;
    let top = points.iter().flat_map(|p| p.1.iter().copied()).fold(1., f64::max);
    let first = points.first().map(|p| day_label(&p.0)).unwrap_or_default();
    let last = points.last().map(|p| day_label(&p.0)).unwrap_or_default();
    let n = points.len();
    let grid = [0., 0.5, 1.].map(|f| (f, fmt(top * f)));
    let plot = canvas(|_, _, _| {}, move |bounds, _, window, _| {
        let (w, h) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
        let x = |i: usize| if n > 1 { i as f32 * w / (n - 1) as f32 } else { w / 2. };
        let y = |v: f64| h - (v / top) as f32 * h;
        for (s, color) in colors.iter().enumerate() {
            let at = |i: usize| bounds.origin + point(px(x(i)), px(y(points[i].1[s])));
            if n > 1 {
                let mut path = PathBuilder::stroke(px(2.));
                path.move_to(at(0));
                for i in 1..n { path.line_to(at(i)); }
                if let Ok(path) = path.build() { window.paint_path(path, *color); }
            }
            for i in 0..n { window.paint_quad(fill(Bounds::centered_at(at(i), size(px(5.), px(5.))), *color).corner_radii(px(2.5))); }
        }
    }).size_full();
    div().flex().flex_col().gap(px(4.))
        .child(div().flex().child(div().relative().w(px(56.)).h(px(H)).flex_shrink_0().children(grid.iter().map(|(f, label)| {
                div().absolute().right(px(8.)).top(px(H * (1. - *f as f32) - 7.)).text_size(px(10.5)).text_color(theme::faint()).child(label.clone())
            })))
            .child(div().relative().flex_1().h(px(H))
                .children(grid.iter().map(|(f, _)| div().absolute().left_0().right_0().top(px(H * (1. - *f as f32))).h(px(1.)).bg(theme::border())))
                .child(div().absolute().inset_0().child(plot))))
        .child(div().pl(px(56.)).flex().justify_between().text_size(px(10.5)).text_color(theme::faint()).child(first).child(last))
}

// ── Peças de desenho ────────────────────────────────────────────────────────

/// Fundo da página inteira no padrão das páginas soltas (a de Configurações): na caixa solta vira painel.
pub(super) fn page_frame(content: Div) -> AnyElement {
    if theme::is_floating() {
        div().size_full().p(px(10.)).child(content.rounded(px(theme::PANEL_RADIUS)).border_1().border_color(theme::border()).bg(theme::chrome())
            .shadow(theme::panel_shadow()).overflow_hidden()).into_any_element()
    } else { content.bg(theme::chrome()).into_any_element() }
}

/// Cartão com título e explicação, na superfície das caixas das Configurações.
pub(super) fn card(title: String, hint: Option<String>) -> Div {
    card_plain().child(div().flex().flex_col().gap(px(3.))
        .child(div().text_size(px(15.)).font_weight(FontWeight::SEMIBOLD).child(title))
        .children(hint.map(|h| div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(h))))
}

pub(super) fn card_plain() -> Div {
    div().w_full().min_w_0().p(px(16.)).rounded(px(14.)).border_1().border_color(theme::border()).bg(theme::boxed()).flex().flex_col().gap(px(12.))
}

fn two_cols(a: Div, b: Div) -> Div { div().flex().gap(px(16.)).items_start().child(a.flex_1()).child(b.flex_1()) }

pub(super) fn field(label: String, control: AnyElement) -> Div {
    div().flex().flex_col().gap(px(5.)).child(div().text_size(px(12.)).text_color(theme::muted()).child(label)).child(control)
}

pub(super) fn swatch(color: Hsla) -> Div { div().size(px(9.)).flex_shrink_0().rounded(px(2.)).bg(color) }

pub(super) fn hint_text(text: String) -> Div { div().text_size(px(12.5)).text_color(theme::muted()).whitespace_normal().child(text) }

pub(super) fn note_box(text: String, color: Hsla) -> Div {
    div().px(px(14.)).py(px(10.)).rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset())
        .text_size(px(13.)).text_color(color).whitespace_normal().child(text)
}

pub(super) fn loading_state() -> Div {
    div().py(px(40.)).flex().items_center().justify_center().gap(px(10.)).text_color(theme::muted())
        .child(chrome::Spinner::new("costs-loading", IconName::LoaderCircle, px(16.), theme::muted())).child(web("comum_carregando"))
}

pub(super) fn empty_state(text: String) -> Div {
    div().py(px(18.)).flex().justify_center().text_size(px(13.)).text_color(theme::muted()).whitespace_normal().child(text)
}

pub(super) fn error_state(error: String, retry: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Div {
    div().py(px(32.)).flex().flex_col().items_center().gap(px(10.))
        .child(div().text_size(px(13.5)).text_color(theme::warning()).whitespace_normal().child(error))
        .child(Button::new("costs-retry").outline().small().label(web("config_server_tentar_de_novo")).on_click(retry))
}

fn kpi(label: String, value: String, foot: Vec<String>, secondary: bool, dash: bool) -> Div {
    div().flex_1().min_w_0().p(px(14.)).rounded(px(12.)).border_1().border_color(theme::border()).bg(theme::boxed()).flex().flex_col().gap(px(4.))
        .child(div().text_size(px(12.5)).text_color(theme::muted()).child(label))
        .child(div().text_size(px(26.)).font_weight(if secondary { FontWeight::NORMAL } else { FontWeight::SEMIBOLD })
            .text_color(if secondary || dash { theme::muted() } else { theme::text() }).child(value))
        .children(foot.into_iter().map(|f| div().text_size(px(12.)).text_color(theme::muted()).whitespace_normal().child(f)))
}

fn mini_kpi(label: String, value: String, color: Option<Hsla>) -> Div {
    div().flex_1().p(px(12.)).rounded(px(10.)).border_1().border_color(theme::border()).bg(theme::inset()).flex().flex_col().gap(px(4.))
        .child(div().text_size(px(12.)).text_color(theme::muted()).child(label))
        .child(div().text_size(px(19.)).font_weight(FontWeight::SEMIBOLD).text_color(color.unwrap_or_else(theme::text)).child(value))
}

/// Barra de 100% com uma fatia por valor positivo.
pub(super) fn stack_bar(parts: Vec<(f64, Hsla)>) -> Div {
    let total: f64 = parts.iter().map(|p| p.0.max(0.)).sum();
    div().h(px(28.)).w_full().flex().gap(px(2.)).rounded(px(8.)).overflow_hidden()
        .children(parts.into_iter().filter(|p| p.0 > 0. && total > 0.).map(|(v, c)| div().h_full().bg(c).flex_basis(px(0.)).flex_grow(v as f32)))
}

/// Linha de tabela de texto: a primeira coluna estica, as outras têm largura fixa e alinham à direita.
fn table_row(cells: Vec<(String, Option<f32>)>, header: bool) -> Div {
    div().flex().items_center().px(px(8.)).py(px(6.)).text_size(px(if header { 11.5 } else { 13. }))
        .when(header, |el| el.text_color(theme::faint()).font_weight(FontWeight::MEDIUM).border_b_1().border_color(theme::border()))
        .children(cells.into_iter().map(|(text, width)| match width {
            None => div().flex_1().min_w_0().truncate().child(text),
            Some(w) => div().w(px(w)).flex_shrink_0().flex().justify_end().child(text),
        }))
}

fn table_row_el(first: Div, cells: Vec<(String, f32, bool)>) -> Div {
    div().flex().items_center().px(px(8.)).py(px(6.)).text_size(px(13.)).border_t_1().border_color(theme::border())
        .child(first.flex_1().min_w_0())
        .children(cells.into_iter().map(|(text, w, dim)| div().w(px(w)).flex_shrink_0().flex().justify_end()
            .text_color(if dim { theme::muted() } else { theme::text() }).child(text)))
}

#[cfg(test)]
mod tests {
    use super::*;
    // O glob traz o `test` da gpui, que colide com o atributo padrão; o nome explícito vence o glob.
    use core::prelude::v1::test;

    fn combo(dia: &str, source: &str, model: &str, sub: bool, id: &str, cost: f64) -> Combo {
        Combo { dia: dia.into(), provider: "p".into(), source: source.into(), project: "/x/proj".into(), model: model.into(), subagente: sub,
            session_ids: Some(vec![id.into()]), input: 10., output: 5., cost, ..Default::default() }
    }

    #[test]
    fn cut_sums_sessions_once_and_separates_subagents() {
        let rows = [combo("2026-09-01", "claude", "m", false, "a", 1.), combo("2026-09-02", "claude", "m", false, "a", 2.),
            combo("2026-09-02", "codex", "m", true, "b", 4.)];
        let all: Vec<&Combo> = rows.iter().collect();
        let total = sum(&all);
        assert_eq!((total.sessions, total.subagentes, total.cost), (2., 1., 7.));
        let mut f = Filter::default();
        f.set(Dim::Source, vec!["codex".into()], true);
        assert_eq!(sum(&filtered(&rows, &f)).cost, 4.);
        // Sem detalhamento o recorte volta a uma dimensão e um valor.
        f.set(Dim::Model, vec!["m".into(), "n".into()], false);
        assert!(f.get(Dim::Source).is_empty() && f.get(Dim::Model) == ["m".to_owned()]);
        assert_eq!(group(&all, Dim::Source)[0].key, "codex");
    }

    #[test]
    fn merge_sums_machines_and_names_who_stayed_out() {
        let report = |cost: f64, id: &str| Report { totals: Bucket { sessions: 1., cost, ..Default::default() },
            by_model: vec![Bucket { key: "m".into(), cost, ..Default::default() }], combos: vec![combo("2026-09-01", "claude", "m", false, id, cost)],
            anterior: Some(Bucket { cost: 1., ..Default::default() }), cache_detailed: true, ..Default::default() };
        let part = |id: &str, label: &str, part| MachinePart { id: id.into(), label: label.into(), part };
        let parts = vec![part("a", "casa", Part::Ok(report(2., "s1"))), part("b", "notebook", Part::Ok(report(3., "s2"))),
            part("c", "vps", Part::Failed("x".into())), part("d", "velho", Part::Mismatched(Some(5.)))];
        let merged = merge_costs(&parts);
        assert_eq!((merged.totals.cost, merged.by_model.len(), merged.by_model[0].cost, merged.usd_brl), (5., 1, 5., Some(5.)));
        assert_eq!(merged.by_machine.iter().map(|b| b.key.as_str()).collect::<Vec<_>>(), ["b", "a"]);
        let mut f = Filter::default();
        f.set(Dim::Machine, vec!["a".into()], true);
        assert_eq!(sum(&filtered(&merged.combos, &f)).cost, 2.);
        assert_eq!(merged.anterior.map(|a| a.cost), Some(2.));
        let partial = Partial::of(&parts);
        assert_eq!((partial.failed, partial.mismatched), (vec!["vps".to_owned()], vec!["velho".to_owned()]));
        // Uma máquina sem detalhamento derruba o cruzamento de todas, e uma sem anterior derruba a comparação.
        let bare = Report { combos: Vec::new(), anterior: None, ..report(1., "s3") };
        let merged = merge_costs(&[part("a", "casa", Part::Ok(report(2., "s1"))), part("e", "antiga", Part::Ok(bare))]);
        assert!(merged.combos.is_empty() && merged.anterior.is_none());
    }

    #[test]
    fn formats_and_free_models_follow_the_web() {
        assert!(is_free("kimi-k3-free:high") && is_free("x/free") && is_free("m:free") && !is_free("freedom") && !is_free("carefree"));
        assert_eq!(days_between("2026-09-28", "2026-10-02", 1).len(), 5);
        assert_eq!(monday("2026-09-27"), "2026-09-21");
        assert_eq!(compare_series(&[&combo("2026-09-01", "c", "m", false, "a", 1.), &combo("2026-09-03", "c", "m", false, "a", 1.)],
            Dim::Source, &["c".into()], Metric::Cost, false).len(), 3);
    }
}
