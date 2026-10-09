// crates/hangar-server/src/transcript/history.rs
//! Porte de `pqueue.merged_history` (backend/app/pqueue.py:977): transcript + fila de pendentes,
//! ordenados pelo relógio, com o corte por `limit` que a rota `/history` faz (api.py:2918).

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::UNIX_EPOCH;

use hangar_api::chat::{ChatEvent, ChatKind};
use regex::Regex;
use serde_json::{Map, Value};

use super::py::{self, py_re, strip};
use super::{claude, codex, event, finish, peer, pyjson, Provider, SKIPPED_LINES};

/// Janela inicial da leitura de trás para frente (pqueue.py:958).
pub const TAIL_WINDOW: u64 = 256 * 1024;
// pqueue.py:168
const HANDOFF_GRACE_S: f64 = 15.0 * 60.0;
// pqueue.py:1115: a fila cai depois dos eventos do transcript de mesmo relógio.
const QUEUE_ORDER: u64 = 1_000_000_000;

// pqueue.py:285, 294, 298
static ATTACH: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)(?:\s*—\s*)?📎\s*(?:imagem|arquivo):.*$"));
static IMG_PREFIX: LazyLock<Regex> = LazyLock::new(|| py_re(r"\A(?:\[Image #\d+\])+\s*"));
static IMG_SOURCE: LazyLock<Regex> = LazyLock::new(|| py_re(r"\[Image: source: ([^\]]+)\]"));
// pqueue.py `_COMMAND_NAME`/`_COMMAND_ARGS`
static COMMAND_NAME: LazyLock<Regex> = LazyLock::new(|| py_re(r"<command-name>([^<]*)</command-name>"));
static COMMAND_ARGS: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)<command-args>(.*?)</command-args>"));
// pqueue.py `_PEER_BODY`/`_PEER_STARTS`
static PEER_BODY: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?s)<cross-session-message\b[^>]*>\n?(.*?)\n?</cross-session-message>"));
const PEER_STARTS: [&str; 2] = ["<cross-session-message", "Another Claude session sent a message:"];

/// Corpo de cada recado de um registro que É recado; vazio para qualquer outro texto.
pub(crate) fn peer_bodies(text: &str) -> Vec<&str> {
    let t = strip(text);
    if !PEER_STARTS.iter().any(|p| t.starts_with(p)) { return Vec::new(); }
    PEER_BODY.captures_iter(t).filter_map(|c| c.get(1)).map(|m| strip(m.as_str())).filter(|b| !b.is_empty()).collect()
}

/// O que o Python devolve em `GET /internal/sessions/{name}/info`.
#[derive(serde::Deserialize, Clone, Debug)]
pub struct InternalInfo {
    pub provider: String,
    #[serde(default)]
    pub jsonl: Option<PathBuf>,
    #[serde(default)]
    pub session_key: String,
    /// `{"queue": "<sidecar da fila>"}`.
    #[serde(default)]
    pub history: Value,
    /// Codex sem terminal (sidecar `headless`): o estado ao vivo dele é do feed do hub.
    #[serde(default)]
    pub headless: bool,
}

#[derive(Clone, Debug)]
pub struct HistoryRequest {
    pub provider: Provider,
    pub jsonl: PathBuf,
    /// Sidecar da fila (`PromptQueue(name).path`); ausente = sessão sem fila.
    pub queue: Option<PathBuf>,
    /// `None` = histórico inteiro, como `limit` ausente ou `<= 0` no Python.
    pub limit: Option<usize>,
    /// Janela inicial da leitura de trás para frente; a rota usa `TAIL_WINDOW`.
    pub tail_window: u64,
}

impl InternalInfo {
    /// None quando o provider não é lido pelo Rust ou a sessão ainda não tem transcript.
    pub fn history_request(&self, limit: Option<usize>) -> Option<HistoryRequest> {
        Some(HistoryRequest {
            provider: Provider::parse(&self.provider)?,
            jsonl: self.jsonl.clone()?,
            queue: self.history.get("queue").and_then(Value::as_str).map(PathBuf::from),
            limit: limit.filter(|&n| n > 0),
            tail_window: TAIL_WINDOW,
        })
    }
}

struct Parsed {
    items: Vec<(f64, u64, ChatEvent)>,
    committed: HashMap<String, f64>,
    prev_ts: f64,
    start_ts: f64,
}

/// `merged_history` + o corte `evs[-limit:]` da rota. Transcript ausente dá histórico vazio.
pub fn merged_history(req: &HistoryRequest) -> io::Result<Vec<ChatEvent>> {
    merged_history_capped(req, u64::MAX)
}

/// Como `merged_history`, mas a janela com `limit` para em `max_window` bytes mesmo sem `limit`
/// eventos: quem só quer o fim (a última resposta da lista) não relê o transcript inteiro quando
/// a cauda é só anexo e tool_result.
pub(crate) fn merged_history_capped(req: &HistoryRequest, max_window: u64) -> io::Result<Vec<ChatEvent>> {
    let mut parsed = match req.limit {
        Some(limit) => {
            let mut window = req.tail_window.max(1).min(max_window);
            loop {
                let off = tail_offset(&req.jsonl, window);
                let parsed = parse_from(req, off)?;
                if off == 0 || parsed.items.len() >= limit || window >= max_window {
                    break parsed;
                }
                window = window.saturating_mul(4).min(max_window);
            }
        }
        None => parse_from(req, 0)?,
    };
    if let Some(queue) = &req.queue {
        merge_queue(queue, &mut parsed)?;
    }
    let mut items = parsed.items;
    // Estável como o sort do Python: empate em (ts, i) mantém a ordem de chegada.
    items.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(Ordering::Equal).then(a.1.cmp(&b.1)));
    let mut evs: Vec<ChatEvent> = items.into_iter().map(|(_, _, ev)| ev).collect();
    if let Some(limit) = req.limit {
        if evs.len() > limit {
            evs.drain(..evs.len() - limit);
        }
    }
    Ok(evs)
}

/// `_tail_offset` (pqueue.py:961): primeira linha completa dentro dos últimos `window` bytes.
fn tail_offset(path: &Path, window: u64) -> u64 {
    let read = || -> io::Result<u64> {
        let mut f = File::open(path)?;
        let size = f.metadata()?.len();
        if size <= window {
            return Ok(0);
        }
        f.seek(SeekFrom::Start(size - window))?;
        let mut partial = Vec::new();
        let n = BufReader::new(f).read_until(b'\n', &mut partial)?;
        Ok(size - window + n as u64)
    };
    read().unwrap_or(0)
}

/// `_ts_of_obj` (pqueue.py:227).
fn ts_of_obj(obj: &Map<String, Value>) -> f64 {
    if let Some(t) = obj.get("timestamp").and_then(Value::as_str) {
        return py::iso_timestamp(&t.replace('Z', "+00:00")).unwrap_or(0.0);
    }
    if let Some(n) = obj.get("message").and_then(Value::as_object).and_then(|m| m.get("timestamp")).and_then(py::number) {
        return n / 1000.0;
    }
    ["time", "created_at"].iter().find_map(|k| obj.get(*k).and_then(py::number)).map_or(0.0, |n| n / 1000.0)
}

/// `_transcript_start_ts` (pqueue.py:258) com o `or 0.0` de quem só poda.
fn transcript_start_ts(path: &Path) -> f64 {
    let Ok(f) = File::open(path) else { return 0.0 };
    let mut rd = BufReader::new(f);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match rd.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => return 0.0,
            Ok(_) => {}
        }
        if let Some(Value::Object(obj)) = pyjson::loads_lossless(&String::from_utf8_lossy(&raw)) {
            let ts = ts_of_obj(&obj);
            if ts > 0.0 {
                return ts;
            }
        }
    }
}

/// `_parse_from` (pqueue.py:1021).
fn parse_from(req: &HistoryRequest, offset: u64) -> io::Result<Parsed> {
    let mut p = Parsed {
        items: Vec::new(),
        committed: HashMap::new(),
        prev_ts: 0.0,
        start_ts: if offset > 0 { transcript_start_ts(&req.jsonl) } else { 0.0 },
    };
    // Só a ausência é histórico vazio: um 200 vazio com ETag faria o aparelho apagar a conversa.
    let file = match File::open(&req.jsonl) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(p),
        Err(e) => return Err(e),
    };
    let mut rd = BufReader::new(file);
    rd.seek(SeekFrom::Start(offset))?;
    // Reescrita do `--resume`: só o Claude regrava o jsonl.
    let mut rewrite = (req.provider != Provider::Codex).then(claude::RewriteFilter::default);
    let mut held_ids = HashSet::new();
    let mut raw = Vec::new();
    let mut i = 0u64;
    loop {
        raw.clear();
        if rd.read_until(b'\n', &mut raw)? == 0 {
            break;
        }
        let idx = i;
        i += 1;
        // O Python lê em modo texto: "\r\n" chega como "\n".
        let mut line = String::from_utf8_lossy(&raw).into_owned();
        if line.ends_with("\r\n") {
            line.truncate(line.len() - 2);
            line.push('\n');
        }
        let silent = rewrite.as_ref().and_then(|_| claude::silent_attachment_timestamp(&line));
        let (line_ts, evs) = match silent {
            Some(att) => (py::iso_timestamp(&att.replace('Z', "+00:00")).unwrap_or(0.0), Vec::new()),
            None => {
                let Some(value) = pyjson::loads_lossless(&line) else {
                    if !strip(&line).is_empty() {
                        SKIPPED_LINES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    }
                    continue;
                };
                let Value::Object(obj) = &value else {
                    SKIPPED_LINES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    continue;
                };
                if let Some(rw) = rewrite.as_mut() {
                    if !rw.keep(obj) {
                        continue;
                    }
                }
                let evs = if rewrite.is_some() { claude::parse_obj(obj, peer::name_of_pid) } else { codex::parse_rollout_obj(&value) };
                (ts_of_obj(obj), evs)
            }
        };
        if line_ts > 0.0 {
            if p.start_ts == 0.0 {
                p.start_ts = line_ts;
            }
            p.prev_ts = line_ts;
        }
        if evs.is_empty() {
            continue;
        }
        let ts = if line_ts != 0.0 { line_ts } else { p.prev_ts };
        p.prev_ts = ts;
        absorb(&mut p, ts, idx, evs, &mut held_ids);
    }
    Ok(p)
}

/// `_absorve` (pqueue.py:1044).
fn absorb(p: &mut Parsed, ts: f64, i: u64, evs: Vec<ChatEvent>, held_ids: &mut HashSet<String>) {
    for ev in evs {
        // Cada reenvio de um prompt barrado repete o id "held:"; a lista leva um só.
        if ev.id.starts_with("held:") && !held_ids.insert(ev.id.clone()) {
            continue;
        }
        let ets = ev.ts.filter(|t| *t != 0.0).unwrap_or(ts);
        if matches!(ev.kind, ChatKind::UserMsg) {
            if let Some(text) = ev.text.as_deref().filter(|t| !t.is_empty()) {
                for ln in chaves_de_commit(text) {
                    if ets > p.committed.get(&ln).copied().unwrap_or(0.0) {
                        p.committed.insert(ln, ets);
                    }
                }
            }
        }
        p.items.push((ets, i, ev));
    }
}

pub(crate) fn strip_attach(text: &str) -> String {
    ATTACH.replace(text, "").into_owned()
}

/// `_chaves_de_commit` (pqueue.py:301). Repetição não importa: quem usa só compara o relógio.
pub(crate) fn chaves_de_commit(text: &str) -> Vec<String> {
    let t = strip(text);
    let base = IMG_PREFIX.replace(t, "").into_owned();
    let fonte = IMG_SOURCE.replace_all(t, |c: &regex::Captures| format!("📎 imagem: {}", &c[1])).into_owned();
    let mut out = Vec::new();
    for variant in [t.to_string(), base.clone(), strip_attach(t), strip_attach(&base), fonte] {
        let v = strip(&variant);
        if v.is_empty() {
            continue;
        }
        out.push(v.to_string());
        out.extend(v.split('\n').map(strip).filter(|ln| !ln.is_empty()).map(str::to_string));
    }
    // `/comando args` digitado vira `<command-name>/comando</command-name>` + `<command-args>` no
    // transcript. Só a mensagem que É o comando conta (citar a tag não conta), e ela entra inteira.
    if t.starts_with("<command-name>") || t.starts_with("<command-message>") {
        if let Some(c) = COMMAND_NAME.captures(t) {
            let args = COMMAND_ARGS.captures(t).map(|a| strip(&a[1]).to_string()).unwrap_or_default();
            let comando = strip(&format!("{} {args}", strip(&c[1]))).to_string();
            if !comando.is_empty() { out.push(comando); }
        }
    }
    out.extend(peer_bodies(t).into_iter().map(str::to_string));
    out
}

/// `PromptQueue.load` (pqueue.py:875): `splitlines` do Python, linha que não é objeto JSON pula.
fn load_queue(path: &Path) -> io::Result<Vec<Map<String, Value>>> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let text = String::from_utf8_lossy(&bytes);
    Ok(py::splitlines(&text)
        .into_iter()
        .map(strip)
        .filter(|l| !l.is_empty())
        .filter_map(|l| match pyjson::loads_lossless(l) {
            Some(Value::Object(m)) => Some(m),
            _ => None,
        })
        .collect())
}

fn is_local_output(entry: &Map<String, Value>) -> bool {
    entry.get("papel").and_then(Value::as_str) == Some("assistant")
}

/// `_da_sessao_atual` (pqueue.py:171) com o relógio já resolvido.
fn from_current_session(entry: &Map<String, Value>, min_ts: f64, ts: f64) -> bool {
    let grace = if py::truthy(entry.get("pre_transcript")) { HANDOFF_GRACE_S } else { 0.0 };
    ts >= min_ts - grace
}

/// `_entry_event` (pqueue.py:210).
fn entry_event(entry: &Map<String, Value>) -> ChatEvent {
    let id = entry.get("id").map_or_else(|| "None".to_string(), py::py_str);
    let text = entry.get("text").and_then(Value::as_str).map(str::to_string);
    let mut ev = if is_local_output(entry) {
        ChatEvent { text, ..event(ChatKind::AssistantMsg, format!("local-{id}")) }
    } else {
        ChatEvent {
            text,
            queued_delivered: entry.get("delivered").and_then(Value::as_bool),
            desistiu: py::truthy(entry.get("desistiu")).then_some(true),
            queued_ts: entry.get("ts").and_then(py::number),
            ..event(ChatKind::UserMsg, format!("queued-{id}"))
        }
    };
    finish(&mut ev);
    ev
}

/// Junção da fila (pqueue.py:1107-1137).
fn merge_queue(path: &Path, p: &mut Parsed) -> io::Result<()> {
    for entry in load_queue(path)? {
        let Some(text) = entry.get("text").and_then(Value::as_str).map(strip) else { continue };
        if text.is_empty() {
            continue;
        }
        // `float(entry.get("ts") or prev_ts)`.
        // ponytail: texto que o float() do Python não lê derrubaria a rota; aqui vira prev_ts.
        let ts = match entry.get("ts") {
            Some(Value::String(s)) if !s.is_empty() => strip(s).parse().unwrap_or(p.prev_ts),
            v if py::truthy(v) => v.and_then(py::number).unwrap_or(p.prev_ts),
            _ => p.prev_ts,
        };
        if is_local_output(&entry) {
            if p.start_ts == 0.0 || from_current_session(&entry, p.start_ts, ts) {
                p.items.push((ts, QUEUE_ORDER, entry_event(&entry)));
            }
            continue;
        }
        if py::truthy(entry.get("confirmed")) {
            continue;
        }
        let cap = strip(&strip_attach(text)).to_string();
        let committed = |k: &str| p.committed.get(k).copied().unwrap_or(-1.0);
        let committed_at = committed(text).max(if cap.is_empty() { -1.0 } else { committed(&cap) });
        if committed_at >= ts {
            continue;
        }
        if p.start_ts != 0.0 && !from_current_session(&entry, p.start_ts, ts) {
            continue;
        }
        p.items.push((ts, QUEUE_ORDER, entry_event(&entry)));
    }
    Ok(())
}

fn stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime = meta.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    Some(format!("{}.{mtime}", meta.len()))
}

// Os mesmos bytes rendem outra resposta quando o parser muda: o binário novo invalida o ETag.
static CODE_MARK: LazyLock<u128> = LazyLock::new(|| {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos())
});

/// Validador do `/history` pelos metadados do transcript e da fila (o papel de `historico_etag`,
/// pqueue.py:1155). Formato próprio: o cliente só devolve o valor. Sem transcript, sem validador.
pub fn history_etag(req: &HistoryRequest) -> Option<String> {
    let transcript = stamp(&req.jsonl)?;
    let queue_stamp = req.queue.as_deref().and_then(stamp).unwrap_or_else(|| "-".into());
    let limit = req.limit.map_or_else(|| "None".to_string(), |n| n.to_string());
    Some(format!("\"rs-{transcript}-{queue_stamp}-{}-{limit}-{}\"", req.provider.as_str(), *CODE_MARK))
}

#[cfg(test)]
mod tests {
    use super::{chaves_de_commit, merged_history, merged_history_capped, HistoryRequest, Provider, TAIL_WINDOW};

    #[test]
    fn capped_window_stops_before_reading_the_whole_transcript() {
        let dir = tempfile::tempdir().unwrap();
        let jsonl = dir.path().join("s.jsonl");
        let assistant = |id: &str, text: &str| {
            format!(
                "{{\"type\":\"assistant\",\"uuid\":\"{id}\",\"timestamp\":\"2026-01-01T00:00:00Z\",\
                 \"message\":{{\"role\":\"assistant\",\"content\":[{{\"type\":\"text\",\"text\":\"{text}\"}}]}}}}\n"
            )
        };
        // Cauda de ~1 MiB sem evento entre a resposta antiga e o começo do arquivo.
        let filler = "{\"type\":\"progress\",\"pad\":\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\"}\n".repeat(16_000);
        std::fs::write(&jsonl, format!("{}{filler}", assistant("old", "antiga"))).unwrap();
        let req = HistoryRequest { provider: Provider::Claude, jsonl, queue: None, limit: Some(8), tail_window: 1024 };
        let texts = |evs: Vec<hangar_api::chat::ChatEvent>| evs.into_iter().filter_map(|e| e.text).collect::<Vec<_>>();
        assert_eq!(texts(merged_history(&req).unwrap()), ["antiga"]);
        assert!(texts(merged_history_capped(&req, TAIL_WINDOW).unwrap()).is_empty());
    }

    #[test]
    fn typed_slash_command_matches_its_transcript_form() {
        let skill = "<command-message>acme:deploy</command-message>\n<command-name>/acme:deploy</command-name>";
        assert!(chaves_de_commit(skill).contains(&"/acme:deploy".to_string()));
        let with_args = "<command-name>/btw</command-name>\n<command-args>qual a cor do céu</command-args>";
        assert!(chaves_de_commit(with_args).contains(&"/btw qual a cor do céu".to_string()));
        let quoting = "veja como fica <command-name>/compact</command-name> no transcript";
        assert!(!chaves_de_commit(quoting).contains(&"/compact".to_string()));
        let multiline = "<command-name>/x</command-name>\n<command-args>a\nb</command-args>";
        let keys = chaves_de_commit(multiline);
        assert!(keys.contains(&"/x a\nb".to_string()) && !keys.contains(&"b".to_string()));
    }
}
