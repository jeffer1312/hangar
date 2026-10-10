use crate::common;
use common::costs::fixtures_copy;
use hangar_server::costs::collect::*;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, atomic::{AtomicUsize, Ordering}};
use std::time::{Duration, Instant};

struct Fixed {
    value: Mutex<Result<Scopes, ()>>,
    calls: AtomicUsize,
    active: AtomicUsize,
    peak: AtomicUsize,
    delay: Mutex<Duration>,
    signal: (Mutex<(usize, bool)>, Condvar),
    first_call: Mutex<Option<Instant>>,
}
impl Fixed {
    fn new(value: Result<Scopes, ()>) -> Arc<Self> {
        Arc::new(Self { value: Mutex::new(value), calls: AtomicUsize::new(0),
            active: AtomicUsize::new(0), peak: AtomicUsize::new(0), delay: Mutex::new(Duration::ZERO),
            signal: (Mutex::new((0, false)), Condvar::new()), first_call: Mutex::new(None) })
    }
    fn hold(&self) { self.signal.0.lock().unwrap().1 = true; }
    fn release(&self) { self.signal.0.lock().unwrap().1 = false; self.signal.1.notify_all(); }
    fn wait_calls(&self, calls: usize, timeout: Duration) {
        let signal = self.signal.0.lock().unwrap();
        let (signal, _) = self.signal.1.wait_timeout_while(signal, timeout, |s| s.0 < calls).unwrap();
        assert!(signal.0 >= calls, "a fonte não foi consultada no prazo");
    }
}
impl ScopeSource for Fixed {
    fn fetch(&self) -> Result<Scopes, CollectError> {
        assert_eq!(std::thread::current().name(), Some("custos-scan"));
        assert!(tokio::runtime::Handle::try_current().is_err(), "a varredura roda fora do executor Tokio");
        self.first_call.lock().unwrap().get_or_insert_with(Instant::now);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(active, Ordering::SeqCst);
        let mut signal = self.signal.0.lock().unwrap();
        signal.0 += 1; self.signal.1.notify_all();
        let (signal, _) = self.signal.1.wait_timeout_while(signal, Duration::from_secs(10), |s| s.1).unwrap();
        drop(signal);
        std::thread::sleep(*self.delay.lock().unwrap());
        self.active.fetch_sub(1, Ordering::SeqCst);
        self.value.lock().unwrap().clone().map_err(|_| CollectError::NoScopes)
    }
}
fn scopes(base: &Path) -> Scopes {
    Scopes {
        claude: vec![ClaudeScope { root: base.join("claude/projects"), account: "anthropic:u-fixture".into(), label: "fixture@exemplo".into() }],
        codex: vec![CodexScope { home: base.join("codex"), account: format!("codex:{}", base.join("codex").display()), label: "Codex · default".into() }],
        pi: vec![PiScope { root: base.join("pi"), source: "pi".into() }],
        kimi: Some(KimiScope { root: base.join("kimi/sessions"), index: base.join("kimi/session_index.jsonl") }),
        repo: base.to_path_buf(),
    }
}
fn collector(base: &Path, source: Arc<dyn ScopeSource>) -> Arc<Collector> {
    Arc::new(Collector::new(base.join("../idx"), base.join("pricing"), base.join("../sem-mapa.json"), source))
}
/// Só flagra a varredura que não termina: no runner Windows o disco faz uma varredura das amostras
/// passar de 5 s de vez em quando.
const SCAN_WAIT: Duration = Duration::from_secs(30);
fn wait_ready(c: &Arc<Collector>) {
    let start = Instant::now();
    while start.elapsed() < SCAN_WAIT {
        if matches!(c.prepare(false).unwrap(), Ready::Go) { return; }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("a varredura não terminou");
}
/// Pedido fresco até a varredura terminar com sucesso. O `prepare(true)` espera no máximo `FRESH_WAIT` (3 s) e
/// depois devolve `Warming`; no runner Windows a varredura às vezes passa disso, e o pedido seguinte espera a
/// mesma varredura, sem abrir outra.
fn fresh_go(c: &Arc<Collector>) {
    let start = Instant::now();
    while start.elapsed() < SCAN_WAIT {
        if matches!(c.prepare(true).unwrap(), Ready::Go) { return; }
    }
    panic!("a varredura fresca não terminou");
}
fn wait_failed(c: &Arc<Collector>) {
    let start = Instant::now();
    while start.elapsed() < SCAN_WAIT {
        if c.prepare(false).is_err() { return; }
        std::thread::yield_now();
    }
    panic!("a varredura não informou a falha");
}
#[test]
fn first_request_warms_and_reads_sources_accounts_and_usage() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base)));
    let c = collector(&base, source.clone());
    assert!(matches!(c.prepare(false).unwrap(), Ready::Warming { .. }));
    wait_ready(&c);
    let rows = c.read_costs(None).unwrap();
    for source in ["claude", "codex", "pi", "kimi"] {
        assert!(rows.iter().any(|r| r.source == source), "{source}");
    }
    assert!(rows.iter().filter(|r| r.source == "kimi").all(|r| r.project == "/repo/k"));
    assert!(rows.iter().filter(|r| r.source == "claude").all(|r| r.account_id.is_none()));
    assert!(rows.iter().filter(|r| r.source == "claude" && r.model == "gpt-5.6-sol").all(|r| r.provider == "openai"));
    assert!(rows.iter().filter(|r| r.source == "claude" && r.model.starts_with("claude-")).all(|r| r.provider == "anthropic:u-fixture"));
    assert!(rows.iter().filter(|r| r.source == "codex").all(|r| r.account_id.as_deref() == Some(scopes(&base).codex[0].account.as_str())));
    assert_eq!(c.label("anthropic:u-fixture").as_deref(), Some("fixture@exemplo"));
    assert_eq!(c.repo(), Some(base.clone()));
    let (usage, tokens) = c.read_usage(None).unwrap();
    assert!(!usage.is_empty());
    assert!(tokens.iter().all(|r| r.source == "claude" && r.account_id.as_deref() == Some("anthropic:u-fixture")));
    assert!(c.read_costs(Some("2099-01-01")).unwrap().is_empty());
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
}
#[test]
fn usage_direct_append_preserves_scope_order_accounts_and_every_row_field() {
    use hangar_server::costs::index::Fold;
    use hangar_server::costs::rows::{FoldOutput, UsoLinha};
    #[derive(serde::Serialize, serde::Deserialize)]
    struct UsageOnly { row: UsoLinha }
    impl Fold for UsageOnly {
        fn line(&mut self, _: &[u8]) {}
        fn close(&mut self) -> FoldOutput {
            FoldOutput { costs: vec![], usage: vec![self.row.clone()], areas: None }
        }
    }
    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("base");
    std::fs::create_dir(&base).unwrap();
    let claude = vec![
        ClaudeScope { root: base.join("z"), account: "anthropic:z".into(), label: "z".into() },
        ClaudeScope { root: base.join("a"), account: "anthropic:a".into(), label: "a".into() },
    ];
    let codex = vec![
        CodexScope { home: base.join("cz"), account: "codex:z".into(), label: "z".into() },
        CodexScope { home: base.join("ca"), account: "codex:a".into(), label: "a".into() },
    ];
    for scope in &codex {
        std::fs::create_dir_all(scope.home.join("sessions")).unwrap();
        std::fs::write(scope.home.join("sessions/rollout-fixture.jsonl"), "{}\n").unwrap();
    }
    let source = Fixed::new(Ok(Scopes { claude: claude.clone(), codex: codex.clone(),
        pi: vec![], kimi: None, repo: base.clone() }));
    let c = collector(&base, source);
    c.prepare(false).unwrap();
    wait_ready(&c);
    let keys = vec![format!("claude:{}", claude[0].root.display()),
        format!("claude:{}", claude[1].root.display()), codex[0].account.clone(), codex[1].account.clone()];
    let accounts = ["anthropic:z", "anthropic:a", "codex:z", "codex:a"];
    let mut expected = Vec::new();
    for (n, key) in keys.iter().enumerate() {
        let row = UsoLinha {
            dia: "2026-10-01".into(), cwd: "projeto".into(), model: "modelo".into(),
            tipo: "ferramenta".into(), nome: format!("ação-{n}"), plugin: "extensão".into(),
            detalhe: "detalhe".into(), origem: "origem".into(), chamadas: 1, ctx_chars: 2,
            tokens_est: 3, input: 4, output: 5, cache_write: 6, cache_read: 7,
            cache_write_1h: 8, fast: true, ocupados: 9, respostas: 10, ocupados_eq: 11,
            fonte: "fonte".into(), subagente: true, session_id: "sessão".into(),
        };
        let path = base.join(format!("usage-{n}.jsonl"));
        std::fs::write(&path, "{}\n").unwrap();
        c.index().unwrap().try_sync_file(&path, &|_| UsageOnly { row: row.clone() },
            "usage:1", key, "", &|_| vec![]).unwrap();
        expected.push((row, accounts[n].to_owned()));
    }
    let (usage, tokens) = c.read_usage(Some("2026-10-01")).unwrap();
    assert_eq!(usage, expected);
    assert!(tokens.is_empty());
    let prefix = (UsoLinha { nome: "prefixo".into(), ..UsoLinha::default() }, "prévia".to_owned());
    let streamed = c.fold_usage(Some("2026-10-01"), |tokens, _| {
        assert!(tokens.is_empty());
        vec![prefix.clone()]
    }, &mut |rows: &mut Vec<(UsoLinha, String)>, row, account, _| rows.push((row.clone(), account.to_owned()))).unwrap();
    assert_eq!(streamed, std::iter::once(prefix).chain(expected).collect::<Vec<_>>());
    let empty = c.fold_usage(Some("2026-10-02"), |_, _| Vec::new(),
        &mut |rows: &mut Vec<(UsoLinha, String)>, row, account, _| rows.push((row.clone(), account.to_owned()))).unwrap();
    assert!(empty.is_empty());
    assert!(c.read_usage(Some("2026-10-02")).unwrap().0.is_empty());
}

#[test]
fn streamed_usage_report_matches_materialized_rows_with_exact_order_floats_and_filters() {
    use hangar_server::costs::{py::LocalTs, report_uso::{build, UsoBuilder, UsoFilters}};
    let (_d, base) = fixtures_copy();
    let c = collector(&base, Fixed::new(Ok(scopes(&base))));
    c.prepare(false).unwrap();
    wait_ready(&c);
    let now = LocalTs::from_iso("2026-10-03T02:59:59Z").unwrap();
    let origins = indexmap::IndexMap::from([("brainstorming".to_owned(), "superpowers".to_owned())]);
    for filters in [UsoFilters::default(),
        UsoFilters { conta: vec!["anthropic:u-fixture".into()], ..Default::default() },
        UsoFilters { foco: Some("conversa".into()), ..Default::default() },
        UsoFilters { plugin: vec!["superpowers".into()], ..Default::default() }] {
        for (period, since) in [("all", None), ("1d", Some("2026-10-02"))] {
            let (rows, tokens) = c.read_usage(since).unwrap();
            let expected = build(&rows, &tokens, period, now,
                &filters, Some(&origins), &c.pricing(), &|account| c.label(account));
            let builder = c.fold_usage(since,
                |tokens, pricing| UsoBuilder::new(tokens, period, now, &filters, Some(&origins), pricing),
                &mut |builder: &mut UsoBuilder<'_>, row, account, pricing| builder.push(row, account, pricing)).unwrap();
            let got = builder.finish(&|account| c.label(account));
            assert_eq!(serde_json::to_string(&got).unwrap(), serde_json::to_string(&expected).unwrap(), "{period}");
        }
    }
}

#[test]
fn missing_scopes_propagate_and_last_good_scopes_survive_failure() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Err(()));
    let c = collector(&base, source.clone());
    c.schedule_warmup(Duration::ZERO);
    wait_failed(&c);
    assert!(matches!(c.prepare(true), Err(CollectError::NoScopes)));
    *source.value.lock().unwrap() = Ok(scopes(&base));
    std::thread::sleep(Duration::from_millis(15));
    fresh_go(&c);
    let before = c.read_costs(None).unwrap();
    *source.value.lock().unwrap() = Err(());
    std::thread::sleep(Duration::from_millis(15));
    fresh_go(&c);
    assert_eq!(before, c.read_costs(None).unwrap());
}
#[test]
fn concurrent_fresh_requests_share_exactly_one_scan_and_sequential_fresh_scans_again() {
    for delay in [Duration::ZERO, Duration::from_millis(60)] {
        let (_d, base) = fixtures_copy();
        let source = Fixed::new(Ok(scopes(&base)));
        let c = collector(&base, source.clone());
        c.prepare(false).unwrap();
        wait_ready(&c);
        *source.delay.lock().unwrap() = delay;
        let before = source.calls.load(Ordering::SeqCst);
        source.hold();
        let first = { let c = c.clone(); std::thread::spawn(move || c.prepare(true).unwrap()) };
        source.wait_calls(before + 1, Duration::from_secs(2));
        let threads: Vec<_> = (0..7).map(|_| {
            let c = c.clone();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
                rt.block_on(async { c.prepare(true).unwrap() })
            })
        }).collect();
        for thread in threads { assert!(matches!(thread.join().unwrap(), Ready::Warming { .. })); }
        source.release();
        assert!(matches!(first.join().unwrap(), Ready::Go));
        assert_eq!(source.calls.load(Ordering::SeqCst) - before, 1, "todos compartilham a mesma varredura");
        assert_eq!(source.peak.load(Ordering::SeqCst), 1);
        fresh_go(&c);
        assert_eq!(source.calls.load(Ordering::SeqCst) - before, 2, "fresh posterior à conclusão pede outra coleta");
    }
}
#[test]
fn kimi_index_and_label_changes_invalidate_without_touching_wire() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base)));
    let c = collector(&base, source.clone());
    c.prepare(false).unwrap(); wait_ready(&c);
    let before = c.data_version();
    let index = base.join("kimi/session_index.jsonl");
    let text = std::fs::read_to_string(&index).unwrap().replace("/repo/k", "/repo/novo");
    std::fs::write(index, text).unwrap();
    assert!(c.data_version() > before);
    assert!(c.read_costs(None).unwrap().iter().filter(|r| r.source == "kimi").all(|r| r.project == "/repo/novo"));
    let before = c.data_version();
    source.value.lock().unwrap().as_mut().unwrap().claude[0].label = "Novo rótulo".into();
    std::thread::sleep(Duration::from_millis(15));
    c.prepare(true).unwrap();
    assert!(c.data_version() > before);
    assert_eq!(c.label("anthropic:u-fixture").as_deref(), Some("Novo rótulo"));
}
#[test]
fn single_rollout_write_invalidates_collector_version() {
    let (_d, base) = fixtures_copy();
    let c = collector(&base, Fixed::new(Ok(scopes(&base))));
    c.prepare(false).unwrap(); wait_ready(&c);
    let before = c.data_version();
    let path = base.join("single-rollout.jsonl");
    std::fs::write(&path, "{}\n").unwrap();
    let index = c.index().unwrap();
    index.sync_file(&path, &hangar_server::costs::codex::new_fold,
        hangar_server::costs::codex::VERSION, "codex:avulso", c.areas().signature(), &|a| c.areas().area_lines(a)).unwrap();
    assert!(c.data_version() > before);
}
#[cfg(unix)]
#[test]
fn rollout_owners_use_canonical_sessions_roots_and_reject_ambiguity() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a"); let b = d.path().join("b");
    std::fs::create_dir_all(a.join("sessions/x")).unwrap();
    std::fs::create_dir_all(b.join("sessions")).unwrap();
    std::fs::create_dir_all(a.join("other")).unwrap();
    std::fs::write(a.join("sessions/x/rollout-1.jsonl"), "{}\n").unwrap();
    std::fs::write(a.join("other/rollout-outside.jsonl"), "{}\n").unwrap();
    std::os::unix::fs::symlink(a.join("sessions/x"), b.join("sessions/x")).unwrap();
    std::os::unix::fs::symlink(a.join("other/rollout-outside.jsonl"), b.join("sessions/rollout-outside.jsonl")).unwrap();
    let mk = |home: &Path, account: &str| CodexScope { home: home.to_owned(), account: account.into(), label: String::new() };
    let owners = rollout_owners(&[mk(&a, "a"), mk(&b, "b")]);
    assert_eq!(owners.values().map(|(_, f)| f.len()).sum::<usize>(), 1);
    assert!(owners.contains_key("a"));
    assert!(rollout_owners(&[mk(&a, "a"), mk(&a, "alias")]).is_empty());
}
#[test]
fn source_panic_is_an_error_and_can_recover() {
    struct Broken(Arc<Fixed>);
    impl ScopeSource for Broken {
        fn fetch(&self) -> Result<Scopes, CollectError> {
            if self.0.calls.load(Ordering::SeqCst) == 0 {
                self.0.calls.fetch_add(1, Ordering::SeqCst);
                panic!("falha sintética");
            }
            self.0.fetch()
        }
    }
    hangar_server::install_panic_hook();
    let (_d, base) = fixtures_copy();
    let c = collector(&base, Arc::new(Broken(Fixed::new(Ok(scopes(&base))))));
    c.prepare(false).unwrap();
    wait_failed(&c);
    assert!(c.read_costs(None).is_err());
    fresh_go(&c);
}

#[test]
fn cold_concurrent_requests_return_immediately_and_share_the_warmup() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base))); source.hold();
    let c = collector(&base, source.clone());
    let threads: Vec<_> = (0..8).map(|_| {
        let c = c.clone(); std::thread::spawn(move || c.prepare(false).unwrap())
    }).collect();
    for thread in threads { assert!(matches!(thread.join().unwrap(), Ready::Warming { .. })); }
    source.wait_calls(1, Duration::from_secs(2));
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    assert_eq!(source.peak.load(Ordering::SeqCst), 1);
    source.release(); wait_ready(&c);
}

#[test]
fn fresh_wait_is_bounded_and_does_not_block_the_async_executor() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base)));
    let c = collector(&base, source.clone()); c.prepare(false).unwrap(); wait_ready(&c);
    source.hold();
    let start = Instant::now();
    let first = { let c = c.clone(); std::thread::spawn(move || c.prepare(true).unwrap()) };
    source.wait_calls(2, Duration::from_secs(2));
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let quick = Instant::now();
        assert!(matches!(c.prepare(true).unwrap(), Ready::Warming { .. }));
        assert!(quick.elapsed() < Duration::from_millis(100));
        tokio::time::timeout(Duration::from_millis(100), tokio::task::yield_now()).await.unwrap();
    });
    assert!(matches!(first.join().unwrap(), Ready::Warming { .. }));
    assert!(start.elapsed() >= Duration::from_secs(3));
    assert!(start.elapsed() < Duration::from_secs(4));
    source.release(); c.prepare(true).unwrap();
}

#[test]
fn stale_data_refreshes_behind_the_reader_after_thirty_seconds() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base)));
    let c = collector(&base, source.clone()); c.prepare(false).unwrap(); wait_ready(&c);
    let rows = c.read_costs(None).unwrap();
    assert!(matches!(c.prepare(false).unwrap(), Ready::Go));
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    std::thread::sleep(Duration::from_secs(31));
    source.hold();
    assert!(matches!(c.prepare(false).unwrap(), Ready::Go));
    source.wait_calls(2, Duration::from_secs(2));
    assert_eq!(c.read_costs(None).unwrap(), rows);
    source.release(); fresh_go(&c);
}

#[test]
fn boot_warmup_waits_thirty_seconds_and_keeps_one_scan() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base)));
    let c = collector(&base, source.clone());
    let scheduled = Instant::now();
    c.schedule_warmup(Duration::from_secs(30));
    c.schedule_warmup(Duration::from_secs(30));
    // A hora da consulta, e não a contagem aos 29 s: no runner o sono do próprio teste atrasa.
    source.wait_calls(1, Duration::from_secs(40));
    let waited = source.first_call.lock().unwrap().unwrap() - scheduled;
    assert!(waited >= Duration::from_secs(30), "a varredura começou aos {waited:?}");
    wait_ready(&c);
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
}

#[test]
fn warming_progress_sums_all_active_sources() {
    let (_d, base) = fixtures_copy();
    let c = collector(&base, Fixed::new(Ok(scopes(&base))));
    c.index().unwrap();
    let conn = rusqlite::Connection::open(base.join("../idx").join(hangar_server::costs::index::FILE_NAME)).unwrap();
    conn.execute_batch("BEGIN IMMEDIATE").unwrap();
    let total = list_files(&base.join("claude/projects"), |n| n.ends_with(".jsonl")).len()
        + rollout_owners(&scopes(&base).codex).values().map(|(_, f)| f.len()).sum::<usize>()
        + list_files(&base.join("pi"), |n| n.ends_with(".jsonl")).len()
        + list_files(&base.join("kimi/sessions"), |n| n == "wire.jsonl").len();
    c.prepare(false).unwrap();
    let start = Instant::now();
    loop {
        if let Ready::Warming { read, total: observed } = c.prepare(false).unwrap() {
            if observed == total { assert!(read <= total); break; }
        }
        assert!(start.elapsed() < Duration::from_secs(2));
        std::thread::yield_now();
    }
    assert_eq!(c.label("anthropic:u-fixture").as_deref(), Some("fixture@exemplo"));
    conn.execute_batch("ROLLBACK").unwrap();
    wait_ready(&c);
}

#[test]
fn http_scopes_use_internal_secret_and_reject_invalid_http_or_json() {
    use std::io::{Read, Write};
    for status in [200, 401, 302] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let d = tempfile::tempdir().unwrap();
        let expected = scopes(d.path()); let payload = serde_json::to_vec(&expected).unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = Vec::new(); let mut byte = [0; 1];
            while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
            let request = String::from_utf8(request).unwrap().to_lowercase();
            assert!(request.starts_with("get /internal/costs/scopes http/1.1"));
            assert!(request.contains("x-hangar-internal: synthetic-secret\r\n"));
            write!(stream, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", payload.len()).unwrap();
            stream.write_all(&payload).unwrap();
        });
        let result = HttpScopes::new(address, "synthetic-secret".into()).fetch();
        if status == 200 { assert_eq!(result.unwrap(), expected); }
        else { assert!(matches!(result, Err(CollectError::NoScopes))); }
        server.join().unwrap();
    }
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0; 2048]; stream.read(&mut request).unwrap();
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
    });
    assert!(matches!(HttpScopes::new(address, "synthetic-secret".into()).fetch(), Err(CollectError::NoScopes)));
    server.join().unwrap();
}

#[test]
fn empty_kimi_project_falls_back_and_removed_scope_is_not_read() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base)));
    let c = collector(&base, source.clone()); c.prepare(false).unwrap(); wait_ready(&c);
    std::fs::write(base.join("kimi/session_index.jsonl"), "").unwrap();
    assert!(c.read_costs(None).unwrap().iter().filter(|r| r.source == "kimi").all(|r| r.project == "desconhecido"));
    let before = c.data_version();
    let mut next = scopes(&base); next.claude.clear(); next.codex.clear(); next.pi.clear(); next.kimi = None;
    *source.value.lock().unwrap() = Ok(next);
    fresh_go(&c);
    assert!(c.read_costs(None).unwrap().is_empty());
    assert!(c.data_version() > before);
    assert!(c.labels_key().is_empty());
}

#[test]
fn writer_timestamp_panic_marks_collector_failed_and_recovers_after_partial_commit() {
    hangar_server::install_panic_hook();
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base)));
    let c = collector(&base, source);
    c.prepare(false).unwrap(); wait_ready(&c);
    let before = c.data_version();
    let rollout = list_files(&base.join("codex/sessions"), |n| n.starts_with("rollout-") && n.ends_with(".jsonl")).remove(0);
    common::costs::append(&rollout, b"{}\n");
    let wire = base.join("kimi/sessions/wd_x/session_k1/agents/main/wire.jsonl");
    let saved = std::fs::read(&wire).unwrap();
    common::costs::append(&wire, b"{\"type\":\"usage.record\",\"time\":1e308,\"model\":\"apikey/k3\",\"usage\":{\"inputOther\":1}}\n");
    assert!(matches!(c.prepare(true), Err(CollectError::Index(hangar_server::costs::index::IndexError::ReaderPanic))));
    assert!(c.read_costs(None).is_err(), "o erro impede servir uma coleta parcial como sucesso");
    let partial = c.data_version();
    assert!(partial > before, "o commit Codex anterior ao pânico invalida a versão");
    std::fs::write(wire, saved).unwrap();
    fresh_go(&c);
    assert!(c.data_version() >= partial);
    assert!(!c.read_costs(None).unwrap().is_empty());
}

#[test]
fn labels_preserve_scope_insertion_order_and_pricing_is_reloaded_on_prepare() {
    let (_d, base) = fixtures_copy();
    let mut all = scopes(&base);
    let mut second = all.claude[0].clone(); second.root = base.join("missing"); second.account = "anthropic:a".into();
    all.claude.push(second);
    let c = collector(&base, Fixed::new(Ok(all)));
    c.prepare(false).unwrap(); wait_ready(&c);
    let labels = c.labels_key();
    assert_eq!(labels[0].0, "anthropic:u-fixture");
    assert_eq!(labels[1].0, "anthropic:a");
    let before = c.pricing().generation();
    let overrides = base.join("pricing/overrides.json");
    let original = std::fs::metadata(&overrides).unwrap().modified().unwrap();
    std::fs::write(&overrides, "{}").unwrap();
    std::fs::File::options().write(true).open(overrides).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original + Duration::from_secs(2))).unwrap();
    c.prepare(false).unwrap();
    assert!(c.pricing().generation() > before);
    let _pricing = c.pricing();
    assert!(c.data_version() > 0, "a versão não tenta adquirir Pricing novamente");
}

#[test]
fn constructor_and_metadata_do_not_create_index_before_explicit_use() {
    let d = tempfile::tempdir().unwrap();
    let index_dir = d.path().join("index");
    let c = Arc::new(Collector::new(index_dir.clone(), d.path().join("pricing"), d.path().join("map.json"),
        Fixed::new(Ok(scopes(d.path())))));
    assert!(!index_dir.exists());
    assert_eq!(c.data_version(), 0);
    assert!(c.label("missing").is_none());
    assert!(c.labels_key().is_empty());
    assert!(c.repo().is_none());
    assert!(!index_dir.exists(), "ler metadados não abre o banco incidentalmente");
    c.prepare(false).unwrap(); wait_ready(&c);
    assert!(index_dir.join(hangar_server::costs::index::FILE_NAME).exists());
}

#[test]
fn http_scopes_timeout_covers_the_response_body() {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (release, finish) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = Vec::new(); let mut byte = [0; 1];
        while !request.ends_with(b"\r\n\r\n") { stream.read_exact(&mut byte).unwrap(); request.push(byte[0]); }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n").unwrap();
        let _ = finish.recv_timeout(Duration::from_secs(12));
    });
    let start = Instant::now();
    let result = HttpScopes::new(address, "synthetic-secret".into()).fetch();
    let elapsed = start.elapsed();
    release.send(()).unwrap(); server.join().unwrap();
    assert!(matches!(result, Err(CollectError::NoScopes)));
    assert!(elapsed >= Duration::from_secs(10));
    assert!(elapsed < Duration::from_secs(11), "o prazo inclui o corpo, não só a conexão");
}

#[test]
fn failed_lazy_index_open_can_recover_on_the_next_scan() {
    let (_d, base) = fixtures_copy();
    let dir = base.join("../idx"); std::fs::write(&dir, "bloqueado").unwrap();
    let c = collector(&base, Fixed::new(Ok(scopes(&base))));
    c.prepare(false).unwrap(); wait_failed(&c);
    assert!(matches!(c.read_costs(None), Err(CollectError::Index(hangar_server::costs::index::IndexError::NoDisk))));
    std::fs::remove_file(dir).unwrap();
    fresh_go(&c);
    assert!(!c.read_costs(None).unwrap().is_empty());
}

#[test]
fn fresh_blocking_inside_tokio_waits_while_async_prepare_stays_responsive() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Ok(scopes(&base)));
    let c = collector(&base, source.clone()); c.prepare(false).unwrap(); wait_ready(&c);
    source.hold();
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    rt.block_on(async {
        let mut fresh = {
            let c = c.clone();
            tokio::task::spawn_blocking(move || c.prepare_blocking(true))
        };
        {
            let source = source.clone();
            tokio::task::spawn_blocking(move || source.wait_calls(2, Duration::from_secs(2))).await.unwrap();
        }
        let quick = Instant::now();
        assert!(matches!(c.prepare(true).unwrap(), Ready::Warming { .. }));
        assert!(quick.elapsed() < Duration::from_millis(100));
        let waiting = tokio::time::timeout(Duration::from_millis(40), &mut fresh).await.is_err();
        source.release();
        assert!(waiting, "spawn_blocking deve esperar a varredura em vez de responder Warming");
        assert!(matches!(fresh.await.unwrap().unwrap(), Ready::Go));
        assert_eq!(source.calls.load(Ordering::SeqCst), 2, "ambos compartilham a varredura");
    });
}

#[test]
fn persistent_failure_does_not_start_a_scan_per_request() {
    let (_d, base) = fixtures_copy();
    let source = Fixed::new(Err(()));
    let c = collector(&base, source.clone());
    c.schedule_warmup(Duration::ZERO);
    wait_failed(&c);
    let after_first = source.calls.load(Ordering::SeqCst);
    // Pedidos em rajada (a tela inicial e a de Custos abertas) dentro do intervalo: só a falha guardada.
    for _ in 0..20 {
        assert!(matches!(c.prepare(false), Err(CollectError::NoScopes)));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(source.calls.load(Ordering::SeqCst), after_first);
}

/// Pasta sem permissão de leitura; `None` quando o processo roda como root (o chmod não vale).
#[cfg(unix)]
struct Restore(std::path::PathBuf);
#[cfg(unix)]
impl Drop for Restore {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
    }
}
#[cfg(unix)]
fn lock_dir(dir: &Path) -> Option<Restore> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let restore = Restore(dir.to_path_buf());
    std::fs::read_dir(dir).is_err().then_some(restore)
}

#[cfg(unix)]
#[test]
fn unreadable_folder_keeps_its_rows_and_reports_the_cause() {
    let (_d, base) = fixtures_copy();
    let c = collector(&base, Fixed::new(Ok(scopes(&base))));
    c.schedule_warmup(Duration::ZERO);
    wait_ready(&c);
    let before = c.read_costs(None).unwrap();
    assert!(c.unread_issue().is_none());
    // Uma pasta de projeto do Claude e a pasta de dias do Codex: nas duas, ler falha, nada sumiu.
    let Some(claude) = lock_dir(&base.join("claude/projects/-repo-a")) else { return };
    let Some(codex) = lock_dir(&base.join("codex/sessions/2026")) else { return };
    fresh_go(&c);
    assert_eq!(c.read_costs(None).unwrap(), before);
    assert_eq!(c.unread_issue().as_deref(), Some("costs_dir_permission_denied"));
    drop((claude, codex));
    fresh_go(&c);
    assert_eq!(c.read_costs(None).unwrap(), before);
    assert!(c.unread_issue().is_none());
}

#[cfg(unix)]
#[test]
fn codex_folder_listed_but_not_searchable_keeps_its_rows() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, base) = fixtures_copy();
    let c = collector(&base, Fixed::new(Ok(scopes(&base))));
    c.schedule_warmup(Duration::ZERO);
    wait_ready(&c);
    let before = c.read_costs(None).unwrap();
    // `r--`: a listagem lê os nomes, mas resolver cada rollout falha com permissão negada.
    let day = base.join("codex/sessions/2026/09/30");
    std::fs::set_permissions(&day, std::fs::Permissions::from_mode(0o444)).unwrap();
    let restore = Restore(day.clone());
    if std::fs::canonicalize(day.join("rollout-c1.jsonl")).is_ok() { return; }
    fresh_go(&c);
    assert_eq!(c.read_costs(None).unwrap(), before);
    assert_eq!(c.unread_issue().as_deref(), Some("costs_dir_permission_denied"));
    drop(restore);
}
