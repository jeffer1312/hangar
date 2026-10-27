use serde_json::{Value, json};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
pub fn bounded(
    program: &str,
    args: &[&str],
    cwd: Option<&Path>,
    timeout: Duration,
) -> io::Result<Output> {
    let mut out = tempfile::tempfile()?;
    let mut err = tempfile::tempfile()?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdout(Stdio::from(out.try_clone()?))
        .stderr(Stdio::from(err.try_clone()?));
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let mut child = cmd.spawn()?;
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "o programa não respondeu no prazo",
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    use std::io::{Seek, SeekFrom};
    out.seek(SeekFrom::Start(0))?;
    err.seek(SeekFrom::Start(0))?;
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    out.read_to_end(&mut stdout)?;
    err.read_to_end(&mut stderr)?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}
pub fn read(path: &Path) -> io::Result<(String, f64)> {
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((String::new(), 0.)),
        Err(e) => return Err(e),
    };
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok((text, mtime_meta(&file.metadata()?)))
}
pub fn mtime_meta(meta: &fs::Metadata) -> f64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.)
}
pub fn write(path: &Path, text: &str, expected: Option<f64>) -> Result<f64, String> {
    let current = match fs::metadata(path) {
        Ok(m) => mtime_meta(&m),
        Err(e) if e.kind() == io::ErrorKind::NotFound => 0.,
        Err(e) => return Err(e.to_string()),
    };
    if expected.is_some_and(|n| (current - n).abs() > 1e-6) {
        return Err("o arquivo mudou desde a leitura — recarregue".into());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    crate::groups::store::replace_atomic(path, text.as_bytes()).map_err(|e| e.to_string())?;
    fs::metadata(path)
        .map(|m| mtime_meta(&m))
        .map_err(|e| e.to_string())
}
pub fn expand(path: &Path, home: &Path) -> PathBuf {
    crate::accounts::environment::expand(&path.to_string_lossy(), home)
}
pub fn home() -> PathBuf {
    std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
        .map(PathBuf::from)
        .unwrap_or_default()
}
pub fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
pub fn kind(v: &Value) -> &str {
    text(&v["tipo"])
}
pub fn array(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or(&[])
}
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
pub fn integer(v: &Value) -> Option<i128> {
    v.as_i64()
        .map(i128::from)
        .or_else(|| v.as_u64().map(i128::from))
}
pub fn py_string(v: &Value) -> String {
    crate::orq_conductor::python_string(v)
}

pub fn increment(v: &mut Value, key: &str, n: i64) {
    v[key] = json!(v[key].as_i64().unwrap_or(0) + n)
}
pub fn round(v: f64, digits: i32) -> f64 {
    if digits >= 0 {
        format!("{v:.precision$}", precision = digits as usize)
            .parse()
            .unwrap_or(v)
    } else {
        let scale = 10f64.powi(digits);
        (v * scale).round_ties_even() / scale
    }
}
pub fn sha256(s: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, s.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
