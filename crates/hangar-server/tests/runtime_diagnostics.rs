use hangar_server::runtime::gateway::{self, RuntimeRegistry};
use std::{io::Write, sync::{Arc, Mutex}};

#[derive(Clone)]
struct Buffer(Arc<Mutex<Vec<u8>>>);
impl Write for Buffer {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
}

/// Um teste só neste binário: o assinante de log é global.
#[tokio::test]
async fn refusals_leave_their_reason_in_the_log() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let capture = output.clone();
    let subscriber = tracing_subscriber::fmt().without_time().with_ansi(false)
        .with_writer(move || Buffer(capture.clone())).finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "secret-test".into(), "instance-test".into()));
    let server = tokio::spawn(gateway::serve(listener, registry, "secret-test".into(), "instance-test".into(), hangar_server::INTERNAL_PROTOCOL));
    let client = reqwest::Client::new();
    let envelope = |protocol: u32, key: &str, command: serde_json::Value| serde_json::json!({"protocol":protocol,"instance":"instance-test",
        "key":key,"generation":1,"operation_id":"op","clock":{"monotonic_s":0.0,"epoch_s":0.0},"command":command}).to_string();
    let cases = [
        ("private-not-json".to_string(), 400),
        (envelope(hangar_server::INTERNAL_PROTOCOL + 1, "key-protocol", serde_json::json!({"kind":"snapshot"})), 409),
        (envelope(hangar_server::INTERNAL_PROTOCOL, "key-missing", serde_json::json!({"kind":"snapshot","private-field":"private-text"})), 503),
        (envelope(hangar_server::INTERNAL_PROTOCOL, "key-missing", serde_json::json!({"kind":"snapshot"})), 503),
    ];
    for _ in 0..3 {
        for (body, status) in &cases {
            let response = client.post(format!("http://{address}/runtime/op"))
                .header("x-hangar-internal", "secret-test").header("x-hangar-runtime-instance", "instance-test")
                .body(body.clone()).send().await.unwrap();
            assert_eq!(response.status().as_u16(), *status);
        }
    }
    server.abort();

    // Vigia do Codex com terminal sem multiplexador: a rodada sai sem decidir, mas diz por quê.
    let home = tempfile::tempdir().unwrap();
    let missing = home.path().join("missing-mux");
    let watcher = hangar_server::runtime::codex_terminal::Watcher::new(hangar_server::runtime::codex_terminal::WatchConfig {
        home: home.path().into(), queue_dir: home.path().join("queue"),
        mux: hangar_server::list::mux::Mux::with_program(&missing, std::time::Duration::from_secs(5)), mux_program: missing.clone().into_os_string() });
    let watched = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "secret-test".into(), "instance-test".into()));
    // Sem sessão para adotar a rodada nem abre o multiplexador: não há o que avisar.
    watcher.round(&watched).await;
    let idle = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(!idle.contains("codex_watch_panes_unavailable"), "sem nada a decidir a vigia abriu o multiplexador: {idle}");
    // O sidecar dá a ela o que decidir.
    let sidecars = hangar_server::runtime::codex_sidecar::dir(home.path());
    std::fs::create_dir_all(&sidecars).unwrap();
    std::fs::write(sidecars.join("watched.json"),
        serde_json::json!({"name":"watched","endpoint":"ws://127.0.0.1:9","thread_id":"thread-test","app_pid":1}).to_string()).unwrap();
    watcher.round(&watched).await;

    let log = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    let count = |needle: &[&str]| log.lines().filter(|line| needle.iter().all(|n| line.contains(n))).count();
    assert_eq!(count(&["runtime recusou envelope", "status=400", "check=envelope"]), 1, "{log}");
    assert_eq!(count(&["runtime recusou envelope", "status=409", "key=key-protocol", "check=protocol"]), 1, "{log}");
    assert_eq!(count(&["runtime recusou operação", "key=key-missing", "code=command_fields"]), 1, "{log}");
    assert_eq!(count(&["runtime recusou operação", "key=key-missing", "kind=snapshot", "code=runtime_binding"]), 1, "{log}");
    assert!(!log.contains("private-"), "corpo do pedido chegou ao log: {log}");
    assert_eq!(count(&["vigia do Codex com terminal sem a lista de panes", "codex_watch_panes_unavailable"]), 1, "{log}");
}
