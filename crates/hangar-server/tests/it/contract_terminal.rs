use hangar_server::terminal_state::{analyze, reduce, ReducerFacts, ReducerMemory, STALE_LIMIT, IDLE_DEBOUNCE};

#[test]
fn terminal_debounce_limits_match_python_state_monitor() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/app/state.py");
    let source = std::fs::read_to_string(path).expect("referência Python deve existir");
    let class = regex::Regex::new(r"(?m)^class StateMonitor:\r?$").unwrap()
        .find(&source).expect("classe StateMonitor deve existir");
    let rest = &source[class.end()..];
    let end = regex::Regex::new(r"(?m)^(?:class|def) ").unwrap().find(rest)
        .map_or(rest.len(), |next| next.start());
    let body = &rest[..end];
    for (name, rust) in [("STALE_LIMIT", STALE_LIMIT), ("IDLE_DEBOUNCE", IDLE_DEBOUNCE)] {
        let declaration = regex::Regex::new(&format!(r"(?m)^    {name}[ \t]*=[ \t]*([0-9]+)[ \t]*(?:#.*)?\r?$")).unwrap();
        let values: Vec<_> = declaration.captures_iter(body).collect();
        assert_eq!(values.len(), 1, "StateMonitor deve declarar {name} uma vez");
        let python: u32 = values[0][1].parse().expect("limite Python deve caber em u32");
        assert_eq!(rust, python, "limite {name} precisa acompanhar a referência Python");
    }
}

#[test]
fn terminal_parsers_and_sequences_match_python() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../backend/tests/fixtures/contract");
    let rows: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("golden/terminal.json")).unwrap(),
    ).unwrap();
    let mut failures = Vec::new();
    for row in rows.as_array().unwrap() {
        if let Some(pane) = row["pane"].as_str() {
            let actual = serde_json::to_value(analyze(pane)).unwrap();
            if actual != row["expected"] {
                failures.push(format!("{}: Rust {actual}; Python {}", row["name"], row["expected"]));
            }
        } else {
            let mut memory = ReducerMemory::default();
            for (index, frame) in row["sequence"].as_array().unwrap().iter().enumerate() {
                let facts: ReducerFacts = serde_json::from_value(frame["facts"].clone()).unwrap();
                let result = reduce(frame["pane"].as_str().unwrap(), memory, facts);
                assert_eq!(serde_json::to_value(&result).unwrap(), row["expected_sequence"][index], "{} frame {}", row["name"], index);
                memory = result.memory;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

mod monitor {
    use hangar_api::state::{ShellVivo, StateEvent};
    use hangar_server::state::facts::{Dead, Received, StateFacts};
    use hangar_server::state::monitor::{CaptureFailed, FileFacts, Frame, LoopInfo, Monitor, RoundFacts, Sources};
    use hangar_server::terminal_state::{analyze, TerminalQuestion};
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    use tokio::sync::Notify;

    /// Repete as entradas que o `StateMonitor` do Python recebeu, uma por rodada.
    struct Scripted { frames: Vec<Value>, rounds: Vec<Value>, round: AtomicUsize, wake: Arc<Notify>, done: Notify,
                      events: Mutex<Vec<(usize, StateEvent)>> }

    impl Scripted {
        fn current(&self) -> &Value { &self.frames[self.round.load(Ordering::SeqCst) - 1] }
        fn in_transfer(&self) -> bool {
            let f = &self.current()["facts"];
            !f.is_null() && (f["transfer_active"] == true || f["in_transfer_ms"].as_u64() > Some(0))
        }
    }

    /// Do lado de fora da crate o `Sources` não vai num `Arc` alheio.
    struct Src(Arc<Scripted>);

    impl std::ops::Deref for Src {
        type Target = Scripted;
        fn deref(&self) -> &Scripted { &self.0 }
    }

    impl Sources for Src {
        fn name(&self) -> &str { "fixture" }
        fn sid(&self) -> Option<String> { Some("fixture-sid".into()) }
        fn epoch(&self) -> u64 { 0 }
        fn wake(&self) -> Arc<Notify> { self.wake.clone() }
        async fn facts(&self) -> RoundFacts {
            let now = Instant::now();
            let raw = &self.current()["facts"];
            if self.current()["wake"] == true { self.wake.notify_waiters(); }
            if raw.is_null() { return RoundFacts::from_received(None, now, None); }
            let facts: StateFacts = serde_json::from_value(raw.clone()).unwrap();
            RoundFacts::from_received(Some(&Received { facts: Arc::new(facts), at: now }), now, None)
        }
        async fn capture(&self) -> Result<Frame, CaptureFailed> {
            let i = self.round.fetch_add(1, Ordering::SeqCst);
            if i >= self.frames.len() {
                self.done.notify_one();
                std::future::pending::<()>().await;
            }
            let f = &self.frames[i];
            if let Some(code) = f["fail"].as_str() {
                return Err(CaptureFailed { code: code.into(), attempt: f["attempt"].as_u64().map(|a| a as u32) });
            }
            let text = f["pane"].as_str().unwrap();
            Ok(Frame { text: text.into(), analysis: analyze(text) })
        }
        async fn has_session(&self) -> Option<bool> {
            let f = self.current();
            if f.get("exists").is_none() { Some(true) } else { f["exists"].as_bool() }
        }
        async fn dead(&self) -> Result<Dead, String> { Ok(if self.in_transfer() { Dead::InTransfer } else { Dead::Ok }) }
        async fn observe_permission(&self, key: &str, _: &str) -> Result<(String, String), String> {
            assert_eq!(key, "fixture-sid");
            let i = self.round.load(Ordering::SeqCst) - 1;
            let answer = &self.rounds[i]["observe"];
            assert!(!answer.is_null(), "o Rust perguntou numa rodada em que o Python não chamou");
            Ok((answer[0].as_str().unwrap().into(), answer[1].as_str().unwrap().into()))
        }
        async fn files(&self, _: Option<&str>) -> FileFacts {
            let f = self.current();
            let text = |k: &str| f[k].as_str().map(String::from);
            FileFacts {
                marker: text("marker"),
                open_question: f["open_question"].as_object().map(|q| TerminalQuestion {
                    question: q["question"].as_str().map(String::from),
                    options: q["options"].as_array().unwrap().iter().map(|o| o.as_str().unwrap().into()).collect() }),
                status_line: text("status_line"),
                loop_info: f["loop"].as_object().map(|l| LoopInfo { status: l["status"].as_str().map(String::from),
                    iter: l["iter"].as_u64().map(|v| v as u32), max: l["max_iters"].as_u64().map(|v| v as u32) }),
                shells: f.get("shells").map_or_else(Vec::new, |s| serde_json::from_value::<Vec<ShellVivo>>(s.clone()).unwrap()),
                ..FileFacts::default()
            }
        }
        async fn publish(&self, event: StateEvent) -> bool {
            self.events.lock().unwrap().push((self.round.load(Ordering::SeqCst) - 1, event));
            true
        }
        fn hub_wake(&self) -> Arc<Notify> { Arc::default() }
        async fn emit(&self, _: &'static str, _: Value) -> bool { true }
        fn runtime_wake(&self) -> Arc<Notify> { Arc::default() }
        fn runtime_problem(&self) -> Option<(String, String)> { None }
        async fn ask_payload(&self) -> Result<Option<hangar_api::ask::AskQuestion>, String> { Ok(None) }
        fn deliverable(&self) {}
        async fn preview_capture(&self) -> Option<Result<Frame, CaptureFailed>> { None }
        async fn preview_files(&self, _: &str) -> Vec<hangar_server::state::preview::HookFile> { Vec::new() }
        fn committed(&self) -> Option<Arc<str>> { None }
        async fn publish_preview(&self, _: hangar_api::preview::PreviewEvent) -> bool { true }
        fn wall(&self) -> f64 { hangar_server::state::monitor::wall_now() }
    }

    #[tokio::test(start_paused = true)]
    async fn monitor_sequences() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract");
        let rows: Value = serde_json::from_slice(&std::fs::read(root.join("golden/terminal_monitor.json")).unwrap()).unwrap();
        let rows = rows.as_array().unwrap();
        assert!(rows.len() >= 15);
        for row in rows {
            let name = row["name"].as_str().unwrap();
            let rounds = row["rounds"].as_array().unwrap().clone();
            let src = Arc::new(Scripted { frames: row["frames"].as_array().unwrap().clone(), rounds: rounds.clone(),
                round: AtomicUsize::new(0), wake: Arc::default(), done: Notify::new(), events: Mutex::default() });
            let mut task = tokio::spawn(Monitor::new(Src(src.clone())).run());
            tokio::select! {
                _ = &mut task => {}
                () = src.done.notified() => task.abort(),
            }
            let ran = src.round.load(Ordering::SeqCst).min(src.frames.len());
            assert_eq!(ran, rounds.len(), "{name}: rodadas");
            let events = src.events.lock().unwrap();
            for (i, expected) in rounds.iter().enumerate() {
                let got: Vec<&StateEvent> = events.iter().filter(|(r, _)| *r == i).map(|(_, e)| e).collect();
                let want: Option<StateEvent> = (!expected["event"].is_null()).then(|| serde_json::from_value(expected["event"].clone()).unwrap());
                assert_eq!(got.first().copied(), want.as_ref(), "{name}: rodada {i}");
                assert!(got.len() <= 1, "{name}: rodada {i} publicou duas vezes");
            }
        }
    }
    /// A lista (sem `Monitor`) repete as entradas do caso real e diz, rodada a rodada, o mesmo
    /// estado que o `Monitor` publicou: a permissão segurada pelo hook é `awaiting_input` nos dois.
    struct Pane(Mutex<String>);

    impl hangar_server::list::classify::CaptureSource for Pane {
        async fn capture(&self, _: &str) -> Result<String, hangar_server::list::classify::CaptureFailed> {
            Ok(self.0.lock().unwrap().clone())
        }
        async fn pause(&self, _: std::time::Duration) {}
        fn wall(&self) -> f64 { 1000.0 }
        fn mono(&self) -> f64 { 1000.0 }
    }

    #[tokio::test]
    async fn permission_card_after_bash() {
        use hangar_server::list::classify::{Classifier, Facts};
        use hangar_server::list::facts_files::HookStates;
        use std::collections::BTreeMap;
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract");
        let rows: Value = serde_json::from_slice(&std::fs::read(root.join("golden/terminal_monitor.json")).unwrap()).unwrap();
        let row = rows.as_array().unwrap().iter().find(|r| r["name"] == "permission_card_after_bash").expect("sequência gravada");
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join(".claude");
        std::fs::create_dir_all(cfg.join(".hangar-state")).unwrap();
        let dirs = vec![cfg.clone()];
        let base: hangar_api::session::SessionRow = serde_json::from_value(serde_json::json!({
            "name": "fixture", "jsonl": dir.path().join("fixture-sid.jsonl").to_str().unwrap()})).unwrap();
        let (pane, mut classifier, mut monitor_state, mut awaiting) = (Pane(Mutex::default()), Classifier::default(), None, 0);
        let (headless, problems, monitors) = (BTreeMap::new(), BTreeMap::new(), hangar_server::state::published::Published::default());
        for (i, (frame, round)) in row["frames"].as_array().unwrap().iter().zip(row["rounds"].as_array().unwrap()).enumerate() {
            if let Some(state) = round["event"]["state"].as_str() { monitor_state = Some(state.to_owned()); }
            *pane.0.lock().unwrap() = frame["pane"].as_str().unwrap().to_owned();
            std::fs::write(cfg.join(".hangar-state/fixture-sid.json"),
                format!(r#"{{"state":"{}","ts":999.0}}"#, frame["marker"].as_str().unwrap())).unwrap();
            // O Python só manda a pergunta ainda valendo (`pergunta_pendente`).
            let q = &frame["facts"]["question"];
            let held: BTreeMap<String, Value> = (!q.is_null() && q["seen_age_ms"].as_u64() <= Some(35_000))
                .then(|| ("fixture".to_owned(), q.clone())).into_iter().collect();
            let hooks = HookStates::load(&dirs);
            let facts = Facts { hooks: &hooks, alive: &|_| false, config_dirs: &dirs, headless: Some(&headless),
                                problems: &problems, stall_seconds: 300.0, held: &held, monitors: &monitors };
            // Cada tique parte da linha da descoberta, como na produção.
            let mut session = base.clone();
            classifier.classify(std::slice::from_mut(&mut session), &facts, &pane).await;
            assert_eq!(Some(session.state.as_str()), monitor_state.as_deref(), "rodada {i}: lista × Monitor");
            awaiting += usize::from(session.state == "awaiting_input");
        }
        assert!(awaiting >= 4, "a espera pela permissão aparece na lista");
    }
}
