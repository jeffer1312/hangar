use hangar_server::costs::index::{Index, Progress};
use serde_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};

pub fn contract() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract")
}

pub fn fixtures_copy() -> (tempfile::TempDir, PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let base = d.path().join("costs");
    copy_dir(&contract().join("costs"), &base);
    // A varredura grava rollouts pelo caminho canônico e o Python manda as raízes resolvidas: no
    // macOS a pasta temporária é /var → /private/var. No Windows o canônico é `\\?\`, em que `/`
    // não separa pastas, e os testes juntam caminhos com `/`.
    #[cfg(not(windows))]
    let base = std::fs::canonicalize(&base).unwrap();
    (d, base)
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() { copy_dir(&p, &to.join(e.file_name())) }
        else { std::fs::copy(&p, to.join(e.file_name())).unwrap(); }
    }
}

pub fn dump(ix: &Index, base: &Path, prefix: &str) -> Value {
    hangar_server::costs::index::dump_for_tests(ix, base, prefix)
}

pub fn assert_close(got: &Value, want: &Value, at: &str) {
    match (got, want) {
        (Value::Number(a), Value::Number(b)) if a.is_f64() || b.is_f64() => {
            let (a, b) = (a.as_f64().unwrap(), b.as_f64().unwrap());
            assert!((a - b).abs() <= 1e-9 * a.abs().max(b.abs()).max(1.0), "{at}: {a} != {b}");
        }
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{at}: tamanho");
            for (i, (x, y)) in a.iter().zip(b).enumerate() { assert_close(x, y, &format!("{at}[{i}]")); }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(a.keys().collect::<Vec<_>>(), b.keys().collect::<Vec<_>>(), "{at}: chaves");
            for (k, x) in a { assert_close(x, &b[k], &format!("{at}.{k}")); }
        }
        _ => assert_eq!(got, want, "{at}"),
    }
}

pub fn golden_index() -> Value {
    serde_json::from_slice(&std::fs::read(contract().join("golden/costs_index.json")).unwrap()).unwrap()
}

pub fn progress() -> Progress { Progress::default() }

pub fn halve_all(base: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut paths = hangar_server::costs::collect::list_files(base, |n| n.ends_with(".jsonl") && n != "session_index.jsonl");
    paths.sort();
    paths.into_iter().map(|p| {
        let raw = std::fs::read(&p).unwrap();
        let half = raw.len() / 2;
        let cut = raw[half..].iter().position(|b| *b == b'\n').map_or(raw.len(), |i| half + i + 1);
        std::fs::write(&p, &raw[..cut]).unwrap();
        (p, raw[cut..].to_vec())
    }).collect()
}

pub fn append(path: &Path, tail: &[u8]) {
    std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(tail).unwrap();
}
