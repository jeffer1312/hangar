#[path = "../src/protocol.rs"]
mod protocol;
use protocol::Tracker;
use serde_json::{Value, json};
use std::collections::VecDeque;

fn snapshot(tracker: &Tracker) -> Value {
    serde_json::from_str(&tracker.snapshot(42, &VecDeque::new(), None)).unwrap()
}

#[test]
fn pending_ids_keep_type() {
    let mut tracker = Tracker::default();
    tracker.observe_child(r#"{"id":1,"method":"approval"}"#);
    tracker.observe_child(r#"{"id":"1","method":"approval"}"#);
    tracker.observe_client(r#"{"id":1,"result":{}}"#);
    let pending = snapshot(&tracker)["pendentes"].as_array().unwrap().clone();
    assert_eq!(pending.len(), 1);
    assert_eq!(serde_json::from_str::<Value>(pending[0].as_str().unwrap()).unwrap()["id"], "1");
}

#[test]
fn snapshot_keeps_full_prefix_and_parent() {
    let mut tracker = Tracker::default();
    tracker.observe_client(r#"{"type":"user"}"#);
    for delta in ["Olá ", "🌎"] {
        tracker.observe_child(&json!({"type":"stream_event","event":{
            "type":"content_block_delta","delta":{"type":"text_delta","text":delta}}}).to_string());
    }
    tracker.observe_child(r#"{"type":"result","parent_tool_use_id":"child"}"#);
    let view = snapshot(&tracker);
    assert_eq!(view["versao"], 2);
    assert_eq!(view["aberto"], true);
    assert_eq!(view["inflight"]["claude"]["text"], "Olá 🌎");
    assert_eq!(view["inflight"]["claude"]["complete"], true);
}

#[test]
fn codex_prefixes_are_per_thread() {
    let mut tracker = Tracker::default();
    for (thread, text) in [("parent", "texto"), ("child", "filho")] {
        tracker.observe_child(&json!({"method":"item/agentMessage/delta","params":{
            "threadId":thread,"itemId":"item-1","turnId":"turn-1","delta":text}}).to_string());
    }
    let view = snapshot(&tracker);
    assert_eq!(view["inflight"]["codex"]["parent"]["text"], "texto");
    assert_eq!(view["inflight"]["codex"]["child"]["text"], "filho");
}

struct FakeCano(std::process::Child);
impl Drop for FakeCano {
    fn drop(&mut self) { let _ = self.0.kill(); let _ = self.0.wait(); }
}

fn start_fake(address: &str) -> FakeCano {
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-cano"))
        .args(["--escuta", address, "--token", "secret-test", "--", "python3", "-u", "-c",
            "import sys,json\nfor line in sys.stdin:\n print(json.dumps({'type':'echo','frame':json.loads(line)}),flush=True)"])
        .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null()).spawn().unwrap();
    FakeCano(child)
}

fn exercise_stream<S: std::io::Read + std::io::Write>(old: S, peek: S) {
    use std::io::{BufRead, Write};
    let mut old = std::io::BufReader::new(old);
    old.get_mut().write_all(b"secret-test\n").unwrap();
    let mut line = String::new();
    old.read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["versao"], 2);
    let mut peek = std::io::BufReader::new(peek);
    peek.get_mut().write_all(b"peek secret-test\n").unwrap();
    line.clear();
    peek.read_line(&mut line).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["type"], "cano_snapshot");
    line.clear();
    assert_eq!(peek.read_line(&mut line).unwrap(), 0);
    old.get_mut().write_all(b"{\"type\":\"cano_input\",\"operation_id\":\"wire:1\",\"frame\":\"{\\\"type\\\":\\\"user\\\"}\"}\n").unwrap();
    let mut ack = false;
    let mut echo = false;
    for _ in 0..2 {
        line.clear();
        old.read_line(&mut line).unwrap();
        let value: Value = serde_json::from_str(&line).unwrap();
        if value["type"] == "cano_input_ack" {
            assert_eq!(value["operation_id"], "wire:1");
            assert_eq!(value["outcome"], "written");
            ack = true;
        } else {
            assert_eq!(value["type"], "cano_output");
            let frame: Value = serde_json::from_str(value["frame"].as_str().unwrap()).unwrap();
            assert_eq!(frame["frame"], json!({"type":"user"}));
            echo = true;
        }
    }
    assert!(ack && echo);
}

/// Só flagra a resposta que não chega: o eco vem do `python3` filho, e no runner Windows ele já demorou
/// mais de 3 s.
const READ_WAIT: std::time::Duration = std::time::Duration::from_secs(30);

/// Sobe o cano numa porta livre. A porta sai de um bind que é solto antes do cano ocupá-la: com
/// testes em paralelo, outro processo pode pegá-la nesse meio, e quem responde não é o cano.
fn start_fake_tcp(first: std::net::SocketAddr) -> (FakeCano, std::net::SocketAddr) {
    let mut address = first;
    for _ in 0..5 {
        let child = start_fake(&format!("tcp:{address}"));
        if answers_as_cano(address) {
            return (child, address);
        }
        address = free_port();
    }
    panic!("cano sintético não iniciou em nenhuma porta");
}

/// O `peek` só lê o retrato e sai: confere quem está na porta sem mexer no cano.
fn answers_as_cano(address: std::net::SocketAddr) -> bool {
    use std::io::{BufRead, Write};
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if let Ok(mut stream) = std::net::TcpStream::connect(address) {
            let _ = stream.set_read_timeout(Some(READ_WAIT));
            let mut line = String::new();
            return stream.write_all(b"peek secret-test\n").is_ok()
                && std::io::BufReader::new(stream).read_line(&mut line).is_ok()
                && line.contains("cano_snapshot");
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn free_port() -> std::net::SocketAddr {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()
}

#[test]
fn tcp_start_survives_a_port_taken_by_someone_else() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = taken.local_addr().unwrap();
    // O dono da porta aceita e fecha sem ler: do lado de cá, o pedido vira "connection reset".
    std::thread::spawn(move || for stream in taken.incoming().flatten() { drop(stream) });
    let (_child, address) = start_fake_tcp(address);
    let mut stream = std::net::TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(READ_WAIT)).unwrap();
    use std::io::{BufRead, Write};
    stream.write_all(b"peek secret-test\n").unwrap();
    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line).unwrap();
    assert!(line.contains("cano_snapshot"), "{line}");
}

#[test]
fn peek_keeps_old_writer_tcp() {
    let (_child, address) = start_fake_tcp(free_port());
    let old = std::net::TcpStream::connect(address).unwrap();
    let peek = std::net::TcpStream::connect(address).unwrap();
    for stream in [&old, &peek] { stream.set_read_timeout(Some(READ_WAIT)).unwrap(); }
    exercise_stream(old, peek);
}

#[cfg(unix)]
#[test]
fn peek_keeps_old_writer_unix_private_socket() {
    use std::os::unix::{fs::PermissionsExt, net::UnixStream};
    let path = format!("/tmp/hangar-cano-v2-{}.sock", std::process::id());
    let _child = start_fake(&format!("unix:{path}"));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let old = loop {
        match UnixStream::connect(&path) {
            Ok(stream) => break stream,
            Err(_) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
            Err(error) => panic!("cano sintético não iniciou: {error}"),
        }
    };
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    let peek = UnixStream::connect(&path).unwrap();
    for stream in [&old, &peek] { stream.set_read_timeout(Some(READ_WAIT)).unwrap(); }
    exercise_stream(old, peek);
    std::fs::remove_file(path).unwrap();
}
