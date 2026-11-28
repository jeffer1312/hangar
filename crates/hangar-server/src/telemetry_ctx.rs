//! O retrato da máquina e da sessão na hora de uma falha, em valores fechados de `events::CONTEXT`, e o
//! ambiente da máquina (`events::MACHINE`) que vai em cada pacote.
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

use hangar_api::session::SessionRow;
use hangar_api::status::{DirSize, HostBlock};
use hangar_telemetry::events::{self, Provider, Surface};

use crate::routes::AppState;

/// O mesmo leitor entre falhas: cada leitura só refaz memória, núcleos e discos.
static HOST: LazyLock<Mutex<hangar_status::host::HostReader>> = LazyLock::new(Default::default);
/// A versão da CLI de cada provedor, do último retrato do ambiente.
static CLI: LazyLock<Mutex<BTreeMap<Provider, String>>> = LazyLock::new(Default::default);
/// A CLI de cada provedor e a chave dela em `events::MACHINE`.
const CLIS: [(Provider, &str, &str); 6] = [(Provider::Claude, "claude", "cli.claude"), (Provider::Codex, "codex", "cli.codex"),
    (Provider::Pi, "pi", "cli.pi"), (Provider::Omp, "omp", "cli.omp"), (Provider::Kimi, "kimi", "cli.kimi"),
    (Provider::Antigravity, "agy", "cli.antigravity")];
/// `sessions.<provedor>.<superfície>`; a chave do retrato é texto fixo.
static SESSION_KEYS: LazyLock<Vec<((Provider, Surface), &'static str)>> = LazyLock::new(|| {
    Provider::ALL.iter().flat_map(|p| Surface::ALL.iter().map(move |s| {
        ((*p, *s), &*Box::leak(format!("sessions.{}.{}", p.as_str(), s.as_str()).into_boxed_str()))
    })).collect()
});

/// A última leitura vale por alguns segundos: falhas em rajada não releem memória e discos a cada uma.
static LAST: Mutex<Option<(std::time::Instant, Option<HostBlock>)>> = Mutex::new(None);
const HOST_FRESH: std::time::Duration = std::time::Duration::from_secs(15);

fn host() -> Option<HostBlock> {
    const NONE: DirSize = DirSize { bytes: None, computing: false };
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, block)) = last.as_ref().filter(|(at, _)| at.elapsed() < HOST_FRESH) { return block.clone(); }
    let block = HOST.lock().unwrap_or_else(|e| e.into_inner()).read(NONE, NONE);
    *last = Some((std::time::Instant::now(), block.clone()));
    block
}

/// Servidor de pé há, memória e disco livres, carga e sessões vivas por provedor e superfície.
pub(crate) fn machine(st: &AppState) -> Vec<(&'static str, String)> { machine_later(st)() }

/// O mesmo retrato, com a lista lida agora e memória e discos lidos por quem chamar: fora da thread do runtime.
pub(crate) fn machine_later(st: &AppState) -> impl FnOnce() -> Vec<(&'static str, String)> + Send + 'static {
    let rows = st.list.last_rows();
    move || machine_from(crate::telemetry_hooks::uptime().as_secs(), host().as_ref(), cpu_pct(), rows.as_deref().map(Vec::as_slice))
}

/// Provedor, superfície, modelo, estado e versão da CLI da sessão `name`, pela última lista boa.
pub(crate) fn session(st: &AppState, name: &str) -> Vec<(&'static str, String)> {
    let Some(rows) = st.list.last_rows() else { return Vec::new() };
    let Some(row) = rows.iter().find(|row| row.name == name) else { return Vec::new() };
    let cli = CLI.lock().unwrap_or_else(|e| e.into_inner()).get(&kind_of(row).0).cloned();
    session_from(row, cli.as_deref())
}

/// O ambiente da máquina para `Telemetry::set_machine`; guarda as versões das CLIs para o retrato das sessões.
pub(crate) async fn environment(st: &AppState) -> Vec<(&'static str, String)> {
    use sysinfo::System;
    let env = &st.accounts.env;
    let versions = futures_util::future::join_all(CLIS.iter().map(|(_, cli, _)| st.harness_probe.version_text(env, cli))).await;
    let mut facts = os_facts(std::env::consts::OS, &System::distribution_id(), System::os_version().as_deref(), System::kernel_version().as_deref());
    if let Some(host) = tokio::task::spawn_blocking(host).await.ok().flatten() {
        facts.push(("cpus", events::cpus_band(host.cpus).as_str().to_owned()));
        facts.push(("ram_total", events::ram_band(host.mem_total).as_str().to_owned()));
    }
    let mux = st.list.env().mux.version().await;
    facts.push(("mux", match &mux { None => "none", Some(_) if cfg!(windows) => "psmux", Some(_) => "tmux" }.to_owned()));
    // O psmux diz também a versão do tmux que imita: vale a dele.
    if let Some(version) = mux.as_deref().map(|text| text.split_once("psmux").map_or(text, |(_, after)| after)).and_then(public_version) {
        facts.push(("mux_version", version));
    }
    let mut clis = BTreeMap::new();
    for ((provider, _, key), version) in CLIS.iter().zip(versions) {
        let version = match version { None => Some("absent".to_owned()), Some(text) => public_version(&text) };
        if let Some(version) = version {
            clis.insert(*provider, version.clone());
            facts.push((*key, version));
        }
    }
    *CLI.lock().unwrap_or_else(|e| e.into_inner()) = clis;
    let accounts = st.accounts.clone();
    let (claude, codex, connect) = tokio::task::spawn_blocking(move || {
        let connect = crate::connect::state::read(&accounts.env.home.join(".hangar").join("connect")).ok().map(|linked| linked.is_some());
        (accounts.claude_listed().ok().map(|a| a.len()), accounts.snapshot(crate::accounts::types::Provider::Codex).ok().map(|a| a.len()), connect)
    }).await.unwrap_or_default();
    for (key, count) in [("accounts.claude", claude), ("accounts.codex", codex)] {
        if let Some(count) = count { facts.push((key, events::count_band(count as u64).as_str().to_owned())); }
    }
    let on = |on: bool| if on { "on" } else { "off" }.to_owned();
    if let Some(connect) = connect { facts.push(("connect", on(connect))); }
    facts.push(("tailscale", on(crate::machine_peers::network::tailscale().await["BackendState"] == "Running")));
    facts
}

fn kind_of(row: &SessionRow) -> (Provider, Surface) { crate::session_write::input::wire_session(&row.provider, !row.headless) }

/// Só o que `events::CONTEXT` conhece sai, já na lista dele.
fn closed(facts: Vec<(&'static str, String)>) -> Vec<(&'static str, String)> {
    facts.into_iter().filter_map(|(key, value)| Some((key, events::closed(events::CONTEXT, key, &value)?))).collect()
}

fn machine_from(up: u64, host: Option<&HostBlock>, cpu: Option<f32>, rows: Option<&[SessionRow]>) -> Vec<(&'static str, String)> {
    let band = |code: events::Code| code.as_str().to_owned();
    let mut facts = vec![("up", band(events::age_band(up)))];
    if let Some(host) = host {
        if let Some(mem) = events::mem_free_band(host.mem_available, host.mem_total) { facts.push(("mem_free", band(mem))); }
        facts.push(("disk_free", band(events::disk_free_band(host.disk_home.total.saturating_sub(host.disk_home.used)))));
        match host.load {
            Some([one, ..]) => facts.extend(events::load_band(one, host.cpus).map(|load| ("load", band(load)))),
            None => facts.extend(cpu.map(|pct| ("cpu", band(events::cpu_pct_band(pct))))),
        }
    }
    if let Some(rows) = rows {
        facts.push(("sessions", band(events::count_band(rows.len() as u64))));
        facts.push(("working", band(events::count_band(rows.iter().filter(|row| row.state == "working").count() as u64))));
        let mut by_kind: BTreeMap<&'static str, u64> = BTreeMap::new();
        for row in rows {
            let kind = kind_of(row);
            if let Some((_, key)) = SESSION_KEYS.iter().find(|(known, _)| *known == kind) { *by_kind.entry(key).or_default() += 1; }
        }
        facts.extend(by_kind.into_iter().map(|(key, n)| (key, band(events::count_band(n)))));
    }
    closed(facts)
}

fn session_from(row: &SessionRow, cli: Option<&str>) -> Vec<(&'static str, String)> {
    let (provider, surface) = kind_of(row);
    let engine = row.engine.as_deref().is_some_and(|engine| !engine.is_empty());
    let mut facts = vec![("provider", provider.as_str().to_owned()), ("surface", surface.as_str().to_owned()),
        ("model", events::model(provider, row.model.as_deref().unwrap_or_default(), engine).as_str().to_owned()),
        ("state", row.state.clone())];
    if let Some(cli) = cli { facts.push(("cli", cli.to_owned())); }
    closed(facts)
}

/// Sistema, distribuição e versão; no Windows o build separa o 10 do 11.
fn os_facts(os: &str, distribution: &str, os_version: Option<&str>, kernel: Option<&str>) -> Vec<(&'static str, String)> {
    let (os, name, version) = match os {
        "linux" => ("linux", distribution.to_owned(), os_version.and_then(public_version)),
        "macos" => ("macos", "macos".to_owned(), os_version.and_then(public_version)),
        "windows" => {
            let build = kernel.and_then(|k| k.split(|c: char| !c.is_ascii_digit()).filter(|n| n.len() >= 5).last()).map(str::to_owned);
            let eleven = build.as_deref().and_then(|b| b.parse::<u32>().ok()).is_some_and(|b| b >= 22_000);
            ("windows", if eleven { "windows_11" } else { "windows_10" }.to_owned(), build)
        }
        _ => return vec![("os", "other".to_owned())],
    };
    let mut facts = vec![("os", os.to_owned()), ("os_name", name)];
    if let Some(version) = version { facts.push(("os_version", version)); }
    facts.into_iter().filter_map(|(key, value)| Some((key, events::closed(events::MACHINE, key, &value)?))).collect()
}

/// O primeiro número de versão do texto, sem sufixo; uma letra só no fim do número fica.
fn public_version(text: &str) -> Option<String> {
    text.split(|c: char| c.is_whitespace() || matches!(c, ',' | '(' | ')')).find_map(|token| {
        let token = token.strip_prefix('v').unwrap_or(token);
        let end = token.find(|c: char| !c.is_ascii_digit() && c != '.').unwrap_or(token.len());
        let (number, rest) = token.split_at(end);
        let number = number.trim_end_matches('.');
        if !number.starts_with(|c: char| c.is_ascii_digit()) { return None; }
        let letter = rest.chars().next().filter(|c| c.is_ascii_lowercase() && rest.len() == 1);
        Some(letter.map_or_else(|| number.to_owned(), |letter| format!("{number}{letter}")))
    })
}

/// O Windows não tem carga média: uso entre esta leitura e a anterior (a primeira não tem base).
#[cfg(windows)]
fn cpu_pct() -> Option<f32> {
    static CPU: LazyLock<Mutex<(sysinfo::System, bool)>> = LazyLock::new(Default::default);
    let mut cpu = CPU.lock().unwrap_or_else(|e| e.into_inner());
    cpu.0.refresh_cpu_usage();
    std::mem::replace(&mut cpu.1, true).then(|| cpu.0.global_cpu_usage())
}

#[cfg(not(windows))]
fn cpu_pct() -> Option<f32> { None }

#[cfg(test)]
mod tests {
    use super::*;
    use hangar_api::status::DiskUse;
    use serde_json::json;

    fn row(name: &str, provider: &str, headless: bool, state: &str, model: Option<&str>, engine: Option<&str>) -> SessionRow {
        serde_json::from_value(json!({"name": name, "provider": provider, "headless": headless, "state": state,
            "model": model, "engine": engine})).unwrap()
    }

    fn map(facts: Vec<(&'static str, String)>) -> BTreeMap<&'static str, String> { facts.into_iter().collect() }

    fn expected(pairs: &[(&'static str, &str)]) -> BTreeMap<&'static str, String> {
        pairs.iter().map(|(k, v)| (*k, (*v).to_owned())).collect()
    }

    #[test]
    fn telemetry_ctx_machine_bands_the_host_and_counts_the_sessions() {
        const G: u64 = 1 << 30;
        const NONE: DirSize = DirSize { bytes: None, computing: false };
        let mut host = HostBlock { mem_total: 100, mem_available: 9, swap_total: 0, swap_used: 0, load: Some([6.0, 1.0, 1.0]), cpus: 4,
            disk_home: DiskUse { used: 96 * G, total: 100 * G }, disk_tmp: None, dir_hangar: NONE, dir_build_cache: NONE };
        let rows = [row("minha-sessao", "claude", false, "working", None, None), row("b", "claude", false, "idle", None, None),
            row("c", "codex", true, "working", None, None), row("d", "meu-motor", true, "idle", None, None)];
        assert_eq!(map(machine_from(30, Some(&host), None, Some(&rows))), expected(&[("up", "lt1m"), ("mem_free", "lt10"),
            ("disk_free", "lt5g"), ("load", "lt2"), ("sessions", "4-9"), ("working", "2-3"), ("sessions.claude.terminal", "2-3"),
            ("sessions.codex.headless", "1"), ("sessions.other.headless", "1")]));
        // Sem carga média (Windows), o uso do processador; sem lista boa, sem contagem.
        host.load = None;
        assert_eq!(map(machine_from(7200, Some(&host), Some(85.0), None)), expected(&[("up", "lt1d"), ("mem_free", "lt10"),
            ("disk_free", "lt5g"), ("cpu", "ge80")]));
        assert_eq!(map(machine_from(0, None, None, Some(&[]))), expected(&[("up", "lt1m"), ("sessions", "0"), ("working", "0")]));
    }

    #[test]
    fn telemetry_ctx_session_says_only_closed_values() {
        let claude = row("minha-sessao", "claude", false, "awaiting_input", Some("opus"), None);
        assert_eq!(map(session_from(&claude, Some("2.1.195"))), expected(&[("provider", "claude"), ("surface", "terminal"),
            ("model", "opus"), ("state", "awaiting_input"), ("cli", "2.1.195")]));
        let engine = row("x", "claude", true, "dead", Some("meu-motor"), Some("meu-motor"));
        assert_eq!(map(session_from(&engine, None)), expected(&[("provider", "claude"), ("surface", "headless"),
            ("model", "engine"), ("state", "other")]));
        let odd = row("x", "novo", false, "idle", Some("claude-opus-9"), Some(""));
        assert_eq!(map(session_from(&odd, None)), expected(&[("provider", "other"), ("surface", "terminal"),
            ("model", "other"), ("state", "idle")]));
    }

    #[test]
    fn telemetry_ctx_environment_reads_only_the_number_of_each_version() {
        assert_eq!(public_version("2.1.195 (Claude Code)").as_deref(), Some("2.1.195"));
        assert_eq!(public_version("codex-cli 0.120.0").as_deref(), Some("0.120.0"));
        assert_eq!(public_version("tmux 3.5a").as_deref(), Some("3.5a"));
        assert_eq!(public_version("kimi, version v1.2.3-beta").as_deref(), Some("1.2.3"));
        assert_eq!(public_version("tmux next-3.6"), None);
        assert_eq!(public_version(""), None);
        assert_eq!(map(os_facts("linux", "cachyos", None, Some("7.1.3-2-cachyos"))), expected(&[("os", "linux"), ("os_name", "cachyos")]));
        assert_eq!(map(os_facts("linux", "ubuntu", Some("24.04"), None)), expected(&[("os", "linux"), ("os_name", "ubuntu"), ("os_version", "24.04")]));
        assert_eq!(map(os_facts("linux", "Minha Distro", Some("1"), None)), expected(&[("os", "linux"), ("os_name", "other"), ("os_version", "1")]));
        assert_eq!(map(os_facts("windows", "windows", Some("11 (22631)"), Some("22631"))),
            expected(&[("os", "windows"), ("os_name", "windows_11"), ("os_version", "22631")]));
        assert_eq!(map(os_facts("windows", "windows", Some("10 (19045)"), Some("19045"))),
            expected(&[("os", "windows"), ("os_name", "windows_10"), ("os_version", "19045")]));
        assert_eq!(map(os_facts("macos", "macos", Some("15.1.1"), Some("24.1.0"))),
            expected(&[("os", "macos"), ("os_name", "macos"), ("os_version", "15.1.1")]));
        assert_eq!(map(os_facts("freebsd", "freebsd", Some("14.1"), None)), expected(&[("os", "other")]));
    }
}
