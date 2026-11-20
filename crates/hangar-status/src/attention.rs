//! Lista de atenção da tela Status, montada sobre o retrato pronto: uma função pura, sem leitura.
//! Os itens `bad` vêm antes dos `warn`; dentro de cada severidade, na ordem das regras abaixo.
//!
//! Código, severidade, bloco e `args` (na ordem; a chave `status_attention_<code>` usa os nomes entre
//! parênteses, e o cliente monta os nomes a partir da posição):
//! - `machine_down` (bad, machines): máquina (`name`), fora desde (`since`, RFC 3339 que o cliente
//!   converte em "há quanto"; vazio quando desconhecido).
//! - `exits_crash` (bad, exits): quedas nos 7 dias (`count`).
//! - `machine_outdated` (warn, machines): máquina (`name`), versão do servidor dela (`version`).
//! - `app_version_differs` (warn, machines): computador do app (`host`), versão do app (`version`),
//!   versão deste servidor (`server`).
//! - `disk_full` (warn a partir de 75 %, bad a partir de 90 %, host): uso do disco da casa (`pct`).
//! - `tmp_full` (warn a partir de 75 %, host): uso do disco do temporário (`pct`).
//! - `session_dead` (bad, sessions): sessão caída (`name`).
//! - `queue_stuck` (warn a partir de 10 min, sessions): minutos da mensagem mais antiga na fila (`minutes`).
//! - `server_errors` (warn, slow): respostas 5xx nas 24 h (`count`).
//! - `watchdog` (warn, slow): avisos do watchdog nas 24 h (`count`).
//! - `keyring_locked` (warn, outside): sem argumentos.
use hangar_api::status::{Attention, AttentionTarget, DiskUse, ExitKind, MachineState, Severity, StatusSnapshot};

pub const DISK_WARN_PCT: f64 = 75.0;
pub const DISK_BAD_PCT: f64 = 90.0;
pub const TMP_WARN_PCT: f64 = 75.0;
pub const QUEUE_STUCK_SECS: u64 = 600;

/// `package_version` é a deste servidor (`/__hangar_server/health`); sem ela, a versão dos apps não
/// é comparada.
pub fn attention(snapshot: &StatusSnapshot, package_version: Option<&str>) -> Vec<Attention> {
    let mut out = Vec::new();
    let mut push = |severity, code: &str, target, args: Vec<String>| {
        out.push(Attention { severity, code: code.to_owned(), target, args });
    };
    let machines = snapshot.machines.as_deref().unwrap_or_default();
    for machine in machines.iter().filter(|m| !m.this_machine && m.state == MachineState::Down) {
        push(Severity::Bad, "machine_down", AttentionTarget::Machines,
            vec![machine.label.clone(), machine.down_since.clone().unwrap_or_default()]);
    }
    let crashes = snapshot.exits.as_ref().map_or(0, |exits| exits.items.iter().filter(|e| e.kind == ExitKind::Crash).count());
    if crashes > 0 {
        push(Severity::Bad, "exits_crash", AttentionTarget::Exits, vec![crashes.to_string()]);
    }
    for machine in machines.iter().filter(|m| m.outdated && m.state != MachineState::Down) {
        push(Severity::Warn, "machine_outdated", AttentionTarget::Machines,
            vec![machine.label.clone(), machine.server_version.clone().unwrap_or_default()]);
    }
    if let Some(ours) = package_version {
        for app in machines.iter().filter_map(|m| m.app.as_ref()).filter(|app| app.version != ours) {
            push(Severity::Warn, "app_version_differs", AttentionTarget::Machines,
                vec![app.host.clone(), app.version.clone(), ours.to_owned()]);
        }
    }
    if let Some(host) = &snapshot.host {
        let home = pct(host.disk_home);
        if home >= DISK_BAD_PCT {
            push(Severity::Bad, "disk_full", AttentionTarget::Host, vec![whole(home)]);
        } else if home >= DISK_WARN_PCT {
            push(Severity::Warn, "disk_full", AttentionTarget::Host, vec![whole(home)]);
        }
        if let Some(tmp) = host.disk_tmp.map(pct).filter(|tmp| *tmp >= TMP_WARN_PCT) {
            push(Severity::Warn, "tmp_full", AttentionTarget::Host, vec![whole(tmp)]);
        }
    }
    if let Some(sessions) = &snapshot.sessions {
        for dead in &sessions.dead {
            push(Severity::Bad, "session_dead", AttentionTarget::Sessions, vec![dead.name.clone()]);
        }
        if let Some(secs) = sessions.oldest_queued_secs.filter(|secs| *secs >= QUEUE_STUCK_SECS) {
            push(Severity::Warn, "queue_stuck", AttentionTarget::Sessions, vec![(secs / 60).to_string()]);
        }
    }
    if let Some(slow) = &snapshot.slow {
        let errors: u64 = slow.hours.iter().map(|hour| u64::from(hour.errors)).sum();
        if errors > 0 {
            push(Severity::Warn, "server_errors", AttentionTarget::Slow, vec![errors.to_string()]);
        }
        if !slow.watchdog.is_empty() {
            push(Severity::Warn, "watchdog", AttentionTarget::Slow, vec![slow.watchdog.len().to_string()]);
        }
    }
    if snapshot.outside.as_ref().and_then(|o| o.keyring_locked) == Some(true) {
        push(Severity::Warn, "keyring_locked", AttentionTarget::Outside, Vec::new());
    }
    // Estável: dentro de cada severidade fica a ordem das regras.
    out.sort_by_key(|item| item.severity == Severity::Warn);
    out
}

/// A versão `older` é anterior a `than`, número a número (`0.1.0.10790` < `0.1.0.10824`). Versão que
/// não é só de números não se compara: nunca vira "desatualizada" por engano.
pub fn older(older: &str, than: &str) -> bool {
    match (numbers(older), numbers(than)) {
        (Some(a), Some(b)) => a < b,
        _ => false,
    }
}

fn numbers(version: &str) -> Option<Vec<u64>> {
    version.trim().split('.').map(|part| part.parse().ok()).collect()
}

fn pct(disk: DiskUse) -> f64 {
    if disk.total == 0 { 0.0 } else { disk.used as f64 * 100.0 / disk.total as f64 }
}

fn whole(pct: f64) -> String { (pct.floor() as u64).to_string() }

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_api::status::{
        AppReport, DeadSession, DirSize, Exit, ExitsBlock, HostBlock, HourBucket, Machine, OutsideBlock, SessionsBlock,
        SlowBlock, WatchdogEvent, WatchdogKind,
    };

    const GB: u64 = 1_000_000_000;

    fn empty() -> StatusSnapshot {
        StatusSnapshot {
            generated_at: "2026-11-17T14:00:00Z".into(), server: None, exits: None, update: None, machines: None,
            sessions: None, slow: None, host: None, outside: None, leftovers: None, attention: Vec::new(),
        }
    }

    fn machine(label: &str, state: MachineState) -> Machine {
        Machine { id: label.to_lowercase(), label: label.into(), this_machine: false, state, down_since: None,
            latency_ms: None, server_version: None, outdated: false, lan: false, app: None }
    }

    fn app(host: &str, version: &str) -> AppReport {
        AppReport { version: version.into(), rss_bytes: 1, cpu_pct: 0.0, platform: "linux".into(), host: host.into() }
    }

    fn host(home_used: u64, tmp_used: Option<u64>) -> HostBlock {
        let none = DirSize { bytes: None, computing: false };
        HostBlock { mem_total: 8 * GB, mem_available: 4 * GB, swap_total: 0, swap_used: 0, load: None, cpus: 4,
            disk_home: DiskUse { used: home_used, total: 100 * GB },
            disk_tmp: tmp_used.map(|used| DiskUse { used, total: 100 * GB }), dir_hangar: none, dir_build_cache: none }
    }

    fn sessions(dead: &[&str], oldest: Option<u64>) -> SessionsBlock {
        SessionsBlock { open: 1, by_provider: vec![], by_process: vec![],
            dead: dead.iter().map(|name| DeadSession { name: (*name).into(), cause: "terminal_died".into() }).collect(),
            asleep: 0, queued: u32::from(oldest.is_some()), oldest_queued_secs: oldest }
    }

    fn codes(list: &[Attention]) -> Vec<(&str, Severity)> { list.iter().map(|a| (a.code.as_str(), a.severity)).collect() }

    #[test]
    fn a_healthy_snapshot_has_no_attention() {
        let mut snap = empty();
        let mut this = machine("Notebook", MachineState::Ok);
        this.this_machine = true;
        this.app = Some(app("notebook", "0.1.0.10824"));
        snap.machines = Some(vec![this, machine("Casa", MachineState::Ok), machine("Desligado", MachineState::Unknown)]);
        snap.host = Some(host(50 * GB, Some(10 * GB)));
        snap.sessions = Some(sessions(&[], Some(599)));
        snap.slow = Some(SlowBlock { hours: vec![HourBucket { hour_start: "x".into(), slow: 3, errors: 0 }], top_routes: vec![], watchdog: vec![] });
        snap.outside = Some(OutsideBlock { connect: None, funnel: Some(false), keyring_locked: Some(false) });
        assert_eq!(attention(&snap, Some("0.1.0.10824")), vec![]);
        assert_eq!(attention(&empty(), None), vec![], "sem bloco, sem ponto");
    }

    #[test]
    fn bad_comes_first_and_each_rule_keeps_its_order() {
        let mut snap = empty();
        let mut vps = machine("VPS", MachineState::Down);
        vps.down_since = Some("2026-11-17T12:23:05Z".into());
        let mut casa = machine("Casa", MachineState::Ok);
        (casa.outdated, casa.server_version, casa.app) = (true, Some("0.1.0.10790".into()), Some(app("casa", "0.1.0.10790")));
        snap.machines = Some(vec![vps, casa]);
        snap.exits = Some(ExitsBlock { items: vec![
            Exit { ts: "a".into(), kind: ExitKind::Update, cause: None, from: None, to: None, error_lines: vec![] },
            Exit { ts: "b".into(), kind: ExitKind::Crash, cause: Some("out_of_memory".into()), from: None, to: None, error_lines: vec![] },
        ] });
        snap.host = Some(host(92 * GB, Some(80 * GB)));
        snap.sessions = Some(sessions(&["hangar-3"], Some(840)));
        snap.slow = Some(SlowBlock {
            hours: vec![HourBucket { hour_start: "x".into(), slow: 0, errors: 2 }, HourBucket { hour_start: "y".into(), slow: 1, errors: 1 }],
            top_routes: vec![], watchdog: vec![WatchdogEvent { ts: "z".into(), kind: WatchdogKind::Late, ms: Some(4200) }],
        });
        snap.outside = Some(OutsideBlock { connect: None, funnel: None, keyring_locked: Some(true) });
        let list = attention(&snap, Some("0.1.0.10824"));
        assert_eq!(codes(&list), vec![
            ("machine_down", Severity::Bad), ("exits_crash", Severity::Bad), ("disk_full", Severity::Bad), ("session_dead", Severity::Bad),
            ("machine_outdated", Severity::Warn), ("app_version_differs", Severity::Warn), ("tmp_full", Severity::Warn),
            ("queue_stuck", Severity::Warn), ("server_errors", Severity::Warn), ("watchdog", Severity::Warn), ("keyring_locked", Severity::Warn),
        ]);
        let args: Vec<Vec<&str>> = list.iter().map(|a| a.args.iter().map(String::as_str).collect()).collect();
        assert_eq!(args, vec![
            vec!["VPS", "2026-11-17T12:23:05Z"], vec!["1"], vec!["92"], vec!["hangar-3"],
            vec!["Casa", "0.1.0.10790"], vec!["casa", "0.1.0.10790", "0.1.0.10824"], vec!["80"],
            vec!["14"], vec!["3"], vec!["1"], vec![],
        ]);
        let targets: Vec<AttentionTarget> = list.iter().map(|a| a.target).collect();
        assert_eq!(targets, vec![
            AttentionTarget::Machines, AttentionTarget::Exits, AttentionTarget::Host, AttentionTarget::Sessions,
            AttentionTarget::Machines, AttentionTarget::Machines, AttentionTarget::Host, AttentionTarget::Sessions,
            AttentionTarget::Slow, AttentionTarget::Slow, AttentionTarget::Outside,
        ]);
    }

    #[test]
    fn disk_and_queue_thresholds() {
        let at = |used: u64| {
            let mut snap = empty();
            snap.host = Some(host(used, None));
            codes(&attention(&snap, None)).into_iter().map(|(c, s)| (c.to_owned(), s)).collect::<Vec<_>>()
        };
        assert_eq!(at(74 * GB + 900_000_000), vec![], "74,9 % não chega");
        assert_eq!(at(75 * GB), vec![("disk_full".to_owned(), Severity::Warn)]);
        assert_eq!(at(90 * GB), vec![("disk_full".to_owned(), Severity::Bad)]);

        let queue = |secs: u64| {
            let mut snap = empty();
            snap.sessions = Some(sessions(&[], Some(secs)));
            attention(&snap, None)
        };
        assert_eq!(queue(599), vec![]);
        assert_eq!(queue(600)[0].args, vec!["10".to_owned()]);
    }

    #[test]
    fn versions_compare_only_when_known() {
        let mut snap = empty();
        let mut this = machine("Notebook", MachineState::Ok);
        (this.this_machine, this.app) = (true, Some(app("notebook", "0.1.0.10790")));
        let mut down_and_old = machine("VPS", MachineState::Down);
        down_and_old.outdated = true;
        snap.machines = Some(vec![this, down_and_old]);
        assert_eq!(codes(&attention(&snap, None)), vec![("machine_down", Severity::Bad)],
            "sem a versão deste servidor o app não se compara, e a máquina fora não vira desatualizada");
        assert_eq!(attention(&snap, None)[0].args, vec!["VPS".to_owned(), String::new()], "fora desde desconhecido");

        assert!(older("0.1.0.10790", "0.1.0.10824"));
        assert!(older("0.1.0.9", "0.1.0.10"), "número a número, não texto");
        assert!(!older("0.1.0.10824", "0.1.0.10824"));
        assert!(!older("0.1.1.0", "0.1.0.99"));
        assert!(!older("f8ef8e932", "0.1.0.10824"), "commit do modo desenvolvedor não se compara");
    }
}
