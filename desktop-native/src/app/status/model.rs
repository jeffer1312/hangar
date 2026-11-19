//! O que a página Status mostra, montado na chegada do retrato e nunca no desenho. Cada bloco existe só
//! quando o servidor mandou a fonte dele: bloco ausente some da tela. Os tempos relativos ("há 2 h") contam
//! do `generated_at` do retrato, não do relógio deste computador.
use chrono::{DateTime, Datelike, NaiveDateTime, TimeZone, Timelike, Utc};

use hangar_api::status::{
    Attention, AttentionTarget, ConnectState, DirSize, DiskUse, Exit, ExitKind, HostBlock, LeftoversBlock, Machine, MachineState,
    OutsideBlock, Point, ServerBlock, SessionsBlock, Severity, SlowBlock, StatusSnapshot, UpdateBlock, Usage, WatchdogKind,
};

use crate::i18n::{tr, tr_shared, tr_web};
use super::super::costs::dec;
use super::super::side::ago;

const MIB: f64 = (1u64 << 20) as f64;
const GIB: f64 = (1u64 << 30) as f64;
const WEEK_SECS: f64 = 7. * 86_400.;

/// Papel da cor; a vista troca pelo token do tema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tone { Ok, Warn, Bad, Faint, Accent }

pub(crate) enum Block {
    Health(Health),
    Server(ServerCards),
    Days(Days),
    Machines(Machines),
    Sessions(Sessions),
    Slow(Slow),
    Host(Host),
    Outside(Outside),
}

pub(crate) struct AttentionItem { pub tone: Tone, pub text: String, pub target: AttentionTarget }

pub(crate) struct Health {
    pub title: String,
    /// "De pé há 3 d 4 h"; ausente sem o bloco do servidor.
    pub uptime: Option<String>,
    pub version: Option<String>,
    pub items: Vec<AttentionItem>,
}

pub(crate) struct SparkPoint { pub v: f64, pub text: String, pub ago: String }

pub(crate) struct Spark {
    pub value: String,
    pub average: Option<String>,
    pub avg_value: Option<f64>,
    pub points: Vec<SparkPoint>,
    pub slot: usize,
}

pub(crate) struct ServerCards { pub memory: Spark, pub cpu: Spark }

pub(crate) struct Event { pub pos: f32, pub kind: ExitKind, pub title: String, pub detail: Option<String> }

pub(crate) struct Timeline { pub days: Vec<String>, pub events: Vec<Event>, pub has_restart: bool }

pub(crate) struct UpdateLine { pub title: String, pub meta: String }

pub(crate) struct Days { pub hint: Option<String>, pub timeline: Option<Timeline>, pub update: Option<UpdateLine> }

pub(crate) struct MachineCard {
    pub id: String,
    pub tone: Tone,
    pub name: String,
    pub tag: Option<String>,
    /// "42 ms" e quantos tracinhos do sinal acendem (1 a 3).
    pub latency: Option<(String, u8)>,
    /// Sem latência: "De pé", "Fora há 2 h", "Sem resposta".
    pub state: Option<(String, Tone)>,
    pub server: String,
    pub outdated: bool,
    pub app: Option<String>,
}

pub(crate) struct Machines { pub cards: Vec<MachineCard>, pub alone: bool }

/// Pedaço de uma barra dividida; `slot` é a cor de série (0 = espaço livre).
pub(crate) struct Seg { pub label: String, pub count: Option<String>, pub weight: f32, pub inside: Option<String>, pub tip: String, pub slot: usize }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChipIcon { None, Clean, Check }

pub(crate) struct Chip { pub icon: ChipIcon, pub bold: Option<String>, pub text: String, pub tone: Tone, pub tip: Option<String> }

pub(crate) struct Sessions { pub hint: String, pub empty: bool, pub by_provider: Vec<Seg>, pub by_process: Vec<Seg>, pub chips: Vec<Chip> }

pub(crate) struct Hour { pub slow: u32, pub errors: u32, pub label: Option<String>, pub tip: String, pub when: String }

pub(crate) struct Route { pub route: String, pub share: f32, pub bad: bool, pub value: String, pub tip: String }

pub(crate) struct Slow { pub hint: String, pub hours: Vec<Hour>, pub max: u32, pub watchdog: Option<String>, pub routes: Vec<Route> }

pub(crate) struct Meter { pub name: String, pub pct: f32, pub tone: Tone, pub figure: String, pub tip: String }

pub(crate) struct Host { pub hint: String, pub meters: Vec<Meter>, pub disk: Vec<Seg> }

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TileIcon { Globe, Tunnel, Key }

pub(crate) struct Tile { pub icon: TileIcon, pub tone: Tone, pub title: String, pub value: String }

pub(crate) struct Outside { pub tiles: Vec<Tile>, pub chips: Vec<Chip> }

impl Block {
    /// O bloco para onde um ponto de atenção leva.
    pub(crate) fn target(&self) -> Option<AttentionTarget> {
        Some(match self {
            Block::Health(_) => return None,
            Block::Server(_) => AttentionTarget::Server,
            Block::Days(_) => AttentionTarget::Exits,
            Block::Machines(_) => AttentionTarget::Machines,
            Block::Sessions(_) => AttentionTarget::Sessions,
            Block::Slow(_) => AttentionTarget::Slow,
            Block::Host(_) => AttentionTarget::Host,
            Block::Outside(_) => AttentionTarget::Outside,
        })
    }
}

/// Os blocos da tela, na ordem do mock, no fuso deste computador.
pub(crate) fn view_model(snapshot: &StatusSnapshot) -> Vec<Block> { view_model_in(snapshot, &chrono::Local) }

pub(crate) fn view_model_in<Tz: TimeZone>(snapshot: &StatusSnapshot, tz: &Tz) -> Vec<Block> {
    let now = parse(&snapshot.generated_at).unwrap_or_else(Utc::now);
    let mut blocks = vec![Block::Health(health(snapshot, now))];
    if let Some(server) = &snapshot.server { blocks.push(Block::Server(server_cards(server, now))); }
    if snapshot.exits.is_some() || snapshot.update.is_some() {
        blocks.push(Block::Days(days(snapshot.exits.as_ref().map(|e| e.items.as_slice()), snapshot.update.as_ref(), now, tz)));
    }
    if let Some(machines) = &snapshot.machines { blocks.push(Block::Machines(machine_cards(machines, now))); }
    if let Some(sessions) = &snapshot.sessions { blocks.push(Block::Sessions(sessions_block(sessions))); }
    if let Some(slow) = &snapshot.slow { blocks.push(Block::Slow(slow_block(slow, tz))); }
    if let Some(host) = &snapshot.host { blocks.push(Block::Host(host_block(host))); }
    if let Some(outside) = outside_block(snapshot.outside.as_ref(), snapshot.leftovers.as_ref()) { blocks.push(Block::Outside(outside)); }
    blocks
}

// ---------------------------------------------------------------------------------------------
// Texto

pub(super) fn fill(text: String, params: &[(&str, &str)]) -> String {
    params.iter().fold(text, |text, (name, value)| text.replace(&format!("{{{name}}}"), value))
}

/// Memória de processo: "412 MB" até mil, depois "2,1 GB".
pub(crate) fn mb(bytes: f64) -> String {
    let mib = bytes / MIB;
    if mib < 1000. { format!("{} MB", mib.round()) } else { format!("{} GB", dec(bytes / GIB, 1)) }
}

/// Duração curta: "4 min", "2 h", "3 d 4 h".
pub(crate) fn span(secs: i64) -> String {
    let secs = secs.max(0);
    match secs {
        s if s < 60 => tr("status_under_minute"),
        s if s < 3_600 => format!("{} min", s / 60),
        s if s < 86_400 => format!("{} h", s / 3_600),
        s => match (s % 86_400) / 3_600 { 0 => format!("{} d", s / 86_400), h => format!("{} d {h} h", s / 86_400) },
    }
}

fn parse(ts: &str) -> Option<DateTime<Utc>> { DateTime::parse_from_rfc3339(ts).ok().map(|d| d.with_timezone(&Utc)) }

fn count(n: u64, one: String, many: String) -> String { if n == 1 { one } else { fill(many, &[("n", &n.to_string())]) } }

/// Gigabytes de disco e memória da máquina: uma casa abaixo de 100 (sem ",0"), inteiro acima.
fn gb_number(bytes: u64) -> String {
    let gib = bytes as f64 / GIB;
    if gib >= 100. { return format!("{}", gib.round()); }
    let text = format!("{gib:.1}");
    let text = text.strip_suffix(".0").unwrap_or(&text).to_owned();
    text.replace('.', &tr("decimal"))
}

fn pct_text(pct: f64) -> String { format!("{} %", dec(pct, 1)) }

fn local<Tz: TimeZone>(at: DateTime<Utc>, tz: &Tz) -> NaiveDateTime { at.with_timezone(tz).naive_local() }

fn list_item(key_text: String, index: usize) -> String { key_text.split(',').nth(index).unwrap_or_default().trim().to_owned() }

/// "15 nov, 03:12" (en "Nov 15, 03:12").
fn date_time(at: NaiveDateTime) -> String {
    fill(tr("status_date"), &[("day", &at.day().to_string()), ("month", &list_item(tr("status_months"), at.month0() as usize)),
        ("time", &format!("{:02}:{:02}", at.hour(), at.minute()))])
}

/// Mesmos códigos e chaves do Expo (`status_exit_cause_*`); o que não tem frase própria vai em "Causa: <código>".
fn cause_text(cause: &str) -> String {
    tr_web(&format!("status_exit_cause_{cause}"), &Default::default())
        .unwrap_or_else(|| tr_shared("status_exit_cause_other", &[("code", cause)]))
}

// ---------------------------------------------------------------------------------------------
// Saúde

fn attention_text(item: &Attention, now: DateTime<Utc>) -> String {
    let names: &[&str] = match item.code.as_str() {
        "machine_down" => &["name", "since"],
        "exits_crash" | "server_errors" | "watchdog" => &["count"],
        "machine_outdated" => &["name", "version"],
        "app_version_differs" => &["host", "version", "server"],
        "disk_full" | "tmp_full" => &["pct"],
        "session_dead" => &["name"],
        "queue_stuck" => &["minutes"],
        _ => &[],
    };
    let mut values = item.args.clone();
    // A queda vem como horário; a frase diz há quanto tempo.
    if item.code == "machine_down" && let Some(since) = values.get_mut(1) {
        *since = parse(since).map(|at| span((now - at).num_seconds())).unwrap_or_else(|| "—".into());
    }
    let params: Vec<(&str, &str)> = names.iter().copied().zip(values.iter().map(String::as_str)).collect();
    let key = format!("status_attention_{}", item.code);
    let text = tr_shared(&key, &params);
    // Código que esta versão não conhece aparece cru, nunca some.
    if text == key { item.code.clone() } else { text }
}

fn health(snapshot: &StatusSnapshot, now: DateTime<Utc>) -> Health {
    let items: Vec<AttentionItem> = snapshot.attention.iter().map(|item| AttentionItem {
        tone: if item.severity == Severity::Bad { Tone::Bad } else { Tone::Warn },
        text: attention_text(item, now),
        target: item.target,
    }).collect();
    let title = if items.is_empty() { tr("status_all_ok") } else { count(items.len() as u64, tr("status_points_one"), tr("status_points_many")) };
    let server = snapshot.server.as_ref();
    Health {
        title,
        uptime: server.and_then(|s| parse(&s.started_at)).map(|at| fill(tr("status_uptime"), &[("span", &span((now - at).num_seconds()))])),
        version: server.map(|s| s.version.clone()),
        items,
    }
}

// ---------------------------------------------------------------------------------------------
// Servidor

fn spark(series: &[Point], now: DateTime<Utc>, scale: f64, text: impl Fn(f64) -> String) -> Vec<SparkPoint> {
    series.iter().map(|p| SparkPoint {
        v: p.v / scale,
        text: text(p.v),
        ago: parse(&p.ts).map(|at| ago((now - at).num_seconds() as f64)).unwrap_or_default(),
    }).collect()
}

fn server_cards(server: &ServerBlock, now: DateTime<Utc>) -> ServerCards {
    let average = |text: String| fill(tr("status_average"), &[("value", &text)]);
    ServerCards {
        memory: Spark {
            value: mb(server.rss_bytes as f64),
            average: server.rss_avg_bytes.map(|b| average(mb(b as f64))),
            avg_value: server.rss_avg_bytes.map(|b| b as f64 / MIB),
            points: spark(&server.rss_series, now, MIB, mb),
            slot: 1,
        },
        cpu: Spark {
            value: server.cpu_pct.map(pct_text).unwrap_or_else(|| "—".into()),
            average: server.cpu_avg_pct.map(|c| average(pct_text(c))),
            avg_value: server.cpu_avg_pct,
            points: spark(&server.cpu_series, now, 1., pct_text),
            slot: 2,
        },
    }
}

// ---------------------------------------------------------------------------------------------
// Últimos 7 dias

fn kind_label(kind: ExitKind) -> String {
    match kind { ExitKind::Crash => tr("status_legend_crash"), ExitKind::Update => tr("status_legend_update"), ExitKind::Requested => tr("status_legend_restart") }
}

fn event_detail(exit: &Exit) -> Option<String> {
    match exit.kind {
        ExitKind::Update => match (&exit.from, &exit.to) {
            (Some(from), Some(to)) => Some(format!("{from} → {to}")),
            (None, Some(to)) => Some(to.clone()),
            _ => exit.cause.as_deref().map(cause_text),
        },
        // Sem causa gravada, a última linha de erro explica o que der.
        _ => exit.cause.as_deref().map(cause_text).or_else(|| exit.error_lines.last().cloned()),
    }
}

fn days<Tz: TimeZone>(exits: Option<&[Exit]>, update: Option<&UpdateBlock>, now: DateTime<Utc>, tz: &Tz) -> Days {
    let timeline = exits.map(|items| {
        let today = local(now, tz).date();
        let first = today - chrono::Duration::days(6);
        let start = first.and_hms_opt(0, 0, 0).expect("meia-noite é uma hora válida");
        let weekdays = tr("status_weekdays");
        let days = (0..7).map(|i| {
            let day = first + chrono::Duration::days(i);
            fill(tr("status_day_label"), &[("weekday", &list_item(weekdays.clone(), day.weekday().num_days_from_sunday() as usize)),
                ("day", &day.day().to_string())])
        }).collect();
        let events: Vec<Event> = items.iter().filter_map(|exit| {
            let at = local(parse(&exit.ts)?, tz);
            let pos = (at - start).num_seconds() as f64 / WEEK_SECS;
            (0. ..=1.).contains(&pos).then(|| Event {
                pos: pos as f32, kind: exit.kind,
                title: format!("{} · {}", kind_label(exit.kind), date_time(at)),
                detail: event_detail(exit),
            })
        }).collect();
        Timeline { has_restart: events.iter().any(|e| e.kind == ExitKind::Requested), days, events }
    });
    // A janela do servidor é móvel (7 × 24 h) e a linha começa à meia-noite: o resumo conta só o que a linha desenha.
    let hint = timeline.as_ref().map(|line| {
        let n = |kind: ExitKind| line.events.iter().filter(|e| e.kind == kind).count() as u64;
        let (crashes, updates, restarts) = (n(ExitKind::Crash), n(ExitKind::Update), n(ExitKind::Requested));
        let mut parts = vec![if crashes == 0 { tr("status_crashes_none") } else { count(crashes, tr("status_crashes_one"), tr("status_crashes_many")) }];
        if updates > 0 { parts.push(count(updates, tr("status_updates_one"), tr("status_updates_many"))); }
        if restarts > 0 { parts.push(count(restarts, tr("status_restarts_one"), tr("status_restarts_many"))); }
        parts.join(" · ")
    });
    Days { hint, timeline, update: update.map(|u| update_line(u, now)) }
}

fn update_line(update: &UpdateBlock, now: DateTime<Utc>) -> UpdateLine {
    let title = match update.available.as_deref().filter(|v| *v != update.installed) {
        Some(version) => fill(tr("status_update_available"), &[("version", version)]),
        None => tr("status_update_latest"),
    };
    let channel = match update.channel.as_str() {
        "stable" => tr("status_channel_stable"),
        "beta" => tr("status_channel_beta"),
        other => fill(tr("status_channel_other"), &[("name", other)]),
    };
    let mut meta = vec![channel];
    if let Some(at) = update.last_check.as_deref().and_then(parse) {
        meta.push(fill(tr("status_checked"), &[("span", &span((now - at).num_seconds()))]));
    }
    if let Some(result) = &update.last_result {
        meta.push(match (result.ok, result.step.as_deref()) {
            (true, _) => tr("status_last_update_ok"),
            (false, Some(step)) => fill(tr("status_last_update_failed_step"), &[("step", step)]),
            (false, None) => tr("status_last_update_failed"),
        });
    }
    UpdateLine { title, meta: meta.join(" · ") }
}

// ---------------------------------------------------------------------------------------------
// Máquinas

fn platform(name: &str) -> String {
    match name { "linux" => "Linux".into(), "windows" => "Windows".into(), "macos" | "darwin" => "macOS".into(), other => other.to_owned() }
}

fn machine_cards(machines: &[Machine], now: DateTime<Utc>) -> Machines {
    let cards = machines.iter().map(|m| {
        let tone = match m.state { MachineState::Ok => Tone::Ok, MachineState::Down => Tone::Bad, MachineState::Unknown => Tone::Faint };
        let tag = if m.this_machine { Some(tr("status_machine_this")) } else if m.lan { Some(tr("status_machine_lan")) }
            else { m.app.as_ref().map(|a| platform(&a.platform)) };
        let latency = m.latency_ms.filter(|_| m.state != MachineState::Down)
            .map(|ms| (format!("{ms} ms"), match ms { 0..60 => 3, 60..150 => 2, _ => 1 }));
        let state = latency.is_none().then(|| match m.state {
            MachineState::Ok => (tr("status_machine_up"), Tone::Faint),
            MachineState::Down => match m.down_since.as_deref().and_then(parse) {
                Some(at) => (fill(tr("status_machine_down_for"), &[("span", &span((now - at).num_seconds()))]), Tone::Bad),
                None => (tr("status_machine_down"), Tone::Bad),
            },
            MachineState::Unknown => (tr("status_machine_unknown"), Tone::Faint),
        });
        MachineCard {
            id: m.id.clone(), tone, name: m.label.clone(), tag, latency, state,
            server: m.server_version.clone().unwrap_or_else(|| "—".into()),
            outdated: m.outdated,
            app: m.app.as_ref().map(|a| format!("{} · {}", mb(a.rss_bytes as f64), pct_text(a.cpu_pct))),
        }
    }).collect::<Vec<_>>();
    Machines { alone: machines.iter().all(|m| m.this_machine), cards }
}

// ---------------------------------------------------------------------------------------------
// Sessões

fn provider_name(key: &str) -> (String, usize) {
    match key {
        "claude" => ("Claude".into(), 1), "codex" => ("Codex".into(), 2), "pi" => ("Pi".into(), 3), "kimi" => ("Kimi".into(), 4),
        "omp" => ("oh-my-pi".into(), 5), other => (other.to_owned(), 5),
    }
}

fn process_name(key: &str) -> (String, usize) {
    match key {
        "claude" => ("claude".into(), 1), "codex_app_server" => (tr("status_process_codex"), 2), "pi_omp_kimi" => (tr("status_process_pi"), 3),
        "cano" => ("cano".into(), 4), "tmux" => ("tmux".into(), 5), other => (other.to_owned(), 5),
    }
}

/// A barra pesa pela memória quando todos a têm; senão, pela contagem.
fn usage_segs(items: &[Usage], name: fn(&str) -> (String, usize), one: String, many: String) -> Vec<Seg> {
    let by_memory = !items.is_empty() && items.iter().all(|u| u.rss_bytes.is_some());
    let weight = |u: &Usage| if by_memory { u.rss_bytes.unwrap_or(0) as f64 } else { f64::from(u.count) };
    let total: f64 = items.iter().map(weight).sum();
    items.iter().map(|u| {
        let (label, slot) = name(&u.key);
        let share = if total > 0. { weight(u) / total } else { 0. };
        let size = u.rss_bytes.map(|b| mb(b as f64));
        let inside = (share > 0.12).then(|| if by_memory { size.clone().unwrap_or_default() } else { format!("{} %", (share * 100.).round()) });
        let mut tip = vec![label.clone(), count(u64::from(u.count), one.clone(), many.clone())];
        if let Some(size) = size { tip.insert(0, size); }
        if let Some(cpu) = u.cpu_pct { tip.push(fill(tr("status_cpu_share"), &[("value", &pct_text(cpu))])); }
        Seg { label, count: Some(u.count.to_string()), weight: weight(u) as f32, inside, tip: tip.join(" · "), slot }
    }).collect()
}

fn sessions_block(s: &SessionsBlock) -> Sessions {
    // Só soma quando todos os provedores trazem a memória: um faltando daria um total menor que o real.
    let memory: Option<u64> = if s.by_provider.is_empty() { None } else { s.by_provider.iter().map(|u| u.rss_bytes).sum() };
    let hint = if s.open == 0 { tr("status_sessions_none") } else {
        let open = count(u64::from(s.open), tr("status_sessions_open_one"), tr("status_sessions_open_many"));
        match memory { Some(bytes) => format!("{open} · {}", mb(bytes as f64)), None => open }
    };
    let mut chips = Vec::new();
    if !s.dead.is_empty() {
        let names = s.dead.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ");
        let text = if s.dead.len() == 1 { tr("status_chip_dead_one") } else { tr("status_chip_dead_many") };
        chips.push(Chip { icon: ChipIcon::None, bold: Some(s.dead.len().to_string()), text: fill(text, &[("names", &names)]), tone: Tone::Bad, tip: None });
    }
    if s.asleep > 0 {
        let text = if s.asleep == 1 { tr("status_chip_asleep_one") } else { tr("status_chip_asleep_many") };
        chips.push(Chip { icon: ChipIcon::None, bold: Some(s.asleep.to_string()), text, tone: Tone::Faint, tip: None });
    }
    if s.queued > 0 {
        let text = match s.oldest_queued_secs { Some(secs) => fill(tr("status_chip_queued_for"), &[("span", &span(secs as i64))]), None => tr("status_chip_queued") };
        let stuck = s.oldest_queued_secs.is_some_and(|secs| secs >= 600);
        chips.push(Chip { icon: ChipIcon::None, bold: Some(s.queued.to_string()), text, tone: if stuck { Tone::Warn } else { Tone::Faint }, tip: None });
    }
    Sessions {
        hint, empty: s.open == 0, chips,
        by_provider: usage_segs(&s.by_provider, provider_name, tr("status_sessions_one"), tr("status_sessions_many")),
        by_process: usage_segs(&s.by_process, process_name, tr("status_processes_one"), tr("status_processes_many")),
    }
}

// ---------------------------------------------------------------------------------------------
// Lentidão e erros

fn seconds_text(ms: u64) -> String { if ms >= 1000 { format!("{} s", dec(ms as f64 / 1000., 1)) } else { format!("{ms} ms") } }

fn slow_block<Tz: TimeZone>(slow: &SlowBlock, tz: &Tz) -> Slow {
    let total_slow: u64 = slow.hours.iter().map(|h| u64::from(h.slow)).sum();
    let total_errors: u64 = slow.hours.iter().map(|h| u64::from(h.errors)).sum();
    let hint = format!("24 h · {} · {}", count(total_slow, tr("status_slow_one"), tr("status_slow_many")),
        count(total_errors, tr("status_errors_one"), tr("status_errors_many")));
    let last = slow.hours.len().saturating_sub(1);
    let hours = slow.hours.iter().enumerate().map(|(i, h)| {
        let when = parse(&h.hour_start).map(|at| fill(tr("status_hour"), &[("h", &format!("{:02}", local(at, tz).hour()))])).unwrap_or_default();
        let label = if i == last { Some(tr("status_now")) } else { (i % 6 == 0).then(|| when.clone()) };
        let mut tip = Vec::new();
        if h.slow > 0 { tip.push(count(u64::from(h.slow), tr("status_slow_one"), tr("status_slow_many"))); }
        if h.errors > 0 { tip.push(count(u64::from(h.errors), tr("status_hour_errors_one"), tr("status_hour_errors_many"))); }
        let tip = if tip.is_empty() { tr("status_hour_quiet") } else { tip.join(" · ") };
        Hour { slow: h.slow, errors: h.errors, label, tip, when }
    }).collect();
    let max = slow.hours.iter().map(|h| h.slow + h.errors).max().unwrap_or(0).max(3);
    let late = slow.watchdog.iter().filter(|w| w.kind == WatchdogKind::Late).count() as u64;
    let withheld = slow.watchdog.iter().filter(|w| w.kind == WatchdogKind::Withheld).count() as u64;
    let watchdog = (late + withheld > 0).then(|| {
        let mut parts = Vec::new();
        if late > 0 { parts.push(count(late, tr("status_watchdog_late_one"), tr("status_watchdog_late_many"))); }
        if withheld > 0 { parts.push(count(withheld, tr("status_watchdog_withheld_one"), tr("status_watchdog_withheld_many"))); }
        fill(tr("status_watchdog"), &[("text", &parts.join(" · "))])
    });
    let top = slow.top_routes.iter().map(|r| r.count).max().unwrap_or(0).max(1);
    let routes = slow.top_routes.iter().map(|r| {
        let bad = r.last_code >= 500;
        let worst = r.worst_ms.map(seconds_text);
        let value = if bad { r.last_code.to_string() } else { worst.clone().unwrap_or_else(|| format!("{}×", r.count)) };
        let tip = fill(tr("status_route_tip"), &[("count", &r.count.to_string()), ("worst", &worst.unwrap_or_else(|| "—".into())),
            ("code", &r.last_code.to_string())]);
        Route { route: r.route.clone(), share: r.count as f32 / top as f32, bad, value, tip }
    }).collect();
    Slow { hint, hours, max, watchdog, routes }
}

// ---------------------------------------------------------------------------------------------
// Esta máquina

fn meter(name: String, used: u64, total: u64) -> Meter {
    let pct = if total == 0 { 0. } else { used as f64 * 100. / total as f64 };
    let tone = if pct >= 90. { Tone::Bad } else if pct >= 75. { Tone::Warn } else { Tone::Accent };
    let (used, total) = (gb_number(used), gb_number(total));
    Meter {
        name, pct: pct as f32, tone,
        figure: format!("{used} / {total} GB"),
        tip: fill(tr("status_meter_tip"), &[("pct", &format!("{}", pct.round())), ("used", &used), ("total", &total)]),
    }
}

fn dir_part(label: String, dir: DirSize, slot: usize) -> Option<Seg> {
    let size = match (dir.bytes, dir.computing) {
        (Some(bytes), _) => format!("{} GB", gb_number(bytes)),
        (None, true) => tr("status_disk_computing"),
        (None, false) => return None,
    };
    Some(Seg { label, count: Some(size.clone()), weight: dir.bytes.unwrap_or(0) as f32, inside: None, tip: size, slot })
}

fn disk_parts(home: DiskUse, hangar: DirSize, cache: DirSize) -> Vec<Seg> {
    let mut parts: Vec<Seg> = [dir_part("~/.hangar".into(), hangar, 1), dir_part(tr("status_disk_build_cache"), cache, 3)].into_iter().flatten().collect();
    let known: u64 = hangar.bytes.unwrap_or(0) + cache.bytes.unwrap_or(0);
    let other = home.used.saturating_sub(known);
    let free = home.total.saturating_sub(home.used);
    // Pasta ainda sem medida: o saldo inclui ela, então não é "Outros" e encolheria quando a conta terminasse.
    let pending = |dir: DirSize| dir.bytes.is_none() && dir.computing;
    let rest = if pending(hangar) || pending(cache) { tr("status_disk_unclassified") } else { tr("status_disk_other") };
    for (label, bytes, slot) in [(rest, other, 5), (tr("status_disk_free"), free, 0)] {
        let size = format!("{} GB", gb_number(bytes));
        parts.push(Seg { label, count: Some(size.clone()), weight: bytes as f32, inside: None, tip: size, slot });
    }
    for seg in &mut parts { seg.tip = format!("{} · {}", seg.tip, seg.label); }
    parts
}

fn host_block(host: &HostBlock) -> Host {
    let cpus = count(u64::from(host.cpus), tr("status_host_cpus_one"), tr("status_host_cpus_many"));
    let hint = match host.load { Some([one, ..]) => format!("{} · {cpus}", fill(tr("status_host_load"), &[("load", &dec(one, 2))])), None => cpus };
    let mut meters = vec![meter(tr("status_meter_memory"), host.mem_total.saturating_sub(host.mem_available), host.mem_total)];
    if host.swap_total > 0 { meters.push(meter(tr("status_meter_swap"), host.swap_used, host.swap_total)); }
    meters.push(meter(tr("status_meter_disk"), host.disk_home.used, host.disk_home.total));
    if let Some(tmp) = host.disk_tmp { meters.push(meter(tr("status_meter_tmp"), tmp.used, tmp.total)); }
    Host { hint, meters, disk: disk_parts(host.disk_home, host.dir_hangar, host.dir_build_cache) }
}

// ---------------------------------------------------------------------------------------------
// Acesso de fora e limpeza

fn connect_tile(state: ConnectState) -> Tile {
    let (tone, value) = match (state.enabled, state.running) {
        (true, true) => (Tone::Ok, tr("status_on")),
        (true, false) => (Tone::Warn, tr("status_connect_stopped")),
        (false, _) => (Tone::Faint, tr("status_off")),
    };
    Tile { icon: TileIcon::Globe, tone, title: "Connect".into(), value }
}

fn outside_block(outside: Option<&OutsideBlock>, leftovers: Option<&LeftoversBlock>) -> Option<Outside> {
    let mut tiles = Vec::new();
    if let Some(o) = outside {
        if let Some(state) = o.connect { tiles.push(connect_tile(state)); }
        if let Some(on) = o.funnel {
            tiles.push(Tile { icon: TileIcon::Tunnel, tone: if on { Tone::Ok } else { Tone::Faint }, title: "Funnel".into(),
                value: if on { tr("status_on") } else { tr("status_off") } });
        }
        if let Some(locked) = o.keyring_locked {
            tiles.push(Tile { icon: TileIcon::Key, tone: if locked { Tone::Warn } else { Tone::Ok }, title: tr("status_keyring"),
                value: if locked { tr("status_keyring_locked") } else { tr("status_keyring_unlocked") } });
        }
    }
    let mut chips = Vec::new();
    if let Some(l) = leftovers {
        match l.tmux_orphans.as_deref() {
            Some([]) => chips.push(Chip { icon: ChipIcon::Check, bold: None, text: tr("status_tmux_none"), tone: Tone::Faint, tip: None }),
            Some(names) => chips.push(Chip { icon: ChipIcon::Clean, bold: Some(names.len().to_string()), text: tr("status_tmux_orphans"),
                tone: Tone::Faint, tip: Some(names.join(", ")) }),
            None => {}
        }
        match l.cano_orphans {
            Some(0) => chips.push(Chip { icon: ChipIcon::Check, bold: None, text: tr("status_cano_none"), tone: Tone::Faint, tip: None }),
            Some(n) => chips.push(Chip { icon: ChipIcon::Clean, bold: Some(n.to_string()),
                text: if n == 1 { tr("status_cano_one") } else { tr("status_cano_many") }, tone: Tone::Faint, tip: None }),
            None => {}
        }
    }
    (!tiles.is_empty() || !chips.is_empty()).then_some(Outside { tiles, chips })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    fn fixture() -> StatusSnapshot {
        serde_json::from_str(include_str!("../../../../crates/hangar-api/tests/fixtures/status.json")).expect("fixture do retrato")
    }

    /// São Paulo, sem horário de verão: o fuso do mock.
    fn brt() -> FixedOffset { FixedOffset::west_opt(3 * 3600).unwrap() }

    fn kinds(blocks: &[Block]) -> Vec<&'static str> {
        blocks.iter().map(|b| match b {
            Block::Health(_) => "health", Block::Server(_) => "server", Block::Days(_) => "days", Block::Machines(_) => "machines",
            Block::Sessions(_) => "sessions", Block::Slow(_) => "slow", Block::Host(_) => "host", Block::Outside(_) => "outside",
        }).collect()
    }

    #[test]
    fn every_block_of_the_fixture_shows_in_the_mock_order() {
        crate::i18n::english_in_this_thread(false);
        let blocks = view_model_in(&fixture(), &brt());
        assert_eq!(kinds(&blocks), ["health", "server", "days", "machines", "sessions", "slow", "host", "outside"]);
    }

    #[test]
    fn missing_block_disappears_and_health_stays() {
        crate::i18n::english_in_this_thread(false);
        let mut snapshot = fixture();
        (snapshot.machines, snapshot.sessions, snapshot.outside, snapshot.leftovers) = (None, None, None, None);
        let blocks = view_model_in(&snapshot, &brt());
        assert_eq!(kinds(&blocks), ["health", "server", "days", "slow", "host"]);
        let bare = StatusSnapshot { generated_at: snapshot.generated_at.clone(), server: None, exits: None, update: None, machines: None,
            sessions: None, slow: None, host: None, outside: None, leftovers: None, attention: Vec::new() };
        let blocks = view_model_in(&bare, &brt());
        assert_eq!(kinds(&blocks), ["health"]);
        let Block::Health(health) = &blocks[0] else { unreachable!() };
        assert_eq!(health.title, tr("status_all_ok"));
        assert!(health.uptime.is_none() && health.version.is_none(), "sem o bloco do servidor não há versão nem tempo de pé");
    }

    #[test]
    fn attention_text_comes_from_the_shared_key_with_named_args() {
        crate::i18n::english_in_this_thread(false);
        let blocks = view_model_in(&fixture(), &brt());
        let Block::Health(health) = &blocks[0] else { unreachable!() };
        let texts: Vec<&str> = health.items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, ["VPS fora há 2 h", "Quedas do servidor em 7 dias: 1", "Casa com versão antiga (0.1.0.10790)", "Disco 76% cheio"]);
        assert_eq!(health.items[0].tone, Tone::Bad);
        assert_eq!(health.items[3].tone, Tone::Warn);
        assert_eq!(health.items[3].target, AttentionTarget::Host);
        assert_eq!(health.title, fill(tr("status_points_many"), &[("n", "4")]));
        assert_eq!(health.uptime.as_deref(), Some(fill(tr("status_uptime"), &[("span", "3 d 4 h")]).as_str()));
    }

    #[test]
    fn unknown_attention_code_shows_raw() {
        crate::i18n::english_in_this_thread(false);
        let mut snapshot = fixture();
        snapshot.attention = vec![Attention { severity: Severity::Warn, code: "brand_new".into(), target: AttentionTarget::Server, args: vec![] }];
        let blocks = view_model_in(&snapshot, &brt());
        let Block::Health(health) = &blocks[0] else { unreachable!() };
        assert_eq!(health.items[0].text, "brand_new");
    }

    #[test]
    fn timeline_places_events_in_the_local_week() {
        crate::i18n::english_in_this_thread(false);
        let blocks = view_model_in(&fixture(), &brt());
        let Some(Block::Days(days)) = blocks.iter().find(|b| matches!(b, Block::Days(_))) else { panic!("sem o bloco dos 7 dias") };
        let timeline = days.timeline.as_ref().expect("linha do tempo");
        assert_eq!(timeline.days.first().map(String::as_str), Some("qua 11"));
        assert_eq!(timeline.days.last().map(String::as_str), Some("ter 17"));
        let crash = timeline.events.iter().find(|e| e.kind == ExitKind::Crash).expect("a queda");
        // 15/11 03:12 no fuso de Brasília: 4 dias e 3,2 h depois do começo da semana.
        assert!((crash.pos - 4.133 / 7.).abs() < 0.001, "posição {}", crash.pos);
        assert!(crash.title.ends_with("15 nov, 03:12"), "{}", crash.title);
        assert_eq!(crash.detail.as_deref(), Some(tr_shared("status_exit_cause_out_of_memory", &[]).as_str()));
        assert_eq!(cause_text("restart_only"), tr_shared("status_exit_cause_other", &[("code", "restart_only")]));
        let update = timeline.events.iter().find(|e| e.kind == ExitKind::Update).expect("a atualização");
        assert_eq!(update.detail.as_deref(), Some("0.1.0.10751 → 0.1.0.10790"));
        assert!(!timeline.has_restart);
        let line = days.update.as_ref().expect("linha da atualização");
        assert_eq!(line.title, fill(tr("status_update_available"), &[("version", "0.1.0.10833")]));
        assert!(line.meta.contains(&fill(tr("status_checked"), &[("span", "12 min")])), "{}", line.meta);
    }

    #[test]
    fn disk_rest_is_other_only_when_every_folder_was_measured() {
        crate::i18n::english_in_this_thread(false);
        let home = DiskUse { used: 60 << 30, total: 100 << 30 };
        let measured = DirSize { bytes: Some(20 << 30), computing: false };
        let rest = |cache: DirSize| disk_parts(home, measured, cache).into_iter().find(|s| s.slot == 5).expect("o saldo").label;
        assert_eq!(rest(DirSize { bytes: None, computing: true }), tr("status_disk_unclassified"));
        assert_eq!(rest(DirSize { bytes: Some(15 << 30), computing: true }), tr("status_disk_other"));
        // Sem pasta de cache não há o que esperar.
        assert_eq!(rest(DirSize { bytes: None, computing: false }), tr("status_disk_other"));
    }

    #[test]
    fn hint_counts_only_the_exits_the_timeline_draws() {
        crate::i18n::english_in_this_thread(false);
        let hint_of = |snapshot: &StatusSnapshot| {
            let blocks = view_model_in(snapshot, &brt());
            let Some(Block::Days(days)) = blocks.iter().find(|b| matches!(b, Block::Days(_))) else { panic!("sem o bloco dos 7 dias") };
            days.hint.clone()
        };
        let mut snapshot = fixture();
        let before = hint_of(&snapshot);
        // Dentro das 168 h do servidor, mas antes da meia-noite em que a linha começa (qua 11).
        let exits = snapshot.exits.as_mut().expect("saídas na fixture");
        let mut early = exits.items.iter().find(|e| e.kind == ExitKind::Crash).expect("a queda").clone();
        early.ts = "2026-11-10T22:00:00-03:00".into();
        exits.items.push(early);
        assert_eq!(hint_of(&snapshot), before);
    }

    #[test]
    fn machines_sessions_and_host_read_the_fixture() {
        crate::i18n::english_in_this_thread(false);
        let blocks = view_model_in(&fixture(), &brt());
        let Some(Block::Machines(machines)) = blocks.iter().find(|b| matches!(b, Block::Machines(_))) else { panic!() };
        assert!(!machines.alone);
        let vps = machines.cards.iter().find(|c| c.id == "vps").expect("VPS");
        assert_eq!(vps.tone, Tone::Bad);
        assert_eq!(vps.state.as_ref().map(|s| s.0.clone()), Some(fill(tr("status_machine_down_for"), &[("span", "2 h")])));
        assert!(vps.app.is_none());
        let casa = machines.cards.iter().find(|c| c.id == "casa").expect("Casa");
        assert_eq!(casa.latency, Some(("42 ms".to_owned(), 3u8)));
        assert!(casa.outdated);
        assert_eq!(casa.tag.as_deref(), Some("Linux"));

        let Some(Block::Sessions(sessions)) = blocks.iter().find(|b| matches!(b, Block::Sessions(_))) else { panic!() };
        assert_eq!(sessions.by_provider.len(), 4);
        assert_eq!(sessions.by_process.len(), 5);
        assert_eq!(sessions.by_provider[0].inside.as_deref(), Some("2,1 GB"));
        assert_eq!(sessions.chips.len(), 3, "caída, adormecidas e fila");
        assert_eq!(sessions.chips[0].tone, Tone::Bad);

        let Some(Block::Host(host)) = blocks.iter().find(|b| matches!(b, Block::Host(_))) else { panic!() };
        assert_eq!(host.meters.len(), 4);
        assert_eq!(host.meters[0].figure, "21,2 / 31 GB");
        assert_eq!(host.meters[2].tone, Tone::Warn, "disco a 76 %");
        assert_eq!(host.disk.len(), 4);

        let Some(Block::Slow(slow)) = blocks.iter().find(|b| matches!(b, Block::Slow(_))) else { panic!() };
        assert_eq!(slow.hours.len(), 24);
        assert_eq!(slow.hours[23].label.as_deref(), Some(tr("status_now").as_str()));
        assert!(slow.routes.iter().any(|r| r.bad && r.value == "502"));
        assert!(slow.watchdog.is_some());
    }

    #[test]
    fn spark_point_ago_counts_from_the_snapshot() {
        let now = parse("2026-11-17T14:00:00Z").unwrap();
        let point = |ts: &str| Point { ts: ts.to_owned(), v: 1. };
        let points = spark(&[point("2026-11-17T13:57:00Z"), point("2026-11-17T14:00:00Z")], now, 1., |v| v.to_string());
        assert_eq!(points[0].ago, tr("ago_min").replace("{n}", "3"));
        assert_eq!(points[1].ago, tr("ago_now"));
    }
}
