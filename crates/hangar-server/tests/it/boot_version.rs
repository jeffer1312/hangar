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

    fn advance_checkout(root: &Path, version: &str) -> String {
        let git = |args: &[&str]| {
            let output = Command::new("git").arg("-C").arg(root)
                .args(["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "-c", "commit.gpgSign=false"])
                .args(args).env("GIT_CONFIG_GLOBAL", "/dev/null").env("GIT_CONFIG_NOSYSTEM", "1")
                .env("GIT_AUTHOR_DATE", "2026-11-06T10:00:00+00:00").env("GIT_COMMITTER_DATE", "2026-11-06T10:00:00+00:00")
                .output().unwrap();
            assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
            String::from_utf8(output.stdout).unwrap()
        };
        if !root.join(".git").exists() { git(&["init", "--initial-branch=main"]); }
        std::fs::write(root.join("VERSION"), version).unwrap();
        git(&["add", "VERSION"]); git(&["commit", "-m", "Fixture version"]);
        git(&["rev-parse", "HEAD"])
    }

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
        let path = "/api/migration/status";
        let client = reqwest::Client::builder().no_gzip().build().unwrap();
        if std::env::var_os("V01_LATE_PUBLICATION").is_some() {
            let directory = home.join(".hangar/logs/privado/python-versions");
            let snapshot = std::fs::read_dir(directory).unwrap().next().unwrap().unwrap().path();
            let bytes = std::fs::read(&snapshot).unwrap();
            std::fs::remove_file(&snapshot).unwrap();
            let missing: Value = client.get(format!("http://{address}{path}")).bearer_auth(crate::fake::OWNER)
                .send().await.unwrap().json().await.unwrap();
            assert_eq!(missing["python"]["version"], "indisponivel");
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(snapshot).unwrap().write_all(&bytes).unwrap();
        }
        let response = client.get(format!("http://{address}{path}")).bearer_auth(crate::fake::OWNER)
            .header("accept-encoding", "identity").send().await.unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let bytes = response.bytes().await.unwrap();
        std::fs::write(home.join("status.json"), bytes).unwrap();
        server.abort();
    }

    async fn status(home: &Path, upstream: std::net::SocketAddr, late_publication: bool) -> Value {
        let mut command = child("status_child", home);
        command.env("V01_UPSTREAM", upstream.to_string());
        if late_publication { command.env("V01_LATE_PUBLICATION", "1"); }
        let output = tokio::task::spawn_blocking(move || command.output().unwrap()).await.unwrap();
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        crate::assert_ran_one(&output.stdout);
        serde_json::from_slice(&std::fs::read(home.join("status.json")).unwrap()).unwrap()
    }

    #[tokio::test]
    async fn python_version_survives_checkout_advance_and_actual_rust_restart() {
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path();
        let root = home.join(".hangar/versions/original");
        package(&root, "0.1.0.8207", "abc1234def567890");
        std::os::unix::fs::symlink(&root, install_root::current(home)).unwrap();
        let first_head = advance_checkout(&root, "0.1.0\n");
        let backend = start_backend(home);
        let pid = backend.0.id();
        let birth = hangar_server::terminal_facts::agent_birth(pid).unwrap();
        let facts = json!({"mode":"rust", "version":"must-not-trust-python-git", "processes":{"python":{"pid":pid}}});
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        let server = tokio::spawn(axum::serve(listener, axum::Router::new()
            .route("/internal/migration/status", axum::routing::get(move || { let facts = facts.clone(); async move { axum::http::Response::builder().header("content-type", "application/json").body(axum::body::Body::from(facts.to_string())).unwrap() } }))
            .fallback(|| async { (axum::http::StatusCode::BAD_GATEWAY, "V01 upstream trap") })).into_future());
        let first = status(home, upstream, true).await;
        // Literal da captura original V01-manifest-priority, não calculado pelo candidato.
        assert_eq!(first["python"]["version"], "0.1.0.8207-gabc1234def56");
        std::fs::write(root.join("manifest.json"), json!({"version":"0.1.0.9999","commit":"ffffffffffffffff","channel":"latest"}).to_string()).unwrap();
        assert_ne!(advance_checkout(&root, "0.2.0\n"), first_head);
        let after = status(home, upstream, false).await;
        assert_eq!(after["python"]["version"], "0.1.0.8207-gabc1234def56");
        assert_eq!(hangar_server::terminal_facts::agent_birth(pid), Some(birth));
        assert_eq!(after["python"]["processes"]["python"]["pid"], pid);
        let dir = home.join(".hangar/logs/privado/python-versions");
        let path = std::fs::read_dir(&dir).unwrap().next().unwrap().unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(snapshot["root"], root.to_string_lossy().as_ref());
        assert_eq!(snapshot["version"]["readable"], "0.1.0.8207");
        assert!(snapshot.get("argv").is_none() && snapshot.get("environ").is_none());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        for field in ["pid", "birth", "root", "executable"] {
            let mut corrupt = snapshot.clone();
            corrupt[field] = match field {
                "pid" => json!(pid + 1), "birth" => json!(birth + 1.0), _ => json!("/not-this-python-life"),
            };
            std::fs::write(&path, corrupt.to_string()).unwrap();
            assert_eq!(status(home, upstream, false).await["python"]["version"], "indisponivel", "{field}");
        }
        // Entrada direta ou publicação ainda ausente: sem versão emprestada, nem cache negativo.
        std::fs::remove_file(&path).unwrap();
        assert_eq!(status(home, upstream, false).await["python"]["version"], "indisponivel");
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).unwrap().write_all(&bytes).unwrap();
        assert_eq!(status(home, upstream, false).await["python"]["version"], "0.1.0.8207-gabc1234def56");
        server.abort();
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
