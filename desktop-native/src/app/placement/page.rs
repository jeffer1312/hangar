//! Página Balanceamento: modos, limites, regras e pasta de clone. Lê a máquina conectada e grava em todas as ligadas,
//! dizendo qual ficou com regras diferentes ou numa versão sem o balanceamento.
use super::*;
use super::super::create::choice;
use super::super::settings::{Page, section_head, segments, setting_row, settings_box};
use gpui_kit::component::{select::{Select, SelectState}, searchable_list::{SearchableListItem, SearchableVec}};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const UI_MODES: [&str; 3] = ["off", "suggest", "ask"];
const AGENT_MODES: [&str; 3] = ["off", "auto", "ask"];

/// `PlacementConfig` do `hangar-placement` (`~/.hangar/placement.json`); o nativo não importa o crate, só a API.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Config {
    ui_mode: String,
    agent_mode: String,
    limits: Limits,
    project_rules: Vec<ProjectRule>,
    schedule_rules: Vec<ScheduleRule>,
    clone_dirs: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Limits { mem_free_pct: u32, working: u32, cpu_pct: u32 }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ProjectRule { project: String, server: String }

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ScheduleRule { #[serde(default)] days: Vec<u8>, from: String, to: String, tz: String, avoid: String }

/// O que a página usa de `GET /api/placement/facts`.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct Facts { version: String, repos: Vec<Repo>, clone_dir: Option<String> }

#[derive(Clone, Debug, Default, Deserialize)]
struct Repo { project: String }

#[derive(Clone, Debug, PartialEq)]
enum Read { Loading, Old, Failed(String), Ready(Config) }

struct Machine {
    label: String,
    active: bool,
    api: Option<Api>,
    ident: Option<String>,
    read: Read,
    facts: Option<Facts>,
    /// A última gravação desta tela nela.
    saved: Option<Result<(), String>>,
}

#[derive(Clone)]
struct Item { value: String, title: SharedString }

impl SearchableListItem for Item {
    type Value = String;
    fn title(&self) -> SharedString { self.title.clone() }
    fn value(&self) -> &String { &self.value }
}

type Choices = (Vec<String>, Vec<(String, String)>);

struct Inputs {
    mem: Entity<InputState>,
    working: Entity<InputState>,
    cpu: Entity<InputState>,
    from: Entity<InputState>,
    to: Entity<InputState>,
    tz: Entity<InputState>,
    /// Pasta de clone por id de máquina.
    clone: Vec<(String, Entity<InputState>)>,
    project: Entity<SelectState<SearchableVec<Item>>>,
    project_server: Entity<SelectState<Vec<Item>>>,
    avoid: Entity<SelectState<Vec<Item>>>,
    /// Projetos e máquinas com que as listas foram montadas: fato novo de uma máquina as refaz.
    choices: Choices,
}

#[derive(Default)]
pub(in crate::app) struct PlacementSettings {
    generation: u64,
    connection: u64,
    opened: bool,
    machines: Vec<Machine>,
    draft: Option<Config>,
    inputs: Option<Inputs>,
    new_days: Vec<u8>,
    /// Gravações ainda sem resposta.
    saving: usize,
    error: Option<String>,
}

async fn read(api: Api) -> (Option<String>, Read, Option<Facts>) {
    let (ident, config, facts) = tokio::join!(super::ident(&api),
        api.server_read(&["placement", "config"], &[], super::WAIT), api.server_read(&["placement", "facts"], &[], super::WAIT));
    let read = match config {
        Ok(value) => serde_json::from_value(value).map_or_else(|_| Read::Failed(tr("invalid_response")), Read::Ready),
        // Rota que não existe lá (404 sem código) é versão sem o balanceamento.
        Err(error) if error.status == Some(404) && error.code.is_none() => Read::Old,
        Err(error) => Read::Failed(Hangar::fetch_failure(&error)),
    };
    (ident, read, facts.ok().and_then(|value| serde_json::from_value(value).ok()))
}

/// Os números dos limites como o servidor aceita (`store::validate`); `Err` é a chave do aviso.
fn limits(mem: &str, working: &str, cpu: &str) -> Result<Limits, &'static str> {
    let pct = |s: &str| s.trim().parse::<u32>().ok().filter(|n| *n <= 100);
    let mem_free_pct = pct(mem).ok_or("placement_limit_pct_invalid")?;
    let cpu_pct = pct(cpu).ok_or("placement_limit_pct_invalid")?;
    let working = working.trim().parse::<u32>().ok().filter(|n| *n >= 1).ok_or("placement_limit_working_invalid")?;
    Ok(Limits { mem_free_pct, working, cpu_pct })
}

/// A regra de horário como o servidor aceita: hora `HH:MM` e fuso IANA, senão ela nunca valeria.
fn schedule_rule(days: &[u8], from: &str, to: &str, tz: &str, avoid: Option<&String>) -> Result<ScheduleRule, &'static str> {
    let time = |s: &str| chrono::NaiveTime::parse_from_str(s.trim(), "%H:%M").ok().map(|_| s.trim().to_owned());
    let from = time(from).ok_or("placement_schedule_time_invalid")?;
    let to = time(to).ok_or("placement_schedule_time_invalid")?;
    let tz = tz.trim();
    if tz.parse::<chrono_tz::Tz>().is_err() { return Err("placement_schedule_tz_invalid"); }
    let avoid = avoid.filter(|a| !a.is_empty()).ok_or("placement_schedule_machine_missing")?;
    let mut days = days.to_vec();
    days.sort_unstable();
    Ok(ScheduleRule { days, from, to, tz: tz.to_owned(), avoid: avoid.clone() })
}

/// Como a máquina está em relação às regras desta: a chave do texto.
fn status(read: &Read, here: Option<&Config>) -> &'static str {
    match read {
        Read::Loading => "placement_machine_loading",
        Read::Old => "placement_machine_old",
        Read::Failed(_) => "placement_machine_silent",
        Read::Ready(config) if Some(config) == here => "placement_machine_same",
        Read::Ready(_) => "placement_machine_different",
    }
}

/// O fuso deste computador pelo nome IANA.
// ponytail: TZ e o atalho /etc/localtime; no Windows o campo fica vazio e a pessoa escreve o fuso.
fn local_tz() -> String {
    std::env::var("TZ").ok().map(|tz| tz.trim_start_matches(':').to_owned())
        .or_else(|| std::fs::read_link("/etc/localtime").ok()
            .and_then(|path| path.to_str()?.split_once("zoneinfo/").map(|(_, zone)| zone.to_owned())))
        .filter(|tz| tz.parse::<chrono_tz::Tz>().is_ok()).unwrap_or_default()
}

/// As pastas de clone dos campos sobre `dirs`: campo vazio tira a pasta da máquina.
fn merge_clone_dirs(dirs: &mut BTreeMap<String, String>, inputs: &Inputs, cx: &App) {
    for (ident, input) in &inputs.clone {
        let dir = input.read(cx).value().trim().to_owned();
        if dir.is_empty() { dirs.remove(ident); } else { dirs.insert(ident.clone(), dir); }
    }
}

fn project_title(project: &str) -> String { project.strip_prefix("git:").unwrap_or(project).to_owned() }

fn day_name(day: u8) -> String { tr(&format!("placement_day_{day}")) }

impl PlacementSettings {
    fn here(&self) -> Option<&Config> {
        self.machines.iter().find(|m| m.active).and_then(|m| match &m.read { Read::Ready(c) => Some(c), _ => None })
    }

    fn label_of(&self, ident: &str) -> String {
        self.machines.iter().find(|m| m.ident.as_deref() == Some(ident)).map_or_else(|| ident.to_owned(), |m| m.label.clone())
    }

    fn choices(&self) -> Choices {
        let mut projects: Vec<String> = self.machines.iter().filter_map(|m| m.facts.as_ref())
            .flat_map(|f| f.repos.iter().map(|r| r.project.clone())).collect();
        projects.sort();
        projects.dedup();
        let machines = self.machines.iter().filter_map(|m| Some((m.ident.clone()?, m.label.clone()))).collect();
        (projects, machines)
    }
}

impl Hangar {
    pub(in crate::app) fn placement_opened(&mut self, cx: &mut Context<Self>) {
        if self.placement.opened && self.placement.connection == self.connection { return; }
        self.load_placement(cx);
    }

    fn load_placement(&mut self, cx: &mut Context<Self>) {
        let Some(active) = self.api.clone() else { return };
        let here = super::super::servers::norm(&active.identity());
        let label = |label: &str, address: &str| if label.is_empty() { super::super::servers::default_label(address) } else { label.to_owned() };
        let mut machines: Vec<Machine> = self.server_choices().into_iter().map(|c| Machine {
            active: c.key == here, label: label(&c.label, &c.address),
            api: if c.key == here { Some(active.clone()) } else { Api::new(&c.address, &c.token).ok() },
            ident: None, read: Read::Loading, facts: None, saved: None,
        }).collect();
        if !machines.iter().any(|m| m.active) {
            machines.insert(0, Machine { label: label("", &here), active: true, api: Some(active), ident: None,
                read: Read::Loading, facts: None, saved: None });
        }
        let generation = self.placement.generation + 1;
        let apis: Vec<Option<Api>> = machines.iter().map(|m| m.api.clone()).collect();
        self.placement = PlacementSettings { generation, connection: self.connection, opened: true, machines, ..Default::default() };
        for (n, api) in apis.into_iter().enumerate() {
            let Some(api) = api else {
                self.placement.machines[n].read = Read::Failed(tr("invalid_response"));
                continue;
            };
            let task = self.runtime.spawn(read(api));
            cx.spawn(async move |this, cx| {
                let answer = task.await.ok();
                let _ = this.update(cx, |this, cx| {
                    let s = &mut this.placement;
                    if s.generation != generation { return; }
                    let Some(m) = s.machines.get_mut(n) else { return };
                    match answer {
                        Some((ident, read, facts)) => (m.ident, m.read, m.facts) = (ident, read, facts),
                        None => m.read = Read::Failed(tr("invalid_response")),
                    }
                    if s.draft.is_none() { s.draft = s.here().cloned(); }
                    cx.notify();
                });
            }).detach();
        }
        cx.notify();
    }

    /// Monta os campos com o rascunho; as listas de projeto e máquina e as pastas de clone se refazem quando chega fato novo.
    fn placement_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.placement.draft.is_none() { return; }
        let choices = self.placement.choices();
        if self.placement.inputs.as_ref().is_some_and(|i| i.choices == choices) { return; }
        let Some(draft) = &self.placement.draft else { return };
        let limits = draft.limits.clone();
        // As pastas digitadas antes do fato novo ficam no rascunho, senão a lista refeita as perderia.
        let mut clone_dirs = draft.clone_dirs.clone();
        if let Some(inputs) = &self.placement.inputs { merge_clone_dirs(&mut clone_dirs, inputs, cx); }
        let field = |value: String, placeholder: String, window: &mut Window, cx: &mut Context<Self>|
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder).default_value(value));
        let (projects, machines) = &choices;
        let project_items = projects.iter().map(|p| Item { value: p.clone(), title: project_title(p).into() }).collect::<Vec<_>>();
        let machine_items = || machines.iter().map(|(id, label)| Item { value: id.clone(), title: label.clone().into() }).collect::<Vec<_>>();
        let project = cx.new(|cx| SelectState::new(SearchableVec::new(project_items), None, window, cx).searchable(true));
        let project_server = cx.new(|cx| SelectState::new(machine_items(), None, window, cx));
        let avoid = cx.new(|cx| SelectState::new(machine_items(), None, window, cx));
        let clone = machines.iter().map(|(ident, _)| {
            let computed = self.placement.machines.iter().find(|m| m.ident.as_deref() == Some(ident.as_str()))
                .and_then(|m| m.facts.as_ref()?.clone_dir.clone());
            let placeholder = computed.map_or_else(|| tr("placement_clone_auto_unknown"), |dir| tr("placement_clone_auto").replace("{dir}", &dir));
            (ident.clone(), field(clone_dirs.get(ident).cloned().unwrap_or_default(), placeholder, window, cx))
        }).collect();
        let kept = self.placement.inputs.take();
        let inputs = match kept {
            Some(old) => Inputs { clone, project, project_server, avoid, choices, ..old },
            None => Inputs {
                mem: field(limits.mem_free_pct.to_string(), String::new(), window, cx),
                working: field(limits.working.to_string(), String::new(), window, cx),
                cpu: field(limits.cpu_pct.to_string(), String::new(), window, cx),
                from: field(String::new(), "09:00".into(), window, cx),
                to: field(String::new(), "18:00".into(), window, cx),
                tz: field(local_tz(), "America/Sao_Paulo".into(), window, cx),
                clone, project, project_server, avoid, choices,
            },
        };
        self.placement.inputs = Some(inputs);
        if let Some(d) = &mut self.placement.draft { d.clone_dirs = clone_dirs; }
    }

    /// O rascunho com o que está nos campos: limites e pastas de clone.
    fn placement_value(&self, cx: &App) -> Result<Config, String> {
        let (Some(draft), Some(inputs)) = (&self.placement.draft, &self.placement.inputs) else { return Err(tr("invalid_response")) };
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let mut config = draft.clone();
        config.limits = limits(&text(&inputs.mem), &text(&inputs.working), &text(&inputs.cpu)).map_err(tr)?;
        merge_clone_dirs(&mut config.clone_dirs, inputs, cx);
        Ok(config)
    }

    fn edit_placement(&mut self, change: impl FnOnce(&mut Config), cx: &mut Context<Self>) {
        if self.placement.saving > 0 { return; }
        if let Some(draft) = &mut self.placement.draft { change(draft); }
        self.placement.error = None;
        cx.notify();
    }

    fn add_project_rule(&mut self, cx: &mut Context<Self>) {
        let Some(inputs) = &self.placement.inputs else { return };
        let project = inputs.project.read(cx).selected_value().cloned();
        let server = inputs.project_server.read(cx).selected_value().cloned();
        let (Some(project), Some(server)) = (project, server) else {
            self.placement.error = Some(tr("placement_project_missing"));
            cx.notify();
            return;
        };
        // Um projeto tem uma máquina só: a regra nova troca a anterior.
        self.edit_placement(|d| {
            d.project_rules.retain(|r| r.project != project);
            d.project_rules.push(ProjectRule { project, server });
        }, cx);
    }

    fn add_schedule_rule(&mut self, cx: &mut Context<Self>) {
        let Some(inputs) = &self.placement.inputs else { return };
        let text = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let avoid = inputs.avoid.read(cx).selected_value().cloned();
        match schedule_rule(&self.placement.new_days, &text(&inputs.from), &text(&inputs.to), &text(&inputs.tz), avoid.as_ref()) {
            Ok(rule) => {
                self.placement.new_days.clear();
                self.edit_placement(|d| d.schedule_rules.push(rule), cx);
            }
            Err(key) => { self.placement.error = Some(tr(key)); cx.notify(); }
        }
    }

    /// Grava o rascunho inteiro em cada máquina que respondeu com o balanceamento; a resposta de cada uma vai na linha dela.
    fn save_placement(&mut self, cx: &mut Context<Self>) {
        if self.placement.saving > 0 { return; }
        let config = match self.placement_value(cx) {
            Ok(config) => config,
            Err(error) => { self.placement.error = Some(error); cx.notify(); return; }
        };
        let generation = self.placement.generation;
        let targets: Vec<(usize, Api)> = self.placement.machines.iter().enumerate()
            .filter(|(_, m)| matches!(m.read, Read::Ready(_))).filter_map(|(n, m)| Some((n, m.api.clone()?))).collect();
        self.placement.error = None;
        self.placement.draft = Some(config.clone());
        for machine in &mut self.placement.machines { machine.saved = None; }
        for (n, api) in targets {
            self.placement.saving += 1;
            let body = json!(config);
            let task = self.runtime.spawn(async move {
                api.server_send(reqwest::Method::PUT, &["placement", "config"], Some(body), super::WAIT).await
            });
            let sent = config.clone();
            cx.spawn(async move |this, cx| {
                let result = task.await.unwrap_or_else(|error| Err(Failure::local(error.to_string())));
                let _ = this.update(cx, |this, cx| {
                    let s = &mut this.placement;
                    if s.generation != generation { return; }
                    s.saving = s.saving.saturating_sub(1);
                    let Some(m) = s.machines.get_mut(n) else { return };
                    match result {
                        Ok(value) => {
                            m.read = Read::Ready(serde_json::from_value(value).unwrap_or(sent));
                            m.saved = Some(Ok(()));
                        }
                        Err(error) => m.saved = Some(Err(Hangar::fetch_failure(&error))),
                    }
                    cx.notify();
                });
            }).detach();
        }
        cx.notify();
    }

    pub(in crate::app) fn render_placement(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let page = div().flex().flex_col().gap_4()
            .child(div().text_xl().font_weight(FontWeight::SEMIBOLD).child(Page::Placement.title()))
            .child(div().text_sm().text_color(theme::muted()).whitespace_normal().child(tr("placement_intro")));
        if self.api.is_none() { return page.child(div().text_sm().text_color(theme::muted()).child(tr("settings_offline"))).into_any_element(); }
        let failure = |page: Div, text: String, cx: &mut Context<Self>| page
            .child(div().id("placement-error").role(Role::Alert).text_sm().text_color(theme::danger()).whitespace_normal().child(text))
            .child(Button::new("placement-retry").outline().small().label(tr("server_retry"))
                .on_click(cx.listener(|this, _, _, cx| this.load_placement(cx)))).into_any_element();
        match self.placement.machines.iter().find(|m| m.active).map(|m| m.read.clone()) {
            None | Some(Read::Loading) => return page.child(div().id("placement-loading").role(Role::Status).aria_label(tr("loading"))
                .text_sm().text_color(theme::muted()).child(tr("loading"))).into_any_element(),
            Some(Read::Old) => return failure(page, tr("placement_old_here"), cx),
            Some(Read::Failed(error)) => return failure(page, tr("placement_read_failed").replace("{motivo}", &error), cx),
            Some(Read::Ready(_)) => {}
        }
        self.placement_inputs(window, cx);
        let live = self.settings_ui.live;
        let s = &self.placement;
        let (Some(draft), Some(inputs)) = (&s.draft, &s.inputs) else { return page.into_any_element() };
        let off = s.saving > 0;
        let mode = |list: &[&str], value: &str| list.iter().position(|m| *m == value).unwrap_or(0);
        let ui_labels = ["placement_mode_off", "placement_mode_suggest", "placement_mode_ask"].map(tr);
        let agent_labels = ["placement_mode_off", "placement_mode_auto", "placement_mode_ask"].map(tr);
        let ui = segments("placement-ui-mode", &ui_labels, mode(&UI_MODES, &draft.ui_mode), 3, off, String::new(),
            |this: &mut Hangar, n, _: &mut Window, cx| this.edit_placement(|d| d.ui_mode = UI_MODES[n].into(), cx), cx);
        let agent = segments("placement-agent-mode", &agent_labels, mode(&AGENT_MODES, &draft.agent_mode), 3, off, String::new(),
            |this: &mut Hangar, n, _: &mut Window, cx| this.edit_placement(|d| d.agent_mode = AGENT_MODES[n].into(), cx), cx);
        let modes = settings_box().child(section_head(IconName::Workflow, tr("placement_when"), Some(tr("placement_when_help")), None, px(16.)))
            .child(self.row(IconName::Monitor, "placement_ui_mode", Some(tr("placement_ui_mode_help")), true, ui))
            .child(self.row(IconName::Bot, "placement_agent_mode", Some(tr("placement_agent_mode_help")), true, agent));

        let number = |id: &'static str, input: &Entity<InputState>, unit: &str, aria: String| div().flex().items_center().gap_2()
            .child(div().w(px(72.)).child(Input::new(input).id(id).small().disabled(off).aria_label(aria)))
            .child(div().text_sm().text_color(theme::muted()).child(unit.to_owned()));
        let limits = settings_box().child(section_head(IconName::Gauge, tr("placement_limits"), Some(tr("placement_limits_help")), None, px(16.)))
            .child(self.row(IconName::HardDrive, "placement_limit_memory", Some(tr("placement_limit_memory_help")), true,
                number("placement-mem", &inputs.mem, "%", tr("placement_limit_memory")).into_any_element()))
            .child(self.row(IconName::Activity, "placement_limit_working", Some(tr("placement_limit_working_help")), true,
                number("placement-working", &inputs.working, "", tr("placement_limit_working")).into_any_element()))
            .child(self.row(IconName::Cpu, "placement_limit_cpu", Some(tr("placement_limit_cpu_help")), true,
                number("placement-cpu", &inputs.cpu, "%", tr("placement_limit_cpu")).into_any_element()));

        let remove = |id: SharedString, aria: String, edit: Box<dyn Fn(&mut Config)>, cx: &mut Context<Self>|
            Button::new(id).ghost().small().icon(IconName::Trash).disabled(off).accessibility_label(aria)
                .on_click(cx.listener(move |this, _, _, cx| this.edit_placement(|d| edit(d), cx))).into_any_element();
        let empty = |key: &str| div().border_t_1().border_color(theme::border()).px_4().py(px(12.)).text_sm().text_color(theme::muted())
            .whitespace_normal().child(tr(key));
        let form = || div().border_t_1().border_color(theme::border()).px_4().py(px(12.)).flex().flex_wrap().items_center().gap_2();

        let project_rows = draft.project_rules.iter().enumerate().map(|(n, rule)| {
            let title = project_title(&rule.project);
            let machine = s.label_of(&rule.server);
            setting_row(live, IconName::GitBranch, title.clone(), div().child(tr("placement_project_to").replace("{maquina}", &machine)), true,
                remove(SharedString::from(format!("placement-project-remove-{n}")), tr("placement_remove_rule").replace("{regra}", &title),
                    Box::new(move |d: &mut Config| { if n < d.project_rules.len() { d.project_rules.remove(n); } }), cx))
        }).collect::<Vec<_>>();
        let projects = settings_box().child(section_head(IconName::Folder, tr("placement_projects"), Some(tr("placement_projects_help")), None, px(16.)))
            .when(project_rows.is_empty(), |el| el.child(empty("placement_projects_empty")))
            .children(project_rows)
            .child(form()
                .child(div().w(px(260.)).child(Select::new(&inputs.project).id("placement-project").small().disabled(off)
                    .placeholder(tr(if inputs.choices.0.is_empty() { "placement_project_none_known" } else { "placement_project_pick" }))
                    .accessibility_label(tr("placement_project_pick"))))
                .child(div().w(px(180.)).child(Select::new(&inputs.project_server).id("placement-project-server").small().disabled(off)
                    .placeholder(tr("placement_machine_pick")).accessibility_label(tr("placement_machine_pick"))))
                .child(Button::new("placement-project-add").outline().small().icon(IconName::Plus).label(tr("placement_add")).disabled(off)
                    .on_click(cx.listener(|this, _, _, cx| this.add_project_rule(cx)))));

        let schedule_rows = draft.schedule_rules.iter().enumerate().map(|(n, rule)| {
            let days = if rule.days.is_empty() { tr("placement_every_day") } else { rule.days.iter().map(|d| day_name(*d)).collect::<Vec<_>>().join(", ") };
            let title = tr("placement_schedule_row").replace("{dias}", &days).replace("{de}", &rule.from).replace("{ate}", &rule.to);
            let desc = tr("placement_schedule_avoid").replace("{maquina}", &s.label_of(&rule.avoid)).replace("{fuso}", &rule.tz);
            setting_row(live, IconName::Clock, title.clone(), div().child(desc), true,
                remove(SharedString::from(format!("placement-schedule-remove-{n}")), tr("placement_remove_rule").replace("{regra}", &title),
                    Box::new(move |d: &mut Config| { if n < d.schedule_rules.len() { d.schedule_rules.remove(n); } }), cx))
        }).collect::<Vec<_>>();
        let day_buttons = (1..=7u8).map(|day| {
            let on = s.new_days.contains(&day);
            choice(SharedString::from(format!("placement-day-{day}")), on, cx).small().label(day_name(day)).disabled(off)
                .on_click(cx.listener(move |this, _, _, cx| {
                    let days = &mut this.placement.new_days;
                    if let Some(at) = days.iter().position(|d| *d == day) { days.remove(at); } else { days.push(day); }
                    cx.notify();
                }))
        }).collect::<Vec<_>>();
        let time = |id: &'static str, input: &Entity<InputState>, aria: &str|
            div().w(px(72.)).child(Input::new(input).id(id).small().disabled(off).aria_label(tr(aria)));
        let schedules = settings_box().child(section_head(IconName::Clock, tr("placement_schedules"), Some(tr("placement_schedules_help")), None, px(16.)))
            .when(schedule_rows.is_empty(), |el| el.child(empty("placement_schedules_empty")))
            .children(schedule_rows)
            .child(div().border_t_1().border_color(theme::border()).px_4().py(px(12.)).flex().flex_col().gap_2()
                .child(div().id("placement-days").role(Role::Group).aria_label(tr("placement_days")).flex().flex_wrap().gap_1().children(day_buttons))
                .child(div().text_size(px(12.5)).text_color(theme::muted()).child(tr("placement_days_help")))
                .child(div().flex().flex_wrap().items_center().gap_2()
                    .child(time("placement-from", &inputs.from, "placement_from"))
                    .child(div().text_sm().text_color(theme::muted()).child("–"))
                    .child(time("placement-to", &inputs.to, "placement_to"))
                    .child(div().w(px(180.)).child(Input::new(&inputs.tz).id("placement-tz").small().disabled(off).aria_label(tr("placement_tz"))))
                    .child(div().w(px(180.)).child(Select::new(&inputs.avoid).id("placement-avoid").small().disabled(off)
                        .placeholder(tr("placement_machine_avoid")).accessibility_label(tr("placement_machine_avoid"))))
                    .child(Button::new("placement-schedule-add").outline().small().icon(IconName::Plus).label(tr("placement_add")).disabled(off)
                        .on_click(cx.listener(|this, _, _, cx| this.add_schedule_rule(cx))))));

        let clone_rows = inputs.clone.iter().map(|(ident, input)| {
            setting_row(live, IconName::FolderOpen, s.label_of(ident), div(), true,
                div().w(px(260.)).child(Input::new(input).id(SharedString::from(format!("placement-clone-{ident}"))).small().disabled(off)
                    .aria_label(tr("placement_clone_for").replace("{maquina}", &s.label_of(ident)))).into_any_element())
        }).collect::<Vec<_>>();
        let clones = settings_box().child(section_head(IconName::Download, tr("placement_clone"), Some(tr("placement_clone_help")), None, px(16.)))
            .when(clone_rows.is_empty(), |el| el.child(empty("placement_clone_empty")))
            .children(clone_rows);

        let here = s.here();
        let machine_rows = s.machines.iter().map(|m| {
            let mut lines = vec![match &m.read {
                Read::Failed(error) => tr("placement_machine_silent").replace("{motivo}", error),
                read => tr(status(read, here)),
            }];
            if let Some(version) = m.facts.as_ref().map(|f| f.version.as_str()).filter(|v| !v.is_empty()) {
                lines.push(tr("placement_machine_version").replace("{versao}", version));
            }
            let saved = match &m.saved {
                Some(Ok(())) => Some((tr("placement_machine_saved"), theme::success())),
                Some(Err(error)) => Some((tr("placement_machine_save_failed").replace("{motivo}", error), theme::danger())),
                None => None,
            };
            let title = if m.active { tr("placement_machine_this").replace("{maquina}", &m.label) } else { m.label.clone() };
            setting_row(live, IconName::Server, title, div().child(lines.join(" · "))
                .children(saved.map(|(text, color)| div().text_color(color).child(text))), true, div().into_any_element())
        }).collect::<Vec<_>>();
        let machines = settings_box().child(section_head(IconName::Server, tr("placement_machines"), Some(tr("placement_machines_help")), None, px(16.)))
            .children(machine_rows);

        let saved_all = !s.machines.is_empty() && s.machines.iter().all(|m| !matches!(m.read, Read::Ready(_)) || m.saved == Some(Ok(())));
        page.child(modes).child(limits).child(projects).child(schedules).child(clones).child(machines)
            .when_some(s.error.clone(), |el, error| el.child(div().id("placement-validation").role(Role::Alert)
                .text_sm().text_color(theme::danger()).whitespace_normal().child(error)))
            .child(div().flex().items_center().justify_end().gap_3()
                .when(saved_all && !off, |el| el.child(div().id("placement-saved").role(Role::Status).text_sm()
                    .text_color(theme::success()).child(tr("placement_saved_all"))))
                .child(Button::new("placement-save").primary().small()
                    .label(tr(if off { "server_saving" } else { "placement_save_all" })).loading(off).disabled(off)
                    .on_click(cx.listener(|this, _, _, cx| this.save_placement(cx)))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    // Itens nomeados: o `*` traria o `test` do gpui, que toma o lugar do `#[test]`.
    use super::{Config, Limits, Read, limits, schedule_rule, status};
    use serde_json::json;

    #[test]
    fn the_server_config_round_trips_without_losing_a_field() {
        let wire = json!({"ui_mode": "suggest", "agent_mode": "auto", "limits": {"mem_free_pct": 15, "working": 6, "cpu_pct": 80},
            "project_rules": [{"project": "git:github.com/a/b", "server": "nb"}],
            "schedule_rules": [{"days": [1, 5], "from": "09:00", "to": "18:00", "tz": "America/Sao_Paulo", "avoid": "nb"}],
            "clone_dirs": {"nb": "/h/Projetos"}});
        let config: Config = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(json!(config), wire);
    }

    #[test]
    fn limits_follow_what_the_server_accepts() {
        assert_eq!(limits("15", " 6 ", "80"), Ok(Limits { mem_free_pct: 15, working: 6, cpu_pct: 80 }));
        assert_eq!(limits("101", "6", "80"), Err("placement_limit_pct_invalid"));
        assert_eq!(limits("15", "6", "abc"), Err("placement_limit_pct_invalid"));
        assert_eq!(limits("15", "0", "80"), Err("placement_limit_working_invalid"));
    }

    #[test]
    fn a_schedule_rule_needs_time_zone_and_machine() {
        let nb = "nb".to_owned();
        let rule = schedule_rule(&[5, 1], "09:00", "18:00", "America/Sao_Paulo", Some(&nb)).unwrap();
        assert_eq!((rule.days, rule.avoid), (vec![1, 5], nb.clone()));
        assert_eq!(schedule_rule(&[], "9h", "18:00", "America/Sao_Paulo", Some(&nb)), Err("placement_schedule_time_invalid"));
        assert_eq!(schedule_rule(&[], "09:00", "18:00", "Lua/Base", Some(&nb)), Err("placement_schedule_tz_invalid"));
        assert_eq!(schedule_rule(&[], "09:00", "18:00", "UTC", None), Err("placement_schedule_machine_missing"));
    }

    #[test]
    fn each_machine_says_how_its_rules_compare_with_this_one() {
        let here = Config { ui_mode: "suggest".into(), ..Default::default() };
        let other = Config { ui_mode: "off".into(), ..Default::default() };
        assert_eq!(status(&Read::Ready(here.clone()), Some(&here)), "placement_machine_same");
        assert_eq!(status(&Read::Ready(other), Some(&here)), "placement_machine_different");
        assert_eq!(status(&Read::Old, Some(&here)), "placement_machine_old");
        assert_eq!(status(&Read::Failed("x".into()), Some(&here)), "placement_machine_silent");
    }
}
