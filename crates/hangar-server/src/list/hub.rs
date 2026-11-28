//! `ListHub`: a lista do dono no Rust (`GET /api/sessions` e `/api/sessions/events`). Um produtor
//! por servidor, ligado enquanto houver lista aberta: tique de 1,5 s depois do trabalho (descoberta,
//! fatos do Python, classificação, decoração e rebaixamento, tudo pela ponte), assinatura, e JSON
//! só quando ela muda. Cada conexão só lê o último publicado. Contrato público do `list_events`
//! (`sse.py`): `sessions`, `list_error` uma vez na transição, `shortcut_terminals`, `nav` uma vez
//! por conexão, `ping` a cada 8 s e comentário a cada 15 s. O convidado da 8766 lê o mesmo produtor,
//! recortado ao token dele, e não conta como lista do dono.
//!
//! Entre os tiques, o produtor observa as pastas de estado das contas (marcador, registro nativo,
//! pergunta aberta): escrita nelas reclassifica só a sessão do arquivo e publica na hora, com a
//! rajada juntada em 150 ms. O observador que falha vai ao diário e o tique continua valendo.
use std::collections::{BTreeMap, HashMap};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use futures_util::future::BoxFuture;
use hangar_api::session::SessionRow;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{Value, json};
use tokio::sync::{mpsc, watch};

use super::bridge::{ListBridge, ListError, Partial, Produced};
use super::facts_files;
use super::sig;
use crate::devices::Registry;
use crate::diag::DiagClient;
use crate::routes::{AppState, cors, pass, who};
use crate::share::ShareState;
use crate::share::store::{Guest, Kind};
use crate::tail::{comment_frame, ping_frame, sse_frame};

/// O tique do `_ListRefresher`, contado do fim do trabalho.
pub(crate) const TICK: Duration = Duration::from_millis(1500);
const PING_EVERY: Duration = Duration::from_secs(8);
const COMMENT_EVERY: Duration = Duration::from_secs(15);
/// Envio preso por 30 s fecha a conexão, como o `send_timeout` do Python.
const SEND_TIMEOUT: Duration = Duration::from_secs(30);
/// Janela que junta uma rajada de escritas numa rodada só.
const COALESCE: Duration = Duration::from_millis(150);
/// Avisos de arquivo entre duas rodadas; cheia, os avisos se perdem e a rodada é a inteira.
const WAKE_QUEUE: usize = 256;

/// Retrato do runtime das sessões sem terminal, por chave da sessão.
pub trait HeadlessSource: Send + Sync {
    fn snapshots(&self) -> BoxFuture<'_, BTreeMap<String, Value>>;
}

impl HeadlessSource for crate::runtime::gateway::RuntimeRegistry {
    fn snapshots(&self) -> BoxFuture<'_, BTreeMap<String, Value>> { Box::pin(self.list_snapshots_readonly()) }
}

/// O que o produtor publicou por último, em quadros prontos: cada conexão só clona o `Bytes`.
/// Os `*_seq` andam quando o quadro deve sair de novo.
#[derive(Clone, Default)]
struct Published {
    sessions: Option<Bytes>,
    sessions_seq: u64,
    /// A produção do quadro `sessions` (só `Arc`s): cada convidado recorta as linhas dele.
    rows: Option<Produced>,
    /// Código e quadro enquanto a lista está em erro.
    error: Option<(&'static str, Bytes)>,
    error_seq: u64,
    shortcuts: Option<Bytes>,
    shortcuts_seq: u64,
    /// `{state}` do chaveiro desta máquina; só o dono recebe.
    keyring: Option<Bytes>,
    keyring_seq: u64,
    /// Etiquetas de máquina e contas; só o dono recebe.
    tags: Option<Bytes>,
    tags_seq: u64,
    /// Avisos da CLI ignorados nesta máquina; só o dono recebe.
    notice_ignores: Option<Bytes>,
    notice_ignores_seq: u64,
    /// Resumo da revisão do trabalho de cada sessão atual; só o dono recebe.
    work_reviews: Option<Bytes>,
    work_reviews_seq: u64,
    /// Aparelhos que já abriram a lista; só o dono recebe.
    devices: Option<Bytes>,
    devices_seq: u64,
    /// (nome, `ts` do pedido, quadro `{name, url}`)
    nav: Arc<Vec<(String, f64, Bytes)>>,
    /// `{state, requests}` do cofre desta máquina; só o dono recebe.
    vault: Option<Bytes>,
    vault_seq: u64,
    /// Pedidos de aprovação das integrações MCP desta máquina; só o dono recebe.
    integrations_approvals: Option<Bytes>,
    integrations_approvals_seq: u64,
    /// `{drafts}` dos rascunhos de WhatsApp pendentes; só o dono recebe.
    whatsapp_outbox: Option<Bytes>,
    whatsapp_outbox_seq: u64,
}

/// `clients`: listas do dono (o que o Python conta como app aberto); `guests`, as da 8766 e a
/// assinatura interna do retorno da filha ligada: mantêm o produtor ligado sem contar como app aberto.
#[derive(Default)]
struct Refs { clients: u32, guests: u32, running: bool }

pub struct ListHub {
    tx: watch::Sender<Arc<Published>>,
    refs: Mutex<Refs>,
    /// Pedidos do cofre (`vault::requests`); o padrão é um canal sem quem escreva.
    vault: watch::Receiver<Arc<crate::vault::requests::Feed>>,
    /// Pedidos de aprovação das integrações MCP; o padrão é um canal sem quem escreva.
    integrations_approvals: watch::Receiver<Arc<hangar_integrations::approvals::ApprovalFeed>>,
    /// Rascunhos de WhatsApp pendentes; ligado por `whatsapp_outbox::spawn`, na subida.
    whatsapp: std::sync::OnceLock<OutboxFeed>,
}

impl Default for ListHub {
    fn default() -> Self {
        Self {
            tx: watch::channel(Arc::default()).0,
            refs: Mutex::default(),
            vault: watch::channel(Arc::default()).1,
            integrations_approvals: watch::channel(Arc::default()).1,
            whatsapp: std::sync::OnceLock::new(),
        }
    }
}

/// Uma lista aberta; solta a contagem ao cair.
struct Subscription { hub: Arc<ListHub>, rx: watch::Receiver<Arc<Published>>, owner: bool }

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut r = lock(&self.hub.refs);
        let n = if self.owner { &mut r.clients } else { &mut r.guests };
        *n = n.saturating_sub(1);
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

impl ListHub {
    pub fn with_vault(vault: watch::Receiver<Arc<crate::vault::requests::Feed>>) -> Self { Self { vault, ..Self::default() } }

    pub fn with_feeds(
        vault: watch::Receiver<Arc<crate::vault::requests::Feed>>,
        integrations_approvals: watch::Receiver<Arc<hangar_integrations::approvals::ApprovalFeed>>,
    ) -> Self {
        Self { vault, integrations_approvals, ..Self::default() }
    }

    /// A primeira ligação vale: o WhatsApp do servidor é um só.
    pub fn bind_whatsapp(&self, outbox: watch::Receiver<Option<Arc<hangar_whatsapp::outbox::Feed>>>) { let _ = self.whatsapp.set(outbox); }

    fn subscribe(self: &Arc<Self>, list: &Arc<ListBridge>, devices: &Arc<Registry>, diag: &DiagClient, owner: bool) -> Subscription {
        let mut r = lock(&self.refs);
        if owner { r.clients += 1 } else { r.guests += 1 }
        if !r.running {
            r.running = true;
            tokio::spawn(supervise(self.clone(), list.clone(), devices.clone(), diag.clone()));
        }
        Subscription { hub: self.clone(), rx: self.tx.subscribe(), owner }
    }

    /// Para tarefa de dentro do servidor: conta como convidado, fora de `set_owner_clients`, senão o
    /// push do celular acharia o app aberto.
    pub(crate) fn subscribe_internal(self: &Arc<Self>, list: &Arc<ListBridge>, devices: &Arc<Registry>, diag: &DiagClient) -> InternalSubscription {
        InternalSubscription(self.subscribe(list, devices, diag, false))
    }
}

/// Assinatura interna; solta a contagem ao cair.
pub(crate) struct InternalSubscription(Subscription);

impl InternalSubscription {
    /// Linhas da última produção boa; `None` antes dela ou com a lista em erro.
    pub(crate) fn rows(&self) -> Option<Arc<Vec<SessionRow>>> {
        let p = self.0.rx.borrow();
        if p.error.is_some() { return None; }
        p.rows.as_ref().map(|r| r.rows.clone())
    }

    /// Como `rows`, esperando até `timeout` pela primeira produção boa.
    pub(crate) async fn wait_rows(&self, timeout: std::time::Duration) -> Option<Arc<Vec<SessionRow>>> {
        let mut rx = self.0.rx.clone();
        let ready = tokio::time::timeout(timeout, rx.wait_for(|p| p.error.is_none() && p.rows.is_some())).await.ok()?.ok()?;
        ready.rows.as_ref().map(|r| r.rows.clone())
    }
}

/// O que o produtor guarda entre tiques para só publicar mudança.
#[derive(Default)]
struct Last { sig: Option<String>, nav: Vec<(String, crate::nav_requests::Marker)>, shortcuts: Option<String>,
    keyring: Option<crate::keyring::KeyringState>, tags: Option<Arc<str>>, tags_error: Option<&'static str>,
    devices: Option<Arc<str>>, vault: Option<Arc<crate::vault::requests::Feed>>,
    integrations_approvals: Option<Arc<hangar_integrations::approvals::ApprovalFeed>>,
    whatsapp_outbox: Option<Arc<hangar_whatsapp::outbox::Feed>>,
    notice_ignores: Option<Arc<str>>, notice_ignores_failed: bool, work_reviews: Option<Value>,
    /// Versão do motor e linhas da última leitura dos resumos: iguais, o corpo não mudou.
    work_reviews_read: Option<(u64, Arc<Vec<SessionRow>>, Arc<crate::list::facts::ListFacts>)> }

/// Pânico fora da produção (assinatura, publicação) não deixa o hub sem produtor: as listas abertas
/// ficariam só com `ping`, e nenhuma nova religaria.
async fn supervise(hub: Arc<ListHub>, list: Arc<ListBridge>, devices: Arc<Registry>, diag: DiagClient) {
    loop {
        match tokio::spawn(produce(hub.clone(), list.clone(), devices.clone(), diag.clone())).await {
            Err(e) if e.is_panic() => {
                diag.report("rust.list_failed", "", "list_producer_panic", "produtor da lista do dono caiu; recomeça");
                tokio::time::sleep(TICK).await;
            }
            _ => return,
        }
    }
}

/// O laço único. Para quando a última lista fecha; quem abrir depois religa.
async fn produce(hub: Arc<ListHub>, list: Arc<ListBridge>, devices: Arc<Registry>, diag: DiagClient) {
    let mut last = Last::default();
    let mut files = FileWake::new();
    let mut keyring = list.keyring.subscribe();
    // A única leitura do arquivo, fora do runtime e sem segurar o laço: disco travado não para a lista
    // das sessões. Terminada, o `changed` publica o retrato sem esperar o tique.
    let (d, g) = (devices.clone(), diag.clone());
    tokio::spawn(async move {
        let (r, rg) = (d.clone(), g.clone());
        match tokio::task::spawn_blocking(move || r.snapshot(&rg)).await {
            Ok(_) => d.changed.notify_one(),
            Err(error) => {
                tracing::warn!(code = "devices_task_failed", %error, "leitura de devices.json caiu");
                g.report("rust.devices", "", "devices_task_failed", "leitura de devices.json caiu");
            }
        }
    });
    let mut vault = hub.vault.clone();
    let mut integrations_approvals = hub.integrations_approvals.clone();
    let mut outbox = hub.whatsapp.get().cloned();
    let review = list.work_review.get().cloned();
    loop {
        bind_outbox(&hub, &mut outbox);
        let clients = {
            let mut r = lock(&hub.refs);
            if r.clients + r.guests == 0 {
                r.running = false;
                list.set_owner_clients(0);
                // Quem abrir depois não recebe primeiro a lista de quando esta fechou.
                hub.tx.send_replace(Arc::default());
                return;
            }
            r.clients
        };
        list.set_owner_clients(clients);
        // Armado antes de produzir: escrita depois da leitura dos marcadores acorda a parcial, e a
        // lista publicada nunca é mais velha que o observador.
        files = files.sync(&list, &diag).await;
        // Pânico na produção vira erro da rodada, e o laço segue.
        let l = list.clone();
        let outcome = match tokio::spawn(async move { l.refresh().await }).await {
            Ok(Ok(p)) if p.facts.unknown => Err("list_facts_unknown"),
            Ok(Ok(p)) => Ok(p),
            Ok(Err(e)) => Err(e.code),
            Err(_) => Err("list_task_failed"),
        };
        if let Err(code) = outcome {
            diag.report("rust.list_failed", "", code, "lista do dono em erro");
        }
        publish(&hub.tx, &mut last, outcome, &diag, list.env().dirs.as_ref().map(|dirs| dirs.home.as_path()));
        publish_nav(&hub.tx, &mut last, list.nav.live(crate::nav_requests::now()));
        publish_keyring(&hub.tx, &mut last, *keyring.borrow_and_update());
        publish_vault(&hub.tx, &mut last, vault.borrow_and_update().clone());
        publish_integrations_approvals(&hub.tx, &mut last, integrations_approvals.borrow_and_update().clone());
        publish_whatsapp_outbox(&hub.tx, &mut last, current_outbox(&mut outbox));
        // Só a lista do dono recebe: sem ela (convidado, assinatura interna) nada é lido.
        if clients > 0 {
            publish_tags(&hub.tx, &mut last, list.tags.read().await, &diag);
            publish_notice_ignores(&hub.tx, &mut last, list.notice_ignores.read().await, &diag);
            publish_engine_reviews(&hub.tx, &mut last, review.as_deref());
            // Aparelho novo acorda pelo `changed`; aqui a primeira publicação e o aviso lido com a trava ocupada (o
            // retrato igual não sai de novo).
            if let Some(json) = devices.current() { publish_devices(&hub.tx, &mut last, json); }
        }
        let tick = tokio::time::Instant::now() + TICK;
        // Até o tique, escrita numa pasta de estado reclassifica só a sessão dela.
        loop {
            let first = tokio::select! {
                () = tokio::time::sleep_until(tick) => break,
                () = list.nav.changed.notified() => {
                    publish_nav(&hub.tx, &mut last, list.nav.live(crate::nav_requests::now()));
                    continue;
                }
                Ok(()) = keyring.changed() => {
                    publish_keyring(&hub.tx, &mut last, *keyring.borrow_and_update());
                    continue;
                }
                Ok(()) = vault.changed() => {
                    publish_vault(&hub.tx, &mut last, vault.borrow_and_update().clone());
                    continue;
                }
                Ok(()) = integrations_approvals.changed() => {
                    publish_integrations_approvals(&hub.tx, &mut last, integrations_approvals.borrow_and_update().clone());
                    continue;
                }
                () = outbox_changed(&mut outbox) => {
                    publish_whatsapp_outbox(&hub.tx, &mut last, current_outbox(&mut outbox));
                    continue;
                }
                () = list.tags.changed.notified(), if clients > 0 => {
                    publish_tags(&hub.tx, &mut last, list.tags.read().await, &diag);
                    continue;
                }
                () = list.notice_ignores.changed.notified(), if clients > 0 => {
                    publish_notice_ignores(&hub.tx, &mut last, list.notice_ignores.read().await, &diag);
                    continue;
                }
                // Único consumidor do `changed` do motor (`notify_one`).
                () = work_review_changed(review.as_deref()), if clients > 0 => {
                    publish_engine_reviews(&hub.tx, &mut last, review.as_deref());
                    continue;
                }
                () = devices.changed.notified(), if clients > 0 => {
                    if let Some(json) = devices.current() { publish_devices(&hub.tx, &mut last, json); }
                    continue;
                }
                w = files.recv() => w,
            };
            tokio::time::sleep(COALESCE).await;
            let Some(asked) = files.drain(first, &diag) else { break };
            match list.reclassify(asked).await {
                Ok(Partial::Done(p)) => {
                    publish(&hub.tx, &mut last, Ok(p), &diag, list.env().dirs.as_ref().map(|dirs| dirs.home.as_path()));
                    if clients > 0 { publish_engine_reviews(&hub.tx, &mut last, review.as_deref()); }
                }
                // Sem produção guardada (fatos desconhecidos, invalidada) quem resolve é o tique: rodar a
                // inteira a cada escrita tiraria o teto de frequência.
                Ok(Partial::Unchanged | Partial::NeedsFull) => {}
                Err(e) => diag.report("rust.list_failed", "", e.code, "rodada acordada por arquivo falhou; vale o tique"),
            }
        }
    }
}

enum Wake { Path(PathBuf), Failed }

fn watch_code(e: &notify::Error) -> &'static str {
    match e.kind {
        notify::ErrorKind::MaxFilesWatch => "list_watch_limit",
        notify::ErrorKind::PathNotFound => "list_watch_missing",
        notify::ErrorKind::Io(_) => "list_watch_io",
        _ => "list_watch_failed",
    }
}

/// Identidade da pasta; `None` se não é pasta (ou sumiu). Fora do Unix, só a existência.
fn dir_inode(dir: &Path) -> Option<u64> {
    let meta = std::fs::metadata(dir).ok().filter(std::fs::Metadata::is_dir)?;
    #[cfg(unix)]
    { use std::os::unix::fs::MetadataExt; Some(meta.ino()) }
    #[cfg(not(unix))]
    { let _ = meta; Some(0) }
}

/// `<conta>/<pasta>`: o diário diz qual pasta sem o caminho inteiro.
fn dir_label(dir: &Path) -> String {
    let mut parts = dir.iter().rev().take(2).map(|p| p.to_string_lossy()).collect::<Vec<_>>();
    parts.reverse();
    parts.join("/")
}

/// Observador das pastas de estado das contas, vivo enquanto o produtor roda: um só (um inotify)
/// para todas, sem tarefa por evento. O callback só enfileira o caminho.
struct FileWake {
    watcher: Option<RecommendedWatcher>,
    tx: mpsc::Sender<Wake>,
    rx: mpsc::Receiver<Wake>,
    /// Fila cheia ou `rescan` do sistema: avisos perdidos, só a rodada inteira serve.
    lost: Arc<AtomicBool>,
    /// Última falha vista pelo callback; fora da fila, que pode estar cheia justamente nela.
    fault: Arc<Mutex<Option<&'static str>>>,
    /// Pasta observada e o inode dela: apagada e recriada entre dois tiques, o observador da velha
    /// já foi solto pelo sistema.
    watched: HashMap<PathBuf, u64>,
    /// Último código de falha por pasta (a do observador na chave vazia): o diário só na troca.
    failed: HashMap<PathBuf, &'static str>,
}

impl FileWake {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel(WAKE_QUEUE);
        Self { watcher: None, tx, rx, lost: Arc::default(), fault: Arc::default(), watched: HashMap::new(), failed: HashMap::new() }
    }

    /// Arma o que falta e solta a pasta que sumiu, uma vez por tique. Stat e `inotify_add_watch`
    /// fora da thread do runtime.
    async fn sync(mut self, list: &Arc<ListBridge>, diag: &DiagClient) -> Self {
        let list = list.clone();
        let (me, report) = match tokio::task::spawn_blocking(move || {
            let report = match list.watch_dirs() {
                Ok(dirs) => self.arm(&dirs),
                // Sem pastas a ponte também recusa a produção, que já vai ao diário.
                Err(_) => Vec::new(),
            };
            (self, report)
        }).await {
            Ok(v) => v,
            Err(_) => {
                diag.report("rust.list_watch", "", "list_watch_task_failed", "observador das pastas de estado interrompido; vale o tique");
                return Self::new();
            }
        };
        for (dir, code) in report {
            diag.report("rust.list_watch", &dir, code, "pasta de estado sem observador; vale o tique");
        }
        me
    }

    fn arm(&mut self, dirs: &[PathBuf]) -> Vec<(String, &'static str)> {
        let mut report = Vec::new();
        let mut note = |failed: &mut HashMap<PathBuf, &'static str>, dir: &Path, code: &'static str| {
            if failed.insert(dir.to_owned(), code) != Some(code) {
                report.push((dir_label(dir), code));
            }
        };
        if self.watcher.is_none() {
            let (tx, lost, fault) = (self.tx.clone(), self.lost.clone(), self.fault.clone());
            let failed = move |code: &'static str| {
                *lock(&fault) = Some(code);
                // Cheia, já há aviso na fila para acordar o produtor.
                let _ = tx.try_send(Wake::Failed);
            };
            let tx = self.tx.clone();
            let made = notify::recommended_watcher(move |res: notify::Result<notify::Event>| match res {
                // A fila do sistema transbordou: o que mudou não está nos avisos.
                Ok(ev) if ev.need_rescan() => {
                    lost.store(true, Ordering::Relaxed);
                    failed("list_watch_overflow");
                }
                // O notify 7 assina abrir/fechar: a leitura da própria lista o acordaria sem fim.
                Ok(ev) if ev.kind.is_access() => {}
                Ok(ev) => {
                    for p in ev.paths.into_iter().filter(|p| p.extension().is_some_and(|x| x == "json")) {
                        if tx.try_send(Wake::Path(p)).is_err() {
                            lost.store(true, Ordering::Relaxed);
                        }
                    }
                }
                Err(e) => failed(watch_code(&e)),
            });
            match made {
                Ok(w) => {
                    self.watcher = Some(w);
                    self.failed.remove(Path::new(""));
                }
                Err(e) => {
                    note(&mut self.failed, Path::new(""), watch_code(&e));
                    return report;
                }
            }
        }
        let Some(w) = self.watcher.as_mut() else { return report };
        let gone: Vec<PathBuf> = self.watched.iter()
            .filter(|(d, ino)| dir_inode(d) != Some(**ino) || !dirs.contains(d)).map(|(d, _)| d.clone()).collect();
        for d in gone {
            // O sistema já soltou a pasta apagada; o erro do `unwatch` é esse.
            let _ = w.unwatch(&d);
            self.watched.remove(&d);
            if dir_inode(&d).is_none() {
                note(&mut self.failed, &d, "list_watch_gone");
            }
        }
        for d in dirs {
            // Conta sem a pasta é normal: o tique cobre até ela nascer.
            let Some(ino) = dir_inode(d).filter(|_| !self.watched.contains_key(d)) else { continue };
            match w.watch(d, RecursiveMode::NonRecursive) {
                Ok(()) => {
                    self.watched.insert(d.clone(), ino);
                    self.failed.remove(d);
                }
                Err(e) => note(&mut self.failed, d, watch_code(&e)),
            }
        }
        report
    }

    /// Sem observador, só o tique acorda.
    async fn recv(&mut self) -> Wake {
        match self.rx.recv().await {
            Some(w) => w,
            // O `tx` mora aqui: a fila nunca fecha.
            None => std::future::pending().await,
        }
    }

    /// O que chegou na janela: sessões cuja pergunta aberta mudou (marcador e registro nativo a
    /// ponte acha sozinha), ou `None` se só a rodada inteira serve.
    fn drain(&mut self, first: Wake, diag: &DiagClient) -> Option<Vec<String>> {
        let mut asked = Vec::new();
        let mut take = |w: Wake| match w {
            Wake::Path(p) => {
                if let Some(sid) = facts_files::askq_sid(&p).filter(|s| !asked.contains(s)) { asked.push(sid) }
            }
            Wake::Failed => {}
        };
        take(first);
        while let Ok(w) = self.rx.try_recv() {
            take(w);
        }
        if let Some(code) = lock(&self.fault).take() {
            diag.report("rust.list_watch", "", code, "observador das pastas de estado falhou; vale o tique");
        }
        (!self.lost.swap(false, Ordering::Relaxed)).then_some(asked)
    }
}

/// Linhas que o dono vê: as escondidas dele (sessão de convidado que ele não vê) não saem.
fn visible(p: &Produced) -> Vec<&SessionRow> {
    p.rows.iter().filter(|r| !p.facts.hidden.contains(&r.name)).collect()
}

fn error_frame(code: &str) -> Bytes { sse_frame("list_error", &json!({"code": code}).to_string(), None) }

fn publish(tx: &watch::Sender<Arc<Published>>, last: &mut Last, outcome: Result<Produced, &'static str>, diag: &DiagClient,
    home: Option<&std::path::Path>) {
    let prev = tx.borrow().clone();
    let mut next = (*prev).clone();
    let mut changed = false;
    match outcome {
        Err(code) => changed |= fail(&prev, &mut next, code),
        Ok(p) => {
            let mut rows = visible(&p);
            let fallen = home.map(|home| crate::session_open::fallen::rows(home, &rows)).unwrap_or_default();
            rows.extend(fallen.iter());
            let sig = sig::list_sig(rows.iter().copied());
            // Volta do erro reemite a mesma lista: é ela que limpa o erro na tela.
            if prev.error.is_some() || last.sig.as_deref() != Some(sig.as_str()) {
                match serde_json::to_string(&rows) {
                    Ok(json) => {
                        next.sessions = Some(sse_frame("sessions", &json, None));
                        next.sessions_seq += 1;
                        next.rows = Some(p.clone());
                        next.error = None;
                        last.sig = Some(sig);
                        changed = true;
                    }
                    Err(_) => {
                        diag.report("rust.list_failed", "", "list_unserializable", "lista do dono sem serializar");
                        changed |= fail(&prev, &mut next, "list_unserializable");
                    }
                }
            }
            // `None` é antes da primeira leitura boa: nada a mandar, como no Python.
            if let Some(sc) = p.facts.shortcuts.as_deref().filter(|sc| last.shortcuts.as_deref() != Some(*sc)) {
                next.shortcuts = Some(sse_frame("shortcut_terminals", sc, None));
                next.shortcuts_seq += 1;
                last.shortcuts = Some(sc.to_owned());
                changed = true;
            }
        }
    }
    if changed {
        tx.send_replace(Arc::new(next));
    }
}

/// Pedidos de navegador vivos, fora da rodada: lista em erro não segura nem repete o pedido.
fn publish_nav(tx: &watch::Sender<Arc<Published>>, last: &mut Last, nav: Vec<(String, crate::nav_requests::Marker)>) {
    if nav == last.nav { return; }
    let mut next = (**tx.borrow()).clone();
    next.nav = Arc::new(nav_entries(&nav));
    last.nav = nav;
    tx.send_replace(Arc::new(next));
}

/// Fora da rodada, como o `nav`: lista em erro não esconde o chaveiro trancado.
fn publish_keyring(tx: &watch::Sender<Arc<Published>>, last: &mut Last, state: crate::keyring::KeyringState) {
    if last.keyring == Some(state) { return; }
    let mut next = (**tx.borrow()).clone();
    next.keyring = Some(sse_frame("keyring", &json!({"state": state}).to_string(), None));
    next.keyring_seq += 1;
    last.keyring = Some(state);
    tx.send_replace(Arc::new(next));
}

/// Fora da rodada, como o chaveiro; sai só quando muda e chega só à lista do dono (`frames`).
fn publish_vault(tx: &watch::Sender<Arc<Published>>, last: &mut Last, feed: Arc<crate::vault::requests::Feed>) {
    if last.vault.as_deref() == Some(&*feed) { return; }
    let Ok(json) = serde_json::to_string(&*feed) else { return };
    let mut next = (**tx.borrow()).clone();
    next.vault = Some(sse_frame("vault", &json, None));
    next.vault_seq += 1;
    last.vault = Some(feed);
    tx.send_replace(Arc::new(next));
}

/// Fora da rodada, como o cofre; sai só quando muda e chega só à lista do dono (`frames`).
fn publish_integrations_approvals(
    tx: &watch::Sender<Arc<Published>>,
    last: &mut Last,
    feed: Arc<hangar_integrations::approvals::ApprovalFeed>,
) {
    if last.integrations_approvals.as_deref() == Some(&*feed) { return; }
    let Ok(json) = serde_json::to_string(&*feed) else { return };
    let mut next = (**tx.borrow()).clone();
    next.integrations_approvals = Some(sse_frame("integrations_approvals", &json, None));
    next.integrations_approvals_seq += 1;
    last.integrations_approvals = Some(feed);
    tx.send_replace(Arc::new(next));
}

type OutboxFeed = watch::Receiver<Option<Arc<hangar_whatsapp::outbox::Feed>>>;

/// O WhatsApp é ligado no fim da subida, e a lista aberta antes já tem produtor: até a ligação, ele a procura a cada
/// volta.
fn bind_outbox(hub: &ListHub, outbox: &mut Option<OutboxFeed>) {
    if outbox.is_none() { *outbox = hub.whatsapp.get().cloned(); }
}

fn current_outbox(outbox: &mut Option<OutboxFeed>) -> Option<Arc<hangar_whatsapp::outbox::Feed>> {
    outbox.as_mut().and_then(|outbox| outbox.borrow_and_update().clone())
}

/// A lista dos rascunhos mudou; sem WhatsApp ligado, nunca.
async fn outbox_changed(outbox: &mut Option<OutboxFeed>) {
    if let Some(outbox) = outbox.as_mut() && outbox.changed().await.is_ok() { return; }
    std::future::pending::<()>().await;
}

/// Como o cofre: fora da rodada, só quando muda, e só à lista do dono (`frames`). Lista vazia lida do binário sai
/// (quem abre a folha por um aviso espera resposta); antes da primeira leitura com envio (`None`), nada sai. O
/// `Outbox` só troca o `Arc` quando a lista muda: o mesmo ponteiro é a mesma lista.
fn publish_whatsapp_outbox(tx: &watch::Sender<Arc<Published>>, last: &mut Last, feed: Option<Arc<hangar_whatsapp::outbox::Feed>>) {
    let Some(feed) = feed else { return };
    if last.whatsapp_outbox.as_ref().is_some_and(|previous| Arc::ptr_eq(previous, &feed)) { return; }
    let Ok(json) = serde_json::to_string(&*feed) else { return };
    let mut next = (**tx.borrow()).clone();
    next.whatsapp_outbox = Some(sse_frame("whatsapp_outbox", &json, None));
    next.whatsapp_outbox_seq += 1;
    last.whatsapp_outbox = Some(feed);
    tx.send_replace(Arc::new(next));
}

/// Fora da rodada; sai só quando o texto muda (mesmo `Arc` compara pelo ponteiro). Leitura com falha
/// mantém o último publicado.
fn publish_tags(tx: &watch::Sender<Arc<Published>>, last: &mut Last, read: Option<Result<Arc<str>, &'static str>>, diag: &DiagClient) {
    match read {
        None => {}
        Some(Err(code)) => {
            if last.tags_error.replace(code) != Some(code) {
                diag.report("rust.list_tags", "", code, "etiquetas sem leitura; vale a última publicada");
            }
        }
        Some(Ok(json)) => {
            last.tags_error = None;
            if last.tags.as_ref() == Some(&json) { return; }
            let mut next = (**tx.borrow()).clone();
            next.tags = Some(sse_frame("tags", &json, None));
            next.tags_seq += 1;
            last.tags = Some(json);
            tx.send_replace(Arc::new(next));
        }
    }
}

/// Fora da rodada, como `tags`: sai quando a rota grava. Leitura com falha mantém o último publicado.
fn publish_notice_ignores(tx: &watch::Sender<Arc<Published>>, last: &mut Last, read: Option<Result<Arc<str>, &'static str>>, diag: &DiagClient) {
    match read {
        None => {}
        Some(Err(code)) => {
            if !std::mem::replace(&mut last.notice_ignores_failed, true) {
                diag.report("rust.list_notice_ignores", "", code, "avisos ignorados sem leitura; vale a última lista publicada");
            }
        }
        Some(Ok(json)) => {
            last.notice_ignores_failed = false;
            if last.notice_ignores.as_ref() == Some(&json) { return; }
            let mut next = (**tx.borrow()).clone();
            next.notice_ignores = Some(sse_frame("notice_ignores", &json, None));
            next.notice_ignores_seq += 1;
            last.notice_ignores = Some(json);
            tx.send_replace(Arc::new(next));
        }
    }
}

async fn work_review_changed(engine: Option<&hangar_work_review::Engine>) {
    match engine {
        Some(engine) => engine.changed().notified().await,
        None => std::future::pending().await,
    }
}

/// Fora da rodada: segue o `changed` do motor e as linhas publicadas, porque sessão renomeada ou fechada
/// sai do evento. Sai só quando o JSON muda; antes da primeira lista boa não há sessões a resumir.
fn publish_engine_reviews(tx: &watch::Sender<Arc<Published>>, last: &mut Last, engine: Option<&hangar_work_review::Engine>) {
    if let Some(engine) = engine {
        publish_work_reviews(tx, last, engine.summaries_version(), |current| engine.summaries_for(current));
    }
}

/// `version` lida antes de `summaries`: mesma versão e mesmas linhas publicadas não releem o motor.
fn publish_work_reviews(tx: &watch::Sender<Arc<Published>>, last: &mut Last, version: u64, summaries: impl FnOnce(&[(&str, Option<&str>)]) -> Value) {
    let Some(produced) = tx.borrow().rows.clone() else { return };
    let same = last.work_reviews_read.as_ref()
        .is_some_and(|(seen, rows, facts)| *seen == version && Arc::ptr_eq(rows, &produced.rows) && Arc::ptr_eq(facts, &produced.facts));
    if same { return; }
    last.work_reviews_read = Some((version, produced.rows.clone(), produced.facts.clone()));
    let current: Vec<(&str, Option<&str>)> = visible(&produced).into_iter().map(|row| (row.name.as_str(), row.lifecycle_id.as_deref())).collect();
    let body = summaries(&current);
    // Igualdade de `Value`, não de texto: a ordem das chaves segue a das linhas.
    if last.work_reviews.as_ref() == Some(&body) { return; }
    let mut next = (**tx.borrow()).clone();
    next.work_reviews = Some(sse_frame("work_reviews", &body.to_string(), None));
    next.work_reviews_seq += 1;
    last.work_reviews = Some(body);
    tx.send_replace(Arc::new(next));
}

/// Fora da rodada, como `tags`: sai só quando um aparelho entra (o registro troca o `Arc`).
fn publish_devices(tx: &watch::Sender<Arc<Published>>, last: &mut Last, json: Arc<str>) {
    if last.devices.as_ref().is_some_and(|prev| Arc::ptr_eq(prev, &json)) { return; }
    let mut next = (**tx.borrow()).clone();
    next.devices = Some(sse_frame("devices", &json, None));
    next.devices_seq += 1;
    last.devices = Some(json);
    tx.send_replace(Arc::new(next));
}

/// Na transição ou na troca de motivo: lista vazia por falha seria igual a nenhuma sessão.
fn fail(prev: &Published, next: &mut Published, code: &'static str) -> bool {
    if prev.error.as_ref().map(|e| e.0) == Some(code) {
        return false;
    }
    next.error = Some((code, error_frame(code)));
    next.error_seq += 1;
    true
}

/// Pedidos em quadros prontos.
fn nav_entries(nav: &[(String, crate::nav_requests::Marker)]) -> Vec<(String, f64, Bytes)> {
    nav.iter().map(|(name, m)| (name.clone(), m.ts, sse_frame("nav", &json!({"name": name, "url": m.url}).to_string(), None))).collect()
}

/// O que uma conexão já entregou.
#[derive(Default)]
struct Sent { sessions: u64, error: u64, shortcuts: u64, keyring: u64, tags: u64, notice_ignores: u64, work_reviews: u64, devices: u64, vault: u64, integrations_approvals: u64, whatsapp_outbox: u64, nav: HashMap<String, f64> }

/// Quadros que esta conexão ainda não mandou do que está publicado.
fn frames(p: &Published, sent: &mut Sent) -> Vec<Bytes> {
    let mut out = Vec::new();
    if let Some((_, frame)) = &p.error {
        if sent.error != p.error_seq {
            sent.error = p.error_seq;
            out.push(frame.clone());
        }
    } else if let Some(s) = p.sessions.as_ref().filter(|_| sent.sessions != p.sessions_seq) {
        sent.sessions = p.sessions_seq;
        out.push(s.clone());
    }
    if let Some(sc) = p.shortcuts.as_ref().filter(|_| sent.shortcuts != p.shortcuts_seq) {
        sent.shortcuts = p.shortcuts_seq;
        out.push(sc.clone());
    }
    if let Some(k) = p.keyring.as_ref().filter(|_| sent.keyring != p.keyring_seq) {
        sent.keyring = p.keyring_seq;
        out.push(k.clone());
    }
    if let Some(t) = p.tags.as_ref().filter(|_| sent.tags != p.tags_seq) {
        sent.tags = p.tags_seq;
        out.push(t.clone());
    }
    if let Some(n) = p.notice_ignores.as_ref().filter(|_| sent.notice_ignores != p.notice_ignores_seq) {
        sent.notice_ignores = p.notice_ignores_seq;
        out.push(n.clone());
    }
    if let Some(w) = p.work_reviews.as_ref().filter(|_| sent.work_reviews != p.work_reviews_seq) {
        sent.work_reviews = p.work_reviews_seq;
        out.push(w.clone());
    }
    if let Some(d) = p.devices.as_ref().filter(|_| sent.devices != p.devices_seq) {
        sent.devices = p.devices_seq;
        out.push(d.clone());
    }
    if let Some(v) = p.vault.as_ref().filter(|_| sent.vault != p.vault_seq) {
        sent.vault = p.vault_seq;
        out.push(v.clone());
    }
    if let Some(ia) = p.integrations_approvals.as_ref().filter(|_| sent.integrations_approvals != p.integrations_approvals_seq) {
        sent.integrations_approvals = p.integrations_approvals_seq;
        out.push(ia.clone());
    }
    if let Some(w) = p.whatsapp_outbox.as_ref().filter(|_| sent.whatsapp_outbox != p.whatsapp_outbox_seq) {
        sent.whatsapp_outbox = p.whatsapp_outbox_seq;
        out.push(w.clone());
    }
    for (name, ts, frame) in p.nav.iter() {
        if sent.nav.get(name) != Some(ts) {
            sent.nav.insert(name.clone(), *ts);
            out.push(frame.clone());
        }
    }
    // Pedido confirmado ou vencido sai do mapa: a memória da conexão não cresce com eles.
    sent.nav.retain(|n, _| p.nav.iter().any(|(m, ..)| m == n));
    out
}

async fn push(out: &mpsc::Sender<Bytes>, frame: Bytes) -> bool {
    matches!(tokio::time::timeout(SEND_TIMEOUT, out.send(frame)).await, Ok(Ok(())))
}

async fn client_loop(mut sub: Subscription, out: mpsc::Sender<Bytes>) {
    // O ping sai antes de qualquer lista: a primeira produção pode levar segundos.
    if !push(&out, ping_frame()).await {
        return;
    }
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING_EVERY, PING_EVERY);
    let mut comment = tokio::time::interval_at(start + COMMENT_EVERY, COMMENT_EVERY);
    let mut sent = Sent::default();
    loop {
        let p = sub.rx.borrow_and_update().clone();
        for f in frames(&p, &mut sent) {
            if !push(&out, f).await {
                return;
            }
        }
        tokio::select! {
            _ = out.closed() => return,
            _ = ping.tick() => if !push(&out, ping_frame()).await { return },
            _ = comment.tick() => if !push(&out, comment_frame()).await { return },
            changed = sub.rx.changed() => if changed.is_err() { return },
        }
    }
}

/// Linhas que o convidado vê (`guest_safe` do Python): só as sessões do token, com o grupo
/// recortado ao que ele enxerga (nenhum nome de outra sessão do dono), com o tipo de acesso a cada
/// uma. A tarefa do grupo é texto do dono: só o par, que trabalha nela, a recebe.
fn guest_rows<'a>(rows: impl IntoIterator<Item = &'a SessionRow>, guest: &Guest) -> Vec<SessionRow> {
    rows.into_iter().filter(|r| guest.sees(&r.name)).map(|r| {
        let mut r = r.clone();
        if let Some(peers) = r.pair_peers.as_mut() { peers.retain(|p| guest.sees(p)); }
        r.linked_by = r.linked_by.filter(|p| guest.sees(p));
        r.pair_lead = r.pair_lead.filter(|p| guest.sees(p));
        if guest.invite { r.pair_task = None; }
        (r.pair_external, r.pair_externals, r.then_target) = (None, None, None);
        // Renomear pela sugestão é do dono.
        (r.rename_suggestion, r.rename_weak) = (None, false);
        // Ocultar é organização da lista do dono.
        r.hidden_from_list = false;
        // As portas são da máquina do dono: o túnel não atende convidado.
        r.ports = Vec::new();
        r.guest_kind = guest.kind_of(&r.name).map(|k| match k { Kind::Share => "share", Kind::Pair => "pair" }.to_owned());
        r
    }).collect()
}

/// A lista do convidado: o registro é relido a cada volta (sessão ligada ao token ou renomeada
/// depois de aberto aparece), e só sai o que mudou no recorte dele. Sem atalhos e sem `nav`.
async fn guest_loop(mut sub: Subscription, share: Arc<ShareState>, token: String, out: mpsc::Sender<Bytes>) {
    if !push(&out, ping_frame()).await {
        return;
    }
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + PING_EVERY, PING_EVERY);
    let mut comment = tokio::time::interval_at(start + COMMENT_EVERY, COMMENT_EVERY);
    let (mut last, mut error_seq) = (None::<String>, 0);
    // Lista publicada e recorte do token da última volta: iguais, o quadro sairia igual.
    let mut seen: Option<(u64, Vec<(String, Option<Kind>)>)> = None;
    loop {
        let p = sub.rx.borrow_and_update().clone();
        if let Some((_, frame)) = p.error.as_ref().filter(|_| p.error_seq != error_seq) {
            error_seq = p.error_seq;
            // A volta do erro reemite a lista.
            (last, seen) = (None, None);
            if !push(&out, frame.clone()).await { return }
        } else if let (None, Some(produced), Some(guest)) = (&p.error, &p.rows, share.store.lookup_token(&token)) {
            let mut view: Vec<(String, Option<Kind>)> = guest.sessions().into_iter().map(|n| { let k = guest.kind_of(&n); (n, k) }).collect();
            view.sort_by(|a, b| a.0.cmp(&b.0));
            let key = (p.sessions_seq, view);
            if seen.as_ref() != Some(&key) {
                match serde_json::to_string(&guest_rows(visible(produced), &guest)) {
                    Ok(json) if last.as_deref() != Some(json.as_str()) => {
                        if !push(&out, sse_frame("sessions", &json, None)).await { return }
                        last = Some(json);
                    }
                    Ok(_) => {}
                    Err(_) => return,
                }
                seen = Some(key);
            }
        }
        tokio::select! {
            _ = out.closed() => return,
            _ = ping.tick() => if !push(&out, ping_frame()).await { return },
            _ = comment.tick() => if !push(&out, comment_frame()).await { return },
            changed = sub.rx.changed() => if changed.is_err() { return },
        }
    }
}

/// O Python não tem mais o `GET` da lista: sem dono, a recusa (401/429) sai daqui, não um 405 de lá.
async fn refuse_or_pass(st: &AppState, peer: SocketAddr, req: Request, fwd: &crate::proxy::Forward) -> Response {
    if req.method() == Method::GET && !crate::routes::on_guest_port(&req)
        && req.extensions().get::<crate::login_guests::gate::Login>().is_none()
        && let Err(mut refused) = crate::routes::owner_gate(st, peer, &req).await {
        cors(req.headers(), refused.headers_mut());
        return refused;
    }
    pass(st, req, fwd).await
}

/// `GET /api/sessions/events` do dono e do convidado da 8766.
pub async fn events(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, who) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    let grant = match &who {
        Some(caller) if req.method() == Method::GET => caller.grant().cloned(),
        _ => return refuse_or_pass(&st, peer, req, &fwd).await,
    };
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    if let Some(login) = req.extensions().get::<crate::login_guests::gate::Login>() {
        tokio::spawn(login_loop(st.hub.subscribe(&st.list, &st.devices, &st.diag, false), st.login_guests.clone(), login.token.clone(), tx));
    } else { match (grant, st.share.clone()) {
        (None, _) => {
            // Só o dono; pelo Connect, só com credencial de aparelho. Aparelho ilegível é ignorado sem afetar a lista.
            let trusted = fwd.client_ip != crate::devices::CONNECT_PEER || req.extensions().get::<crate::connect::gate::DeviceCaller>().is_some();
            if trusted && let Some((kind, id)) = crate::devices::parse(req.uri().query())
                && !st.devices.is_known(&id) {
                let (devices, diag) = (st.devices.clone(), st.diag.clone());
                // Antes de assinar: a primeira lista desta conexão já traz o aparelho.
                if let Err(error) = tokio::task::spawn_blocking({ let diag = diag.clone(); move || devices.record(kind, &id, &diag) }).await {
                    tracing::warn!(code = "devices_task_failed", %error, "registro do aparelho caiu");
                    diag.report("rust.devices", "", "devices_task_failed", "registro do aparelho caiu");
                }
            }
            tokio::spawn(client_loop(st.hub.subscribe(&st.list, &st.devices, &st.diag, true), tx));
        }
        (Some(grant), Some(share)) => {
            tokio::spawn(guest_loop(st.hub.subscribe(&st.list, &st.devices, &st.diag, false), share, grant.token.clone(), tx));
        }
        // Convidado sem convites no Rust não existe: o porteiro nem sobe.
        (Some(_), None) => return pass(&st, req, &fwd).await,
    }
    }
    let stream = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|b| (Ok::<Bytes, Infallible>(b), rx)) });
    let mut resp = Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/event-stream; charset=utf-8"));
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(header::CONNECTION, HeaderValue::from_static("keep-alive"));
    h.insert("x-accel-buffering", HeaderValue::from_static("no"));
    cors(req.headers(), h);
    resp
}

/// `GET /api/sessions` do dono e do convidado: o retrato de até 2 s (o do tique, com lista aberta).
pub async fn list(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, who) = match who(&st, peer, &req).await { Ok(value) => value, Err(e) => return crate::auth::unavailable(e) };
    if who.is_none() || req.method() != Method::GET {
        return refuse_or_pass(&st, peer, req, &fwd).await;
    }
    // Relido agora: o grant do porteiro é desta mesma requisição, mas o registro é a fonte.
    let login = req.extensions().get::<crate::login_guests::gate::Login>().cloned();
    let guest = match who.as_ref().and_then(|c| c.grant()).filter(|_|login.is_none()) {
        None => None,
        Some(grant) => match st.share.as_ref().and_then(|s| s.store.lookup_token(&grant.token)) {
            Some(guest) => Some(guest),
            None => return pass(&st, req, &fwd).await,
        },
    };
    let produced = match st.list.snapshot().await {
        // Sem resposta dos fatos, acesso e escondidas são desconhecidos, não vazios.
        Ok(p) if p.facts.unknown => return unavailable(&st, req.headers(), ListError { code: "list_facts_unknown", detail: "" }),
        Ok(p) => p,
        Err(e) => return unavailable(&st, req.headers(), e),
    };
    let body = if let Some(login) = login {
        match login_rows(st.login_guests.clone(), produced.clone(), login.guest).await {
            Ok(rows) => serde_json::to_vec(&rows),
            Err(()) => return unavailable(&st, req.headers(), ListError { code: "guests_worker_failed", detail: "" }),
        }
    } else { match &guest {
        None => {
            let mut rows = visible(&produced);
            let fallen = st.list.env().dirs.as_ref().map(|dirs| crate::session_open::fallen::rows(&dirs.home, &rows)).unwrap_or_default();
            rows.extend(fallen.iter());
            serde_json::to_vec(&rows)
        }
        Some(guest) => serde_json::to_vec(&guest_rows(visible(&produced), guest)),
    }};
    let body = match body {
        Ok(b) => b,
        Err(_) => return unavailable(&st, req.headers(), ListError { code: "list_unserializable", detail: "" }),
    };
    let mut resp = Response::new(Body::empty());
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    let body = crate::routes::maybe_gzip(req.headers(), resp.headers_mut(), body);
    *resp.body_mut() = Body::from(body);
    cors(req.headers(), resp.headers_mut());
    resp
}

/// 503 no formato do `api.py` (`_mux_indisponivel`, `_lista_indisponivel`): "não sei quais sessões
/// existem" nunca vira "você não tem nenhuma".
fn unavailable(st: &AppState, req: &HeaderMap, e: ListError) -> Response {
    st.diag.report("rust.list_route_failed", "", e.code, "lista do dono indisponível no GET");
    let mut resp = (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "application/json")],
        json!({"detail": unavailable_detail(&e)}).to_string()).into_response();
    cors(req, resp.headers_mut());
    resp
}

/// O `detail` do 503 da lista, também para as rotas que dependem dela para saber se a sessão existe.
pub(crate) fn unavailable_detail(e: &ListError) -> Value {
    if e.code == "mux_unavailable" {
        json!({"code": "erro_mux_indisponivel", "params": {"detalhe": e.detail},
            "msg": "o tmux não respondeu — a lista de sessões está indisponível"})
    } else {
        json!({"code": "erro_lista_indisponivel", "params": {"detalhe": e.code}, "msg": "a lista de sessões está indisponível"})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn published(sessions: &str, seq: u64) -> Published {
        Published { sessions: Some(sse_frame("sessions", sessions, None)), sessions_seq: seq, ..Default::default() }
    }

    #[test]
    fn a_connection_sends_each_frame_once() {
        let mut sent = Sent::default();
        let p = published("[1]", 1);
        assert_eq!(frames(&p, &mut sent).len(), 1);
        assert!(frames(&p, &mut sent).is_empty(), "nada mudou");
        let err = Published { error: Some(("mux_unavailable", error_frame("mux_unavailable"))), error_seq: 1, ..p.clone() };
        let f = frames(&err, &mut sent);
        assert!(f.len() == 1 && f[0].starts_with(b"event: list_error"));
        assert!(frames(&err, &mut sent).is_empty(), "erro uma vez só");
        let back = Published { sessions_seq: 2, error: None, ..err.clone() };
        assert!(frames(&back, &mut sent)[0].starts_with(b"event: sessions"));
    }

    /// A lista de avisos ignorados sai quando a rota grava, e de novo só com outra gravação.
    #[tokio::test]
    async fn notice_ignores_event_follows_what_the_route_published() {
        let (tx, _rx) = watch::channel(Arc::new(Published::default()));
        let (mut last, mut sent, diag) = (Last::default(), Sent::default(), DiagClient::new());
        let read = |json: &str| Some(Ok(Arc::<str>::from(json)));
        publish_notice_ignores(&tx, &mut last, read(r#"{"keys":[]}"#), &diag);
        let first = frames(&tx.borrow(), &mut sent);
        assert!(first.len() == 1 && first[0].starts_with(b"event: notice_ignores"), "{first:?}");
        publish_notice_ignores(&tx, &mut last, read(r#"{"keys":[]}"#), &diag);
        assert!(frames(&tx.borrow(), &mut sent).is_empty(), "a mesma lista não sai de novo");
        // Leitura com falha não apaga o que os clientes já têm.
        publish_notice_ignores(&tx, &mut last, Some(Err("notice_ignores_unreadable")), &diag);
        assert!(frames(&tx.borrow(), &mut sent).is_empty());
        publish_notice_ignores(&tx, &mut last, read(r#"{"keys":["hook:Stop"]}"#), &diag);
        let changed = frames(&tx.borrow(), &mut sent);
        assert!(changed.len() == 1 && std::str::from_utf8(&changed[0]).unwrap().contains("hook:Stop"));
    }

    fn with_rows(tx: &watch::Sender<Arc<Published>>, rows: &[(&str, &str)]) {
        let rows = rows.iter().map(|(name, life)| serde_json::from_value(json!({"name": name, "lifecycle_id": life})).unwrap()).collect();
        let produced = Produced { rows: Arc::new(rows), facts: Arc::new(crate::list::facts::ListFacts { unknown: false, ..Default::default() }), facts_ok: true };
        let mut next = (**tx.borrow()).clone();
        next.rows = Some(produced);
        tx.send_replace(Arc::new(next));
    }

    /// Resumos guardados por (nome, vida), filtrados pelas linhas atuais como o motor faz.
    fn summaries(stored: &'static [(&'static str, &'static str, &'static str)]) -> impl Fn(&[(&str, Option<&str>)]) -> Value {
        move |current| {
            let reviews: serde_json::Map<String, Value> = stored.iter()
                .filter(|(name, life, _)| current.iter().any(|(n, l)| n == name && *l == Some(*life)))
                .map(|(name, _, state)| ((*name).to_owned(), json!({"review_id": "r-1", "state": state, "badge": null, "updated_at": "2026-11-18T10:00:00Z"})))
                .collect();
            json!({"reviews": reviews})
        }
    }

    fn work_reviews_body(frames: &[Bytes]) -> Option<Value> {
        let frame = std::str::from_utf8(frames.iter().find(|f| f.starts_with(b"event: work_reviews\r\n"))?).unwrap();
        let data = frame.lines().find_map(|line| line.strip_prefix("data: "))?;
        Some(serde_json::from_str(data).unwrap())
    }

    /// O evento sai na primeira lista, de novo só quando o resumo muda, e perde a sessão que saiu das linhas.
    #[test]
    fn work_reviews_event_follows_the_summaries_and_the_rows() {
        let (tx, _rx) = watch::channel(Arc::new(Published::default()));
        let (mut last, mut sent) = (Last::default(), Sent::default());
        publish_work_reviews(&tx, &mut last, 0, summaries(&[("a", "l1", "waiting")]));
        assert!(frames(&tx.borrow(), &mut sent).is_empty(), "sem lista boa não há o que resumir");
        with_rows(&tx, &[("a", "l1"), ("b", "l2")]);
        frames(&tx.borrow(), &mut sent);
        publish_work_reviews(&tx, &mut last, 0, summaries(&[("a", "l1", "waiting")]));
        let first = work_reviews_body(&frames(&tx.borrow(), &mut sent)).expect("primeira publicação");
        assert_eq!(first["reviews"]["a"]["state"], "waiting");
        assert!(first["reviews"].get("b").is_none(), "sessão sem revisão fica fora");
        publish_work_reviews(&tx, &mut last, 0, |_| panic!("mesma versão e mesmas linhas não releem o motor"));
        publish_work_reviews(&tx, &mut last, 1, summaries(&[("a", "l1", "waiting")]));
        assert!(frames(&tx.borrow(), &mut sent).is_empty(), "o mesmo resumo não sai de novo");
        publish_work_reviews(&tx, &mut last, 2, summaries(&[("a", "l1", "done")]));
        let changed = work_reviews_body(&frames(&tx.borrow(), &mut sent)).expect("resumo mudou");
        assert_eq!(changed["reviews"]["a"]["state"], "done");
        // Renomeada: o nome antigo sai das linhas, e o resumo dele com ele, mesmo sem versão nova.
        with_rows(&tx, &[("a2", "l1"), ("b", "l2")]);
        frames(&tx.borrow(), &mut sent);
        publish_work_reviews(&tx, &mut last, 2, summaries(&[("a", "l1", "done")]));
        let renamed = work_reviews_body(&frames(&tx.borrow(), &mut sent)).expect("linhas mudaram");
        assert_eq!(renamed, json!({"reviews": {}}));
    }

    #[tokio::test]
    async fn the_work_reviews_event_reaches_the_owner_list_and_never_a_guest() {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::list::discover_other::Dirs { home: tmp.path().into(), claude: tmp.path().join(".claude"),
            codex_home: tmp.path().join(".codex"), pi_sessions: tmp.path().join("pi"), omp_config: tmp.path().join("omp"),
            omp_agent: tmp.path().join("omp/agent"), kimi_home: tmp.path().join("kimi") };
        let share = crate::share::from_env(Some(tmp.path().join("shares.json")), Some(&dirs), "missing-test-mux".into(), None).unwrap();
        let hub = Arc::new(ListHub::default());
        with_rows(&hub.tx, &[("a", "l1")]);
        publish_work_reviews(&hub.tx, &mut Last::default(), 0, summaries(&[("a", "l1", "waiting")]));
        let (owner_tx, owner_rx) = mpsc::channel(16);
        tokio::spawn(client_loop(Subscription { hub: hub.clone(), rx: hub.tx.subscribe(), owner: true }, owner_tx));
        let (guest_tx, guest_rx) = mpsc::channel(16);
        let rx = hub.tx.subscribe();
        tokio::spawn(guest_loop(Subscription { hub, rx, owner: false }, share, "tok".into(), guest_tx));
        assert!(work_reviews_body(&drain(owner_rx).await).is_some());
        assert!(work_reviews_body(&drain(guest_rx).await).is_none());
    }

    #[test]
    fn a_guest_row_never_carries_the_ports() {
        let share: crate::share::store::Share = serde_json::from_value(json!({"id": "1", "session": "s", "life": null,
            "created_at": 0.0, "code_expires_at": 0.0, "code_hash": "h"})).unwrap();
        let guest = Guest { token_hash: "t".into(), records: vec![share], invite: true };
        let row: SessionRow = serde_json::from_value(json!({"name": "s", "ports": [{"port": 8123, "process": "python3", "pid": 7}]})).unwrap();
        assert_eq!(row.ports.len(), 1);
        let rows = guest_rows([&row], &guest);
        assert_eq!(rows.len(), 1);
        assert!(serde_json::to_value(&rows[0]).unwrap().get("ports").is_none());
    }

    #[test]
    fn nav_map_is_pruned_with_the_request() {
        let mut sent = Sent::default();
        let marker = crate::nav_requests::Marker { url: "u".into(), ts: 1.0 };
        let p = Published { nav: Arc::new(nav_entries(&[("a".into(), marker)])), ..Default::default() };
        assert_eq!(frames(&p, &mut sent).len(), 1);
        frames(&Published::default(), &mut sent);
        assert!(sent.nav.is_empty());
    }

    #[test]
    fn the_vault_event_goes_out_only_on_change_and_again_to_a_new_list() {
        let (tx, _rx) = watch::channel(Arc::new(Published::default()));
        let mut last = Last::default();
        let feed = Arc::new(crate::vault::requests::Feed { state: Some(crate::vault::State::Open), requests: vec![] });
        publish_vault(&tx, &mut last, feed.clone());
        let seq = tx.borrow().vault_seq;
        publish_vault(&tx, &mut last, Arc::new((*feed).clone()));
        assert_eq!(tx.borrow().vault_seq, seq, "igual não republica");
        let p = tx.borrow().clone();
        let f = frames(&p, &mut Sent::default());
        assert!(f.iter().any(|f| f.starts_with(b"event: vault\r\n")), "quem abre a lista recebe o último");
        let mut sent = Sent::default();
        frames(&p, &mut sent);
        assert!(frames(&p, &mut sent).is_empty(), "uma vez por conexão");
    }

    async fn drain(mut rx: mpsc::Receiver<Bytes>) -> Vec<Bytes> {
        let mut out = Vec::new();
        while let Ok(Some(frame)) = tokio::time::timeout(Duration::from_millis(300), rx.recv()).await { out.push(frame); }
        out
    }

    #[tokio::test]
    async fn the_vault_event_reaches_the_owner_list_and_never_a_guest() {
        assert!(crate::side::owner_only(b"event: vault\r\ndata: {}\r\n\r\n"), "no chat também é só do dono");
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::list::discover_other::Dirs { home: tmp.path().into(), claude: tmp.path().join(".claude"),
            codex_home: tmp.path().join(".codex"), pi_sessions: tmp.path().join("pi"), omp_config: tmp.path().join("omp"),
            omp_agent: tmp.path().join("omp/agent"), kimi_home: tmp.path().join("kimi") };
        let share = crate::share::from_env(Some(tmp.path().join("shares.json")), Some(&dirs), "missing-test-mux".into(), None).unwrap();
        let requests = crate::vault::requests::Requests::default();
        let hub = Arc::new(ListHub::with_vault(requests.feed()));
        requests.note_state(crate::vault::State::Open);
        publish_vault(&hub.tx, &mut Last::default(), hub.vault.clone().borrow_and_update().clone());
        let (owner_tx, owner_rx) = mpsc::channel(16);
        tokio::spawn(client_loop(Subscription { hub: hub.clone(), rx: hub.tx.subscribe(), owner: true }, owner_tx));
        let (guest_tx, guest_rx) = mpsc::channel(16);
        let rx = hub.tx.subscribe();
        tokio::spawn(guest_loop(Subscription { hub, rx, owner: false }, share, "tok".into(), guest_tx));
        let is_vault = |f: &Bytes| f.starts_with(b"event: vault\r\n");
        assert!(drain(owner_rx).await.iter().any(is_vault));
        assert!(!drain(guest_rx).await.iter().any(is_vault));
    }

    #[test]
    fn the_integrations_approvals_event_goes_out_only_on_change_and_again_to_a_new_list() {
        let (tx, _rx) = watch::channel(Arc::new(Published::default()));
        let mut last = Last::default();
        let feed = Arc::new(hangar_integrations::approvals::ApprovalFeed { requests: vec![] });
        publish_integrations_approvals(&tx, &mut last, feed.clone());
        let seq = tx.borrow().integrations_approvals_seq;
        publish_integrations_approvals(&tx, &mut last, Arc::new((*feed).clone()));
        assert_eq!(tx.borrow().integrations_approvals_seq, seq, "igual não republica");
        let p = tx.borrow().clone();
        let f = frames(&p, &mut Sent::default());
        assert!(
            f.iter().any(|f| f.starts_with(b"event: integrations_approvals\r\n")),
            "quem abre a lista recebe o último"
        );
        let mut sent = Sent::default();
        frames(&p, &mut sent);
        assert!(frames(&p, &mut sent).is_empty(), "uma vez por conexão");
    }

    #[tokio::test]
    async fn the_integrations_approvals_event_reaches_the_owner_list_and_never_a_guest() {
        assert!(
            crate::side::owner_only(b"event: integrations_approvals\r\ndata: {}\r\n\r\n"),
            "no chat também é só do dono"
        );
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::list::discover_other::Dirs {
            home: tmp.path().into(),
            claude: tmp.path().join(".claude"),
            codex_home: tmp.path().join(".codex"),
            pi_sessions: tmp.path().join("pi"),
            omp_config: tmp.path().join("omp"),
            omp_agent: tmp.path().join("omp/agent"),
            kimi_home: tmp.path().join("kimi"),
        };
        let share = crate::share::from_env(Some(tmp.path().join("shares.json")), Some(&dirs), "missing-test-mux".into(), None).unwrap();
        let approvals = hangar_integrations::ApprovalStore::new();
        let hub = Arc::new(ListHub::with_feeds(watch::channel(Arc::default()).1, approvals.feed()));
        let _waiter = approvals.open_request(hangar_integrations::OpenApprovalRequest {
            origin: "pc::s1".into(),
            session_key: "k1".into(),
            integration_id: "github".into(),
            integration_name: "GitHub".into(),
            tool: "create_issue".into(),
            arguments_preview: "{}".into(),
            reason: "criar issue".into(),
        }).unwrap();
        publish_integrations_approvals(&hub.tx, &mut Last::default(), hub.integrations_approvals.clone().borrow_and_update().clone());
        let (owner_tx, owner_rx) = mpsc::channel(16);
        tokio::spawn(client_loop(Subscription { hub: hub.clone(), rx: hub.tx.subscribe(), owner: true }, owner_tx));
        let (guest_tx, guest_rx) = mpsc::channel(16);
        let rx = hub.tx.subscribe();
        tokio::spawn(guest_loop(Subscription { hub, rx, owner: false }, share, "tok".into(), guest_tx));
        let is_approvals = |f: &Bytes| f.starts_with(b"event: integrations_approvals\r\n");
        assert!(drain(owner_rx).await.iter().any(is_approvals));
        assert!(!drain(guest_rx).await.iter().any(is_approvals));
    }

    fn outbox_feed(ids: &[&str]) -> Arc<hangar_whatsapp::outbox::Feed> {
        Arc::new(hangar_whatsapp::outbox::Feed { drafts: ids.iter().map(|id| json!({"id": id, "state": "pending"})).collect(), edit: false })
    }

    /// Sem WhatsApp com envio nada sai; lida a lista, sai mesmo vazia, também para a lista que fechou e reabriu (o
    /// produtor recomeça do zero), senão a folha aberta pelo aviso fica esperando para sempre.
    #[test]
    fn the_whatsapp_outbox_event_goes_out_once_read_and_only_on_change() {
        let (tx, _rx) = watch::channel(Arc::new(Published::default()));
        let mut last = Last::default();
        publish_whatsapp_outbox(&tx, &mut last, None);
        assert!(tx.borrow().whatsapp_outbox.is_none(), "sem WhatsApp com envio, nada sai");
        publish_whatsapp_outbox(&tx, &mut last, Some(outbox_feed(&[])));
        assert!(tx.borrow().whatsapp_outbox.as_ref().is_some_and(|f| f.starts_with(b"event: whatsapp_outbox\r\ndata: {\"drafts\":[],\"edit\":false}")),
            "lida e vazia, sai");
        let (reopened, _rx) = watch::channel(Arc::new(Published::default()));
        publish_whatsapp_outbox(&reopened, &mut Last::default(), Some(outbox_feed(&[])));
        assert!(reopened.borrow().whatsapp_outbox.is_some(), "a lista reaberta recebe a vazia");
        let d1 = outbox_feed(&["d1"]);
        publish_whatsapp_outbox(&tx, &mut last, Some(d1.clone()));
        let seq = tx.borrow().whatsapp_outbox_seq;
        publish_whatsapp_outbox(&tx, &mut last, Some(d1));
        assert_eq!(tx.borrow().whatsapp_outbox_seq, seq, "a mesma lista não republica");
        let p = tx.borrow().clone();
        let mut sent = Sent::default();
        assert!(frames(&p, &mut sent).iter().any(|f| f.starts_with(b"event: whatsapp_outbox\r\n")), "quem abre a lista recebe o último");
        assert!(frames(&p, &mut sent).is_empty(), "uma vez por conexão");
        publish_whatsapp_outbox(&tx, &mut last, Some(outbox_feed(&[])));
        let f = frames(&tx.borrow(), &mut sent);
        assert!(f.iter().any(|f| f.starts_with(b"event: whatsapp_outbox\r\ndata: {\"drafts\":[],\"edit\":false}")), "o cartão respondido some");
    }

    /// Lista aberta antes de a subida ligar o WhatsApp: o produtor que nasceu sem ele passa a publicar depois da ligação.
    #[test]
    fn a_producer_born_before_the_whatsapp_binding_publishes_after_it() {
        let hub = ListHub::default();
        let (tx, _rx) = watch::channel(Arc::new(Published::default()));
        let mut last = Last::default();
        let mut outbox = hub.whatsapp.get().cloned();
        bind_outbox(&hub, &mut outbox);
        publish_whatsapp_outbox(&tx, &mut last, current_outbox(&mut outbox));
        assert!(outbox.is_none() && tx.borrow().whatsapp_outbox.is_none(), "ainda sem WhatsApp");
        let (_feed_tx, feed_rx) = watch::channel(Some(outbox_feed(&["d1"])));
        hub.bind_whatsapp(feed_rx);
        bind_outbox(&hub, &mut outbox);
        publish_whatsapp_outbox(&tx, &mut last, current_outbox(&mut outbox));
        assert!(tx.borrow().whatsapp_outbox.as_ref().is_some_and(|f| f.starts_with(b"event: whatsapp_outbox\r\ndata: {\"drafts\":[{")),
            "a volta seguinte à ligação publica o cartão");
    }

    #[tokio::test]
    async fn the_whatsapp_outbox_event_reaches_the_owner_list_and_never_a_guest() {
        assert!(crate::side::owner_only(b"event: whatsapp_outbox\r\ndata: {}\r\n\r\n"), "no chat também é só do dono");
        let tmp = tempfile::tempdir().unwrap();
        let dirs = crate::list::discover_other::Dirs { home: tmp.path().into(), claude: tmp.path().join(".claude"),
            codex_home: tmp.path().join(".codex"), pi_sessions: tmp.path().join("pi"), omp_config: tmp.path().join("omp"),
            omp_agent: tmp.path().join("omp/agent"), kimi_home: tmp.path().join("kimi") };
        let share = crate::share::from_env(Some(tmp.path().join("shares.json")), Some(&dirs), "missing-test-mux".into(), None).unwrap();
        let (feed_tx, feed_rx) = watch::channel(Some(outbox_feed(&[])));
        let hub = Arc::new(ListHub::default());
        hub.bind_whatsapp(feed_rx);
        feed_tx.send_replace(Some(outbox_feed(&["d1"])));
        let mut outbox = hub.whatsapp.get().cloned();
        publish_whatsapp_outbox(&hub.tx, &mut Last::default(), current_outbox(&mut outbox));
        let (owner_tx, owner_rx) = mpsc::channel(16);
        tokio::spawn(client_loop(Subscription { hub: hub.clone(), rx: hub.tx.subscribe(), owner: true }, owner_tx));
        let (guest_tx, guest_rx) = mpsc::channel(16);
        let rx = hub.tx.subscribe();
        tokio::spawn(guest_loop(Subscription { hub, rx, owner: false }, share, "tok".into(), guest_tx));
        let is_outbox = |f: &Bytes| f.starts_with(b"event: whatsapp_outbox\r\n");
        assert!(drain(owner_rx).await.iter().any(is_outbox));
        assert!(!drain(guest_rx).await.iter().any(is_outbox));
    }
}

async fn login_rows(service:Arc<crate::login_guests::Service>,produced:Produced,guest:crate::login_guests::store::Guest)->Result<Vec<SessionRow>,()> {
    tokio::task::spawn_blocking(move||produced.rows.iter().filter(|row|service.store.visible(Some(&guest),&row.name,row.lifecycle_id.as_deref())).map(|row|SessionRow{ports:Vec::new(),..row.clone()}).collect()).await.map_err(|_|())
}

async fn login_loop(mut sub:Subscription,service:Arc<crate::login_guests::Service>,token:String,out:mpsc::Sender<Bytes>) {
    let start=tokio::time::Instant::now();
    let mut refresh=tokio::time::interval(TICK);
    let mut ping=tokio::time::interval_at(start+PING_EVERY,PING_EVERY);
    let mut comment=tokio::time::interval_at(start+COMMENT_EVERY,COMMENT_EVERY);
    let mut previous=None;let mut error=None;
    loop {
        let read=service.clone();let credential=token.clone();
        let Some(guest)=tokio::task::spawn_blocking(move||read.store.lookup(credential.as_bytes())).await.ok().flatten() else{return};
        let published=sub.rx.borrow_and_update().clone();
        if let Some((code,frame))=&published.error {
            if error!=Some(*code) {if !push(&out,frame.clone()).await{return;}error=Some(*code);}
            previous=None;
        }else if let Some(rows)=&published.rows {
            error=None;
            let Ok(rows)=login_rows(service.clone(),rows.clone(),guest).await else{let _=push(&out,error_frame("guests_worker_failed")).await;return;};
            let Ok(json)=serde_json::to_string(&rows) else{return};
            if previous.as_deref()!=Some(json.as_str()) {
                if !push(&out,sse_frame("sessions",&json,None)).await{return;}
                previous=Some(json);
            }
        }
        tokio::select! {
            _=out.closed()=>return,
            _=refresh.tick()=>{},
            _=ping.tick()=>if !push(&out,ping_frame()).await{return},
            _=comment.tick()=>if !push(&out,comment_frame()).await{return},
            changed=sub.rx.changed()=>if changed.is_err(){return},
        }
    }
}
