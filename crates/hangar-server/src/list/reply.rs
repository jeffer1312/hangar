// crates/hangar-server/src/list/reply.rs
//! Última resposta da sessão parada na linha da lista (`_decorate_replies`, registry.py:1919),
//! lida pelo histórico Rust com o mesmo corte de 8 eventos do Python.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::{Duration, SystemTime};

use hangar_api::chat::{ChatEvent, ChatKind};
use hangar_api::session::SessionRow;
use regex::Regex;

use super::capped::Capped;
use crate::transcript::py::{is_space, py_re};
use crate::transcript::history::merged_history_capped;
use crate::transcript::{HistoryRequest, Provider, TAIL_WINDOW};

const HISTORY_LIMIT: usize = 8;
// Mesmo teto do `DEEP_TAIL` dos links: cauda sem evento não pode fazer a lista reler o arquivo todo.
const MAX_TAIL: u64 = 8 * 1024 * 1024;
const MAX_CHARS: usize = 160;

// archive.py:90-92
static MD_LINK: LazyLock<Regex> = LazyLock::new(|| py_re(r"\[([^\]]+)\]\([^)]*\)"));
static MD_MARK: LazyLock<Regex> = LazyLock::new(|| py_re(r"[*_`~]{1,3}"));
static MD_START: LazyLock<Regex> = LazyLock::new(|| py_re(r"(?m)^\s{0,3}(?:[#>]+\s*|[-*+]\s+|\d+\.\s+)"));

/// `archive._texto_simples`: tira link, início de bloco e marcação do markdown, junta os espaços.
pub fn plain_text(text: &str) -> String {
    let text = MD_LINK.replace_all(text, "${1}");
    let text = MD_START.replace_all(&text, "");
    let text = MD_MARK.replace_all(&text, "");
    text.split(is_space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")
}

fn last_event_at(events: &[ChatEvent]) -> Option<f64> {
    events.iter().rev()
        .filter(|ev| matches!(ev.kind, ChatKind::UserMsg | ChatKind::AssistantMsg | ChatKind::ToolUse | ChatKind::ToolResult | ChatKind::Thinking))
        .find_map(|ev| ev.ts.filter(|&ts| ts != 0.0))
}

/// Há quanto a conversa do Claude parou. O mtime só vale sem evento datado na cauda: ele anda sem conversa
/// nenhuma. O rollout do Codex não data evento, por isso ele fica fora daqui.
pub fn conversation_idle(transcript: &Path) -> Option<Duration> {
    let req = HistoryRequest { provider: Provider::Claude, jsonl: transcript.to_owned(), queue: None, limit: Some(HISTORY_LIMIT), tail_window: TAIL_WINDOW };
    let event = merged_history_capped(&req, MAX_TAIL).ok().and_then(|events| last_event_at(&events))
        .and_then(|ts| SystemTime::UNIX_EPOCH.checked_add(Duration::try_from_secs_f64(ts).ok()?));
    event.or_else(|| std::fs::metadata(transcript).and_then(|m| m.modified()).ok())?.elapsed().ok()
}

struct Cached {
    jsonl: String,
    mtime: Option<f64>,
    provider: String,
    text: Option<String>,
    at: Option<f64>,
    activity: Option<f64>,
}

/// Por sessão, a resposta lida para (transcript, mtime, provider): a lista roda a cada 2 s e o
/// transcript só é relido quando muda.
#[derive(Default)]
pub struct ReplyCache(Capped<String, Cached>);

impl ReplyCache {
    /// Preenche `last_reply`/`last_reply_at` das linhas Claude e Codex: só a parada com transcript
    /// tem resposta. Linha de provedor que o Rust não lê fica como veio. Falha de leitura deixa a
    /// linha sem resposta, sem cache, e volta para quem chamou registrar. Na parada, `last_activity`
    /// entra com o mtime (a chave do cache) e sai com a hora do último evento da conversa.
    #[must_use = "sem registrar a falha, a linha parece só não ter resposta"]
    pub fn decorate(
        &mut self,
        rows: &mut [SessionRow],
        queue_of: impl Fn(&str) -> Option<PathBuf>,
    ) -> Vec<(String, io::Error)> {
        let mut failures = Vec::new();
        for row in rows {
            let Some(provider) = Provider::parse(&row.provider) else { continue };
            row.last_reply = None;
            row.last_reply_at = None;
            let Some(jsonl) = row.jsonl.clone().filter(|j| !j.is_empty() && row.state == "idle") else {
                continue;
            };
            let hit = self.0.get(&row.name).is_some_and(|c| {
                c.jsonl == jsonl && c.mtime == row.last_activity && c.provider == row.provider
            });
            if !hit {
                let req = HistoryRequest {
                    provider,
                    jsonl: PathBuf::from(&jsonl),
                    queue: queue_of(&row.name),
                    limit: Some(HISTORY_LIMIT),
                    tail_window: TAIL_WINDOW,
                };
                let events = match merged_history_capped(&req, MAX_TAIL) {
                    Ok(events) => events,
                    Err(error) => {
                        failures.push((row.name.clone(), error));
                        continue;
                    }
                };
                let activity = last_event_at(&events);
                let event = events
                    .into_iter()
                    .rev()
                    .find(|ev| ev.kind == ChatKind::AssistantMsg && ev.text.as_deref().is_some_and(|t| !t.is_empty()));
                let (text, at) = match event {
                    Some(ev) => (
                        ev.text.map(|t| plain_text(&t).chars().take(MAX_CHARS).collect()),
                        ev.ts.filter(|&ts| ts != 0.0).or(row.last_activity),
                    ),
                    None => (None, None),
                };
                let entry =
                    Cached { jsonl, mtime: row.last_activity, provider: row.provider.clone(), text, at, activity };
                self.0.insert(row.name.clone(), entry);
            }
            let Some(cached) = self.0.get(&row.name) else { continue };
            row.last_reply = cached.text.clone();
            row.last_reply_at = cached.at;
            // O mtime do transcript anda sem conversa nenhuma (o Claude toca o arquivo parado), e o prazo de ocultar e adormecer nunca venceria.
            if let Some(at) = cached.activity { row.last_activity = Some(at); }
        }
        failures
    }

    /// `_forget`: nome reusado por outra sessão não herda a resposta da morta.
    pub fn forget(&mut self, name: &str) {
        self.0.remove(name);
    }

    /// Renomear a sessão leva a resposta junto (registry.py:2830).
    pub fn rename(&mut self, old: &str, new: &str) {
        if let Some(cached) = self.0.remove(old) {
            self.0.insert(new.to_string(), cached);
        }
    }
}
