use hangar_server::costs::index::{Fold, Index, IndexError, Progress};
use hangar_server::costs::py::LocalTs;
use hangar_server::costs::rows::{AreaEntries, FoldOutput, UsageRow, UsoLinha};
use hangar_server::costs::areas::{AreaHeader, Unit};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Serialize, Deserialize, Default)]
struct Sum {
    n: i64,
    lines: i64,
}

impl Fold for Sum {
    fn line(&mut self, raw: &[u8]) {
        if let Ok(n) = std::str::from_utf8(raw).unwrap_or("").trim().parse::<i64>() {
            self.n += n;
            self.lines += 1;
        }
    }

    fn close(&mut self) -> FoldOutput {
        let row = UsageRow {
            ts: LocalTs::from_iso("2026-09-30T12:00:00Z").unwrap(),
            source: "t".into(), provider: "".into(), model: "m".into(), project: "".into(),
            session_id: "s".into(), input: self.n, output: self.lines, cache_write: 0,
            cache_read: 0, subagente: false, account_id: None, codex_long_context: false,
            cache_write_1h: 0, fast: false, regravado: 0, regravado_1h: 0,
        };
        // Fechar pode consumir o acumulador: esse valor não deve entrar na retomada.
        self.n = -999;
        FoldOutput { costs: vec![row], usage: vec![], areas: None }
    }
}

fn new_sum(_: &Path) -> Sum { Sum::default() }
fn no_areas(_: &AreaEntries) -> Vec<UsoLinha> { vec![] }

#[test]
fn existing_index_orders_files_by_path_after_insertion_and_update() {
    for reverse in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.jsonl");
        let b = dir.path().join("b.jsonl");
        std::fs::write(&a, "2\n").unwrap();
        std::fs::write(&b, "5\n").unwrap();
        let index = Index::open(&dir.path().join("index")).unwrap();
        for path in if reverse { [&b, &a] } else { [&a, &b] } {
            index.try_sync_file(path, &new_areas, "v1", "scope", "sig", &area_rows).unwrap();
        }
        let assert_order = || {
            assert_eq!(index.read_costs(Some("scope"), None, None).unwrap().iter().map(|row| row.input).collect::<Vec<_>>(), [2, 5]);
            let usage = index.read_usage("scope", None).unwrap();
            assert_eq!(usage.iter().map(|row| row.input).collect::<Vec<_>>(), [14, 2, 14, 5]);
            let values = index.fold_usage("scope", None, Vec::new(), &mut |values, row| values.push(row.input)).unwrap();
            assert_eq!(values, [14, 2, 14, 5]);
        };
        assert_order();
        std::fs::write(&a, "3\n").unwrap();
        index.try_sync_file(&a, &new_areas, "v1", "scope", "sig", &area_rows).unwrap();
        assert_eq!(index.read_costs(Some("scope"), None, None).unwrap().iter().map(|row| row.input).collect::<Vec<_>>(), [3, 5]);
        assert_eq!(index.read_usage("scope", None).unwrap().iter().map(|row| row.input).collect::<Vec<_>>(), [14, 3, 14, 5]);
    }
}

#[test]
fn default_directory_uses_synthetic_local_cache_without_changing_environment() {
    use hangar_server::costs::index::default_dir_from;
    let d = tempfile::tempdir().unwrap();
    let home = d.path().join("home");
    let cache = d.path().join("cache");
    let fallback = if cfg!(windows) { home.join("AppData").join("Local") } else { home.join(".cache") }.join("hangar").join("custos");
    assert_eq!(default_dir_from(&home, None), fallback);
    assert_eq!(default_dir_from(&home, Some("".as_ref())), fallback);
    assert_eq!(default_dir_from(&home, Some(cache.as_os_str())), cache.join("hangar").join("custos"));
    if !cfg!(windows) { assert_eq!(default_dir_from(&home, Some("relative".as_ref())), fallback); }
}

fn corrupt_table(dir: &Path, table: &str) {
    use std::io::{Seek, SeekFrom};
    let conn = database(dir);
    let page: i64 = conn.query_row("SELECT rootpage FROM sqlite_master WHERE name=?", [table], |r| r.get(0)).unwrap();
    let size: i64 = conn.pragma_query_value(None, "page_size", |r| r.get(0)).unwrap();
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    drop(conn);
    let mut file = std::fs::OpenOptions::new().write(true).open(dir.join(hangar_server::costs::index::FILE_NAME)).unwrap();
    file.seek(SeekFrom::Start(((page - 1) * size) as u64)).unwrap();
    file.write_all(&[0xff]).unwrap();
    file.sync_all().unwrap();
    let conn = database(dir);
    assert_eq!(conn.query_row("SELECT v FROM meta WHERE k='esquema'", [], |r| r.get::<_, String>(0)).unwrap(), "1");
    let error = conn.execute_batch(&format!("SELECT * FROM {table}")).unwrap_err();
    assert_eq!(error.sqlite_error_code(), Some(rusqlite::ErrorCode::DatabaseCorrupt));
}

#[test]
fn corrupt_data_page_rebuilds_the_complete_list_and_invalidates_clones() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let a = d.path().join("a.jsonl");
    let b = d.path().join("b.jsonl");
    std::fs::write(&a, "2\n").unwrap();
    std::fs::write(&b, "3\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    let python = dir.join("custos.sqlite3");
    std::fs::write(&python, b"cache Python preservado").unwrap();
    sync(&ix, &[a.clone(), b.clone()]);
    let clone = ix.clone();
    let before = clone.generation();
    corrupt_table(&dir, "files");
    assert!(sync(&ix, &[a, b]));
    assert_eq!(sum(&clone), (5, 2));
    assert!(clone.generation() > before);
    assert_eq!(std::fs::read(python).unwrap(), b"cache Python preservado");
}

#[test]
fn corrupt_data_pages_are_recovered_by_reads_and_forgetting() {
    for operation in ["costs", "usage", "forget"] {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("idx");
        let f = d.path().join("a.jsonl");
        std::fs::write(&f, "2\n").unwrap();
        let ix = Index::open(&dir).unwrap();
        if operation == "usage" {
            ix.sync_file(&f, &new_areas, "v1", "t", "sig", &area_rows).unwrap();
        } else {
            sync(&ix, &[f]);
        }
        let before = ix.generation();
        corrupt_table(&dir, match operation { "costs" => "custo", "usage" => "uso", _ => "files" });
        match operation {
            "costs" => assert!(ix.read_costs(None, None, None).unwrap().is_empty()),
            "usage" => assert!(ix.read_usage("t", None).unwrap().is_empty()),
            _ => { ix.forget_outside(&[]).unwrap(); },
        }
        assert!(ix.generation() > before, "operação: {operation}");
    }
}

#[test]
fn single_file_corruption_after_open_retries_only_once() {
    for persistent in [false, true] {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join("idx");
        let f = d.path().join("a.jsonl");
        std::fs::write(&f, "2\n").unwrap();
        let ix = Index::open(&dir).unwrap();
        let calls = AtomicUsize::new(0);
        let new = |_: &Path| {
            let attempt = calls.fetch_add(1, Ordering::SeqCst);
            if attempt == 0 || persistent { corrupt_table(&dir, "custo"); }
            Sum::default()
        };
        let result = ix.sync_file(&f, &new, "v1", "t", "sig", &no_areas);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(result.is_some(), !persistent);
        if !persistent { assert_eq!(sum(&ix), (2, 1)); }
    }
}

#[test]
fn reentrant_corruption_defers_recovery_without_blocking_the_outer_operation() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    let calls = AtomicUsize::new(0);
    let new = |_: &Path| {
        if calls.fetch_add(1, Ordering::SeqCst) == 0 {
            corrupt_table(&dir, "custo");
            assert!(matches!(ix.read_costs(None, None, None), Err(IndexError::Sqlite(ref error)) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseCorrupt)));
            assert!(ix.try_sync_file(&f, &new_sum, "v1", "t", "sig", &no_areas).is_err());
            assert!(ix.sync_file(&f, &new_sum, "v1", "t", "sig", &no_areas).is_none());
        }
        Sum::default()
    };
    ix.try_sync_file(&f, &new, "v1", "t", "sig", &no_areas).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(sum(&ix), (2, 1));
}

#[test]
fn pending_recovery_does_not_turn_a_reader_panic_into_success() {
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    let before = ix.generation();
    let calls = AtomicUsize::new(0);
    let new = |_: &Path| -> Sum {
        calls.fetch_add(1, Ordering::SeqCst);
        corrupt_table(&dir, "custo");
        assert!(ix.read_costs(None, None, None).is_err());
        panic!("falha sintética após corrupção")
    };
    assert!(matches!(ix.try_sync_file(&f, &new, "v1", "t", "sig", &no_areas), Err(IndexError::ReaderPanic)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(ix.generation() > before);
    assert!(ix.read_costs(None, None, None).unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn reader_panic_survives_failed_rebuild_and_pending_reads_fail_until_repaired() {
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    let clone = ix.clone();
    let before = ix.generation();
    let calls = AtomicUsize::new(0);
    let database = dir.join(hangar_server::costs::index::FILE_NAME);
    let new = |_: &Path| -> Sum {
        calls.fetch_add(1, Ordering::SeqCst);
        corrupt_table(&dir, "custo");
        assert!(ix.read_costs(None, None, None).is_err());
        // A conexão conserva o inode aberto; a pasta impede remove_file sem mudar permissões.
        std::fs::rename(&database, dir.join("blocked.sqlite3")).unwrap();
        std::fs::create_dir(&database).unwrap();
        panic!("falha sintética com reconstrução impedida")
    };
    assert!(matches!(ix.try_sync_file(&f, &new, "v1", "t", "sig", &no_areas), Err(IndexError::ReaderPanic)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(clone.generation() > before);
    assert!(database.is_dir());
    assert!(matches!(clone.read_costs(None, None, None), Err(IndexError::NoDisk)));
    assert!(matches!(clone.read_usage("t", None), Err(IndexError::NoDisk)));
    std::fs::remove_dir(&database).unwrap();
    assert!(clone.read_costs(None, None, None).unwrap().is_empty());
    clone.try_sync_file(&f, &new_sum, "v1", "t", "sig", &no_areas).unwrap();
    assert_eq!(sum(&ix), (2, 1));
}

#[test]
fn checked_single_file_propagates_the_second_real_corruption() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    let calls = AtomicUsize::new(0);
    let new = |_: &Path| {
        calls.fetch_add(1, Ordering::SeqCst);
        corrupt_table(&dir, "custo");
        Sum::default()
    };
    assert!(matches!(ix.try_sync_file(&f, &new, "v1", "t", "sig", &no_areas), Err(IndexError::Sqlite(ref error)) if error.sqlite_error_code() == Some(rusqlite::ErrorCode::DatabaseCorrupt)));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn concurrent_corruption_keeps_the_open_connection_then_recovers_on_drain() {
    use std::sync::{Mutex, mpsc};
    use std::time::Duration;
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    let clone = ix.clone();
    let before = clone.generation();
    let (entered, waiting) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let gate = Mutex::new(gate);
    let calls = AtomicUsize::new(0);
    std::thread::scope(|threads| {
        let scan = threads.spawn(|| {
            let new = |_: &Path| {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    entered.send(()).unwrap();
                    gate.lock().unwrap().recv_timeout(Duration::from_secs(4)).unwrap();
                }
                Sum::default()
            };
            ix.try_sync_file(&f, &new, "v1", "t", "sig", &no_areas)
        });
        waiting.recv_timeout(Duration::from_secs(2)).unwrap();
        corrupt_table(&dir, "custo");
        assert!(clone.read_costs(None, None, None).is_err());
        assert!(clone.generation() > before, "a invalidação não espera a drenagem");
        let conn = database(&dir);
        assert!(conn.execute_batch("SELECT * FROM custo").is_err(), "a conexão ativa impede a remoção prematura");
        drop(conn);
        release.send(()).unwrap();
        scan.join().unwrap().unwrap();
    });
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(sum(&clone), (2, 1));
}

#[test]
fn integrity_failure_preserves_database_and_sidecars() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    sync(&ix, &[f.clone()]);
    let conn = database(&dir);
    conn.execute_batch("CREATE TRIGGER reject_cost BEFORE INSERT ON custo BEGIN SELECT RAISE(ABORT, 'falha sintética'); END").unwrap();
    let before = ix.generation();
    append(&f, b"3\n");
    let error = ix.sync("t", &[f], &new_sum, "v1", "sig", &no_areas, &Progress::default()).unwrap_err();
    assert!(matches!(error, IndexError::Sqlite(ref e) if e.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation)));
    for suffix in ["", "-wal", "-shm"] { assert!(dir.join(format!("{}{}", hangar_server::costs::index::FILE_NAME, suffix)).exists()); }
    assert_eq!(ix.generation(), before);
    assert_eq!(sum(&ix), (2, 1));
    assert_eq!(conn.query_row("SELECT COUNT(*) FROM sqlite_master WHERE name='reject_cost'", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
}

fn sync(ix: &Index, files: &[PathBuf]) -> bool {
    ix.sync("t", files, &new_sum, "v1", "sig", &no_areas, &Progress::default()).unwrap()
}

fn sum(ix: &Index) -> (i64, i64) {
    ix.read_costs(Some("t"), None, None).unwrap().iter()
        .fold((0, 0), |a, r| (a.0 + r.input, a.1 + r.output))
}

fn append(path: &Path, raw: &[u8]) {
    std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(raw).unwrap();
}

#[test]
fn append_reads_only_new_bytes_and_matches_full_read() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "1\n2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let starts = AtomicUsize::new(0);
    let new = |_: &Path| { starts.fetch_add(1, Ordering::Relaxed); Sum::default() };
    let progress = Progress::default();
    assert!(ix.sync("t", &[f.clone()], &new, "v1", "sig", &no_areas, &progress).unwrap());
    append(&f, b"3\n");
    assert!(ix.sync("t", &[f.clone()], &new, "v1", "sig", &no_areas, &progress).unwrap());
    assert_eq!(starts.load(Ordering::Relaxed), 1, "retoma sem criar outra dobra");
    assert_eq!(sum(&ix), (6, 3));
    let full = Index::open(&d.path().join("full")).unwrap();
    sync(&full, &[f.clone()]);
    assert_eq!(ix.read_costs(None, None, None).unwrap(), full.read_costs(None, None, None).unwrap());
    assert!(!sync(&ix, &[f]), "sem mudança não grava");
    assert_eq!(progress.total(), (1, 1));
}

#[test]
fn line_without_newline_counts_once_and_is_not_saved_in_state() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "1\n2").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    sync(&ix, &[f.clone()]);
    assert_eq!(sum(&ix), (3, 2));
    append(&f, b"0\n");
    sync(&ix, &[f.clone()]);
    assert_eq!(sum(&ix), (21, 2), "a linha incompleta 2 virou 20");
    append(&f, b"4");
    sync(&ix, &[f.clone()]);
    assert_eq!(sum(&ix), (25, 3));
    append(&f, b"\n");
    sync(&ix, &[f]);
    assert_eq!(sum(&ix), (25, 3));
}

#[test]
fn shrunk_replaced_or_rewritten_file_is_reread_from_zero() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "5\n5\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    sync(&ix, &[f.clone()]);
    std::fs::write(&f, "1\n").unwrap();
    sync(&ix, &[f.clone()]);
    assert_eq!(sum(&ix), (1, 1));
    std::fs::write(&f, "9\n1\n7\n").unwrap();
    sync(&ix, &[f.clone()]);
    assert_eq!(sum(&ix), (17, 3));
    let g = d.path().join("b.jsonl");
    std::fs::write(&g, "4\n").unwrap();
    std::fs::rename(&g, &f).unwrap();
    sync(&ix, &[f]);
    assert_eq!(sum(&ix), (4, 1));
}

#[test]
fn vanished_file_leaves_the_scope_and_other_version_rereads() {
    let d = tempfile::tempdir().unwrap();
    let (a, b) = (d.path().join("a.jsonl"), d.path().join("b.jsonl"));
    std::fs::write(&a, "1\n").unwrap();
    std::fs::write(&b, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    sync(&ix, &[a.clone(), b.clone()]);
    std::fs::remove_file(&b).unwrap();
    assert!(sync(&ix, &[a.clone(), b.clone()]));
    assert_eq!(sum(&ix), (1, 1));
    append(&a, b"3\n");
    let new = |_: &Path| Sum { n: 100, lines: 0 };
    assert!(ix.sync("t", &[a.clone()], &new, "v2", "sig", &no_areas, &Progress::default()).unwrap());
    assert_eq!(sum(&ix), (104, 2), "a versão descarta o estado anterior");
    assert!(!ix.sync("t", &[a], &new, "v2", "sig", &no_areas, &Progress::default()).unwrap());
}

#[test]
fn unreadable_database_is_rebuilt() {
    let d = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(d.path().join("idx")).unwrap();
    std::fs::write(d.path().join("idx").join(hangar_server::costs::index::FILE_NAME), b"invalid_sqlite").unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    sync(&ix, &[f]);
    assert_eq!(sum(&ix), (2, 1));
}

#[test]
fn unavailable_disk_is_an_error_instead_of_an_in_memory_index() {
    let d = tempfile::tempdir().unwrap();
    let blocked = d.path().join("file");
    std::fs::write(&blocked, "arquivo").unwrap();
    assert!(matches!(Index::open(&blocked.join("idx")), Err(IndexError::NoDisk)));
}

#[test]
fn single_file_preserves_owner_and_forget_only_removes_missing_inactive_files() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a.jsonl");
    let b = d.path().join("b.jsonl");
    std::fs::write(&a, "1\n").unwrap();
    std::fs::write(&b, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    sync(&ix, &[a.clone()]);
    append(&a, b"3\n");
    let id = ix.sync_file(&a, &new_sum, "v1", "avulso", "sig", &no_areas).unwrap();
    assert_eq!(sum(&ix), (4, 2));
    assert_eq!(ix.read_costs(None, None, Some(id)).unwrap()[0].input, 4);
    ix.sync_file(&b, &new_sum, "v1", "avulso", "sig", &no_areas).unwrap();
    assert!(!ix.forget_outside(&["t".into()]).unwrap());
    std::fs::remove_file(&a).unwrap();
    std::fs::remove_file(&b).unwrap();
    assert!(ix.forget_outside(&["t".into()]).unwrap());
    assert_eq!(sum(&ix), (4, 2));
    assert!(!ix.forget_outside(&["t".into()]).unwrap());
    assert!(sync(&ix, &[]));
    assert!(ix.read_costs(None, None, None).unwrap().is_empty());
    assert!(ix.sync_file(&b, &new_sum, "v1", "avulso", "sig", &no_areas).is_none());
}

fn database(dir: &Path) -> rusqlite::Connection {
    rusqlite::Connection::open(dir.join(hangar_server::costs::index::FILE_NAME)).unwrap()
}

#[test]
fn different_schema_rebuilds_its_own_database_and_preserves_the_python_database() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let ix = Index::open(&dir).unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    sync(&ix, &[f.clone()]);
    std::fs::write(dir.join("custos.sqlite3"), b"python_index").unwrap();
    database(&dir).execute("UPDATE meta SET v='999' WHERE k='esquema'", []).unwrap();
    let ix = Index::open(&dir).unwrap();
    assert!(ix.read_costs(None, None, None).unwrap().is_empty());
    assert!(sync(&ix, &[f]));
    assert_eq!(sum(&ix), (2, 1));
    assert_eq!(std::fs::read(dir.join("custos.sqlite3")).unwrap(), b"python_index");
    let conn = database(&dir);
    let mode: String = conn.query_row("PRAGMA journal_mode", [], |r| r.get(0)).unwrap();
    assert_eq!(mode, "wal");
}

#[test]
fn corrupt_state_is_reread_and_unreadable_file_keeps_old_rows() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    sync(&ix, &[f.clone()]);
    database(&dir).execute("UPDATE files SET estado=x'00'", []).unwrap();
    append(&f, b"3\n");
    sync(&ix, &[f.clone()]);
    assert_eq!(sum(&ix), (5, 2));
    std::fs::remove_file(&f).unwrap();
    std::fs::create_dir(&f).unwrap();
    assert!(!sync(&ix, &[f.clone()]));
    assert_eq!(sum(&ix), (5, 2));
    std::fs::remove_dir(&f).unwrap();
    assert!(sync(&ix, &[f]));
    assert_eq!(sum(&ix), (0, 0));
}

#[derive(Serialize, Deserialize, Default)]
struct WithAreas { sum: Sum }

fn usage_row() -> UsoLinha {
    UsoLinha {
        dia: "2026-09-30".into(), cwd: "projeto".into(), model: "modelo".into(),
        tipo: "ferramenta".into(), nome: "edição".into(), plugin: "extensão".into(),
        detalhe: "ação".into(), origem: "origem".into(), chamadas: 11, ctx_chars: 12,
        tokens_est: 13, input: 14, output: 15, cache_write: 16, cache_read: 17,
        cache_write_1h: 18, fast: true, ocupados: 19, respostas: 20, ocupados_eq: 21,
        fonte: "fonte".into(), subagente: true, session_id: "sessão".into(),
    }
}

impl Fold for WithAreas {
    fn line(&mut self, raw: &[u8]) { self.sum.line(raw); }
    fn close(&mut self) -> FoldOutput {
        let mut output = self.sum.close();
        let cost = &mut output.costs[0];
        cost.source = "fonte".into();
        cost.provider = "provedor".into();
        cost.model = "modelo".into();
        cost.project = "projeto".into();
        cost.session_id = "sessão".into();
        cost.cache_write = 11;
        cost.cache_read = 12;
        cost.subagente = true;
        cost.account_id = Some("conta".into());
        cost.codex_long_context = true;
        cost.cache_write_1h = 13;
        cost.fast = true;
        cost.regravado = 14;
        cost.regravado_1h = 15;
        output.usage = vec![usage_row()];
        output.areas = Some(AreaEntries {
            header: AreaHeader { fonte: None, session_id: None, subagente: None },
            turns: vec![(vec![], vec![Unit {
                dia: "2026-09-30".into(), cwd: "projeto".into(), model: "modelo".into(),
                fast: false, values: [cost.input, 0, 0, 0, 0],
            }])],
        });
        output
    }
}

fn new_areas(_: &Path) -> WithAreas { WithAreas::default() }

fn area_rows(areas: &AreaEntries) -> Vec<UsoLinha> {
    vec![UsoLinha { tipo: "area".into(), input: areas.turns[0].1[0].values[0], ..usage_row() }]
}

#[test]
fn rows_roundtrip_filters_and_area_changes_do_not_reread_the_transcript() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let progress = Progress::default();
    ix.sync("t", &[f.clone()], &new_areas, "v1", "sig1", &area_rows, &progress).unwrap();
    let mut expected = WithAreas::default();
    expected.line(b"2\n");
    let output = expected.close();
    assert_eq!(ix.read_costs(Some("t"), Some("2026-09-30"), None).unwrap(), output.costs);
    assert!(ix.read_costs(Some("t"), Some("2026-10-01"), None).unwrap().is_empty());
    assert!(ix.read_costs(Some("outro"), None, None).unwrap().is_empty());
    assert_eq!(ix.read_usage("t", Some("2026-09-30")).unwrap(), vec![usage_row(), area_rows(&output.areas.unwrap())[0].clone()]);
    assert!(ix.read_usage("t", Some("2026-10-01")).unwrap().is_empty());
    let new = |_: &Path| -> WithAreas { panic!("não deve reler") };
    let redo = |a: &AreaEntries| {
        let mut rows = area_rows(a);
        rows[0].input *= 10;
        rows
    };
    assert!(ix.sync("t", &[f.clone()], &new, "v1", "sig2", &redo, &progress).unwrap());
    assert_eq!(ix.read_usage("t", None).unwrap()[1].input, 20);
    assert!(!ix.sync("t", &[f.clone()], &new, "v1", "sig2", &redo, &progress).unwrap());
    ix.sync("outro", &[f], &new, "v1", "sig2", &redo, &progress).unwrap();
    assert!(ix.read_costs(Some("t"), None, None).unwrap().is_empty());
    assert_eq!(ix.read_usage("outro", None).unwrap()[1].input, 20);
    assert_eq!(progress.total(), (2, 2));
    progress.reset();
    assert_eq!(progress.total(), (0, 0));
}

#[test]
fn corrupt_areas_preserve_rows_and_force_a_full_read_on_the_next_sync() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&dir).unwrap();
    let p = Progress::default();
    ix.sync("t", &[f.clone()], &new_areas, "v1", "sig1", &area_rows, &p).unwrap();
    database(&dir).execute("UPDATE files SET areas=x'00'", []).unwrap();
    assert!(ix.sync("t", &[f.clone()], &new_areas, "v1", "sig2", &area_rows, &p).unwrap());
    assert_eq!(ix.read_usage("t", None).unwrap()[1].input, 2);
    let starts = AtomicUsize::new(0);
    let new = |_: &Path| { starts.fetch_add(1, Ordering::Relaxed); WithAreas::default() };
    assert!(ix.sync("t", &[f], &new, "v1", "sig2", &area_rows, &p).unwrap());
    assert_eq!(starts.load(Ordering::Relaxed), 1);
    assert_eq!(ix.read_usage("t", None).unwrap().len(), 2);
}

#[test]
fn private_single_thread_pool_can_sync_without_initializing_or_reconfiguring_the_global_pool() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    assert!(pool.install(|| sync(&ix, &[f])));
    assert_eq!(sum(&ix), (2, 1));
}

#[test]
fn bounded_scan_inside_its_single_worker_pool_finishes_multiple_windows() {
    use std::sync::{Arc, mpsc};
    use std::time::Duration;
    let (done, result) = mpsc::channel();
    let scan = std::thread::spawn(move || {
        let d = tempfile::tempdir().unwrap();
        let files = (1..=33).map(|n| {
            let path = d.path().join(format!("{n:02}.jsonl"));
            std::fs::write(&path, format!("{n}\n")).unwrap();
            path
        }).collect::<Vec<_>>();
        let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap());
        let ix = Index::open(&d.path().join("idx")).unwrap().with_pool(Arc::clone(&pool));
        assert!(pool.install(|| sync(&ix, &files)));
        let rows = ix.read_costs(Some("t"), None, None).unwrap();
        done.send(rows.iter().map(|r| r.input).collect::<Vec<_>>()).unwrap();
    });
    // Só flagra o worker travado: o disco do runner Windows numa rodada lenta não cabe em segundos.
    assert_eq!(result.recv_timeout(Duration::from_secs(60)).expect("a janela não deve bloquear o próprio worker"),
        (1..=33).collect::<Vec<_>>());
    scan.join().unwrap();
}

#[test]
fn saved_tail_is_64_bytes_and_state_excludes_fragment_and_close_mutations() {
    use std::io::Read;
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let f = d.path().join("a.jsonl");
    let raw = format!("{}7", "1\n".repeat(100));
    std::fs::write(&f, &raw).unwrap();
    let ix = Index::open(&dir).unwrap();
    sync(&ix, &[f.clone()]);
    let conn = database(&dir);
    let (offset, size, tail, blob): (i64, i64, Vec<u8>, Vec<u8>) = conn.query_row(
        "SELECT offset, size, cauda, estado FROM files", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    ).unwrap();
    assert_eq!((offset, size), (200, 201));
    assert_eq!(tail, raw.as_bytes()[136..200]);
    let mut decoded = String::new();
    flate2::read::ZlibDecoder::new(blob.as_slice()).read_to_string(&mut decoded).unwrap();
    let state: Sum = serde_json::from_str(&decoded).unwrap();
    assert_eq!((state.n, state.lines), (100, 100));
    let changed = format!("{}9\n7\n", "1\n".repeat(99));
    std::fs::write(&f, changed).unwrap();
    sync(&ix, &[f]);
    assert_eq!(sum(&ix), (115, 101), "a cauda reescrita invalida a retomada");
}

#[cfg(unix)]
#[test]
fn another_inode_with_the_same_size_and_mtime_forces_a_full_read() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    let g = d.path().join("b.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    sync(&ix, &[f.clone()]);
    let mtime = std::fs::metadata(&f).unwrap().modified().unwrap();
    std::fs::write(&g, "2\n").unwrap();
    std::fs::File::options().write(true).open(&g).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(mtime)).unwrap();
    std::fs::rename(g, &f).unwrap();
    let new = |_: &Path| Sum { n: 100, lines: 0 };
    assert!(ix.sync("t", &[f], &new, "v1", "sig", &no_areas, &Progress::default()).unwrap());
    assert_eq!(sum(&ix), (102, 1));
}

#[cfg(unix)]
#[test]
fn stat_errors_other_than_not_found_preserve_old_rows() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    sync(&ix, &[f.clone()]);
    std::fs::remove_file(&f).unwrap();
    std::os::unix::fs::symlink(&f, &f).unwrap();
    assert!(std::fs::metadata(&f).is_err());
    assert!(!sync(&ix, &[f]));
    assert_eq!(sum(&ix), (2, 1));
}

#[derive(Deserialize, Default)]
struct FailingSerialization { sum: Sum, fail: bool }

impl Serialize for FailingSerialization {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.fail { return Err(serde::ser::Error::custom("conteúdo privado")); }
        serde_json::json!({"sum": {"n": self.sum.n, "lines": self.sum.lines}, "fail": false}).serialize(serializer)
    }
}

impl Fold for FailingSerialization {
    fn line(&mut self, raw: &[u8]) {
        if raw == b"invalid\n" { self.fail = true; }
        self.sum.line(raw);
    }
    fn close(&mut self) -> FoldOutput { self.sum.close() }
}

#[test]
fn serialization_error_keeps_rows_and_does_not_stop_other_files() {
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a.jsonl");
    let b = d.path().join("b.jsonl");
    std::fs::write(&a, "2\n").unwrap();
    std::fs::write(&b, "3\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let new = |_: &Path| FailingSerialization::default();
    let p = Progress::default();
    ix.sync("t", &[a.clone(), b.clone()], &new, "v1", "sig", &no_areas, &p).unwrap();
    append(&a, b"invalid\n");
    append(&b, b"4\n");
    assert!(ix.sync("t", &[a, b], &new, "v1", "sig", &no_areas, &p).unwrap());
    assert_eq!(sum(&ix), (9, 3));
    assert_eq!(p.total(), (2, 2));
}

#[test]
fn owner_assigned_while_single_file_is_read_wins_the_upsert() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let new = |_: &Path| {
        ix.sync("t", &[f.clone()], &new_sum, "v1", "sig", &no_areas, &Progress::default()).unwrap();
        Sum::default()
    };
    ix.sync_file(&f, &new, "v1", "avulso", "sig", &no_areas).unwrap();
    assert_eq!(sum(&ix), (2, 1));
    assert!(ix.read_costs(Some("avulso"), None, None).unwrap().is_empty());
}

#[test]
fn scan_takes_ownership_of_a_path_inserted_while_it_was_read() {
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let new = |_: &Path| {
        ix.sync_file(&f, &new_sum, "v1", "avulso", "sig", &no_areas).unwrap();
        Sum::default()
    };
    ix.sync("t", &[f.clone()], &new, "v1", "sig", &no_areas, &Progress::default()).unwrap();
    assert_eq!(sum(&ix), (2, 1));
    assert!(ix.read_costs(Some("avulso"), None, None).unwrap().is_empty());
}

#[derive(Serialize, Deserialize)]
struct BrokenResume {
    sum: Sum,
    #[serde(skip)]
    fresh: bool,
}

impl Fold for BrokenResume {
    fn line(&mut self, raw: &[u8]) {
        assert!(self.fresh, "conteúdo privado da retomada");
        self.sum.line(raw);
    }
    fn close(&mut self) -> FoldOutput { self.sum.close() }
}

#[test]
fn resumed_fold_that_panics_is_retried_from_zero() {
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let new = |_: &Path| BrokenResume { sum: Sum::default(), fresh: true };
    ix.sync_file(&f, &new, "v1", "t", "sig", &no_areas).unwrap();
    append(&f, b"3\n");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ix.sync_file(&f, &new, "v1", "t", "sig", &no_areas)
    }));
    assert!(result.is_ok(), "o pânico da dobra não pode sair do índice");
    assert!(result.unwrap().is_some());
    assert_eq!(sum(&ix), (5, 2));
}

#[derive(Serialize, Deserialize, Default)]
struct PanickingFold { sum: Sum }

impl Fold for PanickingFold {
    fn line(&mut self, raw: &[u8]) {
        assert!(raw != b"invalid\n", "conteúdo privado da conversa");
        self.sum.line(raw);
    }
    fn close(&mut self) -> FoldOutput { self.sum.close() }
}

#[test]
fn persistent_fold_panic_preserves_rows_and_other_files_finish() {
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a.jsonl");
    let b = d.path().join("b.jsonl");
    std::fs::write(&a, "2\n").unwrap();
    std::fs::write(&b, "3\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let new = |_: &Path| PanickingFold::default();
    let p = Progress::default();
    ix.sync("t", &[a.clone(), b.clone()], &new, "v1", "sig", &no_areas, &p).unwrap();
    append(&a, b"invalid\n");
    append(&b, b"4\n");
    assert!(matches!(ix.sync("t", &[a, b], &new, "v1", "sig", &no_areas, &p), Err(IndexError::ReaderPanic)));
    assert_eq!(sum(&ix), (9, 3));
    assert_eq!(p.total(), (2, 2));
}

#[test]
fn selected_pool_reads_and_calculates_areas_in_parallel_then_commits_in_file_order() {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;
    let d = tempfile::tempdir().unwrap();
    let a = d.path().join("a.jsonl");
    let b = d.path().join("b.jsonl");
    std::fs::write(&a, "1\n").unwrap();
    std::fs::write(&b, "2\n").unwrap();
    let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2)
        .thread_name(|n| format!("costs-index-test-{n}")).build().unwrap());
    let ix = Index::open(&d.path().join("idx")).unwrap().with_pool(pool);
    let signal = (Mutex::new((false, false)), Condvar::new());
    let active = AtomicUsize::new(0);
    let peak = AtomicUsize::new(0);
    let caller = std::thread::current().id();
    let area_calls = AtomicUsize::new(0);
    let new = |path: &Path| {
        assert!(std::thread::current().name().unwrap().starts_with("costs-index-test-"));
        let count = active.fetch_add(1, Ordering::SeqCst) + 1;
        peak.fetch_max(count, Ordering::SeqCst);
        let mut entered = signal.0.lock().unwrap();
        if path == a { entered.0 = true; } else { entered.1 = true; }
        signal.1.notify_all();
        let (entered, _) = signal.1.wait_timeout_while(entered, Duration::from_secs(2), |s| !s.0 || !s.1).unwrap();
        assert!(entered.0 && entered.1, "as leituras devem ocorrer simultaneamente");
        drop(entered);
        active.fetch_sub(1, Ordering::SeqCst);
        WithAreas::default()
    };
    let redo = |areas: &AreaEntries| {
        assert_ne!(std::thread::current().id(), caller, "o cálculo puro acompanha a leitura no pool");
        assert!(std::thread::current().name().unwrap().starts_with("costs-index-test-"));
        area_calls.fetch_add(1, Ordering::SeqCst);
        area_rows(areas)
    };
    assert!(ix.sync("t", &[a.clone(), b], &new, "v1", "sig", &redo, &Progress::default()).unwrap());
    assert_eq!(peak.load(Ordering::SeqCst), 2);
    assert_eq!(area_calls.load(Ordering::SeqCst), 2);
    let rows = ix.read_costs(Some("t"), None, None).unwrap();
    assert_eq!(rows.iter().map(|r| r.input).collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(ix.read_usage("t", None).unwrap().iter().filter(|row| row.tipo == "area")
        .map(|row| row.input).collect::<Vec<_>>(), vec![1, 2]);
}

#[test]
fn completed_batch_commits_while_another_file_is_still_reading() {
    use std::sync::{Arc, Mutex, mpsc};
    use std::time::Duration;
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let a = d.path().join("a.jsonl");
    let b = d.path().join("b.jsonl");
    std::fs::write(&a, "1\n").unwrap();
    std::fs::write(&b, "2\n").unwrap();
    let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap());
    let ix = Index::open(&dir).unwrap().with_pool(pool);
    let (release, blocked) = mpsc::channel();
    let blocked = Mutex::new(blocked);
    let progress = Progress::default();
    let observed = std::thread::scope(|threads| {
        let scan = threads.spawn(|| {
            let new = |path: &Path| {
                if path == b { let _ = blocked.lock().unwrap().recv_timeout(Duration::from_secs(4)); }
                Sum::default()
            };
            ix.sync("t", &[a, b.clone()], &new, "v1", "sig", &no_areas, &progress)
        });
        let conn = database(&dir);
        let start = std::time::Instant::now();
        let mut observed = false;
        while start.elapsed() < Duration::from_secs(3) {
            let count: i64 = conn.query_row("SELECT COUNT(*) FROM custo", [], |r| r.get(0)).unwrap();
            if count == 1 {
                assert_eq!(progress.total(), (1, 2));
                observed = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = release.send(());
        assert!(scan.join().unwrap().unwrap());
        observed
    });
    assert!(observed, "o primeiro lote deve estar no disco antes de a segunda leitura terminar");
    assert_eq!(sum(&ix), (3, 2));
    assert_eq!(progress.total(), (2, 2));
}

#[test]
fn slow_first_file_keeps_later_reads_bounded_and_preserves_order() {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};
    let d = tempfile::tempdir().unwrap();
    let files = (0..128).map(|n| {
        let path = d.path().join(format!("{n:03}.jsonl"));
        std::fs::write(&path, format!("{}\n", n + 1)).unwrap();
        path
    }).collect::<Vec<_>>();
    let pool = Arc::new(rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap());
    let ix = Index::open(&d.path().join("idx")).unwrap().with_pool(pool);
    let gate = (Mutex::new(false), Condvar::new());
    let entered = AtomicUsize::new(0);
    let first_started = std::sync::atomic::AtomicBool::new(false);
    let progress = Progress::default();
    let before = ix.generation();
    let bounded = std::thread::scope(|threads| {
        let scan = threads.spawn(|| {
            let new = |path: &Path| {
                entered.fetch_add(1, Ordering::SeqCst);
                if path == files[0] {
                    first_started.store(true, Ordering::SeqCst);
                    let locked = gate.0.lock().unwrap();
                    let _ = gate.1.wait_timeout_while(locked, Duration::from_secs(4), |released| !*released).unwrap();
                }
                Sum::default()
            };
            ix.sync("t", &files, &new, "v1", "sig", &no_areas, &progress)
        });
        let start = Instant::now();
        while !first_started.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(2) {
            std::thread::yield_now();
        }
        let started = first_started.load(Ordering::SeqCst);
        let start = Instant::now();
        while entered.load(Ordering::SeqCst) <= 64 && start.elapsed() < Duration::from_millis(150) {
            std::thread::yield_now();
        }
        let bounded = started && entered.load(Ordering::SeqCst) == 64;
        let empty = ix.read_costs(Some("t"), None, None).unwrap().is_empty();
        let unchanged = ix.generation() == before;
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        assert!(scan.join().unwrap().unwrap());
        assert!(empty && unchanged, "a ordem impede publicar arquivos posteriores antes do primeiro");
        bounded
    });
    assert!(bounded, "a janela deve preencher 64 leituras sem acumular todos os arquivos posteriores");
    assert_eq!(progress.total(), (128, 128));
    assert_eq!(ix.read_costs(Some("t"), None, None).unwrap().iter().map(|r| r.input).collect::<Vec<_>>(),
        (1..=128).collect::<Vec<_>>());
}

#[test]
fn dense_results_commit_before_more_files_are_read_and_allow_reentrant_reads() {
    use std::sync::Arc;
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("idx");
    let files = (0..128).map(|n| {
        let path = d.path().join(format!("{n}.jsonl"));
        std::fs::write(&path, "1\n").unwrap();
        path
    }).collect::<Vec<_>>();
    let ix = Index::open(&dir).unwrap().with_pool(Arc::new(
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()));
    let before = ix.generation();
    let read_before_finish = std::sync::atomic::AtomicBool::new(false);
    let new = |_: &Path| {
        if !read_before_finish.load(Ordering::SeqCst) && ix.generation() > before
            && !ix.read_costs(Some("t"), None, None).unwrap().is_empty() {
            read_before_finish.store(true, Ordering::SeqCst);
        }
        Dense { sum: Sum::default() }
    };
    ix.sync("t", &files, &new, "v1", "sig", &no_areas, &Progress::default()).unwrap();
    assert!(read_before_finish.load(Ordering::SeqCst),
        "um lote denso deve liberar seus resultados e publicar a geração antes de ler todos os arquivos");
    assert_eq!(ix.read_costs(Some("t"), None, None).unwrap().len(), 128 * 1024);
}

#[derive(Serialize, Deserialize)]
struct Dense { sum: Sum }

impl Fold for Dense {
    fn line(&mut self, raw: &[u8]) { self.sum.line(raw); }
    fn close(&mut self) -> FoldOutput {
        let row = self.sum.close().costs.remove(0);
        FoldOutput { costs: vec![row; 1024], usage: vec![], areas: None }
    }
}

#[test]
fn area_callback_panic_rolls_back_single_file_write_and_preserves_rows() {
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    ix.sync_file(&f, &new_areas, "v1", "t", "sig", &area_rows).unwrap();
    append(&f, b"3\n");
    let redo = |_: &AreaEntries| -> Vec<UsoLinha> { panic!("conteúdo privado das áreas") };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ix.sync_file(&f, &new_areas, "v1", "t", "sig", &redo)
    }));
    assert!(result.is_ok(), "o pânico da área não pode sair do índice");
    assert!(result.unwrap().is_none());
    assert_eq!(sum(&ix), (2, 1));
    assert_eq!(ix.read_usage("t", None).unwrap()[1].input, 2);
    ix.sync_file(&f, &new_areas, "v1", "t", "sig", &area_rows).unwrap();
    assert_eq!(sum(&ix), (5, 2));
}

#[test]
fn area_panic_in_the_read_pool_remains_reader_panic_and_preserves_committed_rows() {
    use std::sync::Arc;
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap().with_pool(Arc::new(
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()));
    let progress = Progress::default();
    ix.sync("t", &[f.clone()], &new_areas, "v1", "sig", &area_rows, &progress).unwrap();
    let before = ix.generation();
    let usage = ix.read_usage("t", None).unwrap();
    append(&f, b"3\n");
    let caller = std::thread::current().id();
    let calls = AtomicUsize::new(0);
    let fail = |_: &AreaEntries| -> Vec<UsoLinha> {
        assert_ne!(std::thread::current().id(), caller);
        calls.fetch_add(1, Ordering::SeqCst);
        panic!("falha sintética nas áreas")
    };
    let result = ix.sync("t", &[f.clone()], &new_areas, "v1", "sig", &fail, &progress);
    assert!(matches!(result, Err(IndexError::ReaderPanic)));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(ix.generation(), before);
    assert_eq!(sum(&ix), (2, 1));
    assert_eq!(ix.read_usage("t", None).unwrap(), usage);
    ix.sync("t", &[f], &new_areas, "v1", "sig", &area_rows, &progress).unwrap();
    assert_eq!(sum(&ix), (5, 2));
    assert_eq!(ix.read_usage("t", None).unwrap()[1].input, 5);
}

#[test]
fn saved_area_callback_panic_preserves_rows_and_forces_a_fresh_fold() {
    hangar_server::install_panic_hook();
    let d = tempfile::tempdir().unwrap();
    let f = d.path().join("a.jsonl");
    std::fs::write(&f, "2\n").unwrap();
    let ix = Index::open(&d.path().join("idx")).unwrap();
    let p = Progress::default();
    ix.sync("t", &[f.clone()], &new_areas, "v1", "sig1", &area_rows, &p).unwrap();
    let redo = |_: &AreaEntries| -> Vec<UsoLinha> { panic!("conteúdo privado das áreas") };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        ix.sync("t", &[f.clone()], &new_areas, "v1", "sig2", &redo, &p)
    }));
    assert!(result.is_ok(), "o pânico da área salva não pode sair do índice");
    assert!(matches!(result.unwrap(), Err(IndexError::ReaderPanic)));
    assert_eq!(ix.read_usage("t", None).unwrap()[1].input, 2);
    let starts = AtomicUsize::new(0);
    let new = |_: &Path| { starts.fetch_add(1, Ordering::Relaxed); WithAreas::default() };
    assert!(ix.sync("t", &[f], &new, "v1", "sig2", &area_rows, &p).unwrap());
    assert_eq!(starts.load(Ordering::Relaxed), 1);
}
