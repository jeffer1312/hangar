//! Um observador por `plugins/cache` de conta Codex, dividido entre as sessões dela.
//!
//! O Codex confere o hash da pasta inteira do plugin a cada hook e reinstala plugins de
//! marketplace git sozinho ao subir: quem observa a reinstalação reaprova antes da recusa.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::watch;

struct Shared {
    _watcher: RecommendedWatcher,
    changes: watch::Sender<u64>,
}

pub struct Subscription {
    _shared: Arc<Shared>,
    pub changes: watch::Receiver<u64>,
}

static SHARED: Mutex<BTreeMap<PathBuf, Weak<Shared>>> = Mutex::new(BTreeMap::new());

/// `plugins/cache` da conta: a do metadado, ou `~/.codex` quando a sessão não diz.
pub fn cache_dir(metadata: &serde_json::Value) -> Option<PathBuf> {
    let home = metadata["codex_home"].as_str().map(PathBuf::from).or_else(|| {
        std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(|home| PathBuf::from(home).join(".codex"))
    })?;
    Some(home.join("plugins").join("cache"))
}

pub fn subscribe(cache: &Path) -> notify::Result<Subscription> {
    let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
    shared.retain(|_, weak| weak.strong_count() > 0);
    if let Some(live) = shared.get(cache).and_then(Weak::upgrade) {
        return Ok(Subscription { changes: live.changes.subscribe(), _shared: live });
    }
    let (changes, receiver) = watch::channel(0u64);
    let sender = changes.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if relevant(&event) { sender.send_modify(|count| *count += 1); }
    })?;
    watcher.watch(cache, RecursiveMode::Recursive)?;
    let live = Arc::new(Shared { _watcher: watcher, changes });
    shared.insert(cache.to_path_buf(), Arc::downgrade(&live));
    Ok(Subscription { _shared: live, changes: receiver })
}

/// Só o que muda o hash do Codex: abrir e ler (os próprios hooks rodando) e bytecode Python ficam de fora.
fn relevant(event: &notify::Result<notify::Event>) -> bool {
    let Ok(event) = event else { return false };
    if event.need_rescan() { return true; }
    if event.kind.is_access() { return false; }
    event.paths.iter().any(|path| !path.components().any(|part| {
        let name = part.as_os_str().to_string_lossy();
        matches!(name.as_ref(), "__pycache__" | ".venv") || name.ends_with(".pyc") || name.ends_with(".egg-info")
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{AccessKind, CreateKind, EventKind};

    fn event(kind: EventKind, path: &str) -> notify::Result<notify::Event> {
        Ok(notify::Event::new(kind).add_path(PathBuf::from(path)))
    }

    #[test]
    fn only_changes_the_codex_fingerprint_sees_wake_the_sessions() {
        let create = EventKind::Create(CreateKind::Any);
        assert!(relevant(&event(create, "/c/exemplo-marketplace/exemplo/local/hooks/hooks.json")));
        assert!(!relevant(&event(EventKind::Access(AccessKind::Any), "/c/exemplo/local/scripts/guard.py")));
        assert!(!relevant(&event(create, "/c/exemplo/local/skills/_shared/__pycache__/trava.cpython-314.pyc")));
        assert!(!relevant(&event(create, "/c/x/local/.venv/bin/python")));
    }

    #[test]
    fn sessions_of_the_same_account_share_one_watcher() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (subscribe(dir.path()).unwrap(), subscribe(dir.path()).unwrap());
        assert!(Arc::ptr_eq(&a._shared, &b._shared));
    }
}
