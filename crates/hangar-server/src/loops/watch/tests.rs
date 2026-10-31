use super::*;
use crate::loops::store::new_loop;

#[derive(Default)]
struct Fake {
    claude: std::path::PathBuf,
    gates: crate::runtime::ingress::IngressGates,
    guard_error: Option<&'static str>,
    rows: Mutex<Option<Vec<SessionRow>>>,
    sent: bool,
    automations: bool,
    config: Option<Arc<AppState>>,
    deliver_fails: bool,
    ticks: Arc<Mutex<Vec<String>>>,
    delivered: Mutex<Vec<(String, String)>>,
    pushed: Mutex<Vec<(String, String, String)>>,
    diags: Mutex<Vec<String>>,
}

impl Host for Fake {
    fn store(&self) -> Store { Store::new(&self.claude) }
    fn guard<'a>(&'a self, expected: &'a SessionRow) -> BoxFuture<'a, Result<(crate::runtime::ingress::IngressPass, SessionRow), &'static str>> {
        Box::pin(async move {
            if let Some(code) = self.guard_error { return Err(code); }
            enter_tick(&self.gates, expected, async { Ok(lock(&self.rows).as_ref().and_then(|rows| rows.iter().find(|row| row.name == expected.name).cloned())) }).await
        })
    }
    fn row_by_sid<'a>(&'a self, sid: &'a str) -> BoxFuture<'a, Option<SessionRow>> {
        Box::pin(async move { lock(&self.rows).as_ref()?.iter().find(|r| key_of(r).as_deref() == Some(sid)).cloned() })
    }
    fn rows(&self) -> BoxFuture<'_, Option<Vec<SessionRow>>> { Box::pin(async move { lock(&self.rows).clone() }) }
    fn just_sent<'a>(&'a self, _name: &'a str, _since: f64) -> BoxFuture<'a, bool> { Box::pin(async move { self.sent }) }
    fn tick(&self, row: SessionRow) -> BoxFuture<'static, ()> {
        let ticks = self.ticks.clone();
        Box::pin(async move { lock(&ticks).push(row.name); })
    }
    fn automations(&self) -> Result<bool, &'static str> { self.config.as_ref().map_or(Ok(self.automations), |state| super::super::automations(state)) }
    fn deliver<'a>(&'a self, target: &'a str, text: &'a str) -> BoxFuture<'a, Result<(), String>> {
        Box::pin(async move {
            if self.deliver_fails { return Err("erro_sessao_inexistente".into()); }
            lock(&self.delivered).push((target.into(), text.into()));
            Ok(())
        })
    }
    fn notify(&self, name: &str, status: &str, reason: &str) { lock(&self.pushed).push((name.into(), status.into(), reason.into())); }
    fn now(&self) -> f64 { 1000.0 }
    fn diag(&self, event: &'static str, _key: &str, code: &str, _reason: &'static str) { lock(&self.diags).push(format!("{event}:{code}")); }
}

fn row(name: &str, provider: &str, headless: bool, state: &str) -> SessionRow {
    serde_json::from_value(json!({"name": name, "provider": provider, "headless": headless, "state": state,
        "jsonl": format!("/p/{name}-0000.jsonl"), "lifecycle_id": format!("k:{name}"), "cwd": "/w"})).unwrap()
}

fn host(dir: &tempfile::TempDir, rows: Vec<SessionRow>) -> Fake {
    Fake { claude: dir.path().to_owned(), rows: Mutex::new(Some(rows)), automations: true, ..Fake::default() }
}

fn idle(name: &str) -> TurnEvent { TurnEvent { sid: format!("{name}-0000"), state: "idle".into(), ts: 999.0 } }

async fn settle() { tokio::time::sleep(Duration::from_millis(120)).await; }

fn running(store: &Store, name: &str) {
    let mut d = new_loop("g", Some("false"), 10, true, 1.0);
    d.insert("goal_delivered_ts".into(), json!(1.0));
    store.set(name, &d).unwrap();
}

/// B7: a sessão Claude sem terminal no fim de turno também dá a volta do loop.
#[tokio::test]
async fn headless_claude_idle_ticks_the_loop() {
    let dir = tempfile::tempdir().unwrap();
    let w = Watcher::new(Arc::new(host(&dir, vec![row("h", "claude", true, "idle")])), Duration::from_millis(20));
    running(&w.host.store(), "h");
    w.on_event(idle("h"));
    settle().await;
    assert_eq!(*lock(&w.host.ticks), ["h"]);
}

#[tokio::test]
async fn idle_right_after_the_queue_typed_does_not_tick() {
    let dir = tempfile::tempdir().unwrap();
    let w = Watcher::new(Arc::new(Fake { sent: true, ..host(&dir, vec![row("s", "claude", false, "idle")]) }), Duration::from_millis(20));
    running(&w.host.store(), "s");
    w.on_event(idle("s"));
    settle().await;
    assert!(lock(&w.host.ticks).is_empty());
}

#[tokio::test]
async fn new_turn_inside_the_settle_cancels_the_tick() {
    let dir = tempfile::tempdir().unwrap();
    let w = Watcher::new(Arc::new(host(&dir, vec![row("s", "claude", false, "idle")])), Duration::from_millis(60));
    running(&w.host.store(), "s");
    w.on_event(idle("s"));
    w.on_event(TurnEvent { sid: "s-0000".into(), state: "working".into(), ts: 999.5 });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(lock(&w.host.ticks).is_empty());
}

#[tokio::test]
async fn question_pauses_a_running_loop() {
    let dir = tempfile::tempdir().unwrap();
    let w = Watcher::new(Arc::new(host(&dir, vec![row("s", "claude", false, "awaiting_input")])), Duration::from_millis(20));
    running(&w.host.store(), "s");
    w.on_event(TurnEvent { sid: "s-0000".into(), state: "awaiting_input".into(), ts: 999.0 });
    settle().await;
    assert_eq!(status(&w.host.store().get("s").unwrap()), "paused_awaiting");
}

/// Loop ativo segura o `then`; sem loop ele dispara uma vez e sai.
#[tokio::test]
async fn active_loop_holds_the_chain_and_idle_fires_it_once() {
    let dir = tempfile::tempdir().unwrap();
    let w = Watcher::new(Arc::new(host(&dir, vec![row("s", "claude", false, "idle"), row("t", "pi", false, "idle")])), Duration::from_millis(20));
    let store = w.host.store();
    store.set_chain("s", "t", "roda os testes").unwrap();
    running(&store, "s");
    w.on_event(idle("s"));
    settle().await;
    assert!(lock(&w.host.delivered).is_empty());
    assert!(store.chain("s").is_some());
    store.forget("s").unwrap();
    w.on_event(idle("s"));
    settle().await;
    assert_eq!(*lock(&w.host.delivered), [("t".to_owned(), "roda os testes".to_owned())]);
    assert!(store.chain("s").is_none());
}

/// B4: entrega que falha mantém o vínculo e vai ao diário; chave mestra desligada não consome.
#[tokio::test]
async fn failed_or_switched_off_chain_keeps_the_link() {
    let dir = tempfile::tempdir().unwrap();
    let failing = Fake { deliver_fails: true, ..host(&dir, vec![]) };
    failing.store().set_chain("s", "t", "x").unwrap();
    assert_eq!(fire_chain(&failing, "s").await, Chain::Failed("erro_sessao_inexistente".into()));
    assert!(failing.store().chain("s").is_some());
    assert_eq!(*lock(&failing.diags), ["loop.chain_failed:erro_sessao_inexistente"]);
    let off = Fake { automations: false, ..host(&dir, vec![]) };
    assert_eq!(fire_chain(&off, "s").await, Chain::Off);
    assert!(off.store().chain("s").is_some());
    std::fs::write(dir.path().join(".hangar-chain/s.json"), r#"{"target": "", "text": "x"}"#).unwrap();
    assert_eq!(fire_chain(&host(&dir, vec![]), "s").await, Chain::Dropped);
    assert!(off.store().chain("s").is_none());
}

/// O `then` de Kimi, Pi e omp ainda sai do Python: dois leitores do vínculo o mandariam duas vezes.
#[tokio::test]
async fn kimi_pi_and_omp_stay_with_the_python() {
    for provider in ["kimi", "pi", "omp"] {
        let dir = tempfile::tempdir().unwrap();
        let w = Watcher::new(Arc::new(host(&dir, vec![row("k", provider, false, "idle")])), Duration::from_millis(20));
        w.host.store().set_chain("k", "t", "x").unwrap();
        w.on_event(idle("k"));
        settle().await;
        assert!(lock(&w.host.delivered).is_empty(), "{provider}");
        assert!(w.host.store().chain("k").is_some(), "{provider}");
    }
}

#[tokio::test]
async fn one_session_holding_its_loop_lock_does_not_block_another() {
    let _a = crate::loops::store::lock("lock-a").await;
    let b = tokio::time::timeout(Duration::from_secs(2), crate::loops::store::lock("lock-b")).await;
    assert!(b.is_ok(), "a trava do loop é por sessão");
    drop(b);
    let again = tokio::time::timeout(Duration::from_millis(100), crate::loops::store::lock("lock-a")).await;
    assert!(again.is_err(), "a mesma sessão segue em fila");
}

#[tokio::test]
async fn boot_fails_dead_sessions_and_ticks_idle_ones() {
    let dir = tempfile::tempdir().unwrap();
    let w = Watcher::new(Arc::new(host(&dir, vec![row("vivo", "claude", false, "idle"), row("ocupado", "claude", false, "working")])), Duration::ZERO);
    let store = w.host.store();
    for name in ["vivo", "ocupado", "morto"] { running(&store, name); }
    let mut done = new_loop("g", None, 3, true, 1.0);
    done.insert("status".into(), json!("done"));
    store.set("antigo", &done).unwrap();
    w.boot(1, Duration::ZERO).await;
    settle().await;
    assert_eq!(*lock(&w.host.ticks), ["vivo"]);
    let dead = store.get("morto").unwrap();
    assert_eq!((status(&dead), dead["ended_reason"].as_str()), ("failed", Some("sessão morta no boot")));
    assert_eq!(*lock(&w.host.pushed), [("morto".to_owned(), "failed".to_owned(), "sessão morta no boot".to_owned())]);
    assert_eq!(status(&store.get("antigo").unwrap()), "done");
}

/// Lista sem os fatos não prova morte: nenhum loop é encerrado.
#[tokio::test]
async fn boot_without_the_list_kills_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let w = Watcher::new(Arc::new(Fake { rows: Mutex::new(None), ..host(&dir, vec![]) }), Duration::ZERO);
    running(&w.host.store(), "s");
    w.boot(2, Duration::ZERO).await;
    assert_eq!(status(&w.host.store().get("s").unwrap()), "running");
    assert_eq!(*lock(&w.host.diags), ["loop.boot_resume_falhou:list_unavailable"]);
}

#[tokio::test]
async fn guest_loop_invalid_config_preserves_chain_and_reports() {
    let home = tempfile::tempdir().unwrap();
    let source = Fake {config:Some(crate::loops::tests::invalid_config_state(home.path())),..host(&home,vec![])};
    source.store().set_chain("s","target","text").unwrap();
    let before = source.store().chain("s");
    assert_eq!(fire_chain(&source,"s").await,Chain::Failed("loop_config_invalid".into()));
    assert_eq!(source.store().chain("s"),before);
    assert!(lock(&source.delivered).is_empty());
    assert_eq!(*lock(&source.diags),["loop.chain_failed:loop_config_invalid"]);
}

/// D59: sidecar herdado de outra vida não autoriza comandos no provedor novo.
#[tokio::test]
async fn guest_loop_unsupported_provider_preserves_sidecar_at_every_entry() {
    for provider in ["pi", "omp", "kimi", "codex"] {
        for trigger in ["pause", "idle", "boot"] {
            let dir = tempfile::tempdir().unwrap();
            let w = Watcher::new(Arc::new(host(&dir, vec![row("s", provider, false, "idle")])), Duration::ZERO);
            running(&w.host.store(), "s");
            let path = w.host.store().loop_dir().join("s.json");
            let before = std::fs::read(&path).unwrap();
            match trigger {
                "pause" => w.pause("s-0000").await,
                "idle" => w.idle(idle("s")).await,
                _ => w.boot(1, Duration::ZERO).await,
            }
            tokio::task::yield_now().await;
            assert_eq!(std::fs::read(&path).unwrap(), before, "D59 {provider}/{trigger}");
            assert!(lock(&w.host.ticks).is_empty(), "D59 {provider}/{trigger}");
            assert!(lock(&w.host.delivered).is_empty());
            assert!(lock(&w.host.pushed).is_empty());
            assert_eq!(*lock(&w.host.diags), ["loop.provider_refused:loop_provider_unsupported"], "D59 {provider}/{trigger}");
        }
    }
}

fn selected_life() -> SessionRow {
    let mut selected = row("s", "claude", false, "idle");
    selected.lifecycle_id = Some("k:original".into());
    selected
}

#[tokio::test]
async fn guest_loop_life_guard_rejects_changed_snapshots_and_releases_ingress() {
    use crate::runtime::ingress::IngressGates;
    let expected = selected_life();
    let mut variants = Vec::new();
    for provider in ["pi", "omp", "kimi", "codex"] {
        let mut changed = expected.clone(); changed.provider = provider.into();
        variants.push((Some(changed), "loop_provider_unsupported"));
    }
    let mut changed = expected.clone(); changed.lifecycle_id = Some("k:nova".into());
    variants.push((Some(changed), "loop_life_changed"));
    let mut changed = expected.clone(); changed.jsonl = Some("outra.jsonl".into());
    variants.push((Some(changed), "loop_life_changed"));
    let mut changed = expected.clone(); changed.lifecycle_id = None;
    variants.push((Some(changed), "loop_life_unavailable"));
    variants.push((None, "loop_life_changed"));
    for (current, code) in variants {
        let gates = IngressGates::default();
        assert_eq!(enter_tick(&gates, &expected, async { Ok(current) }).await.err(), Some(code));
        gates.hold("s", Duration::from_secs(1)).await.unwrap();
        gates.release("s");
    }
    let gates = IngressGates::default();
    assert_eq!(enter_tick(&gates, &expected, async { Err("loop_list_unavailable") }).await.err(), Some("loop_list_unavailable"));
    gates.hold("s", Duration::from_secs(1)).await.unwrap();
    let read = std::sync::atomic::AtomicBool::new(false);
    let result = enter_tick(&gates, &expected, async { read.store(true, std::sync::atomic::Ordering::SeqCst); Ok(Some(expected.clone())) }).await;
    assert_eq!(result.err(), Some("session_transfer_busy"));
    assert!(!read.load(std::sync::atomic::Ordering::SeqCst), "porta retida recusa antes da releitura");
    gates.release("s");
}

#[tokio::test]
async fn guest_loop_life_guard_checks_the_snapshot_after_acquiring_ingress_and_cancels_cleanly() {
    use crate::runtime::ingress::IngressGates;
    for cancel in [false, true] {
        let gates = IngressGates::default();
        let current = Arc::new(Mutex::new(selected_life()));
        let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
        let (resume_tx, resume_rx) = tokio::sync::oneshot::channel();
        let (task_gates, task_current) = (gates.clone(), current.clone());
        let task = tokio::spawn(async move {
            enter_tick(&task_gates, &selected_life(), async {
                entered_tx.send(()).unwrap();
                let _ = resume_rx.await;
                Ok(Some(lock(&task_current).clone()))
            }).await.err()
        });
        entered_rx.await.unwrap();
        assert!(gates.hold("s", Duration::from_millis(20)).await.is_err(), "releitura já segura a vida");
        lock(&current).lifecycle_id = Some("k:substituida".into());
        if cancel {
            task.abort();
            assert!(task.await.unwrap_err().is_cancelled());
        } else {
            resume_tx.send(()).unwrap();
            assert_eq!(task.await.unwrap(), Some("loop_life_changed"));
        }
        gates.hold("s", Duration::from_secs(1)).await.unwrap();
        gates.release("s");
    }
}

#[tokio::test]
async fn guest_loop_runtime_uses_the_existing_pass_while_close_waits() {
    use crate::runtime::{gateway::RuntimeRegistry, ingress::IngressGates};
    let runtime = Arc::new(RuntimeRegistry::new("127.0.0.1:1".parse().unwrap(), "fixture".into(), "fixture".into()));
    let gates = runtime.ingress().clone();
    let expected = selected_life();
    let mut fresh = expected.clone(); fresh.cwd = Some("pasta-atual".into());
    let (pass, current) = enter_tick(&gates, &expected, async { Ok(Some(fresh)) }).await.unwrap();
    assert_eq!(current.cwd.as_deref(), Some("pasta-atual"));
    let wrong = IngressGates::default().enter("s", Duration::ZERO).await.unwrap();
    assert_eq!(runtime.adopt_claude_terminal_entered("s", &wrong).await.unwrap_err().code, "runtime_ingress_mismatch");
    assert_eq!(runtime.adopt_claude_terminal_entered("outra", &pass).await.unwrap_err().code, "runtime_ingress_mismatch");
    let close_gates = gates.clone();
    let closing = tokio::spawn(async move { close_gates.close("s", Duration::from_secs(2)).await });
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if gates.enter("s", Duration::ZERO).await.is_err() { break; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert!(!closing.is_finished());
    tokio::time::timeout(Duration::from_secs(1), runtime.adopt_claude_terminal_entered("s", &pass)).await.unwrap().unwrap();
    let error = tokio::time::timeout(Duration::from_secs(1), runtime.drain_entered("s", &pass)).await.unwrap().unwrap_err();
    assert_eq!(error.code, "runtime_binding", "sem ator é erro de vínculo, não reentrada na porta fechando");
    drop(pass);
    closing.await.unwrap().unwrap();
    gates.open("s");
}

#[tokio::test]
async fn guest_loop_pause_preserves_sidecar_when_the_life_guard_refuses() {
    let dir = tempfile::tempdir().unwrap();
    let host = Fake { guard_error: Some("loop_life_changed"), ..host(&dir, vec![row("s", "claude", false, "awaiting_input")]) };
    let w = Watcher::new(Arc::new(host), Duration::ZERO);
    running(&w.host.store(), "s");
    let path = w.host.store().loop_dir().join("s.json");
    let before = std::fs::read(&path).unwrap();
    w.pause("s-0000").await;
    assert_eq!(std::fs::read(path).unwrap(), before, "D59: pergunta antiga não pausa outra vida");
    assert_eq!(*lock(&w.host.diags), ["loop.pause_refused:loop_life_changed"]);
}

#[tokio::test]
async fn guest_loop_life_pass_is_released_when_the_cycle_is_cancelled() {
    let gates = crate::runtime::ingress::IngressGates::default();
    let task_gates = gates.clone();
    let (entered_tx, entered_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let expected = selected_life();
        let (pass, _) = enter_tick(&task_gates, &expected, async { Ok(Some(expected.clone())) }).await.unwrap();
        entered_tx.send(()).unwrap();
        std::future::pending::<()>().await;
        drop(pass);
    });
    entered_rx.await.unwrap();
    assert!(gates.hold("s", Duration::from_millis(20)).await.is_err());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    gates.hold("s", Duration::from_secs(1)).await.unwrap();
    gates.release("s");
}
