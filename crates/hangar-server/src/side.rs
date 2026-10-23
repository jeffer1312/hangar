// crates/hangar-server/src/side.rs
//! Uma conexão interna por sessão (Python → hangar-server) e o hub que reparte, entre os
//! aparelhos daquele chat, o que vem dela e o que o leitor do transcript produz.
use std::collections::{HashMap, VecDeque};
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
    /// Quadro da conexão interna ou de uma fonte do Rust (estado, mods, fila). A fila e o estado
    /// conferem a geração ao publicar.
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
    /// Vida da sessão (`lifecycle_id`): muda quando outra sessão nasce com o mesmo nome.
    pub life: Option<String>,
}

impl Binding {
    /// Mesmo transcript, provider e modo; a vida não entra (ela pode faltar numa rodada).
    pub fn same_conversation(&self, other: &Binding) -> bool {
        (self.provider, &self.jsonl, &self.key, self.headless) == (other.provider, &other.jsonl, &other.key, other.headless)
    }

    /// Troca que vale na hora: provider ou modo (a sessão terminando de se identificar) ou outra
    /// sessão com o mesmo nome. A troca de transcript dentro da mesma sessão espera confirmação.
    pub fn changes_at_once(&self, other: &Binding) -> bool {
        let other_life = matches!((&self.life, &other.life), (Some(a), Some(b)) if a != b);
        self.provider != other.provider || self.headless != other.headless || other_life
    }

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
            life: info.lifecycle_id.clone(),
        })
    }
}

/// Intervalo entre as leituras da descoberta que vigiam a troca de transcript.
const WATCH_EVERY: Duration = Duration::from_secs(2);

/// O que a leitura da descoberta pede ao hub.
#[derive(Debug, PartialEq)]
enum Verdict {
    Keep,
    /// Leitura diferente da ligação, ainda sem confirmação.
    Wait,
    Rebind(Binding),
    /// Sessão sumida ou provider fora do Rust.
    Close,
}

/// Troca de transcript (ou sessão sumida) só vale lida duas vezes seguidas: a descoberta pode
/// apontar outro arquivo da pasta por uma rodada, e cada troca apaga a conversa na tela de todos.
/// `Binding::changes_at_once` vale na hora.
fn judge(current: &Binding, seen: Option<Option<Binding>>, pending: &mut Option<Option<Option<Binding>>>) -> Verdict {
    let immediate = match &seen {
        Some(Some(b)) if b.same_conversation(current) => {
            *pending = None;
            return Verdict::Keep;
        }
        Some(Some(b)) => current.changes_at_once(b),
        // Provider que o Rust não lê.
        Some(None) => true,
        None => false,
    };
    let repeated = pending.as_ref().is_some_and(|p| match (p, &seen) {
        (Some(Some(a)), Some(Some(b))) => a.same_conversation(b),
        (a, b) => a == b,
    });
    if !immediate && !repeated {
        *pending = Some(seen);
        return Verdict::Wait;
    }
    *pending = None;
    match seen {
        Some(Some(b)) => Verdict::Rebind(b),
        _ => Verdict::Close,
    }
}

fn file_name(p: &std::path::Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Eventos cujo último valor vale para quem chega depois: o Python só os manda na mudança.
/// `nav` fica de fora: repetir um pedido já atendido reabriria o navegador; quem chega depois o
/// recebe pela lista de sessões. `plugin_ui` (faixa dos mods) sai quando muda: sem o cache, quem
/// abre o chat depois ficava sem a faixa até o mod redesenhar. `queue` (painel da fila) sai quando muda e vale
/// para quem chega depois.
const LATEST: [&str; 9] = ["state", "suggest", "ask_question", "stats", "preview", "pensamento", "ferramenta", "plugin_ui", crate::queue_panel::EVENT];
const ASK_QUESTION: usize = 2;
const PLUGIN_UI: usize = 7;
/// Mesmos tetos do Python (`plugin_bridge.TOASTS_KEPT`, `TOAST_MAX_MS`).
pub(crate) const TOASTS_KEPT: usize = 20;
/// Bolhas da fila que o retrato guarda para quem abre o chat depois; acima disso sai a confirmada mais velha.
const QUEUE_KEPT: usize = 32;
/// Ids apagados lembrados para recusar um quadro atrasado deles.
// Um único corte pode retirar toda a fila; seus quadros antigos ainda podem chegar do produtor.
const RETIRED_KEPT: usize = crate::runtime::queue::CAP;
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
    /// Qual conversa é a de cada sessão (a descoberta do Rust); o hub a relê para seguir trocas.
    pub locate: Arc<dyn crate::locate::Locate>,
    /// `None`: sem estado no Rust (testes do hub); o Python segue produzindo os quatro eventos.
    pub monitors: Option<SpawnMonitor>,
    /// Faixa e avisos que o Rust publica nas sessões sem terminal dele (interface dos mods).
    pub mods: crate::mods::state::Mods,
    /// Registro do runtime, quando ligado: diz se o ator do Codex com terminal é do Rust (dono do estado).
    pub runtime: Arc<std::sync::OnceLock<Arc<crate::runtime::gateway::RuntimeRegistry>>>,
    /// Pasta das projeções da fila (`<conta Claude>/.hangar-queue`); `None`: hub sem fila (testes).
    pub queue_dir: Option<PathBuf>,
    /// Quanto um problema de estado espera antes de sair (`problem_gate::HOLD`; zero nos testes dos produtores).
    pub problem_hold: std::time::Duration,
    /// O problema que a lista mostra de cada sessão (`ListBridge::gate_problems`): o hub nasce com ele.
    pub shown_problems: Arc<crate::problem_gate::Shown>,
    /// Diário do dono: o aviso de diagnóstico que o portão tira da tela vai para ele.
    pub diag: crate::diag::DiagClient,
}

impl SideCtx {
    /// A conversa da sessão: a do runtime daqui quando ele abriu o Claude (ele já confirmou a troca e
    /// religou o hub), senão a descoberta. Sem isso a vigia desfaria o `/clear` que a descoberta ainda não viu.
    pub(crate) async fn info(&self, name: &str) -> Result<Option<InternalInfo>, crate::locate::InfoFailed> {
        if let Some(runtime) = self.runtime.get() && let Some(info) = runtime.claude_info(name).await { return Ok(Some(info)); }
        self.locate.info(name).await
    }
}

#[derive(Default)]
struct SideCache {
    latest: [Option<Bytes>; 9],
    /// Sobe a cada `plugin_ui` gravado; é o que o marcador `Out::Ui` leva.
    ui_version: u64,
    /// A vista da versão atual, lida, para calcular a diferença da próxima.
    ui_prev: Option<serde_json::Value>,
    /// `plugin_ui_delta` da versão anterior para a atual: o aparelho que anunciou e já tem a anterior
    /// recebe só o que mudou (`ui_delta`).
    ui_delta: Option<Bytes>,
    queue: Vec<QueueSlot>,
    /// Bolhas (`queued-<id>`) que o painel da fila deu como apagadas: saem do retrato e não voltam por um
    /// quadro que o produtor leu antes da exclusão. O id da fila não se repete, então guardar os últimos basta.
    retired: VecDeque<String>,
    /// Avisos de mod (`plugin_toast`) ainda vivos: (id, quando vence, dado). A conexão interna é
    /// uma só por sessão, então quem abre o chat depois não os receberia do Python; o retrato os
    /// repõe com o tempo que resta, e o app descarta pelo id o que já mostrou.
    toasts: Vec<(String, Instant, serde_json::Map<String, serde_json::Value>)>,
}

/// Uma bolha da fila no retrato. `confirmed`: confirmada ou nota local, a única que o teto descarta.
struct QueueSlot {
    id: String,
    frame: Bytes,
    confirmed: bool,
    local_ts: Option<f64>,
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
            if event == crate::queue_panel::EVENT {
                self.retire_removed(data);
            }
            if event == "state" && pane_question {
                let awaiting = serde_json::from_str::<serde_json::Value>(data)
                    .ok()
                    .is_some_and(|v| v.get("state").and_then(|s| s.as_str()) == Some("awaiting_input"));
                if !awaiting {
                    self.latest[ASK_QUESTION] = None;
                }
            }
        }
    }

    /// Bolha da fila, guardada pelo id: a mais nova do mesmo id substitui a anterior no lugar dela.
    /// `false`: bolha que o painel já deu como apagada, que não vai ao retrato nem aos aparelhos.
    fn record_queue(&mut self, item: &crate::queue_feed::QueueFrame, frame: &Bytes) -> bool {
        if self.retired.contains(&item.id) {
            return false;
        }
        let slot = QueueSlot { id: item.id.clone(), frame: frame.clone(), confirmed: item.event == "queue_confirmed" || item.local_ts.is_some(), local_ts: item.local_ts };
        match self.queue.iter_mut().find(|s| s.id == slot.id) {
            Some(old) => *old = slot,
            None => self.queue.push(slot),
        }
        // Num chat aberto por horas, cada aparelho novo receberia todas as confirmações antigas: sai a
        // confirmada mais velha. Pendente nunca sai; sem confirmada, o retrato passa do teto.
        if self.queue.len() > QUEUE_KEPT && let Some(i) = self.queue.iter().position(|s| s.confirmed) {
            self.queue.remove(i);
        }
        true
    }

    /// O `removed` do painel da fila: as que esperavam e foram apagadas. Sem isto, o aparelho que abre o chat
    /// depois recebia do retrato a bolha `queued-<id>` delas ainda "na fila", sem a entrada no painel. A
    /// entregue nunca vem em `removed` (continua na projeção) e fica.
    fn retire_removed(&mut self, data: &str) {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(data) else { return };
        let Some(removed) = value.get("removed").and_then(serde_json::Value::as_array) else { return };
        for id in removed.iter().filter_map(serde_json::Value::as_str).map(|id| format!("queued-{id}")) {
            self.queue.retain(|slot| slot.id != id);
            if !self.retired.contains(&id) {
                self.retired.push_back(id);
            }
        }
        while self.retired.len() > RETIRED_KEPT {
            self.retired.pop_front();
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
        self.replay_at(Instant::now(), crate::queue_feed::wall_now())
    }

    /// `wall`: relógio de quem entra, em segundos.
    fn replay_at(&self, now: Instant, wall: f64) -> Vec<Bytes> {
        let toasts = self.toasts.iter().filter(|(_, at, _)| *at > now).map(|(_, at, toast)| {
            let mut toast = toast.clone();
            toast.insert("timeoutMs".into(), remaining_ms(*at, now).into());
            sse_frame("plugin_toast", &serde_json::Value::Object(toast).to_string(), None)
        });
        // Nota local mais velha que a folga já está no `/history`, no lugar do relógio dela.
        let queue = self.queue.iter().filter(|s| s.local_ts.is_none_or(|ts| ts >= wall - crate::queue_feed::LOCAL_NOTE_GRACE_S));
        self.latest.iter().flatten().cloned().chain(queue.map(|s| s.frame.clone())).chain(toasts).collect()
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
    /// O ator do Codex com terminal desta sessão é do Rust: o feed publica o estado. O pane continua no
    /// `Binding` (`headless: false`); dono é outra coisa.
    runtime_owned: std::sync::atomic::AtomicBool,
    /// Assinantes que entraram pelo convite (não contam como app do dono).
    guests: std::sync::atomic::AtomicUsize,
    /// Aparelhos do dono no chat: a conexão interna leva `app=1` só com algum (convidado e canal
    /// privado não calam o push do dono).
    owners: std::sync::atomic::AtomicUsize,
    /// Produtor da fila da geração atual (`queue_feed::run`); renasce a cada troca de conversa.
    queue_feed: Mutex<Option<tokio::task::AbortHandle>>,
    /// Produtor do `stats` da geração atual (`stats::run`); renasce a cada troca de conversa.
    stats_feed: Mutex<Option<tokio::task::AbortHandle>>,
    /// Vigia da fila desta sessão (`queue_panel::watch`); sai no `close`.
    queue_watch: Mutex<Option<tokio::task::AbortHandle>>,
    /// Acorda a vigia da fila: arquivo da fila mudou, a troca de conversa limpou o retrato ou o caminho chegou.
    queue_wake: Arc<Notify>,
    /// Arquivo da fila que a descoberta deu, avisado por quem já tem a `InternalInfo` (rota, canal privado,
    /// vigia da ligação): a vigia da fila nunca pergunta a descoberta por conta própria.
    queue_path: Mutex<Option<PathBuf>>,
    /// O portão do `problema` do estado (`problem_gate`) e o reenvio do estado retido quando o prazo vence.
    problems: Mutex<StateProblems>,
    me: std::sync::Weak<Hub>,
}

#[derive(Default)]
struct StateProblems { gate: crate::problem_gate::ProblemGate, timer: Option<tokio::task::AbortHandle>, epoch: u64 }

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
    fn start(name: &str, binding: Binding, ctx: SideCtx, owners: usize) -> Arc<Hub> {
        let (tx, _) = broadcast::channel(CHANNEL);
        let owned = ctx.runtime.get().is_some_and(|registry| registry.owns_codex_terminal(name));
        let seed = ctx.shown_problems.get(name);
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
                runtime_owned: std::sync::atomic::AtomicBool::new(owned),
                guests: std::sync::atomic::AtomicUsize::new(0),
                owners: std::sync::atomic::AtomicUsize::new(owners),
                queue_feed: Mutex::new(None),
                stats_feed: Mutex::new(None),
                queue_watch: Mutex::new(None),
                queue_wake: Arc::new(Notify::new()),
                queue_path: Mutex::new(None),
                problems: Mutex::new(StateProblems { gate: crate::problem_gate::ProblemGate::showing(seed), ..Default::default() }),
                me: weak.clone(),
            }
        });
        if let Some((generation, binding)) = hub.source_binding() {
            hub.start_queue_feed(generation, &binding);
            hub.start_stats_feed(generation, &binding);
        }
        hub.ensure_monitor();
        hub.restart_side();
        let watch = tokio::spawn(crate::queue_panel::watch(Arc::downgrade(&hub), hub.ctx.clone(), hub.queue_wake.clone())).abort_handle();
        *hub.queue_watch.lock().unwrap() = Some(watch);
        hub
    }

    /// Claude com terminal ganha o `Monitor` e Claude ou Codex sem terminal o feed (um por hub); a ligação que
    /// troca de dono (`/modo-execucao`) troca a tarefa, e sem dono ela sai. Dono que acabou (sessão
    /// morta, pânico) volta quando alguém religa ou assina de novo.
    fn ensure_monitor(self: &Arc<Self>) {
        let Some(spawn) = self.ctx.monitors.clone() else { return };
        let wanted = self.bound.lock().unwrap().as_ref().and_then(|b| self.owner_events(&b.binding).map(|e| (e, b.binding.provider)));
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
        self.bound.lock().unwrap().as_ref().and_then(|b| self.owner_events(&b.binding)).unwrap_or(&STATE_EVENTS)
    }

    /// Eventos que o Rust publica nesta ligação: os do `Binding`, ou os do feed quando o ator do Codex com
    /// terminal é do Rust (o registro avisa o hub na abertura e no fechamento).
    fn owner_events(&self, binding: &Binding) -> Option<&'static [&'static str]> {
        binding.state_events().or_else(|| self.codex_terminal_owned(binding).then_some(&FEED_EVENTS[..]))
    }

    fn codex_terminal_owned(&self, binding: &Binding) -> bool {
        binding.provider == Provider::Codex && !binding.headless && self.runtime_owned.load(Ordering::Acquire)
    }

    /// O estado sai do feed do runtime: sem terminal pelo `Binding`, Codex com terminal pelo dono.
    pub fn runtime_feed(&self) -> bool {
        self.bound.lock().unwrap().as_ref().is_some_and(|b| b.binding.runtime_feed() || self.codex_terminal_owned(&b.binding))
    }

    /// O registro abriu ou fechou o ator do Codex com terminal desta sessão: troca o dono do estado.
    pub(crate) fn set_runtime_owned(self: &Arc<Self>, owned: bool) {
        if self.runtime_owned.swap(owned, Ordering::AcqRel) != owned {
            self.ensure_monitor();
            self.wake.notify_one();
        }
    }

    /// A descoberta leu a sessão: o arquivo da fila que ela traz passa à vigia, que acorda se ele mudou.
    pub(crate) fn learn_queue(&self, info: &InternalInfo) {
        let path = info.history.get("queue").and_then(serde_json::Value::as_str).map(PathBuf::from);
        let mut slot = self.queue_path.lock().unwrap();
        if *slot != path {
            *slot = path;
            drop(slot);
            self.queue_wake.notify_one();
        }
    }

    pub(crate) fn queue_path(&self) -> Option<PathBuf> { self.queue_path.lock().unwrap().clone() }

    pub(crate) fn source_binding(&self) -> Option<(u64, Binding)> {
        self.bound.lock().unwrap().as_ref().map(|bound| (bound.generation, bound.binding.clone()))
    }

    /// A ligação atual: o feed escolhe por ela.
    pub fn binding(&self) -> Option<Binding> { self.bound.lock().unwrap().as_ref().map(|b| b.binding.clone()) }
    /// Os mods da sessão: a faixa e os painéis que cortam a prévia.
    pub(crate) fn mods(&self) -> &crate::mods::state::Mods { &self.ctx.mods }
    /// O registro do runtime, quando ligado.
    pub(crate) fn runtime_registry(&self) -> Option<Arc<crate::runtime::gateway::RuntimeRegistry>> { self.ctx.runtime.get().cloned() }

    /// Geração da ligação atual (a época do `Monitor`); `None` com o hub fechado.
    pub fn generation(&self) -> Option<u64> { self.bound.lock().unwrap().as_ref().map(|b| b.generation) }

    /// Session id da conversa ligada (`/clear` troca).
    pub fn session_key(&self) -> Option<String> {
        self.bound.lock().unwrap().as_ref().map(|b| b.binding.key.clone()).filter(|k| !k.is_empty())
    }

    pub fn jsonl(&self) -> Option<PathBuf> { self.bound.lock().unwrap().as_ref().map(|b| b.binding.jsonl.clone()) }

    pub fn wake(&self) -> Arc<Notify> { self.wake.clone() }

    pub(crate) fn diag(&self) -> &crate::diag::DiagClient { &self.ctx.diag }

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

    pub(crate) fn publish_own_bound(&self, expected: &(u64, Binding), event: &str, data: &str) -> bool {
        self.publish_own_bound_with(expected, event, data, || {})
    }

    pub(crate) fn publish_own_bound_with(&self, expected: &(u64, Binding), event: &str, data: &str, published: impl FnOnce()) -> bool {
        let bound = self.bound.lock().unwrap();
        let Some(bound) = bound.as_ref().filter(|bound| bound.generation == expected.0 && bound.binding == expected.1) else { return false };
        self.deliver_bound(event, data, bound.binding.provider == Provider::Claude);
        published();
        true
    }

    /// Bolha da fila lida na geração `generation`: depois de uma troca de conversa, a da anterior não
    /// entra no retrato nem sai pelo canal. `false`: geração vencida ou hub fechado.
    pub(crate) fn publish_queue(&self, generation: u64, item: &crate::queue_feed::QueueFrame) -> bool {
        // Sob a trava da ligação, como o `rebind`: o quadro sai inteiro antes do `reset` ou não sai.
        let bound = self.bound.lock().unwrap();
        if bound.as_ref().map(|b| b.generation) != Some(generation) {
            return false;
        }
        let frame = sse_frame(item.event, &item.data, None);
        let mut cache = self.cache.lock().unwrap();
        // Lida antes da exclusão no painel: os aparelhos abertos já a retiraram pelo `removed`.
        if cache.record_queue(item, &frame) {
            let _ = self.tx.send(Out::Side(frame));
        }
        true
    }

    /// A fila relida na geração `generation`: bolha cujo id saiu do arquivo sai do retrato. Quem abre o
    /// chat depois recebe o arquivo de agora; o aparelho aberto mantém a bolha até recarregar.
    pub(crate) fn retain_queue(&self, generation: u64, present: &std::collections::HashSet<String>) {
        let bound = self.bound.lock().unwrap();
        if bound.as_ref().map(|b| b.generation) == Some(generation) {
            self.cache.lock().unwrap().queue.retain(|slot| present.contains(&slot.id));
        }
    }

    /// Produtor da fila da geração dada; o anterior sai. Chamado sob a trava da ligação (ou com o hub
    /// ainda só deste chamador): duas trocas seguidas não deixam o da geração velha por último.
    fn start_queue_feed(self: &Arc<Self>, generation: u64, binding: &Binding) {
        let Some(dir) = &self.ctx.queue_dir else {
            if crate::warn_limit::allow(None, "queue_feed_disabled") {
                tracing::warn!(session = %self.name, code = "queue_feed_disabled", "fila: pasta da fila desconhecida; o chat fica sem as bolhas da fila");
            }
            return;
        };
        let path = crate::runtime::queue::projection_path(dir, &self.name);
        let feed = tokio::spawn(crate::queue_feed::run(Arc::downgrade(self), generation, binding.clone(), path, self.ctx.watchers.clone()));
        if let Some(old) = self.queue_feed.lock().unwrap().replace(feed.abort_handle()) {
            old.abort();
        }
    }

    fn stop_queue_feed(&self) {
        if let Some(feed) = self.queue_feed.lock().unwrap().take() {
            feed.abort();
        }
        if let Some(feed) = self.stats_feed.lock().unwrap().take() {
            feed.abort();
        }
    }

    /// Produtor do `stats` da geração dada, chamado junto com o da fila e sob a mesma trava.
    fn start_stats_feed(self: &Arc<Self>, generation: u64, binding: &Binding) {
        let feed = tokio::spawn(crate::stats::run(Arc::downgrade(self), generation, binding.clone(), self.ctx.watchers.clone()));
        if let Some(old) = self.stats_feed.lock().unwrap().replace(feed.abort_handle()) {
            old.abort();
        }
    }

    pub(crate) fn plugin_rates(&self) -> Arc<crate::plugin::sources::RateStore> { self.ctx.hubs.plugin_rates() }

    /// Assina o canal e copia o retrato dos eventos do estado (canal privado).
    fn subscribe_state(&self, guest: bool) -> Option<(broadcast::Receiver<Out>, Vec<Bytes>, &'static [&'static str])> {
        self.bound.lock().unwrap().as_ref()?;
        let events = self.state_events();
        let rx = self.tx.subscribe();
        let cached = self.cache.lock().unwrap().replay().into_iter().filter(|f| private_frame(f, events, guest)).collect();
        Some((rx, cached, events))
    }

    /// Aparelho novo cuja leitura da descoberta difere da ligação. Provider trocado religa já; outro
    /// transcript fica com a vigia do hub (`watch_binding`), que só troca lido duas vezes: uma
    /// leitura solta apagaria a conversa na tela de todos.
    /// Hub já fechado (removido do mapa): nada a fazer; o `attach` dá `None` e o aparelho recebe
    /// `reset` e volta ao Python.
    fn ensure_current(self: &Arc<Self>, binding: &Binding) {
        let current = match self.bound.lock().unwrap().as_ref() {
            None => return,
            Some(b) => b.binding.clone(),
        };
        if current.same_conversation(binding) {
            // Sessão que voltou com a mesma conversa (resume): o `Monitor` que viu a morte renasce.
            self.ensure_monitor();
        } else if current.changes_at_once(binding) {
            tracing::info!(session = %self.name, "provider trocou ou a sessão é outra; reset nos aparelhos");
            self.rebind(binding.clone());
            self.restart_side();
        } else {
            tracing::info!(session = %self.name, ligado = %file_name(&current.jsonl), lido = %file_name(&binding.jsonl),
                "aparelho leu outro transcript; a vigia confirma antes de trocar");
        }
    }

    /// Relê a descoberta a cada `WATCH_EVERY` e segue a troca de transcript (`/clear`, resume) só
    /// quando ela se repete; sessão sumida ou provider fora do Rust fecham o hub.
    async fn watch_binding(self: Arc<Self>) {
        let mut pending = None;
        let mut failing: Option<&'static str> = None;
        loop {
            tokio::time::sleep(WATCH_EVERY).await;
            let Some(before) = self.binding() else { return };
            let info = match self.ctx.info(&self.name).await {
                Ok(info) => {
                    if let Some(code) = failing.take() {
                        tracing::info!(session = %self.name, code, "descoberta da sessão voltou");
                    }
                    info
                }
                // Descoberta fora: nada prova troca nenhuma, e a conversa atual fica.
                Err(crate::locate::InfoFailed(code)) => {
                    if failing.replace(code).is_none() {
                        tracing::warn!(session = %self.name, code, "descoberta da sessão falhou; a vigia segue na conversa atual");
                    }
                    continue;
                }
            };
            let seen = match &info {
                None => None,
                Some(i) if Provider::parse(&i.provider).is_none() => Some(None),
                Some(i) => match Binding::from_info(i) {
                    Some(b) => Some(Some(b)),
                    // Linha ainda sem transcript (sessão nascendo, troca em curso): nada a comparar.
                    None => continue,
                },
            };
            // Um aparelho entrando pode ter religado o hub enquanto a leitura corria.
            let Some(current) = self.binding() else { return };
            if current != before {
                pending = None;
                continue;
            }
            if let Some(Some(b)) = &seen {
                self.learn_life(b);
            }
            if let Some(info) = &info {
                self.learn_queue(info);
            }
            let gone = seen.is_none();
            let had_pending = pending.is_some();
            match judge(&current, seen, &mut pending) {
                Verdict::Keep if had_pending => {
                    tracing::info!(session = %self.name, "troca de transcript não se repetiu; ignorada");
                }
                Verdict::Keep => {}
                Verdict::Wait if gone => {
                    tracing::info!(session = %self.name, "sessão fora da descoberta; confirma na próxima leitura");
                }
                Verdict::Wait => {
                    tracing::info!(session = %self.name, ligado = %file_name(&current.jsonl), "descoberta leu outro transcript; confirma na próxima leitura");
                }
                Verdict::Rebind(next) => {
                    tracing::info!(session = %self.name, de = %file_name(&current.jsonl), para = %file_name(&next.jsonl),
                        "transcript ou provider trocou; reset nos aparelhos");
                    self.rebind(next);
                    // Aborta esta mesma tarefa: a vigia renasce com a conexão interna da ligação nova.
                    self.restart_side();
                    return;
                }
                Verdict::Close => {
                    let why = if gone { "sessão sumiu da descoberta" } else { "provider saiu do Rust" };
                    tracing::info!(session = %self.name, why, "aparelhos voltam ao Python");
                    self.close();
                    return;
                }
            }
        }
    }

    /// Hub que nasceu sem a vida da sessão (rodada sem ela) passa a tê-la: sem isso, outra sessão com
    /// o mesmo nome esperaria duas leituras em vez de trocar na hora.
    fn learn_life(&self, seen: &Binding) {
        let mut bound = self.bound.lock().unwrap();
        if let Some(b) = bound.as_mut().filter(|b| b.binding.life.is_none() && b.binding.same_conversation(seen)) {
            b.binding.life = seen.life.clone();
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
        self.stop_queue_watch();
        self.stop_monitor();
        // Ligação solta antes: um `rebind` no meio não cria produtor que ninguém aborta.
        self.bound.lock().unwrap().take();
        self.stop_queue_feed();
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
        self.start_queue_feed(generation, &binding);
        self.start_stats_feed(generation, &binding);
        *bound = Some(Bound { binding, generation, tail });
        *self.committed.lock().unwrap() = None;
        // O estado retido era da conversa anterior: não volta depois da troca.
        { let mut problems = self.problems.lock().unwrap(); problems.epoch += 1; if let Some(timer) = problems.timer.take() { timer.abort(); } problems.gate = crate::problem_gate::ProblemGate::showing(self.ctx.shown_problems.get(&self.name)); }
        // Os avisos de mod ficam (o `cache.toasts` os guarda): são da sessão, não do transcript. A faixa
        // do Rust também é da sessão: volta ao retrato antes do `reset`, senão quem religa depois de um
        // `/clear` fica sem ela até o mod redesenhar. Limpeza e semeadura sob uma trava só.
        let ui = self.ctx.mods.replay(&self.name).into_iter().find(|(event, _)| *event == "plugin_ui");
        {
            let mut cache = self.cache.lock().unwrap();
            cache.latest = Default::default();
            // A fila é da conversa: o produtor da geração nova a relê.
            cache.queue.clear();
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
        // O retrato acabou de perder o estado: o `Monitor` e a vigia da fila publicam o da época nova sem esperar o tique.
        self.wake.notify_one();
        self.queue_wake.notify_one();
    }

    fn stop_queue_watch(&self) {
        if let Some(watch) = self.queue_watch.lock().unwrap().take() {
            watch.abort();
        }
    }

    /// Quadro de evento para os aparelhos: entra no retrato e sai pelo canal, salvo quando é igual
    /// ao último do mesmo tipo (o aparelho já o tem). Serve ao Python (side-events) e ao Rust (mods).
    fn deliver(&self, event: &str, data: &str) {
        let pane_question = self.bound.lock().unwrap().as_ref().is_some_and(|b| b.binding.provider == Provider::Claude);
        self.deliver_bound(event, data, pane_question);
    }

    fn deliver_bound(&self, event: &str, data: &str, pane_question: bool) {
        let gated = if event == "state" { self.gate_problem(data, pane_question) } else { None };
        self.deliver_frame(event, gated.as_deref().unwrap_or(data), pane_question);
    }

    fn deliver_frame(&self, event: &str, data: &str, pane_question: bool) {
        let frame = sse_frame(event, data, None);
        // Retrato antes do envio: quem assina entre os dois recebe repetido, nunca nada.
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

    /// `Some`: o estado sai sem o problema, que é diagnóstico (vai ao diário, nunca à tela) ou ainda não valeu
    /// pelo prazo; neste caso o original volta a passar por aqui quando o prazo vence, se nenhum estado mais
    /// novo chegou antes.
    fn gate_problem(&self, data: &str, pane_question: bool) -> Option<String> {
        use crate::problem_gate::Gate;
        let mut value: serde_json::Value = serde_json::from_str(data).ok()?;
        // P1: com a pergunta aberta, fatos que não identificam a interação são bloqueio (o card não responde) e saem
        // com o código da lista; a causa fica no diário.
        // Cartão com ID (seletor ou formulário) responde: aí os fatos caídos ficam só no diário.
        let unidentified = value["state"] == "awaiting_input" && value["problema"] == crate::state::facts::UNAVAILABLE
            && value["interaction_id"].is_null() && value["codex_question"]["interaction_id"].is_null()
            && value["problema_detalhe"].as_str().is_some_and(crate::list::bridge::interaction_unidentified);
        if unidentified {
            crate::problem_gate::report_hidden(&self.ctx.diag, &self.name, crate::state::facts::UNAVAILABLE, value["problema_detalhe"].as_str());
            value["problema"] = crate::list::bridge::INTERACTION_UNAVAILABLE.into();
            value["problema_detalhe"] = serde_json::Value::Null;
        }
        let hidden = value["problema"].as_str().filter(|code| crate::problem_gate::diagnostic(code, value["state"].as_str().unwrap_or("")))
            .map(str::to_owned);
        if let Some(code) = &hidden {
            crate::problem_gate::report_hidden(&self.ctx.diag, &self.name, code, value["problema_detalhe"].as_str());
            value["problema"] = serde_json::Value::Null;
            value["problema_detalhe"] = serde_json::Value::Null;
        }
        let cleared = || (hidden.is_some() || unidentified).then(|| value.to_string());
        // Fora de um runtime não há como reenviar: o problema sai como chegou, sem contar prazo.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return cleared() };
        let mut problems = self.problems.lock().unwrap();
        problems.epoch += 1;
        if let Some(timer) = problems.timer.take() { timer.abort(); }
        let now = tokio::time::Instant::now();
        let Gate::Hold(at) = problems.gate.admit(value["problema"].as_str(), now.into_std(), self.ctx.problem_hold) else { return cleared() };
        // O que volta depois do prazo é o estado já traduzido, nunca o código interno.
        let original = if unidentified { value.to_string() } else { data.to_owned() };
        let (hub, epoch) = (self.me.clone(), problems.epoch);
        problems.timer = Some(runtime.spawn(async move {
            tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await;
            if let Some(hub) = hub.upgrade() { hub.redeliver_problem(epoch, &original, pane_question); }
        }).abort_handle());
        value["problema"] = serde_json::Value::Null;
        value["problema_detalhe"] = serde_json::Value::Null;
        Some(value.to_string())
    }

    /// O estado retido sai com o problema, se nenhum estado mais novo nem troca de conversa veio depois. Conferir
    /// e entregar sob a mesma trava: um estado novo espera e sai depois deste, nunca por baixo dele.
    fn redeliver_problem(&self, epoch: u64, original: &str, pane_question: bool) {
        let mut problems = self.problems.lock().unwrap();
        if problems.epoch != epoch { return; }
        problems.timer = None;
        let code = serde_json::from_str::<serde_json::Value>(original).ok().and_then(|v| v["problema"].as_str().map(str::to_owned));
        let _ = problems.gate.admit(code.as_deref(), tokio::time::Instant::now().into_std(), self.ctx.problem_hold);
        self.deliver_frame("state", original, pane_question);
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
        self.stop_queue_watch();
        self.stop_monitor();
        self.stop_queue_feed();
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
        if let Some(hub) = hub.upgrade() {
            hub.close_generation(generation);
        }
    })
}

impl Hub {
    /// Fonte da geração `generation` que morreu (leitor do transcript, produtor da fila) fecha o hub: os
    /// aparelhos recebem `reset`, reconectam, e o hub novo nasce com fontes novas. Geração já trocada: nada.
    pub(crate) fn close_generation(self: &Arc<Self>, generation: u64) {
        if self.bound.lock().unwrap().as_ref().is_some_and(|b| b.generation == generation) {
            self.close();
        }
    }
}

enum SideEnd {
    Gone,
    Retry,
}

/// A conexão interna da ligação atual e a vigia da troca de transcript, que morrem juntas.
async fn run_side(hub: Arc<Hub>) {
    tokio::select! {
        _ = side_loop(hub.clone()) => {}
        _ = hub.clone().watch_binding() => {}
    }
}

async fn side_loop(hub: Arc<Hub>) {
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
    // O Python segue o transcript que o Rust escolheu; a troca religa esta conexão (`restart_side`).
    let Some(binding) = hub.bound.lock().unwrap().as_ref().map(|b| b.binding.clone()) else { return SideEnd::Gone };
    let url = format!(
        "http://{}/internal/sessions/{}/side-events?app={}&provider={}&jsonl={}",
        hub.ctx.upstream,
        utf8_percent_encode(&hub.name, NON_ALPHANUMERIC),
        u8::from(hub.owners.load(Ordering::SeqCst) > 0),
        binding.provider.as_str(),
        utf8_percent_encode(&binding.jsonl.to_string_lossy(), NON_ALPHANUMERIC),
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
        return SideEnd::Gone;
    }
    if resp.status() == StatusCode::BAD_REQUEST {
        // Conversa recusada pelo Python: repetir o mesmo pedido não muda a resposta.
        tracing::warn!(session = %hub.name, provider = binding.provider.as_str(), "conexão interna: 400 (conversa recusada); aparelhos voltam ao Python");
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
        if first {
            *attempt = 0;
            first = false;
        }
        match ev.event.as_str() {
            "ping" => {}
            // A faixa é do produtor do hub (`stats::run`); um Python antigo que ainda a mande não a sobrescreve.
            "stats" => {
                if crate::warn_limit::allow(Some(&hub.name), "stats_python_leak") {
                    tracing::warn!(session = %hub.name, code = "stats_python_leak", "faixa: o Python mandou `stats` de conversa do Rust; descartado");
                }
            }
            // Faixa e avisos de mod são do `Mods` (também da sessão aberta à mão): um do Python é velho.
            "plugin_ui" | "plugin_toast" => {}
            event => {
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

/// Quadro que o canal privado leva ao Python: o estado, a faixa dos mods, o painel da fila e as bolhas dela,
/// que o convidado também vê; ao dono pelo Connect, também os avisos de mod, que o Python não guarda mais.
fn private_frame(frame: &[u8], events: &[&str], guest: bool) -> bool {
    state_frame(frame, events) || state_frame(frame, &[LATEST[PLUGIN_UI], crate::queue_panel::EVENT, "message", "queue_confirmed"])
        || (!guest && state_frame(frame, &["plugin_toast"]))
}

/// Quadro que só o dono recebe: aviso de mod (texto dele) e pedido de navegador (url com o token).
pub(crate) fn owner_only(frame: &[u8]) -> bool {
    state_frame(frame, &["plugin_toast", "nav"])
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
    let last = tokio::task::spawn_blocking(move || match tail::last_committed(&binding.jsonl, binding.provider) {
        Ok(text) => text,
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound && crate::warn_limit::allow(None, "state_committed_seed") {
                tracing::warn!(code = "state_committed_seed", kind = ?e.kind(), "estado: transcript ilegível; a prévia pode repetir a resposta gravada");
            }
            None
        }
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
/// mais o `plugin_ui` e as bolhas da fila (`message`/`queue_confirmed`), de
/// quem entrou pelas portas dele (convite, Connect). Conta
/// como assinante do hub, então liga o `Monitor` e o produtor da fila igual a um aparelho do dono.
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
        st.diag.report("rust.state_channel_failed", &name, "internal_info", crate::routes::INFO_REASON);
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    // Convidado abrindo a conversa também reabre a sessão em erro, como o dono.
    if info.is_some() { st.request_reopen(&name); }
    // Só Claude (com ou sem terminal), Codex sem terminal e Codex com terminal de ator no Rust têm o estado
    // aqui; o resto o Python observa.
    let owned = |b: &Binding| b.provider == Provider::Codex && st.side.runtime.get().is_some_and(|r| r.owns_codex_terminal(&name));
    let Some(binding) = info.as_ref().and_then(Binding::from_info).filter(|b| b.state_events().is_some() || owned(b)) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    // O Python diz se quem entrou é convidado (`x-hangar-app: 0`); sem o cabeçalho, conta como app do dono.
    let guest = req.headers().get("x-hangar-app").is_some_and(|value| value == "0");
    // Quem lê daqui é o Python, que conta o próprio app: o canal nunca é aparelho do dono.
    let lease = st.side.hubs.acquire_with(&name, binding, &st.side, guest, false);
    if let Some(info) = &info { lease.hub.learn_queue(info); }
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
    let (hub, guest) = (lease.hub.clone(), lease.guest);
    let send = |f: Bytes| {
        let out = out.clone();
        async move { matches!(tokio::time::timeout(Duration::from_secs(30), out.send(f)).await, Ok(Ok(()))) }
    };
    if !send(tail::ping_frame()).await {
        return;
    }
    let mut ping = tokio::time::interval_at(tokio::time::Instant::now() + PRIVATE_PING, PRIVATE_PING);
    loop {
        let Some((mut rx, cached, events)) = hub.subscribe_state(guest) else { return };
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
                    Ok(Out::Side(f)) if private_frame(&f, events, guest) => if !send(f).await { return },
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
pub struct Hubs(Arc<HubMap>, Arc<crate::plugin::sources::RateStore>);

/// O mapa de hubs sem segurá-lo vivo: o `Mods` entrega por aqui sem formar ciclo com o `SideCtx`.
#[derive(Clone)]
pub struct WeakHubs(Weak<HubMap>, Weak<crate::plugin::sources::RateStore>);

impl WeakHubs {
    pub fn upgrade(&self) -> Option<Hubs> {
        Some(Hubs(self.0.upgrade()?, self.1.upgrade()?))
    }
}

pub struct Lease {
    hubs: Hubs,
    pub hub: Arc<Hub>,
    /// Convidado: assina o hub, mas não é o app do dono aberto (nem segura permissão, nem cala push).
    guest: bool,
    /// Aparelho do dono no chat (`Hub::owners`).
    owner: bool,
}

impl Hubs {
    pub fn plugin_rates(&self) -> Arc<crate::plugin::sources::RateStore> { self.1.clone() }
    /// Algum aparelho do dono (direto ou pelo Connect) com uma conversa aberta; convidado não conta, como no Python.
    pub fn app_present(&self) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).values().any(|(hub, n)| *n > hub.guests.load(Ordering::Acquire))
    }

    pub fn acquire(&self, name: &str, binding: Binding, ctx: &SideCtx) -> Lease { self.acquire_as(name, binding, ctx, false) }

    /// `guest`: assinante que entrou pelo convite.
    pub fn acquire_as(&self, name: &str, binding: Binding, ctx: &SideCtx, guest: bool) -> Lease {
        self.acquire_with(name, binding, ctx, guest, !guest)
    }

    /// `owner`: aparelho do dono no chat, o que decide o `app=` da conexão interna.
    pub fn acquire_with(&self, name: &str, binding: Binding, ctx: &SideCtx, guest: bool, owner: bool) -> Lease {
        let mut map = self.0.lock().unwrap();
        let hub = match map.get_mut(name) {
            Some((hub, n)) => {
                *n += 1;
                hub.clone()
            }
            None => {
                let hub = Hub::start(name, binding, ctx.clone(), usize::from(owner));
                // O convidado conta sob a mesma trava do `n`: fora dela, `app_present` veria o hub só com ele como do dono.
                if guest { hub.guests.fetch_add(1, Ordering::AcqRel); }
                map.insert(name.to_string(), (hub.clone(), 1));
                drop(map);
                // Hub novo começa sem retrato: a faixa e os avisos que o Rust já publicou entram agora.
                hub.seed(ctx.mods.replay(name));
                return Lease { hubs: self.clone(), hub, guest, owner };
            }
        };
        if guest { hub.guests.fetch_add(1, Ordering::AcqRel); }
        // Contado sob a trava do mapa: o `Drop` de outro aparelho não cruza o zero no meio.
        let first_owner = owner && hub.owners.fetch_add(1, Ordering::SeqCst) == 0;
        drop(map);
        hub.ensure_current(&binding);
        if first_owner {
            // O `app=` da conexão interna só muda religando; os aparelhos seguem no mesmo canal.
            hub.restart_side();
        }
        Lease { hubs: self.clone(), hub, guest, owner }
    }

    pub fn downgrade(&self) -> WeakHubs {
        WeakHubs(Arc::downgrade(&self.0), Arc::downgrade(&self.1))
    }

    /// Evento do próprio Rust (interface dos mods) para o hub da sessão. Sem aparelho não há hub, e o
    /// `Mods` guarda o valor para semear o hub que nascer depois.
    pub fn deliver(&self, name: &str, event: &str, data: &str) {
        let hub = self.0.lock().unwrap().get(name).map(|(hub, _)| hub.clone());
        if let Some(hub) = hub {
            hub.deliver(event, data);
        }
    }

    /// Aviso do registro: o ator do Codex com terminal de `name` abriu (`true`) ou fechou. Sem aparelho não
    /// há hub; o que nascer depois pergunta ao registro.
    pub(crate) fn set_runtime_owned(&self, name: &str, owned: bool) {
        let hub = self.0.lock().unwrap().get(name).map(|(hub, _)| hub.clone());
        if let Some(hub) = hub {
            hub.set_runtime_owned(owned);
        }
    }

    pub(crate) fn rebind_claude(&self,name:&str,conversation:&str,jsonl:PathBuf,queue:PathBuf)->Result<(),crate::runtime::protocol::RuntimeError> {
        self.rebind_claude_info(name,InternalInfo {provider:"claude".into(),jsonl:Some(jsonl),session_key:conversation.into(),history:serde_json::json!({"queue":queue}),headless:false,lifecycle_id:None,cwd:None})
    }
    pub(crate) fn rebind_claude_info(&self,name:&str,info:InternalInfo)->Result<(),crate::runtime::protocol::RuntimeError> {
        let binding=Binding::from_info(&info).filter(|b|matches!(b.provider,Provider::Claude|Provider::ClaudeHeadless)).ok_or_else(||crate::runtime::protocol::RuntimeError::new("lifecycle_hub_binding","vínculo do hub inválido"))?;
        let hub=self.0.lock().map_err(|_|crate::runtime::protocol::RuntimeError::new("lifecycle_hub_unavailable","hub da sessão indisponível"))?.get(name).map(|(hub,_)|hub.clone());
        if let Some(hub)=hub {
            hub.learn_queue(&info);
            // A mesma conversa (reabertura em bypass): nada a religar; o `reset` faria o chat sumir e voltar
            // em todos os aparelhos. Como o Python, só o transcript novo reseta.
            if hub.bound.lock().unwrap().as_ref().is_some_and(|bound|bound.binding.same_conversation(&binding)) {return Ok(());}
            hub.rebind(binding);
            // A conexão interna leva a conversa escolhida: renasce com a nova, como na troca da vigia.
            hub.restart_side();
        }
        Ok(())
    }

    /// A ligação do hub vivo da sessão: o transcript que o chat ao vivo segue.
    pub fn bound(&self, name: &str) -> Option<Binding> {
        let hub = self.0.lock().unwrap().get(name).map(|(h, _)| h.clone())?;
        let bound = hub.bound.lock().unwrap();
        bound.as_ref().map(|b| b.binding.clone())
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
        if self.guest { self.hub.guests.fetch_sub(1, Ordering::AcqRel); }
        *n -= 1;
        let last_owner = self.owner && self.hub.owners.fetch_sub(1, Ordering::SeqCst) == 1;
        if *n == 0 {
            let (h, _) = map.remove(&self.hub.name).expect("presente acima");
            drop(map);
            // Último aparelho saiu: fecha a conexão interna (o Python roda app_saiu) e o leitor.
            h.stop();
        } else if last_owner {
            drop(map);
            // Só convidados ficaram: religa com `app=0`.
            self.hub.restart_side();
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
        locate: Arc::new(crate::locate::Unavailable),
        monitors: None,
        mods: Default::default(),
        runtime: Default::default(),
        queue_dir: None,
        problem_hold: std::time::Duration::ZERO,
        shown_problems: Arc::default(),
        diag: crate::diag::DiagClient::new("127.0.0.1:9".parse().unwrap(), "s".into()),
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

    #[tokio::test]
    async fn stamped_publication_cannot_write_to_rebound_conversation() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = test_ctx();
        let binding = |sid: &str| Binding {provider:Provider::ClaudeHeadless,jsonl:dir.path().join(format!("{sid}.jsonl")),key:sid.into(),headless:true,life:None};
        let a = ctx.hubs.acquire("stamped", binding("a"), &ctx);
        let old = a.hub.source_binding().unwrap();
        let mut reopened = binding("a");
        reopened.jsonl = dir.path().join("another-account/a.jsonl");
        // Troca já confirmada (vigia ou runtime): um aparelho entrando com outra leitura não religa sozinho.
        a.hub.rebind(reopened);
        let b = &a;
        let fresh = b.hub.source_binding().unwrap();
        assert_eq!(old.1.key, fresh.1.key);
        assert_ne!(old.0, fresh.0);
        let published = crate::state::published::Published::default();
        published.set(1, "stamped", Some("a".into()), Arc::new(hangar_api::state::StateEvent {state:"working".into(), ..Default::default()}));
        assert!(!b.hub.publish_own_bound_with(&old, "state", r#"{"state":"idle"}"#,
            || published.set(1, "stamped", Some("a".into()), Arc::new(hangar_api::state::StateEvent {state:"idle".into(), ..Default::default()}))));
        assert_eq!(published.get("stamped",Some("a")).unwrap().state,"working");
        assert!(!b.hub.publish_own_bound(&old, "ask_question", r#"{"interaction_id":"a"}"#));
        assert!(b.hub.publish_own_bound(&fresh, "ask_question", r#"{"interaction_id":"b"}"#));
        let cached = b.hub.cache.lock().unwrap().replay();
        assert!(!cached.iter().any(|frame| String::from_utf8_lossy(frame).contains("\"interaction_id\":\"a\"")));
        assert!(cached.iter().any(|frame| String::from_utf8_lossy(frame).contains("\"interaction_id\":\"b\"")));
    }

    fn bind(jsonl: &str, life: Option<&str>) -> Binding {
        Binding { provider: Provider::Claude, jsonl: jsonl.into(), key: jsonl.into(), headless: false, life: life.map(str::to_owned) }
    }

    #[test]
    fn transcript_change_needs_two_readings_and_a_single_one_is_dropped() {
        let current = bind("a", Some("v1"));
        let mut pending = None;
        assert_eq!(judge(&current, Some(Some(bind("b", Some("v1")))), &mut pending), Verdict::Wait);
        assert_eq!(judge(&current, Some(Some(bind("a", Some("v1")))), &mut pending), Verdict::Keep);
        assert!(pending.is_none(), "a leitura solta não fica guardada");
        assert_eq!(judge(&current, Some(Some(bind("b", None))), &mut pending), Verdict::Wait);
        assert_eq!(judge(&current, Some(Some(bind("b", Some("v1")))), &mut pending), Verdict::Rebind(bind("b", Some("v1"))),
            "a vida que falta numa rodada não desfaz a confirmação");
        // Duas leituras diferentes entre si também não confirmam nada.
        assert_eq!(judge(&current, Some(Some(bind("b", None))), &mut pending), Verdict::Wait);
        assert_eq!(judge(&current, Some(Some(bind("c", None))), &mut pending), Verdict::Wait);
    }

    #[test]
    fn other_life_or_provider_changes_at_once_and_gone_needs_two_readings() {
        let current = bind("a", Some("v1"));
        let mut pending = None;
        assert_eq!(judge(&current, Some(Some(bind("b", Some("v2")))), &mut pending), Verdict::Rebind(bind("b", Some("v2"))));
        let codex = Binding { provider: Provider::Codex, ..bind("a", Some("v1")) };
        assert_eq!(judge(&current, Some(Some(codex.clone())), &mut pending), Verdict::Rebind(codex.clone()));
        let headless = Binding { headless: true, ..codex.clone() };
        assert_eq!(judge(&codex, Some(Some(headless.clone())), &mut pending), Verdict::Rebind(headless), "Codex trocou de modo");
        assert_eq!(judge(&current, Some(None), &mut pending), Verdict::Close, "provider fora do Rust");
        assert_eq!(judge(&current, None, &mut pending), Verdict::Wait);
        assert_eq!(judge(&current, None, &mut pending), Verdict::Close);
        assert_eq!(judge(&current, Some(Some(bind("a", None))), &mut pending), Verdict::Keep, "sem a vida, a mesma conversa");
    }

    #[test]
    fn cache_keeps_last_value_and_queue_by_id_without_nav() {
        assert_eq!(LATEST[PLUGIN_UI], "plugin_ui");
        let mut c = SideCache::default();
        let rec = |c: &mut SideCache, e: &str, d: &str| c.record(e, d, &sse_frame(e, d, None), true);
        rec(&mut c, "state", "{\"state\":\"working\"}");
        rec(&mut c, "state", "{\"state\":\"idle\"}");
        queue(&mut c, "message", "queued-1", None);
        queue(&mut c, "queue_confirmed", "queued-1", None);
        rec(&mut c, "nav", "{\"url\":\"http://x\"}");
        rec(&mut c, "plugin_ui", "{\"band\":null}");
        rec(&mut c, "plugin_ui", "{\"band\":{\"type\":\"Box\"}}");
        let r: Vec<String> = c.replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
        assert_eq!(r.len(), 3);
        assert!(r[0].starts_with("event: state") && r[0].contains("idle"));
        assert!(r[1].starts_with("event: plugin_ui") && r[1].contains("Box"));
        assert!(r[2].starts_with("event: queue_confirmed"));
    }

    /// Bolha da fila no retrato, como o produtor a publica.
    fn queue(c: &mut SideCache, event: &'static str, id: &str, local_ts: Option<f64>) -> bool {
        let item = crate::queue_feed::QueueFrame { event, data: format!("{{\"id\":\"{id}\"}}"), id: id.into(), local_ts };
        c.record_queue(&item, &sse_frame(event, &item.data, None))
    }

    fn replayed(c: &SideCache, wall: f64) -> String {
        c.replay_at(Instant::now(), wall).iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect()
    }

    #[test]
    fn queue_snapshot_drops_old_confirmations_first() {
        let mut c = SideCache::default();
        queue(&mut c, "message", "queued-pending", None);
        for i in 0..40 {
            queue(&mut c, "message", &format!("queued-{i}"), None);
            queue(&mut c, "queue_confirmed", &format!("queued-{i}"), None);
        }
        assert_eq!(c.queue.len(), QUEUE_KEPT);
        assert_eq!(c.queue[0].id, "queued-pending");
        assert_eq!(c.queue.last().unwrap().id, "queued-39");
    }

    /// O teto do retrato só descarta confirmada (ou nota local): pendente nunca sai.
    #[test]
    fn queue_cap_never_drops_a_pending_entry() {
        let mut c = SideCache::default();
        for i in 0..40 {
            queue(&mut c, "message", &format!("queued-{i}"), None);
        }
        queue(&mut c, "message", "local-1", Some(1.0));
        assert_eq!(c.queue.len(), 40, "a nota local sai antes de qualquer pendente");
        assert!(c.queue.iter().all(|s| s.id.starts_with("queued-")));
    }

    /// Quem abre o chat recebe a nota local só se ela nasceu até 30 s antes da entrada; ao vivo ela já saiu.
    #[test]
    fn snapshot_skips_local_notes_older_than_the_grace() {
        let mut c = SideCache::default();
        let wall = 1_790_000_000.0;
        queue(&mut c, "message", "local-old", Some(wall - 31.0));
        queue(&mut c, "message", "local-new", Some(wall - 10.0));
        queue(&mut c, "message", "queued-old", None);
        let r = replayed(&c, wall);
        assert!(!r.contains("local-old"));
        assert!(r.contains("local-new") && r.contains("queued-old"));
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
            c.replay_at(t0 + Duration::from_secs(s), 0.0)
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
        let binding = |n: &str| Binding { provider: Provider::Claude, jsonl: dir.path().join(format!("{n}.jsonl")), key: n.into(), headless: false, life: None };
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

    /// Parity B8: a reabertura em bypass religa a mesma conversa; o chat não recebe `reset` por isso.
    #[tokio::test]
    async fn same_conversation_rebind_does_not_reset_the_chat() {
        let dir = tempfile::tempdir().unwrap();
        let info = |sid: &str| InternalInfo { provider: "claude".into(), jsonl: Some(dir.path().join(format!("{sid}.jsonl"))), session_key: sid.into(),
            history: serde_json::json!({"queue": dir.path().join("s.jsonl")}), headless: false, lifecycle_id: None, cwd: None };
        let (spawn, _) = fake_monitors(IDLE_PANE);
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let lease = ctx.hubs.acquire("s", Binding::from_info(&info("a")).unwrap(), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        side_events(&mut rx, Duration::from_millis(200)).await;
        ctx.hubs.rebind_claude_info("s", info("a")).unwrap();
        let same = side_events(&mut rx, Duration::from_millis(200)).await;
        assert!(!same.iter().any(|(e, _)| e == "rebind"), "{same:?}");
        ctx.hubs.rebind_claude_info("s", info("b")).unwrap();
        let other = side_events(&mut rx, Duration::from_millis(200)).await;
        assert!(other.iter().any(|(e, _)| e == "rebind"), "{other:?}");
    }

    /// Decisão P24, como o Python: o convidado assina o hub, mas não é app do dono presente; o Connect é.
    #[tokio::test]
    async fn guest_subscriber_is_not_the_owner_app() {
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false, life: None };
        let (spawn, _) = fake_monitors(IDLE_PANE);
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let guest = ctx.hubs.acquire_as("s", binding.clone(), &ctx, true);
        assert!(!ctx.hubs.app_present(), "convidado sozinho não segura permissão");
        let owner = ctx.hubs.acquire_as("s", binding, &ctx, false);
        assert!(ctx.hubs.app_present());
        drop(owner);
        assert!(!ctx.hubs.app_present());
        drop(guest);
    }

    #[tokio::test]
    async fn python_state_for_rust_session_dropped_once() {
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false, life: None };
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
        let codex = Binding { provider: Provider::Codex, jsonl: dir.path().join("c.jsonl"), key: "c".into(), headless: false, life: None };
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
        let codex = Binding { provider: Provider::Codex, jsonl: dir.path().join("c.jsonl"), key: "c".into(), headless: true, life: None };
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
        let binding = Binding { provider: Provider::ClaudeHeadless, jsonl: dir.path().join("h.jsonl"), key: "h".into(), headless: false, life: None };
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
        let binding = |provider| Binding { provider, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false, life: None };
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
        let claude = Binding { provider: Provider::ClaudeHeadless, jsonl: dir.path().join("h.jsonl"), key: "h".into(), headless: false, life: None };
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

    #[cfg(unix)]
    #[tokio::test]
    async fn adopted_codex_terminal_moves_the_hub_to_the_runtime_feed_and_back() {
        use crate::runtime::codex_terminal::CodexTerminalTarget;
        use crate::runtime::gateway::RuntimeRegistry;
        let dir = tempfile::tempdir().unwrap();
        // App-server falso: aceita o WebSocket e segura a conexão; o pid é um processo com argv de app-server.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((stream, _)) = listener.accept().await {
                if let Ok(ws) = tokio_tungstenite::accept_async(stream).await { held.push(ws); }
            }
        });
        // `read` embutido, sem `sleep` filho: o fork logo depois da prova deixava o argv ilegível no macOS.
        let mut child = std::process::Command::new("sh").args(["-c", "read x", "app-server", &endpoint])
            .stdin(std::process::Stdio::piped()).spawn().unwrap();
        // O `spawn` volta quando o exec troca a memória do filho, antes de o kernel gravar o argv novo: por um
        // instante o `cmdline` vem vazio, e a admissão lê o argv. O app-server adotado de verdade já roda há tempo.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while crate::runtime::codex_terminal::app_server(child.id(), &endpoint) != crate::runtime::codex_terminal::AppServer::Ours {
            assert!(std::time::Instant::now() < deadline, "o filho não chegou ao argv do app-server");
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let (spawn, count) = fake_feeds();
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "s".into(), "i".into()));
        registry.bind_hubs(ctx.hubs.downgrade());
        let _ = ctx.runtime.set(registry.clone());
        let binding = Binding { provider: Provider::Codex, jsonl: dir.path().join("rollout.jsonl"), key: "thread-1".into(), headless: false, life: None };
        let lease = ctx.hubs.acquire("cx-proof", binding, &ctx);
        assert!(!lease.hub.runtime_feed() && lease.hub.owned_events().is_empty(), "sem ator no Rust, o Python observa");
        let target = CodexTerminalTarget { key: "codex-key".into(), generation: 1, name: "cx-proof".into(), endpoint: endpoint.clone(),
            app_pid: child.id(), thread_id: "thread-1".into(),
            terminal: crate::terminal_input::TerminalBinding { name: "cx-proof".into(), pane: "%7".into(), conversation: "thread-1".into(),
                generation: 1, created: 1, mux_argv: vec!["tmux".into()], windows: false, clipboard_lock_path: None },
            metadata: serde_json::json!({"key": "codex-key", "codex_home": dir.path().join("codex-home")}),
            transcript: None, lease_path: dir.path().join("lock"), state_path: dir.path().join("state"), projection_dir: dir.path().join("projection"),
            sidecar_dir: None };
        registry.open_codex_terminal(target).await.unwrap();
        assert!(lease.hub.runtime_feed(), "ator adotado aberto: o estado sai do feed");
        assert_eq!(lease.hub.owned_events(), &FEED_EVENTS[..]);
        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(!lease.hub.binding().unwrap().headless, "o pane continua no binding");
        registry.close("codex-key", 1).await.unwrap();
        assert!(!lease.hub.runtime_feed() && lease.hub.owned_events().is_empty(), "fechado, o estado volta ao Python");
        child.kill().unwrap();
        child.wait().unwrap();
        server.abort();
    }

    #[tokio::test]
    async fn headless_flip_rebinds_and_swaps_owner() {
        // `/modo-execucao`: o `info` com `headless` trocado religa o hub, que liga ou desliga o feed.
        let dir = tempfile::tempdir().unwrap();
        let (spawn, count) = fake_feeds();
        let ctx = SideCtx { monitors: Some(spawn), ..idle_ctx() };
        let binding = |headless| Binding { provider: Provider::Codex, jsonl: dir.path().join("c.jsonl"), key: "c".into(), headless, life: None };
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
        let binding = Binding { provider: Provider::Codex, jsonl: dir.path().join("c.jsonl"), key: "c".into(), headless: true, life: None };
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
        let codex = Binding { provider: Provider::Codex, jsonl: "c".into(), key: "c".into(), headless: true, life: None };
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
        // Sem esta rota a conexão interna leva 404, que é "sessão sumiu": o hub fecharia com o canal
        // ainda aberto, e o teste dependia de chegar antes disso.
        let python = axum::Router::new()
            .route("/internal/sessions/{name}/side-events", get(|| async {
                axum::body::Body::from_stream(futures_util::stream::pending::<Result<axum::body::Bytes, std::convert::Infallible>>())
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
        st.side.locate = Arc::new(crate::locate::Fixed(Some(serde_json::from_value(serde_json::json!(
            {"provider": "claude", "jsonl": jsonl, "session_key": "k", "history": {}})).unwrap())));
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
    async fn side_events_404_closes_the_hub_with_a_subscriber() {
        // 404 na conexão interna é "sessão sumiu": o hub fecha mesmo com assinante.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, axum::Router::new()).await.unwrap() });
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false, life: None };
        let ctx = SideCtx { upstream, ..test_ctx() };
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        let gone = tokio::time::timeout(Duration::from_secs(2), async {
            while ctx.hubs.0.lock().unwrap().contains_key("s") { tokio::time::sleep(Duration::from_millis(20)).await }
        }).await;
        assert!(gone.is_ok(), "o 404 tira o hub do mapa com o assinante ainda preso");
        drop(lease);
    }

    #[tokio::test]
    async fn own_pane_question_not_replayed_after_answer() {
        // A pergunta do `Monitor` sai uma vez por pergunta: quem chega depois da resposta não a recebe.
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false, life: None };
        let ctx = idle_ctx();
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        let late = |hub: &Hub| hub.subscribe_state(true).unwrap().1.iter().any(|f| f.starts_with(b"event: ask_question"));
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
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("a.jsonl"), key: "a".into(), headless: false, life: None };
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
    fn private_channel_forwards_state_band_and_queue() {
        for (event, pass) in [("state", true), ("preview", true), ("ask_question", true), ("suggest", true), ("plugin_ui", true),
                              ("queue", true), ("message", true), ("queue_confirmed", true),
                              ("stats", false), ("plugin_toast", false), ("nav", false)] {
            assert_eq!(private_frame(&sse_frame(event, "{}", None), &STATE_EVENTS, true), pass, "{event}");
        }
    }

    /// O dono que entra pela porta do Connect recebe os avisos de mod pelo canal privado: o Python não os
    /// guarda mais. O convidado continua sem eles.
    #[test]
    fn private_channel_carries_mod_toasts_only_to_the_owner() {
        let toast = sse_frame("plugin_toast", r#"{"id":"rs-1","text":"x","timeoutMs":4000}"#, None);
        assert!(private_frame(&toast, &STATE_EVENTS, false), "dono pelo Connect");
        assert!(!private_frame(&toast, &STATE_EVENTS, true), "convidado");
    }

    #[test]
    fn queue_snapshot_is_replayed_to_late_devices() {
        let mut c = SideCache::default();
        let frame = sse_frame("queue", r#"{"entries":[],"actions":true}"#, None);
        c.record("queue", r#"{"entries":[],"actions":true}"#, &frame, false);
        assert!(c.replay().iter().any(|f| f.starts_with(b"event: queue\r\n")));
        assert!(private_frame(&frame, &STATE_EVENTS, true), "o canal privado leva a fila a quem entrou pelo Python");
    }

    #[tokio::test]
    async fn queue_watch_publishes_the_waiting_list_and_follows_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let (jsonl, queue) = (dir.path().join("t.jsonl"), dir.path().join("fila").join("s.jsonl"));
        std::fs::write(&jsonl, "{\"type\":\"user\",\"timestamp\":\"2026-10-14T12:00:00Z\",\"message\":{\"content\":\"oi\"}}\n").unwrap();
        // 2026-10-14T12:00:00Z, o começo da conversa: as linhas são dela.
        let start = 1_791_979_200.0;
        let write = |rows: &[serde_json::Value]| {
            std::fs::create_dir_all(queue.parent().unwrap()).unwrap();
            let tmp = queue.with_extension("tmp");
            std::fs::write(&tmp, rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
            std::fs::rename(&tmp, &queue).unwrap();
        };
        write(&[serde_json::json!({"id":"a","text":"primeira","ts":start + 1.0,"delivered":false}),
                serde_json::json!({"id":"sent","text":"entregue","ts":start + 2.0,"delivered":true})]);
        let info: InternalInfo = serde_json::from_value(serde_json::json!({"provider": "claude", "jsonl": jsonl, "session_key": "k", "history": {"queue": queue}})).unwrap();
        // Descoberta fora do ar: a vigia da fila anda só com o caminho que o hub recebeu.
        let ctx = SideCtx { locate: Arc::new(crate::locate::Unavailable), ..idle_ctx() };
        let binding = Binding { provider: Provider::Claude, jsonl: jsonl.clone(), key: "k".into(), headless: false, life: None };
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        let mut rx = lease.hub.tx.subscribe();
        // A rota que abriu o chat já leu a descoberta e passa o caminho; a vigia não pergunta de novo.
        lease.hub.learn_queue(&info);
        let queue_frames = |hub: &Hub| hub.cache.lock().unwrap().replay().into_iter()
            .filter(|f| f.starts_with(b"event: queue\r\n")).map(|f| String::from_utf8_lossy(&f).into_owned()).collect::<Vec<_>>();
        tokio::time::timeout(Duration::from_secs(5), async { while queue_frames(&lease.hub).is_empty() { tokio::time::sleep(Duration::from_millis(20)).await } })
            .await.expect("a lista sai sem esperar mudança");
        let first = queue_frames(&lease.hub);
        assert!(first[0].contains(r#""entries":[{"id":"a","text":"primeira","ts":1791979201.0}]"#), "{first:?}");
        assert!(first[0].contains(r#""actions":false"#), "sem registro do runtime, sem botões");
        // Outro aparelho reordenou: a projeção regravada chega a quem está no chat.
        write(&[serde_json::json!({"id":"b","text":"segunda","ts":start + 3.0,"delivered":false}),
                serde_json::json!({"id":"a","text":"primeira","ts":start + 1.0,"delivered":false})]);
        let ids = |frame: &Bytes| {
            let data = String::from_utf8_lossy(frame).lines().nth(1).unwrap_or("").trim_start_matches("data: ").to_owned();
            serde_json::from_str::<serde_json::Value>(&data).ok().and_then(|v| v["entries"].as_array().cloned()).unwrap_or_default()
                .iter().filter_map(|e| e["id"].as_str().map(str::to_owned)).collect::<Vec<_>>()
        };
        // O observador avisa na hora; sem ele, a releitura de 5 s cobre.
        let changed = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                match rx.recv().await {
                    Ok(Out::Side(f)) if f.starts_with(b"event: queue\r\n") && ids(&f) != ["a"] => return ids(&f),
                    Ok(_) => {}
                    Err(error) => panic!("canal do hub: {error}"),
                }
            }
        }).await.expect("a mudança chega");
        assert_eq!(changed, ["b", "a"]);
        async fn next_queue(rx: &mut broadcast::Receiver<Out>) -> serde_json::Value {
            tokio::time::timeout(Duration::from_secs(8), async {
                loop {
                    match rx.recv().await {
                        Ok(Out::Side(f)) if f.starts_with(b"event: queue\r\n") => {
                            let data = String::from_utf8_lossy(&f).lines().nth(1).unwrap_or("").trim_start_matches("data: ").to_owned();
                            return serde_json::from_str::<serde_json::Value>(&data).unwrap();
                        }
                        Ok(_) => {}
                        Err(error) => panic!("canal do hub: {error}"),
                    }
                }
            }).await.expect("o painel republica")
        }
        // Apagada noutro aparelho: some da projeção e vai em `removed`; a entregue fica como linha entregue.
        write(&[serde_json::json!({"id":"a","text":"primeira","ts":start + 1.0,"delivered":true})]);
        let gone = next_queue(&mut rx).await;
        assert_eq!(gone["entries"], serde_json::json!([]));
        assert_eq!(gone["removed"], serde_json::json!(["b"]), "{gone}");
        // A troca de conversa republica mesmo igual, sem perguntar a descoberta.
        let other = dir.path().join("t2.jsonl");
        std::fs::write(&other, "").unwrap();
        lease.hub.rebind(Binding { provider: Provider::Claude, jsonl: other, key: "k2".into(), headless: false, life: None });
        let again = next_queue(&mut rx).await;
        assert_eq!(again["removed"], serde_json::json!([]), "{again}");
        close_hub(&lease.hub);
        assert!(lease.hub.queue_watch.lock().unwrap().is_none(), "a vigia sai com o hub");
    }

    #[test]
    fn queued_batch_history_large_cut_does_not_resurrect_late_member_frames() {
        let mut cache=SideCache::default();
        let removed:Vec<_>=(0..1000).map(|n|format!("part-{n}")).collect();
        let data=serde_json::json!({"entries":[],"removed":removed}).to_string();
        cache.record("queue",&data,&sse_frame("queue",&data,None),false);
        for id in removed {assert!(!queue(&mut cache,"message",&format!("queued-{id}"),None),"membro selado reapareceu: {id}");}
    }

    #[tokio::test]
    async fn queued_batch_history_two_consumers_and_reconnect_see_one_prompt() {
        use crate::runtime::{queue::{Action,State,Store},protocol::ClockSample};
        let dir=tempfile::tempdir().unwrap();let projection=dir.path().join("projection");
        let ts=crate::queue_feed::wall_now();let clock=ClockSample {monotonic_s:0.0,epoch_s:ts};
        let mut store=Store::open(&dir.path().join("state"),&projection,State::new("key",1,"fila",vec![])).unwrap();
        for id in ["a","b","c"] {store.exec(1,id,clock,Action::Append {text:id.into(),delivered:false,ts:None,pre_transcript:false,entry_id:Some(id.into())}).unwrap();}
        let ctx=SideCtx {queue_dir:Some(projection.clone()),..test_ctx()};
        let binding=queue_binding(dir.path(),"chat");let jsonl=binding.jsonl.clone();
        let lease=ctx.hubs.acquire("fila",binding.clone(),&ctx);
        let mut first=lease.hub.tx.subscribe();let mut second=lease.hub.tx.subscribe();
        let info:InternalInfo=serde_json::from_value(serde_json::json!({"provider":"claude","jsonl":jsonl,"session_key":"chat","history":{"queue":crate::runtime::queue::projection_path(&projection,"fila")}})).unwrap();
        lease.hub.learn_queue(&info);
        queue_frame(&mut first,"queued-c","queued_delivered").await;
        queue_frame(&mut second,"queued-c","queued_delivered").await;
        tokio::time::timeout(Duration::from_secs(10),async {loop {
            let ready=lease.hub.cache.lock().unwrap().latest.iter().flatten().any(|frame| {
                if !frame.starts_with(b"event: queue\r\n") {return false;}
                let frame=String::from_utf8_lossy(frame);let data=frame.lines().find_map(|line|line.strip_prefix("data: ")).unwrap();
                serde_json::from_str::<serde_json::Value>(data).unwrap()["entries"].as_array().is_some_and(|rows|rows.len()==3)
            });
            if ready {break;}tokio::task::yield_now().await;
        }}).await.unwrap();
        let (panel_first,panel_second)=(first.resubscribe(),second.resubscribe());
        let rows=store.exec(1,"batch",clock,Action::ClaimBatch {min_ts:0.0}).unwrap();
        let id=rows[0]["id"].as_str().unwrap().to_owned();
        let next_panel=|mut rx:broadcast::Receiver<Out>| async move {tokio::time::timeout(Duration::from_secs(10),async move {loop {
            if let Ok(Out::Side(frame))=rx.recv().await && frame.starts_with(b"event: queue\r\n") {
                let frame=String::from_utf8_lossy(&frame);let data=frame.lines().find_map(|line|line.strip_prefix("data: ")).unwrap();
                let value:serde_json::Value=serde_json::from_str(data).unwrap();
                if value["removed"].as_array().is_some_and(|ids|ids.len()==3) {break value;}
            }
        }}).await.unwrap()};
        // Os dois consumidores conservam a mesma remoção dos IDs originais.
        let (a,b)=tokio::join!(next_panel(panel_first),next_panel(panel_second));
        assert_eq!(a["removed"],b["removed"]);
        let echo=serde_json::json!({"type":"user","uuid":"echo","timestamp":ts+1.0,"message":{"role":"user","content":"a\nb\nc"}});
        std::fs::write(&jsonl,format!("{echo}\n")).unwrap();
        store.exec(1,"confirmed",clock,Action::Confirm {entry_ids:vec![id.clone()]}).unwrap();
        queue_frame(&mut first,&format!("queued-{id}"),"queue_confirmed").await;
        queue_frame(&mut second,&format!("queued-{id}"),"queue_confirmed").await;
        let snapshot=lease.hub.attach(None).await.unwrap();
        let replay=snapshot.frames.iter().map(|frame|String::from_utf8_lossy(frame).into_owned()).collect::<String>();
        for member in ["a","b","c"] {assert!(!replay.contains(&format!("\"id\":\"queued-{member}\"")));}
        assert_eq!(replay.matches(&format!("\"id\":\"queued-{id}\"")).count(),1);
        let request=crate::transcript::history::HistoryRequest {provider:Provider::Claude,jsonl,queue:Some(crate::runtime::queue::projection_path(&projection,"fila")),limit:None,tail_window:crate::transcript::history::TAIL_WINDOW};
        let history=crate::transcript::history::merged_history(&request).unwrap();
        assert_eq!(history.len(),1,"o histórico conserva apenas o eco real: {history:?}");
        drop(snapshot);
        close_hub(&lease.hub);
    }

    #[test]
    fn removed_queue_entries_leave_the_snapshot_and_late_frames_stay_out() {
        let mut c = SideCache::default();
        let panel = |c: &mut SideCache, d: &str| c.record("queue", d, &sse_frame("queue", d, None), false);
        queue(&mut c, "message", "queued-a", None);
        queue(&mut c, "message", "queued-b", None);
        queue(&mut c, "queue_confirmed", "queued-sent", None);
        panel(&mut c, r#"{"entries":[{"id":"a"}],"actions":true,"removed":["b"]}"#);
        let ids = |c: &SideCache| c.queue.iter().map(|s| s.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&c), ["queued-a", "queued-sent"], "a apagada sai; a que espera e a entregue ficam");
        // Quadro que o produtor leu antes da exclusão não a traz de volta ao retrato.
        assert!(!queue(&mut c, "message", "queued-b", None));
        assert_eq!(ids(&c), ["queued-a", "queued-sent"]);
        // A publicação seguinte (sem `removed`) não esquece a apagada.
        panel(&mut c, r#"{"entries":[{"id":"a"}],"actions":false,"removed":[]}"#);
        assert!(!queue(&mut c, "message", "queued-b", None));
        assert_eq!(ids(&c), ["queued-a", "queued-sent"]);
    }

    #[tokio::test]
    async fn deleted_waiting_entry_is_not_replayed_to_a_late_device() {
        let dir = tempfile::tempdir().unwrap();
        let (jsonl, queue) = (dir.path().join("t.jsonl"), dir.path().join("fila").join("s.jsonl"));
        std::fs::write(&jsonl, "{\"type\":\"user\",\"timestamp\":\"2026-10-14T12:00:00Z\",\"message\":{\"content\":\"oi\"}}\n").unwrap();
        let start = 1_791_979_200.0;
        let write = |rows: &[serde_json::Value]| {
            std::fs::create_dir_all(queue.parent().unwrap()).unwrap();
            let tmp = queue.with_extension("tmp");
            std::fs::write(&tmp, rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
            std::fs::rename(&tmp, &queue).unwrap();
        };
        let a = serde_json::json!({"id":"a","text":"primeira","ts":start + 1.0,"delivered":false});
        let sent = serde_json::json!({"id":"sent","text":"entregue","ts":start + 2.0,"delivered":true,"confirmed":true});
        write(&[sent.clone(), a.clone(), serde_json::json!({"id":"b","text":"segunda","ts":start + 3.0,"delivered":false})]);
        let info: InternalInfo = serde_json::from_value(serde_json::json!({"provider": "claude", "jsonl": jsonl, "session_key": "k", "history": {"queue": queue}})).unwrap();
        let ctx = SideCtx { locate: Arc::new(crate::locate::Unavailable), queue_dir: Some(dir.path().join("fila")), ..idle_ctx() };
        let binding = Binding { provider: Provider::Claude, jsonl: jsonl.clone(), key: "k".into(), headless: false, life: None };
        let lease = ctx.hubs.acquire("s", binding, &ctx);
        let mut rx = lease.hub.tx.subscribe();
        lease.hub.learn_queue(&info);
        let generation = lease.hub.generation().expect("ligado");
        let queue_event = |frame: &Bytes| frame.starts_with(b"event: queue\r\n").then(|| {
            let data = String::from_utf8_lossy(frame).lines().nth(1).unwrap_or("").trim_start_matches("data: ").to_owned();
            serde_json::from_str::<serde_json::Value>(&data).unwrap()
        });
        let next_queue = |rx: &mut broadcast::Receiver<Out>| {
            let mut rx = rx.resubscribe();
            async move {
                tokio::time::timeout(Duration::from_secs(8), async {
                    loop {
                        match rx.recv().await {
                            Ok(Out::Side(f)) => if let Some(v) = queue_event(&f) { return v },
                            Ok(_) => {}
                            Err(error) => panic!("canal do hub: {error}"),
                        }
                    }
                }).await.expect("o painel publica")
            }
        };
        // A primeira lista sai com as duas esperando, e o produtor já pôs as três bolhas no retrato.
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let (listed, bubbles) = {
                    let cache = lease.hub.cache.lock().unwrap();
                    (cache.latest.iter().flatten().find_map(queue_event), cache.queue.len())
                };
                if listed.is_some_and(|v| v["entries"].as_array().is_some_and(|e| e.len() == 2)) && bubbles == 3 { break }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("a lista e as bolhas saem");
        // Apagada noutro aparelho (`DELETE ?only_waiting=true`): some da projeção.
        let waiting_for_removal = next_queue(&mut rx);
        write(&[sent, a]);
        let gone = waiting_for_removal.await;
        assert_eq!(gone["removed"], serde_json::json!(["b"]), "{gone}");
        // Quadro que o produtor leu antes da exclusão: sai depois e não volta ao retrato nem a quem já
        // está no chat (esse já retirou a bolha pelo `removed`).
        let mut open_device = lease.hub.tx.subscribe();
        let late = crate::queue_feed::QueueFrame { event: "message", id: "queued-b".into(), local_ts: None,
            data: r#"{"kind":"user_msg","id":"queued-b","text":"segunda","queued_delivered":false}"#.into() };
        assert!(lease.hub.publish_queue(generation, &late), "a geração segue valendo");
        assert!(on_side_event(&lease.hub, "stats", r#"{"turns":1}"#));
        let seen = side_events(&mut open_device, Duration::from_millis(100)).await;
        assert!(seen.iter().any(|(event, _)| event == "stats"), "o canal segue entregando: {seen:?}");
        assert!(!seen.iter().any(|(_, data)| data.contains("queued-b")), "bolha fantasma no aparelho aberto: {seen:?}");
        // O aparelho que abre o chat agora recebe o retrato.
        let late = lease.hub.attach(None).await.expect("aparelho novo");
        let frames: Vec<String> = late.frames.iter().map(|f| String::from_utf8_lossy(f).into_owned()).collect();
        let has = |id: &str| frames.iter().any(|f| f.contains(&format!(r#""id":"{id}""#)));
        assert!(!has("queued-b"), "bolha fantasma da apagada no retrato: {frames:#?}");
        assert!(has("queued-a"), "a que ainda espera continua no retrato");
        assert!(has("queued-sent"), "a entregue continua no retrato");
        close_hub(&lease.hub);
    }

    #[tokio::test]
    async fn seed_does_not_overwrite_a_newer_band() {
        use serde_json::json;
        let dir = tempfile::tempdir().unwrap();
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("t.jsonl"), key: "k".into(), headless: false, life: None };
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
        let binding = Binding { provider: Provider::Claude, jsonl: dir.path().join("t.jsonl"), key: tail::PANIC_KEY.into(), headless: false, life: None };
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
        let binding = |n: &str| Binding { provider: Provider::Claude, jsonl: dir.path().join(n), key: n.into(), headless: false, life: None };
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

    /// Claude com terminal aberto no runtime: o hub ligado por `SideCtx::info` leva a conversa como
    /// chave, que é o que o `Monitor` compara antes de capturar (senão `state_binding_changed`).
    #[tokio::test]
    async fn terminal_hub_bound_by_the_runtime_keys_the_conversation() {
        use crate::runtime::gateway::RuntimeRegistry;
        let dir = tempfile::tempdir().unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let registry = Arc::new(RuntimeRegistry::new("127.0.0.1:9".parse().unwrap(), "secret-test".into(), "instance-test".into()));
        let ctx = test_ctx();
        registry.bind_hubs(ctx.hubs.downgrade());
        assert!(ctx.runtime.set(registry.clone()).is_ok());
        let server = tokio::spawn(crate::runtime::gateway::serve(listener, registry.clone(), "secret-test".into(), "instance-test".into(), crate::INTERNAL_PROTOCOL));
        let pid = std::process::id();
        let terminal = serde_json::json!({"name":"s","pane":"%1","conversation":"sid","generation":1,"created":1,"mux_argv":["fake"],"windows":false,"clipboard_lock_path":null});
        let descriptor = serde_json::json!({"name":"s","key":"terminal_key","provider":"claude","headless":false,
            "meta":{"key":"terminal_key","terminal":terminal,"config_dir":dir.path(),"agent_pid":pid,"agent_birth":crate::terminal_facts::agent_birth(pid).unwrap(),"fingerprint":"fixture-life","message_origin":"uds:fixture"},
            "jsonl":dir.path().join("sid.jsonl"),"projection_dir":dir.path().join("projection"),"state_path":dir.path().join("state"),"lock_path":dir.path().join("lease"),"generation":1});
        let send = |command: serde_json::Value| reqwest::Client::new().post(format!("http://{address}/runtime/op"))
            .header("x-hangar-internal", "secret-test").header("x-hangar-runtime-instance", "instance-test").header("content-type", "application/json")
            .body(serde_json::json!({"protocol":crate::INTERNAL_PROTOCOL,"instance":"instance-test","key":"terminal_key","generation":1,"operation_id":"op","clock":{"monotonic_s":0.0,"epoch_s":0.0},"command":command}).to_string()).send();
        assert!(send(serde_json::json!({"kind":"open","descriptor":descriptor})).await.unwrap().status().is_success());
        let info = ctx.info("s").await.unwrap().unwrap();
        let lease = ctx.hubs.acquire("s", Binding::from_info(&info).unwrap(), &ctx);
        let life = registry.interaction_binding_readonly("s").await.unwrap();
        assert_eq!(lease.hub.session_key().as_deref(), Some(life.conversation.as_str()));
        assert!(send(serde_json::json!({"kind":"close"})).await.unwrap().status().is_success());
        server.abort();
    }

    fn write_queue(dir: &std::path::Path, name: &str, rows: &[serde_json::Value]) {
        let path = crate::runtime::queue::projection_path(dir, name);
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, rows.iter().map(|r| format!("{r}\n")).collect::<String>()).unwrap();
        std::fs::rename(tmp, path).unwrap();
    }

    /// Espera no canal o quadro da fila com o id e o trecho dados.
    async fn queue_frame(rx: &mut broadcast::Receiver<Out>, id: &str, has: &str) -> String {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Ok(Out::Side(f)) = rx.recv().await {
                    let f = String::from_utf8_lossy(&f).into_owned();
                    if f.contains(&format!("\"id\":\"{id}\"")) && f.contains(has) {
                        return f;
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("quadro {id} com {has} não chegou"))
    }

    fn queue_binding(dir: &std::path::Path, sid: &str) -> Binding {
        Binding { provider: Provider::Claude, jsonl: dir.join(format!("{sid}.jsonl")), key: sid.into(), headless: false, life: None }
    }

    /// A fila do chat sai do Rust: a projeção vira bolha no retrato e no canal, e a gravação seguinte
    /// reemite só a entrada cuja entrega mudou.
    #[tokio::test]
    async fn queue_feed_follows_the_projection() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), ..test_ctx() };
        let ts = crate::queue_feed::wall_now() - 5.0;
        write_queue(dir.path(), "fila", &[serde_json::json!({"id":"a1","text":"oi","ts":ts,"delivered":false})]);
        let lease = ctx.hubs.acquire("fila", queue_binding(dir.path(), "a"), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        queue_frame(&mut rx, "queued-a1", "\"queued_delivered\":false").await;
        write_queue(dir.path(), "fila", &[serde_json::json!({"id":"a1","text":"oi","ts":ts,"delivered":true})]);
        let frame = queue_frame(&mut rx, "queued-a1", "\"queued_delivered\":true").await;
        assert!(frame.starts_with("event: message\r\n"));
        let replay: String = lease.hub.cache.lock().unwrap().replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
        assert_eq!(replay.matches("queued-a1").count(), 1, "o retrato guarda a última de cada id");
        assert!(replay.contains("\"queued_delivered\":true"));
    }

    /// Bolha que saiu da projeção (a desistida que o reenvio apaga) sai do retrato: quem abre o chat
    /// depois recebe o que o arquivo tem agora, como uma conexão nova.
    #[tokio::test]
    async fn snapshot_forgets_entries_gone_from_the_projection() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), ..test_ctx() };
        let row = |id: &str| serde_json::json!({"id":id,"text":id,"ts":crate::queue_feed::wall_now(),"delivered":false});
        write_queue(dir.path(), "fila", &[row("a1"), row("a2")]);
        let lease = ctx.hubs.acquire("fila", queue_binding(dir.path(), "a"), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        queue_frame(&mut rx, "queued-a2", "queued_delivered").await;
        write_queue(dir.path(), "fila", &[row("a2"), row("a3")]);
        queue_frame(&mut rx, "queued-a3", "queued_delivered").await;
        let replay: String = lease.hub.cache.lock().unwrap().replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
        assert!(!replay.contains("queued-a1"), "saiu do arquivo, sai do retrato");
        assert!(replay.contains("queued-a2") && replay.contains("queued-a3"));
    }

    /// Hub aberto antes da primeira linha do transcript fica sem corte; o corte chega com ela, e a bolha
    /// da conversa anterior sai do retrato em vez de voltar confirmada.
    #[tokio::test]
    async fn queue_cut_arrives_with_the_first_transcript_line() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), ..test_ctx() };
        let now = crate::queue_feed::wall_now();
        let old = |confirmed: bool| serde_json::json!({"id":"old","text":"velha","ts":now - 600.0,"delivered":true,"confirmed":confirmed});
        write_queue(dir.path(), "fila", &[old(false)]);
        let binding = queue_binding(dir.path(), "a");
        let lease = ctx.hubs.acquire("fila", binding.clone(), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        queue_frame(&mut rx, "queued-old", "queued_delivered").await;
        let start = chrono::DateTime::from_timestamp((now - 10.0) as i64, 0).unwrap().to_rfc3339();
        std::fs::write(&binding.jsonl, format!("{}\n", serde_json::json!({"type":"user","timestamp":start,"message":{"role":"user","content":"oi"}}))).unwrap();
        write_queue(dir.path(), "fila", &[old(true), serde_json::json!({"id":"new","text":"nova","ts":now,"delivered":false})]);
        queue_frame(&mut rx, "queued-new", "queued_delivered").await;
        let replay: String = lease.hub.cache.lock().unwrap().replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
        assert!(!replay.contains("queued-old"), "bolha da conversa anterior ficou no retrato");
        assert!(replay.contains("queued-new"));
    }

    /// Dono que entra num hub só com convidados religa a conexão interna (`app=1`), mas não o produtor
    /// da fila: a fila não sai de novo, duplicada.
    #[tokio::test]
    async fn owner_joining_a_guest_hub_keeps_the_queue_feed() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), ..test_ctx() };
        let guest = ctx.hubs.acquire_as("fila", queue_binding(dir.path(), "a"), &ctx, true);
        let feed = |hub: &Hub| hub.queue_feed.lock().unwrap().as_ref().map(|f| f.id());
        let side = |hub: &Hub| hub.side.lock().unwrap().as_ref().map(|s| s.id());
        let (feed_before, side_before) = (feed(&guest.hub).expect("o produtor nasce com o convidado"), side(&guest.hub));
        let _owner = ctx.hubs.acquire("fila", queue_binding(dir.path(), "a"), &ctx);
        assert_ne!(side(&guest.hub), side_before, "a conexão interna religou com app=1");
        assert_eq!(feed(&guest.hub), Some(feed_before), "o produtor da fila segue o mesmo");
    }

    /// Problema de estado só sai depois de valer pelo prazo (o hub o reenvia sozinho quando vence); o que
    /// dura menos nunca sai, e resolvido some na hora. O estado em si sai sem espera.
    #[tokio::test]
    async fn a_state_problem_goes_out_only_after_it_held() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), problem_hold: crate::problem_gate::HOLD, ..test_ctx() };
        let lease = ctx.hubs.acquire("prob", queue_binding(dir.path(), "a"), &ctx);
        tokio::time::pause();
        let state = |hub: &Hub| -> serde_json::Value {
            let frames: String = hub.cache.lock().unwrap().replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect();
            frames.split("\n\n").find(|f| f.starts_with("event: state")).and_then(|f| f.split_once("data: "))
                .map(|(_, data)| serde_json::from_str(data.trim()).unwrap()).unwrap_or_default()
        };
        let with = |problema: Option<&str>| serde_json::json!({"state":"working","problema":problema,"problema_detalhe":problema.map(|p| format!("{p}: detalhe"))}).to_string();
        assert!(lease.hub.publish_own("state", &with(Some("headless_processo_caiu"))));
        assert_eq!((state(&lease.hub)["state"].as_str(), state(&lease.hub)["problema"].as_str()), (Some("working"), None), "estado já, problema não");
        // O temporizador do tokio arredonda o prazo para o milissegundo seguinte.
        tokio::time::advance(crate::problem_gate::HOLD + std::time::Duration::from_millis(5)).await;
        for _ in 0..5 { tokio::task::yield_now().await; }
        assert_eq!(state(&lease.hub)["problema"], "headless_processo_caiu", "valeu pelo prazo: sai sozinho");
        assert!(lease.hub.publish_own("state", &with(None)));
        assert!(state(&lease.hub)["problema"].is_null(), "resolvido some na hora");
        assert!(lease.hub.publish_own("state", &with(Some("passageiro"))));
        tokio::time::advance(std::time::Duration::from_secs(1)).await;
        assert!(lease.hub.publish_own("state", &with(None)));
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        for _ in 0..5 { tokio::task::yield_now().await; }
        assert!(state(&lease.hub)["problema"].is_null(), "o passageiro nunca saiu");
        // Retido na conversa anterior: a troca o descarta, e o prazo vencido não o põe de volta.
        assert!(lease.hub.publish_own("state", &with(Some("da_anterior"))));
        lease.hub.rebind(queue_binding(dir.path(), "b"));
        tokio::time::advance(crate::problem_gate::HOLD + std::time::Duration::from_secs(1)).await;
        for _ in 0..5 { tokio::task::yield_now().await; }
        assert_ne!(state(&lease.hub)["problema"], "da_anterior", "o estado da conversa anterior não volta");
    }

    /// A conversa aberta depois começa do problema que a lista já mostra: o mesmo sai na hora, sem esperar.
    #[tokio::test]
    async fn a_hub_opens_with_the_problem_the_list_already_shows() {
        let dir = tempfile::tempdir().unwrap();
        let shown = Arc::new(crate::problem_gate::Shown::default());
        shown.set("seed", Some("headless_processo_caiu"));
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), problem_hold: crate::problem_gate::HOLD, shown_problems: shown, ..test_ctx() };
        let lease = ctx.hubs.acquire("seed", queue_binding(dir.path(), "a"), &ctx);
        let frames = |hub: &Hub| -> String { hub.cache.lock().unwrap().replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect() };
        assert!(lease.hub.publish_own("state", r#"{"state":"idle","problema":"headless_processo_caiu","problema_detalhe":"rc=143"}"#));
        assert!(frames(&lease.hub).contains(r#""problema":"headless_processo_caiu""#), "a lista já mostra: a conversa também, sem prazo");
        assert!(lease.hub.publish_own("state", r#"{"state":"idle","problema":"outro","problema_detalhe":null}"#));
        assert!(!frames(&lease.hub).contains(r#""problema":"outro""#), "problema diferente espera o prazo");
    }

    /// Os quadros guardados no hub, em texto.
    fn replay_text(hub: &Hub) -> String { hub.cache.lock().unwrap().replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect() }

    /// O `state` guardado no hub (vazio sem nenhum).
    fn replayed_state(hub: &Hub) -> serde_json::Value {
        replay_text(hub).split("\n\n").find(|f| f.starts_with("event: state")).and_then(|f| f.split_once("data: "))
            .map(|(_, data)| serde_json::from_str(data.trim()).unwrap()).unwrap_or_default()
    }

    /// Aviso de diagnóstico (fatos velhos, Codex sem conexão, controle do Codex caído) sai do estado na hora e o
    /// prazo não o traz de volta; o de tela (troca de conta sem destino, mensagem presa) espera o prazo e sai.
    #[tokio::test]
    async fn a_diagnostic_problem_never_goes_out_and_a_screen_problem_still_does() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), problem_hold: crate::problem_gate::HOLD, ..test_ctx() };
        let lease = ctx.hubs.acquire("diagnostico", queue_binding(dir.path(), "a"), &ctx);
        tokio::time::pause();
        let state = replayed_state;
        let with = |problema: &str| serde_json::json!({"state":"working","problema":problema,"problema_detalhe":"detalhe"}).to_string();
        let past_hold = || async {
            tokio::time::advance(crate::problem_gate::HOLD + std::time::Duration::from_millis(5)).await;
            for _ in 0..5 { tokio::task::yield_now().await; }
        };
        for code in ["state_facts_unavailable", "codex_sem_conexao", "codex_control_lost"] {
            assert!(lease.hub.publish_own("state", &with(code)));
            let now = state(&lease.hub);
            assert_eq!((now["state"].as_str(), now["problema"].as_str(), now["problema_detalhe"].as_str()), (Some("working"), None, None), "{code}: fora da tela já");
            past_hold().await;
            assert!(state(&lease.hub)["problema"].is_null(), "{code}: o prazo não o traz");
        }
        for code in ["limit_switch_no_target", "headless_delivery_stalled"] {
            assert!(lease.hub.publish_own("state", &with(code)));
            assert!(state(&lease.hub)["problema"].is_null(), "{code}: espera o prazo, como hoje");
            past_hold().await;
            assert_eq!(state(&lease.hub)["problema"], code, "{code}: de tela, sai depois do prazo");
        }
    }

    /// P1 na conversa: pergunta aberta (`awaiting_input`) cuja interação não tem identidade é bloqueio e sai com o
    /// código da lista; fora da espera, ou fatos velhos por outra causa, ficam no diário.
    #[tokio::test]
    async fn an_unidentified_open_question_shows_only_while_awaiting() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), ..test_ctx() };
        let lease = ctx.hubs.acquire("p1", queue_binding(dir.path(), "a"), &ctx);
        let problem = |state: &str, detail: &str| -> serde_json::Value {
            let data = serde_json::json!({"state":state,"problema":"state_facts_unavailable","problema_detalhe":detail}).to_string();
            assert!(lease.hub.publish_own("state", &data));
            replayed_state(&lease.hub)["problema"].clone()
        };
        assert_eq!(problem("awaiting_input", "askq_pane_mismatch"), "interacao_indisponivel", "card sem como responder: tela");
        assert_eq!(problem("awaiting_input", "runtime_binding"), "interacao_indisponivel");
        assert!(problem("idle", "askq_pane_mismatch").is_null(), "fora da espera: diário");
        assert!(problem("awaiting_input", "state_files_timeout").is_null(), "fatos velhos por outra causa: diário");
        // Cartão identificado (seletor ou formulário com ID) responde: os fatos do plugin caídos ficam no diário.
        let identified = |extra: serde_json::Value| -> serde_json::Value {
            let mut data = serde_json::json!({"state":"awaiting_input","problema":"state_facts_unavailable","problema_detalhe":"plugin_store_unavailable"});
            data.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            assert!(lease.hub.publish_own("state", &data.to_string()));
            replayed_state(&lease.hub)["problema"].clone()
        };
        assert!(identified(serde_json::json!({"interaction_id":"sel-1"})).is_null(), "seletor com ID");
        assert!(identified(serde_json::json!({"codex_question":{"interaction_id":"form-1","questions":[]}})).is_null(), "formulário com ID");
        assert_eq!(identified(serde_json::json!({"interaction_id":null,"codex_question":null})), "interacao_indisponivel", "sem ID nenhum");
    }

    /// Com o prazo, o que sai quando ele vence é o código traduzido, nunca o `state_facts_unavailable` original.
    #[tokio::test]
    async fn an_unidentified_open_question_after_the_hold_goes_out_translated() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), problem_hold: crate::problem_gate::HOLD, ..test_ctx() };
        let lease = ctx.hubs.acquire("p1-prazo", queue_binding(dir.path(), "a"), &ctx);
        tokio::time::pause();
        assert!(lease.hub.publish_own("state", r#"{"state":"awaiting_input","problema":"state_facts_unavailable","problema_detalhe":"askq_pane_mismatch"}"#));
        tokio::time::advance(crate::problem_gate::HOLD + std::time::Duration::from_millis(5)).await;
        for _ in 0..5 { tokio::task::yield_now().await; }
        let frames = replay_text(&lease.hub);
        assert!(frames.contains(r#""problema":"interacao_indisponivel""#), "{frames}");
        assert!(!frames.contains("state_facts_unavailable") && !frames.contains("askq_pane_mismatch"), "{frames}");
    }

    /// "Estado talvez velho" (`state_facts_unavailable`, detalhe `runtime_binding`) não sai na conversa e vai ao diário.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn stale_facts_leave_the_state_and_go_to_the_diary() {
        let (sink, reports) = crate::runtime::gateway::tests::diag_sink().await;
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), problem_hold: crate::problem_gate::HOLD,
            diag: crate::diag::DiagClient::new(sink, "s".into()), ..test_ctx() };
        let lease = ctx.hubs.acquire("velho", queue_binding(dir.path(), "a"), &ctx);
        assert!(lease.hub.publish_own("state", r#"{"state":"idle","problema":"state_facts_unavailable","problema_detalhe":"runtime_binding"}"#));
        let frames = replay_text(&lease.hub);
        assert!(frames.contains("event: state"), "o estado sai");
        assert!(!frames.contains("state_facts_unavailable") && !frames.contains("runtime_binding"), "sem o aviso: {frames}");
        assert!(lease.hub.problems.lock().unwrap().timer.is_none(), "nada retido para sair depois do prazo");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !reports.lock().unwrap().iter().any(|r| r["evento"] == "rust.problem_hidden" && r["codigo"] == "state_facts_unavailable" && r["sessao"] == "velho") {
            assert!(std::time::Instant::now() < deadline, "o aviso tirado da tela não foi ao diário: {:?}", reports.lock().unwrap());
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    /// Troca de conversa: o retrato perde a fila da anterior na hora, quadro da geração anterior não
    /// entra mais, e a geração nova relê a fila.
    #[tokio::test]
    async fn rebind_clears_the_queue_and_drops_old_generation_frames() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = SideCtx { queue_dir: Some(dir.path().to_path_buf()), ..test_ctx() };
        write_queue(dir.path(), "fila", &[serde_json::json!({"id":"a1","text":"oi","ts":crate::queue_feed::wall_now(),"delivered":false})]);
        let lease = ctx.hubs.acquire("fila", queue_binding(dir.path(), "a"), &ctx);
        let mut rx = lease.hub.tx.subscribe();
        queue_frame(&mut rx, "queued-a1", "queued_delivered").await;
        let old = lease.hub.generation().unwrap();
        lease.hub.rebind(queue_binding(dir.path(), "b"));
        let replay = |hub: &Hub| -> String { hub.cache.lock().unwrap().replay().iter().map(|b| String::from_utf8_lossy(b).into_owned()).collect() };
        assert!(!replay(&lease.hub).contains("queued-a1"), "a troca esvazia a fila do retrato");
        let stale = crate::queue_feed::QueueFrame { event: "message", data: "{\"id\":\"queued-x\"}".into(), id: "queued-x".into(), local_ts: None };
        assert!(!lease.hub.publish_queue(old, &stale));
        assert!(!replay(&lease.hub).contains("queued-x"));
        queue_frame(&mut rx, "queued-a1", "queued_delivered").await;
        assert!(replay(&lease.hub).contains("queued-a1"), "a geração nova relê a fila");
    }
}
