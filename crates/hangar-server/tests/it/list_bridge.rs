//! Ponte privada `list.*`: só na porta privada com o segredo, uma descoberta (e uma produção) por
//! vez, e `newer_than` força uma descoberta nova. O multiplexador é um script que conta as chamadas.
#![cfg(unix)]
use crate::fake;

use fake::{SECRET, client, config};
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, terminal_router};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// `tmux` de mentira: anota o subcomando, demora um pouco (para as perguntas se cruzarem) e lista
/// uma sessão sem pid; `exit` diferente de 0 faz o `list-panes` recusar.
fn fake_mux(dir: &Path, exit: i32) -> std::path::PathBuf {
    let log = dir.join("calls.log");
    let script = dir.join("tmux");
    std::fs::write(&script, format!(
        "#!/bin/sh\necho \"$1\" >> '{}'\nsleep 0.3\n[ \"$1\" = list-panes ] || exit 0\n[ {exit} = 0 ] || exit {exit}\nprintf 'alpha\\t1\\t\\t{}\\t%%1\\t\\t\\t\\t0\\t0\\n'\n",
        log.display(), dir.display())).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn calls(dir: &Path, sub: &str) -> usize {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default().lines().filter(|l| *l == sub).count()
}

async fn spawn(dir: &Path, exit: i32) -> SocketAddr {
    spawn_with(dir, exit, "127.0.0.1:9".parse().unwrap()).await
}

/// `facts`: o Python dos fatos; o de `spawn` não responde.
async fn spawn_with(dir: &Path, exit: i32, facts: SocketAddr) -> SocketAddr {
    let script = fake_mux(dir, exit);
    let home = dir.join("home");
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    assert!(dirs.is_some());
    let mut state = AppState::new(config("127.0.0.1:9".parse().unwrap(), ""));
    state.list = Arc::new(ListBridge::new(ListEnv {
        mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(),
        procs: Arc::new(hangar_server::list::procs::SystemProcs::default()),
        dirs,
    }, FactsClient::new(facts, SECRET.into())));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = terminal_router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

async fn call(addr: SocketAddr, secret: Option<&str>, op: &str, args: Value) -> (u16, Value) {
    let mut req = client().post(format!("http://{addr}/__hangar_server/list"))
        .header("content-type", "application/json").body(json!({"op": op, "args": args}).to_string());
    if let Some(s) = secret {
        req = req.header("x-hangar-internal", s);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status().as_u16();
    (status, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
}

fn names(v: &Value) -> Vec<&str> {
    v["result"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect()
}

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() }

#[tokio::test]
async fn requires_secret() {
    let dir = tempfile::tempdir().unwrap();
    let addr = spawn(dir.path(), 0).await;
    assert_eq!(call(addr, None, "list.invalidate", json!({})).await.0, 404);
    assert_eq!(call(addr, Some("outro"), "list.invalidate", json!({})).await.0, 404);
    let (status, body) = call(addr, Some(SECRET), "list.discover", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(names(&body), ["alpha"]);
}

#[tokio::test]
async fn discover_is_single_flight() {
    let dir = tempfile::tempdir().unwrap();
    let addr = spawn(dir.path(), 0).await;
    let (a, b) = tokio::join!(call(addr, Some(SECRET), "list.discover", json!({})),
                              call(addr, Some(SECRET), "list.discover", json!({})));
    assert_eq!((names(&a.1), names(&b.1)), (vec!["alpha"], vec!["alpha"]));
    assert_eq!(calls(dir.path(), "list-panes"), 1, "a segunda esperou a primeira e reaproveitou");
}

#[tokio::test]
async fn newer_than_forces_fresh() {
    let dir = tempfile::tempdir().unwrap();
    let addr = spawn(dir.path(), 0).await;
    let before = now();
    call(addr, Some(SECRET), "list.discover", json!({})).await;
    call(addr, Some(SECRET), "list.discover", json!({})).await;
    assert_eq!(calls(dir.path(), "list-panes"), 1, "dentro de 1 s reaproveita");
    call(addr, Some(SECRET), "list.discover", json!({"newer_than": before})).await;
    assert_eq!(calls(dir.path(), "list-panes"), 2, "a comum, mesmo mais nova, leu o mapa de processos em cache");
    let (_, body) = call(addr, Some(SECRET), "list.discover", json!({"newer_than": now()})).await;
    assert_eq!(names(&body), ["alpha"]);
    assert_eq!(calls(dir.path(), "list-panes"), 3, "pedido de sessão recém-criada varre de novo");
    call(addr, Some(SECRET), "list.invalidate", json!({})).await;
    call(addr, Some(SECRET), "list.discover", json!({})).await;
    assert_eq!(calls(dir.path(), "list-panes"), 4, "invalidada não serve a de antes");
}

#[tokio::test]
async fn snapshot_is_single_flight() {
    let dir = tempfile::tempdir().unwrap();
    let (_python, upstream) = fake::spawn_fake().await;
    let addr = spawn_with(dir.path(), 0, upstream).await;
    let (a, b) = tokio::join!(call(addr, Some(SECRET), "list.snapshot", json!({})),
                              call(addr, Some(SECRET), "list.snapshot", json!({})));
    assert_eq!((names(&a.1), names(&b.1)), (vec!["alpha"], vec!["alpha"]));
    assert_eq!((calls(dir.path(), "list-panes"), calls(dir.path(), "capture-pane")), (1, 1),
        "duas perguntas sem retrato produzem uma vez");
}

/// Fatos que nunca responderam não viram lista servida: acesso e escondidas seriam vazios, e o
/// convidado veria sessões que não são dele.
#[tokio::test]
async fn snapshot_without_facts_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let addr = spawn(dir.path(), 0).await;
    let (status, body) = call(addr, Some(SECRET), "list.snapshot", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!((&body["ok"], &body["error"]["code"]), (&json!(false), &json!("list_facts_unknown")));
    // Não fica no retrato: a próxima pergunta produz de novo (e captura de novo).
    call(addr, Some(SECRET), "list.snapshot", json!({})).await;
    assert_eq!(calls(dir.path(), "capture-pane"), 2);
}

/// Com o Python respondendo, o retrato sai com os fatos aplicados.
#[tokio::test(flavor = "multi_thread")]
async fn snapshot_with_facts_serves_rows() {
    let dir = tempfile::tempdir().unwrap();
    let (bridge, _python) = bridge_with_sessions(dir.path(), 1, ("idle", -60.0)).await;
    let mut state = AppState::new(config("127.0.0.1:9".parse().unwrap(), ""));
    state.list = Arc::new(bridge);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = terminal_router(Arc::new(state)).into_make_service_with_connect_info::<SocketAddr>();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let (status, body) = call(addr, Some(SECRET), "list.snapshot", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(names(&body), ["s0"]);
}

#[tokio::test]
async fn mux_unavailable_is_an_error_never_empty() {
    let dir = tempfile::tempdir().unwrap();
    let addr = spawn(dir.path(), 2).await;
    let (status, body) = call(addr, Some(SECRET), "list.discover", json!({})).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"ok": false, "error": {"code": "mux_unavailable", "detail": "mux_refused"}}));
}

#[tokio::test]
async fn seed_then_resolve_then_forget() {
    let dir = tempfile::tempdir().unwrap();
    let addr = spawn(dir.path(), 0).await;
    let cwd = dir.path().to_str().unwrap();
    call(addr, Some(SECRET), "list.seed", json!({"name": "alpha", "jsonl": "/x/semeado.jsonl"})).await;
    let (_, body) = call(addr, Some(SECRET), "list.resolve", json!({"name": "alpha", "cwd": cwd})).await;
    assert_eq!(body["result"], json!({"jsonl": "/x/semeado.jsonl", "tracked": true}));
    call(addr, Some(SECRET), "list.rename", json!({"old": "alpha", "new": "beta"})).await;
    let (_, body) = call(addr, Some(SECRET), "list.resolve", json!({"name": "beta", "cwd": cwd})).await;
    assert_eq!(body["result"]["jsonl"], "/x/semeado.jsonl", "o cache segue o nome novo");
    call(addr, Some(SECRET), "list.forget", json!({"name": "beta"})).await;
    let (_, body) = call(addr, Some(SECRET), "list.resolve", json!({"name": "beta", "cwd": cwd})).await;
    assert_eq!(body["result"], json!({"jsonl": null, "tracked": false}));
}

/// Ponte com `n` sessões Claude paradas num `tmux` de mentira (pane pronto) e fatos de um Python de
/// mentira; `marker` é o estado e a idade (s) do marcador de cada uma.
async fn bridge_with_sessions(root: &Path, n: usize, marker: (&str, f64)) -> (ListBridge, Arc<fake::Fake>) {
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
    std::fs::write(&script, format!("#!/bin/sh\n[ \"$1\" = list-panes ] || {{ printf '● pronto\\n❯\\n'; exit 0; }}\nprintf '{panes}'\n")).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let (python, upstream) = fake::spawn_fake().await;
    let bridge = ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.clone().into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new(upstream, SECRET.into()));
    (bridge, python)
}

/// A sombra produz sem entregar nada: o pane pronto contradiz o `awaiting` velho, e só a produção de
/// verdade pede ao Python para rebaixá-lo.
#[tokio::test(flavor = "multi_thread")]
async fn shadow_never_demotes_and_tells_python() {
    let dir = tempfile::tempdir().unwrap();
    let (bridge, python) = bridge_with_sessions(dir.path(), 1, ("awaiting_input", 60.0)).await;
    let shadow = hangar_server::list::bridge::ProduceFacts { shadow: true, ..Default::default() };
    let p = bridge.produce(&shadow).await.unwrap();
    assert_eq!(p.rows.len(), 1);
    assert!(p.facts_ok);
    assert_eq!(python.list_facts_last.lock().unwrap()["shadow"], true);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(python.hits_to("/internal/list/demote"), 0);
    let sid = "00000000-0000-0000-0000-000000000000";
    let marker: Value = serde_json::from_slice(&std::fs::read(dir.path().join(format!("home/.claude/.hangar-state/{sid}.json"))).unwrap()).unwrap();
    let ts = marker["ts"].as_f64().unwrap();
    assert!(!bridge.demoted.applies(sid, ts), "a sombra não rebaixa nem aqui");
    bridge.invalidate();
    bridge.produce(&Default::default()).await.unwrap();
    assert!(bridge.demoted.applies(sid, ts), "o rebaixamento vale já no Rust, para a lista e o `Monitor`");
    assert_eq!(python.list_facts_last.lock().unwrap()["shadow"], false);
    tokio::time::timeout(Duration::from_secs(5), async {
        while python.hits_to("/internal/list/demote") == 0 { tokio::time::sleep(Duration::from_millis(20)).await; }
    }).await.expect("sem a sombra, o rebaixamento sai");
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
    // Os fatos vêm de um Python de mentira, para o tique pagar a ida e volta real.
    let (bridge, python) = bridge_with_sessions(dir.path(), 20, ("idle", -60.0)).await;
    let first = bridge.produce(&Default::default()).await.unwrap();
    assert!(first.rows.iter().all(|r| r.problema.is_none()), "fatos do Python de mentira responderam");
    assert_eq!(first.rows.len(), 20);
    let rss0 = peak_rss_kb();
    let cpu = |s: &str| -> f64 { let f: Vec<&str> = s.rsplit(')').next().unwrap().split_whitespace().collect();
        (11..=14).map(|i| f[i].parse::<f64>().unwrap()).sum::<f64>() / 100.0 };
    let cpu0 = cpu(&std::fs::read_to_string("/proc/self/stat").unwrap());
    let t0 = std::time::Instant::now();
    let n = 300;
    for i in 0..n {
        bridge.invalidate();
        // Entrada nova a cada tique: paga a pergunta ao Python, como o hub (tique de 1,5 s > prazo de 1 s).
        let input = hangar_server::list::bridge::ProduceFacts { owner_clients: i % 2, ..Default::default() };
        let p = bridge.produce(&input).await.unwrap();
        assert_eq!(p.rows.len(), 20);
    }
    let wall = t0.elapsed().as_secs_f64() / n as f64;
    let cpu_tick = (cpu(&std::fs::read_to_string("/proc/self/stat").unwrap()) - cpu0) / n as f64;
    println!("tique (20 sessões): parede {:.2} ms, CPU (com filhos) {:.2} ms, pico RSS {} → {} kB",
        wall * 1e3, cpu_tick * 1e3, rss0, peak_rss_kb());
    // Tique da sombra: a mesma produção, sem rebaixar, mais a comparação com a assinatura do Python.
    let sigs: serde_json::Map<String, Value> = first.rows.iter().map(|r| {
        let raw = serde_json::to_value(r).unwrap();
        let sig: serde_json::Map<String, Value> = SIG_FIELDS.iter().map(|f| ((*f).to_owned(), raw.get(*f).cloned().unwrap_or(Value::Null))).collect();
        (r.name.clone(), Value::Object(sig))
    }).collect();
    let mut reply = python.list_facts.lock().unwrap().0.clone();
    reply["shadow"] = Value::Object(sigs);
    python.list_facts.lock().unwrap().0 = reply;
    let mut reporter = hangar_server::list::shadow::Reporter::default();
    let (cpu0, t0) = (cpu(&std::fs::read_to_string("/proc/self/stat").unwrap()), std::time::Instant::now());
    let mut cmp = Duration::ZERO;
    for i in 0..n {
        bridge.invalidate();
        let input = hangar_server::list::bridge::ProduceFacts { owner_clients: i % 2, shadow: true, ..Default::default() };
        let p = bridge.produce(&input).await.unwrap();
        let py = p.facts.shadow.as_ref().expect("assinatura do Python");
        let c = std::time::Instant::now();
        reporter.record(hangar_server::list::shadow::compare(&p.rows, py), std::time::Instant::now());
        cmp += c.elapsed();
    }
    println!("tique da sombra (20 sessões): parede {:.2} ms, CPU (com filhos) {:.2} ms, comparação {:.0} µs, pico RSS {} kB",
        t0.elapsed().as_secs_f64() / n as f64 * 1e3,
        (cpu(&std::fs::read_to_string("/proc/self/stat").unwrap()) - cpu0) / n as f64 * 1e3,
        cmp.as_secs_f64() / n as f64 * 1e6, peak_rss_kb());
}

/// `list_facts.SIG_FIELDS` do Python.
const SIG_FIELDS: [&str; 44] = ["name", "cwd", "branch", "git_cwd", "worktree_gone", "git_dirty", "state", "tracked", "headless",
    "jsonl", "question", "stalled", "limited", "lifecycle_id", "transfer_id", "transfer_phase", "last_reply", "last_reply_at",
    "pending_questions", "limit_reset", "then_target", "status_line", "context", "model", "label", "startup_steps",
    "loop_status", "loop_iter", "engine", "conta", "codex_service_tier", "plan_name", "plan_done", "plan_total", "plan_task",
    "plan_task_total", "plan_complete", "plan_tasks", "plan_hidden", "problema", "provider", "shared", "owner", "orq_arbiter"];
