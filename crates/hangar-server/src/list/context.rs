//! Contexto e modelo da sessão Claude lidos do transcript (porte de `claude_context.py` e de
//! `_claude_reading` em `registry.py`), sem depender da statusline ser a do Hangar.
//!
//! O `usage` da última resposta do agente principal é o pedido inteiro. A janela não vem nele (o id
//! do modelo não diz se é a variante de 1M): sai da janela declarada, do modelo da sessão ou da
//! conta, e do próprio uso.
use super::capped::Capped;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use hangar_api::session::ContextUse;
use regex::Regex;
use serde_json::Value;

/// Fim do arquivo lido: a última resposta fica perto do fim, e um transcript longo pesa megabytes.
const TAIL: u64 = 512 * 1024;
pub const WINDOW_DEFAULT: u64 = 200_000;
pub const WINDOW_1M: u64 = 1_000_000;
/// Mesmo prazo da statusline: é leitura do fim de um arquivo a cada rodada da lista.
pub const TTL_SECS: f64 = 20.0;

// `claude-haiku-4-5-20251001`: a data do snapshot não é parte do nome que a tela mostra.
static DATED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-\d{8}$").unwrap());
static USAGE: LazyLock<memchr::memmem::Finder<'static>> = LazyLock::new(|| memchr::memmem::Finder::new(b"\"usage\""));
static FAMILY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^(?:claude-)?(opus|sonnet|haiku|fable)\b").unwrap());

pub type Reading = (Option<ContextUse>, Option<String>);

/// O que a sessão diz de si; quem monta a linha resolve de onde cada um vem.
pub struct ReadingInputs<'a> {
    pub jsonl: &'a Path,
    /// Pasta da conta da sessão (`config_dir_of`), senão a padrão do servidor: `CLAUDE_CONFIG_DIR`
    /// ou `~/.claude`, como `_account_model`.
    pub account_dir: &'a Path,
    /// Modelo que a statusline recebeu (`/model` no meio da sessão); vence o da abertura.
    pub chosen: Option<&'a str>,
    /// `--model` do processo ou `model` do sidecar da sessão sem terminal.
    pub opened: Option<&'a str>,
    /// `CLAUDE_CODE_MAX_CONTEXT_TOKENS` do processo ou `context_window` do sidecar.
    pub declared: Option<u64>,
    pub engine: bool,
}

/// Contexto e modelo em uso da sessão, numa leitura só do transcript.
pub fn claude_reading(inputs: &ReadingInputs) -> Reading {
    let model = non_empty(inputs.chosen).or(non_empty(inputs.opened));
    let (ctx, answered) = read(inputs.jsonl, inputs.account_dir, model, inputs.declared);
    let used = ctx.map_or(0, |c| c.used);
    (ctx, session_model(answered.as_deref(), model, inputs.account_dir, used, inputs.engine))
}

/// Pasta de configuração da conta (`conta` = "claude:<pasta>").
pub fn config_dir_of(conta: Option<&str>) -> Option<PathBuf> {
    conta.and_then(|c| c.strip_prefix("claude:")).filter(|d| !d.is_empty()).map(PathBuf::from)
}

/// `--model` do argv como `procinfo._model_of`: o cmdline juntado e quebrado por espaço.
pub fn opened_model(argv: &[String]) -> Option<String> {
    let joined = argv.join(" ");
    let words: Vec<&str> = joined.split_whitespace().collect();
    let i = words.iter().position(|w| *w == "--model")?;
    words.get(i + 1).map(|w| (*w).to_owned())
}

/// Janela declarada em texto: só dígitos, senão nada.
pub fn declared_window(raw: &str) -> Option<u64> {
    if raw.is_empty() || !raw.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    raw.parse().ok()
}

/// Janela declarada no sidecar: inteiro sem sinal ou texto de dígitos (o `str(x).isdigit()`).
pub fn declared_window_value(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => declared_window(s),
        _ => None,
    }
}

/// Contexto da última resposta do agente principal e o id do modelo que a deu.
pub fn read(jsonl: &Path, account_dir: &Path, model: Option<&str>, window_tokens: Option<u64>) -> Reading {
    let measured = measured(jsonl).map(|mut c| {
        if let Some(declared) = window_tokens.filter(|n| *n > 0) { c.window = declared; }
        c
    });
    let tail = match read_tail(jsonl) {
        Ok(tail) => tail,
        // Sessão que acabou de fechar; o cache guarda o último valor do mesmo transcript.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (None, None),
        Err(e) => {
            // Calado, o contexto ficaria velho ou cairia na janela de 200k sem rastro.
            let sid = jsonl.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            super::facts::note("rust.list_context_unreadable", &sid, format!("list_context_unreadable:{:?}", e.kind()),
                "rabo do transcript ilegível; o contexto fica no último valor");
            return (None, None);
        }
    };
    for line in tail.split(|b| *b == b'\n').rev() {
        if USAGE.find(line).is_none() {
            continue;
        }
        let Ok(Value::Object(obj)) = serde_json::from_slice::<Value>(line) else { continue };
        // Subagente roda noutro contexto; a resposta sintética (erro, interrupção) não tem uso real.
        if obj.get("type").and_then(Value::as_str) != Some("assistant") || obj.get("isSidechain").is_some_and(truthy) {
            continue;
        }
        let Some(Value::Object(message)) = obj.get("message") else { continue };
        let Some(Value::Object(usage)) = message.get("usage") else { continue };
        if message.get("model").and_then(Value::as_str) == Some("<synthetic>") {
            continue;
        }
        let used: i128 = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"]
            .iter()
            .map(|k| usage.get(*k).map_or(0, as_int))
            .sum();
        if used > 0 {
            let used = u64::try_from(used).unwrap_or(u64::MAX);
            let answered = message.get("model").and_then(Value::as_str).filter(|m| !m.is_empty());
            let ctx = ContextUse { used, window: measured.map_or_else(|| window(used, account_dir, model, window_tokens), |c| c.window) };
            return (Some(ctx), answered.map(str::to_owned));
        }
    }
    (measured, None)
}

/// O plugin publica a janela medida pelo Claude, inclusive com a statusline personalizada.
fn measured(jsonl: &Path) -> Option<ContextUse> {
    let raw = std::fs::read(jsonl.with_extension("context.json")).ok()?;
    let value: Value = serde_json::from_slice(&raw).ok()?;
    let used = value.get("used")?.as_u64().filter(|n| *n > 0)?;
    let window = value.get("window")?.as_u64().filter(|n| *n > 0)?;
    Some(ContextUse { used, window })
}

/// Id do modelo em uso: o da última resposta, senão o da abertura e por fim o `model` da conta.
/// O `[1m]` volta quando o uso só cabe nele ou quando o configurado é a variante de 1M da mesma
/// família. Sessão de motor não cai na conta, que guarda o modelo da Anthropic.
pub fn session_model(answered: Option<&str>, opened: Option<&str>, account_dir: &Path, used: u64, engine: bool) -> Option<String> {
    let configured = match non_empty(opened) {
        Some(m) => Some(m.to_owned()),
        None if engine => None,
        None => account_model(account_dir),
    };
    let Some(answered) = non_empty(answered) else {
        return configured.map(|c| DATED.replace(&c, "").into_owned());
    };
    let base = DATED.replace(answered, "").into_owned();
    let Some(family) = family(&base) else { return Some(base) };
    if base.to_lowercase().ends_with("[1m]") {
        return Some(base);
    }
    let same_1m = configured.as_deref().is_some_and(|c| c.to_lowercase().ends_with("[1m]") && family_of_is(c, &family));
    Some(if used > WINDOW_DEFAULT || same_1m { format!("{base}[1m]") } else { base })
}

/// 1M quando o modelo é a variante `[1m]` ou quando o uso já passou da janela padrão; o modelo da
/// sessão vence o da conta, e a janela declarada vence os dois.
pub fn window(used: u64, account_dir: &Path, model: Option<&str>, window_tokens: Option<u64>) -> u64 {
    if let Some(w) = window_tokens.filter(|w| *w > 0) {
        return w;
    }
    let is_1m = match non_empty(model) {
        Some(m) => m.to_lowercase().ends_with("[1m]"),
        None => account_model(account_dir).is_some_and(|m| m.to_lowercase().ends_with("[1m]")),
    };
    if used > WINDOW_DEFAULT || is_1m { WINDOW_1M } else { WINDOW_DEFAULT }
}

/// Leitura por sessão com prazo de 20 s por (nome, transcript).
#[derive(Default)]
pub struct ContextCache {
    entries: Capped<String, Entry>,
}

struct Entry {
    at: f64,
    jsonl: String,
    ctx: Option<ContextUse>,
    model: Option<String>,
    files: SourceVersion,
}

#[derive(PartialEq)]
pub struct SourceVersion {
    transcript: Option<super::facts_files::FileKey>,
    measurement: Option<super::facts_files::FileKey>,
}

/// Capture antes de ler: uma escrita durante a leitura precisa invalidar o resultado guardado.
pub fn source_version(jsonl: &str) -> SourceVersion {
    let path = Path::new(jsonl);
    let key = |p: &Path| std::fs::metadata(p).ok().and_then(|m| super::facts_files::file_key(&m));
    SourceVersion { transcript: key(path), measurement: key(&path.with_extension("context.json")) }
}

/// Em duas pontas para o chamador ler os transcripts vencidos em paralelo, fora do lock, como o
/// `gather` do Python. Só entra sessão Claude com transcript (o chamador filtra o vazio).
impl ContextCache {
    /// `now` é relógio monotônico em segundos.
    pub fn stale(&self, name: &str, jsonl: &str, now: f64) -> bool {
        !self.entries.peek(name).is_some_and(|e| {
            if e.jsonl != jsonl || now - e.at > TTL_SECS { return false; }
            let files = source_version(jsonl);
            // A primeira resposta e a janela medida chegam sem esperar; uso já conhecido mantém
            // a cadência de leitura do transcript.
            e.files.measurement == files.measurement && (e.ctx.is_some() || e.files.transcript == files.transcript)
        })
    }

    /// Grava a leitura nova e devolve o valor que a linha mostra.
    pub fn store(&mut self, name: &str, jsonl: &str, now: f64, reading: Reading, files: SourceVersion) -> Reading {
        let (ctx, mut model) = reading;
        // Sem resposta lida, o valor anterior só vale para o MESMO transcript; o modelo também,
        // senão a pílula cai no da conta quando a resposta sai do trecho lido.
        let before = self.entries.remove(name).filter(|e| e.jsonl == jsonl);
        let kept = before.as_ref().and_then(|e| e.ctx);
        if ctx.is_none() {
            if let Some(prev) = before.and_then(|e| e.model).filter(|m| !m.is_empty()) {
                model = Some(prev);
            }
        }
        let ctx = ctx.or(kept);
        self.entries.insert(name.to_owned(), Entry { at: now, jsonl: jsonl.to_owned(), ctx, model: model.clone(), files });
        (ctx, model)
    }

    /// Valor guardado, só se foi lido do mesmo transcript.
    pub fn cached(&self, name: &str, jsonl: &str) -> Reading {
        match self.entries.peek(name) {
            Some(e) if e.jsonl == jsonl => (e.ctx, e.model.clone()),
            _ => (None, None),
        }
    }

    pub fn forget(&mut self, name: &str) {
        self.entries.remove(name);
    }
}

fn read_tail(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut fh = std::fs::File::open(path)?;
    let end = fh.seek(SeekFrom::End(0))?;
    let start = end.saturating_sub(TAIL);
    fh.seek(SeekFrom::Start(start))?;
    let mut buf = Vec::with_capacity(usize::try_from(end - start).unwrap_or(0));
    fh.read_to_end(&mut buf)?;
    Ok(buf)
}

fn account_model(account_dir: &Path) -> Option<String> {
    // Conta sem settings.json é normal.
    let raw = std::fs::read(account_dir.join("settings.json")).ok()?;
    match serde_json::from_slice::<Value>(&raw) {
        Ok(Value::Object(obj)) => obj.get("model").and_then(Value::as_str).filter(|m| !m.is_empty()).map(str::to_owned),
        // settings.json quebrado deixaria a pílula do modelo em branco sem rastro.
        Ok(_) => {
            settings_rejected(account_dir, "object".into());
            None
        }
        Err(e) => {
            settings_rejected(account_dir, format!("{:?}", e.classify()));
            None
        }
    }
}

/// Lido a cada rodada por sessão: aviso com o teto do `warn_limit`, no log e no diário.
fn settings_rejected(account_dir: &Path, field: String) {
    let account = account_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if crate::warn_limit::allow(Some(&account), "list_settings_invalid") {
        tracing::warn!(dir = %account_dir.display(), field, "settings.json da conta ilegível");
    }
    super::facts::note("rust.list_file_rejected", &account, format!("list_settings_invalid:{field}"), "settings.json da conta ilegível");
}

fn family(model: &str) -> Option<String> {
    FAMILY.captures(model).map(|c| c[1].to_lowercase())
}

fn family_of_is(model: &str, fam: &str) -> bool {
    family(model).as_deref() == Some(fam)
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.filter(|s| !s.is_empty())
}

/// `int(x or 0)` do Python sobre um valor do `usage`. Divergência deliberada: texto que não é
/// número vale 0 (o Python levantaria e derrubaria a lista inteira).
fn as_int(v: &Value) -> i128 {
    match v {
        Value::Number(n) => n.as_i128().or_else(|| n.as_f64().map(|f| f as i128)).unwrap_or(0),
        Value::Bool(b) => i128::from(*b),
        Value::String(s) => s.trim().parse().unwrap_or_else(|_| {
            tracing::debug!("usage com texto que não é número");
            0
        }),
        _ => 0,
    }
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn last_main_answer_wins_and_unreadable_tail_reaches_the_diary() {
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("ctx-ok.jsonl");
        std::fs::write(&jsonl, concat!(
            r#"{"type": "assistant", "message": {"model": "claude-haiku-4-5", "usage": {"input_tokens": 10}}}"#, "\n",
            r#"{"type": "assistant", "isSidechain": true, "message": {"usage": {"input_tokens": 99}}}"#, "\n")).unwrap();
        let (ctx, model) = read(&jsonl, tmp.path(), None, None);
        assert_eq!((ctx.map(|c| (c.used, c.window)), model.as_deref()), (Some((10, WINDOW_DEFAULT)), Some("claude-haiku-4-5")));
        // Pasta no lugar do transcript: a leitura falha (o Windows nem abre).
        #[cfg(unix)]
        {
            let dir = tmp.path().join("ctx-pasta.jsonl");
            std::fs::create_dir(&dir).unwrap();
            assert_eq!(read(&dir, tmp.path(), None, None), (None, None));
            let notes = super::super::facts::notes_for("ctx-pasta");
            assert!(matches!(&notes[..], [n] if n.starts_with("list_context_unreadable:")), "{notes:?}");
        }
    }
}
