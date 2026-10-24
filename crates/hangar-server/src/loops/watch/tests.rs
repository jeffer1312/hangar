use super::*;
use crate::loops::store::new_loop;

#[derive(Default)]
struct Fake {
    claude: std::path::PathBuf,
    rows: Mutex<Option<Vec<SessionRow>>>,
    sent: bool,
    automations: bool,
    deliver_fails: bool,
    ticks: Arc<Mutex<Vec<String>>>,
    delivered: Mutex<Vec<(String, String)>>,
    pushed: Mutex<Vec<(String, String, String)>>,
    diags: Mutex<Vec<String>>,
}

impl Host for Fake {
    fn store(&self) -> Store { Store::new(&self.claude) }
    fn row_by_sid<'a>(&'a self, sid: &'a str) -> BoxFuture<'a, Option<SessionRow>> {
        Box::pin(async move { lock(&self.rows).as_ref()?.iter().find(|r| key_of(r).as_deref() == Some(sid)).cloned() })
    }
    fn rows(&self) -> BoxFuture<'_, Option<Vec<SessionRow>>> { Box::pin(async move { lock(&self.rows).clone() }) }
    fn just_sent<'a>(&'a self, _name: &'a str, _since: f64) -> BoxFuture<'a, bool> { Box::pin(async move { self.sent }) }
    fn tick(&self, row: SessionRow) -> BoxFuture<'static, ()> {
        let ticks = self.ticks.clone();
        Box::pin(async move { lock(&ticks).push(row.name); })
    }
    fn automations(&self) -> bool { self.automations }
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
        "jsonl": format!("/p/{name}-0000.jsonl"), "cwd": "/w"})).unwrap()
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
