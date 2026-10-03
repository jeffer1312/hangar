// crates/hangar-server/src/transcript/mod.rs
//! Leitura das conversas do Claude e do Codex, portada de backend/app/transcript.py,
//! adapters/codex/rollout.py e pqueue.py. A saída tem que sair igual à do Python: os aparelhos
//! deduplicam por id, e um id diferente na troca vira mensagem repetida na tela.

mod claude;
mod codex;
mod history;
mod peer;
mod py;
pub mod pyjson;

use std::sync::atomic::{AtomicU64, Ordering};

use hangar_api::chat::{ChatEvent, ChatKind};
use serde_json::Value;

pub use history::{history_etag, merged_history, HistoryRequest, InternalInfo, TAIL_WINDOW};
pub use py::ts_of_iso;

/// Linhas com texto que não viraram objeto JSON. O Python pula calado; aqui o log mostra a conta.
pub static SKIPPED_LINES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    Claude,
    ClaudeHeadless,
    Codex,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Provider> {
        match s {
            "claude" => Some(Self::Claude),
            "claude-headless" => Some(Self::ClaudeHeadless),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ClaudeHeadless => "claude-headless",
            Self::Codex => "codex",
        }
    }
}

/// Linha crua do transcript como JSON, com surrogate solto trocado por U+FFFD (`scrub_surrogates`,
/// models.py:22). None em linha em branco ou que não é JSON.
pub fn decode_line(raw: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(raw);
    let mut v = pyjson::loads_lossless(py::strip(&text))?;
    py::scrub_value(&mut v);
    Some(v)
}

/// Um leitor por arquivo: guarda o estado do `RewriteFilter` entre as linhas, como o
/// `TranscriptTailer` (transcript.py:777).
pub struct LineParser {
    provider: Provider,
    rewrite: claude::RewriteFilter,
    peer_resolver: claude::PeerResolver,
}

impl LineParser {
    pub fn new(provider: Provider) -> Self {
        Self { provider, rewrite: claude::RewriteFilter::default(), peer_resolver: peer::name_of_pid }
    }

    /// Troca o resolvedor do nome tmux do remetente; os testes não dependem do tmux da máquina.
    pub fn with_peer_resolver(mut self, resolver: fn(i64) -> Option<String>) -> Self {
        self.peer_resolver = resolver;
        self
    }

    /// Só passa a linha pelo filtro de reescrita, sem gerar evento: o leitor compartilhado começa
    /// no fim do arquivo e precisa do relógio das linhas anteriores, como o Python que lê a cauda.
    pub fn seed(&mut self, line: &[u8]) {
        if self.provider == Provider::Codex {
            return;
        }
        let text = String::from_utf8_lossy(line);
        if let Some(Value::Object(obj)) = pyjson::loads_lossless(py::strip(&text)) {
            self.rewrite.keep(&obj);
        }
    }

    /// Eventos de uma linha completa; `offset` é o byte onde ela começa (vira o `id:` do SSE).
    pub fn feed(&mut self, line: &[u8], offset: u64) -> Vec<ChatEvent> {
        let Some(value) = line_value(line) else { return Vec::new() };
        let mut evs = match (self.provider, &value) {
            (Provider::Codex, _) => codex::parse_rollout_obj(&value),
            (Provider::Claude | Provider::ClaudeHeadless, Value::Object(obj)) if self.rewrite.keep(obj) => {
                claude::parse_obj(obj, self.peer_resolver)
            }
            _ => Vec::new(),
        };
        for ev in &mut evs {
            ev.offset = Some(offset);
        }
        evs
    }
}

/// `parse_line` (transcript.py:304): strip e json; linha em branco não conta como pulada.
fn line_value(raw: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(raw);
    let t = py::strip(&text);
    if t.is_empty() {
        return None;
    }
    match pyjson::loads_lossless(t) {
        Some(v @ Value::Object(_)) => Some(v),
        _ => {
            SKIPPED_LINES.fetch_add(1, Ordering::Relaxed);
            None
        }
    }
}

pub(crate) fn event(kind: ChatKind, id: String) -> ChatEvent {
    ChatEvent { kind, id, ..ChatEvent::default() }
}

/// O `scrub_surrogates` que o validador do `ChatEvent` aplica em todo campo (models.py:241).
pub(crate) fn finish(ev: &mut ChatEvent) {
    py::scrub_str(&mut ev.id);
    for s in [&mut ev.text, &mut ev.tool_name, &mut ev.tool_use_id, &mut ev.result, &mut ev.hook_error]
        .into_iter()
        .flatten()
    {
        py::scrub_str(s);
    }
    for m in [&mut ev.tool_input, &mut ev.skill, &mut ev.orq].into_iter().flatten() {
        py::scrub_map(m);
    }
}
