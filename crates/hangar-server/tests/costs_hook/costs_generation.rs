use hangar_server::costs::index::{Fold, Index, IndexError, Progress};
use hangar_server::costs::rows::FoldOutput;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Default, Serialize, Deserialize)]
struct Empty;
impl Fold for Empty {
    fn line(&mut self, raw: &[u8]) { assert!(raw != b"panic\n", "falha sintética"); }
    fn close(&mut self) -> FoldOutput { FoldOutput { costs: vec![], usage: vec![], areas: None } }
}
fn fold(_: &Path) -> Empty { Empty }
#[test]
fn generation_is_shared_by_clones_and_changes_only_after_committed_mutations() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("file.jsonl"); std::fs::write(&path, "{}\n").unwrap();
    let index = Index::open(&d.path().join("index")).unwrap();
    let clone = index.clone();
    let before = index.generation();
    index.sync_file(&path, &fold, "empty:1", "scope", "", &|_| vec![]).unwrap();
    assert!(index.generation() > before);
    assert_eq!(index.generation(), clone.generation());
    let before = index.generation();
    assert!(!index.sync("scope", &[path.clone()], &fold, "empty:1", "", &|_| vec![], &Progress::default()).unwrap());
    assert_eq!(index.generation(), before);
    std::fs::remove_file(&path).unwrap();
    assert!(clone.forget_outside(&[]).unwrap());
    assert!(index.generation() > before);
}
#[test]
fn partial_success_before_reader_panic_advances_generation() {
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let good = d.path().join("good.jsonl"); let bad = d.path().join("bad.jsonl");
    std::fs::write(&good, "{}\n").unwrap(); std::fs::write(&bad, "panic\n").unwrap();
    let index = Index::open(&d.path().join("index")).unwrap();
    let before = index.generation();
    assert!(matches!(index.sync("scope", &[good, bad], &fold, "empty:1", "", &|_| vec![], &Progress::default()), Err(IndexError::ReaderPanic)));
    assert!(index.generation() > before, "o commit parcial invalida mesmo com retorno de erro");
}

#[derive(Default, Serialize, Deserialize)]
struct WithAreas;
impl Fold for WithAreas {
    fn line(&mut self, _: &[u8]) {}
    fn close(&mut self) -> FoldOutput {
        use hangar_server::costs::areas::{AreaEntries, AreaHeader};
        FoldOutput { costs: vec![], usage: vec![], areas: Some(AreaEntries {
            header: AreaHeader { fonte: None, session_id: None, subagente: None }, turns: vec![],
        }) }
    }
}
#[test]
fn area_rebuild_ownership_and_forgetting_all_advance_generation() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("file.jsonl"); std::fs::write(&path, "{}\n").unwrap();
    let index = Index::open(&d.path().join("index")).unwrap();
    let new = |_: &Path| WithAreas;
    let progress = Progress::default();
    index.sync("first", &[path.clone()], &new, "area:1", "1", &|_| vec![], &progress).unwrap();
    let before = index.generation();
    let never_read = |_: &Path| -> WithAreas { panic!("não deve reler"); };
    index.sync("first", &[path.clone()], &never_read, "area:1", "2", &|_| vec![], &progress).unwrap();
    assert!(index.generation() > before);
    let before = index.generation();
    index.sync("second", &[path.clone()], &never_read, "area:1", "2", &|_| vec![], &progress).unwrap();
    assert!(index.generation() > before, "a troca de escopo em autocommit invalida");
    let before = index.generation();
    std::fs::remove_file(&path).unwrap();
    index.sync("second", &[], &new, "area:1", "2", &|_| vec![], &progress).unwrap();
    assert!(index.generation() > before);
}

#[test]
fn failed_area_write_rolls_back_without_changing_generation() {
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("file.jsonl"); std::fs::write(&path, "{}\n").unwrap();
    let index = Index::open(&d.path().join("index")).unwrap();
    let before = index.generation();
    let new = |_: &Path| WithAreas;
    let fail = |_: &hangar_server::costs::rows::AreaEntries| -> Vec<hangar_server::costs::rows::UsoLinha> { panic!("falha sintética"); };
    assert!(index.sync_file(&path, &new, "area:1", "scope", "1", &fail).is_none());
    assert_eq!(index.generation(), before);
}

#[test]
fn committed_batch_generation_is_visible_before_later_reader_panic() {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let good = d.path().join("good.jsonl"); let bad = d.path().join("bad.jsonl");
    std::fs::write(&good, "{}\n").unwrap(); std::fs::write(&bad, "panic\n").unwrap();
    let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap());
    let index = Index::open(&d.path().join("index")).unwrap().with_pool(pool);
    let before = index.generation();
    let gate = (Mutex::new(false), Condvar::new());
    std::thread::scope(|threads| {
        let scan = threads.spawn(|| {
            let new = |path: &Path| {
                if path == bad {
                    let locked = gate.0.lock().unwrap();
                    let _ = gate.1.wait_timeout_while(locked, Duration::from_secs(4), |released| !*released).unwrap();
                }
                Empty
            };
            index.sync("scope", &[good, bad.clone()], &new, "empty:1", "", &|_| vec![], &Progress::default())
        });
        let start = Instant::now();
        while index.generation() == before && start.elapsed() < Duration::from_secs(3) { std::thread::yield_now(); }
        let observed = index.generation();
        *gate.0.lock().unwrap() = true; gate.1.notify_all();
        assert!(matches!(scan.join().unwrap(), Err(IndexError::ReaderPanic)));
        assert!(observed > before, "a geração avança logo após o commit do lote bom");
        assert!(index.generation() >= observed);
    });
}

#[test]
fn rebuilding_the_schema_invalidates_rows_even_when_reading_only() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("index");
    let index = Index::open(&dir).unwrap();
    let before = index.generation();
    let conn = rusqlite::Connection::open(dir.join(hangar_server::costs::index::FILE_NAME)).unwrap();
    conn.execute("UPDATE meta SET v='999' WHERE k='esquema'", []).unwrap();
    drop(conn);
    assert!(index.read_costs(None, None, None).unwrap().is_empty());
    assert!(index.generation() > before, "a recriação do esquema também confirma uma mudança");
}

#[test]
fn emptied_database_after_open_invalidates_the_generation_for_every_clone() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("index");
    let path = d.path().join("file.jsonl");
    std::fs::write(&path, "{}\n").unwrap();
    let index = Index::open(&dir).unwrap();
    index.try_sync_file(&path, &fold, "v1", "scope", "", &|_| vec![]).unwrap();
    let clone = index.clone();
    let before = clone.generation();
    std::fs::write(dir.join(hangar_server::costs::index::FILE_NAME), []).unwrap();
    assert!(index.read_costs(None, None, None).unwrap().is_empty());
    assert!(clone.generation() > before);
    assert_eq!(clone.generation(), index.generation());
}

#[test]
fn corruption_after_a_partial_commit_repeats_the_complete_scan_and_invalidates_reports() {
    use std::io::{Seek, SeekFrom, Write};
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::{Duration, Instant};
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("index");
    let good = d.path().join("good.jsonl");
    let later = d.path().join("later.jsonl");
    std::fs::write(&good, "{}\n").unwrap();
    std::fs::write(&later, "{}\n").unwrap();
    let index = Index::open(&dir).unwrap().with_pool(Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()));
    let clone = index.clone();
    let before = clone.generation();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let progress = Progress::default();
    std::thread::scope(|threads| {
        let scan = threads.spawn(|| {
            let new = |path: &Path| {
                if path == later && calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    gate.lock().unwrap().recv_timeout(Duration::from_secs(4)).unwrap();
                }
                Cost
            };
            index.sync("scope", &[good, later.clone()], &new, "v1", "", &|_| vec![], &progress)
        });
        let start = Instant::now();
        while clone.generation() == before && start.elapsed() < Duration::from_secs(3) { std::thread::yield_now(); }
        let partial = clone.generation();
        assert!(partial > before);
        let conn = rusqlite::Connection::open(dir.join(hangar_server::costs::index::FILE_NAME)).unwrap();
        assert_eq!(conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        let page: i64 = conn.query_row("SELECT rootpage FROM sqlite_master WHERE name='custo'", [], |r| r.get(0)).unwrap();
        let size: i64 = conn.pragma_query_value(None, "page_size", |r| r.get(0)).unwrap();
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
        drop(conn);
        let mut file = std::fs::OpenOptions::new().write(true).open(dir.join(hangar_server::costs::index::FILE_NAME)).unwrap();
        file.seek(SeekFrom::Start(((page - 1) * size) as u64)).unwrap();
        file.write_all(&[0xff]).unwrap();
        file.sync_all().unwrap();
        drop(file);
        let conn = rusqlite::Connection::open(dir.join(hangar_server::costs::index::FILE_NAME)).unwrap();
        conn.execute("UPDATE meta SET v=v WHERE k='esquema'", []).unwrap();
        let error = conn.execute_batch("SELECT * FROM custo").unwrap_err();
        assert_eq!(error.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseCorrupt));
        drop(conn);
        release.send(()).unwrap();
        assert!(scan.join().unwrap().unwrap());
        assert!(clone.generation() > partial);
    });
    assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
    assert_eq!(progress.total(), (2, 2));
    let conn = rusqlite::Connection::open(dir.join(hangar_server::costs::index::FILE_NAME)).unwrap();
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0)).unwrap(), 2);
    assert_eq!(conn.query_row("PRAGMA quick_check", [], |r| r.get::<_, String>(0)).unwrap(), "ok");
}

#[derive(Serialize, Deserialize)]
struct Cost;
impl Fold for Cost {
    fn line(&mut self, _: &[u8]) {}
    fn close(&mut self) -> FoldOutput {
        use hangar_server::costs::{py::LocalTs, rows::UsageRow};
        FoldOutput { costs: vec![UsageRow {
            ts: LocalTs::from_iso("2026-09-30T12:00:00Z").unwrap(), source: "t".into(),
            provider: "".into(), model: "m".into(), project: "".into(), session_id: "s".into(),
            input: 1, output: 0, cache_write: 0, cache_read: 0, subagente: false,
            account_id: None, codex_long_context: false, cache_write_1h: 0, fast: false,
            regravado: 0, regravado_1h: 0,
        }], usage: vec![], areas: None }
    }
}
