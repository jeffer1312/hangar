//! Uma fonte por provedor; a lista vencida permanece utilizável enquanto a CLI renova.
use super::{ModelError, pure};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, LazyLock, Mutex},
    time::Duration,
};
use tokio::{io::AsyncReadExt, time::Instant};

pub type CatalogFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Value>, ModelError>> + Send + 'a>>;
pub trait Source: Send + Sync {
    fn read<'a>(&'a self, provider: &'a str) -> CatalogFuture<'a>;
}
#[derive(Default)]
pub struct Programs;
fn program(provider: &str) -> Option<PathBuf> {
    if !["pi", "omp"].contains(&provider) {
        return None;
    }
    program_from(provider, &std::env::vars().collect::<Vec<_>>())
}
pub(super) fn program_from(provider: &str, environment: &[(String, String)]) -> Option<PathBuf> {
    if !["pi", "omp"].contains(&provider) {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = environment
            .iter()
            .find(|(key, _)| key == "PATH")?
            .1
            .as_str();
        std::env::split_paths(path)
            .map(|dir| dir.join(provider))
            .find(|file| {
                std::fs::metadata(file)
                    .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
            })
    }
    #[cfg(not(unix))]
    {
        crate::runtime::process::resolve_program(provider, environment.iter().map(|(k, v)| (k, v)))
    }
}

pub(super) fn global_error(provider: &str, message: String, absent: bool) -> ModelError {
    let code = format!(
        "erro_{provider}_{}",
        if absent { "ausente" } else { "list_models" }
    );
    let msg = if absent {
        message.clone()
    } else {
        format!(
            "{} falhou: {message}",
            if provider == "omp" {
                "omp models --json"
            } else {
                "pi --list-models"
            }
        )
    };
    ModelError::coded(502, &code, &msg, json!({"erro":message}))
}
impl Source for Programs {
    fn read<'a>(&'a self, provider: &'a str) -> CatalogFuture<'a> {
        read_from(provider, std::env::vars().collect())
    }
}
pub(super) fn read_from<'a>(
    provider: &'a str,
    environment: Vec<(String, String)>,
) -> CatalogFuture<'a> {
    Box::pin(async move {
        let executable=program_from(provider, &environment).ok_or_else(||global_error(provider,format!("nao achei o executavel `{provider}` no PATH deste servidor — {}",if provider=="omp"{"instale o oh-my-pi ou ajuste o PATH do backend"}else{"instale o Pi (npm i -g @earendil-works/pi-coding-agent) ou ajuste o PATH do backend"}),true))?;
        let args: Vec<&str> = if provider == "omp" {
            vec!["models", "--json"]
        } else {
            vec!["--list-models"]
        };
        let mut command = crate::terminal_input::child_command(executable.as_os_str());
        command
            .env_clear()
            .envs(environment)
            .args(&args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        for key in crate::terminal_process::PRIVATE_ENV_KEYS {
            command.env_remove(key);
        }
        let mut tree = crate::terminal_process::CommandTree::configure(&mut command)
            .map_err(|e| global_error(provider, e.to_string(), false))?;
        let mut child = command
            .spawn()
            .map_err(|e| global_error(provider, e.to_string(), false))?;
        if let Err(error) = tree.attach(&child) {
            crate::terminal_process::finish(&mut tree).await;
            let _ = child.wait().await;
            return Err(global_error(provider, error.to_string(), false));
        }
        let mut stdout = child.stdout.take().ok_or_else(|| {
            global_error(provider, "saída do catálogo indisponível".into(), false)
        })?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| global_error(provider, "erro do catálogo indisponível".into(), false))?;
        let collected = tokio::time::timeout(Duration::from_secs(30), async {
            let mut out = vec![];
            let mut err = vec![];
            let (a, b, c) = tokio::join!(
                stdout.read_to_end(&mut out),
                stderr.read_to_end(&mut err),
                crate::terminal_process::leader_exited(&mut child)
            );
            a?;
            b?;
            c?;
            Ok::<_, std::io::Error>((out, err))
        })
        .await;
        crate::terminal_process::finish(&mut tree).await;
        let status = child
            .wait()
            .await
            .map_err(|e| global_error(provider, e.to_string(), false))?;
        let (out, err) = match collected {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return Err(global_error(provider, e.to_string(), false)),
            Err(_) => {
                return Err(global_error(
                    provider,
                    format!(
                        "Command '[{}, {}]' timed out after 30 seconds",
                        pure::repr(&executable.to_string_lossy()),
                        args.iter()
                            .map(|arg| pure::repr(arg))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                    false,
                ));
            }
        };
        let stdout = String::from_utf8_lossy(&out);
        let stderr = String::from_utf8_lossy(&err);
        if !status.success() {
            return Err(global_error(
                provider,
                if stderr.trim().is_empty() {
                    format!("{} falhou", args.join(" "))
                } else {
                    stderr.trim().into()
                },
                false,
            ));
        }
        let models = if provider == "omp" {
            pure::parse_omp(&stdout)
        } else {
            pure::parse_pi(&stdout)
        };
        validate_models(provider, &args.join(" "), &stdout, models)
            .map_err(|m| global_error(provider, m, false))
    })
}
pub fn validate_models(
    provider: &str,
    args: &str,
    stdout: &str,
    models: Vec<Value>,
) -> Result<Vec<Value>, String> {
    if models.is_empty() {
        let first = stdout
            .trim()
            .lines()
            .next()
            .unwrap_or("")
            .chars()
            .take(120)
            .collect::<String>();
        return Err(format!(
            "{args} nao devolveu modelo nenhum (1a linha: {})",
            pure::repr(&first)
        ));
    }
    let bad: Vec<_> = models
        .iter()
        .filter(|m| {
            m["id"].as_str().is_some_and(|s| s.contains('\u{fffd}'))
                || m["provider"]
                    .as_str()
                    .is_some_and(|s| s.contains('\u{fffd}'))
        })
        .map(|m| m["id"].as_str().unwrap_or(""))
        .collect();
    if !bad.is_empty() {
        let examples = bad
            .iter()
            .take(3)
            .map(|v| pure::repr(v))
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "{provider} devolveu {} id/provider com byte ilegivel (ex: [{examples}])",
            bad.len()
        ));
    }
    Ok(models)
}
struct Cached {
    at: Instant,
    models: Vec<Value>,
}
#[derive(Default)]
struct Entry {
    cached: Mutex<Option<Cached>>,
    refreshing: std::sync::atomic::AtomicBool,
    cold: tokio::sync::Mutex<()>,
}
pub struct Catalogs {
    source: Arc<dyn Source>,
    entries: Mutex<HashMap<String, Arc<Entry>>>,
    ttl: Duration,
}
impl Catalogs {
    pub fn new(source: Arc<dyn Source>, ttl: Duration) -> Arc<Self> {
        Arc::new(Self {
            source,
            entries: Mutex::default(),
            ttl,
        })
    }
    fn entry(&self, provider: &str) -> Arc<Entry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(provider.into())
            .or_default()
            .clone()
    }
    async fn refresh(&self, provider: &str, entry: &Entry) -> Result<Vec<Value>, ModelError> {
        let models = self.source.read(provider).await?;
        *entry.cached.lock().unwrap_or_else(|e| e.into_inner()) = Some(Cached {
            at: Instant::now(),
            models: models.clone(),
        });
        Ok(models)
    }
    pub fn renew(self: &Arc<Self>, provider: &str) {
        let entry = self.entry(provider);
        if entry
            .refreshing
            .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        let this = self.clone();
        let provider = provider.to_owned();
        tokio::spawn(async move {
            struct Reset(Arc<Entry>);
            impl Drop for Reset {
                fn drop(&mut self) {
                    self.0
                        .refreshing
                        .store(false, std::sync::atomic::Ordering::Release);
                }
            }
            let _reset = Reset(entry.clone());
            let _cold = entry.cold.lock().await;
            if this.refresh(&provider, &entry).await.is_err() {
                tracing::warn!(
                    code = "provider_catalog_refresh_failed",
                    provider,
                    "catálogo em segundo plano falhou; leitura anterior conservada"
                );
            }
        });
    }
    pub async fn read(self: &Arc<Self>, provider: &str) -> Result<Vec<Value>, ModelError> {
        if !["pi", "omp"].contains(&provider) {
            return Err(ModelError::text(400, "provider inválido"));
        }
        let entry = self.entry(provider);
        let hit = entry
            .cached
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .map(|hit| (hit.at.elapsed() >= self.ttl, hit.models.clone()));
        if let Some((stale, models)) = hit {
            if stale {
                self.renew(provider);
            }
            return Ok(models);
        }
        let this = self.clone();
        let provider = provider.to_owned();
        // Cancelar o HTTP não larga um processo de catálogo nem os auxiliares que ele abriu.
        tokio::spawn(async move {
            let _cold = entry.cold.lock().await;
            if let Some(hit) = entry
                .cached
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_ref()
            {
                return Ok(hit.models.clone());
            }
            this.refresh(&provider, &entry).await
        })
        .await
        .map_err(|_| ModelError::text(502, "o catálogo falhou antes de publicar a leitura"))?
    }
}
pub static CATALOGS: LazyLock<Arc<Catalogs>> =
    LazyLock::new(|| Catalogs::new(Arc::new(Programs), Duration::from_secs(600)));
pub async fn catalog(provider: &str) -> Result<Vec<Value>, ModelError> {
    CATALOGS.read(provider).await
}
pub fn warm() {
    for provider in ["pi", "omp"] {
        if program(provider).is_some() {
            CATALOGS.renew(provider);
        }
    }
}
