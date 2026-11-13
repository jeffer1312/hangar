//! Reparo do plugin ligado da pasta principal (`~/.claude`) cuja pasta de instalação não existe.
//!
//! A atualização automática do Claude Code às vezes grava no `installed_plugins.json` uma versão
//! cuja pasta nunca chega ao cache. A partir daí toda sessão nova abre sem o plugin, e o
//! `/reload-plugins` carrega do clone do marketplace sem consertar o registro. O registro é um só:
//! a pasta `plugins` de cada conta é link para a do principal, então reparar o principal repara
//! todas. Aqui ficam o observador, o agendamento e a política; quem lê o registro, reinstala e
//! registra é o servidor, pelo [`RepairHost`].
use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::watch;

use crate::{OperationResult, PluginId, PluginsError};

const REGISTRY: &str = "installed_plugins.json";
/// Silêncio exigido do registro antes de conferir: a CLI grava o registro antes de terminar de
/// preencher o cache.
const SETTLE: Duration = Duration::from_secs(30);
/// A pasta pode sumir sem o registro mudar.
const PERIOD: Duration = Duration::from_secs(5 * 60);
/// Plugin cuja pasta a reinstalação não trouxe só é tentado de novo depois disto, para não rodar a
/// CLI em laço (a própria reinstalação regrava o registro e acorda o observador). A espera dobra a
/// cada falha seguida, até [`RETRY_MAX`]: marketplace removido ou plugin que pede revisão não voltam sozinhos.
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);
const RETRY_MAX: Duration = Duration::from_secs(24 * 60 * 60);

pub trait RepairHost {
    /// Plugins ligados com instalação de usuário sem pasta; `None` quando a leitura falhou.
    fn missing(&self) -> impl Future<Output = Option<Vec<PluginId>>> + Send;
    /// Reinstala sob o cadeado das mudanças de plugin, conferindo antes, já com ele, que o plugin
    /// ainda precisa; `None` quando não precisava mais.
    fn reinstall(&self, id: &PluginId) -> impl Future<Output = Option<Result<OperationResult, PluginsError>>> + Send;
    fn record(&self, outcome: &Outcome);
}

pub struct Outcome {
    pub id: PluginId,
    pub result: Result<OperationResult, PluginsError>,
    /// A releitura depois da passada confirmou a pasta de volta.
    pub fixed: bool,
}

/// Confere na subida, quando o registro em `plugins_dir` muda e a cada [`PERIOD`]; nunca termina.
pub async fn run(host: impl RepairHost, plugins_dir: PathBuf) {
    let (changes, mut changed) = watch::channel(());
    // O emissor original fica vivo aqui: sem observador, o `changed()` só espera o período.
    let watcher = watch_registry(&plugins_dir, changes.clone());
    let mut repairer = Repairer::default();
    let mut woken = true;
    loop {
        // Sem observador nenhuma conferência sabe se a CLI acabou de gravar: todas esperam.
        if woken || watcher.is_none() {
            settle(&mut changed, SETTLE).await;
        }
        for outcome in repairer.pass(&host, Instant::now()).await {
            host.record(&outcome);
        }
        woken = tokio::time::timeout(PERIOD, changed.changed()).await.is_ok();
    }
}

/// Espera `quiet` sem mudança nova; cada mudança no meio recomeça a espera.
async fn settle(changed: &mut watch::Receiver<()>, quiet: Duration) {
    changed.borrow_and_update();
    while matches!(tokio::time::timeout(quiet, changed.changed()).await, Ok(Ok(()))) {}
}

fn watch_registry(dir: &Path, changes: watch::Sender<()>) -> Option<RecommendedWatcher> {
    let watcher = (|| {
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            let registry = |path: &PathBuf| path.file_name().is_some_and(|name| name == REGISTRY);
            if event.need_rescan() || (!event.kind.is_access() && event.paths.iter().any(registry)) {
                changes.send_replace(());
            }
        })?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        notify::Result::Ok(watcher)
    })();
    watcher.inspect_err(|error| tracing::warn!(code = "plugins_repair_watch_failed", %error, "plugins: sem observador do registro")).ok()
}

/// Quando falhou por último e quantas vezes seguidas.
#[derive(Default)]
struct Repairer {
    failed: HashMap<String, (Instant, u32)>,
}

impl Repairer {
    /// Reinstala um por vez, pulando o que não voltou há menos de [`RETRY_AFTER`], e confere as
    /// pastas numa releitura só no fim.
    async fn pass(&mut self, host: &impl RepairHost, now: Instant) -> Vec<Outcome> {
        let Some(missing) = host.missing().await else { return vec![] };
        let mut attempts = Vec::new();
        for id in missing {
            if self.failed.get(id.as_str()).is_some_and(|(at, times)| now.duration_since(*at) < retry_after(*times)) {
                continue;
            }
            if let Some(result) = host.reinstall(&id).await {
                attempts.push((id, result));
            }
        }
        if attempts.is_empty() {
            return vec![];
        }
        let left = host.missing().await;
        attempts
            .into_iter()
            .map(|(id, result)| {
                let fixed = result.is_ok() && left.as_ref().is_some_and(|left| !left.contains(&id));
                if fixed {
                    self.failed.remove(id.as_str());
                } else {
                    let times = self.failed.get(id.as_str()).map_or(1, |(_, times)| times + 1);
                    self.failed.insert(id.as_str().to_owned(), (now, times));
                }
                Outcome { id, result, fixed }
            })
            .collect()
    }
}

fn retry_after(times: u32) -> Duration {
    RETRY_AFTER.saturating_mul(1 << times.saturating_sub(1).min(16)).min(RETRY_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn id(raw: &str) -> PluginId {
        raw.parse().unwrap()
    }

    fn ok() -> Result<OperationResult, PluginsError> {
        Ok(OperationResult { outcome: "ok".into(), message: String::new(), old_version: None, new_version: None })
    }

    /// `missing` devolve as leituras na ordem; a última se repete.
    struct Fake {
        reads: Mutex<Vec<Option<Vec<PluginId>>>>,
        needed: bool,
        result: fn() -> Result<OperationResult, PluginsError>,
        installs: Mutex<usize>,
    }

    impl Fake {
        fn new(reads: Vec<Option<Vec<PluginId>>>) -> Self {
            Self { reads: Mutex::new(reads), needed: true, result: ok, installs: Mutex::new(0) }
        }
        fn installs(&self) -> usize {
            *self.installs.lock().unwrap()
        }
    }

    impl RepairHost for Fake {
        async fn missing(&self) -> Option<Vec<PluginId>> {
            let mut reads = self.reads.lock().unwrap();
            if reads.len() > 1 { reads.remove(0) } else { reads[0].clone() }
        }
        async fn reinstall(&self, _: &PluginId) -> Option<Result<OperationResult, PluginsError>> {
            if !self.needed {
                return None;
            }
            *self.installs.lock().unwrap() += 1;
            Some((self.result)())
        }
        fn record(&self, _: &Outcome) {}
    }

    #[tokio::test]
    async fn a_reinstall_that_leaves_the_folder_missing_waits_before_trying_again() {
        let mut repairer = Repairer::default();
        let start = Instant::now();
        let stuck = Fake::new(vec![Some(vec![id("a@m")])]);
        assert!(!repairer.pass(&stuck, start).await[0].fixed);
        assert!(repairer.pass(&stuck, start + Duration::from_secs(60)).await.is_empty());
        assert_eq!(stuck.installs(), 1);
        let back = Fake::new(vec![Some(vec![id("a@m")]), Some(vec![])]);
        assert!(repairer.pass(&back, start + RETRY_AFTER).await[0].fixed);
        assert!(repairer.failed.is_empty());
    }

    #[tokio::test]
    async fn a_failed_read_neither_repairs_nor_confirms() {
        let mut repairer = Repairer::default();
        let unreadable = Fake::new(vec![None]);
        assert!(repairer.pass(&unreadable, Instant::now()).await.is_empty());
        assert_eq!(unreadable.installs(), 0);
        let reread_fails = Fake::new(vec![Some(vec![id("a@m")]), None]);
        assert!(!repairer.pass(&reread_fails, Instant::now()).await[0].fixed);
    }

    #[tokio::test]
    async fn a_plugin_no_longer_needed_under_the_lock_is_not_counted() {
        let mut repairer = Repairer::default();
        let gone = Fake { needed: false, ..Fake::new(vec![Some(vec![id("a@m")])]) };
        assert!(repairer.pass(&gone, Instant::now()).await.is_empty());
        assert!(repairer.failed.is_empty());
    }

    #[tokio::test]
    async fn a_failed_cli_waits_before_trying_again() {
        let mut repairer = Repairer::default();
        let failing = Fake { result: || Err(PluginsError::CliTimeout), ..Fake::new(vec![Some(vec![id("a@m")])]) };
        let start = Instant::now();
        assert!(repairer.pass(&failing, start).await[0].result.is_err());
        assert!(repairer.pass(&failing, start + Duration::from_secs(60)).await.is_empty());
        // A segunda falha seguida dobra a espera.
        let second = start + RETRY_AFTER;
        assert_eq!(repairer.pass(&failing, second).await.len(), 1);
        assert!(repairer.pass(&failing, second + RETRY_AFTER).await.is_empty());
        assert_eq!(repairer.pass(&failing, second + 2 * RETRY_AFTER).await.len(), 1);
        assert_eq!(failing.installs(), 3);
    }

    #[test]
    fn the_wait_doubles_up_to_a_day() {
        assert_eq!(retry_after(1), RETRY_AFTER);
        assert_eq!(retry_after(2), 2 * RETRY_AFTER);
        assert_eq!(retry_after(40), RETRY_MAX);
    }

    #[tokio::test(start_paused = true)]
    async fn each_change_restarts_the_quiet_wait() {
        let (changes, mut changed) = watch::channel(());
        let start = tokio::time::Instant::now();
        let waiting = tokio::spawn(async move {
            settle(&mut changed, SETTLE).await;
            tokio::time::Instant::now()
        });
        tokio::time::sleep(Duration::from_secs(29)).await;
        changes.send_replace(());
        let done = waiting.await.unwrap();
        assert_eq!(done - start, Duration::from_secs(29) + SETTLE);
    }

    #[tokio::test]
    async fn rewriting_the_registry_wakes_the_repair() {
        let root = tempfile::tempdir().unwrap();
        let (changes, mut changed) = watch::channel(());
        let _watcher = watch_registry(root.path(), changes).unwrap();
        let temporary = root.path().join(".installed_plugins.json.tmp");
        std::fs::write(&temporary, "{}").unwrap();
        std::fs::rename(&temporary, root.path().join(REGISTRY)).unwrap();
        tokio::time::timeout(Duration::from_secs(5), changed.changed()).await.unwrap().unwrap();
    }
}
