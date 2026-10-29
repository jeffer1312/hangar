//! Uma captura de capacidade por CLI, compartilhada entre instalação e diagnóstico.
use crate::accounts::environment::AccountEnvironment;
use crate::command_capture::{Output, capture};
use num_bigint::BigInt;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::time::Instant;
type Cached = Option<(Instant, Result<Option<Output>, String>)>;
type Slot = Arc<tokio::sync::Mutex<Cached>>;

#[derive(Clone)]
pub struct Probe {
    cache: Arc<Mutex<BTreeMap<(String, String), Slot>>>,
    pub ttl: Duration,
    pub timeout: Duration,
}
impl Default for Probe {
    fn default() -> Self {
        Self {
            cache: Default::default(),
            ttl: Duration::from_secs(600),
            timeout: Duration::from_secs(10),
        }
    }
}
impl Probe {
    pub fn invalidate_cli(&self, cli: &str) {
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(name, _), _| name != cli);
    }
    pub fn invalidate(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
    async fn raw(
        &self,
        env: &AccountEnvironment,
        cli: &str,
        arg: &str,
    ) -> Result<Option<Output>, String> {
        let slot = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry((cli.into(), arg.into()))
            .or_default()
            .clone();
        let mut slot = slot.lock().await;
        if let Some((at, result)) = &*slot {
            if at.elapsed() < self.ttl {
                return result.clone();
            }
        }
        let program = resolve(cli, env);
        let result = match program {
            None => Ok(None),
            Some(program) => capture(
                vec![program.to_string_lossy().into_owned(), arg.into()],
                env.clone(),
                self.timeout,
            )
            .await
            .map(Some),
        };
        *slot = Some((Instant::now(), result.clone()));
        result
    }
    pub async fn version(&self, env: &AccountEnvironment) -> Option<Vec<BigInt>> {
        parse_version(&self.raw(env, "claude", "--version").await.ok()??.stdout)
    }
    pub async fn supports_plugin_dir(&self, env: &AccountEnvironment) -> bool {
        self.raw(env, "claude", "--help")
            .await
            .ok()
            .flatten()
            .is_some_and(|out| out.stdout.contains("--plugin-dir"))
    }
    pub async fn mods_enabled(&self, env: &AccountEnvironment) -> bool {
        self.version(env)
            .await
            .is_some_and(|version| version >= vec![2.into(), 1.into(), 287.into()])
            || self.supports_plugin_dir(env).await
    }
    pub async fn version_text(&self, env: &AccountEnvironment, cli: &str) -> Option<String> {
        match self
            .raw(env, cli, if cli == "tmux" { "-V" } else { "--version" })
            .await
        {
            Ok(None) => None,
            Err(error) => {
                tracing::warn!(cli,%error,"Sonda de versão falhou");
                Some(String::new())
            }
            Ok(Some(out)) => {
                let raw = if out.stdout.is_empty() {
                    out.stderr
                } else {
                    out.stdout
                };
                Some(
                    raw.trim()
                        .split([
                            '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{85}',
                            '\u{2028}', '\u{2029}',
                        ])
                        .next()
                        .unwrap_or("")
                        .trim()
                        .into(),
                )
            }
        }
    }
}
fn resolve(cli: &str, env: &AccountEnvironment) -> Option<std::path::PathBuf> {
    #[cfg(windows)]
    {
        crate::runtime::process::resolve_program(cli, env.base.iter())
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let executable = |path: &std::path::Path| {
            path.is_file()
                && std::ffi::CString::new(path.as_os_str().as_bytes())
                    .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0)
        };
        let path = std::path::Path::new(cli);
        if path.components().count() > 1 {
            return executable(path).then(|| path.to_path_buf());
        }
        std::env::split_paths(
            env.base
                .get("PATH")
                .map(String::as_str)
                .unwrap_or("/bin:/usr/bin"),
        )
        .map(|path| path.join(cli))
        .find(|path| executable(path))
    }
}
pub fn parse_version(raw: &str) -> Option<Vec<BigInt>> {
    static VERSION: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^\s*(\d+)\.(\d+)\.(\d+)").unwrap());
    let parts = VERSION.captures(raw)?;
    (1..=3)
        .map(|index| {
            let ascii: String = parts[index]
                .chars()
                .map(|c| crate::orq_conductor::decimal(c).map(|n| char::from(b'0' + n)))
                .collect::<Option<_>>()?;
            ascii.parse().ok()
        })
        .collect()
}
