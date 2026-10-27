use super::*;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use std::{
    fmt,
    fs::{self, File, OpenOptions},
    io,
    process::Stdio,
};
use tokio::io::AsyncReadExt;

struct Strict(Value);
impl<'de> Deserialize<'de> for Strict {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Strict;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("JSON")
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Strict, E> {
                Ok(Strict(json!(v)))
            }
            fn visit_unit<E: de::Error>(self) -> Result<Strict, E> {
                Ok(Strict(Value::Null))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut v = vec![];
                while let Some(x) = a.next_element::<Strict>()? {
                    v.push(x.0)
                }
                Ok(Strict(Value::Array(v)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut a: A) -> Result<Strict, A::Error> {
                let mut v = Map::new();
                while let Some((k, x)) = a.next_entry::<String, Strict>()? {
                    if v.contains_key(&k) {
                        return Err(de::Error::custom("JSON contém chaves repetidas"));
                    }
                    v.insert(k, x.0);
                }
                Ok(Strict(Value::Object(v)))
            }
        }
        d.deserialize_any(V)
    }
}
pub fn parse(raw: &str) -> Result<Value, String> {
    let mut decoder = serde_json::Deserializer::from_str(raw);
    let v = Strict::deserialize(&mut decoder)
        .map_err(|e| {
            if e.to_string().contains("JSON contém chaves repetidas") {
                err("JSON contém chaves repetidas")
            } else {
                err("Registro JSON inválido")
            }
        })?
        .0;
    decoder.end().map_err(|_| err("Registro JSON inválido"))?;
    if !v.is_object() {
        return Err(err("Registro deve ser um objeto"));
    }
    Ok(v)
}
pub fn symlink(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink())
}
pub fn read(p: &Path, default: Option<Value>) -> Result<Value, String> {
    if symlink(p) {
        return Err(err("Registro simbólico recusado"));
    }
    match fs::read(p) {
        Ok(raw) => parse(std::str::from_utf8(&raw).map_err(|_| err("Registro JSON inválido"))?),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            default.ok_or_else(|| err("Registro obrigatório ausente"))
        }
        Err(e) => Err(os_error(e)),
    }
}
pub fn os_error(e: io::Error) -> String {
    match e.kind() {
        io::ErrorKind::NotFound => "FileNotFoundError",
        io::ErrorKind::PermissionDenied => "PermissionError",
        io::ErrorKind::InvalidData => "UnicodeDecodeError",
        _ => "OSError",
    }
    .into()
}
pub fn write(p: &Path, v: &Value) -> Result<(), String> {
    if symlink(p) {
        return Err(err("Registro simbólico recusado"));
    }
    fs::create_dir_all(p.parent().ok_or("OSError")?).map_err(os_error)?;
    let mut text = serde_json::to_string_pretty(v).map_err(|_| err("ValueError"))?;
    text.push('\n');
    #[cfg(windows)]
    let text = text.replace('\n', "\r\n");
    crate::runtime::queue::atomic_write(p, text.as_bytes()).map_err(os_error)
}
pub struct Guard(File);
impl Drop for Guard {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}
pub async fn lock(p: &Path) -> Result<Guard, String> {
    if symlink(p) {
        return Err(err("Estado gerenciado simbólico recusado"));
    }
    fs::create_dir_all(p.parent().ok_or("OSError")?).map_err(os_error)?;
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(p)
        .map_err(os_error)?;
    #[cfg(windows)]
    let started = std::time::Instant::now();
    loop {
        #[cfg(windows)]
        if started.elapsed() >= Duration::from_secs(10) {
            return Err(err("PermissionError"));
        }
        match f.try_lock() {
            Ok(()) => return Ok(Guard(f)),
            Err(std::fs::TryLockError::WouldBlock) => {
                tokio::time::sleep(Duration::from_millis(25)).await
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(os_error(e)),
        }
    }
}
pub struct NativeRunner;
impl Runner for NativeRunner {
    fn run<'a>(
        &'a self,
        args: Vec<String>,
        cwd: &'a Path,
        env: &'a BTreeMap<String, String>,
        timeout: Duration,
    ) -> BoxFuture<'a, Result<Output, String>> {
        Box::pin(async move {
            let mut program = PathBuf::from(&args[0]);
            if args[0] == "omp" && args.get(1).is_some_and(|a| a == "config") {
                let found = env.get("PATH").is_some_and(|value| {
                    std::env::split_paths(value).any(|dir| {
                        dir.join(if cfg!(windows) { "omp.exe" } else { "omp" })
                            .is_file()
                    })
                });
                if !found {
                    program = cwd.join(".local/bin/omp");
                }
            }
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(&args[1..])
                .current_dir(cwd)
                .env_clear()
                .envs(env)
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            #[cfg(unix)]
            cmd.process_group(0);
            let mut child = cmd.spawn().map_err(os_error)?;
            let pid = child.id();
            let mut out = child.stdout.take().ok_or("OSError")?;
            let mut stderr = child.stderr.take().ok_or("OSError")?;
            let waited = tokio::time::timeout(timeout, async {
                let read_out = async {
                    let mut bytes = vec![];
                    out.read_to_end(&mut bytes).await.map(|_| bytes)
                };
                let read_error = async {
                    let mut bytes = vec![];
                    stderr.read_to_end(&mut bytes).await.map(|_| bytes)
                };
                let (status, bytes, errors) = tokio::try_join!(child.wait(), read_out, read_error)?;
                Ok::<_, std::io::Error>(Output {
                    code: exit_code(status),
                    stdout: String::from_utf8_lossy(&bytes).into_owned(),
                    stderr: String::from_utf8_lossy(&errors).into_owned(),
                })
            })
            .await;
            match waited {
                Ok(result) => result.map_err(os_error),
                Err(_) => {
                    #[cfg(unix)]
                    if let Some(id) = pid {
                        unsafe {
                            libc::kill(-(id as i32), libc::SIGKILL);
                        }
                    }
                    #[cfg(not(unix))]
                    let _ = pid;
                    let _ = child.kill().await;
                    let _ = child.wait().await;
                    Err(err("TimeoutExpired"))
                }
            }
        })
    }
}

fn exit_code(status: std::process::ExitStatus) -> i64 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        status
            .code()
            .map(i64::from)
            .unwrap_or_else(|| -i64::from(status.signal().unwrap_or(1)))
    }
    #[cfg(windows)]
    {
        status.code().map(|v| i64::from(v as u32)).unwrap_or(-1)
    }
    #[cfg(not(any(unix, windows)))]
    {
        i64::from(status.code().unwrap_or(-1))
    }
}
