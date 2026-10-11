//! Terminal real do dono: `WS /api/sessions/{name}/term` (`?shortcut=`) e
//! `WS /api/hangar-terminals/{ident}/term`. Um cano de bytes para o `tmux attach` da sessão; o
//! servidor não interpreta nada. Quem não é o dono pelo `?token=` segue ao Python (convidado,
//! token errado, bloqueio), que faz a porta de entrada e volta por `/__hangar_server/term`.
pub(crate) mod origin;
mod pty;
mod resolve;
#[cfg(all(test, unix))]
mod tests;
#[cfg(all(test, windows))]
#[path = "conpty_tests.rs"]
mod conpty;

use std::collections::HashMap;
use std::ffi::OsString;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{ConnectInfo, FromRequestParts, Path, Request, State};
use axum::http::{Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Notify, mpsc, watch};

use crate::auth;
use crate::proxy::Forward;
use crate::routes::{AppState, pass};

const TAKEN_OVER: &str = "outra conexao assumiu";

#[derive(Clone)]
pub struct TermConfig {
    pub program: OsString,
    pub socket: Option<PathBuf>,
    /// Ping e prazo do pong: os 20 s / 20 s do uvicorn. Sem isso, o celular que perdeu a rede
    /// segura o painel e o tamanho da janela.
    pub ping_every: Duration,
    pub ping_timeout: Duration,
    /// Teto de painéis vivos, contando os que ainda desmontam (as threads deles).
    pub max_panels: usize,
    pub mux_timeout: Duration,
}

impl Default for TermConfig {
    fn default() -> Self {
        Self { program: "tmux".into(), socket: None, ping_every: Duration::from_secs(20),
               ping_timeout: Duration::from_secs(20), max_panels: 64, mux_timeout: Duration::from_secs(5) }
    }
}

/// Os painéis abertos, um por alvo tmux: dois clientes com `window-size latest` brigariam pelo
/// tamanho a cada quadro.
pub struct Terms {
    pub cfg: TermConfig,
    panels: Mutex<HashMap<String, Panel>>,
    live: Arc<AtomicUsize>,
    next_id: AtomicU64,
}

struct Panel {
    id: u64,
    stop: Arc<Notify>,
    done: watch::Receiver<bool>,
}

struct Claim {
    id: u64,
    stop: Arc<Notify>,
    done: watch::Sender<bool>,
}

/// Vaga no teto de painéis; sai quando o último dono (conexão, leitor, escritor) termina.
pub(crate) struct Slot(Arc<AtomicUsize>);

impl Drop for Slot {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::AcqRel); }
}

impl Default for Terms {
    fn default() -> Self { Self::new(TermConfig::default()) }
}

impl Terms {
    pub fn new(cfg: TermConfig) -> Self {
        Self { cfg, panels: Mutex::default(), live: Arc::default(), next_id: AtomicU64::new(1) }
    }

    /// Alvos com painel aberto.
    pub fn active(&self) -> Vec<String> {
        self.panels.lock().unwrap().keys().cloned().collect()
    }

    pub fn is_active(&self, target: &str) -> bool {
        self.panels.lock().unwrap().contains_key(target)
    }

    /// Só para teste de rota: marca o alvo como de painel aberto, sem abrir PTY.
    #[doc(hidden)]
    pub async fn mark_open_for_test(&self, target: &str, diag: &crate::diag::DiagClient) { let _ = self.claim(target, diag).await; }

    fn slot(&self) -> Option<Arc<Slot>> {
        self.live.fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| (n < self.cfg.max_panels).then_some(n + 1))
            .ok().map(|_| Arc::new(Slot(self.live.clone())))
    }

    /// Registra o painel e derruba o anterior do mesmo alvo, esperando ele desmontar: o tamanho
    /// que ele repõe na saída não pode cair por cima do novo.
    async fn claim(&self, target: &str, diag: &crate::diag::DiagClient) -> Claim {
        let (done, done_rx) = watch::channel(false);
        let stop = Arc::new(Notify::new());
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let previous = self.panels.lock().unwrap()
            .insert(target.to_string(), Panel { id, stop: stop.clone(), done: done_rx });
        if let Some(mut previous) = previous {
            previous.stop.notify_one();
            if tokio::time::timeout(Duration::from_secs(10), previous.done.wait_for(|d| *d)).await.is_err() {
                diag.report("rust.term_failed", target, "takeover_timeout", "o painel anterior não desmontou em 10 s");
            }
        }
        Claim { id, stop, done }
    }

    fn release(&self, target: &str, claim: &Claim) {
        let mut panels = self.panels.lock().unwrap();
        if panels.get(target).is_some_and(|p| p.id == claim.id) {
            panels.remove(target);
        }
        drop(panels);
        let _ = claim.done.send(true);
    }

    /// Repõe o tamanho das sessões que um Rust anterior deixou no tamanho do painel.
    pub async fn restore_after_crash(&self) {
        pty::restore_after_crash(&self.cfg, |name| self.panels.lock().unwrap().contains_key(name)).await;
    }
}

enum Want {
    Session { name: String, shortcut: Option<String> },
    Hangar(String),
}

pub async fn session_ws(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
                        Path(name): Path<String>, req: Request) -> Response {
    let shortcut = auth::query_param(req.uri().query(), "shortcut");
    serve(st, peer, req, Want::Session { name, shortcut }).await
}

pub async fn hangar_ws(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>,
                       Path(ident): Path<String>, req: Request) -> Response {
    serve(st, peer, req, Want::Hangar(ident)).await
}

/// `/__hangar_server/term` na porta privada: o Python liga aqui quem entrou pelas portas dele
/// (convidado, Connect) depois da porta de entrada dele. O alvo já vem conferido; o painel é o
/// mesmo da 8765, então um dono e um convidado nunca anexam juntos.
pub async fn private_ws(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let query = req.uri().query();
    let Some(target) = auth::query_param(query, "target").filter(|t| !t.is_empty()) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some((cols, rows)) = size_of(query) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let (mut parts, _) = req.into_parts();
    let Ok(ws) = WebSocketUpgrade::from_request_parts(&mut parts, &()).await else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // Conferido de novo aqui: a sessão pode ter morrido entre a porta do Python e esta conexão,
    // e um painel nela registraria um painel fantasma no `term.active`.
    match resolve::has_session(&st.term.cfg, &target).await {
        Ok(true) => {}
        Ok(false) => return refuse("sessao nao existe"),
        Err(resolve::MuxDown) => return mux_down(&st, ws, &target),
    }
    let Some(slot) = st.term.slot() else {
        st.diag.report("rust.term_failed", &target, "panel_cap", "teto de painéis de terminal atingido");
        return close_after_accept(ws, 1013, "limite de paineis");
    };
    ws.on_upgrade(move |socket| run(st, socket, target, cols, rows, slot))
}

/// `cols`/`rows` da query com o clamp; ausente vale 80x24, inválido é `None`.
fn size_of(query: Option<&str>) -> Option<(u16, u16)> {
    let number = |key, default: i64| match auth::query_param(query, key) {
        None => Some(default),
        Some(v) => v.trim().parse::<i64>().ok(),
    };
    Some(pty::clamp(number("cols", 80)?, number("rows", 24)?))
}

/// Recusa antes do aceite: o que o `ws.close` antes do `accept` do Python vira, um 403.
fn refuse(reason: &'static str) -> Response {
    (StatusCode::FORBIDDEN, reason).into_response()
}

/// Aceita só para fechar com código: 1013 é "tente de novo", e o cliente precisa ler o motivo.
fn close_after_accept(ws: WebSocketUpgrade, code: u16, reason: &'static str) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        let _ = socket.send(Message::Close(Some(CloseFrame { code, reason: reason.into() }))).await;
    })
}

async fn serve(st: Arc<AppState>, peer: SocketAddr, req: Request, want: Want) -> Response {
    let (client_ip, https) = st.cfg.trusted.resolve(peer.ip(), req.headers());
    let fwd = Forward { client_ip, https };
    // Só `?token=`, como o termsock: Bearer e cookie não abrem shell.
    let owner = auth::query_param(req.uri().query(), "token").filter(|t| !t.is_empty())
        .is_some_and(|t| st.auth.is_owner(&fwd.client_ip, Some(t.as_bytes())));
    if !owner || req.method() != Method::GET {
        return pass(&st, req, &fwd).await;
    }
    let (mut parts, body) = req.into_parts();
    let ws = match WebSocketUpgrade::from_request_parts(&mut parts, &()).await {
        Ok(ws) => ws,
        Err(_) => return pass(&st, Request::from_parts(parts, body), &fwd).await,
    };
    let label = match &want { Want::Session { name, .. } => name.clone(), Want::Hangar(_) => "hangar".into() };
    if let Some(origin) = parts.headers.get(header::ORIGIN).filter(|o| !o.is_empty()) {
        // Origin que nem é texto não passa como "sem Origin".
        let Ok(origin) = origin.to_str() else { return refuse("origem recusada") };
        let host = parts.headers.get(header::HOST).and_then(|v| v.to_str().ok());
        match origin::ask(&st.http, st.cfg.upstream, &st.cfg.internal_secret, origin, host).await {
            Ok(true) => {}
            Ok(false) => {
                tracing::warn!(session = %label, "terminal: origem recusada");
                return refuse("origem recusada");
            }
            Err(code) => {
                st.diag.report("rust.term_failed", &label, code, "a origem do terminal não foi conferida");
                let body = serde_json::json!({"ok": false, "error_code": code,
                    "message": "a origem do terminal não foi conferida"}).to_string();
                return (StatusCode::SERVICE_UNAVAILABLE, [(header::CONTENT_TYPE, "application/json")], body).into_response();
            }
        }
    }
    let cfg = &st.term.cfg;
    let resolved = match &want {
        Want::Session { name, shortcut: None } => Ok(Some(name.clone())),
        Want::Session { name, shortcut: Some(id) } => resolve::shortcut(cfg, name, id).await,
        Want::Hangar(id) => resolve::shortcut(cfg, "", id).await,
    };
    let target = match resolved {
        Ok(Some(t)) => t,
        Ok(None) => return refuse("terminal nao existe"),
        Err(resolve::MuxDown) => return mux_down(&st, ws, &label),
    };
    match resolve::has_session(cfg, &target).await {
        Ok(true) => {}
        Ok(false) => return refuse("sessao nao existe"),
        Err(resolve::MuxDown) => return mux_down(&st, ws, &label),
    }
    let Some((cols, rows)) = size_of(parts.uri.query()) else {
        return refuse("cols/rows invalidos");
    };
    let Some(slot) = st.term.slot() else {
        st.diag.report("rust.term_failed", &label, "panel_cap", "teto de painéis de terminal atingido");
        return close_after_accept(ws, 1013, "limite de paineis");
    };
    ws.on_upgrade(move |socket| run(st, socket, target, cols, rows, slot))
}

fn mux_down(st: &AppState, ws: WebSocketUpgrade, label: &str) -> Response {
    st.diag.report("rust.term_failed", label, "mux_unavailable", "o multiplexador não respondeu");
    close_after_accept(ws, 1013, "multiplexador indisponivel")
}

enum End { Client, Output, TakenOver, PingTimeout, InputGone }

async fn run(st: Arc<AppState>, socket: WebSocket, target: String, cols: u16, rows: u16, slot: Arc<Slot>) {
    let terms = &st.term;
    let cfg = &terms.cfg;
    let claim = terms.claim(&target, &st.diag).await;
    let close = |code, reason: &'static str| Message::Close(Some(CloseFrame { code, reason: reason.into() }));
    // Outra conexão já assumiu enquanto esta esperava a anterior: nem chega a anexar.
    if futures_util::FutureExt::now_or_never(claim.stop.notified()).is_some() {
        let mut socket = socket;
        let _ = socket.send(close(1000, TAKEN_OVER)).await;
        terms.release(&target, &claim);
        return;
    }
    // Lido ANTES do attach: depois dele a janela já está no tamanho do painel.
    let saved = pty::remember_size(cfg, &target).await;
    let open_cfg = cfg.clone();
    let open_target = target.clone();
    let opened = tokio::task::spawn_blocking(move || pty::open(&open_cfg, &open_target, cols, rows, slot)).await
        .unwrap_or(Err("pty_panic"));
    let pty::Opened { pty, mut output, input, held_failure } = match opened {
        Ok(o) => o,
        Err(code) => {
            st.diag.report("rust.term_failed", &target, code, "o terminal não abriu");
            let mut socket = socket;
            let _ = socket.send(close(1011, "terminal nao abriu")).await;
            if let Some(saved) = saved {
                if let Err(code) = pty::restore_size(cfg, &target, saved).await {
                    st.diag.report("rust.term_failed", &target, code, "o tamanho da janela não foi reposto");
                }
            }
            terms.release(&target, &claim);
            return;
        }
    };
    tracing::info!(session = %target, cols, rows, "terminal: anexado");
    if let Some(failed) = held_failure {
        let (diag, target) = (st.diag.clone(), target.clone());
        tokio::spawn(async move {
            if let Ok(code) = failed.await {
                diag.report("rust.term_failed", &target, code, "a entrada do terminal não esperou o sinal de que o attach lê o teclado, ou não foi escrita");
            }
        });
    }
    let (mut sink, mut stream) = socket.split();
    let (ctl, mut ctl_rx) = mpsc::channel::<Message>(4);
    let mut sender = tokio::spawn(async move {
        loop {
            tokio::select! {
                biased;
                m = ctl_rx.recv() => match m {
                    Some(m) => {
                        let close = matches!(m, Message::Close(_));
                        if sink.send(m).await.is_err() || close { return }
                    }
                    None => return,
                },
                chunk = output.recv() => match chunk {
                    Some(b) => if sink.send(Message::Binary(b)).await.is_err() { return },
                    // O PTY acabou (`exit`, sessão morta): a tela inteira já saiu, fecha normal.
                    None => {
                        let _ = sink.send(Message::Close(Some(CloseFrame { code: 1000, reason: "".into() }))).await;
                        return;
                    }
                },
            }
        }
    });
    let start = tokio::time::Instant::now();
    let mut ping = tokio::time::interval_at(start + cfg.ping_every, cfg.ping_every);
    let mut pong_due: Option<tokio::time::Instant> = None;
    let mut warned = false;
    let end = loop {
        tokio::select! {
            _ = claim.stop.notified() => break End::TakenOver,
            _ = &mut sender => break End::Output,
            // Fila de controle cheia é o envio parado: cliente que não lê nem um ping no prazo morreu.
            _ = ping.tick() => if pong_due.is_none() {
                match tokio::time::timeout(cfg.ping_timeout, ctl.send(Message::Ping(Default::default()))).await {
                    Ok(Ok(())) => pong_due = Some(tokio::time::Instant::now() + cfg.ping_timeout),
                    Ok(Err(_)) => break End::Output,
                    Err(_) => break End::PingTimeout,
                }
            },
            _ = tokio::time::sleep_until(pong_due.unwrap_or(start)), if pong_due.is_some() => break End::PingTimeout,
            msg = stream.next() => match msg {
                // Escrita parada (tmux sem ler) não pode segurar a troca de painel.
                Some(Ok(Message::Binary(b))) => tokio::select! {
                    sent = input.send(b) => if sent.is_err() { break End::InputGone },
                    _ = claim.stop.notified() => break End::TakenOver,
                },
                Some(Ok(Message::Text(t))) => match resize_of(t.as_str()) {
                    Ok(Some((c, r))) => pty.resize(c, r),
                    Ok(None) => {}
                    Err(()) if !warned => {
                        warned = true;
                        tracing::warn!(session = %target, "terminal: quadro de controle inválido descartado (aviso único)");
                    }
                    Err(()) => {}
                },
                Some(Ok(Message::Pong(_))) => pong_due = None,
                Some(Ok(Message::Ping(_))) => {}
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break End::Client,
            },
        }
    };
    let farewell = match end {
        End::TakenOver => Some(close(1000, TAKEN_OVER)),
        End::PingTimeout => {
            tracing::info!(session = %target, "terminal: sem pong no prazo; painel fechado");
            Some(close(1011, "sem resposta ao ping"))
        }
        End::InputGone => {
            st.diag.report("rust.term_failed", &target, "input_write_failed", "a entrada do terminal parou de ser aceita");
            Some(close(1011, "entrada do terminal falhou"))
        }
        End::Output | End::Client => None,
    };
    if let Some(m) = farewell {
        // `try_send`: com o envio parado, o fechamento não espera a fila.
        if ctl.try_send(m).is_ok() {
            let _ = tokio::time::timeout(Duration::from_secs(1), &mut sender).await;
        }
    }
    sender.abort();
    // Socket solto antes da desmontagem, que leva segundos: o cliente vê o fechamento na hora.
    drop(stream);
    drop(input);
    if let Err(code) = pty::teardown(cfg, &target, pty, saved).await {
        st.diag.report("rust.term_failed", &target, code, "a desmontagem do painel não terminou limpa");
    }
    terms.release(&target, &claim);
    tracing::info!(session = %target, "terminal: desanexado");
}

/// `{"t":"resize","cols":..,"rows":..}` com o clamp da abertura. Como o termsock: JSON que não é
/// pedido de resize se ignora (`Ok(None)`); JSON torto ou resize sem número vira `Err`. O `int()`
/// do Python aceita número, fração e texto numérico.
fn resize_of(text: &str) -> Result<Option<(u16, u16)>, ()> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| ())?;
    if v.get("t").and_then(serde_json::Value::as_str) != Some("resize") {
        return Ok(None);
    }
    let int = |x: Option<&serde_json::Value>| {
        let x = x?;
        x.as_i64().or_else(|| x.as_f64().filter(|f| f.is_finite()).map(|f| f as i64))
            .or_else(|| x.as_str()?.trim().parse().ok())
    };
    match (int(v.get("cols")), int(v.get("rows"))) {
        (Some(c), Some(r)) => Ok(Some(pty::clamp(c, r))),
        _ => Err(()),
    }
}
