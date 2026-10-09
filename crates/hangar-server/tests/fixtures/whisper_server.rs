//! Servidor mínimo da fixture: valida os argumentos e recebe áudio sem depender do Whisper real.
use std::{io::{Read, Write}, net::{TcpListener, TcpStream}, path::PathBuf};

fn argument(name: &str) -> String {
    let args: Vec<_> = std::env::args().collect();
    args.windows(2).find(|pair| pair[0] == name).map(|pair| pair[1].clone()).unwrap()
}

fn reply(mut stream: TcpStream) {
    let mut received = Vec::new();
    let mut buffer = [0; 4096];
    let end;
    loop {
        let count = stream.read(&mut buffer).unwrap();
        if count == 0 { return; }
        received.extend_from_slice(&buffer[..count]);
        if let Some(found) = received.windows(4).position(|part| part == b"\r\n\r\n") { end = found + 4; break; }
    }
    let headers = String::from_utf8_lossy(&received[..end]).to_string();
    let length = headers.lines().find_map(|line| line.split_once(':')
        .filter(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())).unwrap_or(0);
    while received.len() < end + length {
        let count = stream.read(&mut buffer).unwrap();
        if count == 0 { break; }
        received.extend_from_slice(&buffer[..count]);
    }
    let (status, body) = if headers.starts_with("GET /health ") { (200, "{\"status\":\"ok\"}") }
        else if headers.starts_with("POST /inference ") && received[end..].windows(4).any(|p| p == b"RIFF") {
            (200, "{\"text\":\"Transcrição local em português.\"}")
        } else { (400, "{\"error\":\"pedido incompatível\"}") };
    write!(stream, "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
}

fn main() {
    assert_eq!(argument("--host"), "127.0.0.1");
    assert!(std::env::args().any(|a| a == "--no-gpu"));
    assert_eq!(argument("--language"), "pt");
    let model = PathBuf::from(argument("--model"));
    assert!(model.is_file());
    let starts = model.with_extension("starts");
    let mut file = std::fs::OpenOptions::new().create(true).append(true).open(starts).unwrap();
    writeln!(file, "{}", std::process::id()).unwrap();
    let listener = TcpListener::bind(format!("127.0.0.1:{}", argument("--port"))).unwrap();
    for stream in listener.incoming() { reply(stream.unwrap()); }
}
