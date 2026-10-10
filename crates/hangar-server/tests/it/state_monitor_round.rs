//! Medida de uma rodada do `state::Monitor` (captura pelo `TerminalPool` + `reduce`) num tmux
//! isolado (`-L`), para o `docs/migracao-rust/parte4/medicao.md`. Só em release e à mão:
//! `cargo test --release -p hangar-server --test state_monitor_round -- --ignored --nocapture`.
#![cfg(target_os = "linux")]
use hangar_server::state::monitor::PoolCapture;
use hangar_server::terminal_control::{Limits, TerminalPool};
use hangar_server::terminal_state::{reduce_analysis, ReducerFacts, ReducerMemory};
use std::process::Command;
use std::time::Instant;

const WARMUP: usize = 20;
const ROUNDS: usize = 3000;
const BATCHES: usize = 4;

/// utime + stime do processo, em ticks de `/proc/<pid>/stat`.
fn cpu_ticks(pid: &str) -> u64 {
    let raw = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let fields: Vec<&str> = raw[raw.rfind(')').unwrap() + 2..].split(' ').collect();
    fields[11].parse::<u64>().unwrap() + fields[12].parse::<u64>().unwrap()
}

fn tmux(label: &str, args: &[&str]) -> String {
    let out = Command::new("tmux").args(["-u", "-L", label]).args(args).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

async fn round(capture: &PoolCapture, memory: &mut ReducerMemory) {
    let frame = capture.capture().await.expect("captura");
    let (r, _) = reduce_analysis(frame.analysis, std::mem::take(memory), ReducerFacts::default());
    *memory = r.memory;
    assert_eq!(r.analysis.state, if memory.frozen >= 3 { "idle" } else { "working" });
}

#[tokio::test]
#[ignore = "medida manual em release"]
async fn one_round_cost() {
    let label = format!("hangar-medida-{}", std::process::id());
    // Pane 100×40 com 180 linhas de resposta e spinner parado, como a medida do Python.
    let script = "i=0; while [ $i -lt 180 ]; do echo \"● linha $i de uma resposta com texto comum\"; i=$((i+1)); done; \
        printf '✻ Thinking…\\n────────────\\n❯\\n────────────\\n⏵⏵ bypass permissions on (shift+tab to cycle)'; sleep 100000";
    tmux(&label, &["-f", "/dev/null", "new-session", "-d", "-s", "fixture", "-x", "100", "-y", "40", "sh", "-c", script]);
    std::thread::sleep(std::time::Duration::from_millis(300));
    let socket = tmux(&label, &["display-message", "-p", "#{socket_path}"]).trim().to_owned();
    let server = tmux(&label, &["display-message", "-p", "#{pid}"]).trim().to_owned();
    let pool = TerminalPool::with_program("tmux", Some(socket.into()), Limits::default());
    let capture = PoolCapture::new(pool, "fixture", "medida", "=fixture:".into());
    let me = std::process::id().to_string();
    let mut memory = ReducerMemory::default();
    for _ in 0..WARMUP { round(&capture, &mut memory).await; }
    // O cliente `-C` do pool fica vivo durante a medida: a CPU dele entra na conta.
    let client = tmux(&label, &["list-clients", "-F", "#{client_pid}"]).trim().to_owned();
    assert!(!client.is_empty() && !client.contains('\n'), "um cliente de controle");
    let tick_ms = 1000.0 / 100.0;
    let per = |ticks: u64| ticks as f64 * tick_ms / ROUNDS as f64;
    for batch in 0..BATCHES {
        let (rust0, tmux0, client0, wall0) = (cpu_ticks(&me), cpu_ticks(&server), cpu_ticks(&client), Instant::now());
        for _ in 0..ROUNDS { round(&capture, &mut memory).await; }
        let wall = wall0.elapsed().as_secs_f64() * 1000.0 / ROUNDS as f64;
        let (rust, server_cpu, client_cpu) = (per(cpu_ticks(&me) - rust0), per(cpu_ticks(&server) - tmux0), per(cpu_ticks(&client) - client0));
        println!("lote {batch}: rodada {wall:.3} ms parede; CPU Rust {rust:.3} ms, servidor tmux {server_cpu:.3} ms, \
            cliente -C {client_cpu:.3} ms, total {:.3} ms", rust + server_cpu + client_cpu);
    }
    // Só o redutor (análise + reduce), sem captura.
    let text = capture.capture().await.unwrap().text;
    let t0 = Instant::now();
    let mut m = ReducerMemory::default();
    for _ in 0..ROUNDS {
        let (r, _) = reduce_analysis(hangar_server::terminal_state::analyze(&text), m, ReducerFacts::default());
        m = r.memory;
    }
    println!("analyze + reduce: {:.3} ms", t0.elapsed().as_secs_f64() * 1000.0 / ROUNDS as f64);
    let hwm = std::fs::read_to_string("/proc/self/status").unwrap().lines()
        .find_map(|l| l.strip_prefix("VmHWM:").map(|v| v.trim().to_owned())).unwrap();
    println!("pico de memória do processo de teste: {hwm}");
    capture.release().await;
    let _ = Command::new("tmux").args(["-L", &label, "kill-server"]).output();
}
