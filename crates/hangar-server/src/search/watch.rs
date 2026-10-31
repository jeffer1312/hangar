// crates/hangar-server/src/search/watch.rs
//! Produtor do índice: 1ª passada depois da subida, depois uma passada a cada rajada de escrita em
//! `*.jsonl` das contas (observador de arquivos), relendo o catálogo a cada passada. Observador que
//! não sobe ou cai vai ao diário, e a busca volta a disparar passada quando o índice está velho.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Instant;

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use super::Index;

pub(super) enum Wake {
    /// Escrita num transcript (ou a pasta `projects` de uma conta nasceu).
    Changed,
    /// Uma passada que o produtor não fez terminou: refazer o observador se as contas mudaram.
    Rewatch,
    Failed(String),
}

pub(super) fn spawn(index: Index) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new().name("busca-indice".into()).spawn(move || run(index))
        .expect("thread do índice da busca")
}

type Targets = Vec<(PathBuf, RecursiveMode)>;

fn run(index: Index) {
    let (tx, rx) = mpsc::channel();
    *index.inner.wake.lock().unwrap() = Some(tx.clone());
    if let Err(e) = index.open_now() {
        // Sem índice a busca varre o disco a cada pedido: fica no diário exportável, não só no log.
        tracing::warn!(error = %e, "índice da busca indisponível");
        (index.inner.report)("busca.indice_indisponivel", e.to_string());
        return;
    }
    std::thread::sleep(index.inner.timing.first_pass);
    let mut watcher: Option<(RecommendedWatcher, Targets)> = None;
    let mut need_pass = true;
    loop {
        if need_pass {
            if let Err(e) = index.update() {
                tracing::warn!(error = %e, "passada do índice da busca falhou");
            }
        }
        rewatch(&index, &tx, &mut watcher);
        need_pass = loop {
            match rx.recv() {
                Ok(Wake::Changed) => break settle(&index, &rx, &mut watcher),
                Ok(Wake::Rewatch) => break false,
                Ok(Wake::Failed(detail)) => down(&index, &mut watcher, detail),
                Err(_) => return,
            }
        };
    }
}

/// Espera a calma depois do último evento; rajada sem fim passa mesmo assim depois de dez esperas.
fn settle(index: &Index, rx: &mpsc::Receiver<Wake>, watcher: &mut Option<(RecommendedWatcher, Targets)>) -> bool {
    let debounce = index.inner.timing.debounce;
    let deadline = Instant::now() + debounce * 10;
    loop {
        match rx.recv_timeout(debounce) {
            Ok(Wake::Failed(detail)) => down(index, watcher, detail),
            Ok(_) if Instant::now() < deadline => {}
            Ok(_) | Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return true,
        }
    }
}

fn down(index: &Index, watcher: &mut Option<(RecommendedWatcher, Targets)>, detail: String) {
    if watcher.take().is_some() || index.inner.watch_ok.load(Ordering::Acquire) {
        index.inner.watch_ok.store(false, Ordering::Release);
        tracing::warn!(detail = %detail, "busca: observador de arquivos caiu");
        (index.inner.report)("busca.observador_falhou", detail);
    }
}

/// `projects` de cada conta, recursivo; conta ainda sem `projects` vigia a própria pasta até ele nascer.
fn targets(index: &Index) -> Targets {
    let mut out: Targets = Vec::new();
    for account in (index.inner.accounts)() {
        let target = if account.projects.is_dir() {
            (account.projects, RecursiveMode::Recursive)
        } else if account.dir.is_dir() {
            (account.dir, RecursiveMode::NonRecursive)
        } else {
            continue;
        };
        if !out.contains(&target) { out.push(target); }
    }
    out
}

fn rewatch(index: &Index, tx: &mpsc::Sender<Wake>, watcher: &mut Option<(RecommendedWatcher, Targets)>) {
    let wanted = targets(index);
    if watcher.as_ref().is_some_and(|(_, current)| *current == wanted) {
        return;
    }
    *watcher = None;
    let events = tx.clone();
    let built = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let wake = match event {
            // O notify assina abrir e fechar: a leitura do próprio índice acordaria sem fim.
            Ok(e) if e.kind.is_access() => return,
            Ok(e) if e.paths.iter().any(|p| p.extension().is_some_and(|x| x == "jsonl") || p.file_name().is_some_and(|n| n == "projects")) => Wake::Changed,
            Ok(_) => return,
            Err(e) => Wake::Failed(e.to_string()),
        };
        let _ = events.send(wake);
    });
    let result = built.and_then(|mut w| {
        for (path, mode) in &wanted {
            w.watch(path, *mode)?;
        }
        Ok(w)
    });
    match result {
        Ok(w) => {
            *watcher = Some((w, wanted));
            index.inner.watch_ok.store(true, Ordering::Release);
        }
        Err(e) => {
            index.inner.watch_ok.store(false, Ordering::Release);
            tracing::warn!(error = %e, "busca: observador de arquivos não subiu");
            (index.inner.report)("busca.observador_falhou", e.to_string());
        }
    }
}
