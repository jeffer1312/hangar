//! Leitura das conversas estruturadas dos agentes, portada dos leitores de transcript e fila.
//! A saída tem que sair igual à do Python: os aparelhos
//! deduplicam por id, e um id diferente na troca vira mensagem repetida na tela.

mod claude;
mod codex;
mod agents;
pub use agents::{parse_pi_object, parse_kimi_object, PiStream};
pub mod history;
mod peer;
pub mod interactions;
pub use hangar_core::pycompat::py;
pub use hangar_core::pycompat::pyjson;
// Re-export de compatibilidade: ninguém o usa mais pelo caminho `transcript::`.
pub use hangar_core::pycompat::json_tree;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Once, OnceLock};

use hangar_api::chat::{ChatEvent, ChatKind};
use serde_json::Value;

pub use history::{history_etag, merged_history, merged_history_with_notices, HistoryRequest, InternalInfo, TAIL_WINDOW};
pub use py::ts_of_iso;
pub use claude::{hook_failure_reason, is_cli_note, is_command_meta, strip_meta_blocks, RewriteFilter};
pub use history::turn_seed_start;
pub use codex::is_context_wrapper;

/// O histórico transferido já validou ramo e fronteira; não passa pelo filtro temporal do stream.
pub fn parse_claude_object(object: &serde_json::Map<String, Value>) -> Vec<ChatEvent> {
    claude::parse_obj(object, peer::name_of_pid)
}

/// Linhas com texto que não viraram objeto JSON. O Python pula calado; aqui o log mostra a conta.
pub static SKIPPED_LINES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Provider {
    Claude,
    ClaudeHeadless,
    Codex,
    Pi,
    Omp,
    Kimi,
    Orq,
}

impl Provider {
    pub fn parse(s: &str) -> Option<Provider> {
        match s {
            "claude" => Some(Self::Claude),
            "claude-headless" => Some(Self::ClaudeHeadless),
            "codex" => Some(Self::Codex),
            "pi" => Some(Self::Pi),
            "omp" => Some(Self::Omp),
            "kimi" => Some(Self::Kimi),
            "orq" => Some(Self::Orq),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::ClaudeHeadless => "claude-headless",
            Self::Codex => "codex",
            Self::Pi => "pi",
            Self::Omp => "omp",
            Self::Kimi => "kimi",
            Self::Orq => "orq",
        }
    }
}

pub use hangar_core::pycompat::decode_line;

/// Leitor da linha de `orq`: mora na orquestração, que depende deste módulo, e entra pela subida.
pub type OrqParser = fn(&Value, Option<&std::path::Path>) -> Vec<ChatEvent>;

static ORQ_PARSER: OnceLock<OrqParser> = OnceLock::new();

/// A primeira chamada vale; as seguintes não trocam o leitor.
pub fn set_orq_parser(parser: OrqParser) {
    let _ = ORQ_PARSER.set(parser);
}

pub(crate) fn parse_orq(value: &Value, dir: Option<&std::path::Path>) -> Vec<ChatEvent> {
    if let Some(parser) = ORQ_PARSER.get() {
        return parser(value, dir);
    }
    static MISSING: Once = Once::new();
    MISSING.call_once(|| {
        tracing::error!(code = "transcript_orq_parser_missing", "linha de orq sem leitor registrado; a conversa sai vazia");
    });
    Vec::new()
}

/// Um leitor por arquivo: guarda o estado do `RewriteFilter` entre as linhas, como o
/// `TranscriptTailer` (transcript.py:777).
pub struct LineParser {
    provider: Provider,
    rewrite: claude::RewriteFilter,
    peer_resolver: claude::PeerResolver,
    run_dir: Option<std::path::PathBuf>,
    notices: Option<claude::NoticeFold>,
    pi: PiStream,
}

impl LineParser {
    /// Libera a mensagem Pi sem irmã no fim do lote; o offset continua o da mensagem retida.
    pub fn flush(&mut self) -> Vec<ChatEvent> { self.pi.flush() }
    pub fn has_pending(&self) -> bool { self.pi.has_pending() }
    pub fn new(provider: Provider) -> Self {
        Self { provider, rewrite: claude::RewriteFilter::default(), peer_resolver: peer::name_of_pid, run_dir: None, notices: None, pi: PiStream::default() }
    }

    /// Com as notas dos avisos da CLI (hook que falha, nova tentativa, recusa). Só o chat as pede: quem procura
    /// a última resposta ou o texto da pessoa (push, loops, retomada) tomaria uma nota por resposta.
    pub fn with_notices(mut self) -> Self {
        if matches!(self.provider, Provider::Claude | Provider::ClaudeHeadless) {
            self.notices = Some(claude::NoticeFold::default());
        }
        self
    }

    pub fn with_path(mut self, path: &std::path::Path) -> Self {
        self.run_dir = path.parent().map(std::path::Path::to_path_buf);
        self
    }

    /// Troca o resolvedor do nome tmux do remetente; os testes não dependem do tmux da máquina.
    pub fn with_peer_resolver(mut self, resolver: fn(i64) -> Option<String>) -> Self {
        self.peer_resolver = resolver;
        self
    }

    /// Só passa a linha pelo filtro de reescrita, sem gerar evento: o leitor compartilhado começa
    /// no fim do arquivo e precisa do relógio das linhas anteriores, como o Python que lê a cauda.
    pub fn seed(&mut self, line: &[u8], offset: u64) {
        if !matches!(self.provider, Provider::Claude | Provider::ClaudeHeadless) {
            return;
        }
        self.at_line(offset);
        let text = String::from_utf8_lossy(line);
        if let Some(Value::Object(obj)) = pyjson::loads_lossless(py::strip(&text)) {
            // O fold também precisa do começo do turno que já passou: senão a 2ª falha viraria a "primeira".
            if self.rewrite.keep(&obj) && let Some(fold) = self.notices.as_mut() { fold.feed(&obj, &mut Vec::new()); }
        }
    }

    fn at_line(&mut self, offset: u64) {
        if offset == 0 { self.pi = PiStream::default(); }
        if offset == 0 && let Some(fold) = self.notices.as_mut() { fold.file_start(); }
    }

    /// Eventos de uma linha completa; `offset` é o byte onde ela começa (vira o `id:` do SSE).
    pub fn feed(&mut self, line: &[u8], offset: u64) -> Vec<ChatEvent> {
        self.at_line(offset);
        let Some(value) = line_value(line) else { return Vec::new() };
        let mut evs = match (self.provider, &value) {
            (Provider::Codex, _) => codex::parse_rollout_obj(&value),
            (Provider::Pi | Provider::Omp, _) => self.pi.feed_with_offset(&value, Some(offset)),
            (Provider::Kimi, _) => parse_kimi_object(&value),
            (Provider::Orq, _) => parse_orq(&value, self.run_dir.as_deref()),
            (Provider::Claude | Provider::ClaudeHeadless, Value::Object(obj)) if self.rewrite.keep(obj) => {
                let mut evs = claude::parse_obj(obj, self.peer_resolver);
                if let Some(fold) = self.notices.as_mut() { fold.feed(obj, &mut evs); }
                evs
            }
            _ => Vec::new(),
        };
        for ev in &mut evs {
            if ev.offset.is_none() { ev.offset = Some(offset); }
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

pub fn event(kind: ChatKind, id: String) -> ChatEvent {
    ChatEvent { kind, id, ..ChatEvent::default() }
}

/// O `scrub_surrogates` que o validador do `ChatEvent` aplica em todo campo (models.py:241).
pub fn finish(ev: &mut ChatEvent) {
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
    for hunk in ev.patch.iter_mut().flatten() {
        hunk.lines.iter_mut().for_each(py::scrub_str);
    }
}

#[cfg(test)]
mod orq_registry {
    use super::*;

    // Nenhum teste deste crate registra o leitor: o registro é do servidor.
    #[test]
    fn an_orq_line_without_a_registered_parser_yields_no_event() {
        let line = br#"{"ts":"2026-09-29T21:53:40-03:00","kind":"advance","text":"T1: abriu","task":1}"#;
        assert!(LineParser::new(Provider::Orq).feed(line, 0).is_empty());
    }
}
