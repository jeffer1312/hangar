//! Reinstala sozinho o plugin ligado da pasta principal (`~/.claude`) cuja pasta de instalação não
//! existe no disco.
//!
//! A atualização automática do Claude Code às vezes grava no `installed_plugins.json` uma versão
//! cuja pasta nunca chega ao cache. A partir daí toda sessão nova abre sem o plugin, e o
//! `/reload-plugins` carrega do clone do marketplace sem consertar o registro. O registro é um só:
//! a pasta `plugins` de cada conta é link para a do principal, então reparar aqui repara todas.
use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use hangar_plugins::{OperationResult, PluginId, PluginsError};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::json;
use tokio::sync::watch;

use crate::{
    accounts::claude_config::{PluginFinding, plugin_findings},
    routes::AppState,
};

const REGISTRY: &str = "installed_plugins.json";
/// Espera depois da subida e de cada mudança do registro: a CLI grava o registro antes de terminar
/// de preencher o cache.
const SETTLE: Duration = Duration::from_secs(30);
/// A pasta pode sumir sem o registro mudar.
const PERIOD: Duration = Duration::from_secs(5 * 60);
/// Plugin cuja pasta a reinstalação não trouxe só é tentado de novo depois disto, para não rodar a
/// CLI em laço (a própria reinstalação regrava o registro e acorda o observador).
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

pub(crate) fn spawn(state: Arc<AppState>) {
    tokio::spawn(run(state));
}

async fn run(state: Arc<AppState>) {
    let home = state.accounts.env.home.clone();
    let (changes, mut changed) = watch::channel(());
    // O emissor original fica vivo aqui: sem observador, o `changed()` só espera o período.
    let _watcher = watch_registry(&home.join(".claude").join("plugins"), changes.clone());
    let mut repairer = Repairer::default();
    let mut woken = true;
    loop {
        if woken {
            tokio::time::sleep(SETTLE).await;
            changed.borrow_and_update();
        }
        let st = state.clone();
        for outcome in repairer.pass(&home, Instant::now(), |id| crate::plugins::reinstall(st.clone(), id)).await {
            record(&state, &outcome);
        }
        woken = tokio::time::timeout(PERIOD, changed.changed()).await.is_ok();
    }
}

fn watch_registry(dir: &Path, changes: watch::Sender<()>) -> Option<RecommendedWatcher> {
    let watcher = (|| {
        let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
            let Ok(event) = event else { return };
            if event.need_rescan() || (!event.kind.is_access() && event.paths.iter().any(|path| path.file_name().is_some_and(|name| name == REGISTRY))) {
                changes.send_replace(());
            }
        })?;
        watcher.watch(dir, RecursiveMode::NonRecursive)?;
        notify::Result::Ok(watcher)
    })();
    watcher.inspect_err(|error| tracing::warn!(code = "plugins_repair_watch_failed", %error, "plugins: sem observador do registro")).ok()
}

/// Plugins ligados no principal com instalação de usuário sem pasta. Os de projeto ficam de fora: a
/// reinstalação roda na home, com escopo de usuário. Arquivo ilegível não repara nada.
async fn missing(home: &Path) -> Vec<PluginId> {
    let home: PathBuf = home.to_path_buf();
    let findings = tokio::task::spawn_blocking(move || plugin_findings(&home, &home.join(".claude")).unwrap_or_default());
    findings
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|finding| match finding {
            PluginFinding::FolderGone { id, user: true, .. } => id.parse().ok(),
            _ => None,
        })
        .collect()
}

struct Outcome {
    id: PluginId,
    result: Result<OperationResult, PluginsError>,
    fixed: bool,
}

#[derive(Default)]
struct Repairer {
    failed: HashMap<String, Instant>,
}

impl Repairer {
    /// Reinstala um por vez, pulando o que não voltou há menos de [`RETRY_AFTER`], e confere a pasta
    /// numa releitura só no fim.
    async fn pass<F, Fut>(&mut self, home: &Path, now: Instant, mut install: F) -> Vec<Outcome>
    where
        F: FnMut(PluginId) -> Fut,
        Fut: Future<Output = Result<OperationResult, PluginsError>>,
    {
        let mut attempts = Vec::new();
        for id in missing(home).await {
            if self.failed.get(id.as_str()).is_some_and(|at| now.duration_since(*at) < RETRY_AFTER) {
                continue;
            }
            let result = install(id.clone()).await;
            attempts.push((id, result));
        }
        if attempts.is_empty() {
            return vec![];
        }
        let left = missing(home).await;
        attempts
            .into_iter()
            .map(|(id, result)| {
                let fixed = result.is_ok() && !left.contains(&id);
                if fixed {
                    self.failed.remove(id.as_str());
                } else {
                    self.failed.insert(id.as_str().to_owned(), now);
                }
                Outcome { id, result, fixed }
            })
            .collect()
    }
}

fn record(state: &AppState, outcome: &Outcome) {
    let plugin = outcome.id.as_str();
    let (level, code, detail) = match &outcome.result {
        Ok(_) if outcome.fixed => {
            tracing::info!(code = "plugins_repaired", plugin, "plugins: pasta de instalação sumiu, plugin reinstalado");
            ("aviso", "reinstalado", String::new())
        }
        Ok(_) => {
            tracing::warn!(code = "plugins_repair_failed", plugin, "plugins: reinstalado pela CLI, mas a pasta continua ausente");
            ("erro", "pasta_ainda_ausente", String::new())
        }
        Err(error) => {
            tracing::warn!(code = "plugins_repair_failed", plugin, %error, "plugins: pasta de instalação sumiu e a reinstalação falhou");
            ("erro", error.code(), error.message().unwrap_or_default().chars().take(300).collect())
        }
    };
    state.diag.record_line(json!({"evento": "plugins.reparo", "nivel": level, "codigo": code, "plugin": plugin, "detalhe": detail}));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn id(raw: &str) -> PluginId {
        raw.parse().unwrap()
    }

    fn ok() -> Result<OperationResult, PluginsError> {
        Ok(OperationResult { outcome: "ok".into(), message: String::new(), old_version: None, new_version: None })
    }

    /// Home com o registro e o `settings.json` principal; devolve a pasta que falta de `sumido@m`.
    fn home_with(registry: serde_json::Value, settings: serde_json::Value) -> (tempfile::TempDir, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let claude = root.path().join(".claude");
        fs::create_dir_all(claude.join("plugins/cache/m/presente/1")).unwrap();
        let text = registry.to_string().replace("<H>", &root.path().display().to_string());
        fs::write(claude.join("plugins").join(REGISTRY), text).unwrap();
        fs::write(claude.join("settings.json"), settings.to_string()).unwrap();
        let gone = claude.join("plugins/cache/m/sumido/2");
        (root, gone)
    }

    const GONE: &str = "<H>/.claude/plugins/cache/m/sumido/2";

    #[tokio::test]
    async fn only_enabled_user_installs_whose_folder_is_gone() {
        let registry = json!({"version": 2, "plugins": {
            "sumido@m": [{"scope": "user", "installPath": GONE}],
            "presente@m": [{"scope": "user", "installPath": "<H>/.claude/plugins/cache/m/presente/1"}],
            "desligado@m": [{"scope": "user", "installPath": GONE}],
            "projeto@m": [{"scope": "project", "projectPath": "/x", "installPath": GONE}],
            "fora@m": [{"scope": "user", "installPath": GONE}],
        }});
        let settings = json!({"enabledPlugins": {
            "sumido@m": true, "presente@m": true, "desligado@m": false, "projeto@m": true,
        }});
        let (root, _) = home_with(registry, settings);
        assert_eq!(missing(root.path()).await, vec![id("sumido@m")]);
    }

    #[tokio::test]
    async fn unreadable_files_repair_nothing() {
        let root = tempfile::tempdir().unwrap();
        assert!(missing(root.path()).await.is_empty());
        fs::create_dir_all(root.path().join(".claude/plugins")).unwrap();
        fs::write(root.path().join(".claude/settings.json"), "{quebrado").unwrap();
        assert!(missing(root.path()).await.is_empty());
    }

    #[tokio::test]
    async fn rewriting_the_registry_wakes_the_repair() {
        let root = tempfile::tempdir().unwrap();
        let (changes, mut changed) = watch::channel(());
        let _watcher = watch_registry(root.path(), changes).unwrap();
        let temporary = root.path().join(".installed_plugins.json.tmp");
        fs::write(&temporary, "{}").unwrap();
        fs::rename(&temporary, root.path().join(REGISTRY)).unwrap();
        tokio::time::timeout(Duration::from_secs(5), changed.changed()).await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_reinstall_that_leaves_the_folder_missing_waits_before_trying_again() {
        let registry = json!({"version": 2, "plugins": {"sumido@m": [{"scope": "user", "installPath": GONE}]}});
        let (root, gone) = home_with(registry, json!({"enabledPlugins": {"sumido@m": true}}));
        let mut repairer = Repairer::default();
        let start = Instant::now();
        let calls = std::cell::Cell::new(0);
        // A CLI diz que deu certo, mas a pasta não aparece.
        let silent = |_| {
            calls.set(calls.get() + 1);
            async { ok() }
        };
        let first = repairer.pass(root.path(), start, silent).await;
        assert!(!first[0].fixed);
        assert!(repairer.pass(root.path(), start + Duration::from_secs(60), silent).await.is_empty());
        assert_eq!(calls.get(), 1);
        let done = repairer
            .pass(root.path(), start + RETRY_AFTER, |_| {
                fs::create_dir_all(&gone).unwrap();
                async { ok() }
            })
            .await;
        assert!(done[0].fixed);
        assert!(repairer.failed.is_empty());
    }
}
