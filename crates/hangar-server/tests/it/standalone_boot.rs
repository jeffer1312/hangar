//! O `hangar-server` sem o Python na frente: configuração pelo `.env`, parada por sinal, vida do
//! servidor e aviso ao systemd. Sobe o binário de verdade, com HOME e portas próprias e sem tmux no
//! PATH (o servidor de quem roda os testes fica de fora).
use std::io::{BufRead, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn write_env(home: &Path, text: &str) {
    let dir = home.join(".hangar").join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(".env"), text).unwrap();
}

fn command(home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_hangar-server"));
    command.env_clear().env("HOME", home).env("USERPROFILE", home).env("PATH", home)
        .env("CP_RUST_NO_ORPHAN_SWEEP", "1")
        // A 8766 de verdade é do servidor de quem roda os testes; o processo vence o que a subida monta.
        .env("HANGAR_SERVER_GUEST_LISTEN", "127.0.0.1:0")
        .current_dir(home).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if cfg!(windows) {
        for key in ["SystemRoot", "WINDIR"] {
            if let Some(value) = std::env::var_os(key) { command.env(key, value); }
        }
    }
    command
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

fn health(port: u16) -> bool {
    let Ok(mut stream) = std::net::TcpStream::connect(("127.0.0.1", port)) else { return false };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    if stream.write_all(b"GET /__hangar_server/health HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").is_err() { return false; }
    let mut head = [0u8; 12];
    stream.read_exact(&mut head).is_ok() && head.starts_with(b"HTTP/1.1 200")
}

fn start(home: &Path, port: u16) -> Server {
    let mut server = Server(command(home).spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(60);
    while !health(port) {
        if let Some(status) = server.0.try_wait().unwrap() {
            let mut stderr = String::new();
            let _ = server.0.stderr.take().unwrap().read_to_string(&mut stderr);
            panic!("o servidor saiu ({status}) antes de subir: {stderr}");
        }
        assert!(Instant::now() < deadline, "o servidor não abriu a porta {port}");
        std::thread::sleep(Duration::from_millis(50));
    }
    server
}

fn wait_exit(server: &mut Server, limit: Duration) -> std::process::ExitStatus {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = server.0.try_wait().unwrap() { return status; }
        assert!(Instant::now() < deadline, "o servidor não saiu em {limit:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn diary(home: &Path) -> Vec<Value> {
    let Ok(dir) = std::fs::read_dir(home.join(".hangar/logs/diario/servidor-rust")) else { return Vec::new() };
    let mut files: Vec<_> = dir.flatten().map(|entry| entry.path()).collect();
    files.sort();
    files.iter().flat_map(|file| std::fs::read_to_string(file).unwrap().lines()
        .filter_map(|line| serde_json::from_str(line).ok()).collect::<Vec<Value>>()).collect()
}

fn events<'a>(lines: &'a [Value], name: &str) -> Vec<&'a Value> {
    lines.iter().filter(|line| line["evento"] == name).collect()
}

fn life(home: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(home.join(".hangar").join(hangar_boot::life::FILE)).unwrap()).unwrap()
}

/// A marca de pé só sai com a subida pronta, depois da porta aberta.
fn wait_running(home: &Path) {
    let deadline = Instant::now() + Duration::from_secs(60);
    while std::fs::read(home.join(".hangar").join(hangar_boot::life::FILE)).ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok()).is_none_or(|life| life["running"] != true) {
        assert!(Instant::now() < deadline, "a vida não se marcou de pé");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(unix)]
fn terminate(server: &Server) {
    assert!(Command::new("kill").args(["-TERM", &server.0.id().to_string()]).status().unwrap().success());
}

#[tokio::test]
async fn b01_b05_b13_b29_serves_from_the_env_file_without_python_and_ignores_stdin() {
    let home = tempfile::tempdir().unwrap();
    let port = free_port();
    write_env(home.path(), &format!("CP_PORT={port}\nCP_AUTH_TOKEN=\"proof\" # do .env\n"));
    let mut server = start(home.path(), port);
    // B-13: a identidade da instância nasce sem o Python, e com ela o registro de sessões.
    let stdout = server.0.stdout.take().unwrap();
    let first = tokio::task::spawn_blocking(move || {
        let mut line = String::new();
        std::io::BufReader::new(stdout).read_line(&mut line).unwrap();
        line
    });
    let first: Value = serde_json::from_str(tokio::time::timeout(Duration::from_secs(30), first).await.unwrap().unwrap().trim()).unwrap();
    assert_eq!(first["type"], "runtime_ready");
    assert_eq!(first["instance"].as_str().unwrap().len(), 32);
    // B-05: o token vem do `.env`; B-01: a porta também.
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}/api/migration/status");
    assert_eq!(client.get(&url).bearer_auth("errado").send().await.unwrap().status(), 401);
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let response = client.get(&url).bearer_auth("proof").send().await.unwrap();
        if response.status() == 200 { break; }
        assert!(Instant::now() < deadline, "status do dono não respondeu: {}", response.status());
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    // B-29: sem o Python o stdin é /dev/null (systemd) e o servidor segue de pé.
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert!(server.0.try_wait().unwrap().is_none() && health(port));
}

#[cfg(unix)]
#[test]
fn b25_b26_sigterm_stops_in_order_and_the_next_boot_counts_nothing() {
    let home = tempfile::tempdir().unwrap();
    let port = free_port();
    write_env(home.path(), &format!("CP_PORT={port}\nCP_AUTH_TOKEN=proof\n"));
    let mut server = start(home.path(), port);
    wait_running(home.path());
    terminate(&server);
    assert!(wait_exit(&mut server, Duration::from_secs(120)).success());
    assert_eq!(life(home.path())["running"], false);
    let lines = diary(home.path());
    assert_eq!(events(&lines, "backend.inicio").len(), 1);
    assert_eq!(events(&lines, "backend.encerrando")[0]["causa"], "SIGTERM");
    drop(server);
    let mut again = start(home.path(), port);
    assert!(events(&diary(home.path()), "hangar_server.caiu").is_empty(), "parada limpa não é queda");
    wait_running(home.path());
    terminate(&again);
    assert!(wait_exit(&mut again, Duration::from_secs(120)).success());
    assert_eq!(life(home.path())["unclean_exits"], 0);
}

#[tokio::test]
async fn b24_a_killed_life_is_counted_on_the_next_boot() {
    let home = tempfile::tempdir().unwrap();
    let port = free_port();
    write_env(home.path(), &format!("CP_PORT={port}\nCP_AUTH_TOKEN=proof\n"));
    let mut server = start(home.path(), port);
    wait_running(home.path());
    server.0.kill().unwrap();
    server.0.wait().unwrap();
    drop(server);
    let _again = start(home.path(), port);
    let lines = diary(home.path());
    let fall = events(&lines, "hangar_server.caiu");
    assert_eq!(fall.len(), 1, "{lines:?}");
    assert_eq!(fall[0]["saidas_nao_limpas"], 1);
    assert!(fall[0]["saida"].is_string());
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let status: Value = loop {
        let response = client.get(format!("http://127.0.0.1:{port}/api/migration/status")).bearer_auth("proof").send().await.unwrap();
        if response.status() == 200 { break response.json().await.unwrap(); }
        assert!(Instant::now() < deadline, "status do dono não respondeu");
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(status["rust"]["unclean_exits"], 1);
}

/// O systemd religa quem sai com a porta tomada: cada tentativa diz por quê no diário e nenhuma conta queda.
#[test]
fn b07_port_taken_retries_are_not_falls() {
    let home = tempfile::tempdir().unwrap();
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    write_env(home.path(), &format!("CP_PORT={}\nCP_AUTH_TOKEN=proof\n", taken.local_addr().unwrap().port()));
    for _ in 0..3 {
        assert_eq!(command(home.path()).output().unwrap().status.code(), Some(1));
    }
    let lines = diary(home.path());
    assert_eq!(events(&lines, "hangar_server.partida").iter().filter(|line| line["codigo"] == "port_unavailable").count(), 3);
    assert!(events(&lines, "hangar_server.caiu").is_empty(), "{lines:?}");
}

#[test]
fn b04_change_me_off_loopback_refuses_to_start() {
    let home = tempfile::tempdir().unwrap();
    write_env(home.path(), &format!("CP_LAN_BIND_IP=0.0.0.0\nCP_PORT={}\n", free_port()));
    let out = command(home.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Refusing to start: CP_AUTH_TOKEN is still the default 'change-me' while binding 0.0.0.0."));
    let lines = diary(home.path());
    assert_eq!(events(&lines, "hangar_server.partida")[0]["codigo"], "startup_refused");
}

/// Decisão 27: o Python subia com `CP_AUTH_TOKEN=` vazio; o Rust recusa e diz por quê.
#[test]
fn d27_empty_token_refuses_to_start() {
    let home = tempfile::tempdir().unwrap();
    write_env(home.path(), &format!("CP_AUTH_TOKEN=\nCP_PORT={}\n", free_port()));
    let out = command(home.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&out.stderr).contains("Refusing to start: CP_AUTH_TOKEN is empty."));
    assert_eq!(events(&diary(home.path()), "hangar_server.partida")[0]["codigo"], "startup_refused");
}

#[test]
fn b04_change_me_on_loopback_warns_and_serves() {
    let home = tempfile::tempdir().unwrap();
    let port = free_port();
    write_env(home.path(), &format!("CP_PORT={port}\n"));
    let mut server = start(home.path(), port);
    let mut stdout = std::io::BufReader::new(server.0.stdout.take().unwrap());
    let mut line = String::new();
    stdout.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "WARNING: using the default 'change-me' token on loopback. Set CP_AUTH_TOKEN before exposing this on your LAN.");
}

#[test]
fn b01_bad_port_refuses_with_a_code() {
    let home = tempfile::tempdir().unwrap();
    write_env(home.path(), "CP_PORT=abc\nCP_AUTH_TOKEN=proof\n");
    let out = command(home.path()).output().unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("configuração inválida: CP_PORT inválido"));
    assert_eq!(events(&diary(home.path()), "hangar_server.partida")[0]["codigo"], "config_invalid");
}

#[cfg(target_os = "linux")]
#[test]
fn p2_ready_watchdog_and_stopping_reach_the_systemd_socket() {
    let home = tempfile::tempdir().unwrap();
    let port = free_port();
    write_env(home.path(), &format!("CP_PORT={port}\nCP_AUTH_TOKEN=proof\n"));
    let socket_path = home.path().join("notify.sock");
    let socket = std::os::unix::net::UnixDatagram::bind(&socket_path).unwrap();
    socket.set_read_timeout(Some(Duration::from_secs(60))).unwrap();
    let mut server = Server(command(home.path()).env("NOTIFY_SOCKET", &socket_path).env("WATCHDOG_USEC", "400000").spawn().unwrap());
    let mut seen = Vec::new();
    let mut buffer = [0u8; 256];
    while !seen.iter().any(|message: &String| message == "WATCHDOG=1") {
        let read = socket.recv(&mut buffer).unwrap_or_else(|e| panic!("sem aviso ao systemd ({seen:?}): {e}"));
        seen.push(String::from_utf8_lossy(&buffer[..read]).into_owned());
    }
    assert_eq!(seen[0], "READY=1");
    terminate(&server);
    loop {
        let read = socket.recv(&mut buffer).unwrap();
        if &buffer[..read] == b"STOPPING=1" { break; }
    }
    assert!(wait_exit(&mut server, Duration::from_secs(120)).success());
}
