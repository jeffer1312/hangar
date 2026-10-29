//! Executável independente dos testes: simula motores locais com processos e pipes reais.
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::Duration,
};

fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("");
    if mode == "child" {
        std::fs::write(&args[1], std::process::id().to_string()).unwrap();
        std::thread::sleep(Duration::from_secs(120));
        return;
    }
    if mode == "hold" {
        std::fs::write(&args[1], std::process::id().to_string()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["child", &args[2]])
            .stdin(Stdio::null())
            .spawn()
            .unwrap();
        let _ = child.wait();
        return;
    }
    if mode == "fail" {
        eprint!("{}", "á".repeat(350));
        std::process::exit(7);
    }
    if mode == "empty" {
        return;
    }
    if mode == "invalid" {
        print!("erro do motor");
        return;
    }
    let mut text = String::new();
    std::io::stdin().read_to_string(&mut text).unwrap();
    if let Some(path) = args.get(1) {
        if !path.is_empty() {
            let mut count = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(Path::new(path))
                .unwrap();
            count.write_all(b"call\n").unwrap();
            std::thread::sleep(Duration::from_millis(60));
        }
    }
    let mut out = std::io::stdout().lock();
    out.write_all(if mode == "wav" { b"RIFF" } else { b"ID3" })
        .unwrap();
    out.write_all(text.as_bytes()).unwrap();
}
