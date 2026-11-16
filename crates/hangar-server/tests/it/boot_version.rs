use hangar_server::boot_version::{self,Snapshot};

#[cfg(unix)]
mod v01 {
    use std::{future::IntoFuture, io::{Read, Write}, path::Path, process::{Child, Command}, sync::Arc};
    use serde_json::{Value, json};
    use hangar_server::{accounts::{AccountService, environment::AccountEnvironment}, install_root, routes::AppState};

    struct Backend(Child);
    impl Drop for Backend { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

    fn child(name: &str, home: &Path) -> Command {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", &crate::exact_name(module_path!(), name), "--ignored", "--nocapture"])
            .env_clear().env("HOME", home).env("USERPROFILE", home)
            .env("PATH", std::env::var_os("PATH").unwrap()).env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1").current_dir(home);
        command
    }

    fn package(root: &Path, version: &str, commit: &str) {
        std::fs::create_dir_all(root.join("backend")).unwrap();
        std::fs::create_dir_all(root.join("python/bin")).unwrap();
        std::fs::copy("/bin/sh", root.join("python/bin/python3")).unwrap();
        std::fs::write(root.join("manifest.json"), json!({"version": version, "commit": commit, "channel": "latest"}).to_string()).unwrap();
        // O executável da fixture é shell, não interpretador Python nem app.main pessoal.
        std::fs::write(root.join("backend/app.main"), "printf ready > \"$HOME/ready\"\nread release < \"$HOME/release\"\n").unwrap();
    }

    fn fifo(path: &Path) { assert!(Command::new("mkfifo").arg(path).status().unwrap().success()); }

    fn start_backend(home: &Path) -> Backend {
        fifo(&home.join("ready")); fifo(&home.join("release"));
        use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};
        let mut ready = std::fs::OpenOptions::new().read(true).write(true).custom_flags(libc::O_NONBLOCK).open(home.join("ready")).unwrap();
        let mut command = child("launcher_child", home);
        command.stdout(std::process::Stdio::piped());
        let mut backend = Backend(command.spawn().unwrap());
        let mut fd = libc::pollfd { fd:ready.as_raw_fd(), events:libc::POLLIN, revents:0 };
        assert_eq!(unsafe { libc::poll(&mut fd, 1, 5000) }, 1, "o lançador não publicou o sinal de prontidão");
        let mut signal = [0; 5]; ready.read_exact(&mut signal).unwrap();
        assert_eq!(&signal, b"ready");
        let mut prefix = [0; 512];
        let size = backend.0.stdout.as_mut().unwrap().read(&mut prefix).unwrap();
        crate::assert_ran_one(&prefix[..size]);
        backend
    }

    #[test]
    #[ignore]
    fn launcher_child() { assert_eq!(install_root::launch(&["backend".into()]), std::process::ExitCode::SUCCESS); }

    #[tokio::test]
    #[ignore]
    async fn status_child() {
        let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
        let mut state = AppState::new(crate::fake::config(std::env::var("V01_UPSTREAM").unwrap().parse().unwrap(), ""));
        state.accounts = AccountService::new(AccountEnvironment::capture());
        state.share = None;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, hangar_server::routes::router(Arc::new(state))
            .into_make_service_with_connect_info::<std::net::SocketAddr>()).into_future());
        let path = std::env::var("V01_PATH").unwrap_or_else(|_| "/api/migration/status".into());
        let compressed = std::env::var_os("V01_GZIP").is_some();
        let client = reqwest::Client::builder().no_gzip().build().unwrap();
        let mut request = client.get(format!("http://{address}{path}")).bearer_auth(crate::fake::OWNER);
        if std::env::var_os("V01_NO_ACCEPT_ENCODING").is_none() {
            request = request.header("accept-encoding", if compressed { "gzip" } else { "identity" });
        }
        if std::env::var_os("V01_ORIGIN_VARY").is_some() { request = request.header("x-v01-vary-origin", "1"); }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), 200);
        if path == "/api/migration/status" { assert_eq!(response.headers()["cache-control"], "no-store"); }
        else if path == "/api/diag/arquivo" {
            assert_eq!(response.headers()["content-type"], "application/x-ndjson");
            assert_eq!(response.headers()["content-disposition"], "attachment; filename=\"hangar-uso.jsonl\"");
            assert_eq!(response.headers()["x-v01-source"], "original-nongit-producer");
        }
        if path == "/api/diag" { assert_eq!(response.headers()["content-type"], "application/json"); }
        let encoded = response.headers().get("content-encoding").is_some_and(|value| value == "gzip");
        if let Ok(expected) = std::env::var("V01_EXPECT_VARY") {
            assert_eq!(response.headers().get("vary").and_then(|value| value.to_str().ok()), Some(expected.as_str()));
            assert!(response.headers().get("content-encoding").is_none());
            assert!(response.headers().get("content-length").is_some());
            assert_eq!(response.headers()["x-v01-source-accept"], if std::env::var_os("V01_NO_ACCEPT_ENCODING").is_some() { "absent" } else { "identity" });
        } else if encoded {
            assert!(compressed);
            assert_eq!(response.headers()["vary"], "Accept-Encoding");
        }
        let length = response.headers().get("content-length").map(|v| v.to_str().unwrap().parse::<usize>().unwrap());
        let bytes = response.bytes().await.unwrap();
        if let Some(length) = length { assert_eq!(length, bytes.len()); }
        let bytes = if encoded {
            let mut out = Vec::new(); flate2::read::GzDecoder::new(bytes.as_ref()).read_to_end(&mut out).unwrap(); out
        } else { bytes.to_vec() };
        assert_eq!(encoded, std::env::var_os("V01_EXPECT_GZIP").is_some(), "gzip considera o corpo final");
        std::fs::write(home.join("status.json"), bytes).unwrap();
        server.abort();
    }

    async fn download(home: &Path, upstream: std::net::SocketAddr, compressed: bool, path: &str) -> String {
        let mut command = child("status_child", home);
        command.env("V01_UPSTREAM", upstream.to_string()).env("V01_PATH", path);
        if compressed { command.env("V01_GZIP", "1"); }
        if compressed && path == "/api/diag/arquivo" { command.env("V01_EXPECT_GZIP", "1"); }
        let output = tokio::task::spawn_blocking(move || command.output().unwrap()).await.unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        crate::assert_ran_one(&output.stdout);
        std::fs::read_to_string(home.join("status.json")).unwrap()
    }

    fn diary(pid: u32, started: &str, padding: usize) -> String {
        let padding = "x".repeat(padding);
        format!("{{\"evento\":\"diag.formato\",\"pid_backend\":{pid},\"inicio_backend\":{started:?},\"python\":\"3.14.6\",\"unknown\":{{\"literal\":1.00}},\"padding\":{padding:?}}}\n{{\"evento\":\"backend.inicio\",\"origem\":\"servidor\",\"pid_backend\":{pid},\"inicio_backend\":{started:?},\"ts\":{started:?},\"nivel\":\"ok\"}}\n")
    }

    fn wire(text: String, headers: axum::http::HeaderMap, download: bool) -> axum::response::Response {
        let mut bytes = text.into_bytes();
        let mut response = axum::http::Response::builder().header("content-type", if download { "application/x-ndjson" } else { "application/json" })
            .header("x-v01-source", "original-nongit-producer");
        if download { response = response.header("content-disposition", "attachment; filename=\"hangar-uso.jsonl\""); }
        if bytes.len() >= 1024 { response = response.header("vary", "Accept-Encoding"); }
        if bytes.len() >= 1024 && headers.get("accept-encoding").is_some_and(|v| v == "gzip") {
            let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(5));
            encoder.write_all(&bytes).unwrap(); bytes = encoder.finish().unwrap();
            response = response.header("content-encoding", "gzip");
        }
        response.header("content-length", bytes.len()).body(axum::body::Body::from(bytes)).unwrap()
    }

    #[tokio::test]
    async fn diary_header_and_two_python_lives_keep_their_own_version_after_rust_restart() { diary_case(false).await; }

    #[tokio::test]
    async fn compressed_diary_preserves_encoding_headers_and_enriched_body() { diary_case(true).await; }

    async fn diary_case(compressed: bool) {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join(".hangar/versions/original");
        package(&root, "0.1.0.8207", "abc1234def567890");
        std::os::unix::fs::symlink(&root, install_root::current(home)).unwrap();
        let mut backend = start_backend(home);
        let first_pid = backend.0.id();
        // estavel: carimbo após o FIFO pertence à vida real; não há asserção de duração.
        let first_start = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false);
        let first = diary(first_pid, &first_start, if compressed { 1100 } else { 0 });
        let body = Arc::new(std::sync::Mutex::new(first.clone()));
        let reply = body.clone();
        let summary = body.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, axum::Router::new()
            .route("/api/diag/arquivo", axum::routing::get(move |headers: axum::http::HeaderMap| { let reply = reply.clone(); async move {
                let text = reply.lock().unwrap().clone();
                wire(text, headers, true)
            } }))
            .route("/api/diag", axum::routing::get(move |headers: axum::http::HeaderMap| { let summary = summary.clone(); async move {
                let text = summary.lock().unwrap();
                wire(format!("{{\"unknown\":1.00,\"ultimas\":[{}],\"bytes\":5.00}}", text.lines().nth(1).unwrap()), headers, false)
            } }))).into_future());
        let first_download = download(home, upstream, compressed, "/api/diag/arquivo").await;
        let first_rows: Vec<Value> = first_download.lines().map(|row| serde_json::from_str(row).unwrap()).collect();
        assert_eq!(first_rows[0]["backend"], "0.1.0.8207-gabc1234def56");
        assert_eq!(first_rows[1]["backend"], "0.1.0.8207-gabc1234def56");
        assert!(first_download.contains("\"unknown\":{\"literal\":1.00}"), "campo alheio não é reserializado");
        let summary = download(home, upstream, compressed, "/api/diag").await;
        assert!(summary.contains("\"unknown\":1.00") && summary.contains("\"bytes\":5.00"));
        let summary: Value = serde_json::from_str(&summary).unwrap();
        assert_eq!(summary["ultimas"][0]["backend"], "0.1.0.8207-gabc1234def56");
        backend.0.kill().unwrap(); backend.0.wait().unwrap();
        std::fs::remove_file(home.join("ready")).unwrap(); std::fs::remove_file(home.join("release")).unwrap();
        std::fs::write(root.join("manifest.json"), json!({"version":"0.1.0.9999","commit":"ffffffffffffffff","channel":"latest"}).to_string()).unwrap();
        // Outro lançador, não o OnceLock do Rust que leu a primeira versão.
        backend = start_backend(home);
        // estavel: carimbo após o FIFO pertence à vida real; não há asserção de duração.
        let second_start = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false);
        let second = diary(backend.0.id(), &second_start, if compressed { 1100 } else { 0 });
        *body.lock().unwrap() = format!("{}{}", second, first.lines().nth(1).unwrap());
        let after = download(home, upstream, compressed, "/api/diag/arquivo").await;
        let rows: Vec<Value> = after.lines().map(|row| serde_json::from_str(row).unwrap()).collect();
        assert_eq!(rows[0]["backend"], "0.1.0.9999-gffffffffffff");
        assert_eq!(rows[1]["backend"], "0.1.0.9999-gffffffffffff");
        assert_eq!(rows[2]["backend"], "0.1.0.8207-gabc1234def56");
        assert_eq!(rows[2]["inicio_backend"], first_start);
        server.abort();
    }

    #[tokio::test]
    async fn gzip_threshold_uses_the_enriched_representation() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join(".hangar/versions/original");
        package(&root, "0.1.0.8207", "abc1234def567890");
        std::os::unix::fs::symlink(&root, install_root::current(home)).unwrap();
        let backend = start_backend(home);
        // estavel: carimbo após o FIFO pertence à vida real; não há asserção de duração.
        let started = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false);
        let empty = diary(backend.0.id(), &started, 0);
        let body = diary(backend.0.id(), &started, 1000 - empty.len());
        assert_eq!(body.len(), 1000, "produtor genérico ainda abaixo do mínimo");
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, axum::Router::new().route("/api/diag/arquivo",
            axum::routing::get(move |headers: axum::http::HeaderMap| { let body = body.clone(); async move { wire(body, headers, true) } }))).into_future());
        let text = download(home, upstream, true, "/api/diag/arquivo").await;
        assert!(text.len() >= 1024, "representação com identidade ultrapassa o mínimo");
        let rows: Vec<Value> = text.lines().map(|line| serde_json::from_str(line).unwrap()).collect();
        assert_eq!(rows[0]["backend"], "0.1.0.8207-gabc1234def56");
        assert_eq!(rows[1]["backend"], "0.1.0.8207-gabc1234def56");
        server.abort();
    }

    #[tokio::test]
    async fn identity_threshold_preserves_vary_without_gzip() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join(".hangar/versions/original");
        package(&root, "0.1.0.8207", "abc1234def567890");
        std::os::unix::fs::symlink(&root, install_root::current(home)).unwrap();
        let backend = start_backend(home);
        // estavel: carimbo após o FIFO pertence à vida real; não há asserção de duração.
        let started = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, false);
        let empty = diary(backend.0.id(), &started, 0);
        let body = diary(backend.0.id(), &started, 1000 - empty.len());
        assert_eq!(body.len(), 1000);
        let expected = body.replace("}\n", ",\"backend\":\"0.1.0.8207-gabc1234def56\"}\n");
        assert!(expected.len() >= 1024);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, axum::Router::new().route("/api/diag/arquivo",
            axum::routing::get(move |headers: axum::http::HeaderMap| { let body = body.clone(); async move {
                let origin = headers.contains_key("x-v01-vary-origin");
                let acceptance = headers.get("accept-encoding").and_then(|value| value.to_str().ok()).unwrap_or("absent").to_owned();
                let mut response = wire(body, headers, true);
                response.headers_mut().insert("x-v01-source-accept", acceptance.parse().unwrap());
                if origin { response.headers_mut().insert("vary", "Origin".parse().unwrap()); }
                response
            } }))).into_future());
        let mut failures = Vec::new();
        for omit in [false, true] {
            for origin in [false, true] {
                let mut command = child("status_child", home);
                command.env("V01_UPSTREAM", upstream.to_string()).env("V01_PATH", "/api/diag/arquivo")
                    .env("V01_EXPECT_VARY", if origin { "Origin, Accept-Encoding" } else { "Accept-Encoding" });
                if omit { command.env("V01_NO_ACCEPT_ENCODING", "1"); }
                if origin { command.env("V01_ORIGIN_VARY", "1"); }
                let output = tokio::task::spawn_blocking(move || command.output().unwrap()).await.unwrap();
                crate::assert_ran_one(&output.stdout);
                if !output.status.success() {
                    failures.push(format!("omit={omit} origin={origin}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr)));
                    continue;
                }
                assert_eq!(std::fs::read_to_string(home.join("status.json")).unwrap(), expected, "corpo integral omit={omit} origin={origin}");
            }
        }
        server.abort();
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}

proptest::proptest!{
    #[test]
    fn boot_version_injection_is_scoped_and_first_publication_wins(describe in ".{0,50}",readable in proptest::option::of(".{0,30}")){
        let root=tempfile::tempdir().unwrap();let other=tempfile::tempdir().unwrap();
        let snapshot=Snapshot{describe,readable};
        proptest::prop_assert_eq!(boot_version::publish(root.path(),snapshot.clone()),snapshot.clone());
        proptest::prop_assert_eq!(boot_version::get(root.path()),Some(snapshot.clone()));
        proptest::prop_assert_eq!(boot_version::get(other.path()),None);
        proptest::prop_assert_eq!(boot_version::publish(root.path(),Snapshot{describe:"later".into(),readable:None}),snapshot);
    }
}
#[test]
fn boot_version_get_does_not_wait_for_capture_or_block_other_roots(){
    use std::{sync::mpsc,time::Duration};
    let root=tempfile::tempdir().unwrap();let ready=tempfile::tempdir().unwrap();
    let expected=Snapshot{describe:"ready".into(),readable:None};
    boot_version::publish(ready.path(),expected.clone());
    let (entered_tx,entered_rx)=mpsc::channel();let (release_tx,release_rx)=mpsc::channel();
    let capture_root=root.path().to_owned();
    let capture=std::thread::spawn(move||{let mut first=true;boot_version::capture_with(&capture_root,|_,_|{if first{first=false;entered_tx.send(()).unwrap();release_rx.recv().unwrap();}Err(())})});
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (read_tx,read_rx)=mpsc::channel();let reading=root.path().to_owned();let ready_path=ready.path().to_owned();
    let reader=std::thread::spawn(move||read_tx.send((boot_version::get(&reading),boot_version::get(&ready_path))).unwrap());
    let observed=read_rx.recv_timeout(Duration::from_millis(300));
    release_tx.send(()).unwrap();capture.join().unwrap();reader.join().unwrap();
    assert_eq!(observed.unwrap(),(None,Some(expected)),"a consulta não pode aguardar os subprocessos da captura");
}
#[test]
fn capture_process_preserves_separate_channels_environment_and_timeout(){
    use std::{collections::BTreeMap,time::{Duration,Instant}};
    let root=tempfile::tempdir().unwrap();let args=vec![std::env::current_exe().unwrap().display().to_string(),"--exact".into(),"boot_version::capture_process_child".into(),"--ignored".into(),"--nocapture".into()];
    let mut environment=BTreeMap::from([("HANGAR_CAPTURE_FIXTURE".into(),"valor sintético".into())]);
    let output=hangar_server::update::process::run_capture_with_env(&args,root.path(),Duration::from_secs(3),&environment).unwrap();
    assert_eq!(output.code,0);assert!(output.stdout.contains("valor sintético\n\u{1b}[31mvalor\u{1b}[0m  \n"));assert!(!output.stdout.contains("diagnóstico"));assert_eq!(output.stderr,"diagnóstico\n");
    environment.insert("HANGAR_CAPTURE_WAIT".into(),"1".into());let began=Instant::now();
    assert!(matches!(hangar_server::update::process::run_capture_with_env(&args,root.path(),Duration::from_millis(150),&environment),Err(hangar_server::update::process::ProcessError::Timeout{..})));
    assert!(began.elapsed()<Duration::from_secs(3));
}
#[test]
#[ignore]
fn capture_process_child(){
    use std::io::Write;
    if std::env::var_os("HANGAR_CAPTURE_WAIT").is_some(){std::thread::sleep(std::time::Duration::from_secs(30));return;}
    std::io::stdout().write_all(format!("{}\r\n\u{1b}[31mvalor\u{1b}[0m  \r",std::env::var("HANGAR_CAPTURE_FIXTURE").unwrap()).as_bytes()).unwrap();
    std::io::stderr().write_all("diagnóstico\r\n".as_bytes()).unwrap();
}
