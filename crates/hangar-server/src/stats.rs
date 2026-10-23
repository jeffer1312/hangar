//! Faixa de estatísticas da conversa aberta (evento `stats`), porte das contas de Claude e Codex de
//! backend/app/stats.py. Lê o mesmo arquivo do transcript, incrementalmente: guarda o ponto de leitura
//! e a cada coleta dobra só as linhas novas. Os tempos e velocidades são aproximados por construção (o
//! cliente prefixa "~"); a medida exata do stream entra por cima (`RateStore`). O esperado vem de rodar o
//! Python (`backend/tests/fixtures/contract/gen_stats.py`).
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufRead, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex, Weak};
use std::time::Duration;

use indexmap::IndexMap;
use serde_json::{Map, Value, json};

use crate::side::{Binding, Hub};
use crate::transcript::{self, Provider};

/// Chamadas que entram na média "recente" da velocidade (`live_rate.RECENT_CALLS`).
pub const RECENT_CALLS: usize = 10;
/// Resposta mais curta que isto é ruído de relógio, não velocidade (`live_rate.MIN_GEN_S`).
pub const MIN_GEN_S: f64 = 0.2;
/// Gap acima disto não é trabalho, é sessão parada.
const GAP_MAX_S: f64 = 3600.0;
/// Ferramentas sem resultado guardadas; cheio, sai a mais velha (a que nunca fecha não trava a conta).
const TOOLS_PENDING_MAX: usize = 200;
/// Acumuladores soltos guardados para a reabertura não reler o transcript inteiro.
const INACTIVE_MAX: usize = 16;
/// Ritmo da faixa: o tok/s "agora" é ao vivo, e a medida exata chega fora do arquivo.
const TICK: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Claude,
    Codex,
}

/// Resposta mais nova do Claude: tokens, início, fim (relógio de parede) e o id da mensagem.
struct Pending {
    tokens: i64,
    start: f64,
    end: f64,
    message: Value,
}

struct Fold {
    kind: Kind,
    turns: i64,
    steps: i64,
    llm_ms: f64,
    tool_ms: f64,
    in_tok: i64,
    out_tok: i64,
    /// Saída sem tempo de LLM na conta (subagente).
    out_untimed: i64,
    calls: VecDeque<(i64, f64)>,
    pend: Option<Pending>,
    cache_read: i64,
    ttft_ms: f64,
    ttft_n: i64,
    prev_ts: Option<f64>,
    prompt_ts: Option<f64>,
    tools: IndexMap<String, f64>,
    /// Mensagens do Claude já contadas: um bloco dela gravado depois de outra linha não conta de novo.
    seen: HashSet<String>,
    codex_total: Option<(i64, i64, i64)>,
    codex_call_llm_ms: f64,
}

impl Fold {
    fn new(kind: Kind) -> Self {
        Fold {
            kind, turns: 0, steps: 0, llm_ms: 0.0, tool_ms: 0.0, in_tok: 0, out_tok: 0, out_untimed: 0,
            calls: VecDeque::with_capacity(RECENT_CALLS), pend: None, cache_read: 0, ttft_ms: 0.0, ttft_n: 0,
            prev_ts: None, prompt_ts: None, tools: IndexMap::new(), seen: HashSet::new(), codex_total: None,
            codex_call_llm_ms: 0.0,
        }
    }

    fn feed(&mut self, obj: &Map<String, Value>) {
        match self.kind {
            Kind::Claude => self.feed_claude(obj),
            Kind::Codex => self.feed_codex(obj),
        }
    }

    /// Gap em segundos desde a linha anterior, e avança o cursor. Linha sem horário não move; horário
    /// que volta para trás não conta nem move o relógio para trás.
    fn gap(&mut self, ts: Option<f64>) -> Option<f64> {
        let ts = ts?;
        if self.prev_ts.is_some_and(|prev| ts < prev) {
            return None;
        }
        let prev = self.prev_ts.replace(ts)?;
        (ts - prev <= GAP_MAX_S).then_some(ts - prev)
    }

    fn llm(&mut self, ts: Option<f64>) {
        if let Some(gap) = self.gap(ts) {
            self.llm_ms += gap * 1000.0;
        }
    }

    fn tool_start(&mut self, id: Option<&str>, ts: Option<f64>) {
        let (Some(id), Some(ts)) = (id.filter(|id| !id.is_empty()), ts) else { return };
        if self.tools.len() >= TOOLS_PENDING_MAX && !self.tools.contains_key(id) {
            self.tools.shift_remove_index(0);
        }
        self.tools.insert(id.to_owned(), ts);
    }

    fn tool_end(&mut self, id: Option<&str>, ts: Option<f64>) {
        let Some(t0) = id.filter(|id| !id.is_empty()).and_then(|id| self.tools.shift_remove(id)) else { return };
        if let Some(ts) = ts.filter(|ts| (0.0..=GAP_MAX_S).contains(&(ts - t0))) {
            self.tool_ms += (ts - t0) * 1000.0;
        }
    }

    fn pend_call(&self) -> Option<(i64, f64)> {
        self.pend.as_ref().filter(|p| p.end - p.start >= MIN_GEN_S).map(|p| (p.tokens, p.end - p.start))
    }

    fn push_call(&mut self, call: (i64, f64)) {
        if self.calls.len() == RECENT_CALLS {
            self.calls.pop_front();
        }
        self.calls.push_back(call);
    }

    fn close_call(&mut self) {
        if let Some(call) = self.pend_call() {
            self.push_call(call);
        }
        self.pend = None;
    }

    /// Fim da resposta mais nova no transcript (relógio de parede).
    fn last_call_ts(&self) -> Option<f64> {
        self.pend.as_ref().map(|p| p.end)
    }

    fn ttft(&mut self, ts: Option<f64>) {
        if let (Some(prompt), Some(ts)) = (self.prompt_ts, ts) {
            if (0.0..=GAP_MAX_S).contains(&(ts - prompt)) {
                self.ttft_ms += (ts - prompt) * 1000.0;
                self.ttft_n += 1;
            }
        }
        self.prompt_ts = None;
    }

    fn human_turn(&mut self, ts: Option<f64>) {
        self.turns += 1;
        self.prompt_ts = ts;
    }

    fn snapshot(&self) -> Option<Map<String, Value>> {
        if self.steps == 0 {
            return None;
        }
        let mut out = Map::new();
        out.insert("turns".into(), self.turns.into());
        out.insert("steps".into(), self.steps.into());
        out.insert("in_tok".into(), self.in_tok.into());
        out.insert("out_tok".into(), self.out_tok.into());
        if self.llm_ms > 0.0 {
            out.insert("llm_ms".into(), (self.llm_ms as i64).into());
            let timed = self.out_tok.saturating_sub(self.out_untimed);
            if timed > 0 {
                out.insert("tok_s".into(), round1(timed as f64 / (self.llm_ms / 1000.0)));
            }
        }
        // Reserva sem stream: o tempo de cada chamada inclui a espera pelo primeiro token.
        let mut calls: Vec<(i64, f64)> = self.calls.iter().copied().collect();
        if let Some(pend) = self.pend_call() {
            calls.push(pend);
            if calls.len() > RECENT_CALLS {
                calls.remove(0);
            }
        }
        out.extend(rates(&calls));
        if self.tool_ms > 0.0 {
            out.insert("tool_ms".into(), (self.tool_ms as i64).into());
        }
        if self.in_tok > 0 {
            out.insert("cache_pct".into(), ((100.0 * self.cache_read as f64 / self.in_tok as f64).round_ties_even() as i64).into());
            out.insert("cache_read_tok".into(), self.cache_read.into());
        }
        if self.ttft_n > 0 {
            out.insert("ttft_ms".into(), ((self.ttft_ms / self.ttft_n as f64) as i64).into());
        }
        Some(out)
    }

    fn feed_claude(&mut self, obj: &Map<String, Value>) {
        let kind = obj.get("type").and_then(Value::as_str);
        if !matches!(kind, Some("user" | "assistant")) {
            return;
        }
        let ts = iso_ts(obj.get("timestamp"));
        let sidechain = truthy(obj.get("isSidechain"));
        if truthy(obj.get("isMeta")) {
            // Recado de outra sessão chega como meta: não é turno, mas o relógio parte dele.
            if kind == Some("user") && !sidechain {
                self.gap(ts);
            }
            return;
        }
        let empty = Map::new();
        let msg = obj.get("message").and_then(Value::as_object).unwrap_or(&empty);
        if kind == Some("assistant") {
            let mid = msg.get("id").filter(|id| truthy(Some(id)));
            let mut out = 0;
            if let (Some(usage), Some(mid)) = (msg.get("usage").and_then(Value::as_object), mid) {
                if self.seen.insert(mid.to_string()) {
                    self.steps += 1;
                    let (input, read, write) = (int(usage.get("input_tokens")), int(usage.get("cache_read_input_tokens")),
                        int(usage.get("cache_creation_input_tokens")));
                    // Saturando: o inteiro do Python não tem limite, e um transcript torto não derruba a conta.
                    self.in_tok = self.in_tok.saturating_add(input).saturating_add(read).saturating_add(write);
                    self.cache_read = self.cache_read.saturating_add(read);
                    out = int(usage.get("output_tokens"));
                    self.out_tok = self.out_tok.saturating_add(out);
                    if sidechain {
                        self.out_untimed = self.out_untimed.saturating_add(out);
                    }
                }
            }
            if !sidechain {
                let t0 = self.prev_ts;
                let gap = self.gap(ts);
                if let Some(gap) = gap {
                    self.llm_ms += gap * 1000.0;
                }
                // Cada bloco da resposta vira uma linha quando ele termina: a chamada vai do cursor antes
                // da 1ª linha até a última linha da mesma mensagem, mesmo com resultado de ferramenta no meio.
                if out != 0 {
                    self.close_call();
                    if let (Some(_), Some(start), Some(end)) = (gap, t0, ts) {
                        self.pend = Some(Pending { tokens: out, start, end, message: mid.cloned().unwrap_or(Value::Null) });
                    }
                } else if let (Some(pend), Some(ts)) = (self.pend.as_mut(), ts) {
                    if Some(&pend.message) == msg.get("id") {
                        pend.end = ts;
                    }
                }
                self.ttft(ts);
                for block in msg.get("content").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object) {
                    if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                        self.tool_start(block.get("id").and_then(Value::as_str), ts);
                    }
                }
            }
            return;
        }
        if sidechain {
            return;
        }
        let content = msg.get("content");
        let blocks: Vec<&Map<String, Value>> = content.and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_object).collect();
        let results: Vec<_> = blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("tool_result")).collect();
        if !results.is_empty() {
            for block in results {
                self.tool_end(block.get("tool_use_id").and_then(Value::as_str), ts);
            }
            // Espera de ferramenta, não LLM: só avança o cursor.
            self.gap(ts);
            return;
        }
        let text = match content {
            Some(Value::String(text)) => text.clone(),
            _ => blocks.iter().filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|b| b.get("text").and_then(Value::as_str)).filter(|t| !t.is_empty()).collect::<Vec<_>>().join("\n"),
        };
        // Turno humano = sobra texto depois de tirar o sintético (eco de /comando, aviso de tarefa, lembrete).
        if !text.is_empty() && !transcript::is_command_meta(&text) && !transcript::strip_meta_blocks(&text).is_empty() {
            self.human_turn(ts);
        }
        // A linha sintética também avança o cursor: a resposta seguinte não engole a espera como LLM.
        self.gap(ts);
    }

    fn codex_usage(&mut self, info: &Map<String, Value>) {
        let Some(total) = info.get("total_token_usage").and_then(Value::as_object) else { return };
        let now = (int(total.get("input_tokens")), int(total.get("cached_input_tokens")), int(total.get("output_tokens")));
        let before = self.codex_total.replace(now).unwrap_or((0, 0, 0));
        // O delta do acumulado, nunca o total: reset (compactação) vira zero em vez de subtrair.
        let d = |a: i64, b: i64| a.saturating_sub(b).max(0);
        let delta = (d(now.0, before.0), d(now.1, before.1), d(now.2, before.2));
        if delta.0 == 0 && delta.2 == 0 {
            return;
        }
        self.steps += 1;
        self.in_tok = self.in_tok.saturating_add(delta.0);
        self.cache_read = self.cache_read.saturating_add(delta.1);
        self.out_tok = self.out_tok.saturating_add(delta.2);
        let seconds = (self.llm_ms - self.codex_call_llm_ms) / 1000.0;
        self.codex_call_llm_ms = self.llm_ms;
        if delta.2 > 0 && seconds >= MIN_GEN_S {
            self.push_call((delta.2, seconds));
        }
    }

    fn feed_codex(&mut self, obj: &Map<String, Value>) {
        let Some(payload) = obj.get("payload").and_then(Value::as_object) else { return };
        let ts = iso_ts(obj.get("timestamp"));
        let kind = payload.get("type").and_then(Value::as_str);
        match obj.get("type").and_then(Value::as_str) {
            Some("event_msg") => {
                if kind == Some("token_count") {
                    if let Some(info) = payload.get("info").and_then(Value::as_object) {
                        self.codex_usage(info);
                    }
                }
                return;
            }
            Some("response_item") => {}
            _ => return,
        }
        match kind {
            Some("message") => match payload.get("role").and_then(Value::as_str) {
                Some("assistant") => {
                    self.llm(ts);
                    self.ttft(ts);
                }
                Some("user") => {
                    let text: String = payload.get("content").and_then(Value::as_array).into_iter().flatten()
                        .filter_map(Value::as_object).filter_map(|b| b.get("text").and_then(Value::as_str)).collect();
                    if !transcript::is_context_wrapper(&text) {
                        self.human_turn(ts);
                    }
                    self.gap(ts);
                }
                _ => {}
            },
            Some("function_call" | "custom_tool_call") => {
                self.llm(ts);
                self.tool_start(payload.get("call_id").and_then(Value::as_str), ts);
            }
            Some("function_call_output" | "custom_tool_call_output") => {
                self.tool_end(payload.get("call_id").and_then(Value::as_str), ts);
                self.gap(ts);
            }
            Some("reasoning") => self.llm(ts),
            _ => {}
        }
    }
}

/// `round(x, 1)` do Python: o arredondamento correto do valor binário exato, como o `{:.1}`.
fn round1(value: f64) -> Value {
    format!("{value:.1}").parse::<f64>().map_or(Value::Null, Value::from)
}

/// Medida do stream que o ator publica no canal `rate`.
#[derive(Debug, PartialEq)]
pub enum Measure {
    Rate { tokens: u64, seconds: f64, conversation: String },
    /// Do início do turno ao 1º texto da resposta (Codex).
    FirstResponse { seconds: f64, conversation: String },
}

impl Measure {
    /// `rate_report`/`first_response_report` do Python: segundos finitos e não negativos, conversa não
    /// vazia, tokens inteiros (nunca bool); `None` é medida malformada.
    pub fn parse(data: &Value) -> Option<Measure> {
        let seconds = data["seconds"].as_f64().filter(|s| s.is_finite() && *s >= 0.0)?;
        let conversation = data["conversation"].as_str().filter(|c| !c.is_empty())?.to_owned();
        if data["first_response"] == true {
            return Some(Measure::FirstResponse { seconds, conversation });
        }
        // Inteiro negativo passava no Python e caía junto com o zero: aqui vira zero, que não é guardado.
        let tokens = data["tokens"].as_i64().map(|t| u64::try_from(t).unwrap_or(0)).or_else(|| data["tokens"].as_u64())?;
        Some(Measure::Rate { tokens, seconds, conversation })
    }
}

/// `tok_s_now`: a última resposta; `tok_s_recent`: as últimas chamadas somadas (`live_rate.rates`).
pub fn rates(calls: &[(i64, f64)]) -> Map<String, Value> {
    let mut out = Map::new();
    let Some(last) = calls.last() else { return out };
    out.insert("tok_s_now".into(), round1(last.0 as f64 / last.1));
    let tokens: f64 = calls.iter().map(|c| c.0 as f64).sum();
    let seconds: f64 = calls.iter().map(|c| c.1).sum();
    out.insert("tok_s_recent".into(), round1(tokens / seconds));
    out
}

fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|n| n != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// `_int`: só inteiro de verdade conta; bool, texto e número com fração valem 0.
fn int(value: Option<&Value>) -> i64 {
    match value {
        Some(Value::Number(n)) if n.is_i64() || n.is_u64() => n.as_i64().unwrap_or(i64::MAX),
        _ => 0,
    }
}

fn iso_ts(value: Option<&Value>) -> Option<f64> {
    transcript::ts_of_iso(value?.as_str()?)
}

/// Identidade do arquivo: (dispositivo, índice). No Windows vem do handle, como o `st_ino` do Python.
fn identity(file: &std::fs::File, metadata: &std::fs::Metadata) -> io::Result<(u64, u64)> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        Ok((metadata.dev(), metadata.ino()))
    }
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::Storage::FileSystem::{BY_HANDLE_FILE_INFORMATION, GetFileInformationByHandle};
        let _ = metadata;
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((u64::from(info.dwVolumeSerialNumber), (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow)))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (file, metadata);
        Ok((0, 0))
    }
}

/// Dobra incremental sobre o arquivo da conversa. `collect` faz IO e roda fora do runtime assíncrono.
pub struct Accumulator {
    path: PathBuf,
    kind: Kind,
    fold: Fold,
    offset: u64,
    identity: Option<(u64, u64)>,
    modified: Option<std::time::SystemTime>,
    /// Linha parcial no fim do arquivo (escrita em andamento).
    rest: Vec<u8>,
}

impl Accumulator {
    fn new(provider: Provider, path: PathBuf) -> Self {
        let kind = if provider == Provider::Codex { Kind::Codex } else { Kind::Claude };
        Accumulator { path, kind, fold: Fold::new(kind), offset: 0, identity: None, modified: None, rest: Vec::new() }
    }

    /// Arquivo ainda inexistente devolve o último retrato; outra falha de leitura sobe.
    pub fn collect(&mut self) -> io::Result<Option<Map<String, Value>>> {
        let mut file = match std::fs::File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(self.fold.snapshot()),
            Err(e) => return Err(e),
        };
        let metadata = file.metadata()?;
        let (size, id, modified) = (metadata.len(), identity(&file, &metadata)?, metadata.modified().ok());
        let swapped = self.identity.is_some_and(|old| old != id);
        let shrunk = size < self.offset;
        let rewritten = size == self.offset && self.modified.is_some() && modified != self.modified;
        if swapped || shrunk || rewritten {
            self.fold = Fold::new(self.kind);
            self.offset = 0;
            self.rest.clear();
        }
        self.identity = Some(id);
        self.modified = modified;
        if size > self.offset {
            file.seek(SeekFrom::Start(self.offset))?;
            let mut reader = io::BufReader::new(file.take(size - self.offset));
            let mut line = std::mem::take(&mut self.rest);
            loop {
                let read = reader.read_until(b'\n', &mut line)?;
                if read == 0 {
                    break;
                }
                self.offset += read as u64;
                if line.last() != Some(&b'\n') {
                    break;
                }
                if let Some(Value::Object(obj)) = transcript::decode_line(&line[..line.len() - 1]) {
                    self.fold.feed(&obj);
                }
                line.clear();
            }
            // Escrita em andamento: completa na próxima coleta, sem reler os bytes.
            self.rest = line;
        }
        Ok(self.fold.snapshot())
    }

    pub fn last_call_ts(&self) -> Option<f64> {
        self.fold.last_call_ts()
    }
}

type Key = (Provider, PathBuf);

/// Um acumulador por (provider, caminho), com quem o segura; os soltos mais recentes ficam para a
/// reabertura (segundo aparelho, religação) não reler o arquivo.
#[derive(Default)]
struct Shared {
    held: HashMap<Key, (Arc<Mutex<Accumulator>>, usize)>,
    inactive: IndexMap<Key, ()>,
}

static SHARED: LazyLock<Mutex<Shared>> = LazyLock::new(Mutex::default);

fn normalized(path: &Path) -> PathBuf {
    path.components().collect()
}

/// Posse de um acumulador compartilhado; ao cair, ele vai para os soltos.
pub struct Lease {
    key: Key,
    pub acc: Arc<Mutex<Accumulator>>,
}

pub fn acquire(provider: Provider, path: &Path) -> Lease {
    let key = (provider, normalized(path));
    let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
    shared.inactive.shift_remove(&key);
    let entry = shared.held.entry(key.clone()).or_insert_with(|| (Arc::new(Mutex::new(Accumulator::new(provider, key.1.clone()))), 0));
    entry.1 += 1;
    Lease { acc: entry.0.clone(), key }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let mut shared = SHARED.lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = shared.held.get_mut(&self.key) else { return };
        entry.1 = entry.1.saturating_sub(1);
        if entry.1 > 0 {
            return;
        }
        shared.inactive.insert(self.key.clone(), ());
        while shared.inactive.len() > INACTIVE_MAX {
            let Some((old, ())) = shared.inactive.shift_remove_index(0) else { break };
            shared.held.remove(&old);
        }
    }
}

/// Mescla a medida exata da vida atual (`RateStore`) no retrato do transcript. Sem vínculo confirmado,
/// sai só a reserva do transcript.
async fn overlay_exact(hub: &Hub, binding: &Binding, snapshot: &mut Map<String, Value>, last_call_ts: Option<f64>) {
    let Some(runtime) = hub.runtime_registry() else { return };
    match runtime.interaction_binding_readonly(&hub.name).await {
        Ok(life) if life.conversation == binding.key => snapshot.extend(hub.plugin_rates().snapshot(&life, last_call_ts)),
        // Fora do Claude com terminal, o aviso ficaria ligado a cada coleta: só a reserva, calado.
        _ if binding.provider != Provider::Claude => {}
        _ => {
            if crate::warn_limit::allow(Some(&hub.name), "plugin_rate_binding_unavailable") {
                tracing::warn!(session = %hub.name, code = "plugin_rate_binding_unavailable", "taxa: vínculo não confirmado; mantém os demais dados");
            }
        }
    }
}

/// Produtor do `stats` de uma ligação: um por hub e por geração, como o da fila. Publica só quando o
/// retrato muda; um retrato que some depois de publicado sai como `null` (a conta recomeçou vazia).
pub(crate) async fn run(hub: Weak<Hub>, generation: u64, binding: Binding, watchers: crate::tail::Watchers) {
    let lease = acquire(binding.provider, &binding.jsonl);
    let mut sub = crate::tail::Subscription::new(watchers, binding.jsonl.clone(), Arc::new(tokio::sync::Notify::new()));
    let expected = (generation, binding.clone());
    let mut last: Option<String> = None;
    loop {
        sub.arm();
        let acc = lease.acc.clone();
        let collected = tokio::task::spawn_blocking(move || {
            let mut acc = acc.lock().unwrap_or_else(|e| e.into_inner());
            acc.collect().map(|snapshot| (snapshot, acc.last_call_ts()))
        })
        .await;
        let Some(h) = hub.upgrade() else { return };
        let (snapshot, last_call_ts) = match collected {
            Ok(Ok(read)) => read,
            Ok(Err(e)) => {
                tracing::warn!(session = %h.name, code = "stats_read_failed", kind = ?e.kind(), "faixa: transcript ilegível; desligada");
                h.diag().report("rust.stats_failed", &h.name, "stats_read_failed", "transcript ilegível; faixa desligada");
                return;
            }
            Err(e) => {
                tracing::warn!(session = %h.name, code = "stats_job_failed", panic = e.is_panic(), "faixa: leitura caiu; desligada");
                h.diag().report("rust.stats_failed", &h.name, "stats_job_failed", "leitura caiu; faixa desligada");
                return;
            }
        };
        let data = match snapshot {
            Some(mut snapshot) => {
                overlay_exact(&h, &binding, &mut snapshot, last_call_ts).await;
                Some(Value::Object(snapshot).to_string())
            }
            None => last.is_some().then(|| json!(null).to_string()),
        };
        if let Some(data) = data.filter(|data| last.as_ref() != Some(data)) {
            if !h.publish_own_bound(&expected, "stats", &data) {
                return;
            }
            last = Some(data);
        }
        drop(h);
        tokio::select! {
            _ = sub.wake.notified() => {}
            _ = tokio::time::sleep(TICK) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    const CASES: &str = include_str!("../../../backend/tests/fixtures/contract/stats/cases.json");

    fn apply(path: &Path, step: &Value) {
        let data = step["data"].as_str().unwrap_or("");
        match step["op"].as_str() {
            Some("append") => std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap().write_all(data.as_bytes()).unwrap(),
            Some("write") => std::fs::write(path, data).unwrap(),
            Some("replace") => {
                let other = path.with_file_name("substituto.jsonl");
                std::fs::write(&other, data).unwrap();
                std::fs::rename(&other, path).unwrap();
            }
            _ => {}
        }
    }

    fn provider(name: &str) -> Provider {
        Provider::parse(name).unwrap()
    }

    /// ST-01..ST-22, ST-44, ST-45: o retrato depois de cada passo é o do Python, campo a campo, nos
    /// formatos do `test_stats.py` e nos transcripts versionados (inteiros e em pedaços).
    #[test]
    fn every_recorded_step_matches_the_python_accumulator() {
        let fixture: Value = serde_json::from_str(CASES).unwrap();
        let mut diffs = Vec::new();
        for case in fixture["cases"].as_array().unwrap() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("s.jsonl");
            let mut acc = Accumulator::new(provider(case["provider"].as_str().unwrap()), path.clone());
            for (i, step) in case["steps"].as_array().unwrap().iter().enumerate() {
                apply(&path, step);
                let got = acc.collect().unwrap().map(Value::Object).unwrap_or(Value::Null);
                if got != step["snapshot"] {
                    diffs.push(format!("{} passo {i}: rust {got} × python {}", case["name"], step["snapshot"]));
                }
                let ts = acc.last_call_ts().map(Value::from).unwrap_or(Value::Null);
                if ts != step["last_call_ts"] {
                    diffs.push(format!("{} passo {i}: last_call_ts rust {ts} × python {}", case["name"], step["last_call_ts"]));
                }
            }
        }
        assert!(diffs.is_empty(), "{} diferenças:\n{}", diffs.len(), diffs.join("\n"));
    }

    fn claude_line(ts: &str, kind: &str, extra: Value) -> String {
        let mut line = json!({"type": kind, "timestamp": ts});
        line.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        format!("{line}\n")
    }

    fn assistant(ts: &str, mid: &str, out: i64) -> String {
        claude_line(ts, "assistant", json!({"message": {"id": mid, "content": [{"type": "text", "text": "r"}],
            "usage": {"input_tokens": 10, "output_tokens": out}}}))
    }

    fn collect(lines: &[String]) -> Map<String, Value> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, lines.concat()).unwrap();
        Accumulator::new(Provider::Claude, path).collect().unwrap().unwrap()
    }

    /// S2 (corrigir): o bloco de uma mensagem gravado depois de uma linha de subagente não conta de novo.
    #[test]
    fn message_usage_counts_once_even_with_a_subagent_line_between_its_blocks() {
        let mut side = json!({"isSidechain": true, "message": {"id": "sub", "usage": {"output_tokens": 7}}});
        let lines = [
            claude_line("2026-10-23T12:00:00Z", "user", json!({"message": {"content": "pedido"}})),
            assistant("2026-10-23T12:00:02Z", "m1", 50),
            claude_line("2026-10-23T12:00:03Z", "assistant", side.take()),
            assistant("2026-10-23T12:00:04Z", "m1", 50),
        ];
        let snap = collect(&lines);
        assert_eq!(snap["steps"], 2, "m1 e o subagente, uma vez cada");
        assert_eq!(snap["out_tok"], 57);
    }

    /// S3 (corrigir): horário que volta para trás não conta nem puxa o relógio para trás.
    #[test]
    fn clock_going_backwards_does_not_rewind_the_cursor() {
        let lines = [
            claude_line("2026-10-23T12:00:00Z", "user", json!({"message": {"content": "pedido"}})),
            assistant("2026-10-23T12:00:10Z", "m1", 50),
            claude_line("2026-10-23T12:00:05Z", "user", json!({"message": {"content": "outro"}})),
            assistant("2026-10-23T12:00:12Z", "m2", 50),
        ];
        // 10 s até m1 e 2 s de m1 a m2; o Python contaria 7 s (de 12:00:05) no segundo.
        assert_eq!(collect(&lines)["llm_ms"], 12000);
    }

    /// S5 (corrigir): 200 ferramentas que nunca fecharam não param a conta de tempo de ferramenta.
    #[test]
    fn abandoned_tools_do_not_stop_tool_time_forever() {
        let mut fold = Fold::new(Kind::Claude);
        for i in 0..TOOLS_PENDING_MAX + 5 {
            fold.tool_start(Some(&format!("lost-{i}")), Some(0.0));
        }
        fold.tool_start(Some("live"), Some(10.0));
        fold.tool_end(Some("live"), Some(13.0));
        assert_eq!(fold.tool_ms, 3000.0);
        assert!(fold.tools.len() <= TOOLS_PENDING_MAX);
    }

    /// ST-36: a medida do ator só vale bem formada; a 1ª resposta tem forma própria.
    #[test]
    fn stream_measure_refuses_malformed_reports() {
        assert_eq!(Measure::parse(&json!({"tokens": 1000, "seconds": 10, "conversation": "c1"})),
            Some(Measure::Rate { tokens: 1000, seconds: 10.0, conversation: "c1".into() }));
        assert_eq!(Measure::parse(&json!({"first_response": true, "seconds": 1.5, "conversation": "t"})),
            Some(Measure::FirstResponse { seconds: 1.5, conversation: "t".into() }));
        for bad in [json!({"tokens": true, "seconds": 1.0, "conversation": "c1"}), json!({"tokens": 10, "seconds": true, "conversation": "c1"}),
            json!({"tokens": 10, "seconds": -1.0, "conversation": "c1"}), json!({"tokens": 10, "seconds": 1.0, "conversation": ""}),
            json!({"tokens": 10.5, "seconds": 1.0, "conversation": "c1"}), json!(null)] {
            assert_eq!(Measure::parse(&bad), None, "{bad}");
        }
        assert_eq!(Measure::parse(&json!({"tokens": -3, "seconds": 1.0, "conversation": "c"})),
            Some(Measure::Rate { tokens: 0, seconds: 1.0, conversation: "c".into() }));
    }

    /// ST-24: a segunda posse do mesmo arquivo é o mesmo acumulador, e o solto volta para a reabertura.
    #[test]
    fn shared_accumulator_survives_release_for_a_cheap_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, assistant("2026-10-23T12:00:02Z", "m1", 5)).unwrap();
        let a = acquire(Provider::Claude, &path);
        let crooked = dir.path().join(".").join("s.jsonl");
        let b = acquire(Provider::Claude, &crooked);
        assert!(Arc::ptr_eq(&a.acc, &b.acc));
        assert!(!Arc::ptr_eq(&a.acc, &acquire(Provider::Codex, &path).acc));
        a.acc.lock().unwrap().collect().unwrap();
        drop((a, b));
        let again = acquire(Provider::Claude, &path);
        assert_eq!(again.acc.lock().unwrap().offset, std::fs::metadata(&path).unwrap().len(), "reabrir aproveita a dobra");
    }

    /// Diferencial em transcripts reais da máquina (o conteúdo não entra no repo): `HANGAR_STATS_DIFF`
    /// aponta o retrato que o Python gravou de cada arquivo; o Rust lê os mesmos bytes e compara.
    #[test]
    #[ignore]
    fn differential_against_python_on_local_transcripts() {
        let Ok(input) = std::env::var("HANGAR_STATS_DIFF") else { panic!("HANGAR_STATS_DIFF não definido: nada foi comparado") };
        let rows: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(input).unwrap()).unwrap();
        let mut diffs = Vec::new();
        for row in &rows {
            let path = PathBuf::from(row["path"].as_str().unwrap());
            // O arquivo cresce entre os dois lados: compara só os bytes que o Python leu.
            let dir = tempfile::tempdir().unwrap();
            let copy = dir.path().join("s.jsonl");
            let size = row["size"].as_u64().unwrap_or(0) as usize;
            std::fs::write(&copy, &std::fs::read(&path).unwrap()[..size]).unwrap();
            let mut acc = Accumulator::new(provider(row["provider"].as_str().unwrap()), copy);
            let got = acc.collect().unwrap().map(Value::Object).unwrap_or(Value::Null);
            if got != row["snapshot"] {
                diffs.push(format!("{}: rust {got}\n   python {}", path.display(), row["snapshot"]));
            }
        }
        eprintln!("{} arquivos comparados, {} diferenças", rows.len(), diffs.len());
        assert!(diffs.is_empty(), "{}", diffs.join("\n"));
    }

    proptest::proptest! {
        /// Linha arbitrária (JSON torto, tipos trocados, bytes soltos) nunca derruba a conta.
        #[test]
        fn arbitrary_lines_never_panic(lines in proptest::collection::vec(proptest::prelude::any::<Vec<u8>>(), 0..8),
                                       shapes in proptest::collection::vec(0u8..6, 0..8)) {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("s.jsonl");
            let mut data = Vec::new();
            for (i, raw) in lines.iter().enumerate() {
                let shaped = match shapes.get(i).copied().unwrap_or(0) {
                    0 => raw.clone(),
                    1 => format!(r#"{{"type":"assistant","timestamp":"2026-10-23T12:00:0{}Z","message":{{"id":{},"usage":{}}}}}"#, i % 10, raw.len(), raw.len()).into_bytes(),
                    2 => format!(r#"{{"type":"user","timestamp":{},"message":{{"content":[{{"type":"tool_result","tool_use_id":{}}}]}}}}"#, raw.len(), raw.len()).into_bytes(),
                    3 => format!(r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{}}}}}}}}}"#, i64::MAX).into_bytes(),
                    4 => format!(r#"{{"type":"response_item","timestamp":"x","payload":{{"type":"message","role":"user","content":{}}}}}"#, raw.len()).into_bytes(),
                    _ => b"{}".to_vec(),
                };
                data.extend(shaped);
                data.push(b'\n');
            }
            std::fs::write(&path, &data).unwrap();
            for provider in [Provider::Claude, Provider::Codex] {
                let _ = Accumulator::new(provider, path.clone()).collect().unwrap();
            }
        }
    }
}
