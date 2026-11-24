//! Contexto e modelo da sessão Antigravity com terminal: o uso vem do `transcript_full.jsonl` (a última
//! `PLANNER_RESPONSE`), o modelo vem do rodapé do `agy` publicado pelo ator, que já mostra a troca por `/model`
//! antes da próxima resposta.
//!
//! A janela do modelo vem do cache do models.dev (`costs::pricing::context_window`): o `agy` não a diz. Modelo
//! fora do cache leva `window: 0`, e as telas mostram o número sem porcentagem.
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use hangar_api::session::{ContextUse, SessionRow};
use serde_json::Value;

use super::capped::Capped;
use super::facts_files::{FileKey, file_key};

/// A última resposta fica no fim; um transcript longo pesa megabytes.
const TAIL: u64 = 512 * 1024;

/// Uso e id do modelo da última resposta com tokens.
pub type Reading = Option<(u64, Option<String>)>;

/// O modelo do retrato do ator (`{"name","effort"}`) como id do `agy` (`gemini-4-neon-low`).
pub fn footer_model(value: &Value) -> Option<String> {
    let name = value["name"].as_str().filter(|name| !name.trim().is_empty())?;
    let footer = crate::terminal_state::AgyFooter {
        mode: None,
        model: name.to_owned(),
        effort: value["effort"].as_str().filter(|effort| !effort.is_empty()).map(str::to_owned),
    };
    Some(footer.model_id())
}

/// O modelo da linha: o do rodapé, escrito como o id real quando o transcript respondeu com o mesmo.
pub fn row_model(footer: Option<&str>, answered: Option<&str>) -> Option<String> {
    use crate::terminal_state::agy_model_key;
    match (footer, answered) {
        (Some(footer), Some(answered)) if agy_model_key(answered) == footer => Some(answered.to_owned()),
        (Some(footer), _) => Some(footer.to_owned()),
        (None, answered) => answered.map(str::to_owned),
    }
}

/// A última `PLANNER_RESPONSE` com tokens no rabo do transcript: contexto = entrada + cache lido (o
/// `input_tokens` do `agy` não inclui o cache).
pub fn read(path: &Path) -> std::io::Result<Reading> {
    let mut file = std::fs::File::open(path)?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(TAIL);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(last_response(&bytes, start > 0))
}

fn last_response(bytes: &[u8], cut: bool) -> Reading {
    let lines: Vec<&[u8]> = bytes.split(|b| *b == b'\n').collect();
    for (index, line) in lines.iter().enumerate().rev() {
        // A primeira linha de um rabo cortado é pedaço de outra.
        if cut && index == 0 { continue; }
        if !line.windows(b"PLANNER_RESPONSE".len()).any(|part| part == b"PLANNER_RESPONSE") { continue; }
        let Ok(Value::Object(record)) = serde_json::from_slice::<Value>(line) else { continue };
        if record.get("type").and_then(Value::as_str) != Some("PLANNER_RESPONSE") { continue; }
        let tokens = |key: &str| record.get(key).and_then(Value::as_u64);
        let (input, cache) = (tokens("input_tokens"), tokens("cache_read_tokens"));
        // Versões antigas gravam os campos nulos: essa resposta não diz o contexto.
        if input.is_none() && cache.is_none() { continue; }
        let used = input.unwrap_or(0).saturating_add(cache.unwrap_or(0));
        let model = record.get("model").and_then(Value::as_str).filter(|model| !model.is_empty()).map(str::to_owned);
        return Some((used, model));
    }
    None
}

/// Uma leitura por versão do arquivo: a lista roda a cada 1,5 s e o transcript só muda por resposta.
#[derive(Default)]
pub struct AgyContextCache {
    entries: Capped<String, (Option<FileKey>, Reading)>,
}

impl AgyContextCache {
    /// Preenche `context` e `model` da linha; `row.model` chega com o do rodapé (ou vazio).
    pub fn decorate(&mut self, row: &mut SessionRow, pricing_dir: &Path) {
        let Some(jsonl) = row.jsonl.clone().filter(|jsonl| !jsonl.is_empty()) else { return };
        let version = std::fs::metadata(&jsonl).ok().and_then(|metadata| file_key(&metadata));
        let cached = self.entries.get(&jsonl).filter(|(seen, _)| version.is_some() && *seen == version).map(|(_, reading)| reading.clone());
        let reading = match cached {
            Some(reading) => reading,
            None => {
                let reading = match read(Path::new(&jsonl)) {
                    Ok(reading) => reading,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(error) => {
                        super::facts::note("rust.list_context_unreadable", &row.name, format!("list_context_unreadable:{:?}", error.kind()),
                            "rabo do transcript do Antigravity ilegível; a linha fica sem contexto");
                        None
                    }
                };
                self.entries.insert(jsonl, (version, reading.clone()));
                reading
            }
        };
        let answered = reading.as_ref().and_then(|(_, model)| model.as_deref());
        row.model = row_model(row.model.as_deref(), answered);
        let window = row.model.as_deref().and_then(|model| crate::costs::pricing::context_window(pricing_dir, model)).unwrap_or(0);
        row.context = reading.as_ref().filter(|(used, _)| *used > 0).map(|(used, _)| ContextUse { used: *used, window });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn line(value: Value) -> String { format!("{value}\n") }

    #[test]
    fn the_context_is_input_plus_cache_read_of_the_last_response() {
        let text = [
            line(json!({"type":"USER_INPUT","content":"oi"})),
            line(json!({"type":"PLANNER_RESPONSE","model":"gemini-4-neon-low","input_tokens":100,"cache_read_tokens":10,"output_tokens":5})),
            line(json!({"type":"PLANNER_RESPONSE","model":"gemini-4-neon-low","input_tokens":2163,"cache_read_tokens":7232,"output_tokens":31})),
            line(json!({"type":"GENERIC","content":"saida"})),
        ].concat();
        assert_eq!(last_response(text.as_bytes(), false), Some((9395, Some("gemini-4-neon-low".into()))));
    }

    #[test]
    fn responses_without_tokens_and_a_cut_first_line_are_skipped() {
        let text = [
            r#"PLANNER_RESPONSE","input_tokens":999}"#.to_owned() + "\n",
            line(json!({"type":"PLANNER_RESPONSE","model":"gemini-4-neon-high","input_tokens":50,"cache_read_tokens":null})),
            line(json!({"type":"PLANNER_RESPONSE","model":"x","input_tokens":null,"cache_read_tokens":null})),
        ].concat();
        assert_eq!(last_response(text.as_bytes(), true), Some((50, Some("gemini-4-neon-high".into()))));
        assert_eq!(last_response(b"", false), None);
    }

    #[test]
    fn the_footer_model_wins_until_the_transcript_answers_with_it() {
        assert_eq!(footer_model(&json!({"name":"Gemini 4 Neon","effort":"medium"})).as_deref(), Some("gemini-4-neon-medium"));
        assert_eq!(footer_model(&Value::Null), None);
        // Trocado pelo `/model` e ainda sem resposta: o rodapé vale.
        assert_eq!(row_model(Some("gemini-4-neon-medium"), Some("gemini-4-neon-low")).as_deref(), Some("gemini-4-neon-medium"));
        // O transcript confirmou: a linha leva o id como o agy o escreve.
        assert_eq!(row_model(Some("gemini-3-8-flash-low"), Some("gemini-3.8-flash-low")).as_deref(), Some("gemini-3.8-flash-low"));
        assert_eq!(row_model(None, Some("gemini-4-neon-low")).as_deref(), Some("gemini-4-neon-low"));
        assert_eq!(row_model(None, None), None);
    }

    #[test]
    fn the_row_gets_tokens_without_a_window_and_rereads_only_a_new_version() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("transcript_full.jsonl");
        std::fs::write(&path, line(json!({"type":"PLANNER_RESPONSE","model":"gemini-4-neon-low","input_tokens":2163,"cache_read_tokens":7232}))).unwrap();
        let mut row: SessionRow = serde_json::from_value(json!({"name": "agy", "provider": "antigravity", "jsonl": path.to_string_lossy()})).unwrap();
        let mut cache = AgyContextCache::default();
        let pricing = dir.path().join("pricing");
        cache.decorate(&mut row, &pricing);
        assert_eq!(row.context, Some(ContextUse { used: 9395, window: 0 }), "sem cache do models.dev, sem janela");
        assert_eq!(row.model.as_deref(), Some("gemini-4-neon-low"));
        row.model = Some("gemini-4-neon-medium".into());
        std::fs::create_dir_all(&pricing).unwrap();
        std::fs::write(pricing.join(crate::costs::pricing_refresh::WINDOWS), r#"{"gemini-4-neon":1048576}"#).unwrap();
        cache.decorate(&mut row, &pricing);
        assert_eq!(row.context, Some(ContextUse { used: 9395, window: 1048576 }), "a janela do modelo base vale para o esforço");
        assert_eq!(row.model.as_deref(), Some("gemini-4-neon-medium"), "o rodapé novo vence o transcript antigo");
    }
}
