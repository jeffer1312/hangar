//! Contratos públicos de abertura, adoção e fim de vida dos três agentes com terminal.
use futures_util::future::BoxFuture;
use hangar_server::{
    runtime::{
        gateway::RuntimeRegistry,
        other_terminal::*,
        protocol::{OperationKind, RuntimeCommand, RuntimeError},
    },
    terminal_input::TerminalBinding,
};
use serde_json::{Value, json};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
const KEY: &str = "0123456789abcdef0123456789abcdef";
fn dirs(root: &Path) -> hangar_server::list::discover_other::Dirs {
    hangar_server::list::discover_other::Dirs {
        home: root.into(),
        claude: root.join(".claude"),
        codex_home: root.join(".codex"),
        pi_sessions: root.join("pi"),
        omp_config: root.join("omp"),
        omp_agent: root.join("omp/agent"),
        kimi_home: root.join("kimi"),
    }
}
fn spec(root: &Path) -> hangar_server::session_open::SessionSpec {
    serde_json::from_value(json!({"version":1,"key":KEY,"name":"pi-a","cwd":root,"provider":"pi","surface":"terminal","account":{"kind":"default"},"account_source":"default","jev":false,"read_only":false,"created_at":100.0})).unwrap()
}
fn binding() -> TerminalBinding {
    TerminalBinding {
        name: "pi-a".into(),
        pane: "%1".into(),
        conversation: "sid-a".into(),
        generation: 1,
        created: 100,
        mux_argv: vec!["absent-mux".into()],
        windows: false,
        clipboard_lock_path: None,
    }
}
fn opening(root: &Path) -> OtherOpening {
    let mut binding = binding();
    binding.conversation.clear();
    OtherOpening {
        spec: spec(root),
        binding,
        pane_address: "%1".into(),
        projection_dir: root.join(".claude/.hangar-queue"),
        dirs: dirs(root),
        config_dirs: vec![root.join(".claude")],
        pane_pid: Some(123),
        agent: None,
    }
}
fn target(root: &Path) -> OtherTarget {
    let projection = root.join(".claude/.hangar-queue");
    let (generation, state_path, lease_path) = queue_identity(&projection, KEY).unwrap();
    let mut binding = binding();
    binding.generation = generation;
    OtherTarget {
        key: KEY.into(),
        generation,
        name: "pi-a".into(),
        provider: hangar_transcript::Provider::Pi,
        binding,
        pane_address: "%1".into(),
        dirs: dirs(root),
        cwd: root.to_string_lossy().into_owned(),
        pane_pid: 123,
        agent_pid: 124,
        agent_birth: 100.5,
        transcript: root.join("sid-a.jsonl"),
        state_path,
        lease_path,
        projection_dir: projection,
        config_dirs: vec![root.join(".claude")],
        created: 100.0,
    }
}
#[derive(Default)]
struct Io {
    alive: AtomicBool,
    writes: Mutex<Vec<String>>,
    captures: AtomicUsize,
    times: Mutex<Vec<tokio::time::Instant>>,
    captured: tokio::sync::Notify,
    pane: Mutex<Option<String>>,
    first_input: Mutex<Option<(OtherTarget, Arc<Mutex<Option<OtherDiscoveryState>>>)>>,
}
impl Io {
    fn live() -> Arc<Self> {
        Arc::new(Self {
            alive: AtomicBool::new(true),
            ..Default::default()
        })
    }
}
impl OtherIo for Io {
    fn prove<'a>(&'a self, _: &'a OtherTarget) -> BoxFuture<'a, Result<bool, RuntimeError>> {
        Box::pin(async move { Ok(self.alive.load(Ordering::SeqCst)) })
    }
    fn capture<'a>(&'a self) -> BoxFuture<'a, Result<OtherFrame, RuntimeError>> {
        Box::pin(async move {
            self.captures.fetch_add(1, Ordering::SeqCst);
            self.times.lock().unwrap().push(tokio::time::Instant::now());
            self.captured.notify_one();
            Ok(OtherFrame::from_styled(
                self.pane.lock().unwrap().clone().unwrap_or_else(||
                    "────────────────────────────────\n\n────────────────────────────────\n".into()),
            ))
        })
    }
    fn write<'a>(
        &'a self,
        _: &'a OtherTarget,
        text: &'a str,
        _: bool,
        _: Option<&'a OtherWriteGuard>,
    ) -> BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(async move {
            self.writes.lock().unwrap().push(text.into());
            if let Some((target, source)) = self.first_input.lock().unwrap().as_ref() {
                if text == "Enter" {
                    let event = if target.provider == hangar_transcript::Provider::Kimi {
                        json!({"type":"context.append_message","message":{"role":"user","content":[{"type":"text","text":"NATIVE_PROOF_1"}],"toolCalls":[],"origin":{"kind":"user"},"id":"first-user"},"time":chrono::Utc::now().timestamp_millis()})
                    } else {
                        json!({"type":"message","id":"first-user","timestamp":chrono::Utc::now().to_rfc3339(),"message":{"role":"user","timestamp":chrono::Utc::now().timestamp_millis(),"content":[{"type":"text","text":"NATIVE_PROOF_1"}]}})
                    };
                    std::fs::write(&target.transcript, format!("{event}\n")).unwrap();
                    *source.lock().unwrap() = Some(OtherDiscoveryState::Ready(target.clone()));
                    *self.pane.lock().unwrap() = None;
                } else {
                    *self.pane.lock().unwrap() = Some(format!("────────────────────────────────\n{text}\n────────────────────────────────\n"));
                }
            }
            Ok(())
        })
    }
    fn release<'a>(&'a self) -> BoxFuture<'a, ()> {
        Box::pin(async {})
    }
}

#[tokio::test]
async fn other_lifecycle_kimi_approval_requires_both_wire_and_visible_panel() {
    let panel = "  ▶ Ready to build with this plan?\n\n  ▶ 1. Approve\n    2. Reject\n    3. Revise\n\n  ↑/↓ select · 1/2/3 choose · ↵ confirm\n";
    for (tool, display, visible, resolved, missing, expected_state, question, choices) in [
        ("ExitPlanMode", json!({"kind":"plan_review","plan":"# plano"}), true, false, false,
            "awaiting_input", Some("Ready to build with this plan?"), json!(["Approve","Reject","Revise"])),
        ("Bash", json!({"kind":"command","command":"ls"}), true, false, false,
            "awaiting_input", Some("Run this command?\n`ls`"), json!(["Approve once","Approve for this session","Reject","Reject with feedback"])),
        ("ExitPlanMode", json!({"kind":"plan_review","plan":"# plano"}), false, false, false,
            "working", None, Value::Null),
        ("ExitPlanMode", json!({"kind":"plan_review","plan":"# plano"}), true, true, false,
            "working", None, Value::Null),
        ("ExitPlanMode", json!({"kind":"plan_review","plan":"# plano"}), true, false, true,
            "idle", None, Value::Null),
    ] {
        let root = tempfile::tempdir().unwrap();
        let mut target = target(root.path());
        target.provider = hangar_transcript::Provider::Kimi;
        let mut lines = vec![json!({"type":"turn.prompt","time":1_786_454_000_000u64}),
            json!({"type":"interaction.request","agentId":"main","id":"approval_1","kind":"approval",
                "toolCallId":"tool_x","request":{"turnId":8,"toolCallId":"tool_x","toolName":tool,
                    "action":"","display":display},"time":1_786_454_000_000u64})];
        if resolved { lines.push(json!({"type":"interaction.resolved","id":"approval_1",
            "response":{"decision":"approved","selectedLabel":"Approve"},"time":1_786_454_010_000u64})); }
        if !missing {
            std::fs::write(&target.transcript, lines.into_iter().map(|line|line.to_string()+"\n").collect::<String>()).unwrap();
        }
        let io = Io::live();
        *io.pane.lock().unwrap() = Some(if visible { panel.into() } else { "composer vazio, sem painel\n".into() });
        let interactions = Arc::new(hangar_server::interactions::InteractionStore::default());
        let mut options = OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(),
            hangar_server::provider_models::inbox::Store::new(),
            Arc::new(hangar_server::list::procs::SystemProcs::default()), interactions.clone());
        options.io_override = Some(io.clone());
        let (live, mut seen) = tokio::sync::watch::channel(None);
        let handle = OtherHandle::spawn(target.clone(), open_queue(&target).await.unwrap(), options, live);
        let published = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Some(value) = seen.borrow().clone() { break value; }
                seen.changed().await.unwrap();
            }
        }).await.unwrap();
        let state = &published.public_state;
        assert_eq!(state["state"], expected_state, "{tool}, visible={visible}, resolved={resolved}");
        assert_eq!(state["question"], question.map(Value::from).unwrap_or(Value::Null));
        assert_eq!(state["options"], choices);
        if question.is_some() {
            let id = state["interaction_id"].as_str().unwrap();
            assert!(interactions.expected(&hangar_server::interactions::InteractionBinding {
                key:target.key.clone(),generation:target.generation,conversation:target.binding.conversation.clone()},Some(id)).is_ok());
        }
        assert!(io.writes.lock().unwrap().is_empty());
        handle.stop().await.unwrap();
    }
}
fn registry(
    root: &Path,
    io: Arc<Io>,
    source: Arc<Mutex<Option<OtherDiscoveryState>>>,
    after: Option<OtherAfterClosed>,
) -> (
    Arc<RuntimeRegistry>,
    Arc<hangar_server::provider_models::inbox::Store>,
) {
    registry_with_interactions(root, io, source, after, Arc::new(hangar_server::interactions::InteractionStore::default()))
}
fn registry_with_interactions(
    root: &Path,
    io: Arc<Io>,
    source: Arc<Mutex<Option<OtherDiscoveryState>>>,
    after: Option<OtherAfterClosed>,
    interactions: Arc<hangar_server::interactions::InteractionStore>,
) -> (Arc<RuntimeRegistry>, Arc<hangar_server::provider_models::inbox::Store>) {
    let inbox = hangar_server::provider_models::inbox::Store::new();
    let mut options = OtherOptions::new(
        hangar_server::terminal_control::TerminalPool::new(),
        inbox.clone(),
        Arc::new(hangar_server::list::procs::SystemProcs::default()),
        interactions.clone(),
    );
    options.io_override = Some(io);
    options.after_closed = after;
    options.tick = Duration::from_millis(500);
    let env = hangar_server::accounts::environment::AccountEnvironment::from_map(
        std::collections::BTreeMap::from([
            ("HOME".into(), root.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), root.to_string_lossy().into_owned()),
        ]),
    );
    let registry = Arc::new(
        RuntimeRegistry::new(
            "127.0.0.1:9".parse().unwrap(),
            "test-only".into(),
            "lifecycle-test".into(),
        )
        .with_interactions(interactions)
        .with_codex_environment(env),
    );
    registry
        .bind_other(
            options,
            OtherDiscovery {
                find: Arc::new(move |_| {
                    let current = source.lock().unwrap().clone();
                    Box::pin(async move { current.map(Ok) })
                }),
                names: Arc::new(|| Box::pin(async { Ok(vec![]) })),
            },
        )
        .unwrap();
    (registry, inbox)
}
#[tokio::test]
async fn other_lifecycle_opening_retains_one_id_until_ready_and_delivers_without_frontend() {
    let root = tempfile::tempdir().unwrap();
    let opening = opening(root.path());
    let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Opening(
        opening.clone(),
    ))));
    let (registry, inbox) = registry(root.path(), Io::live(), source.clone(), None);
    let adopted = registry.adopt_now("pi-a").await;
    let queued = registry
        .queue_other_opening("pi-a", KEY, "continuar a tarefa", "first-request")
        .await;
    let ready = target(root.path());
    std::fs::write(&ready.transcript, b"").unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let line = inbox.register("pi-a", tx);
    *source.lock().unwrap() = Some(OtherDiscoveryState::Ready(ready));
    let delivered = tokio::time::timeout(Duration::from_secs(10), async {
        let frame = rx.recv().await.unwrap();
        inbox.respond("pi-a", &json!({"id":frame["id"],"ok":true}));
        frame
    })
    .await
    .ok();
    let info = registry.other_info("pi-a").await;
    let snapshots = registry.list_snapshots().await;
    registry.shutdown().await.unwrap();
    inbox.remove("pi-a", &line);
    assert!(adopted.is_some());
    assert!(
        queued.is_ok(),
        "Opening sem CLI precisa persistir sem PID fictício"
    );
    assert_eq!(
        delivered.as_ref().map(|frame| frame["id"].clone()),
        Some(json!("first-request"))
    );
    assert_eq!(info.map(|info| info.session_key), Some("sid-a".into()));
    assert_eq!(snapshots[KEY]["queue"][0]["id"], "first-request");
}
#[tokio::test]
async fn other_lifecycle_opening_that_dies_before_the_first_prompt_leaves_the_registry() {
    let root = tempfile::tempdir().unwrap();
    let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Opening(opening(root.path())))));
    let (registry, _inbox) = registry(root.path(), Io::live(), source.clone(), None);
    assert!(registry.adopt_now("pi-a").await.is_some());
    assert!(registry.other_info("pi-a").await.is_some());
    *source.lock().unwrap() = None;
    let gone = tokio::time::timeout(Duration::from_secs(5), async {
        while registry.other_info("pi-a").await.is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    registry.shutdown().await.unwrap();
    assert!(gone.is_ok(), "abertura morta continua no registro");
}
#[tokio::test]
async fn other_lifecycle_reconcile_changes_conversation_without_relaunch_and_keeps_generation_on_restart()
 {
    let root = tempfile::tempdir().unwrap();
    let old = target(root.path());
    std::fs::write(&old.transcript, b"").unwrap();
    let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Ready(old.clone()))));
    let io = Io::live();
    let (registry, _) = registry(root.path(), io.clone(), source.clone(), None);
    registry.open_other_terminal(old.clone()).await.unwrap();
    let mut next = old.clone();
    next.binding.conversation = "sid-b".into();
    next.transcript = root.path().join("sid-b.jsonl");
    std::fs::write(&next.transcript, b"").unwrap();
    *source.lock().unwrap() = Some(OtherDiscoveryState::Ready(next));
    let changed = registry.reconcile_other("pi-a").await;
    let first = registry.writable("pi-a").await;
    let info = registry.other_info("pi-a").await;
    registry.shutdown().await.unwrap();
    assert!(changed.is_some_and(|result| result.is_ok()));
    assert_eq!(info.map(|info| info.session_key), Some("sid-b".into()));
    assert_eq!(first.map(|target| target.generation), Some(2));
    assert!(
        io.writes.lock().unwrap().is_empty(),
        "reconciliar não digita nem relança CLI"
    );
    let next = target(root.path());
    assert_eq!(
        next.generation, 2,
        "a geração durável sobrevive ao processo Rust"
    );
}
#[tokio::test]
async fn other_lifecycle_shutdown_does_not_kill_or_call_external_cleanup() {
    let root = tempfile::tempdir().unwrap();
    let target = target(root.path());
    std::fs::write(&target.transcript, b"").unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let copy = count.clone();
    let after: OtherAfterClosed = Arc::new(move |_| {
        copy.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(vec![]) })
    });
    let io = Io::live();
    let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Ready(target.clone()))));
    let (registry, _) = registry(root.path(), io.clone(), source, Some(after));
    registry.open_other_terminal(target).await.unwrap();
    registry.shutdown().await.unwrap();
    assert!(io.writes.lock().unwrap().is_empty());
    assert_eq!(count.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn other_lifecycle_native_receipt_is_confirmed_by_exact_new_occurrence() {
    use std::io::Write;
    let root = tempfile::tempdir().unwrap();
    let target = target(root.path());
    std::fs::write(&target.transcript, b"").unwrap();
    let inbox = hangar_server::provider_models::inbox::Store::new();
    let io = Io::live();
    let mut options = OtherOptions::new(
        hangar_server::terminal_control::TerminalPool::new(),
        inbox.clone(),
        Arc::new(hangar_server::list::procs::SystemProcs::default()),
        Arc::new(hangar_server::interactions::InteractionStore::default()),
    );
    options.io_override = Some(io);
    let handle = OtherHandle::spawn(
        target.clone(),
        open_queue(&target).await.unwrap(),
        options,
        tokio::sync::watch::channel(None).0,
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let line = inbox.register("pi-a", tx);
    let owned = handle.clone();
    let send = tokio::spawn(async move {
        owned
            .command(RuntimeCommand {
                operation_id: "entry-a".into(),
                kind: OperationKind::Input,
                payload: json!({"text":"seguir"}),
            })
            .await
    });
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .unwrap()
        .unwrap();
    inbox.respond("pi-a", &json!({"id":frame["id"],"ok":true}));
    send.await.unwrap().unwrap();
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&target.transcript)
        .unwrap();
    writeln!(file,"{}",json!({"type":"message","id":"native-a","message":{"role":"user","content":[{"type":"text","text":"seguir"}]}})).unwrap();
    file.flush().unwrap();
    let confirmed = handle.confirm().await;
    let snapshot = handle.snapshot().await.unwrap();
    handle.stop().await.unwrap();
    inbox.remove("pi-a", &line);
    assert!(confirmed.is_ok());
    assert_eq!(snapshot["queue"][0]["confirmed"], true);
}

#[cfg(unix)]
struct Processes {
    root: PathBuf,
}
#[cfg(unix)]
impl hangar_server::list::procs::ProcessView for Processes {
    fn children(
        &self,
        _: Duration,
    ) -> std::io::Result<Arc<hangar_server::list::procs::ChildrenMap>> {
        Ok(Arc::new(
            if self.root.join("agent").exists() && self.root.join("alive").exists() {
                std::collections::HashMap::from([(123, vec![124])])
            } else {
                Default::default()
            },
        ))
    }
    fn argv(&self, pid: i64) -> Vec<String> {
        if !self.root.join("alive").exists() {
            vec![]
        } else if pid == 124 && self.root.join("agent").exists() {
            vec!["pi".into()]
        } else {
            vec!["sh".into()]
        }
    }
    fn cwd(&self, _: i64) -> Option<PathBuf> {
        Some(self.root.clone())
    }
    fn env_var(&self, _: i64, _: &str) -> std::io::Result<Option<std::ffi::OsString>> {
        Ok(None)
    }
    fn start_time(&self, pid: i64) -> Option<f64> {
        self.root
            .join("alive")
            .exists()
            .then_some(if pid == 124 { 100.5 } else { 100.0 })
    }
    fn recipe(&self, pid: i64) -> std::io::Result<hangar_server::list::procs::ProcessRecipe> {
        if !self.root.join("alive").exists() {
            return Err(std::io::ErrorKind::NotFound.into());
        }
        Ok(hangar_server::list::procs::ProcessRecipe {
            pid,
            birth: self.start_time(pid).unwrap(),
            executable: "pi".into(),
            cwd: self.root.clone(),
            argv: vec!["pi".into()],
            environ: vec![],
            cgroup: None,
        })
    }
}
#[cfg(unix)]
fn raw_list(root: &Path, agent: bool) -> Arc<hangar_server::list::bridge::ListBridge> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(root.join("alive"), b"1").unwrap();
    if agent {
        std::fs::write(root.join("agent"), b"1").unwrap();
    }
    hangar_server::session_open::record::write(root, &spec(root)).unwrap();
    let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
    let program = root.join("mux");
    let pane = format!("pi-a\t1\t123\t{}\t%1\t0\tpi\t100\t0\t0\t0", root.display());
    let script = format!(
        "#!/bin/sh\ncase \"$1\" in\nlist-panes) if [ -f {} ]; then printf '%s\\n' '{}'; fi;;\nshow-environment) printf '%s\\n' 'CP_SESSION_KEY={KEY}';;\ndisplay-message) if [ ! -f {} ]; then printf '%s\\n' 'can\047t find session: pi-a' >&2; exit 1; fi\ncase \"$5\" in *pane_pid*) printf '%s\\n' 'pi-a\t%1\t100\t123';; *) printf '%s\\n' 'pi-a\t%1\t100';; esac;;\nhas-session) if [ -f {} ]; then exit 0; else exit 1; fi;;\nkill-session) if [ -f {} ]; then exit 1; fi; /usr/bin/unlink {};;\n*) exit 1;;\nesac\n",
        quote(&root.join("alive")),
        pane.replace('\'', "'\\''"),
        quote(&root.join("alive")),
        quote(&root.join("alive")),
        quote(&root.join("refuse")),
        quote(&root.join("alive"))
    );
    std::fs::write(&program, script).unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    Arc::new(hangar_server::list::bridge::ListBridge::new(
        hangar_server::list::bridge::ListEnv {
            mux: hangar_server::list::mux::Mux::with_program(&program, Duration::from_secs(2)),
            capture_program: program.into_os_string(),
            procs: Arc::new(Processes { root: root.into() }),
            dirs: Some(dirs(root)),
        },
        hangar_server::list::facts::FactsClient::new(
            "127.0.0.1:9".parse().unwrap(),
            "test-only".into(),
        ),
    ))
}
#[cfg(unix)]
#[tokio::test]
async fn other_lifecycle_factory_recognizes_opening_without_agent_and_rejects_stale_key() {
    let root = tempfile::tempdir().unwrap();
    let list = raw_list(root.path(), false);
    let find = discovery(list.clone());
    let found = (find.find)("pi-a".into()).await;
    assert!(
        matches!(found,Some(Ok(OtherDiscoveryState::Opening(ref opening))) if opening.agent.is_none() && opening.spec.key==KEY && opening.binding.conversation.is_empty())
    );
    let mut wrong = spec(root.path());
    wrong.provider = hangar_server::session_open::AgentProvider::Claude;
    hangar_server::session_open::record::write(root.path(), &wrong).unwrap();
    let rejected = (find.find)("pi-a".into()).await;
    assert!(
        matches!(rejected, Some(Err(_))),
        "o nome sozinho não converte spec de outro agente em abertura Pi"
    );
}
#[cfg(unix)]
#[tokio::test]
async fn other_lifecycle_factory_without_key_writes_no_spec_when_the_key_cannot_be_set() {
    let root = tempfile::tempdir().unwrap();
    let list = raw_list(root.path(), true);
    let program = root.path().join("mux");
    let script = std::fs::read_to_string(&program).unwrap().replace(
        &format!("show-environment) printf '%s\\n' 'CP_SESSION_KEY={KEY}';;"),
        "show-environment) printf '%s\\n' 'unknown variable: CP_SESSION_KEY' >&2; exit 1;;",
    );
    std::fs::write(&program, script).unwrap();
    let specs = || std::fs::read_dir(hangar_server::session_open::record::dir(root.path())).unwrap().count();
    let before = specs();
    for _ in 0..2 {
        let found = (discovery(list.clone()).find)("pi-a".into()).await;
        assert!(matches!(found, Some(Err(ref error)) if error.code == "other_terminal_key_unwritable"), "{found:?}");
    }
    assert_eq!(specs(), before, "chave que não chegou ao pane não deixa spec órfão");
}
#[cfg(unix)]
#[tokio::test]
async fn other_lifecycle_factory_accepts_fresh_ticket_before_jsonl_and_preserves_old_same_life_hook()
 {
    let root = tempfile::tempdir().unwrap();
    let list = raw_list(root.path(), true);
    let path = root.path().join("sid-a.jsonl");
    std::fs::create_dir_all(root.path().join(".claude/.hangar-pi")).unwrap();
    std::fs::write(
        root.path().join(".claude/.hangar-pi/1.json"),
        json!({"file":path,"ts":101}).to_string(),
    )
    .unwrap();
    let found = (discovery(list.clone()).find)("pi-a".into()).await;
    assert!(
        matches!(found,Some(Ok(OtherDiscoveryState::Ready(ref target))) if target.transcript==path && target.binding.conversation=="sid-a" && target.agent_pid==124 && target.created<=101.0)
    );
    assert!(
        !path.exists(),
        "primeiro prompt não depende de JSONL já criado"
    );
}
#[cfg(unix)]
#[tokio::test]
async fn other_lifecycle_close_failure_preserves_queue_and_success_cleans_only_after_proof() {
    let root = tempfile::tempdir().unwrap();
    let _list = raw_list(root.path(), true);
    let mut target = target(root.path());
    target.binding.mux_argv = vec![root.path().join("mux").to_string_lossy().into_owned()];
    std::fs::write(&target.transcript, b"").unwrap();
    let after_count = Arc::new(AtomicUsize::new(0));
    let seen = after_count.clone();
    let alive = root.path().join("alive");
    let after: OtherAfterClosed = Arc::new(move |closed| {
        let absent = !alive.exists();
        seen.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            assert!(absent);
            assert_eq!(closed.life.key(), KEY);
            Ok(vec![])
        })
    });
    let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Ready(target.clone()))));
    let (registry, _) = registry(root.path(), Io::live(), source, Some(after));
    registry.open_other_terminal(target.clone()).await.unwrap();
    registry
        .queue_action(
            "pi-a",
            "append".into(),
            hangar_server::runtime::queue::Action::Append {
                text: "guardar".into(),
                delivered: false,
                ts: None,
                pre_transcript: false,
                entry_id: Some("entry-a".into()),
            },
        )
        .await
        .unwrap();
    std::fs::write(root.path().join("refuse"), b"1").unwrap();
    let refused = registry.close_other("pi-a").await;
    let retained = std::fs::read(&target.state_path).unwrap();
    assert!(refused.is_some_and(|result| result.is_err()));
    assert!(String::from_utf8_lossy(&retained).contains("guardar"));
    assert_eq!(after_count.load(Ordering::SeqCst), 0);
    std::fs::remove_file(root.path().join("refuse")).unwrap();
    let closed = registry.close_other("pi-a").await;
    registry.shutdown().await.unwrap();
    assert!(closed.is_some_and(|result| result.is_ok()));
    assert_eq!(after_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn other_lifecycle_question_payload_matches_the_executed_python_model_and_has_only_interaction_id()
 {
    let root = tempfile::tempdir().unwrap();
    let target = target(root.path());
    std::fs::write(&target.transcript,format!("{}\n",json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall","id":"native-question","name":"question","arguments":{"header":"h","question":"Prosseguir?","options":[{"label":"Sim"},{"label":"Não"}]}}]}}))).unwrap();
    let interactions = Arc::new(hangar_server::interactions::InteractionStore::default());
    let mut options = OtherOptions::new(
        hangar_server::terminal_control::TerminalPool::new(),
        hangar_server::provider_models::inbox::Store::new(),
        Arc::new(hangar_server::list::procs::SystemProcs::default()),
        interactions.clone(),
    );
    options.io_override = Some(Io::live());
    let (live, mut seen) = tokio::sync::watch::channel(None);
    let handle = OtherHandle::spawn(
        target.clone(),
        open_queue(&target).await.unwrap(),
        options,
        live,
    );
    let observed = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(state) = seen.borrow().clone() {
                if !state.public_state["codex_question"].is_null() {
                    break state.public_state["codex_question"].clone();
                }
            }
            seen.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    handle.stop().await.unwrap();
    let id = observed["interaction_id"]
        .as_str()
        .expect("cartão precisa da identidade publicada");
    let binding = hangar_server::interactions::InteractionBinding {
        key: KEY.into(),
        generation: 1,
        conversation: "sid-a".into(),
    };
    assert!(interactions.expected(&binding, Some(id)).is_ok());
    let mut payload = observed.as_object().unwrap().clone();
    payload.remove("interaction_id");
    // Esperado capturado executando app.models.AskQuestion da referência, com as mesmas opções.
    assert_eq!(
        Value::Object(payload),
        json!({"questions":[{"header":"h","multiSelect":false,"question":"Prosseguir?","options":[{"description":"","label":"Sim","preview":""},{"description":"","label":"Não","preview":""}]}]})
    );
}
struct Preview {
    interested: AtomicBool,
    updates: AtomicUsize,
    forgot: AtomicUsize,
}
impl OtherPreviewSource for Preview {
    fn interested(&self, _: &OtherTarget) -> bool {
        self.interested.load(Ordering::SeqCst)
    }
    fn update(
        &self,
        context: OtherPreviewContext<'_>,
    ) -> Result<Option<hangar_api::preview::PreviewEvent>, RuntimeError> {
        self.updates.fetch_add(1, Ordering::SeqCst);
        Ok(Some(hangar_api::preview::PreviewEvent {
            session: context.target.name.clone(),
            text: "prévia".into(),
            md: false,
            full: false,
            vivo: true,
        }))
    }
    fn forget(&self, _: &OtherTarget) {
        self.forgot.fetch_add(1, Ordering::SeqCst);
    }
}
async fn preview_actor(
    root: &Path,
    interest: bool,
) -> (
    OtherHandle,
    Arc<Io>,
    Arc<Preview>,
    tokio::sync::watch::Receiver<Option<Arc<hangar_server::runtime::protocol::LiveState>>>,
) {
    let target = target(root);
    std::fs::write(&target.transcript, b"").unwrap();
    let io = Io::live();
    let preview = Arc::new(Preview {
        interested: AtomicBool::new(interest),
        updates: AtomicUsize::new(0),
        forgot: AtomicUsize::new(0),
    });
    let mut options = OtherOptions::new(
        hangar_server::terminal_control::TerminalPool::new(),
        hangar_server::provider_models::inbox::Store::new(),
        Arc::new(hangar_server::list::procs::SystemProcs::default()),
        Arc::new(hangar_server::interactions::InteractionStore::default()),
    );
    options.io_override = Some(io.clone());
    options.preview = Some(preview.clone());
    let (live, seen) = tokio::sync::watch::channel(None);
    let handle = OtherHandle::spawn(
        target.clone(),
        open_queue(&target).await.unwrap(),
        options,
        live,
    );
    (handle, io, preview, seen)
}
#[tokio::test]
async fn other_lifecycle_preview_interest_ends_preview_without_stopping_state() {
    let root = tempfile::tempdir().unwrap();
    let (handle, io, preview, _) = preview_actor(root.path(), false).await;
    let mut frames = handle.last_frame();
    while frames.borrow().is_none() {
        frames.changed().await.unwrap();
    }
    let updates = preview.updates.load(Ordering::SeqCst);
    preview.interested.store(true, Ordering::SeqCst);
    frames.changed().await.unwrap();
    preview.interested.store(false, Ordering::SeqCst);
    frames.changed().await.unwrap();
    let forgotten = preview.forgot.load(Ordering::SeqCst);
    let captures = io.captures.load(Ordering::SeqCst);
    handle.stop().await.unwrap();
    assert_eq!(updates, 0, "sem hub não há produção de prévia");
    assert!(forgotten > 0, "fim do interesse libera cache da prévia");
    assert!(captures >= 3, "estado continua sem chat");
}
#[tokio::test(start_paused = true)]
async fn other_lifecycle_one_producer_uses_working_and_idle_preview_cadences() {
    let root = tempfile::tempdir().unwrap();
    let hooks = root.path().join(".claude/.hangar-state");
    std::fs::create_dir_all(&hooks).unwrap();
    let marker = hooks.join("sid-a.json");
    std::fs::write(&marker, b"{\"state\":\"working\",\"ts\":101}").unwrap();
    let (handle, io, _, live) = preview_actor(root.path(), true).await;
    let mut frames = handle.last_frame();
    while frames.borrow().is_none() {
        frames.changed().await.unwrap();
    }
    frames.changed().await.unwrap();
    let working = {
        let times = io.times.lock().unwrap();
        times[1] - times[0]
    };
    std::fs::write(marker, b"{\"state\":\"idle\",\"ts\":102}").unwrap();
    loop {
        frames.changed().await.unwrap();
        if live
            .borrow()
            .as_ref()
            .is_some_and(|state| state.public_state["state"] == "idle")
        {
            break;
        }
    }
    let before = io.times.lock().unwrap().len();
    frames.changed().await.unwrap();
    let idle = {
        let times = io.times.lock().unwrap();
        times[before] - times[before - 1]
    };
    handle.stop().await.unwrap();
    assert_eq!(working, Duration::from_millis(150));
    assert_eq!(idle, Duration::from_millis(750));
}

async fn kimi_fallback(root: &Path, child: bool) -> Value {
    let mut target = target(root);
    target.provider = hangar_transcript::Provider::Kimi;
    target.transcript = root.join("sid-a/agents/main/wire.jsonl");
    std::fs::create_dir_all(target.transcript.parent().unwrap()).unwrap();
    std::fs::write(&target.transcript, b"").unwrap();
    let stamp =
        std::time::UNIX_EPOCH + Duration::from_millis(if child { 1_000_000 } else { 1_000_500 });
    std::fs::File::options()
        .write(true)
        .open(&target.transcript)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(stamp))
        .unwrap();
    if child {
        let path = root.join("sid-a/agents/agent-1/wire.jsonl");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"").unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(1003)),
            )
            .unwrap();
    }
    let hooks = root.join(".claude/.hangar-state");
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(
        hooks.join("sid-a.json"),
        b"{\"state\":\"idle\",\"ts\":1000}",
    )
    .unwrap();
    let mut options = OtherOptions::new(
        hangar_server::terminal_control::TerminalPool::new(),
        hangar_server::provider_models::inbox::Store::new(),
        Arc::new(hangar_server::list::procs::SystemProcs::default()),
        Arc::new(hangar_server::interactions::InteractionStore::default()),
    );
    options.io_override = Some(Io::live());
    let handle = OtherHandle::spawn(
        target.clone(),
        open_queue(&target).await.unwrap(),
        options,
        tokio::sync::watch::channel(None).0,
    );
    let mut frames = handle.last_frame();
    tokio::time::timeout(Duration::from_secs(10), async {
        while frames.borrow().is_none() {
            frames.changed().await.unwrap();
        }
    })
    .await
    .unwrap();
    let state = handle.snapshot().await.unwrap()["view"]["public_state"]["state"].clone();
    handle.stop().await.unwrap();
    state
}
#[tokio::test]
async fn other_lifecycle_kimi_fallback_keeps_idle_for_a_non_turn_write_within_two_seconds() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(kimi_fallback(root.path(), false).await, "idle");
}
#[tokio::test]
async fn other_lifecycle_kimi_fallback_observes_the_child_wire_only_without_a_turn_boundary() {
    let root = tempfile::tempdir().unwrap();
    assert_eq!(kimi_fallback(root.path(), true).await, "working");
}

#[tokio::test]
async fn other_lifecycle_rename_preserves_queue_generation_and_inbox_without_killing_cli() {
    let root = tempfile::tempdir().unwrap();
    let target = target(root.path());
    std::fs::write(&target.transcript, b"").unwrap();
    let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Ready(target.clone()))));
    let io = Io::live();
    let (registry, inbox) = registry(root.path(), io.clone(), source, None);
    registry.open_other_terminal(target.clone()).await.unwrap();
    registry
        .queue_action(
            "pi-a",
            "seed".into(),
            hangar_server::runtime::queue::Action::Append {
                text: "histórico da fila".into(),
                delivered: true,
                ts: Some(101.0),
                pre_transcript: false,
                entry_id: Some("stable-id".into()),
            },
        )
        .await
        .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let line = inbox.register(KEY, tx);
    let detached = registry.detach_for_rename("pi-a").await;
    let mut renamed = false;
    if let Ok(Some(detached)) = detached {
        renamed = registry
            .reattach_after_rename(detached, "pi-b")
            .await
            .is_ok();
    }
    let info = registry.other_info("pi-b").await;
    let snap = registry.list_snapshots().await;
    let owned = inbox.clone();
    let send = tokio::spawn(async move {
        owned
            .deliver("pi-b", "nome fresco", Some("after-rename"))
            .await
    });
    let frame = tokio::time::timeout(Duration::from_secs(2), rx.recv())
        .await
        .ok()
        .flatten();
    if let Some(frame) = frame {
        hangar_server::provider_models::inbox::Store::respond_registered(
            &line,
            &json!({"id":frame["id"],"ok":true}),
        );
    }
    let delivered = send.await.unwrap();
    registry.shutdown().await.unwrap();
    inbox.remove(KEY, &line);
    assert!(renamed);
    assert_eq!(
        info.map(|info| info.lifecycle_id),
        Some(Some(format!("k:{KEY}:g:1")))
    );
    assert_eq!(snap[KEY]["queue"][0]["id"], "stable-id");
    assert_eq!(delivered, "sent");
    assert!(io.writes.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn other_lifecycle_reused_claude_name_registers_pi_without_killing_current_cli() {
    use hangar_server::runtime::terminal::TerminalTarget;
    const OLD_KEY: &str = "fedcba9876543210fedcba9876543210";
    let root = tempfile::tempdir().unwrap();
    let _list = raw_list(root.path(), true);
    let mut current = target(root.path());
    current.binding.mux_argv = vec![root.path().join("mux").to_string_lossy().into_owned()];
    std::fs::write(&current.transcript, b"").unwrap();
    let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Ready(
        current.clone(),
    ))));
    let io = Io::live();
    let (registry, _) = registry(root.path(), io.clone(), source, None);
    let hubs = hangar_server::side::Hubs::default();
    registry.bind_hubs(hubs.downgrade());
    let mut old_binding = current.binding.clone();
    old_binding.created = 99;
    old_binding.conversation = "claude-old".into();
    let old_path = root.path().join("claude-old.jsonl");
    std::fs::write(&old_path, b"").unwrap();
    let old = TerminalTarget {
        key: OLD_KEY.into(),
        generation: 1,
        name: current.name.clone(),
        binding: old_binding,
        lease_path: current
            .projection_dir
            .join(format!("runtime/{OLD_KEY}.lock")),
        state_path: current
            .projection_dir
            .join(format!("runtime/{OLD_KEY}.json")),
        projection_dir: current.projection_dir.clone(),
        transcript: old_path,
        created: 99.0,
        plugin_key: None,
        metadata: hangar_server::terminal_facts::TerminalMetadata {
            config_dir: root.path().join(".claude"),
            agent_pid: u32::MAX - 7,
            agent_birth: 99.5,
            legacy_import_after: None,
            fingerprint: "old-pane-life".into(),
            origin: "uds:test-old-life".into(),
        },
    };
    let program = root.path().join("mux");
    let new_script = std::fs::read_to_string(&program).unwrap();
    let old_script = new_script
        .replace(
            &format!("CP_SESSION_KEY={KEY}"),
            &format!("CP_SESSION_KEY={OLD_KEY}"),
        )
        .replace("pi-a\t%1\t100", "pi-a\t%1\t99")
        .replace("\tpi\t100\t", "\tpi\t99\t");
    std::fs::write(&program, old_script).unwrap();
    registry.open_terminal(old).await.unwrap();
    let had_old = registry.list_snapshots().await.contains_key(OLD_KEY);
    std::fs::write(program, new_script).unwrap();
    let adopted = registry.adopt_now("pi-a").await;
    let snapshots=registry.list_snapshots().await;
    let old_remains = snapshots.contains_key(OLD_KEY);
    let current_registered = snapshots.contains_key(KEY);
    let info = registry.other_info("pi-a").await;
    let current_alive = root.path().join("alive").exists();
    registry.shutdown().await.unwrap();
    assert!(
        had_old,
        "a reprodução começa com Claude terminal já adotado"
    );
    assert!(
        matches!(adopted, Some(Ok(()))),
        "a entrada Claude da vida anterior não pode bloquear Other: {adopted:?}"
    );
    assert!(!old_remains);
    assert!(current_registered);
    assert_eq!(info.map(|info| info.provider), Some("pi".into()));
    assert!(
        current_alive,
        "retirar o registro antigo não mata o CLI da vida nova"
    );
    assert!(io.writes.lock().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn other_lifecycle_list_and_history_share_the_proven_actor_without_python() {
    hangar_server::diag::disable_diary();
    use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let root = tempfile::tempdir().unwrap();
    let list = raw_list(root.path(), true);
    let target = target(root.path());
    let rows = [
        json!({"type":"message","id":"u1","timestamp":"2026-01-01T00:00:00Z","message":{"role":"user","content":[{"type":"text","text":"ola"}]}}),
        json!({"type":"message","id":"a1","timestamp":"2026-01-01T00:00:01Z","message":{"role":"assistant","content":[{"type":"text","text":"oi!"}]}}),
    ];
    std::fs::write(&target.transcript, rows.iter().map(|row| format!("{row}\n")).collect::<String>()).unwrap();
    let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Ready(target.clone()))));
    let (registry, _) = registry(root.path(), Io::live(), source, None);
    list.set_plugin_runtime(registry.clone());
    registry.adopt_now("pi-a").await.unwrap().unwrap();
    let mut live = registry.live("pi-a");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if live.borrow().as_ref().is_some_and(|state| state.public_state.is_object()) { break; }
            live.changed().await.unwrap();
        }
    }).await.unwrap();
    let snapshot = list.refresh().await.unwrap();
    assert_eq!(snapshot.rows.len(), 1);
    let row = &snapshot.rows[0];
    assert_eq!(row.provider, "pi");
    assert_eq!(row.lifecycle_id.as_deref(), Some(target.lifecycle_id().as_str()));
    assert_eq!(row.jsonl.as_deref(), Some(target.transcript.to_str().unwrap()));
    assert_eq!(row.last_reply.as_deref(), Some("oi!"));
    let mut state = AppState::new(Config {
        listen:"127.0.0.1:0".parse().unwrap(), upstream:"127.0.0.1:9".parse().unwrap(),
        internal_secret:"test-only".into(), auth_token:"owner-test".into(), log_path:None, trusted:TrustedHosts::parse(""),
    });
    state.origins_home = root.path().to_owned();
    state.list = list.clone();
    state.side.locate = Arc::new(hangar_server::locate::ListLocate {list});
    state.side.runtime.set(registry.clone()).ok().unwrap();
    let app = hangar_server::routes::router(Arc::new(state));
    let mut request = axum::http::Request::builder().uri("/api/sessions/pi-a/history")
        .header("authorization", "Bearer owner-test").body(axum::body::Body::empty()).unwrap();
    request.extensions_mut().insert(axum::extract::ConnectInfo("127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap()));
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let events: Vec<Value> = serde_json::from_slice(&body).unwrap();
    assert_eq!(events.iter().map(|event| (event["kind"].as_str(), event["text"].as_str())).collect::<Vec<_>>(), [
        (Some("user_msg"), Some("ola")), (Some("assistant_msg"), Some("oi!")),
    ]);
    registry.shutdown().await.unwrap();
}

#[tokio::test]
async fn other_lifecycle_public_select_and_answer_refuse_an_open_terminal_panel() {
    use axum::{extract::{ConnectInfo, State}, http::{Request, StatusCode}};
    use hangar_server::{auth::TrustedHosts, config::Config, routes::AppState};
    use hangar_transcript::Provider;
    use http_body_util::BodyExt;

    for provider in [Provider::Pi, Provider::Omp, Provider::Kimi] {
        for route in ["select", "answer"] {
            let root = tempfile::tempdir().unwrap();
            let mut target = target(root.path());
            target.provider = provider;
            let question = json!({"question":"Qual cor?","options":[{"label":"Azul"},{"label":"Verde"}]});
            let event = match (provider, route) {
                (Provider::Kimi, "select") => json!({"type":"interaction.request","agentId":"main","id":"approval_1","kind":"approval",
                    "toolCallId":"tool_x","request":{"turnId":8,"toolCallId":"tool_x","toolName":"ExitPlanMode","action":"",
                        "display":{"kind":"plan_review","plan":"# Plano"}},"time":1_786_454_000_000u64}),
                (Provider::Kimi, _) => json!({"type":"context.append_loop_event","event":{"type":"tool.call",
                    "toolCallId":"q1","name":"AskUserQuestion","args":{"questions":[question]}}}),
                _ => {
                    let (name, arguments) = if provider == Provider::Omp {
                        ("ask", json!({"questions":[question]}))
                    } else { ("question", question) };
                    json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall",
                        "name":name,"id":"q1","arguments":arguments}]}})
                }
            };
            std::fs::write(&target.transcript, format!("{event}\n")).unwrap();
            let io = Io::live();
            *io.pane.lock().unwrap() = Some(match (provider, route) {
                (Provider::Kimi, "select") => "  ▶ Ready to build with this plan?\n\n  ▶ 1. Approve\n    2. Reject\n    3. Revise\n\n  ↑/↓ select · 1/2/3 choose · ↵ confirm\n".into(),
                (Provider::Kimi, _) => "  question\n\n   Cor    Submit\n\n  ? Qual cor?\n\n   → [1] Azul\n     [2] Verde\n     [3] Other\n\n   ↑↓ select  1-3 / ↵ choose  ←/→/tab switch  esc cancel".into(),
                _ => format!("● Question Qual cor?\n\n> 1. Azul\n  2. Verde\n  3. Type something.\n↑↓ navigate • Enter to select • Esc to cancel\n{}\n status\n", "─".repeat(60)),
            });
            let interactions = Arc::new(hangar_server::interactions::InteractionStore::default());
            let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Ready(target.clone()))));
            let (registry, _) = registry_with_interactions(root.path(), io.clone(), source, None, interactions.clone());
            registry.open_other_terminal(target.clone()).await.unwrap();
            let mut live = registry.live(&target.name);
            let published_id = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    if let Some(id) = live.borrow().as_ref().and_then(|state|
                        state.public_state["interaction_id"].as_str().map(str::to_owned)) {
                        break id;
                    }
                    live.changed().await.unwrap();
                }
            }).await.unwrap();
            let mut state = AppState::new(Config {
                listen: "127.0.0.1:0".parse().unwrap(), upstream: "127.0.0.1:9".parse().unwrap(),
                internal_secret: "test-only".into(), auth_token: "owner-test".into(),
                log_path: None, trusted: TrustedHosts::parse(""),
            });
            state.interactions = interactions;
            state.state.runtime.set(registry.clone()).ok().unwrap();
            state.term.mark_open_for_test(&target.name, &state.diag).await;
            let body = if route == "select" {
                json!({"option":1,"interaction_id":published_id})
            } else {
                json!({"interaction_id":published_id,"answers":[{"kind":"option","indices":[0],"labels":["Azul"]}]})
            };
            let request = Request::builder().method("POST")
                .uri(format!("/api/sessions/{}/{route}", target.name))
                .header("authorization", "Bearer owner-test")
                .header("content-type", "application/json")
                .body(axum::body::Body::from(body.to_string())).unwrap();
            let peer = ConnectInfo("127.0.0.1:50000".parse::<std::net::SocketAddr>().unwrap());
            let state = State(Arc::new(state));
            let response = if route == "select" {
                hangar_server::session_write::control::select(state, peer, request).await
            } else {
                hangar_server::session_write::answer::answer(state, peer, request).await
            };
            let status = response.status();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            let reply: Value = serde_json::from_slice(&bytes).unwrap();
            registry.shutdown().await.unwrap();
            assert_eq!(status, StatusCode::CONFLICT, "{provider:?}/{route}: {reply}");
            assert_eq!(reply["detail"]["code"], "erro_terminal_aberto", "{provider:?}/{route}: {reply}");
            assert!(io.writes.lock().unwrap().is_empty(), "{provider:?}/{route}: painel aberto não recebe teclado");
        }
    }
}

#[tokio::test]
async fn other_lifecycle_first_input_creates_transcript_then_confirms_the_original_id() {
    for provider in [hangar_transcript::Provider::Pi, hangar_transcript::Provider::Omp, hangar_transcript::Provider::Kimi] {
        let root = tempfile::tempdir().unwrap();
        let mut opening = opening(root.path());
        opening.spec.provider = serde_json::from_value(json!(provider.as_str())).unwrap();
        opening.agent = Some(OtherProcess {pid:124, birth:100.5});
        let source = Arc::new(Mutex::new(Some(OtherDiscoveryState::Opening(opening))));
        let io = Io::live();
        let mut ready = target(root.path());
        ready.provider = provider;
        *io.first_input.lock().unwrap() = Some((ready.clone(), source.clone()));
        let (registry, _) = registry(root.path(), io.clone(), source, None);
        registry.adopt_now("pi-a").await;
        assert!(!ready.transcript.exists());
        let reply = registry.queue_other_opening("pi-a", KEY, "NATIVE_PROOF_1", "first-id").await.unwrap();
        assert_eq!(reply["queue_id"], "first-id");
        assert_eq!(*io.writes.lock().unwrap(), ["NATIVE_PROOF_1", "Enter"], "{provider:?}");
        assert!(ready.transcript.exists(), "a CLI só cria o transcript após o primeiro envio");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = registry.list_snapshots().await;
                if snapshot.get(KEY).and_then(|state| state.get("queue")).and_then(Value::as_array)
                    .and_then(|rows| rows.first()).is_some_and(|row| row["confirmed"] == true) {
                    assert_eq!(snapshot[KEY]["queue"][0]["id"], "first-id");
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        assert_eq!(registry.other_info("pi-a").await.unwrap().session_key, "sid-a");
        assert_eq!(*io.writes.lock().unwrap(), ["NATIVE_PROOF_1", "Enter"]);
        registry.shutdown().await.unwrap();
    }
}
