//! Medidas reproduzíveis dos caminhos centrais de contas e anexos. Ignoradas por padrão:
//! `cargo test -p hangar-server --release --test perf_accounts_uploads -- --ignored --nocapture`.
//! Cada teste imprime uma linha `PERF {json}`; os limites só pegam regressão grosseira.
use bytes::Bytes;
use futures_util::stream;
use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::uploads::store::UploadStore;
use std::{
    fs,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

fn percentile(samples: &mut [f64], p: f64) -> f64 {
    samples.sort_by(f64::total_cmp);
    let index = ((samples.len() - 1) as f64 * p).round() as usize;
    samples[index]
}

/// Disco real: em tmpfs o fsync não custa nada e esconderia o bloqueio do worker.
fn disk_dir() -> tempfile::TempDir {
    tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap()
}

fn claude_home(accounts: usize, files: usize) -> tempfile::TempDir {
    let home = disk_dir();
    let mut dirs = vec![home.path().join(".claude")];
    dirs.extend((0..accounts).map(|i| home.path().join(format!(".claude-a{i}"))));
    for (index, dir) in dirs.iter().enumerate() {
        fs::create_dir_all(dir).unwrap();
        if index > 0 {
            fs::write(dir.join(".hangar-conta"), b"").unwrap();
        }
        fs::write(dir.join(".credentials.json"), b"{}").unwrap();
        for project in 0..files / 50 {
            let folder = dir.join(format!("projects/p{project}"));
            fs::create_dir_all(&folder).unwrap();
            for file in 0..50 {
                fs::write(folder.join(format!("s{file}.jsonl")), b"{}\n").unwrap();
            }
        }
    }
    home
}

fn service(home: &Path) -> AccountService {
    AccountService::new(AccountEnvironment::from_map(
        [
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
        ]
        .into(),
    ))
}

/// O laço das cotas e dos estados de login: catálogo, depois uma busca por rótulo por linha.
#[test]
#[ignore = "medição"]
fn claude_catalog_with_label_lookups() {
    let (accounts, files, rounds) = (8, 2500, 20);
    // Pedido com cache frio: pasta nova, nada guardado ainda para estes caminhos.
    let cold_home = claude_home(accounts, files);
    let cold_service = service(cold_home.path());
    let start = Instant::now();
    for row in cold_service.claude_catalog().unwrap().as_array().unwrap() {
        cold_service
            .claude_by_label(row["label"].as_str().unwrap())
            .unwrap();
    }
    let cold_request = start.elapsed().as_secs_f64() * 1000.0;
    let home = claude_home(accounts, files);
    let service = service(home.path());
    let rows = service.claude_catalog().unwrap();
    assert_eq!(rows.as_array().unwrap().len(), accounts + 1);
    let mut catalog = vec![];
    let mut lookups = vec![];
    for _ in 0..rounds {
        let start = Instant::now();
        let rows = service.claude_catalog().unwrap();
        catalog.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        for row in rows.as_array().unwrap() {
            service
                .claude_by_label(row["label"].as_str().unwrap())
                .unwrap();
        }
        lookups.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "PERF {}",
        serde_json::json!({"test":"claude_catalog_with_label_lookups","accounts":accounts + 1,
            "files_per_account":files,"rounds":rounds,"cold_request_ms":cold_request,
            "catalog_ms_p50":percentile(&mut catalog.clone(), 0.5),
            "catalog_ms_p95":percentile(&mut catalog, 0.95),
            "lookups_ms_p50":percentile(&mut lookups.clone(), 0.5),
            "lookups_ms_p95":percentile(&mut lookups, 0.95)})
    );
}

/// Uploads simultâneos no runtime de vários workers: vazão, latência e o maior atraso de um
/// tique de 1 ms, que mede quanto a escrita e o fsync prendem os workers do tokio.
#[test]
#[ignore = "medição"]
fn concurrent_uploads_throughput_and_worker_stall() {
    let (uploads, mib) = (4usize, 32usize);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let root = disk_dir();
    let store = UploadStore::new(root.path()).unwrap();
    let block = Bytes::from(vec![0x5a; 1024 * 1024]);
    let (latencies, total, stall) = runtime.block_on(async move {
        let running = Arc::new(AtomicBool::new(true));
        let worst = Arc::new(AtomicU64::new(0));
        let ticker = tokio::spawn({
            let (running, worst) = (running.clone(), worst.clone());
            async move {
                while running.load(Ordering::Relaxed) {
                    let start = Instant::now();
                    tokio::time::sleep(Duration::from_millis(1)).await;
                    let late = start.elapsed().saturating_sub(Duration::from_millis(1));
                    worst.fetch_max(late.as_micros() as u64, Ordering::Relaxed);
                }
            }
        });
        let start = Instant::now();
        let jobs: Vec<_> = (0..uploads)
            .map(|index| {
                let (store, block) = (store.clone(), block.clone());
                tokio::spawn(async move {
                    let start = Instant::now();
                    let input = stream::iter((0..mib).map(move |_| Ok(block.clone())));
                    store
                        .publish(
                            "project",
                            &format!("session{index}"),
                            "big.bin",
                            None,
                            input,
                        )
                        .await
                        .unwrap();
                    start.elapsed().as_secs_f64() * 1000.0
                })
            })
            .collect();
        let mut latencies = vec![];
        for job in jobs {
            latencies.push(job.await.unwrap());
        }
        let total = start.elapsed().as_secs_f64();
        running.store(false, Ordering::Relaxed);
        ticker.await.unwrap();
        (
            latencies,
            total,
            worst.load(Ordering::Relaxed) as f64 / 1000.0,
        )
    });
    let mut latencies = latencies;
    println!(
        "PERF {}",
        serde_json::json!({"test":"concurrent_uploads","uploads":uploads,"mib_each":mib,
            "worker_threads":2,"throughput_mib_s":(uploads * mib) as f64 / total,
            "latency_ms_p50":percentile(&mut latencies.clone(), 0.5),
            "latency_ms_max":percentile(&mut latencies, 1.0),
            "worst_tick_lag_ms":stall})
    );
}

/// Listagem e poda numa sessão com muitos anexos: a poda roda a cada upload.
#[test]
#[ignore = "medição"]
fn list_and_prune_with_many_files() {
    let (files, rounds) = (2000, 10);
    let root = disk_dir();
    let folder = root.path().join("project/session");
    fs::create_dir_all(&folder).unwrap();
    for index in 0..files {
        fs::write(folder.join(format!("{index}-abc.bin")), b"x").unwrap();
    }
    let store = UploadStore::new(root.path()).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64();
    let mut list = vec![];
    let mut prune = vec![];
    for _ in 0..rounds {
        let start = Instant::now();
        assert_eq!(
            store.list("project", "session", 7, now).unwrap().len(),
            files
        );
        list.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = Instant::now();
        assert_eq!(store.prune("project", 7, now).unwrap(), 0);
        prune.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    println!(
        "PERF {}",
        serde_json::json!({"test":"list_and_prune","files":files,"rounds":rounds,
            "list_ms_p50":percentile(&mut list.clone(), 0.5),
            "list_ms_p95":percentile(&mut list, 0.95),
            "prune_ms_p50":percentile(&mut prune.clone(), 0.5),
            "prune_ms_p95":percentile(&mut prune, 0.95)})
    );
}
