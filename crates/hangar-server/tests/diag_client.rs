//! Contrato público do diário local; processo próprio para os controles globais de diagnóstico.

use hangar_server::diag::DiagClient;
use notify::Watcher;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

fn rows(dir: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".jsonl"))
        .flat_map(|entry| {
            std::fs::read_to_string(entry.path())
                .unwrap()
                .lines()
                .filter(|line| !line.is_empty())
                .map(|line| serde_json::from_str(line).expect("linha JSON válida no diário"))
                .collect::<Vec<_>>()
        })
        .collect()
}

#[tokio::test]
async fn local_client_records_the_failure_without_any_upstream_request() {
    let dir = tempfile::tempdir().unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let requested = Arc::new(tokio::sync::Notify::new());
    let (seen, called) = (hits.clone(), requested.clone());
    let upstream = tokio::spawn(async move {
        let router = axum::Router::new().fallback(move || {
            let (seen, called) = (seen.clone(), called.clone());
            async move {
                seen.fetch_add(1, Ordering::SeqCst);
                called.notify_one();
                axum::http::StatusCode::SERVICE_UNAVAILABLE
            }
        });
        axum::serve(listener, router).await.unwrap();
    });
    let (tx, mut changes) = tokio::sync::mpsc::unbounded_channel();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if event.is_ok() {
            let _ = tx.send(());
        }
    })
    .unwrap();
    watcher
        .watch(dir.path(), notify::RecursiveMode::NonRecursive)
        .unwrap();
    let client = DiagClient::new(address, "segredo-nunca-usado".into())
        .with_diary(Some(dir.path().to_owned()));
    client.report(
        "rust.history_failed",
        "sessão",
        "history_io",
        "leitura do transcript falhou",
    );
    client.flush().await;
    let observed = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            tokio::select! {
                _ = requested.notified() => break false,
                change = changes.recv() => {
                    assert!(change.is_some());
                    if rows(dir.path()).iter().any(|row| row["evento"] == "rust.history_failed") { break true; }
                }
            }
        }
    }).await.unwrap_or(false);
    upstream.abort();
    assert_eq!(
        hits.load(Ordering::SeqCst),
        0,
        "o diário é local, inclusive em falha do destino HTTP antigo"
    );
    assert!(observed, "o evento precisa estar no arquivo local");
    let line = rows(dir.path())
        .into_iter()
        .find(|row| row["evento"] == "rust.history_failed")
        .unwrap();
    assert_eq!(line["codigo"], "history_io");
    assert_eq!(line["sessao"], "sessão");
    assert_eq!(line["nivel"], "erro");
    assert_eq!(line["detalhe"], "leitura do transcript falhou");
    assert_eq!(line["origem"], "servidor");
}

fn fixed_client(dir: &std::path::Path) -> DiagClient {
    use chrono::TimeZone;
    let now = chrono::Local
        .with_ymd_and_hms(2026, 10, 15, 9, 30, 0)
        .unwrap();
    DiagClient::new("127.0.0.1:9".parse().unwrap(), "segredo-descartado".into())
        .with_diary(Some(dir.to_owned()))
        .with_clock(move || now)
}

#[tokio::test]
async fn local_client_without_directory_never_opens_the_old_transport() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = DiagClient::new(listener.local_addr().unwrap(), "segredo-descartado".into())
        .with_diary(None);
    client.report("rust.without_diary", "ausente", "plain", "motivo fixo");
    client.report_with(
        "rust.without_diary",
        "ausente",
        "extra",
        "motivo fixo",
        serde_json::json!({"etapa":"leitura"}),
    );
    client.flush().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), listener.accept())
            .await
            .is_err(),
        "nenhuma conexão deve ser tentada sem diário"
    );
}

#[tokio::test]
async fn local_client_explicit_directory_and_clock_are_shared_with_existing_clones() {
    use chrono::TimeZone;
    hangar_server::diag::disable_diary();
    assert!(hangar_server::diag::diary_dir().is_none());
    let dir = tempfile::tempdir().unwrap();
    let client = DiagClient::new("127.0.0.1:9".parse().unwrap(), "segredo-descartado".into());
    let source = client.clone();
    let now = chrono::Local
        .with_ymd_and_hms(2026, 10, 15, 9, 30, 0)
        .unwrap();
    let client = client
        .with_diary(Some(dir.path().to_owned()))
        .with_clock(move || now);
    source.report("session.limit_check", "sessão", "limite", "motivo fixo");
    client.flush().await;
    let data = rows(dir.path());
    let line = data
        .iter()
        .find(|line| line["evento"] == "rust.session_limit_check")
        .unwrap();
    assert_eq!(line["codigo"], "limite");
    assert_eq!(
        line["ts"],
        now.to_rfc3339_opts(chrono::SecondsFormat::Millis, false)
    );
    assert!(dir.path().join("uso-2026-10-15.jsonl").is_file());
}

#[tokio::test]
async fn local_client_limits_by_diary_session_event_and_code() {
    let a = tempfile::tempdir().unwrap();
    let b = tempfile::tempdir().unwrap();
    let first = fixed_client(a.path());
    let other_address = DiagClient::new("127.0.0.1:18".parse().unwrap(), "outro-segredo".into())
        .with_diary(Some(a.path().to_owned()));
    first.report("rust.history_failed", "sessão", "io", "motivo fixo");
    other_address.report("rust.history_failed", "sessão", "io", "motivo fixo");
    first.report("rust.history_failed", "outra", "io", "motivo fixo");
    first.report("rust.history_failed", "sessão", "outro", "motivo fixo");
    first.report("rust.events_failed", "sessão", "io", "motivo fixo");
    let independent = fixed_client(b.path());
    independent.report("rust.history_failed", "sessão", "io", "motivo fixo");
    first.flush().await;
    other_address.flush().await;
    independent.flush().await;
    let mut keys: Vec<_> = rows(a.path())
        .into_iter()
        .map(|r| {
            (
                r["evento"].as_str().unwrap().to_owned(),
                r["sessao"].as_str().unwrap().to_owned(),
                r["codigo"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        vec![
            ("rust.events_failed".into(), "sessão".into(), "io".into()),
            ("rust.history_failed".into(), "outra".into(), "io".into()),
            ("rust.history_failed".into(), "sessão".into(), "io".into()),
            (
                "rust.history_failed".into(),
                "sessão".into(),
                "outro".into()
            )
        ]
    );
    assert!(
        rows(b.path())
            .iter()
            .any(|r| r["evento"] == "rust.history_failed"
                && r["sessao"] == "sessão"
                && r["codigo"] == "io")
    );
}

#[tokio::test]
async fn local_client_keeps_canonical_fields_and_the_existing_scalar_projection() {
    let dir = tempfile::tempdir().unwrap();
    let client = fixed_client(dir.path());
    client.report_with("session.limit_check", &"á".repeat(200), "IO-falha.x", "motivo fixo", serde_json::json!({
        "evento":"forjado", "nivel":"ok", "sessao":"forjada", "codigo":"forjado", "motivo":"forjado",
        "ts":"forjado", "origem":"tela", "backend":"forjado", "pid_backend":0,
        "detalhe":"é".repeat(400), "ms":12, "vazio":" ", "flag":true, "objeto":{"raw":"fora"}, "lista":["fora"]
    }));
    client.flush().await;
    let line = rows(dir.path()).pop().unwrap();
    assert_eq!(line["evento"], "rust.session_limit_check");
    assert_eq!(line["codigo"], "IO_falha_x");
    assert_eq!(line["sessao"].as_str().unwrap().chars().count(), 128);
    assert_eq!(line["nivel"], "erro");
    assert_eq!(line["motivo"], "motivo fixo");
    assert_eq!(line["origem"], "servidor");
    assert_eq!(line["pid_backend"], std::process::id());
    assert_ne!(line["backend"], "forjado");
    assert_ne!(line["ts"], "forjado");
    assert_eq!(line["detalhe"].as_str().unwrap().chars().count(), 300);
    assert_eq!(line["ms"], 12);
    for key in ["vazio", "flag", "objeto", "lista"] {
        assert!(line.get(key).is_none(), "{key}");
    }
}

#[tokio::test]
async fn local_client_preserves_levels_and_reason_fields_of_both_entry_points() {
    let dir = tempfile::tempdir().unwrap();
    let client = fixed_client(dir.path());
    for event in [
        "runtime.send_uncertain",
        "runtime.command_deferred",
        "rust.groups_sweep_failed",
        "rust.groups_sweep_recovered",
        "opcao.envio_falhou",
    ] {
        client.report(event, event, "plain", "motivo fixo");
        client.report_with(
            event,
            event,
            "extra",
            "motivo fixo",
            serde_json::json!({"etapa":"prova"}),
        );
    }
    client.flush().await;
    let data = rows(dir.path());
    assert_eq!(
        data.len(),
        10,
        "cada evento conserva as duas entradas com códigos distintos"
    );
    for line in data {
        let event = line["evento"].as_str().unwrap();
        let plain = line["codigo"] == "plain";
        let warning = matches!(
            event,
            "rust.groups_sweep_failed" | "rust.groups_sweep_recovered"
        ) || (plain
            && matches!(event, "runtime.send_uncertain" | "runtime.command_deferred"));
        assert_eq!(line["nivel"], if warning { "aviso" } else { "erro" });
        assert_eq!(
            line[if plain { "detalhe" } else { "motivo" }],
            "motivo fixo"
        );
        assert!(line.get(if plain { "motivo" } else { "detalhe" }).is_none());
    }
}

#[tokio::test]
async fn local_client_prunes_only_daily_files_older_than_the_seven_day_window() {
    let dir = tempfile::tempdir().unwrap();
    for name in [
        "uso-2026-10-08.jsonl",
        "uso-2026-10-09.jsonl",
        "uso-2026-10-20.jsonl",
        "nota.txt",
        "uso-outro.jsonl",
    ] {
        std::fs::write(dir.path().join(name), "preservado").unwrap();
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            dir.path().join("nota.txt"),
            dir.path().join("uso-2000-01-01.jsonl"),
        )
        .unwrap();
    }
    let client = fixed_client(dir.path());
    client.report("rust.retention_probe", "retenção", "io", "motivo fixo");
    client.flush().await;
    assert!(!dir.path().join("uso-2026-10-08.jsonl").exists());
    for name in [
        "uso-2026-10-09.jsonl",
        "uso-2026-10-20.jsonl",
        "nota.txt",
        "uso-outro.jsonl",
    ] {
        assert_eq!(
            std::fs::read_to_string(dir.path().join(name)).unwrap(),
            "preservado"
        );
    }
    #[cfg(unix)]
    assert!(dir.path().join("uso-2000-01-01.jsonl").is_symlink());
}

#[tokio::test]
async fn local_client_keeps_error_records_after_the_soft_cap_and_stops_at_the_hard_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("uso-2026-10-15.jsonl");
    let client = fixed_client(dir.path());
    std::fs::write(
        &path,
        vec![b'\n'; hangar_server::diag::DIARY_DAY_CAP as usize],
    )
    .unwrap();
    client.report("rust.cap_probe", "tetos", "soft", "motivo fixo");
    client.flush().await;
    assert!(
        rows(dir.path())
            .iter()
            .any(|row| row["evento"] == "rust.cap_probe" && row["codigo"] == "soft")
    );
    std::fs::write(&path, vec![b'\n'; 8 << 20]).unwrap();
    client.report("rust.cap_probe", "tetos", "hard", "motivo fixo");
    client.flush().await;
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 8 << 20);
    assert!(rows(dir.path()).is_empty());
}

#[derive(Clone)]
struct TraceBytes(Arc<std::sync::Mutex<Vec<u8>>>);
impl std::io::Write for TraceBytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn local_client_write_failure_traces_only_the_failure_code() {
    let captured = TraceBytes(Arc::default());
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(move || writer.clone())
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let blocked = dir.path().join("arquivo");
    std::fs::write(&blocked, "barreira").unwrap();
    let client = fixed_client(&blocked);
    client.report_with(
        "rust.failure_probe",
        "sessão-secreta",
        "codigo-secreto",
        "motivo fixo",
        serde_json::json!({"detalhe":"texto-secreto", "api_key":"chave-secreta"}),
    );
    client.flush().await;
    let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(text.contains("diag_write_failed"), "{text}");
    for secret in [
        "sessão-secreta",
        "codigo-secreto",
        "texto-secreto",
        "chave-secreta",
        "api_key",
    ] {
        assert!(!text.contains(secret), "{secret}");
    }
}
