//! Atualização conjunta: cada servidor conserva seu resultado e o app só reabre após os demais terminarem.
use super::*;
use super::confirm::{Acceptance, Launch, Notes};
use crate::{app::ServerEntry, i18n::tr_shared};
use gpui_kit::prelude::FluentBuilder;
use std::{cell::Cell, collections::HashSet, rc::Rc};

#[derive(Clone, Debug, PartialEq)]
enum Classification { Start, Current, Held(Hold) }

fn classify(snapshot: &Value) -> Classification {
    if !behind(snapshot) { return Classification::Current; }
    hold(snapshot).map_or(Classification::Start, Classification::Held)
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum Status {
    Checking, Starting, Running { step: u64, total: u64, text: String }, Waiting,
    Available, Current, Held(Hold), Offline(String), Updated, UpdatedManual, Failed(String), Silent, Timeout,
}

impl Status {
    fn busy(&self) -> bool { matches!(self, Self::Checking | Self::Starting | Self::Running { .. } | Self::Waiting) }

    fn text(&self) -> String {
        let key = match self {
            Self::Checking => "update_all_checking", Self::Starting => "update_all_starting",
            Self::Running { step, total, text } => return tr_shared("update_all_running",
                &[("texto", text), ("passo", &step.to_string()), ("total", &total.to_string())]),
            Self::Waiting => "update_all_waiting", Self::Available => "update_all_available", Self::Current => "update_all_up_to_date",
            Self::Held(Hold::WorkBranch(..)) => "update_all_held_branch",
            Self::Held(Hold::LocalChanges) => "update_all_held_changes",
            Self::Held(Hold::Running) => "update_all_held_running",
            Self::Held(Hold::Missing(_)) => "update_all_held_missing",
            Self::Held(Hold::ChannelDraft) => return tr("settings_channel_pending"),
            Self::Offline(_) => "update_all_offline", Self::Updated => "update_all_updated",
            Self::UpdatedManual => "update_all_updated_manual",
            Self::Failed(error) => return tr_shared("update_all_failed", &[("erro", error)]),
            Self::Silent | Self::Timeout => {
                let reason = if matches!(self, Self::Silent) { "update_all_silent" } else { "update_all_timeout" };
                return tr_shared("update_all_failed", &[("erro", &tr_shared(reason, &[]))]);
            }
        };
        tr_shared(key, &[])
    }
}

fn snapshot_status(snapshot: &Value) -> Status {
    match classify(snapshot) {
        Classification::Start => Status::Available,
        Classification::Current => Status::Current,
        Classification::Held(hold) => Status::Held(hold),
    }
}

fn follow_status(state: &Value, baseline: Option<&str>) -> Status {
    // A rodada conjunta exige o ts posterior mesmo depois de ter visto o motor rodando.
    match follow(state, false, baseline) {
        Follow::Running { step, total, text } => Status::Running { step, total, text },
        Follow::Waiting | Follow::NotStarted => Status::Waiting,
        Follow::Done(Outcome::Updated) => Status::Updated,
        Follow::Done(Outcome::UpdatedManual) => Status::UpdatedManual,
        Follow::Done(Outcome::Failed(error)) => Status::Failed(error),
    }
}

fn limit_status(status: Option<&Status>, silent: u32, elapsed: Duration) -> Option<Status> {
    let step = match status {
        Some(status) if !status.busy() => Some(Follow::Done(Outcome::Updated)),
        Some(_) => Some(Follow::Waiting),
        None => None,
    };
    match follow_limit(step.as_ref(), silent, elapsed) {
        Some("update_silent") => Some(Status::Silent),
        Some(_) => Some(Status::Timeout),
        None => None,
    }
}

fn warnings(state: &Value) -> Vec<String> {
    state["avisos"].as_array().map(|list| list.iter().filter_map(Value::as_str).map(str::to_owned).collect()).unwrap_or_default()
}

fn failure_text(error: &Failure) -> String { tr(&error.detail) }

fn batch_machines(machines: Vec<Machine>, excluded: &HashSet<String>) -> Vec<Machine> {
    machines.into_iter().filter(|m| !excluded.contains(&m.key)).collect()
}

fn own_servers(servers: &[ServerEntry]) -> Vec<ServerEntry> {
    servers.iter().filter(|s| !s.disabled && !s.invite && !s.ephemeral).cloned().collect()
}

#[derive(Clone)]
struct Machine { key: String, entry_id: String, label: String, api: Result<Api, Failure>, local: bool }

impl Machine {
    fn from_entry(entry: &ServerEntry, local: Option<&Api>) -> Self {
        let api = Api::new(&entry.address, &entry.token);
        let key = api.as_ref().map(Api::identity).unwrap_or_else(|_| entry.address.clone());
        let local = local.is_some_and(|api| api.identity() == key);
        Self { key, entry_id: entry.id.clone(), label: entry.label.clone(), api, local }
    }
}

async fn identify(machine: &Machine) -> String {
    let result = match &machine.api {
        Ok(api) => api.server_read(&["peers", "identificador"], &[], 15).await,
        Err(error) => Err(error.clone()),
    };
    match result {
        Ok(value) => value["identificador"].as_str().unwrap_or("").trim().to_owned(),
        Err(error) => {
            crate::log_line(&format!("update_all: identificador indisponível ({:?})", error.status));
            String::new()
        }
    }
}

fn unique_machines(machines: Vec<Machine>, identities: &[String]) -> Vec<Machine> {
    let local_identity = machines.iter().zip(identities).find(|(m, _)| m.local).map(|(_, id)| id.as_str()).filter(|id| !id.is_empty());
    let mut seen = HashSet::new();
    machines.iter().zip(identities).filter(|(machine, id)| {
        if !machine.local && local_identity == Some(id.as_str()) { return false; }
        id.is_empty() || seen.insert((*id).clone())
    }).map(|(machine, _)| machine.clone()).collect()
}

#[derive(Clone, Debug, PartialEq)]
struct Row { key: String, label: String, local: bool, status: Status, warnings: Vec<String> }

impl Row {
    fn text(&self) -> String { self.describe(self.status.text()) }

    fn describe(&self, head: String) -> String {
        let mut parts = vec![head];
        match &self.status {
            Status::Held(Hold::WorkBranch(branch, target)) => parts.push(format!("{branch} → {target}")),
            Status::Held(Hold::Missing(what)) => parts.push(what.clone()),
            Status::Offline(error) => parts.push(error.clone()),
            _ => {}
        }
        if !self.warnings.is_empty() {
            parts.push(format!("{}: {}", tr_shared("update_all_warnings", &[]), self.warnings.join("; ")));
        }
        parts.join(" · ")
    }
}

#[derive(Default)]
pub(super) struct Survey { rows: Vec<Row>, aliases: HashMap<String, String>, generation: u64, from_round: bool, pub(super) pending: bool }

impl Survey {
    fn prepare(&mut self, machines: &[Machine], mut aliases: HashMap<String, String>) {
        // Reconsultar mantém o que a linha já mostrava até a resposta nova chegar, sem piscar "procurando".
        let previous = std::mem::take(&mut self.rows);
        self.rows = machines.iter().map(|m| previous.iter().find(|row| row.key == m.key).cloned()
            .unwrap_or_else(|| Row { key: m.key.clone(), label: m.label.clone(), local: m.local,
                status: Status::Checking, warnings: Vec::new() })).collect();
        for machine in machines.iter().filter(|m| !m.entry_id.is_empty()) {
            aliases.insert(machine.entry_id.clone(), machine.key.clone());
        }
        self.aliases = aliases;
    }

    fn change(&mut self, key: &str, status: Status, warnings: Vec<String>) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.key == key) { row.status = status; row.warnings = warnings; }
    }

    fn forget(&mut self, key: &str) { self.rows.retain(|row| row.key != key); }

    fn text(&self, key: &str) -> Option<String> {
        let key = self.aliases.get(key).map(String::as_str).unwrap_or(key);
        let row = self.rows.iter().find(|row| row.key == key)?;
        // Só consultada, a trava é o motivo de não poder atualizar, não o desfecho de uma tentativa.
        let key = match (&row.status, self.from_round) {
            (Status::Held(Hold::WorkBranch(..)), false) => "update_all_blocked_branch",
            (Status::Held(Hold::LocalChanges), false) => "update_all_blocked_changes",
            (Status::Held(Hold::Running), false) => "update_all_blocked_running",
            (Status::Held(Hold::Missing(_)), false) => "update_all_blocked_missing",
            _ => return Some(row.text()),
        };
        Some(row.describe(tr_shared(key, &[])))
    }
}

fn remote_available(count: usize, rows: &[Row]) -> bool {
    count >= 2 && rows.iter().any(|row| !row.local && row.status == Status::Available)
}

fn idle_visible(plan: &Plan, count: usize, rows: &[Row]) -> bool { plan.visible() || remote_available(count, rows) }

#[derive(Default)]
struct Progress { rows: Vec<Row>, preparing: bool, deferred_app: bool }

impl Progress {
    fn busy(&self) -> bool { self.preparing || self.rows.iter().any(|row| row.status.busy()) || self.deferred_app }
    fn remote_busy(&self) -> bool { self.preparing || self.rows.iter().any(|row| !row.local && row.status.busy()) }

    fn change(&mut self, key: &str, status: Status, warnings: Vec<String>) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.key == key) { row.status = status; row.warnings = warnings; }
    }
}

impl Render for Progress {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().gap_3().text_sm()
            .when(self.preparing, |el| el.child(tr_shared("update_all_checking", &[])))
            .when(!self.preparing && self.rows.is_empty(), |el| el.child(tr("search_no_machines")))
            .children(self.rows.iter().map(|row| {
                let color = match row.status {
                    Status::Failed(_) | Status::Silent | Status::Timeout => theme::danger(),
                    Status::Held(_) | Status::Offline(_) | Status::UpdatedManual => theme::warning(),
                    _ => theme::muted(),
                };
                div().id(SharedString::from(format!("update-all-{}", row.key))).flex().flex_col().gap_1().min_w_0()
                    .child(div().font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(row.label.clone()))
                    .child(div().text_color(color).whitespace_normal().child(row.status.text()))
                    .when_some(match &row.status {
                        Status::Held(hold) => Some(hold.text()), Status::Offline(error) => Some(error.clone()), _ => None,
                    }, |el, detail| el.child(div().text_color(theme::muted()).whitespace_normal().child(detail)))
                    .when(!row.warnings.is_empty(), |el| el.child(div().text_color(theme::warning()).whitespace_normal()
                        .child(tr_shared("update_all_warnings", &[])).children(row.warnings.clone())))
            }))
    }
}

pub(super) struct Batch { progress: Entity<Progress> }

enum Event { Prepared(Vec<Machine>, HashMap<String, String>), Changed(String, Status, Vec<String>), Unknown(String) }

async fn prepare_machines(machines: Vec<Machine>) -> (Vec<Machine>, HashMap<String, String>) {
    let identities = futures::future::join_all(machines.iter().map(identify)).await;
    let unique = unique_machines(machines.clone(), &identities);
    let canonical: HashMap<_, _> = machines.iter().zip(&identities)
        .filter(|(m, id)| !id.is_empty() && unique.iter().any(|u| u.key == m.key))
        .map(|(m, id)| (id.clone(), m.key.clone())).collect();
    let aliases = machines.iter().zip(&identities).flat_map(|(m, id)| {
        let key = canonical.get(id).cloned().unwrap_or_else(|| m.key.clone());
        [(m.key.clone(), key.clone()), (m.entry_id.clone(), key)]
    }).filter(|(alias, _)| !alias.is_empty()).collect();
    (unique, aliases)
}

// Consulta que falha não é ação que falhou: a linha some do resumo e o motivo vai só ao log.
async fn read_machine(machine: &Machine) -> Option<Status> {
    let result = match &machine.api {
        Ok(api) => api.server_read(&["atualizacao"], &[], 20).await,
        Err(error) => Err(error.clone()),
    };
    match result {
        Ok(snapshot) => Some(snapshot_status(&snapshot)),
        Err(error) => { eprintln!("update survey: {} unreadable: {}", machine.label, failure_text(&error)); None }
    }
}

/// Os avisos de cada máquina do lote, antes do modal: a desta (primeiro) com a versão, as novidades e o estado lido; as
/// outras só com os avisos.
async fn load_all(machines: Vec<Machine>) -> (Option<Value>, Vec<Notes>) {
    let (machines, _) = prepare_machines(machines).await;
    let mut read = futures::future::join_all(machines.into_iter().map(|m| async move {
        match &m.api {
            Ok(api) if m.local => { let (state, notes) = confirm::load(api.clone()).await; (true, state, notes.labeled(m.label)) }
            _ => (false, None, confirm::load_warnings(m.api.clone(), m.key, m.label).await),
        }
    })).await;
    read.sort_by_key(|(local, ..)| !local);
    let state = read.iter_mut().find_map(|(_, state, _)| state.take());
    (state, read.into_iter().map(|(_, _, notes)| notes).collect())
}

async fn run_machine(machine: Machine, acceptance: Acceptance, mut publish: impl FnMut(Status, Vec<String>)) {
    let mut previous = None;
    let mut publish = |status: Status, warnings: Vec<String>| {
        let value = (status, warnings);
        if previous.as_ref() != Some(&value) { publish(value.0.clone(), value.1.clone()); previous = Some(value); }
    };
    publish(Status::Checking, Vec::new());
    let api = match machine.api {
        Ok(api) => api,
        Err(error) => { publish(Status::Failed(failure_text(&error)), Vec::new()); return; }
    };
    let snapshot = match api.server_read(&["atualizacao"], &[("procurar", "1")], 120).await {
        Ok(value) => value,
        Err(error) => {
            let text = failure_text(&error);
            publish(if error.status.is_some() { Status::Failed(text) } else { Status::Offline(text) }, Vec::new());
            return;
        }
    };
    match classify(&snapshot) {
        Classification::Current => { publish(Status::Current, Vec::new()); return; }
        Classification::Held(hold) => { publish(Status::Held(hold), Vec::new()); return; }
        Classification::Start => {}
    }
    let baseline = snapshot["estado"]["ts"].as_str();
    let started = std::time::Instant::now();
    publish(Status::Starting, Vec::new());
    match confirm::accept_then_start(&api, &acceptance, real_update(&snapshot)).await {
        Launch::Started(Ok(value)) if value["ok"].as_bool() == Some(true) => {}
        Launch::Started(Ok(_)) => { publish(Status::Failed(tr("invalid_response")), Vec::new()); return; }
        Launch::Started(Err(error)) => { publish(Status::Failed(failure_text(&error)), Vec::new()); return; }
        Launch::AcceptFailed(text) => { publish(Status::Failed(text), Vec::new()); return; }
        // Só o aviso estava pendente: aceito, não há versão a puxar.
        Launch::Nothing => { publish(Status::Current, Vec::new()); return; }
    }
    let (mut silent, mut latest_warnings) = (0, Vec::new());
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let status = match api.server_read(&["atualizacao"], &[], 10).await {
            Ok(value) => {
                silent = 0;
                latest_warnings = warnings(&value["estado"]);
                Some(follow_status(&value["estado"], baseline))
            }
            Err(error) if matches!(error.status, Some(401 | 403)) => Some(Status::Failed(failure_text(&error))),
            Err(_) => { silent += 1; None }
        };
        let limit = limit_status(status.as_ref(), silent, started.elapsed());
        let status = limit.or(status).unwrap_or(Status::Waiting);
        let done = !status.busy();
        publish(status, latest_warnings.clone());
        if done { return; }
    }
}

impl Updater {
    pub(crate) fn set_update_all_servers(&mut self, servers: Vec<ServerEntry>, cx: &mut Context<Self>) -> bool {
        let servers = own_servers(&servers);
        if self.all_servers == servers { return false; }
        self.all_servers = servers;
        self.survey.generation += 1;
        cx.notify();
        true
    }

    pub(super) fn update_all_count(&self) -> usize { self.all_servers.len() }

    fn all_machines(&self) -> Vec<Machine> {
        let mut machines: Vec<_> = self.all_servers.iter().map(|entry| Machine::from_entry(entry, self.local.as_ref())).collect();
        if let Some(api) = self.local.clone().filter(|api| !machines.iter().any(|m| m.key == api.identity())) {
            machines.insert(0, Machine { key: api.identity(), entry_id: String::new(), label: api.identity(), api: Ok(api), local: true });
        }
        machines
    }

    pub(crate) fn refresh_machine_updates(&mut self, cx: &mut Context<Self>) {
        // Ocupado, a consulta espera: o observador de `update::start` a roda quando o atualizador fica ocioso.
        if self.busy() { self.survey.pending = true; return; }
        self.survey.pending = false;
        self.survey.generation += 1;
        let generation = self.survey.generation;
        let machines = self.all_machines();
        self.survey.prepare(&machines, HashMap::new());
        cx.notify();
        let (tx, rx) = async_channel::unbounded();
        self.runtime.spawn(async move {
            let (machines, aliases) = prepare_machines(machines).await;
            if tx.send(Event::Prepared(machines.clone(), aliases)).await.is_err() { return; }
            futures::future::join_all(machines.into_iter().map(|machine| {
                let tx = tx.clone();
                async move { let event = match read_machine(&machine).await {
                        Some(status) => Event::Changed(machine.key, status, Vec::new()),
                        None => Event::Unknown(machine.key),
                    };
                    let _ = tx.send(event).await; }
            })).await;
        });
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let result = this.update(cx, |this, cx| {
                    if this.survey.generation != generation { return false; }
                    match event {
                        Event::Prepared(machines, aliases) => { this.survey.from_round = false; this.survey.prepare(&machines, aliases) }
                        Event::Changed(key, status, warnings) => this.survey.change(&key, status, warnings),
                        Event::Unknown(key) => this.survey.forget(&key),
                    }
                    cx.notify();
                    true
                });
                if !matches!(result, Ok(true)) { break; }
            }
        }).detach();
    }

    pub(crate) fn machine_update_text(&self, api: &Api) -> Option<String> { self.survey.text(&api.identity()) }

    pub(crate) fn machine_entry_update_text(&self, id: &str) -> Option<String> {
        self.all_servers.iter().any(|s| s.id == id).then(|| self.survey.text(id)).flatten()
    }

    pub(crate) fn render_machine_update_button(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        (self.all_running || remote_available(self.update_all_count(), &self.survey.rows)).then(|| Button::new("machines-update-all").outline().small()
            .label(tr_shared("update_all_short", &[])).icon(IconName::Download)
            .tooltip(tr_shared("update_all_button", &[("n", &self.update_all_count().to_string())]))
            .loading(self.all_running).disabled(self.busy())
            // O mesmo modal do topo: os avisos de cada máquina são aceitos antes de ela atualizar.
            .on_click(cx.listener(|this, _, window, cx| { if !this.busy() { this.run(window, cx); } }))
            .into_any_element())
    }

    pub(super) fn idle_visible(&self, plan: &Plan) -> bool { idle_visible(plan, self.update_all_count(), &self.survey.rows) }

    /// Lê os avisos de todas as máquinas antes de abrir o modal do lote.
    pub(super) fn confirm_all_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.run = Run::Searching;
        cx.notify();
        let task = self.runtime.spawn(load_all(self.all_machines()));
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let (state, groups) = task.await.unwrap_or_default();
            let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| {
                this.run = Run::Idle;
                if state.is_some() { this.receive_server(state, cx); }
                cx.notify();
                this.confirm_all(groups, window, cx);
            }); });
        }).detach();
    }

    /// `groups`: os avisos de cada máquina; a que não deu para ler fica de fora do lote.
    fn confirm_all(&mut self, groups: Vec<Notes>, window: &mut Window, cx: &mut Context<Self>) {
        let all = Rc::new(Cell::new(true));
        let selected = all.clone();
        let this = cx.entity().downgrade();
        let plan = self.plan();
        let (title, description) = if matches!(plan.server, ServerStep::Update) {
            if plan.app { (tr("app_update_confirm_title"), tr("app_update_confirm_desc")) }
            else { (tr("app_update_confirm_server_title"), tr("app_update_confirm_server_desc")) }
        } else { (tr_shared("update_all_title", &[]), tr("update_confirm_desc")) };
        let servers: Vec<String> = groups.iter().map(|n| n.server().to_owned()).collect();
        let excluded: HashSet<String> = groups.iter().filter(|n| n.excluded()).map(|n| n.server().to_owned()).collect();
        let local = self.local.as_ref().map(Api::identity);
        super::confirm::open(window, cx, title, description, groups,
            Some((tr_shared("update_all_button", &[("n", &self.update_all_count().to_string())]), all)), move |accepted, window, _| {
                let this = this.clone();
                let all = selected.get();
                let accepted: HashMap<String, Acceptance> = servers.iter().cloned().zip(accepted).collect();
                let local_acceptance = local.as_ref().and_then(|key| accepted.get(key)).cloned().unwrap_or_default();
                let excluded = excluded.clone();
                // O alerta fecha antes de o modal de progresso abrir.
                window.on_next_frame(move |window, cx| { let _ = this.update(cx, |this, cx| {
                    if this.busy() { return; }
                    if all { this.start_all(accepted, excluded, window, cx); }
                    else { match this.plan().server {
                        ServerStep::Update => this.search_server(local_acceptance, window, cx),
                        _ => this.run_local(Notes::default(), window, cx),
                    } }
                }); });
            });
    }

    /// `accepted`: o aceite de cada máquina, pela chave; `excluded`: as que não tiveram os avisos lidos.
    fn start_all(&mut self, accepted: HashMap<String, Acceptance>, excluded: HashSet<String>, window: &mut Window, cx: &mut Context<Self>) {
        let machines = batch_machines(self.all_machines(), &excluded);
        let mut local_acceptance = machines.iter().find(|m| m.local).and_then(|m| accepted.get(&m.key)).cloned();
        self.survey.generation += 1;
        self.survey.prepare(&machines, HashMap::new());
        let progress = cx.new(|_| Progress { preparing: true, ..Progress::default() });
        self.batch = Some(Batch { progress });
        self.all_running = true;
        cx.notify();
        self.open_batch(window, cx);
        let (tx, rx) = async_channel::unbounded();
        self.runtime.spawn(async move {
            let (machines, aliases) = prepare_machines(machines).await;
            if tx.send(Event::Prepared(machines.clone(), aliases)).await.is_err() { return; }
            futures::future::join_all(machines.into_iter().filter(|m| !m.local).map(|machine| {
                let tx = tx.clone();
                let key = machine.key.clone();
                let acceptance = accepted.get(&key).cloned().unwrap_or_default();
                run_machine(machine, acceptance, move |status, warnings| { let _ = tx.try_send(Event::Changed(key.clone(), status, warnings)); })
            })).await;
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| {
                    let Some(batch) = &this.batch else { return };
                    match event {
                        Event::Prepared(machines, aliases) => {
                            let local = machines.iter().any(|m| m.local);
                            this.survey.prepare(&machines, aliases);
                            batch.progress.update(cx, |p, cx| {
                                p.preparing = false;
                                p.rows = machines.into_iter().map(|m| Row { key: m.key, label: m.label, local: m.local,
                                    status: Status::Checking, warnings: Vec::new() }).collect();
                                cx.notify();
                            });
                            if local { this.search_server(local_acceptance.take().unwrap_or_default(), window, cx); }
                            else if this.plan().app { this.install_app(window, cx); }
                        }
                        Event::Changed(key, status, warnings) => batch.progress.update(cx, |p, cx| { p.change(&key, status, warnings); cx.notify(); }),
                        Event::Unknown(_) => {}
                    }
                    this.finish_batch(window, cx);
                }); });
            }
        }).detach();
    }

    pub(super) fn open_batch(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch) = &self.batch else { return };
        let progress = batch.progress.clone();
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, _| crate::app::popup::dialog(dialog)
            .w(window.rem_size() * 30.).title(tr_shared("update_all_title", &[])).child(progress.clone())
            .on_close({ let weak = weak.clone(); move |_, _, cx| { let _ = weak.update(cx, |this, cx| {
                if !this.busy() { this.batch = None; cx.notify(); }
            }); } }));
    }

    fn finish_batch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch) = &self.batch else { return };
        let install = batch.progress.update(cx, |p, cx| {
            if p.deferred_app && !p.remote_busy() { p.deferred_app = false; cx.notify(); true } else { false }
        });
        self.survey.rows = batch.progress.read(cx).rows.clone(); self.survey.from_round = true;
        self.all_running = batch.progress.read(cx).busy();
        cx.notify();
        if install { self.install_app(window, cx); }
        else if !self.all_running && self.survey.rows.iter().any(|r| r.local && matches!(r.status, Status::Updated | Status::UpdatedManual)) {
            super::confirm::show_pending(false, window, cx);
        }
    }

    pub(super) fn local_batch_status(&mut self, status: Status, cx: &mut Context<Self>) {
        let Some(batch) = &self.batch else { return };
        batch.progress.update(cx, |p, cx| {
            if let Some(row) = p.rows.iter_mut().find(|r| r.local) { row.status = status; }
            cx.notify();
        });
        self.survey.rows = batch.progress.read(cx).rows.clone(); self.survey.from_round = true;
        self.all_running = batch.progress.read(cx).busy();
        cx.notify();
    }

    pub(super) fn local_batch_snapshot(&mut self, snapshot: &Value, cx: &mut Context<Self>) {
        let status = if self.is_channel_blocked() { Status::Held(Hold::ChannelDraft) } else { match classify(snapshot) {
            Classification::Current => Status::Current,
            Classification::Held(hold) => Status::Held(hold),
            Classification::Start => Status::Starting,
        } };
        self.local_batch_status(status, cx);
    }

    pub(super) fn batch_plan(&self) -> Plan {
        let current = self.plan();
        // A oferta do app não pode impedir a atualização elegível de um servidor do lote.
        let server = if self.is_channel_blocked() { ServerStep::Held(Hold::ChannelDraft) }
            else { plan(None, self.server.as_ref(), self.server_api).server };
        Plan { server, app: current.app }
    }

    pub(super) fn local_batch_read(&mut self, read: &Result<Value, Failure>, baseline: Option<&str>, cx: &mut Context<Self>) {
        let Some(batch) = &self.batch else { return };
        batch.progress.update(cx, |p, cx| {
            if let Some(row) = p.rows.iter_mut().find(|r| r.local) {
                row.status = read.as_ref().map(|v| follow_status(&v["estado"], baseline)).unwrap_or(Status::Waiting);
                if let Ok(value) = read { row.warnings = warnings(&value["estado"]); }
            }
            cx.notify();
        });
        self.survey.rows = batch.progress.read(cx).rows.clone(); self.survey.from_round = true;
        self.all_running = batch.progress.read(cx).busy();
        cx.notify();
    }

    pub(super) fn defer_batch_app(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(batch) = &self.batch else { return false };
        if !batch.progress.read(cx).remote_busy() { return false; }
        batch.progress.update(cx, |p, cx| { p.deferred_app = true; cx.notify(); });
        self.run = Run::Idle;
        self.all_running = true;
        cx.notify();
        true
    }

    pub(super) fn render_batch_button(&self, cx: &mut Context<Self>) -> AnyElement {
        Button::new("topbar-update-all").ghost().small().label(tr_shared("update_all_title", &[]))
            .icon(IconName::Download).on_click(cx.listener(|this, _, window, cx| this.open_batch(window, cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;
    use serde_json::json;

    #[test]
    fn update_all_shared_fixture_matches_classification_and_follow() {
        let fixture: Value = serde_json::from_str(include_str!("../../../packages/core/src/updateAll.fixtures.json")).unwrap();
        for case in fixture["classify"].as_array().unwrap() {
            let actual = match classify(&case["estado_get"]) {
                Classification::Start => "iniciar",
                Classification::Current => "em_dia",
                Classification::Held(Hold::WorkBranch(..)) => "travado:branch",
                Classification::Held(Hold::LocalChanges) => "travado:mudancas",
                Classification::Held(Hold::Running) => "travado:rodando",
                Classification::Held(Hold::Missing(_)) => "travado:falta",
                Classification::Held(Hold::ChannelDraft) => panic!("o canal local não participa do pré-voo remoto"),
            };
            assert_eq!(actual, case["esperado"].as_str().unwrap(), "{}", case["nome"]);
        }
        for case in fixture["follow"].as_array().unwrap() {
            let actual = match follow_status(&case["estado"], case["base_ts"].as_str()) {
                Status::Running { .. } => "rodando", Status::Waiting => "esperando",
                Status::Updated => "atualizado", Status::UpdatedManual => "atualizado_manual", Status::Failed(_) => "falhou",
                other => panic!("{other:?}"),
            };
            assert_eq!(actual, case["esperado"].as_str().unwrap(), "{}", case["nome"]);
        }
    }

    fn entry(id: &str) -> ServerEntry {
        ServerEntry { id: id.into(), label: id.into(), address: format!("http://{id}:8765"), token: "synthetic-token".into(), ..ServerEntry::default() }
    }

    #[test]
    fn update_all_filters_entries_and_requires_two_before_identity_lookup() {
        let mut invite = entry("invite"); invite.invite = true;
        let mut ephemeral = entry("ephemeral"); ephemeral.ephemeral = true;
        let mut disabled = entry("disabled"); disabled.disabled = true;
        let list = own_servers(&[entry("local"), invite, ephemeral, disabled]);
        assert_eq!(list.len(), 1);
        assert_eq!(own_servers(&[entry("local"), entry("remote")]).len(), 2);
    }

    #[test]
    fn update_all_deduplicates_identities_preferring_local_and_keeps_unknowns() {
        let machine = |name: &str, local| { let mut m = Machine::from_entry(&entry(name), None); m.local = local; m };
        let machines = vec![machine("alias", false), machine("local", true), machine("remote", false),
            machine("remote-alias", false), machine("unknown-a", false), machine("unknown-b", false)];
        let ids = ["local", "local", "remote", "remote", "", ""].map(str::to_owned);
        let list = unique_machines(machines, &ids);
        assert_eq!(list.iter().map(|m| m.label.as_str()).collect::<Vec<_>>(), ["local", "remote", "unknown-a", "unknown-b"]);
    }

    #[test]
    fn update_all_limits_waiting_without_discarding_a_finished_result() {
        assert_eq!(limit_status(None, SILENT_READS - 1, Duration::ZERO), None);
        assert_eq!(limit_status(None, SILENT_READS, Duration::ZERO), Some(Status::Silent));
        let running = Status::Running { step: 8, total: 9, text: "fixture-step".into() };
        assert_eq!(limit_status(Some(&running), 0, Duration::from_secs(700)), None);
        assert_eq!(limit_status(Some(&running), 0, FOLLOW_CEILING), Some(Status::Timeout));
        assert_eq!(limit_status(Some(&Status::Waiting), 0, FOLLOW_CEILING), Some(Status::Timeout));
        assert_eq!(limit_status(Some(&Status::Updated), SILENT_READS, FOLLOW_CEILING), None);
        assert_eq!(follow_status(&json!({"fase":"pronto", "ok":true, "ts":"old"}), Some("old")), Status::Waiting);
    }

    #[test]
    fn update_all_local_app_waits_for_remote_outcomes() {
        let mut p = Progress { preparing: false, deferred_app: true, rows: vec![
            Row { key: "local".into(), label: "local".into(), local: true, status: Status::Updated, warnings: vec![] },
            Row { key: "remote".into(), label: "remote".into(), local: false, status: Status::Waiting, warnings: vec![] },
        ] };
        assert!(p.remote_busy() && p.busy());
        p.change("remote", Status::Failed("fixture-error".into()), vec!["fixture-warning".into()]);
        assert!(!p.remote_busy());
        assert!(p.busy(), "o app local ainda aguarda a troca");
        p.deferred_app = false;
        assert!(!p.busy());
        assert_eq!(p.rows[1].warnings, ["fixture-warning"]);
    }

    #[test]
    fn update_all_labels_are_shared_in_both_languages() {
        for source in [include_str!("../../../messages/pt.json"), include_str!("../../../messages/en.json")] {
            let dict: Value = serde_json::from_str(source).unwrap();
            for key in ["button", "title", "short", "available", "checking", "starting", "running", "waiting", "up_to_date", "held_branch",
                "held_changes", "held_running", "held_missing", "offline", "updated", "updated_manual", "failed", "silent", "timeout", "warnings"] {
                assert!(dict[format!("update_all_{key}")].as_str().is_some(), "{key}");
            }
        }
        crate::i18n::english_in_this_thread(false);
        assert_eq!(Status::Running { step: 2, total: 9, text: "fixture-step".into() }.text(), "fixture-step (2/9)");
        assert_eq!(Status::Failed("fixture-error".into()).text(), "Falhou: fixture-error");
        assert!(!Status::Silent.text().contains("update_all_"));
        assert!(!Status::Timeout.text().contains("update_all_"));
    }

    fn mock(responses: Vec<(&'static str, u16, Value)>) -> (Machine, std::thread::JoinHandle<()>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let machine = Machine { key: address.clone(), entry_id: String::new(), label: address.clone(), local: false, api: Api::new(&address, "synthetic-token") };
        let thread = std::thread::spawn(move || {
            for (path, status, body) in responses {
                loop {
                    let start = std::time::Instant::now();
                    let mut socket = loop {
                        match listener.accept() {
                            Ok((socket, _)) => break socket,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && start.elapsed() < Duration::from_secs(15) =>
                                std::thread::sleep(Duration::from_millis(5)),
                            Err(error) => panic!("requisição ausente: {error}"),
                        }
                    };
                    socket.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut request = Vec::new();
                    while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                        let mut bytes = [0; 1024];
                        let read = socket.read(&mut bytes).unwrap();
                        assert!(read > 0);
                        request.extend_from_slice(&bytes[..read]);
                    }
                    let request = String::from_utf8(request).unwrap();
                    // Sondas locais de porta não fazem parte da sequência do motor.
                    if request.starts_with("HEAD / ") {
                        write!(socket, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
                        continue;
                    }
                    assert!(request.starts_with(path), "{}", request.lines().next().unwrap());
                    assert!(request.to_lowercase().contains("authorization: bearer synthetic-token"));
                    let body = body.to_string();
                    write!(socket, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                    break;
                }
            }
        });
        (machine, thread)
    }

    #[test]
    fn update_all_executor_keeps_warnings_and_ignores_old_result_and_pid() {
        let (machine, server) = mock(vec![
            ("GET /api/atualizacao?procurar=1 ", 200, json!({"atualizacao_disponivel": true, "estado":{"ts":"old"}})),
            ("POST /api/atualizacao/iniciar ", 200, json!({"ok":true,"pid":123})),
            ("GET /api/atualizacao ", 200, json!({"estado":{"fase":"pronto","ok":true,"ts":"old"}})),
            ("GET /api/atualizacao ", 200, json!({"estado":{"fase":"rodando","passo":3,"total":9,"texto":"fixture-step"}})),
            ("GET /api/atualizacao ", 200, json!({"estado":{"fase":"pronto","ok":true,"ts":"new","pid":999,"reiniciar_manual":true,"avisos":["fixture-warning"]}})),
        ]);
        let mut states = Vec::new();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(async {
            tokio::time::timeout(Duration::from_secs(20), run_machine(machine, Acceptance::default(), |status, warnings| states.push((status, warnings)))).await
        });
        server.join().unwrap();
        result.expect("o acompanhamento simulado deve terminar em 20 segundos");
        assert!(states.iter().any(|(s, _)| *s == Status::Waiting));
        assert!(states.iter().any(|(s, _)| matches!(s, Status::Running { step:3, total:9, .. })));
        assert_eq!(states.last(), Some(&(Status::UpdatedManual, vec!["fixture-warning".into()])));
    }

    #[test]
    fn update_all_executor_failure_does_not_stop_another_machine() {
        let (failed, first) = mock(vec![("GET /api/atualizacao?procurar=1 ", 503, json!({"detail":"fixture-failure"}))]);
        let (ready, second) = mock(vec![("GET /api/atualizacao?procurar=1 ", 200, json!({"atualizacao_disponivel":false}))]);
        let (mut a, mut b) = (Vec::new(), Vec::new());
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            tokio::join!(run_machine(failed, Acceptance::default(), |s, _| a.push(s)), run_machine(ready, Acceptance::default(), |s, _| b.push(s)));
        });
        first.join().unwrap(); second.join().unwrap();
        assert!(matches!(a.last(), Some(Status::Failed(error)) if error.contains("fixture-failure")));
        assert_eq!(b.last(), Some(&Status::Current));
    }

    #[test]
    fn update_all_accepts_the_warning_and_starts_only_a_real_update() {
        let acceptance = Acceptance { ids: vec!["2026-10-24-pacote-por-versao".into()], mode: Some("package") };
        let runtime = tokio::runtime::Runtime::new().unwrap();
        // Só o aviso pendente: aceito, nada de `iniciar`.
        let (machine, server) = mock(vec![
            ("GET /api/atualizacao?procurar=1 ", 200, json!({"atualizacao_disponivel": true, "aviso_pendente": true, "atras": 0})),
            ("POST /api/update/accept ", 200, json!({"ok": true, "started": false})),
        ]);
        let mut states = Vec::new();
        runtime.block_on(run_machine(machine, acceptance.clone(), |s, _| states.push(s)));
        server.join().unwrap();
        assert_eq!(states.last(), Some(&Status::Current));
        // O aceite lançou a migração: segue o pid dele, sem `iniciar`.
        let (machine, server) = mock(vec![
            ("GET /api/atualizacao?procurar=1 ", 200, json!({"atualizacao_disponivel": true, "aviso_pendente": true, "atras": 0, "estado": {"ts": "old"}})),
            ("POST /api/update/accept ", 200, json!({"ok": true, "started": true, "pid": 7})),
            ("GET /api/atualizacao ", 200, json!({"estado": {"fase": "pronto", "ok": true, "ts": "new"}})),
        ]);
        let mut states = Vec::new();
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(10), run_machine(machine, acceptance.clone(), |s, _| states.push(s))).await })
            .expect("o acompanhamento deve terminar");
        server.join().unwrap();
        assert_eq!(states.last(), Some(&Status::Updated));
        // Aceite recusado: nada é iniciado.
        let (machine, server) = mock(vec![
            ("GET /api/atualizacao?procurar=1 ", 200, json!({"atualizacao_disponivel": true, "atras": 3})),
            ("POST /api/update/accept ", 500, json!({"detail": "fixture-accept"})),
        ]);
        let mut states = Vec::new();
        runtime.block_on(run_machine(machine, acceptance, |s, _| states.push(s)));
        server.join().unwrap();
        assert!(matches!(states.last(), Some(Status::Failed(error)) if error.contains("fixture-accept")), "{states:?}");
    }

    #[test]
    fn update_all_leaves_out_a_machine_whose_warnings_did_not_load() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (down, first) = mock(vec![("GET /api/update/notes ", 503, json!({"detail": "fixture-unavailable"}))]);
        let (old, second) = mock(vec![("GET /api/update/notes ", 404, json!({"detail": "Not Found"}))]);
        let read = |m: &Machine| confirm::load_warnings(m.api.clone(), m.key.clone(), m.label.clone());
        let (a, b) = runtime.block_on(async { tokio::join!(read(&down), read(&old)) });
        first.join().unwrap(); second.join().unwrap();
        assert!(a.excluded(), "leitura que falha deixa a máquina de fora");
        assert!(!b.excluded(), "servidor antigo: sem avisos, segue no lote");
        let excluded: HashSet<String> = [&a, &b].into_iter().filter(|n| n.excluded()).map(|n| n.server().to_owned()).collect();
        let kept = batch_machines(vec![down.clone(), old.clone()], &excluded);
        assert_eq!(kept.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), [old.key.as_str()]);
    }

    #[test]
    fn update_all_pending_warning_alone_is_not_a_real_update() {
        assert!(behind(&json!({"atualizacao_disponivel": true, "aviso_pendente": true, "atras": 0})), "o botão aparece");
        assert!(!real_update(&json!({"atualizacao_disponivel": true, "aviso_pendente": true, "atras": 0})));
        assert!(real_update(&json!({"atualizacao_disponivel": true, "aviso_pendente": true, "atras": 2})));
        assert!(real_update(&json!({"atualizacao_disponivel": true, "aviso_pendente": true, "atras": 0,
            "versoes": {"repo": "b", "backend": "a"}})), "disco diferente do que está no ar");
        assert!(real_update(&json!({"atualizacao_disponivel": true})), "sem a marca, a regra de antes");
        assert!(!real_update(&json!({"atualizacao_disponivel": false})));
    }

    #[test]
    fn update_all_review_follow_auth_failure_finishes_immediately() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for status in [401, 403] {
            let (machine, server) = mock(vec![
                ("GET /api/atualizacao?procurar=1 ", 200, json!({"atualizacao_disponivel":true,"estado":{"ts":"old"}})),
                ("POST /api/atualizacao/iniciar ", 200, json!({"ok":true,"pid":123})),
                ("GET /api/atualizacao ", status, json!({"detail":"fixture-auth-error"})),
            ]);
            let mut states = Vec::new();
            let result = runtime.block_on(async {
                tokio::time::timeout(Duration::from_secs(8), run_machine(machine, Acceptance::default(), |s, _| states.push(s))).await
            });
            server.join().unwrap();
            result.expect("a recusa de autenticação deve encerrar na primeira leitura");
            assert!(matches!(states.last(), Some(Status::Failed(error)) if error.contains("fixture-auth-error")), "{states:?}");
            assert_eq!(states.len(), 3, "consulta, início e falha, sem esperar outra leitura");
        }
    }

    #[test]
    fn update_all_machine_bar_requires_an_available_remote() {
        let row = |local, status| Row { key: "fixture".into(), label: "fixture".into(), local, status, warnings: vec![] };
        let hidden = Plan { server: ServerStep::Skip, app: false };
        assert!(!idle_visible(&hidden, 2, &[]), "cadastros sem consulta não justificam o botão");
        for status in [Status::Current, Status::Held(Hold::LocalChanges), Status::Offline("offline".into()),
            Status::Checking, Status::Updated, Status::Failed("failure".into())] {
            assert!(!idle_visible(&hidden, 2, &[row(false, status)]));
        }
        assert!(!idle_visible(&hidden, 2, &[row(true, Status::Available)]));
        assert!(!idle_visible(&hidden, 1, &[row(false, Status::Available)]));
        assert!(idle_visible(&hidden, 2, &[row(false, Status::Available)]));
        for visible in [Plan { server: ServerStep::Skip, app: true }, Plan { server: ServerStep::Update, app: false },
            Plan { server: ServerStep::Held(Hold::Running), app: false }] {
            assert!(idle_visible(&visible, 1, &[]), "preserva o plano local");
        }
    }

    #[test]
    fn update_all_machine_snapshot_text_uses_existing_classification() {
        crate::i18n::english_in_this_thread(false);
        for (snapshot, expected) in [
            (json!({"atualizacao_disponivel":true}), "Atualização disponível"),
            (json!({"atualizacao_disponivel":false}), "Em dia"),
            (json!({"atualizacao_disponivel":true,"pre_voo":{"sujo":1}}), "Não atualizou: há mudanças locais ou commits não publicados."),
            (json!({"atualizacao_disponivel":true,"pre_voo":{"branch_de_trabalho":true,"branch":"feature","alvo":"main"}}),
                "Não atualizou: está numa branch de trabalho."),
            (json!({"atualizacao_disponivel":true,"estado":{"fase":"rodando"}}), "Não atualizou: já há uma atualização em curso."),
            (json!({"atualizacao_disponivel":true,"pre_voo":{"pode":false,"faltando":["git"]}}), "Não atualizou: faltam requisitos para atualizar."),
        ] {
            assert_eq!(snapshot_status(&snapshot).text(), expected);
        }
        assert_eq!(Status::Offline("connection refused".into()).text(), "Fora do ar");
        assert_eq!(Status::Checking.text(), "Procurando atualizações…");
        assert_eq!(Status::Updated.text(), "Atualizado");
        assert_eq!(Status::UpdatedManual.text(), "Atualizado; reinicie o servidor manualmente.");
    }

    #[test]
    fn update_all_machine_row_keeps_the_remote_reason_and_outcome() {
        crate::i18n::english_in_this_thread(false);
        let mut survey = Survey::default();
        survey.aliases.insert("alias".into(), "remote".into());
        survey.rows.push(Row { key: "remote".into(), label: "remote".into(), local: false,
            status: Status::Held(Hold::WorkBranch("feature".into(), "main".into())), warnings: vec![] });
        assert_eq!(survey.text("alias").as_deref(), Some("Não pode atualizar: está numa branch de trabalho. · feature → main"));
        survey.change("remote", Status::Held(Hold::Missing("git".into())), vec![]);
        assert_eq!(survey.text("alias").as_deref(), Some("Não pode atualizar: faltam requisitos. · git"));
        survey.from_round = true;
        assert_eq!(survey.text("alias").as_deref(), Some("Não atualizou: faltam requisitos para atualizar. · git"));
        survey.change("remote", Status::Running { step: 2, total: 9, text: "fixture-step".into() }, vec![]);
        assert_eq!(survey.text("alias").as_deref(), Some("fixture-step (2/9)"));
        survey.change("remote", Status::Updated, vec!["fixture-warning".into()]);
        assert_eq!(survey.text("alias").as_deref(), Some("Atualizado · Avisos: fixture-warning"));
        assert_eq!(survey.text("unknown"), None);
    }

    #[test]
    fn update_all_machine_survey_reads_cached_state_without_starting() {
        let (machine, server) = mock(vec![("GET /api/atualizacao ", 200, json!({"atualizacao_disponivel":true}))]);
        let status = Runtime::new().unwrap().block_on(read_machine(&machine));
        server.join().unwrap();
        assert_eq!(status, Some(Status::Available));
        let (machine, server) = mock(vec![("GET /api/atualizacao ", 503, json!({"detail":"fixture-unavailable"}))]);
        let status = Runtime::new().unwrap().block_on(read_machine(&machine));
        server.join().unwrap();
        assert_eq!(status, None, "consulta que falha não vira linha de erro");
    }

    #[test]
    fn update_all_machine_survey_keeps_rows_on_refresh_and_forgets_unreadable() {
        crate::i18n::english_in_this_thread(false);
        let machine = |key: &str| Machine { key: key.into(), entry_id: String::new(), label: key.into(),
            api: Err(Failure { status: None, detail: "fixture".into(), retry_after: None, uncertain: false, code: None }), local: false };
        let mut survey = Survey::default();
        survey.prepare(&[machine("a"), machine("b")], HashMap::new());
        survey.change("a", Status::Available, vec![]);
        survey.prepare(&[machine("a"), machine("b")], HashMap::new());
        assert_eq!(survey.text("a").as_deref(), Some("Atualização disponível"), "reconsultar não volta a 'procurando'");
        survey.forget("b");
        assert_eq!(survey.text("b"), None);
    }

    #[gpui::test]
    fn update_all_machine_registry_change_keeps_running_aliases(cx: &mut gpui::TestAppContext) {
        let updater = cx.new(|_| Updater { runtime: Arc::new(Runtime::new().unwrap()), client: reqwest::Client::new(), exe: None, exe_stamp: None, offer: None,
            local: None, server: None, server_api: None, server_seq: 0, active: None, active_state: None, active_api: None, active_seq: 0,
            run: Run::Idle, checking: false, checked: None, checked_channel: None, known_channel: None, channel_blocked: None,
            suspended: false, intent_lock: None, all_servers: vec![entry("local"), entry("remote")], survey: Survey::default(), batch: None, all_running: true });
        updater.update(cx, |this, cx| {
            let machine = Machine::from_entry(&entry("remote"), None);
            this.survey.prepare(&[machine.clone()], HashMap::from([("alias".into(), machine.key.clone())]));
            this.survey.change(&machine.key, Status::Running { step: 2, total: 9, text: "fixture-step".into() }, vec![]);
            this.set_update_all_servers(vec![entry("local"), entry("remote"), entry("added")], cx);
            assert_eq!(this.machine_entry_update_text("remote").as_deref(), Some("fixture-step (2/9)"));
            assert_eq!(this.survey.text("alias").as_deref(), Some("fixture-step (2/9)"));
            this.set_update_all_servers(vec![entry("local")], cx);
            assert_eq!(this.update_all_count(), 1, "cadastro removido não participa do próximo lote");
            assert_eq!(this.machine_entry_update_text("remote"), None);
        });
    }

    #[test]
    fn update_all_review_idle_button_names_all_machines_without_a_local_offer() {
        crate::i18n::english_in_this_thread(false);
        let mut updater = Updater { runtime: Arc::new(Runtime::new().unwrap()), client: reqwest::Client::new(), exe: None, exe_stamp: None, offer: None,
            local: None, server: None, server_api: None, server_seq: 0, active: None, active_state: None, active_api: None, active_seq: 0,
            run: Run::Idle, checking: false, checked: None, checked_channel: None, known_channel: None, channel_blocked: None,
            suspended: false, intent_lock: None, all_servers: vec![entry("local"), entry("remote")], survey: Survey::default(), batch: None, all_running: false };
        assert!(!updater.plan().visible());
        assert!(!updater.idle_visible(&updater.plan()));
        updater.survey.rows.push(Row { key: "remote".into(), label: "remote".into(), local: false, status: Status::Available, warnings: vec![] });
        assert_eq!(updater.idle_texts(&updater.plan()), ("Atualizar todas as máquinas (2)".into(), "Atualizar todas as máquinas".into()));
        updater.all_servers.push(entry("other"));
        assert_eq!(updater.idle_texts(&updater.plan()).0, "Atualizar todas as máquinas (3)");
    }
}
