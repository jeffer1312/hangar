//! Cache persistente e sondagem compartilhada por conta e versão do CLI.
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::time::Instant;

pub type ProbeResult = Result<Vec<Value>, &'static str>;

const FIRST_WAIT: Duration = Duration::from_secs(20);
const KEPT_CONTEXTS: usize = 64;

pub trait Probe: Send + Sync {
    fn cache_key(&self, config_dir: Option<&Path>) -> String {
        config_dir
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
    fn signature(&self, config_dir: Option<&Path>) -> Result<String, &'static str>;
    fn run<'a>(
        &'a self,
        config_dir: Option<&'a Path>,
    ) -> Pin<Box<dyn Future<Output = ProbeResult> + Send + 'a>>;
}

type Key = (PathBuf, String);
type Completion = tokio::sync::watch::Receiver<Option<ProbeResult>>;
enum Status {
    Running(Completion),
    Failed(Instant),
}
pub struct Cache {
    path: PathBuf,
    attempts: Mutex<HashMap<Key, Status>>,
    writer: Mutex<()>,
}

struct Lookup {
    commands: Option<Vec<Value>>,
    pending: Option<Completion>,
}

pub fn valid_commands(value: &Value) -> Vec<Value> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| c.is_object() && c.get("name").is_some_and(Value::is_string))
        .cloned()
        .collect()
}

fn read(path: &Path) -> Value {
    match crate::runtime::queue::read_replaced(path) {
        Ok(raw) => match serde_json::from_slice::<Value>(&raw) {
            Ok(value) if value.is_object() => value,
            _ => {
                tracing::warn!(
                    code = "claude_commands_cache_invalid",
                    "cache de comandos inválido; a CLI será sondada novamente"
                );
                json!({})
            }
        },
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(code = "claude_commands_cache_unreadable", io_kind = ?error.kind(), "cache de comandos ilegível");
            }
            json!({})
        }
    }
}

impl Cache {
    pub fn new(home: &Path) -> Self {
        Self {
            path: home.join(".hangar/claude-slash-commands.json"),
            attempts: Mutex::default(),
            writer: Mutex::default(),
        }
    }

    pub async fn get(
        self: &Arc<Self>,
        probe: Arc<dyn Probe>,
        config_dir: Option<PathBuf>,
    ) -> Option<Vec<Value>> {
        self.lookup(probe, config_dir).await.commands
    }

    /// A primeira abertura do menu espera a consulta compartilhada, em vez de fixar a reserva.
    pub async fn get_ready(
        self: &Arc<Self>,
        probe: Arc<dyn Probe>,
        config_dir: Option<PathBuf>,
    ) -> Option<Vec<Value>> {
        let found = self.lookup(probe, config_dir).await;
        if found.commands.is_some() {
            return found.commands;
        }
        let mut pending = found.pending?;
        // Abaixo do prazo do cliente: consulta lenta entrega a reserva e termina em segundo plano.
        let Ok(waited) = tokio::time::timeout(FIRST_WAIT, pending.wait_for(Option::is_some)).await
        else {
            tracing::warn!(
                code = "claude_commands_probe_slow",
                "sondagem do catálogo passou do prazo da primeira abertura; usando a lista de reserva"
            );
            return None;
        };
        let result = waited.ok()?.clone();
        result?.ok()
    }

    async fn lookup(
        self: &Arc<Self>,
        probe: Arc<dyn Probe>,
        config_dir: Option<PathBuf>,
    ) -> Lookup {
        let cache = self.clone();
        let runtime = tokio::runtime::Handle::current();
        match tokio::task::spawn_blocking(move || cache.load_or_probe(probe, config_dir, runtime))
            .await
        {
            Ok(Ok(result)) => result,
            _ => {
                tracing::warn!(
                    code = "claude_commands_cli_unavailable",
                    "CLI de comandos não encontrada ou ilegível; usando o catálogo de reserva"
                );
                Lookup {
                    commands: None,
                    pending: None,
                }
            }
        }
    }

    fn load_or_probe(
        self: &Arc<Self>,
        probe: Arc<dyn Probe>,
        config_dir: Option<PathBuf>,
        runtime: tokio::runtime::Handle,
    ) -> Result<Lookup, &'static str> {
        let signature = probe.signature(config_dir.as_deref())?;
        // Uma leitura anterior à gravação não pode registrar outra sonda depois dela.
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let cache = read(&self.path);
        let account = probe.cache_key(config_dir.as_deref());
        let entry = &cache[&account];
        let commands = valid_commands(&entry["comandos"]);
        if entry["assinatura"].as_str() == Some(&signature) && !commands.is_empty() {
            return Ok(Lookup {
                commands: Some(commands),
                pending: None,
            });
        }
        let key = (config_dir.clone().unwrap_or_default(), signature);
        let (completed, pending) = tokio::sync::watch::channel(None);
        {
            let mut attempts = self.attempts.lock().unwrap_or_else(|e| e.into_inner());
            match attempts.get(&key) {
                Some(Status::Running(pending)) => {
                    return Ok(Lookup {
                        commands: None,
                        pending: Some(pending.clone()),
                    });
                }
                Some(Status::Failed(at)) if at.elapsed() < Duration::from_secs(600) => {
                    return Ok(Lookup {
                        commands: None,
                        pending: None,
                    });
                }
                _ => {}
            }
            attempts.insert(key.clone(), Status::Running(pending.clone()));
        }
        self.start_probe(probe, config_dir, key, runtime, completed, account);
        Ok(Lookup {
            commands: None,
            pending: Some(pending),
        })
    }

    fn start_probe(
        self: &Arc<Self>,
        probe: Arc<dyn Probe>,
        config_dir: Option<PathBuf>,
        key: Key,
        runtime: tokio::runtime::Handle,
        completed: tokio::sync::watch::Sender<Option<ProbeResult>>,
        scope: String,
    ) {
        let cache = self.clone();
        runtime.spawn(async move {
            let started = Instant::now();
            let mut attempt = Attempt {
                cache: cache.clone(),
                key: key.clone(),
                completed: false,
                result: completed,
            };
            let result = match probe.run(config_dir.as_deref()).await {
                Ok(commands) if !commands.is_empty() => {
                    let count = commands.len();
                    let writing = cache.clone();
                    let signature = key.1.clone();
                    let saved = commands.clone();
                    let stored = tokio::task::spawn_blocking(move || {
                        writing.store(&scope, &signature, saved)
                    })
                    .await;
                    match stored {
                        Ok(Ok(())) => {
                            tracing::info!(
                                code = "claude_commands_probe_ready",
                                count,
                                elapsed_ms = started.elapsed().as_millis() as u64,
                                "catálogo de comandos atualizado"
                            );
                            Ok(commands)
                        }
                        _ => Err("claude_commands_cache_write_failed"),
                    }
                }
                Ok(_) => Err("claude_commands_probe_empty"),
                Err(code) => Err(code),
            };
            let mut attempts = cache.attempts.lock().unwrap_or_else(|e| e.into_inner());
            if let Err(code) = &result {
                attempts.insert(key, Status::Failed(Instant::now()));
                tracing::warn!(
                    code,
                    "sondagem do catálogo falhou; usando a lista de reserva"
                );
            } else {
                attempts.remove(&key);
            }
            attempt.result.send_replace(Some(result));
            attempt.completed = true;
        });
    }

    fn store(&self, key: &str, signature: &str, commands: Vec<Value>) -> std::io::Result<()> {
        let _writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let mut cache = read(&self.path);
        cache[key] = json!({"assinatura":signature,"comandos":commands,"em":SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()});
        // Cada pasta de sessão vira um contexto: sem teto, o arquivo cresce a cada worktree.
        if let Some(entries) = cache.as_object_mut()
            && entries.len() > KEPT_CONTEXTS
        {
            let mut ages: Vec<(f64, String)> = entries
                .iter()
                .map(|(k, v)| (v["em"].as_f64().unwrap_or(0.0), k.clone()))
                .collect();
            ages.sort_by(|a, b| a.0.total_cmp(&b.0));
            for (_, old) in &ages[..ages.len() - KEPT_CONTEXTS] {
                entries.remove(old);
            }
        }
        std::fs::create_dir_all(self.path.parent().unwrap())?;
        crate::runtime::queue::atomic_write(&self.path, &serde_json::to_vec(&cache)?)
    }
}

struct Attempt {
    cache: Arc<Cache>,
    key: Key,
    completed: bool,
    result: tokio::sync::watch::Sender<Option<ProbeResult>>,
}
impl Drop for Attempt {
    fn drop(&mut self) {
        if !self.completed {
            self.cache
                .attempts
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(self.key.clone(), Status::Failed(Instant::now()));
            tracing::warn!(
                code = "claude_commands_probe_interrupted",
                "sondagem interrompida antes de publicar o catálogo"
            );
        }
    }
}
