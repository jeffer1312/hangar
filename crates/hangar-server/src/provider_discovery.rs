//! Descoberta dos CLIs no PATH do shell de login, sem executar os agentes.
use crate::accounts::{environment::AccountEnvironment, quotas::other::py_strip};
use std::{
    collections::BTreeMap,
    sync::{Arc, LazyLock, Mutex},
    time::{Duration, Instant},
};
#[derive(serde::Serialize, Clone, Debug)]
pub struct Availability {
    #[serde(rename = "disponivel")]
    pub available: bool,
    #[serde(rename = "motivo")]
    pub reason: Option<&'static str>,
}
pub type Providers = BTreeMap<String, Availability>;
type Slot<T> = Arc<tokio::sync::Mutex<Option<T>>>;
#[derive(Default)]
pub struct Discovery {
    paths: Mutex<BTreeMap<[u8; 32], Slot<String>>>,
    probes: Mutex<BTreeMap<[u8; 32], Slot<(Instant, Providers)>>>,
}
static SHARED: LazyLock<Discovery> = LazyLock::new(Discovery::default);
pub async fn login_path(env: &AccountEnvironment) -> String {
    SHARED.login_path(env).await
}
pub async fn providers(env: &AccountEnvironment) -> Result<Providers, String> {
    SHARED.providers(env).await
}

impl Discovery {
    pub async fn login_path(&self, env: &AccountEnvironment) -> String {
        let key = crate::cli_probe::environment_key(env);
        let slot = self
            .paths
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key)
            .or_default()
            .clone();
        let mut slot = slot.lock().await;
        if let Some(path) = slot.as_ref() {
            return path.clone();
        }
        let fallback = env.base.get("PATH").cloned().unwrap_or_default();
        let path = if cfg!(windows) {
            fallback
        } else {
            let shell = env
                .base
                .get("SHELL")
                .map(String::as_str)
                .unwrap_or("/bin/sh");
            match crate::command_capture::capture_with_account_env(
                vec![
                    shell.into(),
                    "-l".into(),
                    "-c".into(),
                    "printenv PATH".into(),
                ],
                env.clone(),
                Duration::from_secs(5),
            )
            .await
            {
                Ok(output) if output.success && !py_strip(&output.stdout).is_empty() => {
                    py_strip(&output.stdout).to_owned()
                }
                _ => {
                    tracing::debug!(
                        "PATH do shell de login indisponível; usando o ambiente capturado"
                    );
                    fallback
                }
            }
        };
        *slot = Some(path.clone());
        path
    }
    pub async fn providers(&self, env: &AccountEnvironment) -> Result<Providers, String> {
        let key = crate::cli_probe::environment_key(env);
        let slot = self
            .probes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(key)
            .or_default()
            .clone();
        let mut slot = slot.lock().await;
        if let Some((at, providers)) = slot.as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return Ok(providers.clone());
            }
        }
        let path = self.login_path(env).await;
        let env = env.clone();
        let providers = tokio::task::spawn_blocking(move || scan(&env, &path, cfg!(windows)))
            .await
            .map_err(|_| "falha ao procurar executáveis dos provedores".to_string())?;
        *slot = Some((Instant::now(), providers.clone()));
        Ok(providers)
    }
    pub fn invalidate(&self) {
        self.probes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

/// O parâmetro de plataforma permite conferir os candidatos Windows sem executar um CLI.
#[doc(hidden)]
pub fn scan(env: &AccountEnvironment, path: &str, windows: bool) -> Providers {
    let directories: Vec<_> = path
        .split(if windows { ';' } else { ':' })
        .filter(|dir| !dir.is_empty())
        .collect();
    let extensions: Vec<_> = env
        .base
        .get("PATHEXT")
        .map(String::as_str)
        .unwrap_or("")
        .split(';')
        .map(py_strip)
        .filter(|ext| {
            matches!(
                ext.to_ascii_lowercase().as_str(),
                ".exe" | ".cmd" | ".bat" | ".com"
            )
        })
        .collect();
    ["claude", "codex", "pi", "omp", "kimi"]
        .into_iter()
        .map(|name| {
            let (mut available, mut denied) = (false, false);
            'directories: for directory in &directories {
                let base = std::path::Path::new(directory);
                let mut candidates = Vec::new();
                if windows {
                    for ext in &extensions {
                        candidates.push((base.join(format!("{name}{ext}")), false));
                    }
                }
                candidates.push((base.join(name), windows));
                for (path, extensionless) in candidates {
                    if !path.is_file() {
                        continue;
                    }
                    if extensionless || !executable(&path) {
                        denied = true;
                        continue;
                    }
                    available = true;
                    break 'directories;
                }
            }
            (
                name.into(),
                Availability {
                    available,
                    reason: if available {
                        None
                    } else if denied {
                        Some("sem_permissao")
                    } else {
                        Some("nao_encontrado")
                    },
                },
            )
        })
        .collect()
}
fn executable(path: &std::path::Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        std::ffi::CString::new(path.as_os_str().as_bytes())
            .is_ok_and(|path| unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}
