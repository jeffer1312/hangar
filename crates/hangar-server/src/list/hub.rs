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
use crate::diag::DiagClient;
use crate::routes::{AppState, cors, pass, who};
use crate::share::ShareState;
use crate::share::store::{Guest, Kind};
use crate::tail::{comment_frame, ping_frame, sse_frame};

/// O tique do `_ListRefresher`, contado do fim do trabalho.
const TICK: Duration = Duration::from_millis(1500);
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
    /// (nome, `ts` do pedido, quadro `{name, url}`)
    nav: Arc<Vec<(String, f64, Bytes)>>,
}

/// `clients`: listas do dono (o que o Python conta como app aberto); `guests`, as da 8766 e a
/// assinatura interna do retorno da filha ligada: mantêm o produtor ligado sem contar como app aberto.
#[derive(Default)]
struct Refs { clients: u32, guests: u32, running: bool }

pub struct ListHub {
    tx: watch::Sender<Arc<Published>>,
    refs: Mutex<Refs>,
}

impl Default for ListHub {
    fn default() -> Self { Self { tx: watch::channel(Arc::default()).0, refs: Mutex::default() } }
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
    fn subscribe(self: &Arc<Self>, list: &Arc<ListBridge>, diag: &DiagClient, owner: bool) -> Subscription {
        let mut r = lock(&self.refs);
        if owner { r.clients += 1 } else { r.guests += 1 }
        if !r.running {
            r.running = true;
            tokio::spawn(supervise(self.clone(), list.clone(), diag.clone()));
        }
        Subscription { hub: self.clone(), rx: self.tx.subscribe(), owner }
    }

    /// Para tarefa de dentro do servidor: conta como convidado, fora de `set_owner_clients`, senão o
    /// push do celular acharia o app aberto.
    pub(crate) fn subscribe_internal(self: &Arc<Self>, list: &Arc<ListBridge>, diag: &DiagClient) -> InternalSubscription {
        InternalSubscription(self.subscribe(list, diag, false))
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
}

/// O que o produtor guarda entre tiques para só publicar mudança.
#[derive(Default)]
struct Last { sig: Option<String>, nav: Vec<(String, crate::nav_requests::Marker)>, shortcuts: Option<String>,
    keyring: Option<crate::keyring::KeyringState> }

/// Pânico fora da produção (assinatura, publicação) não deixa o hub sem produtor: as listas abertas
/// ficariam só com `ping`, e nenhuma nova religaria.
async fn supervise(hub: Arc<ListHub>, list: Arc<ListBridge>, diag: DiagClient) {
    loop {
        match tokio::spawn(produce(hub.clone(), list.clone(), diag.clone())).await {
            Err(e) if e.is_panic() => {
                diag.report("rust.list_failed", "", "list_producer_panic", "produtor da lista do dono caiu; recomeça");
                tokio::time::sleep(TICK).await;
            }
            _ => return,
        }
    }
}

/// O laço único. Para quando a última lista fecha; quem abrir depois religa.
async fn produce(hub: Arc<ListHub>, list: Arc<ListBridge>, diag: DiagClient) {
    let mut last = Last::default();
    let mut files = FileWake::new();
    let mut keyring = list.keyring.subscribe();
    loop {
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
        publish(&hub.tx, &mut last, outcome, &diag);
        publish_nav(&hub.tx, &mut last, list.nav.live(crate::nav_requests::now()));
        publish_keyring(&hub.tx, &mut last, *keyring.borrow_and_update());
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
                w = files.recv() => w,
            };
            tokio::time::sleep(COALESCE).await;
            let Some(asked) = files.drain(first, &diag) else { break };
            match list.reclassify(asked).await {
                Ok(Partial::Done(p)) => publish(&hub.tx, &mut last, Ok(p), &diag),
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

fn publish(tx: &watch::Sender<Arc<Published>>, last: &mut Last, outcome: Result<Produced, &'static str>, diag: &DiagClient) {
    let prev = tx.borrow().clone();
    let mut next = (*prev).clone();
    let mut changed = false;
    match outcome {
        Err(code) => changed |= fail(&prev, &mut next, code),
        Ok(p) => {
            let rows = visible(&p);
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
struct Sent { sessions: u64, error: u64, shortcuts: u64, keyring: u64, nav: HashMap<String, f64> }

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

/// `GET /api/sessions/events` do dono e do convidado da 8766.
pub async fn events(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let (fwd, who) = who(&st, peer, &req);
    let grant = match &who {
        Some(caller) if req.method() == Method::GET => caller.grant().cloned(),
        _ => return pass(&st, req, &fwd).await,
    };
    let (tx, rx) = mpsc::channel::<Bytes>(16);
    match (grant, st.share.clone()) {
        (None, _) => { tokio::spawn(client_loop(st.hub.subscribe(&st.list, &st.diag, true), tx)); }
        (Some(grant), Some(share)) => {
            tokio::spawn(guest_loop(st.hub.subscribe(&st.list, &st.diag, false), share, grant.token.clone(), tx));
        }
        // Convidado sem convites no Rust não existe: o porteiro nem sobe.
        (Some(_), None) => return pass(&st, req, &fwd).await,
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
    let (fwd, who) = who(&st, peer, &req);
    if who.is_none() || req.method() != Method::GET {
        return pass(&st, req, &fwd).await;
    }
    // Relido agora: o grant do porteiro é desta mesma requisição, mas o registro é a fonte.
    let guest = match who.as_ref().and_then(|c| c.grant()) {
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
    let body = match &guest {
        None => serde_json::to_vec(&visible(&produced)),
        Some(guest) => serde_json::to_vec(&guest_rows(visible(&produced), guest)),
    };
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
    let detail = if e.code == "mux_unavailable" {
        json!({"code": "erro_mux_indisponivel", "params": {"detalhe": e.detail},
            "msg": "o tmux não respondeu — a lista de sessões está indisponível"})
    } else {
        json!({"code": "erro_lista_indisponivel", "params": {"detalhe": e.code}, "msg": "a lista de sessões está indisponível"})
    };
    let mut resp = (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "application/json")],
        json!({"detail": detail}).to_string()).into_response();
    cors(req, resp.headers_mut());
    resp
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

    #[test]
    fn nav_map_is_pruned_with_the_request() {
        let mut sent = Sent::default();
        let marker = crate::nav_requests::Marker { url: "u".into(), ts: 1.0 };
        let p = Published { nav: Arc::new(nav_entries(&[("a".into(), marker)])), ..Default::default() };
        assert_eq!(frames(&p, &mut sent).len(), 1);
        frames(&Published::default(), &mut sent);
        assert!(sent.nav.is_empty());
    }
}
