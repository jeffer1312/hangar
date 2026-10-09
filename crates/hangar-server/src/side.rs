// crates/hangar-server/src/side.rs
//! Uma conexão interna por sessão (Python → hangar-server) e o hub que reparte, entre os
//! aparelhos daquele chat, o que vem dela e o que o leitor do transcript produz.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::http::StatusCode;
use bytes::Bytes;
use eventsource_stream::{EventStreamError, Eventsource};
use futures_util::StreamExt;
use http_body_util::BodyDataStream;
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use tokio::sync::{Notify, broadcast};

use crate::proxy::HttpClient;
use crate::tail::{self, FileTail, Watchers, sse_frame};
use crate::transcript::{InternalInfo, Provider};

#[derive(Clone, Debug)]
pub enum Out {
    /// Quadro do leitor do transcript, marcado com a geração da ligação que o leu.
    Tail(u64, Bytes),
    /// Quadro da conexão interna (estado, prévia, fila…); vale em qualquer geração.
    Side(Bytes),
    /// O `plugin_ui` mudou para a versão dada. O quadro (a vista inteira dos mods, até ~400 KB) fica só no
    /// retrato, e cada aparelho o lê na hora de enviar: aparelho lento pula os intermediários em vez de
    /// acumulá-los na fila, como o `band_pump` do Python.
    Ui(u64),
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
    /// Codex sem terminal: o estado é do feed do runtime, não do Python. O Claude sem terminal já vem
    /// no provider (`ClaudeHeadless`).
    pub headless: bool,
}

impl Binding {
    /// Eventos do estado que o Rust publica para esta ligação: os quatro do `Monitor` (Claude com
    /// terminal), os do feed (Claude e Codex sem terminal) ou nenhum (o Python observa).
    pub fn state_events(&self) -> Option<&'static [&'static str]> {
        match (self.provider, self.headless) {
            (Provider::Claude, _) => Some(&STATE_EVENTS),
            (Provider::ClaudeHeadless, _) | (Provider::Codex, true) => Some(&FEED_EVENTS),
            _ => None,
        }
    }

    /// O estado desta ligação sai do feed do runtime (o ator escreve no canal em processo).
    pub fn runtime_feed(&self) -> bool {
        matches!((self.provider, self.headless), (Provider::ClaudeHeadless, _) | (Provider::Codex, true))
    }

    /// Mesmo critério de `InternalInfo::history_request`: provider lido pelo Rust e com jsonl.
    pub fn from_info(info: &InternalInfo) -> Option<Binding> {
        Some(Binding {
            provider: Provider::parse(&info.provider)?,
            jsonl: info.jsonl.clone()?,
            key: info.session_key.clone(),
            headless: info.headless,
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
/// recebe pela lista de sessões. `plugin_ui` (faixa dos mods) sai quando muda: sem o cache, quem
/// abre o chat depois ficava sem a faixa até o mod redesenhar.
const LATEST: [&str; 8] = ["state", "suggest", "ask_question", "stats", "preview", "pensamento", "ferramenta", "plugin_ui"];
const ASK_QUESTION: usize = 2;
const PLUGIN_UI: usize = 7;
/// Mesmos tetos do Python (`plugin_bridge.TOASTS_KEPT`, `TOAST_MAX_MS`).
pub(crate) const TOASTS_KEPT: usize = 20;
pub(crate) const TOAST_MAX_MS: f64 = 5.0 * 60.0 * 1000.0;
/// Tempo que resta a um aviso: o 0 seria "sem prazo" para o app, então quem está no último
/// milissegundo ainda leva 1.
pub(crate) fn remaining_ms(until: Instant, now: Instant) -> u64 {
    (until.saturating_duration_since(now).as_millis() as u64).max(1)
}
/// O `plugin_ui` `now` só com o que mudou desde `prev`: a faixa sai só quando muda (ausente vale nula, como
/// na vista inteira), e o painel igual ao de mesmo id em `prev` vira `{id, same: true}`. O app junta com a
/// vista que já tem. Painel sem id, ou com id repetido em `prev`, vai inteiro.
fn ui_delta(prev: &serde_json::Value, now: &serde_json::Value) -> Option<serde_json::Value> {
    use serde_json::Value;
    let (prev, now) = (prev.as_object()?, now.as_object()?);
    let mut before: HashMap<&str, Option<&Value>> = HashMap::new();
    for pane in prev.get("panes").and_then(Value::as_array).into_iter().flatten() {
        if let Some(id) = pane.get("id").and_then(Value::as_str) {
            before.entry(id).and_modify(|seen| *seen = None).or_insert(Some(pane));
        }
    }
    let mut delta = serde_json::Map::new();
    // Id repetido em `now` com o mesmo conteúdo: só a primeira ocorrência vira `same`, a outra vai inteira.
    let mut used = std::collections::HashSet::new();
    let above = |view: &serde_json::Map<String, Value>| view.get("above").cloned().unwrap_or(Value::Null);
    if above(prev) != above(now) { delta.insert("above".into(), above(now)); }
    for (key, value) in now {
        match key.as_str() {
            "above" => {}
            "panes" => {
                let panes = value.as_array()?.iter().map(|pane| {
                    let id = pane.get("id").and_then(Value::as_str);
                    match id.and_then(|id| before.get(id).copied().flatten()) {
                        Some(old) if old == pane && used.insert(id) => serde_json::json!({"id": id, "same": true}),
                        _ => pane.clone(),
                    }
                });
                delta.insert(key.clone(), Value::Array(panes.collect()));
            }
            _ => { delta.insert(key.clone(), value.clone()); }
        }
    }
    Some(Value::Object(delta))
}
const CHANNEL: usize = 1024;
const SIDE_CONNECT: Duration = Duration::from_secs(10);
/// O Python manda `ping` a cada 10 s; três calados = conexão morta.
const SIDE_IDLE: Duration = Duration::from_secs(30);

/// Liga o `Monitor` de estado a um hub de Claude com terminal e devolve a tarefa dele.
pub type SpawnMonitor = Arc<dyn Fn(&Arc<Hub>) -> tokio::task::JoinHandle<()> + Send + Sync>;

/// Os quatro eventos que, com o `Monitor` vivo, só o Rust produz para a sessão.
const STATE_EVENTS: [&str; 4] = ["state", "preview", "ask_question", "suggest"];
/// Os seis do feed (Claude e Codex sem terminal): os quatro mais pensamento e ferramenta em voo.
const FEED_EVENTS: [&str; 6] = ["state", "preview", "ask_question", "suggest", "pensamento", "ferramenta"];

#[derive(Clone)]
pub struct SideCtx {
    pub upstream: SocketAddr,
    pub secret: String,
    pub http: HttpClient,
    pub watchers: Watchers,
    pub hubs: Hubs,
    pub infos: InfoCache,
    /// `None`: sem estado no Rust (testes do hub); o Python segue produzindo os quatro eventos.
    pub monitors: Option<SpawnMonitor>,
    /// Faixa e avisos que o Rust publica nas sessões sem terminal dele (interface dos mods).
    pub mods: crate::mods::state::Mods,
}

#[derive(Default)]
struct SideCache {
    latest: [Option<Bytes>; 8],
    /// Sobe a cada `plugin_ui` gravado; é o que o marcador `Out::Ui` leva.
    ui_version: u64,
    /// A vista da versão atual, lida, para calcular a diferença da próxima.
    ui_prev: Option<serde_json::Value>,
    /// `plugin_ui_delta` da versão anterior para a atual: o aparelho que anunciou e já tem a anterior
    /// recebe só o que mudou (`ui_delta`).
    ui_delta: Option<Bytes>,
    queue: Vec<(String, Bytes)>,
    /// Avisos de mod (`plugin_toast`) ainda vivos: (id, quando vence, dado). A conexão interna é
    /// uma só por sessão, então quem abre o chat depois não os receberia do Python; o retrato os
    /// repõe com o tempo que resta, e o app descarta pelo id o que já mostrou.
    toasts: Vec<(String, Instant, serde_json::Map<String, serde_json::Value>)>,
}

impl SideCache {
    /// `pane_question`: Claude com terminal, cujo `ask_question` sai uma vez por pergunta e nada o
    /// apaga depois. Só vale enquanto o último `state` for `awaiting_input`; senão quem chega
    /// depois abriria uma pergunta já respondida. Codex e Claude sem terminal mandam o próprio
    /// `ask_question` vazio ao fechar.
    fn record(&mut self, event: &str, data: &str, frame: &Bytes, pane_question: bool) {
        if event == "plugin_toast" {
            self.record_toast(data, Instant::now());
            return;
        }
        if let Some(i) = LATEST.iter().position(|e| *e == event) {
            self.latest[i] = Some(frame.clone());
            if i == PLUGIN_UI {
                self.ui_version += 1;
                let now = serde_json::from_str::<serde_json::Value>(data).ok();
                self.ui_delta = self.ui_prev.as_ref().zip(now.as_ref()).and_then(|(prev, now)| ui_delta(prev, now))
                    .map(|delta| sse_frame("plugin_ui_delta", &delta.to_string(), None));
                self.ui_prev = now;
            }
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

    fn record_toast(&mut self, data: &str, now: Instant) {
        let Ok(serde_json::Value::Object(toast)) = serde_json::from_str(data) else { return };
        let Some(id) = toast.get("id").and_then(|v| v.as_str()).map(str::to_owned) else { return };
        let Some(ms) = toast.get("timeoutMs").and_then(|v| v.as_f64()).filter(|ms| *ms > 0.0) else { return };
        let expires = now + Duration::from_millis(ms.min(TOAST_MAX_MS) as u64);
        self.toasts.retain(|(k, at, _)| *k != id && *at > now);
        self.toasts.push((id, expires, toast));
        if self.toasts.len() > TOASTS_KEPT {
            self.toasts.drain(..self.toasts.len() - TOASTS_KEPT);
        }
    }

    fn replay(&self) -> Vec<Bytes> {
        self.replay_at(Instant::now())
    }

    fn replay_at(&self, now: Instant) -> Vec<Bytes> {
        let toasts = self.toasts.iter().filter(|(_, at, _)| *at > now).map(|(_, at, toast)| {
            let mut toast = toast.clone();
            toast.insert("timeoutMs".into(), remaining_ms(*at, now).into());
            sse_frame("plugin_toast", &serde_json::Value::Object(toast).to_string(), None)
        });
        self.latest.iter().flatten().cloned().chain(self.queue.iter().map(|(_, f)| f.clone())).chain(toasts).collect()
    }
}

#[derive(Clone)]
struct Bound {
    binding: Binding,
    generation: u64,
    tail: Arc<FileTail>,
}

/// Eventos do dono e o provider da ligação: o feed de Claude e o de Codex publicam os mesmos seis,
/// mas cada um lê fatos diferentes, então a transferência de conversa troca o dono.
type OwnerKey = (&'static [&'static str], Provider);

pub struct Hub {
    pub name: String,
    pub tx: broadcast::Sender<Out>,
    ctx: SideCtx,
    bound: Mutex<Option<Bound>>,
    cache: Mutex<SideCache>,
    side: Mutex<Option<tokio::task::AbortHandle>>,
    /// Dono do estado (o `Monitor` de Claude com terminal ou o feed de quem não tem terminal), o leitor
    /// das respostas gravadas e os eventos que esse dono publica, com o provider para o qual nasceu.
    pub(crate) monitor: Mutex<Option<(tokio::task::AbortHandle, tokio::task::AbortHandle, OwnerKey)>>,
    /// Última resposta gravada no transcript desta ligação, normalizada (`preview::norm`).
    committed: Mutex<Option<Arc<str>>>,
    /// Acorda o `Monitor`: `rebind` (rodada já) ou resposta gravada (prévia sai já).
    wake: Arc<Notify>,
    /// Vezes que a troca vazou (o Python mandou um dos quatro para sessão do Rust) e foi registrada.
    pub(crate) python_leaks: AtomicU32,
}

/// O que um aparelho recebe ao entrar: cauda + retrato, e o canal para o resto. `ui`: a versão do
/// `plugin_ui` que já vai no retrato; marcador dela ou anterior não precisa sair de novo.
pub struct Attach {
    pub generation: u64,
    pub rx: broadcast::Receiver<Out>,
    pub frames: Vec<Bytes>,
    pub ui: u64,
    /// O retrato levou o `plugin_ui` da versão `ui`.
    pub has_ui: bool,
}

/// Item da fila de envio de um aparelho: quadro pronto, ou o marcador do `plugin_ui`, resolvido pelo
/// retrato só quando a conexão pode escrever (`Hub::resolve`).
pub enum Queued {
    Frame(Bytes),
    Ui(u64),
    /// A versão do `plugin_ui` que o retrato da entrada levou ao aparelho (`None`: nenhuma); não escreve nada.
    Has(Option<u64>),
}

impl From<Bytes> for Queued {
    fn from(frame: Bytes) -> Self {
        Queued::Frame(frame)
    }
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
                monitor: Mutex::new(None),
                committed: Mutex::new(None),
                wake: Arc::new(Notify::new()),
                python_leaks: AtomicU32::new(0),
            }
        });
        hub.ensure_monitor();
        hub.restart_side();
        hub
    }

    /// Claude com terminal ganha o `Monitor` e Claude ou Codex sem terminal o feed (um por hub); a ligação que
    /// troca de dono (`/modo-execucao`) troca a tarefa, e sem dono ela sai. Dono que acabou (sessão
    /// morta, pânico) volta quando alguém religa ou assina de novo.
    fn ensure_monitor(self: &Arc<Self>) {
        let Some(spawn) = self.ctx.monitors.clone() else { return };
        let wanted = self.bound.lock().unwrap().as_ref().and_then(|b| b.binding.state_events().map(|e| (e, b.binding.provider)));
        let mut slot = self.monitor.lock().unwrap();
        // Dono que acabou conta como ausente.
        let alive = slot.as_ref().filter(|(m, _, _)| !m.is_finished()).map(|(_, _, key)| *key);
        if wanted == alive {
            return;
        }
        if let Some((m, c, _)) = slot.take() {
            m.abort();
            c.abort();
        }
        if let Some(key) = wanted {
            let commits = tokio::spawn(watch_commits(Arc::downgrade(self), self.tx.subscribe())).abort_handle();
            *slot = Some((spawn(self).abort_handle(), commits, key));
        }
    }

    fn stop_monitor(&self) {
        if let Some((m, c, _)) = self.monitor.lock().unwrap().take() {
            m.abort();
            c.abort();
        }
    }

    /// Eventos do dono vivo; o que acabou (sessão morta, pânico) não segura mais os do Python.
    fn owned_events(&self) -> &'static [&'static str] {
        self.monitor.lock().unwrap().as_ref().filter(|(m, _, _)| !m.is_finished()).map_or(&[], |(_, _, (events, _))| *events)
    }

    #[cfg(test)]
    fn has_monitor(&self) -> bool { !self.owned_events().is_empty() }

    /// Os eventos do estado que esta ligação tem no Rust, para o canal privado.
    fn state_events(&self) -> &'static [&'static str] {
        self.bound.lock().unwrap().as_ref().and_then(|b| b.binding.state_events()).unwrap_or(&STATE_EVENTS)
    }

    /// A ligação atual: o feed escolhe por ela.
    pub fn binding(&self) -> Option<Binding> { self.bound.lock().unwrap().as_ref().map(|b| b.binding.clone()) }

    /// Geração da ligação atual (a época do `Monitor`); `None` com o hub fechado.
    pub fn generation(&self) -> Option<u64> { self.bound.lock().unwrap().as_ref().map(|b| b.generation) }

    /// Session id da conversa ligada (`/clear` troca).
    pub fn session_key(&self) -> Option<String> {
        self.bound.lock().unwrap().as_ref().map(|b| b.binding.key.clone()).filter(|k| !k.is_empty())
    }

    pub fn jsonl(&self) -> Option<PathBuf> { self.bound.lock().unwrap().as_ref().map(|b| b.binding.jsonl.clone()) }

    pub fn wake(&self) -> Arc<Notify> { self.wake.clone() }

    pub fn committed(&self) -> Option<Arc<str>> { self.committed.lock().unwrap().clone() }

    /// Resposta gravada lida na geração `generation`; troca de ligação no meio descarta.
    pub(crate) fn set_committed(&self, generation: u64, text: String, only_if_empty: bool) {
        let bound = self.bound.lock().unwrap();
        if bound.as_ref().map(|b| b.generation) != Some(generation) {
            return;
        }
        let mut committed = self.committed.lock().unwrap();
        if only_if_empty && committed.is_some() {
            return;
        }
        *committed = Some(text.into());
        drop((committed, bound));
        self.wake.notify_one();
    }

    /// Evento do `Monitor`: mesmo retrato e mesma regra de repetido do que vem do Python.
    /// `false`: hub fechado, ninguém mais ouve.
    pub fn publish_own(&self, event: &str, data: &str) -> bool {
        if self.bound.lock().unwrap().is_none() {
            return false;
        }
        self.deliver(event, data);
        true
    }

    /// Assina o canal e copia o retrato dos eventos do estado (canal privado).
    fn subscribe_state(&self) -> Option<(broadcast::Receiver<Out>, Vec<Bytes>, &'static [&'static str])> {
        self.bound.lock().unwrap().as_ref()?;
        let events = self.state_events();
        let rx = self.tx.subscribe();
        let cached = self.cache.lock().unwrap().replay().into_iter().filter(|f| private_frame(f, events)).collect();
        Some((rx, cached, events))
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
        } else {
            // Sessão que voltou com a mesma conversa (resume): o `Monitor` que viu a morte renasce.
            self.ensure_monitor();
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
        self.stop_monitor();
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
        *self.committed.lock().unwrap() = None;
        // Os avisos de mod ficam (o `cache.toasts` os guarda): são da sessão, não do transcript. A faixa
        // do Rust também é da sessão: volta ao retrato antes do `reset`, senão quem religa depois de um
        // `/clear` fica sem ela até o mod redesenhar. Limpeza e semeadura sob uma trava só.
        let ui = self.ctx.mods.replay(&self.name).into_iter().find(|(event, _)| *event == "plugin_ui");
        {
            let mut cache = self.cache.lock().unwrap();
            cache.latest = Default::default();
            // Sem a vista anterior no retrato, a próxima sai inteira.
            cache.ui_prev = None;
            cache.ui_delta = None;
            if let Some((event, data)) = ui {
                let frame = sse_frame(event, &data, None);
                cache.record(event, &data, &frame, false);
            }
        }
        let _ = self.tx.send(Out::Rebind);
        drop(bound);
        self.ensure_monitor();
        // O retrato acabou de perder o estado: o `Monitor` publica o da época nova sem esperar o tique.
        self.wake.notify_one();
    }

    /// Quadro de evento para os aparelhos: entra no retrato e sai pelo canal, salvo quando é igual
    /// ao último do mesmo tipo (o aparelho já o tem). Serve ao Python (side-events) e ao Rust (mods).
    fn deliver(&self, event: &str, data: &str) {
        let frame = sse_frame(event, data, None);
        // Retrato antes do envio: quem assina entre os dois recebe repetido, nunca nada.
        let pane_question = self.bound.lock().unwrap().as_ref().is_some_and(|b| b.binding.provider == Provider::Claude);
        let mut cache = self.cache.lock().unwrap();
        // Pergunta repetida é pergunta nova: o aparelho já fechou a anterior.
        let repeated = event != "ask_question"
            && LATEST.iter().position(|e| *e == event).is_some_and(|i| cache.latest[i].as_ref() == Some(&frame));
        // A mesma vista de novo (o Python a reenvia a cada religação; o Rust limpa duas vezes seguidas) não
        // sobe a versão: sem marcador novo, a versão nova invalidaria o marcador que a fila de um aparelho
        // lento ainda tem, e ele ficaria com a vista anterior.
        if !(repeated && event == LATEST[PLUGIN_UI]) {
            cache.record(event, data, &frame, pane_question);
        }
        // O envio fica sob a trava do retrato (o `send` do broadcast não bloqueia): com o Python e o Rust
        // escrevendo no mesmo hub, fora dela dois quadros poderiam sair em ordem diferente da do retrato.
        if !repeated {
            let _ = self.tx.send(Self::out(event, frame, &cache));
        }
    }

    /// O `plugin_ui` sai pelo canal só como marcador da versão gravada no retrato.
    fn out(event: &str, frame: Bytes, cache: &SideCache) -> Out {
        if event == LATEST[PLUGIN_UI] { Out::Ui(cache.ui_version) } else { Out::Side(frame) }
    }

    /// O quadro de um item da fila de um aparelho. O marcador do `plugin_ui` vira o quadro só se a versão
    /// dele ainda for a do retrato; vencido, some, porque o marcador da versão nova vem atrás dele na
    /// mesma fila (ou o retrato dela, depois de um `reset`). `ui` é a versão que o aparelho já tem: com
    /// `deltas` e a anterior à do retrato, sai só a diferença.
    pub fn resolve(&self, item: Queued, ui: &mut Option<u64>, deltas: bool) -> Option<Bytes> {
        match item {
            Queued::Frame(frame) => Some(frame),
            Queued::Has(version) => {
                *ui = version;
                None
            }
            Queued::Ui(version) => {
                let cache = self.cache.lock().unwrap();
                if cache.ui_version != version { return None; }
                let frame = match &cache.ui_delta {
                    Some(delta) if deltas && *ui == version.checked_sub(1) => delta.clone(),
                    _ => cache.latest[PLUGIN_UI].clone()?,
                };
                *ui = Some(version);
                Some(frame)
            }
        }
    }

    /// Semeia o hub recém-criado com o retrato que o `Mods` calculou fora das travas. A faixa só entra se
    /// a vaga dela ainda estiver vazia: um `publish_ui` ou `forget` que chegou depois do cálculo já a
    /// preencheu com o dado mais novo. Os avisos se resolvem pelo id (o app descarta o repetido).
    pub(crate) fn seed(&self, frames: Vec<(&'static str, String)>) {
        for (event, data) in frames {
            let frame = sse_frame(event, &data, None);
            let mut cache = self.cache.lock().unwrap();
            if event == LATEST[PLUGIN_UI] && cache.latest[PLUGIN_UI].is_some() {
                continue;
            }
            cache.record(event, &data, &frame, false);
            let _ = self.tx.send(Self::out(event, frame, &cache));
        }
    }

    fn close(self: &Arc<Self>) {
        self.ctx.hubs.evict(&self.name, self);
        self.bound.lock().unwrap().take();
        // Pode ser uma conexão religada por `ensure_current` no meio; a própria sai logo depois.
        if let Some(h) = self.side.lock().unwrap().take() {
            h.abort();
        }
        self.stop_monitor();
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
            let (cached, ui, has_ui) = {
                let cache = self.cache.lock().unwrap();
                (cache.replay(), cache.ui_version, cache.latest[PLUGIN_UI].is_some())
            };
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
            return Some(Attach { generation: b.generation, rx, frames, ui, has_ui });
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
                // Sessão sem terminal atendida pelo Rust: faixa e avisos vêm dele (dono único). Um
                // `plugin_ui` velho do Python (sessão que mudou de modo) não pode sobrescrevê-los.
                if matches!(event, "plugin_ui" | "plugin_toast") && hub.ctx.mods.owns(&hub.name) {
                    continue;
                }
                on_side_event(hub, event, &ev.data);
            }
        }
    }
}

/// Evento da conexão interna (fora `info` e `ping`). Com o dono do estado vivo, os eventos dele
/// (`Binding::state_events`) são só dele: vindo do Python, a troca vazou; sai do canal e
/// vai ao log uma vez por sessão. `true`: repassado aos aparelhos (ou igual ao último).
fn on_side_event(hub: &Arc<Hub>, event: &str, data: &str) -> bool {
    if hub.owned_events().contains(&event) {
        if hub.python_leaks.compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            tracing::warn!(session = %hub.name, code = "state_python_leak", event, "estado: o Python mandou evento de sessão do Rust; descartado");
        }
        return false;
    }
    hub.deliver(event, data);
    true
}

/// Quadro que o canal privado leva ao Python: o estado e a faixa dos mods, que o convidado também vê.
fn private_frame(frame: &[u8], events: &[&str]) -> bool {
    state_frame(frame, events) || state_frame(frame, &[LATEST[PLUGIN_UI]])
}

/// Quadro de um dos eventos do estado dados.
fn state_frame(frame: &[u8], events: &[&str]) -> bool {
    let Some(rest) = frame.strip_prefix(b"event: ") else { return false };
    events.iter().any(|e| rest.strip_prefix(e.as_bytes()).is_some_and(|r| r.starts_with(b"\r\n")))
}

/// Mantém a última resposta gravada da ligação: o leitor do transcript manda cada linha ao canal, e
/// a troca de ligação semeia de novo a partir do fim do arquivo.
async fn watch_commits(hub: Weak<Hub>, mut rx: broadcast::Receiver<Out>) {
    seed_committed(&hub, true).await;
    loop {
        match rx.recv().await {
            Ok(Out::Tail(generation, frame)) => {
                let Some(text) = crate::state::preview::committed_from_frame(&frame) else { continue };
                let Some(h) = hub.upgrade() else { return };
                h.set_committed(generation, text, false);
            }
            Ok(Out::Rebind) => seed_committed(&hub, true).await,
            // A resposta gravada durante o atraso pode ter se perdido: o fim do arquivo vence.
            Err(broadcast::error::RecvError::Lagged(_)) => seed_committed(&hub, false).await,
            Ok(Out::Side(_) | Out::Ui(_)) => {}
            Ok(Out::Close) | Err(broadcast::error::RecvError::Closed) => return,
        }
    }
}

/// O leitor do transcript começa no fim do arquivo: sem isto a resposta já gravada voltaria como
/// prévia logo depois de abrir o chat. Só preenche o vazio; a resposta ao vivo vence.
async fn seed_committed(hub: &Weak<Hub>, only_if_empty: bool) {
    let Some((binding, generation)) = hub.upgrade().and_then(|h| h.bound.lock().unwrap().as_ref().map(|b| (b.binding.clone(), b.generation))) else { return };
    let last = tokio::task::spawn_blocking(move || {
        let size = match std::fs::metadata(&binding.jsonl) {
            Ok(m) => m.len(),
            Err(e) => {
                if e.kind() != std::io::ErrorKind::NotFound && crate::warn_limit::allow(None, "state_committed_seed") {
                    tracing::warn!(code = "state_committed_seed", kind = ?e.kind(), "estado: transcript ilegível; a prévia pode repetir a resposta gravada");
                }
                return None;
            }
        };
        let frames = tail::backfill(&binding.jsonl, &binding.key, binding.provider, None, size);
        frames.iter().rev().find_map(|f| crate::state::preview::committed_from_frame(f))
    })
    .await;
    match last {
        Ok(Some(text)) => if let Some(h) = hub.upgrade() { h.set_committed(generation, text, only_if_empty) },
        Ok(None) => {}
        Err(e) => tracing::warn!(panic = e.is_panic(), code = "state_committed_seed", "estado: leitura da resposta gravada caiu"),
    }
}

/// `GET /__hangar_server/state/{name}/events` na porta privada: o Python lê daqui `state`,
/// `preview`, `ask_question` e `suggest` (sem terminal, Claude ou Codex, também `pensamento` e `ferramenta`),
/// mais o `plugin_ui`, de
/// quem entrou pelas portas dele (convite, Connect). Conta
/// como assinante do hub, então liga o `Monitor` igual a um aparelho do dono.
pub async fn private_events(
    axum::extract::State(st): axum::extract::State<Arc<crate::routes::AppState>>,
    axum::extract::ConnectInfo(peer): axum::extract::ConnectInfo<SocketAddr>,
    axum::extract::Path(name): axum::extract::Path<String>,
    req: axum::extract::Request,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    if !crate::workspace_routes::private_ok(&st, peer, req.headers()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    let Ok(info) = st.info(&name).await else {
        st.diag.report("rust.state_channel_failed", &name, "internal_info", "o backend não devolveu os dados da sessão");
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // Só Claude (com ou sem terminal) e Codex sem terminal têm o estado aqui; o resto o Python observa.
    let Some(binding) = info.as_ref().and_then(Binding::from_info).filter(|b| b.state_events().is_some()) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let lease = st.side.hubs.acquire(&name, binding, &st.side);
    let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(64);
    tokio::spawn(private_loop(lease, tx));
    let stream = futures_util::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|b| (Ok::<Bytes, std::convert::Infallible>(b), rx))
    });
    let mut resp = axum::response::Response::new(Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(axum::http::header::CONTENT_TYPE, axum::http::HeaderValue::from_static("text/event-stream; charset=utf-8"));
    h.insert(axum::http::header::CACHE_CONTROL, axum::http::HeaderValue::from_static("no-store"));
    resp
}

/// Ping a cada 10 s, como o `/events`: o leitor do Python dá a conexão por morta depois de 30 s calada.
const PRIVATE_PING: Duration = Duration::from_secs(10);

async fn private_loop(lease: Lease, out: tokio::sync::mpsc::Sender<Bytes>) {
    let hub = lease.hub.clone();
    let send = |f: Bytes| {
        let out = out.clone();
        async move { matches!(tokio::time::timeout(Duration::from_secs(30), out.send(f)).await, Ok(Ok(()))) }
    };
    if !send(tail::ping_frame()).await {
        return;
    }
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PRIVATE_PING, PRIVATE_PING);
    loop {
        let Some((mut rx, cached, events)) = hub.subscribe_state() else { return };
        for f in cached {
            if !send(f).await {
                return;
            }
        }
        loop {
            tokio::select! {
                _ = out.closed() => return,
                _ = ping.tick() => if !send(tail::ping_frame()).await { return },
                msg = rx.recv() => match msg {
                    Ok(Out::Side(f)) if state_frame(&f, events) => if !send(f).await { return },
                    // O `/events` do Python repassa a vista inteira ao convidado: sem diferença aqui.
                    Ok(Out::Ui(version)) => if let Some(f) = hub.resolve(Queued::Ui(version), &mut None, false) {
                        if !send(f).await { return }
                    },
                    Ok(Out::Side(_) | Out::Tail(..) | Out::Rebind) => {}
                    Ok(Out::Close) | Err(broadcast::error::RecvError::Closed) => return,
                    // Atrasado: o retrato de agora repõe o que se perdeu.
                    Err(broadcast::error::RecvError::Lagged(_)) => break,
                },
            }
        }
    }
}

/// O hub de cada sessão, com a contagem de aparelhos.
type HubMap = Mutex<HashMap<String, (Arc<Hub>, usize)>>;

/// Hubs vivos por nome de sessão, com a contagem de aparelhos.
#[derive(Clone, Default)]
pub struct Hubs(Arc<HubMap>);

/// O mapa de hubs sem segurá-lo vivo: o `Mods` entrega por aqui sem formar ciclo com o `SideCtx`.
#[derive(Clone)]
pub struct WeakHubs(Weak<HubMap>);

impl WeakHubs {
    pub fn upgrade(&self) -> Option<Hubs> {
        self.0.upgrade().map(Hubs)
    }
}

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
                drop(map);
                // Hub novo começa sem retrato: a faixa e os avisos que o Rust já publicou entram agora.
                hub.seed(ctx.mods.replay(name));
                return Lease { hubs: self.clone(), hub };
            }
        };
        drop(map);
        hub.ensure_current(&binding);
        Lease { hubs: self.clone(), hub }
    }

    pub fn downgrade(&self) -> WeakHubs {
        WeakHubs(Arc::downgrade(&self.0))
    }

    /// Evento do próprio Rust (interface dos mods) para o hub da sessão. Sem aparelho não há hub, e o
    /// `Mods` guarda o valor para semear o hub que nascer depois.
    pub fn deliver(&self, name: &str, event: &str, data: &str) {
        let hub = self.0.lock().unwrap().get(name).map(|(hub, _)| hub.clone());
        if let Some(hub) = hub {
            hub.deliver(event, data);
        }
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

/// Contexto de teste: porta sem ninguém (a conexão interna só tenta e espera) e sem dono do estado.
#[cfg(test)]
pub(crate) fn test_ctx() -> SideCtx {
    SideCtx {
        upstream: "127.0.0.1:9".parse().unwrap(),
        secret: "s".into(),
        http: crate::proxy::client(),
        watchers: Watchers::default(),
        hubs: Hubs::default(),
        infos: InfoCache::default(),
        monitors: None,
        mods: Default::default(),
    }
}

#[cfg(test)]
pub(crate) fn close_hub(hub: &Arc<Hub>) { hub.close(); }

/// Pane parado do Claude, usado pelos `Monitor`s de mentira dos testes.
#[cfg(test)]
pub(crate) const IDLE_PANE: &str = "────────────\n❯\n────────────";

/// `Monitor` com fonte de mentira ligado ao hub de verdade: quadro fixo, sem Python nem tmux.
/// Devolve a fábrica e quantos `Monitor`s ela criou.
#[cfg(test)]
pub(crate) fn fake_monitors(pane: &'static str) -> (SpawnMonitor, Arc<std::sync::atomic::AtomicU32>) {
    use crate::state::monitor::Monitor;
    use crate::state::testing::HubFake;
    let count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let spawned = count.clone();
    let spawn: SpawnMonitor = Arc::new(move |hub: &Arc<Hub>| {
        spawned.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let src = HubFake::new(hub, pane);
        tokio::spawn(async move { let _ = Monitor::new(src).run().await; })
    });
    (spawn, count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keeps_last_value_and_queue_by_id_without_nav() {
        assert_eq!(LATEST[PLUGIN_UI], "plugin_ui");
        let mut c = SideCache::default();
        let rec = |c: &mut SideCache, e: &str, d: &str| c.record(e, d, &sse_frame(e, d, None), true);
        rec(&mut c, "state", "{\"state\":\"working\"}");
        rec(&mut c, "state", "{\"state\":\"idle\"}");
        rec(&mut c, "message", "{\"id\":\"queued-1\"}");
        rec(&mut c, "queue_confirmed", "{\"id\":\"queued-1\",\"queued_confirmed\":true}");
        rec(&mut c, "nav", "{\"url\":\"http://x\"}");
        rec(&mut c, "plugin_ui", "{\"band\":null}");
        rec(&mut c, "plugin_ui", "{\"band\":{\"type\":\"Box\"}}");
        let r: Vec<String> = c.replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
        assert_eq!(r.len(), 3);
        assert!(r[0].starts_with("event: state") && r[0].contains("idle"));
        assert!(r[1].starts_with("event: plugin_ui") && r[1].contains("Box"));
        assert!(r[2].starts_with("event: queue_confirmed"));
    }

    #[test]
    fn mod_toast_replays_with_the_time_left_until_it_expires() {
        // Paridade com `plugin_bridge.toasts_after`: quem chega depois recebe o aviso vivo com o
        // tempo que resta; vencido, sai. O mesmo id reenviado (religação interna) não duplica.
        let mut c = SideCache::default();
        let toast = |id: &str, ms: u64| format!("{{\"id\":\"{id}\",\"text\":\"Jenkins configurado.\",\"plugin\":\"demo\",\"timeoutMs\":{ms}}}");
        let data = toast("b-1", 9000);
        c.record("plugin_toast", &data, &sse_frame("plugin_toast", &data, None), true);
        let t0 = c.toasts[0].1 - Duration::from_millis(9000);
        c.record_toast(&toast("b-1", 9000), t0);
        c.record_toast("{\"id\":\"b-2\",\"text\":\"x\"}", t0);
        let at = |c: &SideCache, s: u64| -> Vec<serde_json::Value> {
            c.replay_at(t0 + Duration::from_secs(s))
                .iter()
                .map(|b| String::from_utf8_lossy(b).into_owned())
                .filter(|f| f.starts_with("event: plugin_toast"))
                .map(|f| serde_json::from_str(f.lines().nth(1).unwrap().strip_prefix("data: ").unwrap()).unwrap())
                .collect()
        };
        let live = at(&c, 2);
        assert_eq!(live.len(), 1, "sem prazo não é aviso; o mesmo id fica um só");
        assert_eq!((live[0]["text"].as_str(), live[0]["plugin"].as_str(), live[0]["timeoutMs"].as_u64()),
                   (Some("Jenkins configurado."), Some("demo"), Some(7000)));
        assert!(at(&c, 10).is_empty(), "vencido não chega a quem abre depois");
        for i in 0..25 {
            c.record_toast(&toast(&format!("n-{i}"), 9000), t0);
        }
        let kept = at(&c, 1);
        assert_eq!(kept.len(), TOASTS_KEPT);
        assert_eq!(kept[0]["id"], "n-5", "o mais antigo sai primeiro");
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

    fn idle_ctx() -> SideCtx { test_ctx() }

    /// Quadros `event: <nome>` que chegam ao canal, até `limit` ou o prazo.
    async fn side_events(rx: &mut broadcast::Receiver<Out>, limit: Duration) -> Vec<(String, String)> {
        let mut out = Vec::new();
        let _ = tokio::time::timeout(limit, async {
            loop {
                match rx.recv().await {
                    Ok(Out::Side(f)) => {
                        let f = String::from_utf8_lossy(&f).into_owned();
                        let event = f.lines().next().unwrap_or("").trim_start_matches("event: ").to_owned();
                        let data = f.lines().nth(1).unwrap_or("").trim_start_matches("data: ").to_owned();
                        out.push((event, data));
                    }
                    Ok(Out::Rebind) => out.push(("rebind".into(), String::new())),
                    Ok(_) => {}
                    Err(_) => return,
                }
            }
        })
        .await;
        out
    }

    #[tokio::test]
    async fn monitor_publishes_after_rebind() {
        let dir = tempfile::tempdir().unwrap();
        let binding = |n: &str| Binding { provider: Provider::Claude, jsonl: dir.path().join(format!("{n}.jsonl")), key: n.into(), headless: false };
        let (spawn, count) = fake_monitors(IDLE_PANE);
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let lease = ctx.hubs.acquire("s", binding("a"), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        let first = side_events(&mut rx, Duration::from_millis(300)).await;
        assert_eq!(first.iter().filter(|(e, _)| e == "state").count(), 1, "{first:?}");
        // `/clear`: o retrato some no `rebind` e o estado novo sai logo, sem esperar o tique (0,75 s).
        lease.hub.rebind(binding("b"));
        let after = side_events(&mut rx, Duration::from_millis(300)).await;
        assert_eq!(after.first().map(|(e, _)| e.as_str()), Some("rebind"), "{after:?}");
        assert!(after.iter().any(|(e, _)| e == "state"), "estado novo logo depois do rebind: {after:?}");
        assert!(lease.hub.cache.lock().unwrap().latest[0].is_some(), "o retrato volta a ter o estado");
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1, "o rebind não cria outro Monitor");
    }

    #[tokio::test]
    async fn python_state_for_rust_session_dropped_once() {
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false };
        let (spawn, _) = fake_monitors(IDLE_PANE);
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        // Deixa o Monitor publicar o dele antes.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let mut rx = lease.hub.tx.subscribe();
        for (event, data) in [("state", r#"{"state":"working"}"#), ("preview", r#"{"text":"x"}"#),
                              ("ask_question", "{}"), ("suggest", r#"{"text":"y"}"#), ("state", r#"{"state":"idle"}"#)] {
            assert!(!on_side_event(&lease.hub, event, data), "{event} do Python para sessão do Rust sai");
        }
        assert!(on_side_event(&lease.hub, "stats", r#"{"turns":1}"#), "o resto continua do Python");
        assert!(on_side_event(&lease.hub, "message", r#"{"id":"queued-1"}"#));
        assert_eq!(lease.hub.python_leaks.load(std::sync::atomic::Ordering::SeqCst), 1, "registra uma vez por sessão");
        let got = side_events(&mut rx, Duration::from_millis(100)).await;
        let names: Vec<_> = got.iter().map(|(e, _)| e.as_str()).collect();
        assert_eq!(names, ["stats", "message"]);
        let state = String::from_utf8_lossy(lease.hub.cache.lock().unwrap().latest[0].as_ref().unwrap()).into_owned();
        assert!(!state.contains("working"), "o retrato fica com o estado do Rust");

        // Codex com terminal: o Python segue dono.
        let codex = Binding { provider: Provider::Codex, jsonl: dir.path().join("c.jsonl"), key: "c".into(), headless: false };
        let other = ctx.hubs.acquire("c", codex, &ctx);
        assert!(on_side_event(&other.hub, "state", r#"{"state":"idle"}"#));
        assert!(on_side_event(&other.hub, "pensamento", r#"{"text":""}"#));
        assert_eq!(other.hub.python_leaks.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn python_state_for_codex_headless_dropped_once() {
        // Codex sem terminal: o feed é dono dos seis eventos; os do Python caem com um aviso só.
        let dir = tempfile::tempdir().unwrap();
        let (spawn, _) = fake_feeds();
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let codex = Binding { provider: Provider::Codex, jsonl: dir.path().join("c.jsonl"), key: "c".into(), headless: true };
        let lease = ctx.hubs.acquire("c", codex, &ctx);
        for event in ["state", "preview", "ask_question", "suggest", "pensamento", "ferramenta"] {
            assert!(!on_side_event(&lease.hub, event, r#"{"text":""}"#), "{event} do Python para Codex sem terminal sai");
        }
        assert!(on_side_event(&lease.hub, "stats", "{}"), "o resto continua do Python");
        assert_eq!(lease.hub.python_leaks.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn python_state_for_claude_headless_dropped_once() {
        // Claude sem terminal: o feed é dono dos seis eventos, a sugestão incluída.
        let dir = tempfile::tempdir().unwrap();
        let (spawn, count) = fake_feeds();
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let binding = Binding { provider: Provider::ClaudeHeadless, jsonl: dir.path().join("h.jsonl"), key: "h".into(), headless: false };
        let lease = ctx.hubs.acquire("h", binding, &ctx);
        assert_eq!(count.load(Ordering::SeqCst), 1, "o hub liga o feed");
        for event in ["state", "preview", "ask_question", "suggest", "pensamento", "ferramenta"] {
            assert!(!on_side_event(&lease.hub, event, r#"{"text":""}"#), "{event} do Python para Claude sem terminal sai");
        }
        assert!(on_side_event(&lease.hub, "stats", "{}"));
        assert_eq!(lease.hub.python_leaks.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn claude_mode_flip_swaps_monitor_and_feed_without_gap() {
        // `/modo-execucao` no Claude: o provider troca no `info`, o hub religa e o dono troca na hora.
        let dir = tempfile::tempdir().unwrap();
        let (spawn, count) = fake_feeds();
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let binding = |provider| Binding { provider, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false };
        let lease = ctx.hubs.acquire("s", binding(Provider::Claude), &ctx);
        assert_eq!(lease.hub.owned_events(), &STATE_EVENTS[..], "com terminal: o Monitor");
        let _again = ctx.hubs.acquire("s", binding(Provider::ClaudeHeadless), &ctx);
        assert_eq!(lease.hub.owned_events(), &FEED_EVENTS[..], "sem terminal: o feed, já no rebind");
        assert_eq!(count.load(Ordering::SeqCst), 2);
        assert!(!on_side_event(&lease.hub, "state", r#"{"state":"idle"}"#), "nunca os dois donos");
        assert!(!on_side_event(&lease.hub, "suggest", r#"{"text":""}"#), "a sugestão também é do feed");
        lease.hub.rebind(binding(Provider::Claude));
        assert_eq!(lease.hub.owned_events(), &STATE_EVENTS[..], "de volta ao terminal: o Monitor");
        assert_eq!(count.load(Ordering::SeqCst), 3);
        assert!(!on_side_event(&lease.hub, "suggest", r#"{"text":"x"}"#));
    }

    #[tokio::test]
    async fn conversation_transfer_between_providers_swaps_feed() {
        // Transferência de conversa: mesmo nome, mesmos seis eventos, outro provider; o feed troca.
        let dir = tempfile::tempdir().unwrap();
        let (spawn, count) = fake_feeds();
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let claude = Binding { provider: Provider::ClaudeHeadless, jsonl: dir.path().join("h.jsonl"), key: "h".into(), headless: false };
        let codex = Binding { provider: Provider::Codex, headless: true, ..claude.clone() };
        let lease = ctx.hubs.acquire("s", claude.clone(), &ctx);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        lease.hub.rebind(codex.clone());
        assert_eq!(count.load(Ordering::SeqCst), 2, "Claude → Codex: feed novo");
        lease.hub.rebind(codex);
        assert_eq!(count.load(Ordering::SeqCst), 2, "mesma ligação: o feed fica");
        lease.hub.rebind(claude);
        assert_eq!(count.load(Ordering::SeqCst), 3, "Codex → Claude: feed novo");
        assert_eq!(lease.hub.owned_events(), &FEED_EVENTS[..]);
    }

    /// Fábrica que conta e liga um feed parado (só ocupa a vaga do dono do estado).
    fn fake_feeds() -> (SpawnMonitor, Arc<AtomicU32>) {
        let count = Arc::new(AtomicU32::new(0));
        let spawned = count.clone();
        let spawn: SpawnMonitor = Arc::new(move |_hub: &Arc<Hub>| {
            spawned.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(std::future::pending())
        });
        (spawn, count)
    }

    #[tokio::test]
    async fn headless_flip_rebinds_and_swaps_owner() {
        // `/modo-execucao`: o `info` com `headless` trocado religa o hub, que liga ou desliga o feed.
        let dir = tempfile::tempdir().unwrap();
        let (spawn, count) = fake_feeds();
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let binding = |headless| Binding { provider: Provider::Codex, jsonl: dir.path().join("c.jsonl"), key: "c".into(), headless };
        let lease = ctx.hubs.acquire("c", binding(false), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        assert!(!lease.hub.has_monitor(), "com terminal: sem dono do estado no Rust");
        let _again = ctx.hubs.acquire("c", binding(true), &ctx);
        assert_eq!(side_events(&mut rx, Duration::from_millis(50)).await.first().map(|(e, _)| e.as_str()), Some("rebind"));
        assert!(lease.hub.has_monitor(), "sem terminal: o feed liga");
        assert_eq!(count.load(Ordering::SeqCst), 1);
        lease.hub.rebind(binding(false));
        assert!(!lease.hub.has_monitor(), "de volta ao terminal: o feed sai");
        assert!(on_side_event(&lease.hub, "state", r#"{"state":"idle"}"#), "e o Python volta a ser dono");
    }

    #[tokio::test]
    async fn feed_panic_reports_and_shows_problem() {
        let dir = tempfile::tempdir().unwrap();
        let reported = Arc::new(Mutex::new(Vec::<String>::new()));
        let count = Arc::new(AtomicU32::new(0));
        let (seen, spawned) = (reported.clone(), count.clone());
        let spawn: SpawnMonitor = Arc::new(move |hub: &Arc<Hub>| {
            spawned.fetch_add(1, Ordering::SeqCst);
            let seen = seen.clone();
            tokio::spawn(crate::state::runtime_feed::guarded(Arc::downgrade(hub), async { panic!("feed caiu") },
                move |name| seen.lock().unwrap().push(format!("rust.state_feed_failed:{name}"))))
        });
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let binding = Binding { provider: Provider::Codex, jsonl: dir.path().join("c.jsonl"), key: "c".into(), headless: true };
        let lease = ctx.hubs.acquire("c", binding.clone(), &ctx);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(*reported.lock().unwrap(), ["rust.state_feed_failed:c"]);
        let state = String::from_utf8_lossy(lease.hub.cache.lock().unwrap().latest[0].as_ref().expect("estado com o problema")).into_owned();
        assert!(state.contains("state_feed_failed"), "{state}");
        let _again = ctx.hubs.acquire("c", binding, &ctx);
        assert_eq!(count.load(Ordering::SeqCst), 2, "volta com o próximo assinante");
    }

    #[test]
    fn private_channel_serves_headless_six_events() {
        let six = ["state", "preview", "ask_question", "suggest", "pensamento", "ferramenta"];
        let codex = Binding { provider: Provider::Codex, jsonl: "c".into(), key: "c".into(), headless: true };
        let claude = Binding { headless: false, provider: Provider::Claude, ..codex.clone() };
        let terminal = Binding { headless: false, ..codex.clone() };
        assert_eq!(codex.state_events(), Some(&six[..]));
        assert_eq!(claude.state_events(), Some(&six[..4]), "Claude continua com os quatro");
        assert_eq!(terminal.state_events(), None, "Codex com terminal o Python observa");
        let headless = Binding { provider: Provider::ClaudeHeadless, ..claude.clone() };
        assert_eq!(headless.state_events(), Some(&six[..]), "Claude sem terminal: o feed, com a sugestão");
        assert_eq!(Binding { headless: true, ..headless.clone() }.state_events(), Some(&six[..]));
        assert!(codex.runtime_feed() && headless.runtime_feed());
        assert!(!claude.runtime_feed() && !terminal.runtime_feed());
        for (event, pass) in six.iter().map(|e| (*e, true)).chain([("stats", false), ("message", false)]) {
            assert_eq!(state_frame(&sse_frame(event, "{}", None), &six), pass, "{event}");
        }
        assert!(!state_frame(&sse_frame("pensamento", "{}", None), &six[..4]));
    }

    #[tokio::test]
    async fn private_channel_counts_as_subscriber() {
        use axum::routing::get;
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("k.jsonl");
        std::fs::write(&jsonl, "").unwrap();
        let info = serde_json::json!({"provider": "claude", "jsonl": jsonl, "session_key": "k", "history": {}}).to_string();
        let python = axum::Router::new().route("/internal/sessions/{name}/info", get(move || {
            let info = info.clone();
            async move { ([(axum::http::header::CONTENT_TYPE, "application/json")], info) }
        })).route("/internal/sessions/{name}/side-events", get(|| async {
            // Um 404 aqui encerra o hub; a sessão da fixture precisa continuar viva.
            ([(axum::http::header::CONTENT_TYPE, "text/event-stream")],
                Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::convert::Infallible>>()))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, python).await.unwrap() });
        let side_response = reqwest::Client::new()
            .get(format!("http://{upstream}/internal/sessions/s1/side-events?app=1"))
            .send().await.unwrap();
        assert_eq!(side_response.status(), StatusCode::OK, "A fixture precisa manter a sessão viva também no canal interno");
        let cfg = crate::config::Config { listen: "127.0.0.1:0".parse().unwrap(), upstream, internal_secret: "s".into(),
            auth_token: "dono".into(), log_path: None, trusted: crate::auth::TrustedHosts::parse("127.0.0.1") };
        let mut st = crate::routes::AppState::new(cfg);
        let (spawn, count) = fake_monitors(IDLE_PANE);
        st.side.monitors = Some(spawn);
        let st = Arc::new(st);
        let private = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = private.local_addr().unwrap();
        let app = crate::routes::terminal_router(st.clone()).into_make_service_with_connect_info::<SocketAddr>();
        tokio::spawn(async move { axum::serve(private, app).await.unwrap() });

        let get_events = |secret: &'static str| async move {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            let req = format!("GET /__hangar_server/state/s1/events HTTP/1.0\r\nx-hangar-internal: {secret}\r\n\r\n");
            tokio::io::AsyncWriteExt::write_all(&mut s, req.as_bytes()).await.unwrap();
            s
        };
        let mut refused = get_events("errado").await;
        let mut buf = vec![0u8; 256];
        let n = tokio::io::AsyncReadExt::read(&mut refused, &mut buf).await.unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).starts_with("HTTP/1.0 404"), "segredo errado é 404 mudo");

        let mut s = get_events("s").await;
        let mut got = Vec::new();
        let _ = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                let n = tokio::io::AsyncReadExt::read(&mut s, &mut buf).await.unwrap();
                if n == 0 { return }
                got.extend_from_slice(&buf[..n]);
                if String::from_utf8_lossy(&got).contains("event: state") { return }
            }
        }).await;
        let text = String::from_utf8_lossy(&got).into_owned();
        assert!(text.starts_with("HTTP/1.0 200") && text.contains("event: state"), "{text}");
        assert!(!text.contains("transfer-encoding: chunked"), "HTTP/1.0: corpo até o fim, sem chunk");
        {
            let hubs = st.side.hubs.0.lock().unwrap();
            let (hub, n) = hubs.get("s1").expect("o canal abre o hub");
            assert_eq!(*n, 1, "o canal é um assinante");
            assert!(hub.monitor.lock().unwrap().is_some(), "e liga o Monitor");
        }
        assert_eq!(count.load(std::sync::atomic::Ordering::SeqCst), 1);
        // A faixa dos mods sai pelo marcador da versão e chega ao Python como quadro: o convidado a vê.
        st.side.hubs.deliver("s1", "plugin_ui", "{\"band\":1}");
        got.clear();
        let _ = tokio::time::timeout(Duration::from_millis(500), async {
            loop {
                let n = tokio::io::AsyncReadExt::read(&mut s, &mut buf).await.unwrap();
                if n == 0 { return }
                got.extend_from_slice(&buf[..n]);
                if String::from_utf8_lossy(&got).contains("event: plugin_ui") { return }
            }
        }).await;
        assert!(String::from_utf8_lossy(&got).contains("event: plugin_ui\r\ndata: {\"band\":1}"), "{}", String::from_utf8_lossy(&got));
        // Só os quatro eventos do estado passam: o Python segue dono do resto para quem entra por ele.
        on_side_event(&st.side.hubs.0.lock().unwrap().get("s1").unwrap().0.clone(), "stats", "{}");
        drop(s);
        let gone = tokio::time::timeout(Duration::from_secs(2), async {
            while st.side.hubs.0.lock().unwrap().contains_key("s1") { tokio::time::sleep(Duration::from_millis(20)).await }
        }).await;
        assert!(gone.is_ok(), "fechou o canal, o último assinante saiu e o hub para");
    }

    #[tokio::test]
    async fn own_pane_question_not_replayed_after_answer() {
        // A pergunta do `Monitor` sai uma vez por pergunta: quem chega depois da resposta não a recebe.
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false };
        let ctx = idle_ctx();
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        let late = |hub: &Hub| hub.subscribe_state().unwrap().1.iter().any(|f| f.starts_with(b"event: ask_question"));
        assert!(lease.hub.publish_own("state", r#"{"state":"awaiting_input"}"#));
        assert!(lease.hub.publish_own("ask_question", r#"{"questions":[]}"#));
        assert!(late(&lease.hub), "chegou durante a pergunta: recebe");
        assert!(lease.hub.publish_own("state", r#"{"state":"idle"}"#));
        assert!(!late(&lease.hub), "respondida: quem chega depois não recebe");
        lease.hub.close();
        assert!(!lease.hub.publish_own("state", "{}"), "hub fechado: o Monitor acaba");
    }

    #[tokio::test]
    async fn finished_monitor_releases_python_events_and_returns_with_next_subscriber() {
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false };
        let count = Arc::new(AtomicU32::new(0));
        let spawned = count.clone();
        // `Monitor` que acaba na hora, como depois de ver a sessão morta.
        let spawn: SpawnMonitor = Arc::new(move |_hub: &Arc<Hub>| {
            spawned.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async {})
        });
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let lease = ctx.hubs.acquire("s", binding.clone(), &ctx);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!lease.hub.has_monitor(), "acabou: não é mais dono");
        assert!(on_side_event(&lease.hub, "state", r#"{"state":"idle"}"#), "e não descarta o que chegar");
        let _again = ctx.hubs.acquire("s", binding, &ctx);
        assert_eq!(count.load(Ordering::SeqCst), 2, "a sessão que voltou com a mesma conversa ganha outro");
    }

    #[test]
    fn private_channel_forwards_only_state_events_and_the_band() {
        for (event, pass) in [("state", true), ("preview", true), ("ask_question", true), ("suggest", true), ("plugin_ui", true),
                              ("stats", false), ("message", false), ("plugin_toast", false), ("nav", false)] {
            assert_eq!(private_frame(&sse_frame(event, "{}", None), &STATE_EVENTS), pass, "{event}");
        }
    }

    #[tokio::test]
    async fn seed_does_not_overwrite_a_newer_band() {
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("t.jsonl"), key: "k".into(), headless: false };
        let ctx = idle_ctx();
        ctx.mods.bind_hubs(ctx.hubs.downgrade());
        struct Quiet;
        impl crate::mods::state::SurfaceLink for Quiet {
            fn call(&self, _: crate::mods::model::ModsCall, _: Instant) -> crate::mods::state::CallFuture {
                Box::pin(async { Ok(json!(null)) })
            }
        }
        ctx.mods.attach("s", 1, Arc::new(Quiet));
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        let ui = |text: &str| json!({"above": {"type": "Text", "children": [text]}, "panes": []});
        ctx.mods.publish_ui("s", 1, ui("velha"));
        let stale = ctx.mods.replay("s");
        ctx.mods.publish_ui("s", 1, ui("nova"));
        lease.hub.seed(stale);
        let kept = lease.hub.cache.lock().unwrap().latest[PLUGIN_UI].clone().unwrap();
        assert!(String::from_utf8_lossy(&kept).contains("nova"), "a faixa nova fica");
    }

    #[test]
    fn a_repeated_pane_id_is_same_only_once() {
        use serde_json::json;
        let pane = json!({"id": "a", "tree": {"type": "Text"}});
        let prev = json!({"above": null, "panes": [pane.clone()]});
        let now = json!({"above": null, "panes": [pane.clone(), pane.clone()]});
        assert_eq!(ui_delta(&prev, &now), Some(json!({"panes": [{"id": "a", "same": true}, pane]})));
    }

    #[tokio::test]
    async fn dead_reader_resets_devices_and_the_next_attach_gets_a_fresh_hub() {
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("t.jsonl"), key: tail::PANIC_KEY.into(), headless: false };
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
        let binding = |n: &str| Binding { provider: Provider::Claude, jsonl: dir.path().join(n), key: n.into(), headless: false };
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
