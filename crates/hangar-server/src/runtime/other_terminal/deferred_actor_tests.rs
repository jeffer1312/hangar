use super::*;
struct BusyIo;
impl OtherIo for BusyIo {
    fn prove<'a>(&'a self, _: &'a OtherTarget) -> BoxFuture<'a, Result<bool, RuntimeError>> { Box::pin(async { Ok(true) }) }
    fn capture<'a>(&'a self) -> BoxFuture<'a, Result<OtherFrame, RuntimeError>> {
        Box::pin(async { Ok(OtherFrame::from_styled("────────────────────────────────\nrascunho\n────────────────────────────────\n".into())) })
    }
    fn write<'a>(&'a self, _: &'a OtherTarget, _: &'a str, _: bool, _: Option<&'a OtherWriteGuard>) -> BoxFuture<'a, Result<(), RuntimeError>> {
        Box::pin(async { panic!("rascunho ocupado não autoriza escrita") })
    }
    fn release<'a>(&'a self) -> BoxFuture<'a, ()> { Box::pin(async {}) }
}
#[tokio::test]
async fn other_deferred_actor_reports_thresholds_from_real_drains() {
    let root = tempfile::tempdir().unwrap();
    let diary = tempfile::tempdir().unwrap();
    let diag = crate::diag::DiagClient::new("127.0.0.1:9".parse().unwrap(), String::new())
        .with_diary(Some(diary.path().to_owned()));
    let registry = Arc::new(super::super::gateway::RuntimeRegistry::new(
        "127.0.0.1:9".parse().unwrap(), String::new(), "test".into()).with_diagnostics(diag.clone()));
    let mut target = tests::target();
    target.provider = hangar_transcript::Provider::Kimi;
    target.transcript = root.path().join("conversation.jsonl");
    std::fs::write(&target.transcript, b"").unwrap();
    target.state_path = root.path().join("state.json");
    target.lease_path = root.path().join("state.lock");
    target.projection_dir = root.path().join("queue");
    let store = super::super::queue::Store::open(&target.state_path, &target.projection_dir,
        super::super::queue::State::new(&target.key, 1, &target.name, vec![])).unwrap();
    let queue = QueueActor::start(store, super::super::queue::acquire_lease(&target.lease_path).unwrap());
    let mut options = OtherOptions::new(crate::terminal_control::TerminalPool::new(),
        crate::provider_models::inbox::Store::new(), Arc::new(crate::list::procs::SystemProcs::default()),
        Arc::new(crate::interactions::InteractionStore::default()));
    options.registry = Some(Arc::downgrade(&registry));
    options.io_override = Some(Arc::new(BusyIo));
    options.tick = Duration::from_secs(3600);
    let handle = OtherHandle::spawn(target, queue, options, tokio::sync::watch::channel(None).0);
    let mut frames = handle.last_frame();
    tokio::time::timeout(Duration::from_secs(3), async {
        while frames.borrow().is_none() { frames.changed().await.unwrap(); }
    }).await.unwrap();
    handle.queue("append".into(), Action::Append {text:"continuar".into(), delivered:false,
        ts:None, pre_transcript:false, entry_id:Some("entry".into())}).await.unwrap();
    for _ in 0..10 { handle.drain().await.unwrap(); }
    handle.stop().await.unwrap();
    diag.flush().await;
    let rows = crate::diag::test_support::rows(diary.path());
    let counts: Vec<_> = rows.iter().filter(|row| row["codigo"] == "composer_busy")
        .map(|row| row["count"].as_u64().unwrap()).collect();
    assert_eq!(counts, vec![1, 6, 10]);
}
