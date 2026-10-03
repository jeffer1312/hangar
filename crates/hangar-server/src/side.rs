// crates/hangar-server/src/side.rs
//! Uma conexão interna por sessão (Python → hangar-server) e o hub que reparte, entre os
//! aparelhos daquele chat, o que vem dela e o que o leitor do transcript produz.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::StatusCode;
use bytes::Bytes;
use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::StreamExt;
use http_body_util::BodyDataStream;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::sync::broadcast;

use crate::proxy::HttpClient;
use crate::tail::{self, FileTail, Watchers, sse_frame};
use crate::transcript::{InternalInfo, Provider};

#[derive(Clone, Debug)]
pub enum Out {
    /// Quadro do leitor do transcript, marcado com a geração da ligação que o leu.
    Tail(u64, Bytes),
    /// Quadro da conexão interna (estado, prévia, fila…); vale em qualquer geração.
    Side(Bytes),
    /// O transcript ou o provider trocou: cada aparelho manda `reset` e refaz a cauda.
    Rebind,
    /// Provider fora do Rust ou sessão sumida: `reset` e fim; ao reconectar, vai ao Python.
    Close,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Binding {
    pub provider: Provider,
    pub jsonl: PathBuf,
    pub key: String,
}

impl Binding {
    /// Mesmo critério de `InternalInfo::history_request`: provider lido pelo Rust e com jsonl.
    pub fn from_info(info: &InternalInfo) -> Option<Binding> {
        Some(Binding {
            provider: Provider::parse(&info.provider)?,
            jsonl: info.jsonl.clone()?,
            key: info.session_key.clone(),
        })
    }
}

pub const INFO_TTL: Duration = Duration::from_secs(1);
pub type InfoCache = Arc<Mutex<HashMap<String, (Instant, Option<InternalInfo>)>>>;

/// Guarda o `info` mais recente; o da conexão interna também entra, para quem chega logo depois
/// de uma troca não religar o hub com um `info` velho.
pub fn remember_info(cache: &InfoCache, name: &str, info: Option<InternalInfo>) {
    let mut m = cache.lock().unwrap();
    if m.len() > 256 {
        m.retain(|_, (at, _)| at.elapsed() < INFO_TTL);
    }
    m.insert(name.to_string(), (Instant::now(), info));
}

/// Eventos cujo último valor vale para quem chega depois: o Python só os manda na mudança.
/// `nav` fica de fora: repetir um pedido já atendido reabriria o navegador; quem chega depois o
/// recebe pela lista de sessões.
const LATEST: [&str; 7] = ["state", "suggest", "ask_question", "stats", "preview", "pensamento", "ferramenta"];
const ASK_QUESTION: usize = 2;
const CHANNEL: usize = 1024;
const SIDE_CONNECT: Duration = Duration::from_secs(10);
/// O Python manda `ping` a cada 10 s; três calados = conexão morta.
const SIDE_IDLE: Duration = Duration::from_secs(30);

#[derive(Clone)]
pub struct SideCtx {
    pub upstream: SocketAddr,
    pub secret: String,
    pub http: HttpClient,
    pub watchers: Watchers,
    pub hubs: Hubs,
    pub infos: InfoCache,
}

#[derive(Default)]
struct SideCache {
    latest: [Option<Bytes>; 7],
    queue: Vec<(String, Bytes)>,
}

impl SideCache {
    /// `pane_question`: Claude com terminal, cujo `ask_question` sai uma vez por pergunta e nada o
    /// apaga depois. Só vale enquanto o último `state` for `awaiting_input`; senão quem chega
    /// depois abriria uma pergunta já respondida. Codex e Claude sem terminal mandam o próprio
    /// `ask_question` vazio ao fechar.
    fn record(&mut self, event: &str, data: &str, frame: &Bytes, pane_question: bool) {
        if let Some(i) = LATEST.iter().position(|e| *e == event) {
            self.latest[i] = Some(frame.clone());
            if event == "state" && pane_question {
                let awaiting = serde_json::from_str::<serde_json::Value>(data)
                    .ok()
                    .is_some_and(|v| v.get("state").and_then(|s| s.as_str()) == Some("awaiting_input"));
                if !awaiting {
                    self.latest[ASK_QUESTION] = None;
                }
            }
            return;
        }
        if event != "message" && event != "queue_confirmed" {
            return;
        }
        let id = serde_json::from_str::<serde_json::Value>(data)
            .ok()
            .and_then(|v| v.get("id")?.as_str().map(str::to_owned));
        let Some(id) = id else { return };
        match self.queue.iter_mut().find(|(k, _)| *k == id) {
            Some(slot) => slot.1 = frame.clone(),
            None => self.queue.push((id, frame.clone())),
        }
    }

    fn replay(&self) -> Vec<Bytes> {
        self.latest.iter().flatten().cloned().chain(self.queue.iter().map(|(_, f)| f.clone())).collect()
    }
}

#[derive(Clone)]
struct Bound {
    binding: Binding,
    generation: u64,
    tail: Arc<FileTail>,
}

pub struct Hub {
    pub name: String,
    pub tx: broadcast::Sender<Out>,
    ctx: SideCtx,
    bound: Mutex<Option<Bound>>,
    cache: Mutex<SideCache>,
    side: Mutex<Option<tokio::task::AbortHandle>>,
}

/// O que um aparelho recebe ao entrar: cauda + retrato, e o canal para o resto.
pub struct Attach {
    pub generation: u64,
    pub rx: broadcast::Receiver<Out>,
    pub frames: Vec<Bytes>,
}

impl Hub {
    fn start(name: &str, binding: Binding, ctx: SideCtx) -> Arc<Hub> {
        let (tx, _) = broadcast::channel(CHANNEL);
        let hub = Arc::new_cyclic(|weak| {
            let tail = FileTail::spawn(
                binding.jsonl.clone(),
                binding.key.clone(),
                binding.provider,
                0,
                tx.clone(),
                ctx.watchers.clone(),
                close_on_tail_death(weak.clone(), 0),
            );
            Hub {
                name: name.to_string(),
                tx,
                ctx,
                bound: Mutex::new(Some(Bound { binding, generation: 0, tail })),
                cache: Mutex::default(),
                side: Mutex::new(None),
            }
        });
        hub.restart_side();
        hub
    }

    /// Aparelho novo com `info` diferente (sessão recriada com o mesmo nome): troca o leitor já,
    /// para ele nunca receber a cauda do transcript morto, e religa a conexão interna, cujo
    /// primeiro `info` confirma ou corrige.
    /// Hub já fechado (removido do mapa): nada a fazer; o `attach` dá `None` e o aparelho recebe
    /// `reset` e volta ao Python.
    fn ensure_current(self: &Arc<Self>, binding: &Binding) {
        let same = match self.bound.lock().unwrap().as_ref() {
            None => return,
            Some(b) => b.binding == *binding,
        };
        if !same {
            self.rebind(binding.clone());
            self.restart_side();
        }
    }

    /// Confere `bound` sob a trava de `side`: o `close` solta `bound` antes de pegar essa trava,
    /// então ou a conexão nova nem nasce, ou o `close` a encontra e derruba. Fora do mapa, nenhum
    /// `Lease` a pararia e ela seguiria aberta com app=1.
    fn restart_side(self: &Arc<Self>) {
        let mut side = self.side.lock().unwrap();
        if let Some(h) = side.take() {
            h.abort();
        }
        if self.bound.lock().unwrap().is_none() {
            return;
        }
        *side = Some(tokio::spawn(run_side(self.clone())).abort_handle());
    }

    fn stop(&self) {
        if let Some(h) = self.side.lock().unwrap().take() {
            h.abort();
        }
        self.bound.lock().unwrap().take();
    }

    fn rebind(self: &Arc<Self>, binding: Binding) {
        let mut bound = self.bound.lock().unwrap();
        let Some(cur) = bound.as_ref() else { return };
        let generation = cur.generation + 1;
        let tail = FileTail::spawn(
            binding.jsonl.clone(),
            binding.key.clone(),
            binding.provider,
            generation,
            self.tx.clone(),
            self.ctx.watchers.clone(),
            close_on_tail_death(Arc::downgrade(self), generation),
        );
        *bound = Some(Bound { binding, generation, tail });
        self.cache.lock().unwrap().latest = Default::default();
        let _ = self.tx.send(Out::Rebind);
    }

    fn close(self: &Arc<Self>) {
        self.ctx.hubs.evict(&self.name, self);
        self.bound.lock().unwrap().take();
        // Pode ser uma conexão religada por `ensure_current` no meio; a própria sai logo depois.
        if let Some(h) = self.side.lock().unwrap().take() {
            h.abort();
        }
        let _ = self.tx.send(Out::Close);
    }

    /// Assina o canal e lê o ponto do leitor sob a trava dele (que é quem envia): nenhuma linha
    /// cai entre a cauda e o ao vivo. Troca de ligação no meio = tenta de novo.
    pub async fn attach(&self, resume: Option<String>) -> Option<Attach> {
        loop {
            let b = self.bound.lock().unwrap().clone()?;
            let guard = b.tail.state.clone().lock_owned().await;
            let rx = self.tx.subscribe();
            if self.bound.lock().unwrap().as_ref().map(|x| x.generation) != Some(b.generation) {
                continue;
            }
            let cached = self.cache.lock().unwrap().replay();
            let binding = b.binding.clone();
            let resume = resume.clone();
            let done = tokio::task::spawn_blocking(move || {
                let mut g = guard;
                // Erro já registrado no `cut`: o aparelho recebe `reset` e reconecta.
                let cut = g.cut().ok()?;
                drop(g);
                Some(tail::backfill(&binding.jsonl, &binding.key, binding.provider, resume.as_deref(), cut))
            })
            .await;
            let mut frames = match done {
                Ok(f) => f?,
                Err(e) => {
                    // Sem a mensagem do pânico: ela pode citar o texto da linha.
                    tracing::error!(session = %self.name, panic = e.is_panic(), "cauda do aparelho caiu; reset");
                    return None;
                }
            };
            frames.extend(cached);
            return Some(Attach { generation: b.generation, rx, frames });
        }
    }
}

/// Leitor morto fecha o hub da ligação dele: os aparelhos recebem `reset`, reconectam e o hub novo
/// nasce com leitor novo. Uma troca de ligação já o substituiu, então a geração confere.
fn close_on_tail_death(hub: Weak<Hub>, generation: u64) -> tail::OnDead {
    Box::new(move || {
        let Some(hub) = hub.upgrade() else { return };
        if hub.bound.lock().unwrap().as_ref().is_some_and(|b| b.generation == generation) {
            hub.close();
        }
    })
}

enum SideEnd {
    Gone,
    Retry,
}

async fn run_side(hub: Arc<Hub>) {
    let mut attempt = 0u32;
    loop {
        match side_once(&hub, &mut attempt).await {
            SideEnd::Gone => {
                hub.close();
                return;
            }
            SideEnd::Retry => {}
        }
        // Religa com espera crescente: 1, 2, 4… até 30 s.
        let delay = (1u64 << attempt.min(5)).min(30);
        attempt = attempt.saturating_add(1);
        tokio::time::sleep(Duration::from_secs(delay)).await;
    }
}

async fn side_once(hub: &Arc<Hub>, attempt: &mut u32) -> SideEnd {
    let url = format!(
        "http://{}/internal/sessions/{}/side-events?app=1",
        hub.ctx.upstream,
        utf8_percent_encode(&hub.name, NON_ALPHANUMERIC)
    );
    let req = match axum::http::Request::get(url).header("x-hangar-internal", &hub.ctx.secret).body(Body::empty()) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(session = %hub.name, "conexão interna: pedido inválido: {e}");
            return SideEnd::Gone;
        }
    };
    let resp = match tokio::time::timeout(SIDE_CONNECT, hub.ctx.http.request(req)).await {
        Ok(Ok(r)) => r,
        Ok(Err(e)) => {
            tracing::warn!(session = %hub.name, "conexão interna falhou: {e}");
            return SideEnd::Retry;
        }
        Err(_) => {
            tracing::warn!(session = %hub.name, "conexão interna sem resposta");
            return SideEnd::Retry;
        }
    };
    if resp.status() == StatusCode::NOT_FOUND {
        // O Python dá 404 também ao segredo recusado: com o hub já ligado, vale o aviso.
        tracing::warn!(session = %hub.name, "conexão interna: 404 (sessão sumiu ou segredo interno recusado)");
        remember_info(&hub.ctx.infos, &hub.name, None);
        return SideEnd::Gone;
    }
    if !resp.status().is_success() {
        tracing::warn!(session = %hub.name, status = %resp.status(), "conexão interna recusada");
        return SideEnd::Retry;
    }
    let events = BodyDataStream::new(resp.into_body()).eventsource();
    let mut events = std::pin::pin!(events);
    let mut first = true;
    loop {
        let ev = match tokio::time::timeout(SIDE_IDLE, events.next()).await {
            Ok(Some(Ok(ev))) => ev,
            Ok(Some(Err(e))) => {
                // Só o tipo do erro: o do parser carrega o trecho lido, que pode ser conversa.
                let kind = match e {
                    EventStreamError::Utf8(_) => "utf8",
                    EventStreamError::Parser(_) => "parser",
                    EventStreamError::Transport(_) => "transport",
                };
                tracing::warn!(session = %hub.name, kind, "conexão interna: leitura falhou");
                return SideEnd::Retry;
            }
            Ok(None) => return SideEnd::Retry,
            Err(_) => {
                tracing::warn!(session = %hub.name, "conexão interna calada há 30 s; religa");
                return SideEnd::Retry;
            }
        };
        match ev.event.as_str() {
            "info" => {
                let info = match serde_json::from_str::<InternalInfo>(&ev.data) {
                    Ok(i) => i,
                    Err(e) => {
                        tracing::warn!(session = %hub.name, line = e.line(), column = e.column(), "info interna inválida");
                        return SideEnd::Retry;
                    }
                };
                remember_info(&hub.ctx.infos, &hub.name, Some(info.clone()));
                if first {
                    // Conexão nova manda a fila inteira de novo: o retrato anterior sai.
                    hub.cache.lock().unwrap().queue.clear();
                    *attempt = 0;
                    first = false;
                }
                let Some(binding) = Binding::from_info(&info) else {
                    tracing::info!(session = %hub.name, provider = %info.provider, "provider fora do Rust; aparelhos voltam ao Python");
                    return SideEnd::Gone;
                };
                let same = hub.bound.lock().unwrap().as_ref().is_some_and(|b| b.binding == binding);
                if !same {
                    tracing::info!(session = %hub.name, "transcript ou provider trocou; reset nos aparelhos");
                    hub.rebind(binding);
                }
            }
            "ping" => {}
            event => {
                let frame = sse_frame(event, &ev.data, None);
                // Retrato antes do envio: quem assina entre os dois recebe repetido, nunca nada.
                let pane_question =
                    hub.bound.lock().unwrap().as_ref().is_some_and(|b| b.binding.provider == Provider::Claude);
                hub.cache.lock().unwrap().record(event, &ev.data, &frame, pane_question);
                let _ = hub.tx.send(Out::Side(frame));
            }
        }
    }
}

/// Hubs vivos por nome de sessão, com a contagem de aparelhos.
#[derive(Clone, Default)]
pub struct Hubs(Arc<Mutex<HashMap<String, (Arc<Hub>, usize)>>>);

pub struct Lease {
    hubs: Hubs,
    pub hub: Arc<Hub>,
}

impl Hubs {
    pub fn acquire(&self, name: &str, binding: Binding, ctx: &SideCtx) -> Lease {
        let mut map = self.0.lock().unwrap();
        let hub = match map.get_mut(name) {
            Some((hub, n)) => {
                *n += 1;
                hub.clone()
            }
            None => {
                let hub = Hub::start(name, binding, ctx.clone());
                map.insert(name.to_string(), (hub.clone(), 1));
                return Lease { hubs: self.clone(), hub };
            }
        };
        drop(map);
        hub.ensure_current(&binding);
        Lease { hubs: self.clone(), hub }
    }

    fn evict(&self, name: &str, hub: &Arc<Hub>) {
        let mut map = self.0.lock().unwrap();
        if map.get(name).is_some_and(|(h, _)| Arc::ptr_eq(h, hub)) {
            map.remove(name);
        }
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut map = self.hubs.0.lock().unwrap();
        let Some((h, n)) = map.get_mut(&self.hub.name) else { return };
        if !Arc::ptr_eq(h, &self.hub) {
            return;
        }
        *n -= 1;
        if *n == 0 {
            let (h, _) = map.remove(&self.hub.name).expect("presente acima");
            drop(map);
            // Último aparelho saiu: fecha a conexão interna (o Python roda app_saiu) e o leitor.
            h.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keeps_last_value_and_queue_by_id_without_nav() {
        let mut c = SideCache::default();
        let rec = |c: &mut SideCache, e: &str, d: &str| c.record(e, d, &sse_frame(e, d, None), true);
        rec(&mut c, "state", "{\"state\":\"working\"}");
        rec(&mut c, "state", "{\"state\":\"idle\"}");
        rec(&mut c, "message", "{\"id\":\"queued-1\"}");
        rec(&mut c, "queue_confirmed", "{\"id\":\"queued-1\",\"queued_confirmed\":true}");
        rec(&mut c, "nav", "{\"url\":\"http://x\"}");
        let r: Vec<String> = c.replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
        assert_eq!(r.len(), 2);
        assert!(r[0].starts_with("event: state") && r[0].contains("idle"));
        assert!(r[1].starts_with("event: queue_confirmed"));
    }

    fn has_question(c: &SideCache) -> bool {
        c.replay().iter().any(|b| b.starts_with(b"event: ask_question"))
    }

    #[test]
    fn pane_question_lives_only_while_awaiting_input() {
        let rec = |c: &mut SideCache, e: &str, d: &str, pane: bool| c.record(e, d, &sse_frame(e, d, None), pane);
        let mut c = SideCache::default();
        rec(&mut c, "state", "{\"state\":\"awaiting_input\"}", true);
        rec(&mut c, "ask_question", "{\"q\":1}", true);
        assert!(has_question(&c), "quem chega durante a pergunta a recebe");
        rec(&mut c, "state", "{\"state\":\"awaiting_input\"}", true);
        assert!(has_question(&c));
        rec(&mut c, "state", "{\"state\":\"idle\"}", true);
        assert!(!has_question(&c), "respondida: quem chega depois não a recebe");

        // Codex / Claude sem terminal: a pergunta vive no próprio evento, que chega vazio ao fechar.
        let mut c = SideCache::default();
        rec(&mut c, "ask_question", "{\"q\":1}", false);
        rec(&mut c, "state", "{\"state\":\"working\"}", false);
        assert!(has_question(&c));
    }

    fn idle_ctx() -> SideCtx {
        SideCtx {
            // Porta sem ninguém: a conexão interna só tenta e espera.
            upstream: "127.0.0.1:9".parse().unwrap(),
            secret: "s".into(),
            http: crate::proxy::client(),
            watchers: Watchers::default(),
            hubs: Hubs::default(),
            infos: InfoCache::default(),
        }
    }

    #[tokio::test]
    async fn dead_reader_resets_devices_and_the_next_attach_gets_a_fresh_hub() {
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("t.jsonl"), key: tail::PANIC_KEY.into() };
        let ctx = idle_ctx();
        let lease = ctx.hubs.acquire("s", binding.clone(), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        assert!(lease.hub.attach(None).await.is_none(), "cauda que caiu vira reset, não pânico calado");
        let closed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx.recv().await {
                    Ok(Out::Close) => return,
                    Err(broadcast::error::RecvError::Closed) => panic!("canal fechou sem Close"),
                    _ => {}
                }
            }
        })
        .await;
        assert!(closed.is_ok(), "leitor morto avisa os aparelhos em vez de deixá-los só com ping");
        let again = ctx.hubs.acquire("s", binding, &ctx);
        assert!(!Arc::ptr_eq(&again.hub, &lease.hub), "quem reconecta ganha hub e leitor novos");
    }

    #[tokio::test]
    async fn closed_hub_never_restarts_the_internal_connection() {
        let dir = tempfile::tempdir().unwrap();
        let binding = |n: &str| Binding { provider: Provider::Claude, jsonl: dir.path().join(n), key: n.into() };
        let ctx = idle_ctx();
        let lease = ctx.hubs.acquire("s", binding("a"), &ctx);
        assert!(lease.hub.side.lock().unwrap().is_some());
        lease.hub.close();
        assert!(lease.hub.side.lock().unwrap().is_none(), "close derruba a conexão interna");
        // O aparelho que pegou o hub antes do close chega com outro `info`.
        lease.hub.ensure_current(&binding("b"));
        lease.hub.restart_side();
        assert!(lease.hub.side.lock().unwrap().is_none(), "hub fechado não religa");
        assert!(lease.hub.bound.lock().unwrap().is_none());
    }
}
