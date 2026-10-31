// crates/hangar-server/src/search/watch.rs
//! Produtor do índice: 1ª passada depois da subida, depois uma passada a cada rajada de escrita em
//! `*.jsonl` das contas (observador de arquivos), relendo o catálogo a cada passada. Observador que
//! não sobe ou cai vai ao diário, e a busca volta a disparar passada quando o índice está velho.

use std::path::PathBuf;
use std::sync::PoisonError;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::Instant;

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

use super::Index;

pub(super) enum Wake {
    /// Escrita num transcript, pasta nova, fila de eventos estourada ou catálogo mudado.
    Changed,
    /// Uma passada que o produtor não fez terminou: refazer o observador se as contas mudaram.
    Rewatch,
    Failed(String),
    Stop,
}

pub(super) type Targets = Vec<(PathBuf, RecursiveMode)>;

/// O produtor ligado; soltá-lo para a thread e o observador.
pub struct Producer {
    tx: Sender<Wake>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Producer {
    /// Espera a thread acabar (sozinha ela só acaba quando o banco não abre).
    pub fn join(mut self) {
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

impl Drop for Producer {
    fn drop(&mut self) {
        let _ = self.tx.send(Wake::Stop);
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

pub(super) fn spawn(index: Index) -> Producer {
    let (tx, rx) = mpsc::channel();
    *index.inner.wake.lock().unwrap_or_else(PoisonError::into_inner) = Some(tx.clone());
    let events = tx.clone();
    let thread = std::thread::Builder::new().name("busca-indice".into()).spawn(move || run(index, events, rx))
        .expect("thread do índice da busca");
    Producer { tx, thread: Some(thread) }
}

/// Saída da thread, normal ou por pânico: sem produtor, a busca volta a disparar passada.
struct Alive(Index);

impl Drop for Alive {
    fn drop(&mut self) {
        let inner = &self.0.inner;
        inner.watch_ok.store(false, Ordering::Release);
        *inner.watched.lock().unwrap_or_else(PoisonError::into_inner) = None;
        *inner.wake.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if std::thread::panicking() {
            tracing::error!("produtor do índice da busca caiu");
            (inner.report)("busca.produtor_caiu", "a thread do índice parou com pânico".into());
        }
    }
}

/// Estado do observador, só da thread do produtor.
#[derive(Default)]
struct Watch {
    watcher: Option<RecommendedWatcher>,
    /// Pastas para as quais o estado atual foi montado; `None` = refazer na próxima chance.
    built_for: Option<Targets>,
    /// A queda já foi ao diário: só a mudança de estado é registrada.
    reported: bool,
}

fn run(index: Index, tx: Sender<Wake>, rx: Receiver<Wake>) {
    let _alive = Alive(index.clone());
    if let Err(e) = index.open_now() {
        // Sem índice a busca varre o disco a cada pedido: fica no diário exportável, não só no log.
        tracing::warn!(error = %e, "índice da busca indisponível");
        (index.inner.report)("busca.indice_indisponivel", e.to_string());
        return;
    }
    let deadline = Instant::now() + index.inner.timing.first_pass;
    while let Some(left) = deadline.checked_duration_since(Instant::now()).filter(|d| !d.is_zero()) {
        match rx.recv_timeout(left) {
            Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => break,
        }
    }
    let mut watch = Watch::default();
    let mut need_pass = true;
    loop {
        // O observador nasce antes da passada: escrita feita durante ela não se perde.
        rewatch(&index, &tx, &mut watch);
        if need_pass {
            if let Err(e) = index.update() {
                tracing::warn!(error = %e, "passada do índice da busca falhou");
            }
            rewatch(&index, &tx, &mut watch);
        }
        need_pass = loop {
            match rx.recv() {
                Ok(Wake::Changed) => match settle(&index, &rx, &mut watch) {
                    Some(pass) => break pass,
                    None => return,
                },
                Ok(Wake::Rewatch) => break false,
                Ok(Wake::Failed(detail)) => down(&index, &mut watch, detail),
                Ok(Wake::Stop) | Err(_) => return,
            }
        };
    }
}

/// Espera a calma depois do último evento; rajada sem fim passa mesmo assim depois de dez esperas.
/// `None` = pediram para parar.
fn settle(index: &Index, rx: &Receiver<Wake>, watch: &mut Watch) -> Option<bool> {
    let debounce = index.inner.timing.debounce;
    let deadline = Instant::now() + debounce * 10;
    loop {
        match rx.recv_timeout(debounce) {
            Ok(Wake::Stop) | Err(RecvTimeoutError::Disconnected) => return None,
            Ok(Wake::Failed(detail)) => down(index, watch, detail),
            Ok(_) if Instant::now() < deadline => {}
            Ok(_) | Err(RecvTimeoutError::Timeout) => return Some(true),
        }
    }
}

fn mark_down(index: &Index, watch: &mut Watch, detail: String, what: &'static str) {
    watch.watcher = None;
    watch.built_for = None;
    index.inner.watch_ok.store(false, Ordering::Release);
    *index.inner.watched.lock().unwrap_or_else(PoisonError::into_inner) = None;
    if !watch.reported {
        watch.reported = true;
        tracing::warn!(detail = %detail, "{what}");
        (index.inner.report)("busca.observador_falhou", detail);
    }
}

fn down(index: &Index, watch: &mut Watch, detail: String) {
    mark_down(index, watch, detail, "busca: observador de arquivos caiu");
}

/// `projects` de cada conta, recursivo; conta ainda sem `projects` vigia a própria pasta até ele nascer.
/// `None`: catálogo ilegível agora.
pub(super) fn targets(index: &Index) -> Option<Targets> {
    let mut out: Targets = Vec::new();
    for account in (index.inner.accounts)().ok()? {
        let target = if account.projects.is_dir() {
            (account.projects, RecursiveMode::Recursive)
        } else if account.dir.is_dir() {
            (account.dir, RecursiveMode::NonRecursive)
        } else {
            continue;
        };
        if !out.contains(&target) { out.push(target); }
    }
    // Raiz de agente ainda inexistente: vigia a pasta de cima até ela nascer, como `projects`.
    for root in index.agents().map(super::agents::roots).unwrap_or_default() {
        let target = if root.is_dir() {
            (root, RecursiveMode::Recursive)
        } else if let Some(parent) = root.parent().filter(|p| p.is_dir()) {
            (parent.to_owned(), RecursiveMode::NonRecursive)
        } else {
            continue;
        };
        if !out.contains(&target) { out.push(target); }
    }
    Some(out)
}

/// O que um evento do observador pede ao produtor.
pub(super) fn wakes(event: &notify::Result<notify::Event>) -> Option<Wake> {
    let e = match event {
        Ok(e) => e,
        Err(e) => return Some(Wake::Failed(e.to_string())),
    };
    // Fila de eventos estourada: o que mudou se perdeu, só uma passada sabe.
    if e.need_rescan() {
        return Some(Wake::Changed);
    }
    // O notify assina abrir e fechar: a leitura do próprio índice acordaria sem fim.
    if e.kind.is_access() {
        return None;
    }
    let transcript = e.paths.iter().any(|p| p.extension().is_some_and(|x| x == "jsonl") || p.file_name().is_some_and(|n| n == "projects"));
    // Pasta nova (de projeto): o arquivo criado logo dentro dela pode chegar antes do observador dela.
    let folder = matches!(e.kind, EventKind::Create(_)) && e.paths.iter().any(|p| p.is_dir());
    (transcript || folder).then_some(Wake::Changed)
}

fn rewatch(index: &Index, tx: &Sender<Wake>, watch: &mut Watch) {
    let Some(wanted) = targets(index) else { return };
    if watch.built_for.as_ref() == Some(&wanted) {
        return;
    }
    watch.watcher = None;
    let events = tx.clone();
    let built = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        if let Some(wake) = wakes(&event) { let _ = events.send(wake); }
    });
    let result = built.and_then(|mut w| {
        for (path, mode) in &wanted {
            w.watch(path, *mode)?;
        }
        Ok(w)
    });
    match result {
        Ok(w) => {
            // Sem pasta vigiada não há observador: a busca segue disparando passada.
            let up = !wanted.is_empty();
            watch.watcher = up.then_some(w);
            watch.built_for = Some(wanted.clone());
            *index.inner.watched.lock().unwrap_or_else(PoisonError::into_inner) = Some(wanted);
            index.inner.watch_ok.store(up, Ordering::Release);
            if up { watch.reported = false; }
        }
        Err(e) => mark_down(index, watch, e.to_string(), "busca: observador de arquivos não subiu"),
    }
}
