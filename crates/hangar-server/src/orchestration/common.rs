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
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((String::new(), 0.)),
        Err(e) => return Err(e),
    };
    read_open(file)
}
pub fn read_open(mut file: fs::File) -> io::Result<(String, f64)> {
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    // O leitor Python de texto usa quebras universais, inclusive nos arquivos carimbados.
    if text.contains('\r') {
        text = text.replace("\r\n", "\n").replace('\r', "\n");
    }
    Ok((text, mtime_meta(&file.metadata()?)))
}

#[cfg(test)]
mod windows_readers_tests {
    #[test]
    fn windows_readers_common_read_matches_python_text_mode_and_file_version() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("regras.md");
        std::fs::write(&path, "papéis\r\nprimeiro\rsegundo\núltimo\r\n").unwrap();
        let file = std::fs::File::open(&path).unwrap();
        let modified = super::mtime_meta(&file.metadata().unwrap());
        let (text, version) = super::read_open(file).unwrap();
        assert_eq!(text, "papéis\nprimeiro\nsegundo\núltimo\n");
        assert_eq!(version, modified);
    }
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
/// Identidade numérica da Task sem limitar o inteiro que o Python gravou.
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct Integer(pub num_bigint::BigInt);
impl std::fmt::Display for Integer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::str::FromStr for Integer {
    type Err = num_bigint::ParseBigIntError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}
impl serde::Serialize for Integer {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_string()
            .parse::<serde_json::Number>()
            .map_err(serde::ser::Error::custom)?
            .serialize(serializer)
    }
}
impl PartialEq<i32> for Integer {
    fn eq(&self, other: &i32) -> bool {
        self.0 == num_bigint::BigInt::from(*other)
    }
}
pub fn integer(v: &Value) -> Option<Integer> {
    match v {
        Value::Number(n) => n.to_string().parse().ok(),
        _ => None,
    }
}
/// `int(str)`: sinal e separadores só nas posições aceitas, com algarismos decimais Unicode.
pub fn literal_integer(text: &str) -> Option<Integer> {
    let text = text.trim();
    let (sign, digits) = text
        .strip_prefix('-')
        .map(|s| ("-", s))
        .or_else(|| text.strip_prefix('+').map(|s| ("", s)))
        .unwrap_or(("", text));
    let mut normalized = sign.to_owned();
    let mut previous_digit = false;
    let mut count = 0;
    for c in digits.chars() {
        if c == '_' && previous_digit {
            previous_digit = false;
            continue;
        }
        let digit = crate::orq_conductor::decimal(c)?;
        normalized.push(char::from(b'0' + digit));
        previous_digit = true;
        count += 1;
        if count > 4300 {
            return None;
        }
    }
    (previous_digit && count > 0)
        .then(|| normalized.parse().ok())
        .flatten()
}
/// A coerção da linha do tempo e dos rótulos também aceita bool e float finito.
pub fn coerced_integer(value: &Value) -> Option<Integer> {
    match value {
        Value::Bool(v) => Some(Integer(num_bigint::BigInt::from(u8::from(*v)))),
        Value::Number(n) => integer(value).or_else(|| {
            n.as_f64()
                .filter(|n| n.is_finite())
                .and_then(|n| format!("{:.0}", n.trunc()).parse().ok())
        }),
        Value::String(s) => literal_integer(s),
        _ => None,
    }
}
pub fn py_string(v: &Value) -> String {
    super::codec::python_string(v)
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

/// Divisão de contadores inteiros com um arredondamento para f64, como a divisão longa do Python.
pub fn positive_mean(sum: &Integer, count: u64) -> f64 {
    use num_bigint::BigUint;
    if count == 0 || sum.0 <= num_bigint::BigInt::default() {
        return 0.;
    }
    let numerator = sum.0.magnitude();
    let denominator = BigUint::from(count);
    let mut exponent = numerator.bits() as i64 - denominator.bits() as i64;
    if if exponent >= 0 {
        numerator < &(&denominator << exponent as usize)
    } else {
        &(numerator << (-exponent) as usize) < &denominator
    } {
        exponent -= 1;
    }
    let shift = 52 - exponent;
    let (a, b) = if shift >= 0 {
        (numerator << shift as usize, denominator)
    } else {
        (numerator.clone(), denominator << (-shift) as usize)
    };
    let mut quotient = &a / &b;
    let twice_remainder = (&a % &b) << 1usize;
    if twice_remainder > b || (twice_remainder == b && quotient.bit(0)) {
        quotient += 1u8;
    }
    quotient.to_u64_digits().first().copied().unwrap_or(0) as f64
        * 2f64.powi((exponent - 52) as i32)
}
