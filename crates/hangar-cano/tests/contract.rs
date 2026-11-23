//! Cano da sessão sem terminal: o processo sobrevive ao cliente e o snapshot diz o que está em
//! aberto. O filho é o `hangar-cano-fake-agent` (stream-json do Claude ou JSON-RPC do app-server).
//! Os mesmos casos que o contrato em pytest cobria contra o binário publicado.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const TOKEN: &str = "synthetic-cano-token";
const READ_WAIT: Duration = Duration::from_secs(30);

/// `HANGAR_CANO_BIN` aponta o binário que vai ser publicado (job `build` do CI); sem ele, o deste build.
fn cano() -> Command {
    Command::new(std::env::var_os("HANGAR_CANO_BIN").filter(|bin| !bin.is_empty())
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_hangar-cano").into()))
}
fn fake() -> &'static str { env!("CARGO_BIN_EXE_hangar-cano-fake-agent") }

/// Pasta curta e única: o caminho do socket unix tem teto no kernel.
struct Scratch(PathBuf);
impl Scratch {
    fn new(tag: &str) -> Self {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("cano-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self) -> &Path { &self.0 }
}
impl Drop for Scratch { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

struct Running(Child);
impl Drop for Running { fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); } }

#[cfg_attr(not(unix), allow(dead_code))]
fn wait_exit(child: &mut Child, limit: Duration) -> Option<std::process::ExitStatus> {
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait().unwrap() { return Some(status); }
        if Instant::now() >= deadline { return None; }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Um cliente do cano: manda o token, lê eventos (o `cano_output` já desembrulhado).
struct Client<S: std::io::Read + Write> { reader: BufReader<S> }
impl<S: std::io::Read + Write> Client<S> {
    fn open(stream: S) -> Self {
        let mut reader = BufReader::new(stream);
        reader.get_mut().write_all(format!("{TOKEN}\n").as_bytes()).unwrap();
        Client { reader }
    }
    fn send(&mut self, value: Value) { self.reader.get_mut().write_all(format!("{value}\n").as_bytes()).unwrap(); }
    fn read(&mut self) -> Value {
        let mut line = String::new();
        assert!(self.reader.read_line(&mut line).unwrap() > 0, "cano fechou a conexão");
        let event: Value = serde_json::from_str(&line).unwrap();
        if event["type"] == "cano_output" { serde_json::from_str(event["frame"].as_str().unwrap()).unwrap() } else { event }
    }
    #[cfg_attr(not(unix), allow(dead_code))]
    fn read_until(&mut self, kind: &str) -> Value {
        loop {
            let event = self.read();
            if event["type"] == kind { return event; }
        }
    }
}

#[cfg_attr(not(unix), allow(dead_code))]
fn frames(values: &Value) -> Vec<Value> {
    values.as_array().unwrap().iter().map(|v| serde_json::from_str(v.as_str().unwrap()).unwrap()).collect()
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::net::UnixStream;

    pub struct Cano { pub dir: Scratch, pub sock: PathBuf, pub log: PathBuf, pub child: Running }

    pub fn start(mode: &str) -> Cano {
        let dir = Scratch::new(mode);
        let (sock, log) = (dir.path().join("c.sock"), dir.path().join("cano.log"));
        let child = cano().arg("--escuta").arg(format!("unix:{}", sock.display())).args(["--token", TOKEN, "--log"]).arg(&log)
            .arg("--cwd").arg(dir.path()).args(["--", fake(), mode])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !sock.exists() {
            assert!(Instant::now() < deadline, "{}", std::fs::read_to_string(&log).unwrap_or_else(|_| "sem log".into()));
            std::thread::sleep(Duration::from_millis(20));
        }
        Cano { dir, sock, log, child: Running(child) }
    }

    pub fn client(sock: &Path) -> Client<UnixStream> {
        let stream = UnixStream::connect(sock).unwrap();
        stream.set_read_timeout(Some(READ_WAIT)).unwrap();
        Client::open(stream)
    }

    fn settle() { std::thread::sleep(Duration::from_millis(200)); }

    fn until_logged(log: &Path, line: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !std::fs::read_to_string(log).is_ok_and(|text| text.contains(line)) {
            assert!(Instant::now() < deadline, "o log do cano não registrou {line:?}");
            // estavel: intervalo de consulta do log, com teto de 10 s.
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn the_snapshot_has_the_fields_and_the_version() {
        let cano = start("claude");
        let snap = client(&cano.sock).read();
        let keys: std::collections::BTreeSet<_> = snap.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["type", "versao", "pid", "init", "aberto", "pendentes", "ultimo_result", "rate_limit", "stderr_tail", "saiu", "inflight"].into());
        // O servidor só religa um cano de `versao` 2 (runtime/cano.rs).
        assert_eq!(snap["versao"], 2);
        assert!(snap["pid"].is_u64());
        assert_eq!(snap["pendentes"], json!([]));
        assert_eq!(snap["stderr_tail"], json!([]));
        assert!(snap["saiu"].is_null());
    }

    #[test]
    fn the_snapshot_rebuilds_the_open_turn_and_the_pending_permission() {
        let mut cano = start("claude");
        let mut a = client(&cano.sock);
        let snap = a.read();
        assert_eq!(snap["type"], "cano_snapshot");
        assert!(snap["init"].is_null());
        assert_eq!(snap["aberto"], false);
        a.send(json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "initialize"}}));
        assert_eq!(a.read_until("system")["subtype"], "init");
        a.read_until("control_response");
        a.send(json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "oi"}]}}));
        assert_eq!(a.read_until("control_request")["request_id"], "perm-1");
        // O backend "cai" com a permissão pendente; o filho continua vivo no cano.
        drop(a);
        settle();
        assert!(cano.child.0.try_wait().unwrap().is_none());
        let mut b = client(&cano.sock);
        let snap = b.read();
        let init: Value = serde_json::from_str(snap["init"].as_str().unwrap()).unwrap();
        assert_eq!(init["subtype"], "init");
        assert_eq!(snap["aberto"], true);
        assert_eq!(frames(&snap["pendentes"]).iter().map(|p| p["request_id"].clone()).collect::<Vec<_>>(), [json!("perm-1")]);
        assert!(snap["saiu"].is_null());
        // O backend novo responde a permissão pendente e o turno fecha normalmente.
        b.send(json!({"type": "control_response", "response": {"subtype": "success", "request_id": "perm-1", "response": {"behavior": "allow"}}}));
        assert_eq!(b.read_until("result")["subtype"], "success");
        drop(b);
        let snap = client(&cano.sock).read();
        assert_eq!(snap["aberto"], false);
        assert_eq!(snap["pendentes"], json!([]));
        let last: Value = serde_json::from_str(snap["ultimo_result"].as_str().unwrap()).unwrap();
        assert_eq!(last["type"], "result");
    }

    #[test]
    fn the_snapshot_keeps_a_turn_started_by_a_message_from_another_session() {
        let cano = start("claude");
        let mut a = client(&cano.sock);
        a.read();
        a.send(json!({"type": "control_request", "request_id": "peer", "request": {"subtype": "emit_peer"}}));
        assert_eq!(a.read_until("command_lifecycle")["state"], "started");
        a.read_until("control_response");
        drop(a);
        settle();
        let mut b = client(&cano.sock);
        assert_eq!(b.read()["aberto"], true);
        b.send(json!({"type": "control_request", "request_id": "fim", "request": {"subtype": "finish_peer"}}));
        assert_eq!(b.read_until("result")["subtype"], "success");
        b.read_until("control_response");
        drop(b);
        settle();
        assert_eq!(client(&cano.sock).read()["aberto"], false);
    }

    #[test]
    fn the_child_exit_reaches_the_connected_client_with_stderr() {
        let mut cano = start("claude");
        let mut a = client(&cano.sock);
        a.read();
        a.send(json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": "sair"}]}}));
        assert_eq!(a.read_until("cano_stderr")["linha"], "tchau");
        let exit = a.read_until("cano_saiu");
        assert_eq!(exit["rc"], 3);
        assert_eq!(exit["stderr_tail"], json!(["tchau"]));
        drop(a);
        // Entregou o rc: sai sozinho, sem esperar o teto, e limpa o socket.
        assert!(wait_exit(&mut cano.child.0, Duration::from_secs(5)).is_some());
        assert!(!cano.sock.exists());
    }

    #[test]
    fn the_child_exit_without_a_client_stays_in_the_snapshot() {
        let mut cano = start("claude");
        let mut a = client(&cano.sock);
        a.read();
        // O filho só sai depois de o cano ver o backend cair: saindo antes, o rc já iria ao `a`.
        let gate = cano.dir.path().join("sair");
        a.send(json!({"type": "user", "message": {"role": "user", "content": [{"type": "text", "text": format!("sair quando {}", gate.display())}]}}));
        drop(a);
        until_logged(&cano.log, "cliente saiu");
        std::fs::write(&gate, "").unwrap();
        until_logged(&cano.log, "claude saiu rc=3");
        assert!(wait_exit(&mut cano.child.0, Duration::from_millis(300)).is_none(), "o cano espera alguém buscar o rc");
        let mut b = client(&cano.sock);
        let snap = b.read();
        assert_eq!(snap["saiu"], 3);
        assert_eq!(snap["stderr_tail"], json!(["tchau"]));
        assert_eq!(b.read()["type"], "cano_saiu", "e ainda manda o evento para quem chegou depois");
        drop(b);
        assert!(wait_exit(&mut cano.child.0, Duration::from_secs(5)).is_some());
    }

    #[cfg(target_os = "linux")]
    fn alive(pid: u64) -> bool {
        // Zumbi conta como morto: quem colhe o filho do cano é o init, no tempo dele.
        std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()
            .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| !rest.starts_with('Z')))
            .unwrap_or(false)
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sigterm_ends_the_child_and_cleans_the_socket() {
        let mut cano = start("claude");
        let child = client(&cano.sock).read()["pid"].as_u64().unwrap();
        settle();
        unsafe { libc::kill(cano.child.0.id() as i32, libc::SIGTERM) };
        let status = wait_exit(&mut cano.child.0, Duration::from_secs(5)).expect("o cano não saiu no SIGTERM");
        assert_eq!(status.code(), Some(0));
        assert!(!cano.sock.exists());
        let deadline = Instant::now() + Duration::from_secs(5);
        while alive(child) && Instant::now() < deadline { std::thread::sleep(Duration::from_millis(50)); }
        assert!(!alive(child), "filho {child} vivo depois do SIGTERM; log={}", std::fs::read_to_string(&cano.log).unwrap_or_default());
    }

    #[test]
    fn exit_codes() {
        let dir = Scratch::new("codes");
        let no_command = cano().arg("--escuta").arg(format!("unix:{}", dir.path().join("a.sock").display())).output().unwrap();
        assert_eq!(no_command.status.code(), Some(2));
        let log = dir.path().join("cano.log");
        // Passa do limite do kernel para socket unix.
        let long = format!("unix:{}", dir.path().join(format!("{}.sock", "x".repeat(120))).display());
        let refused = cano().args(["--escuta", &long, "--token", TOKEN, "--log"]).arg(&log).args(["--", fake(), "claude"])
            .stdin(Stdio::null()).output().unwrap();
        assert_eq!(refused.status.code(), Some(1));
        assert!(std::fs::read_to_string(&log).unwrap().contains("não consegui escutar"));
        let missing = cano().arg("--escuta").arg(format!("unix:{}", dir.path().join("b.sock").display())).args(["--token", TOKEN, "--log"]).arg(&log)
            .arg("--").arg(dir.path().join("nao-existe")).stdin(Stdio::null()).output().unwrap();
        assert_eq!(missing.status.code(), Some(1));
        assert!(std::fs::read_to_string(&log).unwrap().contains("claude não subiu"));
    }

    #[test]
    fn a_json_rpc_request_from_the_server_stays_pending_in_the_snapshot() {
        let cano = start("app-server");
        let mut a = client(&cano.sock);
        a.read();
        a.send(json!({"jsonrpc": "2.0", "id": 7, "method": "turn/start", "params": {}}));
        assert_eq!(a.read()["id"], 7);
        assert_eq!(a.read()["method"], "item/commandExecution/requestApproval");
        assert_eq!(a.read()["method"], "item/fileChange/requestApproval");
        a.send(json!({"jsonrpc": "2.0", "id": 0, "result": {"decision": "accept"}})); // respondeu um só
        assert_eq!(a.read()["method"], "serverRequest/resolved");
        drop(a);
        settle();
        let mut b = client(&cano.sock);
        let snap = b.read();
        assert_eq!(frames(&snap["pendentes"]).iter().map(|p| p["id"].clone()).collect::<Vec<_>>(), [json!(1)]);
        // Turno fechado sem resolver o pedido (interrupção) leva o pendente junto.
        b.send(json!({"jsonrpc": "2.0", "id": 8, "method": "turn/interrupt", "params": {"threadId": "th"}}));
        assert_eq!(b.read()["id"], 8);
        assert_eq!(b.read()["method"], "turn/completed");
        drop(b);
        settle();
        assert_eq!(client(&cano.sock).read()["pendentes"], json!([]));
    }
}

/// Sobe o cano em TCP numa porta livre e devolve o processo e o endereço. A porta é escolhida antes
/// de o cano subir e outro teste em paralelo pode tomá-la: só vale a porta em que ESTE cano escutou.
/// Responder na porta não prova isso, porque quem responde pode ser o cano do outro teste.
fn start_tcp(dir: &Path) -> (Running, std::net::SocketAddr) {
    for attempt in 0..5 {
        let address = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
        let log = dir.join(format!("cano-{attempt}.log"));
        let mut child = Running(cano().arg("--escuta").arg(format!("tcp:{address}")).args(["--token", TOKEN, "--log"]).arg(&log)
            .arg("--cwd").arg(dir).args(["--", fake(), "claude"])
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap());
        // O cano só registra o pid do filho depois de escutar; escuta recusada o faz sair.
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if std::fs::read_to_string(&log).is_ok_and(|text| text.contains("claude pid=")) {
                return (child, address);
            }
            if child.0.try_wait().unwrap().is_some() { break; }
            assert!(Instant::now() < deadline, "{}", std::fs::read_to_string(&log).unwrap_or_else(|_| "sem log".into()));
            // estavel: intervalo de consulta da espera pelo log, com teto de 10 s; não é prazo de resultado.
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    panic!("o cano não escutou em nenhuma porta");
}

fn connect(address: std::net::SocketAddr) -> Option<std::net::TcpStream> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(stream) = std::net::TcpStream::connect(address) {
            stream.set_read_timeout(Some(READ_WAIT)).unwrap();
            return Some(stream);
        }
        if Instant::now() >= deadline { return None; }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_wrong_token_is_refused() {
    let dir = Scratch::new("token");
    let (_child, address) = start_tcp(dir.path());
    let mut wrong = connect(address).unwrap();
    wrong.write_all(b"errado\n").unwrap();
    let mut line = String::new();
    // Fechado sem snapshot (no Windows o fechamento pode chegar como reset).
    assert!(BufReader::new(wrong).read_line(&mut line).map_or(true, |n| n == 0), "{line}");
    let mut right = Client::open(connect(address).unwrap());
    assert_eq!(right.read()["type"], "cano_snapshot");
}

/// Roda também no Windows (TCP + token). Com o accept em série, o segundo cliente não recebia
/// snapshot enquanto o primeiro seguia ligado, e quem conecta sem snapshot mata o cano.
#[test]
fn a_new_client_replaces_the_connected_one_over_tcp() {
    let dir = Scratch::new("replace");
    let (mut child, address) = start_tcp(dir.path());
    let mut a = Client::open(connect(address).unwrap());
    assert_eq!(a.read()["type"], "cano_snapshot");
    let mut b = Client::open(connect(address).unwrap());
    assert_eq!(b.read()["type"], "cano_snapshot");
    let mut line = String::new();
    // O antigo foi desligado.
    assert!(a.reader.read_line(&mut line).map_or(true, |n| n == 0), "{line}");
    b.send(json!({"type": "control_request", "request_id": "r1", "request": {"subtype": "initialize"}}));
    assert_eq!(b.read()["type"], "system", "o novo fala com o filho");
    assert!(child.0.try_wait().unwrap().is_none());
}
