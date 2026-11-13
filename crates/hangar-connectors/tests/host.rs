//! Host com processo real: o conector de prova é o mesmo binário em todas as provas, e o
//! comportamento muda pelos argumentos do manifesto.
use hangar_connectors::host::{Host, HostOptions};
use hangar_connectors::manifest::Manifest;
use hangar_connectors::registry::Registry;
use hangar_connectors::work_tracking::{Filter, GetParams, ListParams};
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const FIXTURE: &str = env!("CARGO_BIN_EXE_hangar-fixture-connector");

struct Bench { dir: tempfile::TempDir, host: Arc<Host> }

impl Bench {
    fn new(options: HostOptions) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::new(Registry::new(dir.path().join("connectors.json")), options);
        Self { dir, host }
    }

    fn quick() -> Self {
        Self::new(HostOptions { init_timeout: Duration::from_secs(10), call_timeout: Duration::from_secs(10), idle: Duration::from_secs(60), exit_grace: Duration::from_secs(1) })
    }

    fn path(&self, name: &str) -> PathBuf { self.dir.path().join(name) }

    fn add(&self, id: &str, args: &[&str]) {
        self.add_in(self.dir.path(), id, args);
    }

    fn add_in(&self, folder: &Path, id: &str, args: &[&str]) {
        let mut command = vec![FIXTURE.to_owned(), "--id".into(), id.to_owned()];
        command.extend(args.iter().map(|arg| arg.to_string()));
        let value = json!({"manifest_version": 1, "id": id, "name": id, "version": "1", "protocol_version": 1,
            "capabilities": {"work_tracking": 1}, "command": command});
        self.host.add(Manifest::parse(value.to_string().as_bytes(), folder).unwrap()).unwrap();
    }

    fn lives(&self, file: &str) -> Vec<u32> {
        std::fs::read_to_string(self.path(file)).unwrap_or_default().lines().map(|line| line.parse().unwrap()).collect()
    }
}

impl Drop for Bench {
    fn drop(&mut self) {
        let host = self.host.clone();
        // Cada prova encerra as árvores que abriu.
        std::thread::spawn(move || tokio::runtime::Runtime::new().unwrap().block_on(host.shutdown())).join().unwrap();
    }
}

fn params(limit: u32) -> ListParams { ListParams { filter: Filter::default(), limit, cursor: None } }

async fn gone(pid: u32) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    while hangar_core::terminal_process::pid_alive(pid).unwrap_or(false) {
        assert!(tokio::time::Instant::now() < deadline, "processo {pid} continua vivo");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn list_and_get_share_one_life_and_keep_opaque_ids() {
    let bench = Bench::quick();
    bench.add("prova", &["--pid-file", bench.path("pids").to_str().unwrap()]);
    let first = bench.host.list_tickets("prova", params(2)).await.unwrap();
    assert_eq!(first.items.iter().map(|item| item.id.as_str()).collect::<Vec<_>>(), ["cartão:1/1", "cartão:1/2"]);
    assert_eq!(first.next_cursor.as_deref(), Some("pagina-2"));
    let second = bench.host.list_tickets("prova", ListParams { cursor: first.next_cursor.clone(), ..params(2) }).await.unwrap();
    assert_eq!(second.items[0].id, "cartão:2/1");
    assert_eq!(second.next_cursor, None);
    let detail = bench.host.get_ticket("prova", GetParams { ticket_id: "cartão:2/1".into() }).await.unwrap();
    assert_eq!(detail.summary.id, "cartão:2/1");
    assert_eq!(detail.description_markdown, "**negrito** e `código`");
    assert_eq!(bench.lives("pids").len(), 1, "uma vida só para as três chamadas");
    assert!(bench.host.list().unwrap()[0].running);
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_and_disabled_connectors_refuse_without_starting() {
    let bench = Bench::quick();
    assert_eq!(bench.host.list_tickets("nada", params(1)).await.unwrap_err().code(), "connector_not_found");
    bench.add("prova", &["--pid-file", bench.path("pids").to_str().unwrap()]);
    bench.host.set_enabled("prova", false).await.unwrap();
    assert_eq!(bench.host.list_tickets("prova", params(1)).await.unwrap_err().code(), "connector_disabled");
    assert!(bench.lives("pids").is_empty());
    assert!(!bench.host.list().unwrap()[0].running);
}

#[tokio::test(flavor = "multi_thread")]
async fn connector_error_keeps_the_life_and_is_typed() {
    let bench = Bench::quick();
    bench.add("prova", &["--pid-file", bench.path("pids").to_str().unwrap(), "--fail", "work_tracking.get=auth_required"]);
    let error = bench.host.get_ticket("prova", GetParams { ticket_id: "x".into() }).await.unwrap_err();
    assert_eq!(error.code(), "auth_required");
    bench.host.list_tickets("prova", params(1)).await.unwrap();
    assert_eq!(bench.lives("pids").len(), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn timeout_ends_the_life_and_the_next_call_starts_a_new_one() {
    let bench = Bench::new(HostOptions { call_timeout: Duration::from_millis(500), ..HostOptions { exit_grace: Duration::from_millis(200), ..HostOptions::default() } });
    bench.add("lenta", &["--pid-file", bench.path("pids").to_str().unwrap(), "--hang", "work_tracking.get"]);
    let error = bench.host.get_ticket("lenta", GetParams { ticket_id: "x".into() }).await.unwrap_err();
    assert_eq!(error.code(), "timeout");
    gone(bench.lives("pids")[0]).await;
    bench.host.list_tickets("lenta", params(1)).await.unwrap();
    assert_eq!(bench.lives("pids").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn crash_is_reported_once_and_not_retried_silently() {
    let bench = Bench::quick();
    bench.add("cai", &["--pid-file", bench.path("pids").to_str().unwrap(), "--exit", "work_tracking.get"]);
    assert_eq!(bench.host.get_ticket("cai", GetParams { ticket_id: "x".into() }).await.unwrap_err().code(), "unavailable");
    assert_eq!(bench.lives("pids").len(), 1, "a chamada que falhou não relançou");
    bench.host.list_tickets("cai", params(1)).await.unwrap();
    assert_eq!(bench.lives("pids").len(), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn transport_violations_are_protocol_errors() {
    let bench = Bench::quick();
    bench.add("lixo", &["--garbage", "work_tracking.list"]);
    bench.add("outro-id", &["--wrong-id", "work_tracking.list"]);
    bench.add("impostor", &["--claim-id", "outro"]);
    bench.add("grande", &["--big", "work_tracking.list"]);
    for (id, code) in [("lixo", "protocol_error"), ("outro-id", "protocol_error"), ("impostor", "protocol_error"), ("grande", "payload_too_large")] {
        assert_eq!(bench.host.list_tickets(id, params(1)).await.unwrap_err().code(), code, "{id}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn idle_life_ends_after_the_idle_window() {
    let bench = Bench::new(HostOptions { idle: Duration::from_millis(300), exit_grace: Duration::from_millis(500), ..HostOptions::default() });
    bench.add("ociosa", &["--pid-file", bench.path("pids").to_str().unwrap()]);
    bench.host.list_tickets("ociosa", params(1)).await.unwrap();
    gone(bench.lives("pids")[0]).await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while bench.host.list().unwrap()[0].running {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn disable_cancels_the_pending_call_and_ends_the_tree() {
    let bench = Bench::new(HostOptions { exit_grace: Duration::from_millis(300), ..HostOptions::default() });
    let grandchild = bench.path("neto");
    bench.add("presa", &["--pid-file", bench.path("pids").to_str().unwrap(), "--grandchild", grandchild.to_str().unwrap(), "--hang", "work_tracking.get", "--ignore-shutdown"]);
    let host = bench.host.clone();
    let pending = tokio::spawn(async move { host.get_ticket("presa", GetParams { ticket_id: "x".into() }).await });
    while bench.lives("pids").is_empty() || !grandchild.exists() || std::fs::read_to_string(&grandchild).unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let grandchild: u32 = std::fs::read_to_string(&grandchild).unwrap().parse().unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    bench.host.set_enabled("presa", false).await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), pending).await.unwrap().unwrap().unwrap_err();
    assert_eq!(error.code(), "unavailable");
    gone(bench.lives("pids")[0]).await;
    gone(grandchild).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn remove_stops_the_process_before_dropping_the_entry() {
    let bench = Bench::quick();
    bench.add("sai", &["--pid-file", bench.path("pids").to_str().unwrap()]);
    bench.host.list_tickets("sai", params(1)).await.unwrap();
    bench.host.remove("sai").await.unwrap();
    gone(bench.lives("pids")[0]).await;
    assert!(bench.host.list().unwrap().is_empty());
    assert_eq!(bench.host.list_tickets("sai", params(1)).await.unwrap_err().code(), "connector_not_found");
}

#[tokio::test(flavor = "multi_thread")]
async fn arguments_and_folder_with_spaces_reach_the_connector_intact() {
    let bench = Bench::quick();
    let folder = bench.path("pasta com espaço");
    std::fs::create_dir(&folder).unwrap();
    bench.add_in(&folder, "espacos", &["--title", "título com espaço e 'aspas' \"duplas\""]);
    let result = bench.host.list_tickets("espacos", params(1)).await.unwrap();
    assert_eq!(result.items[0].title, "título com espaço e 'aspas' \"duplas\"");
}

#[tokio::test(flavor = "multi_thread")]
async fn disable_during_negotiation_still_ends_the_whole_tree() {
    let bench = Bench::new(HostOptions { exit_grace: Duration::from_millis(300), ..HostOptions::default() });
    let grandchild = bench.path("neto");
    bench.add("negociando", &["--pid-file", bench.path("pids").to_str().unwrap(), "--grandchild", grandchild.to_str().unwrap(), "--hang", "initialize", "--ignore-shutdown"]);
    let host = bench.host.clone();
    let pending = tokio::spawn(async move { host.list_tickets("negociando", params(1)).await });
    while bench.lives("pids").is_empty() || !grandchild.exists() || std::fs::read_to_string(&grandchild).unwrap().is_empty() {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let grandchild: u32 = std::fs::read_to_string(&grandchild).unwrap().parse().unwrap();
    bench.host.set_enabled("negociando", false).await.unwrap();
    let error = tokio::time::timeout(Duration::from_secs(5), pending).await.unwrap().unwrap().unwrap_err();
    assert_eq!(error.code(), "unavailable");
    gone(bench.lives("pids")[0]).await;
    gone(grandchild).await;
}
