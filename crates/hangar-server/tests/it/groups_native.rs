//! Casos públicos dos callbacks de grupos migrados, com cofre e sidecars isolados.
use super::*;
use hangar_server::groups::service::{BoxFuture, OrqFacts, OrqPhase, PromoteError};
use hangar_server::orchestration::{context, runs};

fn identity_file(home: &Path, name: &str) {
    let dir = home.join(".hangar/claude-headless");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(format!("{name}.json")),
        json!({"key":format!("key-{name}")}).to_string(),
    )
    .unwrap();
}
fn run(home: &Path, id: &str, gid: &str, end: bool) {
    let root = runs::root(home).join(id);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("orq.json"), "{\"auto\":true}").unwrap();
    let mut events = json!({"tipo":"execucao_inicio","gid":gid}).to_string() + "\n";
    if end {
        events += "{\"tipo\":\"execucao_fim\"}\n";
    }
    std::fs::write(root.join("eventos.jsonl"), events).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn phase_uses_actual_runs_and_never_a_python_callback() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let (python, address) = fake::spawn_fake().await;
    let facts = PythonOrq::new(address, SECRET.into(), hangar_server::proxy::client())
        .with_paths(home.clone(), dir.path().join("pair"));
    assert_eq!(facts.phase("g1").await, OrqPhase::NotStarted);
    run(&home, "one", "g1", false);
    assert_eq!(facts.phase("g1").await, OrqPhase::Live);
    run(&home, "one", "g1", true);
    assert_eq!(facts.phase("g1").await, OrqPhase::Ended);
    std::fs::write(runs::root(&home).join("one/orq.json"), "{").unwrap();
    assert_eq!(facts.phase("g1").await, OrqPhase::NotStarted);
    run(&home, "one", "g1", true);
    let events = runs::root(&home).join("one/eventos.jsonl");
    std::fs::remove_file(&events).unwrap();
    std::fs::create_dir(events).unwrap();
    assert_eq!(facts.phase("g1").await, OrqPhase::Unknown);
    assert!(python.internal_bodies("orq/group-phase").is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn promotion_moves_the_exact_draft_and_marks_its_source() {
    let dir = tempfile::tempdir().unwrap();
    let srv = server(dir.path(), 1).await;
    let home = dir.path().join("home");
    identity_file(&home, "s0");
    let draft = context::draft(&home, "s0", "unused").unwrap();
    std::fs::create_dir_all(&srv.pair).unwrap();
    let source = srv.pair.join(format!("regras-{draft}.md"));
    let content = "# Time\n\n| Papel | Nome | Agente | Modelo |\n| --- | --- | --- | --- |\n| Executor | x* | codex | c |\n";
    std::fs::write(&source, content).unwrap();
    let (status, body) = call(
        srv.addr,
        "POST",
        "s0/pair",
        Some(&json!({"orq":true})),
        OWNER,
    )
    .await;
    assert_eq!(status, 200, "{body}");
    assert!(body["warning"].is_null());
    let gid = body["gid"].as_str().unwrap();
    assert_eq!(
        std::fs::read_to_string(srv.pair.join(format!("regras-{gid}.md"))).unwrap(),
        content
    );
    assert_eq!(
        std::fs::read_to_string(source).unwrap(),
        format!("<!-- hangar-orq-group: {gid} -->\n")
    );
    assert_eq!(
        context::promote(&home, &srv.pair, "s0", gid, Some(-1.0), "unused").unwrap_err(),
        "o arquivo mudou desde a leitura — recarregue"
    );
    assert_eq!(
        std::fs::read_to_string(srv.pair.join(format!("regras-{gid}.md"))).unwrap(),
        content
    );
    assert!(srv.python.internal_bodies("orq/promote").is_empty());
    let facts = PythonOrq::new(srv.addr, SECRET.into(), hangar_server::proxy::client())
        .with_paths(home, srv.pair.clone());
    assert!(
        matches!(facts.promote("s0","draft-x").await,Err(PromoteError::Conflict(s)) if s == "grupo de destino inválido")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn conflict_and_actual_disk_failure_restore_the_new_group() {
    for disk in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let srv = server(dir.path(), 2).await;
        let home = dir.path().join("home");
        identity_file(&home, "s0");
        let draft = context::draft(&home, "s0", "unused").unwrap();
        std::fs::create_dir_all(&srv.pair).unwrap();
        let source = srv.pair.join(format!("regras-{draft}.md"));
        if disk {
            std::fs::create_dir(&source).unwrap();
        } else {
            std::fs::write(&source, "<!-- hangar-orq-group: other -->\n").unwrap();
        }
        let (status, body) = call(
            srv.addr,
            "POST",
            "s0/pair",
            Some(&json!({"orq":true,"peer":"s1"})),
            OWNER,
        )
        .await;
        assert_eq!(
            (status, body["detail"]["code"].clone()),
            (409, json!("erro_orq_arquivo_mudou")),
            "{body}"
        );
        if !disk {
            assert_eq!(body["detail"]["msg"], "o time já pertence a outro grupo");
        }
        assert!(srv.groups.link("s0").await.unwrap().is_none());
        assert!(srv.groups.link("s1").await.unwrap().is_none());
        assert!(source.exists());
        assert!(srv.python.internal_bodies("orq/promote").is_empty());
    }
}

struct IncompleteRestoration(PathBuf);
impl OrqFacts for IncompleteRestoration {
    fn phase<'a>(&'a self, _gid: &'a str) -> BoxFuture<'a, OrqPhase> {
        Box::pin(async { OrqPhase::Ended })
    }
    fn promote<'a>(
        &'a self,
        _name: &'a str,
        _gid: &'a str,
    ) -> BoxFuture<'a, Result<(), PromoteError>> {
        Box::pin(async move {
            // A falha real ocorre na restauração: o temporário tornou-se um diretório.
            std::fs::create_dir(self.0.join("s1.json.tmp")).unwrap();
            Err(PromoteError::Conflict("disco cheio".into()))
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn promotion_rollback_names_each_unrestored_member_in_409_and_diary() {
    let dir = tempfile::tempdir().unwrap();
    let pair = dir.path().join("home/.claude/.hangar-pair");
    let srv = server_with_orq(dir.path(), 2, Some(Arc::new(IncompleteRestoration(pair)))).await;
    let (status, body) = call(
        srv.addr,
        "POST",
        "s0/pair",
        Some(&json!({"orq":true,"peer":"s1"})),
        OWNER,
    )
    .await;
    assert_eq!(
        (status, body["detail"]["code"].clone()),
        (409, json!("erro_orq_arquivo_mudou")),
        "{body}"
    );
    assert_eq!(body["detail"]["params"]["nao_sairam"], json!(["s1"]));
    assert!(srv.groups.link("s0").await.unwrap().is_none());
    assert!(srv.groups.link("s1").await.unwrap().is_none());
    assert!(
        srv.pair.join("s1.json.tmp").is_dir(),
        "a limpeza parcial ainda precisa ser informada"
    );
    let python = srv.python.clone();
    fake::wait_until(move || {
        python
            .diag()
            .iter()
            .any(|d| d["evento"] == "rust.groups_promotion_undo_failed" && d["sessao"] == "s1")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn orchestrator_identity_comes_from_a_live_run_not_the_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let (python, address) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(address, ""));
    state.accounts = hangar_server::accounts::AccountService::new(
        hangar_server::accounts::environment::AccountEnvironment::from_map(
            [
                ("HOME".into(), home.to_string_lossy().into_owned()),
                ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
            ]
            .into(),
        ),
    );
    let names = ["a", "g1-orq", "b", "unrelated-orq"].map(str::to_owned);
    assert!(
        hangar_server::groups::orq::is_orchestrator(&state, &names)
            .await
            .unwrap()
            .is_empty()
    );
    run(&home, "one", "g1", false);
    std::fs::write(
        runs::root(&home).join("one/vigia.json"),
        json!({"ts":chrono::Utc::now().to_rfc3339(),"pid":std::process::id()}).to_string(),
    )
    .unwrap();
    assert_eq!(
        hangar_server::groups::orq::is_orchestrator(&state, &names)
            .await
            .unwrap(),
        ["g1-orq"]
    );
    run(&home, "one", "g1", true);
    assert!(
        hangar_server::groups::orq::is_orchestrator(&state, &names)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(python.internal_bodies("orq/is-orchestrator").is_empty());
}

#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn promotion_uses_the_configured_mux_to_resolve_the_live_identity() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join("home");
    let pair = dir.path().join("pair");
    let mux = dir.path().join("custom-mux");
    crate::write_executable(
        &mux,
        format!(
            "#!/bin/sh\n[ \"$1\" = display-message ] || exit 2\nprintf '{}\\t$7\\t123456\\n'\n",
            std::process::id()
        ),
    );
    std::fs::create_dir_all(&pair).unwrap();
    let draft = context::draft(&home, "fixture-mux", mux.to_str().unwrap()).unwrap();
    std::fs::write(pair.join(format!("regras-{draft}.md")), "time local\n").unwrap();
    let (python, address) = fake::spawn_fake().await;
    let facts = PythonOrq::new(address, SECRET.into(), hangar_server::proxy::client())
        .with_paths(home, pair.clone())
        .with_mux(mux.to_string_lossy().into_owned());
    assert_eq!(facts.promote("fixture-mux", "g1").await, Ok(()));
    assert_eq!(
        std::fs::read_to_string(pair.join("regras-g1.md")).unwrap(),
        "time local\n"
    );
    assert!(python.internal_bodies("orq/promote").is_empty());
}
