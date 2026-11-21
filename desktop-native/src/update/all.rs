//! Atualização conjunta: o modal abre na hora com uma linha por máquina, cada uma lida uma vez só (estado e avisos), e
//! acompanha cada servidor e o app desta máquina até o fim, que ele diz. O app só reabre após os demais terminarem.
use super::*;
use super::confirm::{Acceptance, Launch, Notes};
use crate::{app::{ServerEntry, chrome}, i18n::tr_shared};
use gpui_kit::{component::dialog::DialogFooter, prelude::FluentBuilder};
use std::{cell::Cell, collections::HashSet, rc::Rc};

/// Máquina que não diz quem é nesse prazo está fora: o modal não espera por ela o prazo longo da busca.
const IDENTIFY_SECS: u64 = 5;

#[derive(Clone, Debug, PartialEq)]
enum Classification { Start, Current, Held(Hold) }

fn classify(snapshot: &Value) -> Classification {
    if !behind(snapshot) { return Classification::Current; }
    update_hold(snapshot).map_or(Classification::Start, Classification::Held)
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

/// Só consultada, a trava é o motivo de não poder atualizar, não o desfecho de uma tentativa.
fn blocked_key(status: &Status) -> Option<&'static str> {
    match status {
        Status::Held(Hold::WorkBranch(..)) => Some("update_all_blocked_branch"),
        Status::Held(Hold::LocalChanges) => Some("update_all_blocked_changes"),
        Status::Held(Hold::Running) => Some("update_all_blocked_running"),
        Status::Held(Hold::Missing(_)) => Some("update_all_blocked_missing"),
        _ => None,
    }
}

fn snapshot_status(snapshot: &Value) -> Status {
    match classify(snapshot) {
        Classification::Start => Status::Available,
        Classification::Current => Status::Current,
        Classification::Held(hold) => Status::Held(hold),
    }
}

/// A linha de uma máquina depois da leitura da abertura. Sem resposta nenhuma ela está fora do ar e fica de fora; o erro
/// que o servidor deu é falha dela.
fn read_status(snapshot: &Result<Value, Failure>) -> Status {
    match snapshot {
        Ok(snapshot) => snapshot_status(snapshot),
        Err(error) if error.status.is_none() => Status::Offline(failure_text(error)),
        Err(error) => Status::Failed(failure_text(error)),
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

/// Só os representantes que a leitura dos avisos escolheu (`read`), menos os sem avisos lidos. Refazer a escolha aqui
/// pode eleger outro endereço da mesma máquina (identidade que falhou numa das vezes), que rodaria sem o aceite.
/// Uma por chave: endereços que se escrevem diferente podem dar a mesma chave, e a linha repetida nunca sai de "procurando".
fn batch_machines<T>(machines: Vec<Machine>, read: &HashMap<String, T>, excluded: &HashSet<String>) -> Vec<Machine> {
    let mut seen = HashSet::new();
    machines.into_iter().filter(|m| read.contains_key(&m.key) && !excluded.contains(&m.key) && seen.insert(m.key.clone())).collect()
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

/// A identidade da máquina; `None` quando ela não respondeu nada (fora do ar). Erro do servidor dá identidade vazia.
async fn identify(machine: &Machine) -> Option<String> {
    let result = match &machine.api {
        Ok(api) => api.server_read(&["peers", "identificador"], &[], IDENTIFY_SECS).await,
        Err(error) => Err(error.clone()),
    };
    match result {
        Ok(value) => Some(value["identificador"].as_str().unwrap_or("").trim().to_owned()),
        Err(error) => {
            crate::log_line(&format!("update_all: identificador indisponível ({:?})", error.status));
            (error.status.is_some() || machine.api.is_err()).then(String::new)
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
    fn new(machine: &Machine, status: Status) -> Self {
        Self { key: machine.key.clone(), label: machine.label.clone(), local: machine.local, status, warnings: Vec::new() }
    }

    fn text(&self) -> String { self.describe(self.status.text()) }

    fn describe(&self, head: String) -> String {
        let mut parts = vec![head];
        match &self.status {
            Status::Held(Hold::WorkBranch(branch, target)) => parts.push(format!("{branch} → {target}")),
            Status::Held(Hold::Missing(what)) => parts.push(what.clone()),
            Status::Offline(error) if !error.is_empty() => parts.push(error.clone()),
            _ => {}
        }
        if !self.warnings.is_empty() {
            parts.push(format!("{}: {}", tr_shared("update_all_warnings", &[]), self.warnings.join("; ")));
        }
        parts.join(" · ")
    }
}

#[derive(Default)]
pub(super) struct Survey {
    rows: Vec<Row>, aliases: HashMap<String, String>, generation: u64, from_round: bool, pub(super) pending: bool,
    /// Entradas (`ServerEntry::id`) que o app sabe fora do ar, pela lista de cada máquina.
    offline: HashSet<String>,
}

impl Survey {
    fn prepare(&mut self, machines: &[Machine], mut aliases: HashMap<String, String>) {
        // Reconsultar mantém o que a linha já mostrava até a resposta nova chegar, sem piscar "procurando".
        let previous = std::mem::take(&mut self.rows);
        self.rows = machines.iter().map(|m| previous.iter().find(|row| row.key == m.key).cloned()
            .unwrap_or_else(|| Row::new(m, Status::Checking))).collect();
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
        match blocked_key(&row.status).filter(|_| !self.from_round) {
            Some(key) => Some(row.describe(tr_shared(key, &[]))),
            None => Some(row.text()),
        }
    }
}

fn remote_available(count: usize, rows: &[Row]) -> bool {
    count >= 2 && rows.iter().any(|row| !row.local && row.status == Status::Available)
}

fn idle_visible(plan: &Plan, count: usize, rows: &[Row]) -> bool { plan.visible() || remote_available(count, rows) }

/// A máquina e como anda a atualização dela.
pub(crate) fn status_line(label: String, status: String, color: Hsla) -> Div {
    div().flex().flex_col().gap_1().min_w_0()
        .child(div().font_weight(FontWeight::SEMIBOLD).whitespace_normal().child(label))
        .child(div().text_color(color).whitespace_normal().child(status))
}

/// A linha do app desta máquina no lote. `Hidden`: o app não é trocado nesta rodada (servidor travado ou que falhou, ou
/// release que passou a exigir mais), e a linha sai em vez de ficar esperando.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum AppStep { Waiting, Downloading { version: String, done: u64, total: Option<u64> }, Installing, Current, Failed(String), Hidden }

impl AppStep {
    fn busy(&self) -> bool { matches!(self, Self::Downloading { .. } | Self::Installing) }

    fn fraction(&self) -> Option<f32> {
        match self {
            Self::Downloading { done, total: Some(total), .. } if *total > 0 => Some((*done as f32 / *total as f32).min(1.)),
            _ => None,
        }
    }

    /// Quanto esta linha já andou, para a barra do modal.
    fn settled(&self) -> f32 {
        match self { Self::Waiting => 0., Self::Downloading { .. } => self.fraction().unwrap_or(0.), _ => 1. }
    }

    fn text(&self) -> String {
        let mb = |bytes: u64| format!("{:.0}", bytes as f64 / 1_048_576.);
        match self {
            Self::Waiting => tr_shared("update_all_app_waiting", &[]),
            Self::Downloading { version, done, total: Some(total) } if *total > 0 => tr_shared("update_all_app_downloading", &[
                ("version", version), ("percent", &(done.saturating_mul(100) / total).min(100).to_string()), ("done", &mb(*done)), ("total", &mb(*total))]),
            Self::Downloading { version, .. } => tr_shared("update_all_app_downloading_plain", &[("version", version)]),
            Self::Installing => tr_shared("update_all_app_installing", &[]),
            Self::Current => tr_shared("update_all_app_current", &[]),
            Self::Failed(error) => tr_shared("update_all_failed", &[("erro", error)]),
            Self::Hidden => String::new(),
        }
    }
}

/// `Review`: lendo cada máquina e esperando o Atualizar. `Running`: acompanhando a rodada.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Phase { Review, Running }

/// O que a abertura leu de uma máquina: o estado da busca (`procurar=1`), que a rodada usa sem buscar de novo, e os avisos.
struct Read { snapshot: Result<Value, Failure>, notes: Notes }

pub(super) struct Progress {
    phase: Phase,
    rows: Vec<Row>,
    /// As identidades ainda não voltaram: as linhas são as das entradas, uma por endereço.
    preparing: bool,
    deferred_app: bool,
    app: Option<AppStep>,
    machines: Vec<Machine>,
    aliases: HashMap<String, String>,
    reads: HashMap<String, Read>,
    waiting_reads: usize,
    /// O app desta máquina tem versão nova: conta como algo a fazer mesmo com os servidores em dia.
    app_offer: bool,
    understood: Rc<Cell<bool>>,
    fixes_open: Rc<Cell<bool>>,
    developer: HashMap<String, Rc<Cell<bool>>>,
    all: Option<(String, Rc<Cell<bool>>)>,
    started: std::time::Instant,
}

fn plural(kind: &str, n: usize) -> String {
    tr_shared(&format!("update_all_count_{kind}_{}", if n == 1 { "one" } else { "other" }), &[("n", &n.to_string())])
}

impl Progress {
    fn new(phase: Phase, rows: Vec<Row>) -> Self {
        Self { phase, rows, preparing: phase == Phase::Review, deferred_app: false, app: None, machines: Vec::new(), aliases: HashMap::new(),
            reads: HashMap::new(), waiting_reads: 0, app_offer: false, understood: Rc::default(), fixes_open: Rc::default(),
            developer: HashMap::new(), all: None, started: std::time::Instant::now() }
    }

    fn busy(&self) -> bool {
        self.preparing || self.rows.iter().any(|row| row.status.busy()) || self.deferred_app || self.app.as_ref().is_some_and(AppStep::busy)
    }

    fn remote_busy(&self) -> bool { self.preparing || self.rows.iter().any(|row| !row.local && row.status.busy()) }

    fn change(&mut self, key: &str, status: Status, warnings: Vec<String>) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.key == key) { row.status = status; row.warnings = warnings; }
    }

    fn loaded(&self) -> bool { !self.preparing && self.waiting_reads == 0 }

    fn done(&self) -> bool { self.phase == Phase::Running && !self.busy() }

    /// Endereços da mesma máquina viram uma linha só; as que não disseram quem são ficam fora do ar e de fora.
    fn identified(&mut self, machines: Vec<Machine>, aliases: HashMap<String, String>, down: &HashSet<String>) {
        let keep: HashSet<&str> = machines.iter().map(|m| m.key.as_str()).collect();
        self.rows.retain(|row| keep.contains(row.key.as_str()) || matches!(row.status, Status::Offline(_)));
        for row in self.rows.iter_mut().filter(|row| down.contains(&row.key)) { row.status = Status::Offline(String::new()); }
        self.waiting_reads = machines.iter().filter(|m| !down.contains(&m.key)).count();
        self.machines = machines;
        self.aliases = aliases;
        self.preparing = false;
    }

    fn record(&mut self, key: String, snapshot: Result<Value, Failure>, notes: Notes, status: Status) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.key == key) { row.status = status; }
        self.developer.entry(key.clone()).or_default();
        self.reads.insert(key, Read { snapshot, notes });
        self.waiting_reads = self.waiting_reads.saturating_sub(1);
    }

    /// Os avisos lidos, na ordem das linhas e com esta máquina primeiro (ela dá as novidades); a fora do ar não entra.
    fn shown_notes(&self) -> (Vec<Notes>, Vec<Rc<Cell<bool>>>) {
        let mut rows: Vec<&Row> = self.rows.iter().filter(|row| !matches!(row.status, Status::Offline(_))).collect();
        rows.sort_by_key(|row| !row.local);
        rows.into_iter().filter_map(|row| Some((self.reads.get(&row.key)?.notes.clone(), self.developer.get(&row.key)?.clone()))).unzip()
    }

    /// Há o que fazer: uma máquina com versão nova cujos avisos foram lidos, ou o app desta máquina.
    fn actionable(&self) -> bool {
        self.app_offer || self.rows.iter().any(|row| row.status == Status::Available
            && self.reads.get(&row.key).is_some_and(|read| !read.notes.excluded()))
    }

    fn can_start(&self) -> bool {
        self.phase == Phase::Review && self.loaded() && self.actionable()
            && (!self.shown_notes().0.iter().any(Notes::warned) || self.understood.get())
    }

    fn counted(&self) -> (usize, usize) {
        let taking = self.rows.iter().filter(|row| !matches!(row.status, Status::Offline(_)));
        (taking.clone().filter(|row| !row.status.busy()).count(), taking.count())
    }

    fn app_shown(&self) -> Option<&AppStep> { self.app.as_ref().filter(|app| **app != AppStep::Hidden) }

    fn summary(&self) -> String {
        match self.phase {
            Phase::Review if !self.loaded() => tr_shared("update_all_summary_checking", &[("n", &self.counted().1.to_string())]),
            Phase::Review if self.actionable() => tr_shared("update_all_summary_ready", &[]),
            Phase::Review => tr_shared("update_all_up_to_date", &[]),
            Phase::Running if self.done() => self.outcome(),
            Phase::Running if !self.rows.iter().any(|row| row.status.busy()) => tr_shared("update_all_summary_app", &[]),
            Phase::Running => {
                let (done, total) = self.counted();
                tr_shared("update_all_summary_progress", &[("done", &done.to_string()), ("total", &total.to_string())])
            }
        }
    }

    fn outcome(&self) -> String {
        let count = |test: fn(&Status) -> bool| self.rows.iter().filter(|row| test(&row.status)).count();
        let updated = count(|s| matches!(s, Status::Updated | Status::UpdatedManual));
        let current = count(|s| *s == Status::Current);
        let offline = count(|s| matches!(s, Status::Offline(_)));
        let issue = self.rows.len() - updated - current - offline + usize::from(matches!(self.app, Some(AppStep::Failed(_))));
        let head = tr_shared(if issue == 0 { "update_all_done_ok" } else { "update_all_done_issues" }, &[]);
        let parts: Vec<String> = [("updated", updated), ("current", current), ("offline", offline), ("issue", issue)].into_iter()
            .filter(|(_, n)| *n > 0).map(|(kind, n)| plural(kind, n)).collect();
        if parts.is_empty() { head } else { format!("{head} · {}", parts.join(", ")) }
    }

    fn fraction(&self) -> f32 {
        let (done, total) = self.counted();
        let value = match self.phase {
            Phase::Review if self.loaded() => 1.,
            Phase::Review => {
                let read = self.rows.iter().filter(|row| !matches!(row.status, Status::Offline(_) | Status::Checking)).count();
                if total == 0 { 1. } else { read as f32 / total as f32 }
            }
            Phase::Running => {
                let (app_done, app_total) = self.app_shown().map_or((0., 0.), |app| (app.settled(), 1.));
                let total = total as f32 + app_total;
                if total == 0. { 1. } else { (done as f32 + app_done) / total }
            }
        };
        value.clamp(0., 1.)
    }
}

fn row_text(row: &Row, phase: Phase) -> String {
    match (&row.status, phase) {
        (Status::Offline(_), Phase::Review) => tr_shared("update_all_offline_skip", &[]),
        (Status::Offline(_), Phase::Running) => tr_shared("update_all_offline_skipped", &[]),
        (status, Phase::Review) => blocked_key(status).map_or_else(|| status.text(), |key| tr_shared(key, &[])),
        (status, Phase::Running) => status.text(),
    }
}

fn spinner(id: String) -> AnyElement {
    chrome::Spinner::new(SharedString::from(id), IconName::LoaderCircle, px(16.), theme::accent()).into_any_element()
}

fn row_icon(row: &Row) -> AnyElement {
    if row.status.busy() { return spinner(format!("update-all-spin-{}", row.key)); }
    let (icon, color) = match row.status {
        Status::Failed(_) | Status::Silent | Status::Timeout => (IconName::CircleX, theme::danger()),
        Status::Held(_) | Status::Offline(_) | Status::UpdatedManual => (IconName::CircleAlert, theme::warning()),
        _ => (IconName::CircleCheck, theme::success()),
    };
    chrome::small_icon(icon, 16., color).into_any_element()
}

fn icon_slot(icon: AnyElement) -> Div {
    div().size(px(18.)).mt(px(1.)).flex_shrink_0().flex().items_center().justify_center().child(icon)
}

fn bar(fraction: f32, height: f32) -> Div {
    div().h(px(height)).w_full().rounded_full().bg(theme::border())
        .child(div().h_full().rounded_full().bg(theme::accent()).w(relative(fraction)))
}

fn row_view(row: &Row, phase: Phase) -> Div {
    let color = match row.status {
        Status::Failed(_) | Status::Silent | Status::Timeout => theme::danger(),
        Status::Held(_) | Status::Offline(_) | Status::UpdatedManual => theme::warning(),
        _ => theme::muted(),
    };
    div().flex().items_start().gap(px(10.))
        .child(icon_slot(row_icon(row)))
        .child(status_line(row.label.clone(), row_text(row, phase), color)
            .when_some(match &row.status { Status::Held(hold) => Some(hold.text()), _ => None },
                |el, detail| el.child(div().text_color(theme::muted()).whitespace_normal().child(detail)))
            .when(!row.warnings.is_empty(), |el| el.child(div().text_color(theme::warning()).whitespace_normal()
                .child(tr_shared("update_all_warnings", &[])).children(row.warnings.clone()))))
}

fn app_view(app: &AppStep) -> Div {
    let icon = match app {
        AppStep::Downloading { .. } | AppStep::Installing => spinner("update-all-spin-app".into()),
        AppStep::Waiting => chrome::small_icon(IconName::Circle, 16., theme::faint()).into_any_element(),
        AppStep::Failed(_) => chrome::small_icon(IconName::CircleX, 16., theme::danger()).into_any_element(),
        _ => chrome::small_icon(IconName::CircleCheck, 16., theme::success()).into_any_element(),
    };
    let color = if matches!(app, AppStep::Failed(_)) { theme::danger() } else { theme::muted() };
    div().flex().items_start().gap(px(10.)).p(px(12.)).rounded(px(10.)).bg(theme::accent().alpha(0.10))
        .child(icon_slot(icon))
        .child(div().flex_1().min_w_0().child(status_line(tr_shared("update_all_app_label", &[]), app.text(), color))
            .when_some(app.fraction(), |el, fraction| el.child(bar(fraction, 3.).mt(px(8.)))))
}

impl Render for Progress {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let phase = self.phase;
        let (notes, choices) = if phase == Phase::Review && self.loaded() { self.shown_notes() } else { Default::default() };
        let extra = if notes.is_empty() { Vec::new() } else {
            confirm::notes_body(&notes, &choices, &self.understood, &self.fixes_open, self.all.as_ref(), self.started, window, cx)
        };
        div().id("update-all-body").max_h(window.viewport_size().height * 0.7).overflow_y_scroll()
            .flex().flex_col().gap_3().text_sm()
            .child(div().text_color(theme::muted()).whitespace_normal().child(self.summary()))
            .child(bar(self.fraction(), 4.))
            .when(!self.preparing && self.rows.is_empty(), |el| el.child(tr("search_no_machines")))
            .children(self.rows.iter().map(|row| row_view(row, phase)))
            .when_some(self.app_shown().filter(|_| phase == Phase::Running), |el, app| el.child(app_view(app)))
            .children(extra)
    }
}

pub(super) struct Batch { progress: Entity<Progress>, generation: u64 }

enum Event { Prepared(Vec<Machine>, HashMap<String, String>), Changed(String, Status, Vec<String>), Unknown(String) }

enum ReviewEvent { Identified(Vec<Machine>, HashMap<String, String>, HashSet<String>), Read(String, Result<Value, Failure>, Notes) }

/// Uma entrada por máquina, os apelidos de cada endereço e as que não responderam à identificação (`down`).
async fn prepare_machines(machines: Vec<Machine>) -> (Vec<Machine>, HashMap<String, String>, HashSet<String>) {
    let identities = futures::future::join_all(machines.iter().map(identify)).await;
    let down: HashSet<String> = machines.iter().zip(&identities).filter(|(_, id)| id.is_none()).map(|(m, _)| m.key.clone()).collect();
    let identities: Vec<String> = identities.into_iter().map(Option::unwrap_or_default).collect();
    let unique = unique_machines(machines.clone(), &identities);
    let canonical: HashMap<_, _> = machines.iter().zip(&identities)
        .filter(|(m, id)| !id.is_empty() && unique.iter().any(|u| u.key == m.key))
        .map(|(m, id)| (id.clone(), m.key.clone())).collect();
    let aliases = machines.iter().zip(&identities).flat_map(|(m, id)| {
        let key = canonical.get(id).cloned().unwrap_or_else(|| m.key.clone());
        [(m.key.clone(), key.clone()), (m.entry_id.clone(), key)]
    }).filter(|(alias, _)| !alias.is_empty()).collect();
    (unique, aliases, down)
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

/// A leitura da abertura: esta máquina com a versão, as novidades e o estado; as outras com os avisos e o estado.
async fn read_for_review(machine: Machine) -> (String, Result<Value, Failure>, Notes) {
    match &machine.api {
        Ok(api) if machine.local => {
            let (state, notes) = confirm::load(api.clone()).await;
            (machine.key, state.ok_or_else(|| Failure::local(tr_shared("update_all_silent", &[]))), notes.labeled(machine.label))
        }
        _ => {
            let (snapshot, notes) = confirm::load_warnings(machine.api.clone(), machine.key.clone(), machine.label).await;
            (machine.key, snapshot, notes)
        }
    }
}

/// O registro pode mudar com o modal aberto: segue só a entrada lida que ainda existe, com os dados de agora (token).
fn still_registered(targets: Vec<Machine>, now: &[Machine]) -> Vec<Machine> {
    targets.into_iter().filter_map(|t| now.iter().find(|m| m.key == t.key && m.entry_id == t.entry_id).cloned()).collect()
}

/// A busca (`procurar=1`) foi feita na abertura e não se repete aqui.
async fn run_machine(machine: Machine, acceptance: Acceptance, mut publish: impl FnMut(Status, Vec<String>)) {
    let mut previous = None;
    let mut publish = |status: Status, warnings: Vec<String>| {
        let value = (status, warnings);
        if previous.as_ref() != Some(&value) { publish(value.0.clone(), value.1.clone()); previous = Some(value); }
    };
    let api = match machine.api {
        Ok(api) => api,
        Err(error) => { publish(Status::Failed(failure_text(&error)), Vec::new()); return; }
    };
    // A abertura pode ter minutos: o `ts` do último desfecho e a versão vêm de uma releitura rápida, sem buscar de novo.
    let snapshot = match api.server_read(&["atualizacao"], &[], 20).await {
        Ok(fresh) => fresh,
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
    let (mut silent, mut latest_warnings, mut pending, mut failed_at) = (0, Vec::new(), false, None);
    loop {
        tokio::time::sleep(Duration::from_secs(2)).await;
        let status = match api.server_read(&["atualizacao"], &[], 10).await {
            Ok(value) => {
                silent = 0;
                latest_warnings = warnings(&value["estado"]);
                pending = super::report_pending(&value["estado"]);
                Some(follow_status(&value["estado"], baseline))
            }
            Err(error) if matches!(error.status, Some(401 | 403)) => Some(Status::Failed(failure_text(&error))),
            Err(_) => { silent += 1; None }
        };
        let limit = limit_status(status.as_ref(), silent, started.elapsed());
        let status = limit.or(status).unwrap_or(Status::Waiting);
        let report_wait = matches!(status, Status::Failed(_)) && pending
            && failed_at.get_or_insert_with(std::time::Instant::now).elapsed() < super::REPORT_WAIT;
        let done = !status.busy() && !report_wait;
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

    /// As entradas que o app sabe fora do ar; o "Atualizar todas" as deixa de fora sem esperar o prazo de cada uma.
    pub(crate) fn set_update_all_offline(&mut self, entries: HashSet<String>) { self.survey.offline = entries; }

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
            let (machines, aliases, down) = prepare_machines(machines).await;
            if tx.send(Event::Prepared(machines.clone(), aliases)).await.is_err() { return; }
            futures::future::join_all(machines.into_iter().map(|machine| {
                let tx = tx.clone();
                let down = down.contains(&machine.key);
                async move { let event = match if down { None } else { read_machine(&machine).await } {
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

    /// Abre o modal na hora, com uma linha por máquina; a que o app sabe fora do ar já sai de fora, e as outras se
    /// preenchem conforme respondem. A busca (`procurar=1`) feita aqui é a que a rodada usa.
    pub(super) fn confirm_all_machines(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let machines = self.all_machines();
        let known_down = |m: &Machine| !m.local && self.survey.offline.contains(&m.entry_id);
        let rows: Vec<Row> = machines.iter().map(|m| Row::new(m, if known_down(m) { Status::Offline(String::new()) } else { Status::Checking }))
            .collect();
        let reach: Vec<Machine> = machines.into_iter().filter(|m| !known_down(m)).collect();
        let all_label = tr_shared("update_all_button", &[("n", &self.update_all_count().to_string())]);
        let app_offer = self.plan().app;
        let progress = cx.new(|_| {
            let mut progress = Progress::new(Phase::Review, rows);
            (progress.all, progress.app_offer) = (Some((all_label, Rc::new(Cell::new(true)))), app_offer);
            progress
        });
        self.survey.generation += 1;
        let generation = self.survey.generation;
        self.batch = Some(Batch { progress, generation });
        self.run = Run::Searching;
        cx.notify();
        self.open_batch(window, cx);
        let (tx, rx) = async_channel::unbounded();
        self.runtime.spawn(async move {
            let (machines, aliases, down) = prepare_machines(reach).await;
            let readable: Vec<Machine> = machines.iter().filter(|m| !down.contains(&m.key)).cloned().collect();
            if tx.send(ReviewEvent::Identified(machines, aliases, down)).await.is_err() { return; }
            futures::future::join_all(readable.into_iter().map(|machine| {
                let tx = tx.clone();
                async move {
                    let (key, snapshot, notes) = read_for_review(machine).await;
                    let _ = tx.send(ReviewEvent::Read(key, snapshot, notes)).await;
                }
            })).await;
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let alive = handle.update(cx, |_, window, cx| this.update(cx, |this, cx| this.review_event(generation, event, window, cx))
                    .unwrap_or(false)).unwrap_or(false);
                if !alive { break; }
            }
        }).detach();
    }

    fn review_event(&mut self, generation: u64, event: ReviewEvent, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let Some(progress) = self.batch.as_ref().filter(|b| b.generation == generation).map(|b| b.progress.clone()) else { return false };
        match event {
            ReviewEvent::Identified(machines, aliases, down) => progress.update(cx, |p, cx| { p.identified(machines, aliases, &down); cx.notify(); }),
            ReviewEvent::Read(key, snapshot, notes) => {
                let local = self.local.as_ref().is_some_and(|api| api.identity() == key);
                if local && let Ok(state) = &snapshot { self.receive_server(Some(state.clone()), cx); }
                let status = if local && snapshot.is_ok() && self.is_channel_blocked() { Status::Held(Hold::ChannelDraft) } else { read_status(&snapshot) };
                let app_offer = self.plan().app;
                progress.update(cx, |p, cx| { p.record(key, snapshot, notes, status); p.app_offer = app_offer; cx.notify(); });
            }
        }
        if progress.read(cx).loaded() && matches!(self.run, Run::Searching) { self.run = Run::Idle; }
        cx.notify();
        window.refresh();
        true
    }

    /// O Atualizar do modal: todas as máquinas, ou só esta quando a caixa de todas foi desmarcada.
    fn start_from_review(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(progress) = self.batch.as_ref().map(|b| b.progress.clone()) else { return };
        if !progress.read(cx).can_start() { return; }
        let (notes, choices) = progress.read(cx).shown_notes();
        let accepted: HashMap<String, Acceptance> = notes.iter().map(|n| n.server().to_owned()).zip(confirm::acceptances(&notes, &choices)).collect();
        if progress.read(cx).all.as_ref().is_none_or(|(_, all)| all.get()) { return self.start_all(accepted, window, cx); }
        let local = self.local.as_ref().map(Api::identity);
        let acceptance = local.as_ref().and_then(|key| accepted.get(key)).cloned().unwrap_or_default();
        let searched = { let p = progress.read(cx); local.as_ref().and_then(|key| p.reads.get(key)).is_some_and(|read| read.snapshot.is_ok()) };
        self.batch = None;
        window.close_dialog(cx);
        match self.plan().server {
            ServerStep::Update => self.search_server(acceptance, !searched, window, cx),
            _ => self.run_local(Notes::default(), window, cx),
        }
    }

    /// `accepted`: o aceite de cada máquina cujos avisos foram lidos, pela chave.
    fn start_all(&mut self, accepted: HashMap<String, Acceptance>, window: &mut Window, cx: &mut Context<Self>) {
        let Some((progress, generation)) = self.batch.as_ref().map(|b| (b.progress.clone(), b.generation)) else { return };
        let (targets, snapshots, excluded, aliases) = {
            let p = progress.read(cx);
            let excluded: HashSet<String> = p.reads.iter().filter(|(_, read)| read.notes.excluded()).map(|(key, _)| key.clone()).collect();
            let snapshots: HashMap<String, Value> = p.reads.iter()
                .filter_map(|(key, read)| read.snapshot.as_ref().ok().map(|s| (key.clone(), s.clone()))).collect();
            (p.machines.iter().filter(|m| p.reads.contains_key(&m.key)).cloned().collect(), snapshots, excluded, p.aliases.clone())
        };
        let machines = batch_machines(still_registered(targets, &self.all_machines()), &accepted, &excluded);
        let runs: Vec<(Machine, Acceptance)> = machines.iter().filter(|m| !m.local)
            .filter(|m| snapshots.get(&m.key).is_some_and(|s| classify(s) == Classification::Start))
            .map(|m| (m.clone(), accepted.get(&m.key).cloned().unwrap_or_default())).collect();
        let local = machines.iter().find(|m| m.local).cloned();
        let plan_app = self.plan().app;
        progress.update(cx, |p, cx| {
            p.phase = Phase::Running;
            for row in &mut p.rows {
                if excluded.contains(&row.key) && !matches!(row.status, Status::Offline(_)) {
                    row.status = Status::Failed(tr_shared("update_notes_failed", &[]));
                } else if runs.iter().any(|(m, _)| m.key == row.key) { row.status = Status::Starting; }
            }
            p.app = Some(if plan_app { AppStep::Waiting } else { AppStep::Current });
            cx.notify();
        });
        self.survey.prepare(&progress.read(cx).machines.clone(), aliases);
        self.all_running = true;
        cx.notify();
        let (tx, rx) = async_channel::unbounded();
        self.runtime.spawn(async move {
            futures::future::join_all(runs.into_iter().map(|(machine, acceptance)| {
                let tx = tx.clone();
                let key = machine.key.clone();
                run_machine(machine, acceptance, move |status, warnings| { let _ = tx.try_send(Event::Changed(key.clone(), status, warnings)); })
            })).await;
        });
        let handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            while let Ok(event) = rx.recv().await {
                let _ = handle.update(cx, |_, window, cx| { let _ = this.update(cx, |this, cx| {
                    let Some(batch) = this.batch.as_ref().filter(|b| b.generation == generation) else { return };
                    if let Event::Changed(key, status, warnings) = event {
                        batch.progress.update(cx, |p, cx| { p.change(&key, status, warnings); cx.notify(); });
                    }
                    this.finish_batch(window, cx);
                }); });
            }
        }).detach();
        let local_status = local.as_ref().and_then(|m| progress.read(cx).rows.iter().find(|row| row.key == m.key).map(|row| row.status.clone()));
        let no_local_server = self.local.is_none();
        match (local, local_status) {
            (Some(machine), Some(Status::Available)) => {
                let acceptance = accepted.get(&machine.key).cloned().unwrap_or_default();
                // O estado relido na hora (sem buscar de novo): o da abertura pode ter minutos.
                self.search_server(acceptance, !snapshots.contains_key(&machine.key), window, cx);
            }
            (Some(_), Some(Status::Held(hold))) if hold.stops() => self.batch_app(AppStep::Hidden, cx),
            (Some(_), Some(Status::Current | Status::Held(_))) if plan_app => self.install_app(window, cx),
            (Some(_), Some(Status::Current | Status::Held(_))) => {}
            (None, _) if no_local_server && plan_app => self.install_app(window, cx),
            (None, _) if no_local_server => {}
            // Esta máquina fora do ar, com falha ou sem os avisos lidos: o app não é trocado por cima dela.
            _ => self.batch_app(AppStep::Hidden, cx),
        }
        self.finish_batch(window, cx);
    }

    /// Fechar no meio da leitura desiste do lote; durante a rodada ela segue em segundo plano, e o botão do topo reabre.
    fn batch_closed(&mut self, cx: &mut Context<Self>) {
        let Some(batch) = &self.batch else { return };
        if batch.progress.read(cx).phase == Phase::Review {
            self.batch = None;
            if matches!(self.run, Run::Searching) { self.run = Run::Idle; }
        } else if !self.busy() { self.batch = None; }
        cx.notify();
    }

    pub(super) fn open_batch(&self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(batch) = &self.batch else { return };
        let progress = batch.progress.clone();
        let weak = cx.entity().downgrade();
        window.open_dialog(cx, move |dialog, window, cx| {
            let (review, done, can_start) = { let p = progress.read(cx); (p.phase == Phase::Review, p.done(), p.can_start()) };
            let close = { let weak = weak.clone(); move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                let _ = weak.update(cx, |this, cx| this.batch_closed(cx));
                window.close_dialog(cx);
            } };
            let footer = if review {
                let start = weak.clone();
                DialogFooter::new()
                    .child(Button::new("update-all-cancel").label(tr_shared("update_all_cancel", &[])).on_click(close))
                    .child(chrome::light_button("update-all-start", can_start, cx).label(tr_shared("update_all_start", &[]))
                        .on_click(move |_, window, cx| { let _ = start.update(cx, |this, cx| this.start_from_review(window, cx)); }))
            } else if done {
                DialogFooter::new().child(chrome::light_button("update-all-close", true, cx).label(tr_shared("update_all_close", &[])).on_click(close))
            } else {
                DialogFooter::new().child(Button::new("update-all-hide").label(tr_shared("update_all_close_background", &[])).on_click(close))
            };
            let title = tr_shared(if done { "update_all_done_title" } else { "update_all_title" }, &[]);
            crate::app::popup::dialog(dialog).w(window.rem_size() * 30.).title(title).child(progress.clone()).footer(footer)
                .on_close({ let weak = weak.clone(); move |_, _, cx| { let _ = weak.update(cx, |this, cx| this.batch_closed(cx)); } })
        });
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
            // No meio do lote a releitura do fim da atualização local foi pulada; esta traz o que chegou.
            self.refresh_server(cx);
        }
    }

    /// A linha do app no lote em andamento; fora dele, nada.
    pub(super) fn batch_app(&mut self, step: AppStep, cx: &mut Context<Self>) {
        let Some(batch) = &self.batch else { return };
        batch.progress.update(cx, |p, cx| { if p.phase == Phase::Running { p.app = Some(step); cx.notify(); } });
        self.all_running = batch.progress.read(cx).busy();
        cx.notify();
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
        batch.progress.update(cx, |p, cx| { p.deferred_app = true; p.app = Some(AppStep::Waiting); cx.notify(); });
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

    fn row(key: &str, local: bool, status: Status) -> Row {
        Row { key: key.into(), label: key.into(), local, status, warnings: vec![] }
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
        let mut p = Progress::new(Phase::Running, vec![row("local", true, Status::Updated), row("remote", false, Status::Waiting)]);
        p.deferred_app = true;
        assert!(p.remote_busy() && p.busy());
        p.change("remote", Status::Failed("fixture-error".into()), vec!["fixture-warning".into()]);
        assert!(!p.remote_busy());
        assert!(p.busy(), "o app local ainda aguarda a troca");
        p.deferred_app = false;
        p.app = Some(AppStep::Downloading { version: "v".into(), done: 1, total: Some(2) });
        assert!(p.busy(), "o download do app ainda segura o fim");
        p.app = Some(AppStep::Current);
        assert!(!p.busy() && p.done());
        assert_eq!(p.rows[1].warnings, ["fixture-warning"]);
    }

    #[test]
    fn update_all_labels_are_shared_in_both_languages() {
        for source in [include_str!("../../../messages/pt.json"), include_str!("../../../messages/en.json")] {
            let dict: Value = serde_json::from_str(source).unwrap();
            for key in ["button", "title", "short", "available", "checking", "starting", "running", "waiting", "up_to_date", "held_branch",
                "held_changes", "held_running", "held_missing", "offline", "updated", "updated_manual", "failed", "silent", "timeout", "warnings",
                "summary_checking", "summary_ready", "summary_progress", "summary_app", "done_title", "done_ok", "done_issues",
                "count_updated_one", "count_updated_other", "count_current_one", "count_current_other", "count_offline_one",
                "count_offline_other", "count_issue_one", "count_issue_other", "offline_skip", "offline_skipped", "close_background",
                "close", "cancel", "start", "app_label", "app_waiting", "app_downloading", "app_downloading_plain", "app_installing", "app_current"] {
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
    fn update_all_executor_uses_the_review_snapshot_and_ignores_old_result_and_pid() {
        // A busca foi feita na abertura: a rodada relê o estado sem `procurar=1` e segue para os avisos e o `iniciar`.
        let (machine, server) = mock(vec![
            ("GET /api/atualizacao ", 200, json!({"atualizacao_disponivel": true, "estado":{"ts":"old"}})),
            ("GET /api/update/notes ", 200, json!({"warnings": []})),
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
    fn update_all_review_reads_each_machine_on_its_own() {
        let failure = |status| Failure { status, detail: "fixture-failure".into(), retry_after: None, uncertain: false, code: None };
        assert!(matches!(read_status(&Err(failure(Some(503)))), Status::Failed(error) if error.contains("fixture-failure")));
        assert!(matches!(read_status(&Err(failure(None))), Status::Offline(_)), "sem resposta é fora do ar");
        assert_eq!(read_status(&Ok(json!({"atualizacao_disponivel": false}))), Status::Current);
        assert_eq!(read_status(&Ok(json!({"atualizacao_disponivel": true}))), Status::Available);
        // Uma máquina fora do ar não lê os avisos: a falha da busca já basta.
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let (down, server) = mock(vec![("GET /api/atualizacao?procurar=1 ", 503, json!({"detail": "fixture-failure"})),
            ("GET /api/update/notes ", 200, json!({"warnings": []}))]);
        let (snapshot, notes) = runtime.block_on(confirm::load_warnings(down.api.clone(), down.key.clone(), down.label.clone()));
        server.join().unwrap();
        assert!(matches!(read_status(&snapshot), Status::Failed(_)) && !notes.excluded(), "erro do servidor ainda lê os avisos");
    }

    #[test]
    fn update_all_accepts_the_warning_and_starts_only_a_real_update() {
        let acceptance = Acceptance { ids: vec!["2026-10-24-pacote-por-versao".into()], mode: Some("package"), incomplete: false };
        let shown = || ("GET /api/update/notes ", 200, json!({"warnings": [{"id": "2026-10-24-pacote-por-versao", "title": "t"}]}));
        let state = |body: Value| ("GET /api/atualizacao ", 200, body);
        let runtime = tokio::runtime::Runtime::new().unwrap();
        // Só o aviso pendente: aceito, nada de `iniciar`.
        let (machine, server) = mock(vec![state(json!({"atualizacao_disponivel": false, "pending_warning": true, "atras": 0})), shown(),
            ("POST /api/update/accept ", 200, json!({"ok": true, "started": false}))]);
        let mut states = Vec::new();
        runtime.block_on(run_machine(machine, acceptance.clone(), |s, _| states.push(s)));
        server.join().unwrap();
        assert_eq!(states.last(), Some(&Status::Current));
        // O aceite lançou a migração: segue o pid dele, sem `iniciar`.
        let (machine, server) = mock(vec![
            state(json!({"atualizacao_disponivel": false, "pending_warning": true, "atras": 0, "estado": {"ts": "old"}})),
            shown(),
            ("POST /api/update/accept ", 200, json!({"ok": true, "started": true, "pid": 7})),
            ("GET /api/atualizacao ", 200, json!({"estado": {"fase": "pronto", "ok": true, "ts": "new"}})),
        ]);
        let mut states = Vec::new();
        runtime.block_on(async { tokio::time::timeout(Duration::from_secs(10), run_machine(machine, acceptance.clone(), |s, _| states.push(s))).await })
            .expect("o acompanhamento deve terminar");
        server.join().unwrap();
        assert_eq!(states.last(), Some(&Status::Updated));
        // Aceite recusado: nada é iniciado.
        let (machine, server) = mock(vec![state(json!({"atualizacao_disponivel": true, "atras": 3})), shown(),
            ("POST /api/update/accept ", 500, json!({"detail": "fixture-accept"}))]);
        let mut states = Vec::new();
        runtime.block_on(run_machine(machine, acceptance.clone(), |s, _| states.push(s)));
        server.join().unwrap();
        assert!(matches!(states.last(), Some(Status::Failed(error)) if error.contains("fixture-accept")), "{states:?}");
        // Versão publicada depois do modal, com aviso que ele não mostrou: nada é aceito nem iniciado.
        let (machine, server) = mock(vec![state(json!({"atualizacao_disponivel": true, "atras": 1})),
            ("GET /api/update/notes ", 200, json!({"warnings": [{"id": "2026-10-24-pacote-por-versao", "title": "t"}, {"id": "fixture-new", "title": "n"}]})),
        ]);
        let mut states = Vec::new();
        runtime.block_on(run_machine(machine, acceptance, |s, _| states.push(s)));
        server.join().unwrap();
        assert_eq!(states.last(), Some(&Status::Failed(tr("update_new_warnings"))), "{states:?}");
        // O modal leu os avisos da versão nova e a releitura não: nada é aceito nem iniciado.
        let (machine, server) = mock(vec![state(json!({"atualizacao_disponivel": true, "atras": 1})),
            ("GET /api/update/notes ", 200, json!({"warnings": [{"id": "2026-10-24-pacote-por-versao", "title": "t"}], "incomplete": true})),
        ]);
        let mut states = Vec::new();
        runtime.block_on(run_machine(machine,
            Acceptance { ids: vec!["2026-10-24-pacote-por-versao".into()], mode: Some("package"), incomplete: false }, |s, _| states.push(s)));
        server.join().unwrap();
        assert_eq!(states.last(), Some(&Status::Failed(tr_shared("update_notes_failed", &[]))), "{states:?}");
    }

    #[test]
    fn update_all_leaves_out_a_machine_whose_warnings_did_not_load() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let search = || ("GET /api/atualizacao?procurar=1 ", 200, json!({}));
        let (down, first) = mock(vec![search(), ("GET /api/update/notes ", 503, json!({"detail": "fixture-unavailable"}))]);
        let (old, second) = mock(vec![search(), ("GET /api/update/notes ", 404, json!({"detail": "Not Found"}))]);
        let read = |m: &Machine| confirm::load_warnings(m.api.clone(), m.key.clone(), m.label.clone());
        let ((_, a), (_, b)) = runtime.block_on(async { tokio::join!(read(&down), read(&old)) });
        first.join().unwrap(); second.join().unwrap();
        assert!(a.excluded(), "leitura que falha deixa a máquina de fora");
        assert!(!b.excluded(), "servidor antigo: sem avisos, segue no lote");
        let excluded: HashSet<String> = [&a, &b].into_iter().filter(|n| n.excluded()).map(|n| n.server().to_owned()).collect();
        let read: HashMap<String, ()> = [&a, &b].into_iter().map(|n| (n.server().to_owned(), ())).collect();
        let kept = batch_machines(vec![down.clone(), old.clone()], &read, &excluded);
        assert_eq!(kept.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), [old.key.as_str()]);
    }

    #[test]
    fn update_all_pending_warning_alone_is_not_a_real_update() {
        let warning = json!({"atualizacao_disponivel": false, "pending_warning": true, "atras": 0});
        assert!(behind(&warning), "o botão aparece");
        assert_eq!(classify(&warning), Classification::Start, "o lote abre a máquina para aceitar o aviso");
        assert!(!real_update(&warning), "sem versão nova, nada de iniciar");
        assert!(real_update(&json!({"atualizacao_disponivel": true, "pending_warning": true, "atras": 0})), "índice fora: a flag vale");
        assert!(real_update(&json!({"pending_warning": true, "versoes": {"repo": "b", "backend": "a"}})), "disco diferente do que está no ar");
        assert!(!real_update(&json!({"atualizacao_disponivel": false})));
        assert!(!behind(&json!({"atualizacao_disponivel": false})));
    }

    #[test]
    fn update_all_review_follow_auth_failure_finishes_immediately() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        for status in [401, 403] {
            let (machine, server) = mock(vec![
                ("GET /api/atualizacao ", 200, json!({"atualizacao_disponivel":true,"estado":{"ts":"old"}})),
                ("GET /api/update/notes ", 200, json!({"warnings": []})),
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
            assert_eq!(states.len(), 2, "início e falha, sem esperar outra leitura");
        }
    }

    #[test]
    fn update_all_machine_bar_requires_an_available_remote() {
        let hidden = Plan { server: ServerStep::Skip, app: false };
        assert!(!idle_visible(&hidden, 2, &[]), "cadastros sem consulta não justificam o botão");
        for status in [Status::Current, Status::Held(Hold::LocalChanges), Status::Offline("offline".into()),
            Status::Checking, Status::Updated, Status::Failed("failure".into())] {
            assert!(!idle_visible(&hidden, 2, &[row("fixture", false, status)]));
        }
        assert!(!idle_visible(&hidden, 2, &[row("fixture", true, Status::Available)]));
        assert!(!idle_visible(&hidden, 1, &[row("fixture", false, Status::Available)]));
        assert!(idle_visible(&hidden, 2, &[row("fixture", false, Status::Available)]));
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
        survey.rows.push(row("remote", false, Status::Held(Hold::WorkBranch("feature".into(), "main".into()))));
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
    fn update_all_batch_keeps_only_the_addresses_whose_warnings_were_read() {
        let machine = |key: &str| Machine { key: key.into(), entry_id: String::new(), label: key.into(),
            api: Err(Failure { status: None, detail: "fixture".into(), retry_after: None, uncertain: false, code: None }), local: false };
        let raw = || vec![machine("rep"), machine("alias")];
        let keys = |keys: &[&str]| keys.iter().map(|k| (k.to_string(), ())).collect::<HashMap<_, _>>();
        let excluded: HashSet<String> = ["rep".to_owned()].into();
        assert!(batch_machines(raw(), &keys(&["rep"]), &excluded).is_empty(), "o outro endereço da excluída não roda");
        let kept = batch_machines(raw(), &keys(&["rep", "alias"]), &excluded);
        assert_eq!(kept.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), ["alias"], "lidos em separado: o lido segue");
        let twice = batch_machines(vec![machine("alias"), machine("alias")], &keys(&["alias"]), &HashSet::new());
        assert_eq!(twice.len(), 1, "a mesma chave duas vezes vira uma linha só");
        let kept = still_registered(vec![machine("rep"), machine("gone")], &[machine("rep"), machine("new")]);
        assert_eq!(kept.iter().map(|m| m.key.as_str()).collect::<Vec<_>>(), ["rep"], "apagada com o modal aberto não roda; nova não entra sem avisos lidos");
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

    #[test]
    fn update_all_review_opens_with_rows_and_starts_only_when_read() {
        crate::i18n::english_in_this_thread(false);
        let (a, b, alias) = (Machine::from_entry(&entry("a"), None), Machine::from_entry(&entry("b"), None), Machine::from_entry(&entry("alias"), None));
        let mut p = Progress::new(Phase::Review, vec![Row::new(&a, Status::Checking), Row::new(&b, Status::Checking),
            Row::new(&alias, Status::Checking), row("known-down", false, Status::Offline(String::new()))]);
        assert_eq!(p.summary(), "Conferindo 3 máquinas no ar…");
        assert!(!p.can_start(), "antes das identidades não há o que atualizar");
        // O endereço repetido some; a que não respondeu fica fora do ar; a já sabida fora continua na lista.
        p.identified(vec![a.clone(), b.clone()], HashMap::new(), &[b.key.clone()].into());
        assert_eq!(p.rows.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), [a.key.as_str(), b.key.as_str(), "known-down"]);
        assert_eq!(row_text(&p.rows[1], Phase::Review), "Fora do ar · fica de fora");
        assert!(!p.can_start(), "a leitura de a ainda não voltou");
        p.record(a.key.clone(), Ok(json!({"atualizacao_disponivel": true})), Notes::default(), Status::Available);
        assert!(p.loaded() && p.can_start());
        assert_eq!(p.summary(), "Pronto para atualizar");
        p.change(&a.key, Status::Current, vec![]);
        assert!(!p.can_start(), "tudo em dia e sem app novo: nada a fazer");
        assert_eq!(p.summary(), "Em dia");
        p.app_offer = true;
        assert!(p.can_start(), "o app desta máquina também conta");
        assert_eq!(row_text(&p.rows[1], Phase::Running), "Fora do ar · ficou de fora");
    }

    #[test]
    fn update_all_round_tells_progress_and_how_it_ended() {
        crate::i18n::english_in_this_thread(false);
        let mut p = Progress::new(Phase::Running, vec![row("a", true, Status::Running { step: 1, total: 9, text: "x".into() }),
            row("b", false, Status::Current), row("c", false, Status::Waiting), row("d", false, Status::Offline(String::new()))]);
        p.app = Some(AppStep::Waiting);
        assert_eq!(p.summary(), "1 de 3 máquinas prontas");
        assert!((p.fraction() - 0.25).abs() < 1e-6, "{}", p.fraction());
        p.change("a", Status::Updated, vec![]);
        p.change("c", Status::Updated, vec![]);
        p.app = Some(AppStep::Downloading { version: "0.1.0.9214".into(), done: 37 << 20, total: Some(74 << 20) });
        assert_eq!(p.summary(), "Máquinas prontas · trocando o app");
        assert_eq!(p.app.as_ref().unwrap().text(), "Baixando a versão 0.1.0.9214… 50% (37 de 74 MB)");
        p.app = Some(AppStep::Current);
        assert!(p.done());
        assert_eq!(p.summary(), "Tudo pronto · 2 atualizadas, 1 já estava em dia, 1 fora do ar");
        p.change("b", Status::Failed("x".into()), vec![]);
        assert_eq!(p.summary(), "Terminou com pendências · 2 atualizadas, 1 fora do ar, 1 com problema");
        assert_eq!(AppStep::Downloading { version: "v".into(), done: 1, total: None }.text(), "Baixando a versão v…");
    }

    #[::gpui::test]
    fn update_all_machine_registry_change_keeps_running_aliases(cx: &mut gpui::TestAppContext) {
        let updater = cx.new(|_| Updater { runtime: Arc::new(Runtime::new().unwrap()), client: reqwest::Client::new(), exe: None, exe_stamp: None, offer: None,
            local: None, server: None, server_api: None, server_seq: 0, active: None, active_state: None, active_api: None, active_seq: 0,
            run: Run::Idle, checking: false, checked: None, checked_channel: None, known_channel: None, channel_blocked: None,
            suspended: false, intent_lock: None, all_servers: vec![entry("local"), entry("remote")], survey: Survey::default(), batch: None, all_running: true,
            arrived: Default::default() });
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
            suspended: false, intent_lock: None, all_servers: vec![entry("local"), entry("remote")], survey: Survey::default(), batch: None, all_running: false,
            arrived: Default::default() };
        assert!(!updater.plan().visible());
        assert!(!updater.idle_visible(&updater.plan()));
        updater.survey.rows.push(row("remote", false, Status::Available));
        assert_eq!(updater.idle_texts(&updater.plan()), ("Atualizar todas as máquinas (2)".into(), "Atualizar todas as máquinas".into()));
        updater.all_servers.push(entry("other"));
        assert_eq!(updater.idle_texts(&updater.plan()).0, "Atualizar todas as máquinas (3)");
    }
}
