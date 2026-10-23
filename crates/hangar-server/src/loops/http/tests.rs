use super::*;
use std::collections::BTreeMap;
use std::sync::Mutex;

struct Fake {
    store: Store,
    rows: Vec<SessionRow>,
    automations: bool,
    branch: Option<String>,
    queued: Mutex<BTreeMap<String, Vec<Value>>>,
    pushed: Mutex<Vec<Value>>,
}

impl Backend for Fake {
    fn store(&self) -> Store { self.store.clone() }
    fn now(&self) -> f64 { 1000.0 }
    fn automations(&self) -> bool { self.automations }
    fn row<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Option<SessionRow>, Reply>> {
        Box::pin(async move { Ok(self.rows.iter().find(|r| r.name == name).cloned()) })
    }
    fn exists<'a>(&'a self, name: &'a str) -> BoxFuture<'a, bool> { Box::pin(async move { self.rows.iter().any(|r| r.name == name) }) }
    fn branch(&self, _row: &SessionRow) -> Option<String> { self.branch.clone() }
    fn append<'a>(&'a self, name: &'a str, text: &'a str) -> BoxFuture<'a, Result<String, (String, String)>> {
        Box::pin(async move {
            let mut q = self.queued.lock().unwrap();
            let rows = q.entry(name.to_owned()).or_default();
            let id = format!("q-{name}-{}", rows.len());
            rows.push(json!({"id": id, "text": text, "delivered": false}));
            Ok(id)
        })
    }
    fn drain<'a>(&'a self, _name: &'a str) -> BoxFuture<'a, ()> { Box::pin(async {}) }
    fn notify(&self, name: &str, status: &str, reason: &str) {
        self.pushed.lock().unwrap().push(json!([name, tick::body(status, Some(reason))]));
    }
    fn refine<'a>(&'a self, _goal: &'a str, _check: Option<&'a str>) -> BoxFuture<'a, Result<String, String>> {
        Box::pin(async { Err("não chamado".into()) })
    }
}

fn row(v: Value) -> SessionRow { serde_json::from_value(v).unwrap() }

#[tokio::test]
async fn routes_match_the_python() {
    for c in crate::loops::tests::golden()["routes"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake {
            store: Store::new(dir.path()),
            rows: vec![
                row(json!({"name": "cc", "cwd": "/repo", "jsonl": "/repo/t.jsonl", "git_cwd": "/repo-feat", "provider": "claude"})),
                row(json!({"name": "cx", "cwd": "/repo", "jsonl": "/repo/x.jsonl", "provider": "codex"})),
                row(json!({"name": "b", "cwd": "/b", "jsonl": "/b/t.jsonl", "provider": "claude"})),
            ],
            automations: c["automations"].as_bool().unwrap(),
            branch: c["branch"].as_str().map(str::to_owned),
            queued: Mutex::default(), pushed: Mutex::default(),
        };
        let path = c["path"].as_str().unwrap();
        let (name, op) = op(&Method::from_bytes(c["method"].as_str().unwrap().as_bytes()).unwrap(), path).unwrap();
        if let Some(initial) = c["initial"].as_object() { fake.store.set(&name, initial).unwrap(); }
        let body = if c["body"].is_null() { Vec::new() } else { serde_json::to_vec(&c["body"]).unwrap() };
        let (status, resp) = handle(&fake, &name, op, &body).await;
        let label = c["label"].as_str().unwrap();
        assert_eq!(status.as_u16() as u64, c["status"].as_u64().unwrap(), "{label}: {resp}");
        // O 422 do Rust tem o formato do `unprocessable_body`, não o do pydantic.
        if status != StatusCode::UNPROCESSABLE_ENTITY { assert_eq!(resp, c["response"], "{label}"); }
        assert_eq!(Value::Array(fake.pushed.lock().unwrap().clone()), c["pushed"], "{label}");
        assert_eq!(json!(*fake.queued.lock().unwrap()), c["queued"], "{label}");
        assert_eq!(fake.store.get(&name).map(Value::Object).unwrap_or(Value::Null), c["loop_after"], "{label}");
        assert_eq!(fake.store.chain(&name).map(Value::Object).unwrap_or(Value::Null), c["chain_after"], "{label}");
    }
}

#[test]
fn suggestions_match_the_python() {
    for c in crate::loops::tests::golden()["suggestions"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        for (file, body) in c["files"].as_object().unwrap() { std::fs::write(dir.path().join(file), body.as_str().unwrap()).unwrap(); }
        let got = suggest_checks(dir.path());
        // `package.json` que é lista derrubava a rota no Python (500); aqui só não sugere npm.
        if c["suggestions"].get("raises").is_some() { assert!(got.is_empty(), "{c}"); continue; }
        assert_eq!(json!(got), c["suggestions"], "{c}");
    }
}

#[test]
fn op_reads_the_routes_and_the_escaped_name() {
    assert_eq!(op(&Method::PUT, "/api/sessions/a%20b/then"), Some(("a b".into(), Op::ThenSet)));
    assert_eq!(op(&Method::POST, "/api/sessions/s/loop/resolve"), Some(("s".into(), Op::Resolve)));
    assert_eq!(op(&Method::GET, "/api/sessions/s/loop"), Some(("s".into(), Op::Get)));
    assert_eq!(op(&Method::GET, "/api/sessions/s/then"), None);
    assert_eq!(op(&Method::POST, "/api/sessions//loop"), None);
    assert_eq!(op(&Method::POST, "/api/sessions/s/loop/x"), None);
    assert!(crate::migration_status::rust_route(&Method::DELETE, "/api/sessions/s/loop"));
}

/// B3: a trava de branch lê a worktree onde o agente trabalha, na criação e em cada volta.
#[test]
fn branch_reads_the_agent_worktree() {
    let dir = tempfile::tempdir().unwrap();
    let (main, feat) = (dir.path().join("repo"), dir.path().join("repo-feat"));
    std::fs::create_dir_all(main.join(".git")).unwrap();
    std::fs::write(main.join(".git/HEAD"), "ref: refs/heads/main\n").unwrap();
    std::fs::create_dir_all(main.join(".git/worktrees/feat")).unwrap();
    std::fs::write(main.join(".git/worktrees/feat/HEAD"), "ref: refs/heads/feat\n").unwrap();
    std::fs::create_dir_all(feat.join("sub")).unwrap();
    std::fs::write(feat.join(".git"), format!("gitdir: {}\n", main.join(".git/worktrees/feat").display())).unwrap();
    let r = row(json!({"name": "s", "cwd": main, "git_cwd": feat.join("sub"), "provider": "claude"}));
    assert_eq!(crate::loops::branch(&r).as_deref(), Some("feat"));
    let r = row(json!({"name": "s", "cwd": main.join("x"), "provider": "claude"}));
    assert_eq!(crate::loops::branch(&r).as_deref(), Some("main"));
}

proptest::proptest! {
    #[test]
    fn any_body_never_panics(raw in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..128)) {
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let fake = Fake { store: Store::new(dir.path()), rows: vec![row(json!({"name": "s", "jsonl": "/t", "provider": "claude"}))],
            automations: true, branch: None, queued: Mutex::default(), pushed: Mutex::default() };
        for op in [Op::ThenSet, Op::Create, Op::Resolve, Op::Refine] {
            let _ = rt.block_on(handle(&fake, "s", op, &raw));
        }
    }
}
