use std::io::{BufRead, Write};

#[test]
fn two_processes_one_lease() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("lease");
    let backend = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend");
    let mut child = std::process::Command::new("python3")
        .env("PYTHONPATH",backend)
        .args(["-u","-c","from app.runtime_coordinator import WriterLease\nimport sys\nlease=WriterLease(sys.argv[1])\nprint('locked',flush=True)\nsys.stdin.read(1)\nlease.close()"])
        .arg(&path).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut line = String::new();
    std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    assert_eq!(line.trim(),"locked");
    let file = std::fs::OpenOptions::new().read(true).write(true).open(&path).unwrap();
    assert!(file.try_lock().is_err());
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(child.wait().unwrap().success());
    file.try_lock().unwrap();
}
