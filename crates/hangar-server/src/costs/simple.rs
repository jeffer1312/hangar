//! Leitores retomáveis de custo do Pi, omp, Kimi e Antigravity.

use crate::costs::index::Fold;
use crate::costs::pricing::canonizar_provedor;
use crate::costs::py::{LocalTs, parse_obj, py_int};
use crate::costs::rows::{FoldOutput, UsageRow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub const PI_VERSION: &str = "pi:1";
pub const KIMI_VERSION: &str = "kimi:1";
pub const AGY_VERSION: &str = "antigravity:1";
/// Sufixos de esforço que o `agy` põe no id do modelo (`gemini-4-neon-high`); a tarifa é a do modelo base.
const AGY_EFFORTS: [&str; 4] = ["-low", "-medium", "-high", "-xhigh"];

#[derive(Clone, Default, Serialize, Deserialize)]
struct Tokens {
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct PiFold {
    session_id: String,
    source: String,
    cwd: String,
    model: String,
    provider: String,
    ts: Option<LocalTs>,
    tokens: Tokens,
    seen: bool,
}

impl Fold for PiFold {
    fn line(&mut self, raw: &[u8]) {
        let Some(record) = parse_obj(raw) else { return };
        match record.get("type").and_then(Value::as_str) {
            Some("session") => {
                if let Some(cwd) = record.get("cwd").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                    self.cwd = cwd.to_owned();
                }
                if let Some(ts) = record.get("timestamp").and_then(Value::as_str).and_then(LocalTs::from_iso) {
                    self.ts = Some(ts);
                }
            }
            Some("model_change") => {
                if let Some((provider, model)) = record.get("model").and_then(Value::as_str).and_then(|s| s.split_once('/')) {
                    self.provider = provider.to_owned();
                    self.model = model.to_owned();
                } else {
                    if let Some(provider) = record.get("provider").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                        self.provider = provider.to_owned();
                    }
                    if let Some(model) = record.get("modelId").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                        self.model = model.to_owned();
                    }
                }
            }
            Some("message") => {
                let Some(usage) = record.get("message").and_then(Value::as_object)
                    .and_then(|msg| msg.get("usage")).and_then(Value::as_object) else { return };
                self.seen = true;
                self.tokens.input += py_int(usage.get("input"));
                self.tokens.output += py_int(usage.get("output"));
                self.tokens.cache_read += py_int(usage.get("cacheRead"));
                self.tokens.cache_write += py_int(usage.get("cacheWrite"));
            }
            _ => {}
        }
    }

    fn close(&mut self) -> FoldOutput {
        let mut costs = Vec::new();
        if let Some(ts) = self.ts.filter(|_| self.seen) {
            let provider = canonizar_provedor(&self.provider);
            costs.push(row(ts, &self.source, nonempty(&provider, "?"), nonempty(&self.model, "?"),
                nonempty(&self.cwd, "desconhecido"), &self.session_id, &self.tokens, false));
        }
        FoldOutput { costs, usage: Vec::new(), areas: None }
    }
}

pub fn new_pi_fold(root: &Path, source: &str) -> impl Fn(&Path) -> PiFold + Sync {
    let root = root.to_path_buf();
    let source = source.to_owned();
    move |path| {
        let relative = path.strip_prefix(&root).expect("transcript dentro da raiz").with_extension("");
        let session_id = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join(std::path::MAIN_SEPARATOR_STR);
        PiFold { session_id, source: source.clone(), cwd: String::new(), model: String::new(),
            provider: String::new(), ts: None, tokens: Tokens::default(), seen: false }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct KimiFold {
    session_id: String,
    subagent: bool,
    model: String,
    ts: Option<LocalTs>,
    tokens: Tokens,
    seen: bool,
}

impl Fold for KimiFold {
    fn line(&mut self, raw: &[u8]) {
        if !raw.windows(b"usage.record".len()).any(|part| part == b"usage.record") { return; }
        let Some(record) = parse_obj(raw) else { return };
        if record.get("type").and_then(Value::as_str) != Some("usage.record") { return; }
        let Some(usage) = record.get("usage").and_then(Value::as_object) else { return };
        self.seen = true;
        if let Some(model) = record.get("model").and_then(Value::as_str).filter(|s| !s.is_empty()) {
            self.model = model.to_owned();
        }
        let milliseconds = match record.get("time") {
            Some(Value::Number(number)) => number.as_f64(),
            Some(Value::Bool(value)) => Some(f64::from(u8::from(*value))),
            _ => None,
        };
        if let Some(ms) = milliseconds { self.ts = Some(LocalTs::from_millis_f64(ms)); }
        self.tokens.input += py_int(usage.get("inputOther"));
        self.tokens.output += py_int(usage.get("output"));
        self.tokens.cache_read += py_int(usage.get("inputCacheRead"));
        self.tokens.cache_write += py_int(usage.get("inputCacheCreation"));
    }

    fn close(&mut self) -> FoldOutput {
        let mut costs = Vec::new();
        if let Some(ts) = self.ts.filter(|_| self.seen) {
            let prefix = self.model.split_once('/').map_or("", |(provider, _)| provider);
            let provider = canonizar_provedor(prefix);
            costs.push(row(ts, "kimi", nonempty(&provider, nonempty(prefix, "?")), nonempty(&self.model, "?"),
                "desconhecido", &self.session_id, &self.tokens, self.subagent));
        }
        FoldOutput { costs, usage: Vec::new(), areas: None }
    }
}

pub fn new_kimi_fold(path: &Path) -> KimiFold {
    let agent = path.parent();
    let agents = agent.and_then(Path::parent);
    let session = agents.and_then(Path::parent);
    let session_id = session.and_then(Path::file_name).unwrap_or_default().to_string_lossy().into_owned();
    let subagent = path.file_name().is_some_and(|n| n == "wire.jsonl")
        && agents.and_then(Path::file_name).is_some_and(|n| n == "agents")
        && agent.and_then(Path::file_name).is_some_and(|n| n != "main");
    KimiFold { session_id, subagent, model: String::new(), ts: None, tokens: Tokens::default(), seen: false }
}

pub fn kimi_projects(index_file: &Path) -> HashMap<String, String> {
    let mut projects = HashMap::new();
    let Ok(file) = std::fs::File::open(index_file) else { return projects };
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { break };
        let Some(record) = parse_obj(line.as_bytes()) else { continue };
        let Some(id) = record.get("sessionId").and_then(Value::as_str).filter(|s| !s.is_empty()) else { continue };
        projects.insert(id.to_owned(), record.get("workDir").and_then(Value::as_str).unwrap_or("").to_owned());
    }
    projects
}

/// O `transcript_full.jsonl` de uma conversa do `agy`: cada resposta (`PLANNER_RESPONSE`) traz o modelo e os tokens
/// dela, com o cache fora do `input_tokens`. Uma linha por dia e modelo; o projeto vem do `history.jsonl` na leitura.
#[derive(Clone, Serialize, Deserialize)]
pub struct AgyFold {
    session_id: String,
    rows: Vec<UsageRow>,
}

impl Fold for AgyFold {
    fn line(&mut self, raw: &[u8]) {
        if !raw.windows(b"PLANNER_RESPONSE".len()).any(|part| part == b"PLANNER_RESPONSE") { return; }
        let Some(record) = parse_obj(raw) else { return };
        if record.get("type").and_then(Value::as_str) != Some("PLANNER_RESPONSE") { return; }
        // Versões antigas gravam os campos nulos: sem tokens não há o que contar.
        if !["input_tokens", "cache_read_tokens", "output_tokens"].iter().any(|key| record.get(*key).is_some_and(Value::is_number)) { return; }
        let Some(ts) = record.get("created_at").and_then(Value::as_str).and_then(LocalTs::from_iso) else { return };
        let model = agy_base_model(record.get("model").and_then(Value::as_str).unwrap_or(""));
        let model = if model.is_empty() { "?".to_owned() } else { model };
        let (input, output, cache_read) = (py_int(record.get("input_tokens")), py_int(record.get("output_tokens")), py_int(record.get("cache_read_tokens")));
        let day = ts.day();
        if let Some(row) = self.rows.iter_mut().find(|row| row.model == model && row.ts.day() == day) {
            row.input += input;
            row.output += output;
            row.cache_read += cache_read;
            return;
        }
        let tokens = Tokens { input, output, cache_read, cache_write: 0 };
        self.rows.push(row(ts, "antigravity", "google", &model, "desconhecido", &self.session_id, &tokens, false));
    }

    fn close(&mut self) -> FoldOutput {
        FoldOutput { costs: self.rows.clone(), usage: Vec::new(), areas: None }
    }
}

/// O id da conversa é a pasta em `brain/<id>/.system_generated/logs/`.
pub fn new_agy_fold(path: &Path) -> AgyFold {
    let session_id = hangar_antigravity::paths::conversation_of_transcript(path).unwrap_or_default();
    AgyFold { session_id, rows: Vec::new() }
}

/// `gemini-4-neon-high` → `gemini-4-neon`: o esforço não muda o preço por token, e o models.dev só lista o base.
pub fn agy_base_model(model: &str) -> String {
    let model = crate::transcript::py::strip(model);
    AGY_EFFORTS.iter().find_map(|suffix| model.strip_suffix(*suffix)).filter(|base| !base.is_empty()).unwrap_or(model).to_owned()
}

fn nonempty<'a>(value: &'a str, fallback: &'a str) -> &'a str {
    if value.is_empty() { fallback } else { value }
}

fn row(ts: LocalTs, source: &str, provider: &str, model: &str, project: &str,
    session_id: &str, tokens: &Tokens, subagent: bool) -> UsageRow {
    UsageRow { ts, source: source.to_owned(), provider: provider.to_owned(), model: model.to_owned(),
        project: project.to_owned(), session_id: session_id.to_owned(), input: tokens.input,
        output: tokens.output, cache_write: tokens.cache_write, cache_read: tokens.cache_read,
        subagente: subagent, account_id: None, codex_long_context: false, cache_write_1h: 0,
        fast: false, regravado: 0, regravado_1h: 0 }
}
