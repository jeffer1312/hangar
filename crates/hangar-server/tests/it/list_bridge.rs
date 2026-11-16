//! Rodada da lista com sessões Claude num `tmux` de mentira.
#![cfg(unix)]
use crate::fake;

use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() }

/// Ponte com `n` sessões Claude paradas num `tmux` de mentira (pane pronto); `marker` é o estado e a
/// idade (s) do marcador de cada uma.
async fn bridge_with_sessions(root: &Path, n: usize, marker: (&str, f64)) -> ListBridge {
    let home = root.join("home");
    let mut panes = String::new();
    for i in 0..n {
        let cwd = root.join(format!("w{i}"));
        let sid = format!("00000000-0000-0000-0000-{i:012}");
        let proj = home.join(".claude/projects").join(hangar_workspace::worktrees::sanitize_cwd(cwd.to_str().unwrap()));
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let mut lines = String::new();
        for n in 0..200 { lines.push_str(&fake::claude_line(n)); lines.push('\n'); }
        std::fs::write(proj.join(format!("{sid}.jsonl")), lines).unwrap();
        std::fs::create_dir_all(home.join(".claude/.hangar-state")).unwrap();
        std::fs::write(home.join(format!(".claude/.hangar-state/{sid}.json")),
            format!(r#"{{"state":"{}","ts":{}}}"#, marker.0, now() - marker.1)).unwrap();
        panes.push_str(&format!("s{i}\\t1\\t\\t{}\\t%%{i}\\t\\t\\t\\t0\\t0\\n", cwd.display()));
    }
    let script = root.join("tmux");
    crate::write_executable(&script, format!("#!/bin/sh\n[ \"$1\" = list-panes ] || {{ printf '● pronto\\n❯\\n'; exit 0; }}\nprintf '{panes}'\n"));
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let bridge = ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.clone().into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new());
    bridge
}

/// Lista só de Claude: o pane pronto que contradiz o `awaiting` velho rebaixa o marcador no Rust.
#[tokio::test(flavor = "multi_thread")]
async fn claude_only_round_demotes() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = bridge_with_sessions(dir.path(), 1, ("awaiting_input", 60.0)).await;
    bridge.set_problem_hold(Duration::ZERO);
    let p = bridge.produce(&Default::default()).await.unwrap();
    assert_eq!(p.rows.len(), 1);
    assert!(p.facts_ok && !p.facts.unknown);
    assert_eq!(p.rows[0].problema, None);
    let sid = "00000000-0000-0000-0000-000000000000";
    let path = dir.path().join(format!("home/.claude/.hangar-state/{sid}.json"));
    let marker: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert!(bridge.demoted.applies(sid, marker["ts"].as_f64().unwrap()), "o rebaixamento vale já no Rust, para a lista e o `Monitor`");
    // O Rust regrava o marcador, com o `ts` original.
    let rewritten = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let m: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
            if m["state"] == "idle" { return m; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }).await.expect("o marcador é regravado idle");
    assert_eq!(rewritten["ts"], marker["ts"]);
}

fn peak_rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status").unwrap().lines()
        .find_map(|l| l.strip_prefix("VmHWM:")).and_then(|v| v.trim().trim_end_matches("kB").trim().parse().ok()).unwrap_or(0)
}

/// Medida do tique com 20 sessões paradas (marcador `idle`, transcript de 200 linhas): rodar em
/// release com `--ignored --nocapture`. O multiplexador de mentira responde na hora.
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn measure_tick_20_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let bridge = bridge_with_sessions(dir.path(), 20, ("idle", -60.0)).await;
    let first = bridge.produce(&Default::default()).await.unwrap();
    assert!(first.rows.iter().all(|r| r.problema.is_none()));
    assert_eq!(first.rows.len(), 20);
    let rss0 = peak_rss_kb();
    let cpu = |s: &str| -> f64 { let f: Vec<&str> = s.rsplit(')').next().unwrap().split_whitespace().collect();
        (11..=14).map(|i| f[i].parse::<f64>().unwrap()).sum::<f64>() / 100.0 };
    let cpu0 = cpu(&std::fs::read_to_string("/proc/self/stat").unwrap());
    let t0 = std::time::Instant::now();
    let n = 300;
    for i in 0..n {
        bridge.invalidate();
        let input = hangar_server::list::bridge::ProduceFacts { owner_clients: i % 2 };
        let p = bridge.produce(&input).await.unwrap();
        assert_eq!(p.rows.len(), 20);
    }
    let wall = t0.elapsed().as_secs_f64() / n as f64;
    let cpu_tick = (cpu(&std::fs::read_to_string("/proc/self/stat").unwrap()) - cpu0) / n as f64;
    println!("tique (20 sessões): parede {:.2} ms, CPU (com filhos) {:.2} ms, pico RSS {} → {} kB",
        wall * 1e3, cpu_tick * 1e3, rss0, peak_rss_kb());
}

