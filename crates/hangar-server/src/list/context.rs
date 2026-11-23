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
    claude_reading_from(inputs, &scan_tail(inputs.jsonl))
}

/// Como `claude_reading`, a partir do rabo já varrido (`scan_tail` ou `ContextCache::scan`).
pub fn claude_reading_from(inputs: &ReadingInputs, scan: &Scan) -> Reading {
    reading_with(inputs, scan, None)
}

fn reading_with(inputs: &ReadingInputs, scan: &Scan, learned: Option<u64>) -> Reading {
    let model = non_empty(inputs.chosen).or(non_empty(inputs.opened));
    let (ctx, answered) = assemble(scan, inputs.jsonl, inputs.account_dir, model, inputs.declared, learned);
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

/// Uso e modelo da última resposta principal no rabo do transcript. `None` = rabo ilegível ou
/// ausente; `Some(None)` = nenhuma resposta com uso no trecho lido.
pub type Scan = Option<Option<(u64, Option<String>)>>;

/// Contexto da última resposta do agente principal e o id do modelo que a deu.
pub fn read(jsonl: &Path, account_dir: &Path, model: Option<&str>, window_tokens: Option<u64>) -> Reading {
    assemble(&scan_tail(jsonl), jsonl, account_dir, model, window_tokens, None)
}

/// A janela depende do modelo e da declaração, que mudam sem o transcript mudar: fica fora da varredura.
fn assemble(scan: &Scan, jsonl: &Path, account_dir: &Path, model: Option<&str>, window_tokens: Option<u64>, learned: Option<u64>) -> Reading {
    let Some(found) = scan else { return (None, None) };
    let measured = measured(jsonl).map(|mut c| {
        if let Some(declared) = window_tokens.filter(|n| *n > 0) { c.window = declared; }
        c
    });
    match found {
        Some((used, answered)) => {
            let ctx = ContextUse { used: *used, window: measured.map_or_else(|| window(*used, account_dir, model, window_tokens, learned), |c| c.window) };
            (Some(ctx), answered.clone())
        }
        None => (measured, None),
    }
}

pub fn scan_tail(jsonl: &Path) -> Scan {
    let tail = match read_tail(jsonl) {
        Ok(tail) => tail,
        // Sessão que acabou de fechar; o cache guarda o último valor do mesmo transcript.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            // Calado, o contexto ficaria velho ou cairia na janela de 200k sem rastro.
            let sid = jsonl.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            super::facts::note("rust.list_context_unreadable", &sid, format!("list_context_unreadable:{:?}", e.kind()),
                "rabo do transcript ilegível; o contexto fica no último valor");
            return None;
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
            return Some(Some((used, answered.map(str::to_owned))));
        }
    }
    Some(None)
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

/// 1M quando o modelo é a variante `[1m]`; senão a janela que o Claude já informou para o modelo
/// respondido (`learned`), e por fim o uso decide. O modelo da sessão vence o da conta, e a janela
/// declarada vence tudo.
pub fn window(used: u64, account_dir: &Path, model: Option<&str>, window_tokens: Option<u64>, learned: Option<u64>) -> u64 {
    if let Some(w) = window_tokens.filter(|w| *w > 0) {
        return w;
    }
    let is_1m = match non_empty(model) {
        Some(m) => m.to_lowercase().ends_with("[1m]"),
        None => account_model(account_dir).is_some_and(|m| m.to_lowercase().ends_with("[1m]")),
    };
    if is_1m { return WINDOW_1M; }
    // O alias `opus` não leva `[1m]` e já nasce em 1M: a primeira janela informada só chega no fim do turno.
    if let Some(w) = learned.filter(|w| *w >= used) { return w; }
    if used > WINDOW_DEFAULT { WINDOW_1M } else { WINDOW_DEFAULT }
}

/// Leitura por sessão com prazo de 20 s por (nome, transcript).
#[derive(Default)]
pub struct ContextCache {
    entries: Capped<String, Entry>,
    /// Janela que o Claude informou por conta e modelo respondido (sem a data). Fica em disco e sem
    /// prazo: o informe só chega no fim do turno, e a sessão nova não pode cair nos 200k porque o
    /// servidor subiu ou a conta ficou parada. Troca de plano se corrige no informe seguinte.
    windows: std::collections::HashMap<String, u64>,
    /// Aprendido que ainda não chegou ao disco. Só ele é gravado: o resto do mapa pode ser mais velho que o arquivo.
    unsaved: std::collections::HashMap<String, u64>,
    windows_file: Option<PathBuf>,
}

struct Entry {
    at: f64,
    jsonl: String,
    ctx: Option<ContextUse>,
    model: Option<String>,
    files: SourceVersion,
    /// Varredura do rabo desta versão do transcript: vencido o prazo sem o arquivo mudar, só o
    /// modelo e a janela são refeitos. `None` = não guardada.
    scan: Option<Scan>,
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
    /// Como `claude_reading_from`, aprendendo e aplicando a janela informada por modelo respondido.
    /// `declared_reported`: o `declared` veio do Claude (sidecar da sessão sem terminal), não do limite que o
    /// usuário pôs no ambiente.
    pub fn reading(&mut self, inputs: &ReadingInputs, scan: &Scan, declared_reported: bool) -> Reading {
        // Por conta: o plano dela decide a janela do mesmo modelo.
        let key = match scan {
            Some(Some((_, Some(m)))) => Some(format!("{}\n{}", inputs.account_dir.display(), DATED.replace(m, "").to_lowercase())),
            _ => None,
        };
        // Só janela que o Claude informou ensina; a deduzida aqui se perpetuaria. Motor declara a
        // janela dele, que não vale para o mesmo id na conta Anthropic; `[1m]` pedido é variante, não o padrão.
        let variant = [inputs.chosen, inputs.opened].into_iter().flatten().any(|m| m.to_lowercase().ends_with("[1m]"));
        let reported = if inputs.engine || variant { None } else {
            inputs.declared.filter(|w| declared_reported && *w > 0).or_else(|| measured(inputs.jsonl).map(|c| c.window))
        };
        if let (Some(k), Some(w)) = (&key, reported) && self.windows.get(k) != Some(&w) {
            self.windows.insert(k.clone(), w);
            self.unsaved.insert(k.clone(), w);
        }
        if !self.unsaved.is_empty() { self.save_windows(); }
        let learned = key.and_then(|k| self.windows.get(&k).copied());
        reading_with(inputs, scan, learned)
    }

    /// Arquivo da janela aprendida; lido na primeira vez, e o já aprendido nesta vida vence o lido.
    pub fn keep_windows_at(&mut self, path: PathBuf) {
        if self.windows_file.as_ref() == Some(&path) { return; }
        // Sem marcar o arquivo: gravar agora apagaria o que ele guarda; o próximo tique relê.
        let Some(saved) = read_windows(&path) else { return };
        for (key, window) in saved { self.windows.entry(key).or_insert(window); }
        self.windows_file = Some(path);
    }

    /// Relê antes de gravar: na atualização o servidor velho e o novo gravam o mesmo arquivo.
    fn save_windows(&mut self) {
        let Some(path) = &self.windows_file else { return };
        let Some(mut merged) = read_windows(path) else { return };
        merged.extend(self.unsaved.iter().map(|(k, w)| (k.clone(), *w)));
        let saved = serde_json::to_vec(&merged).map_err(std::io::Error::from).and_then(|bytes| {
            path.parent().map_or(Ok(()), std::fs::create_dir_all)?;
            hangar_core::fs::atomic_write(path, &bytes)
        });
        match saved {
            Ok(()) => self.unsaved.clear(),
            Err(e) => super::facts::note("rust.list_context_windows_unwritable", "", format!("list_context_windows_unwritable:{:?}", e.kind()),
                "janela aprendida não gravada; depois de o servidor subir a sessão nova volta a mostrar 200k"),
        }
    }

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
    /// Rabo do transcript: o guardado se for da mesma versão do arquivo, senão lido agora.
    pub fn scan(&self, name: &str, jsonl: &str, files: &SourceVersion) -> Scan {
        match self.entries.peek(name) {
            // Rabo ilegível não fica guardado: a próxima rodada tenta de novo e avisa de novo.
            Some(Entry { jsonl: j, files: f, scan: Some(scan @ Some(_)), .. })
                if j == jsonl && files.transcript.is_some() && f.transcript == files.transcript => scan.clone(),
            _ => scan_tail(Path::new(jsonl)),
        }
    }

    pub fn store(&mut self, name: &str, jsonl: &str, now: f64, reading: Reading, files: SourceVersion) -> Reading {
        self.store_with(name, jsonl, now, reading, files, None)
    }

    /// `store` guardando também a varredura do rabo, para `scan` reaproveitar.
    pub fn store_with(&mut self, name: &str, jsonl: &str, now: f64, reading: Reading, files: SourceVersion, scan: Option<Scan>) -> Reading {
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
        self.entries.insert(name.to_owned(), Entry { at: now, jsonl: jsonl.to_owned(), ctx, model: model.clone(), files, scan });
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

/// `None` só quando o arquivo existe e não deu para ler; ilegível conta como vazio, porque regravar não perde nada.
fn read_windows(path: &std::path::Path) -> Option<std::collections::HashMap<String, u64>> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Some(Default::default()),
        Err(e) => {
            super::facts::note("rust.list_context_windows_unreadable", "", format!("list_context_windows_unreadable:{:?}", e.kind()),
                "janela aprendida não lida; a sessão nova mostra 200k até o fim do primeiro turno");
            return None;
        }
    };
    Some(serde_json::from_slice(&raw).unwrap_or_else(|_| {
        super::facts::note("rust.list_context_windows_corrupt", "", "list_context_windows_corrupt".to_string(),
            "arquivo da janela aprendida ilegível; o próximo informe o regrava");
        Default::default()
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_entry_reuses_the_tail_scan_until_the_transcript_changes() {
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("same.jsonl");
        std::fs::write(&jsonl, r#"{"type": "assistant", "message": {"usage": {"input_tokens": 10}}}"#).unwrap();
        let path = jsonl.to_str().unwrap();
        let mut cache = ContextCache::default();
        let marked: Scan = Some(Some((42, None)));
        cache.store_with("s", path, 0.0, (None, None), source_version(path), Some(marked.clone()));
        assert!(cache.stale("s", path, TTL_SECS + 1.0), "o prazo ainda vence: modelo e janela são refeitos");
        assert_eq!(cache.scan("s", path, &source_version(path)), marked, "arquivo igual não é relido");
        std::fs::write(&jsonl, r#"{"type": "assistant", "message": {"usage": {"input_tokens": 110}}}"#).unwrap();
        assert_eq!(cache.scan("s", path, &source_version(path)), Some(Some((110, None))));
    }

    #[test]
    fn window_reported_for_a_model_reaches_a_new_session_of_the_same_model() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.json"), r#"{"model": "opus"}"#).unwrap();
        let answer = |name: &str, model: &str, used: u64| {
            let path = tmp.path().join(name);
            std::fs::write(&path, format!(r#"{{"type":"assistant","message":{{"model":"{model}","usage":{{"input_tokens":{used}}}}}}}"#)).unwrap();
            path
        };
        let (old, new, other) = (answer("old.jsonl", "claude-opus-5-5", 50_000), answer("new.jsonl", "claude-opus-5-5", 186_979),
            answer("other.jsonl", "claude-sonnet-5-5", 1_000));
        let inputs = |jsonl, declared| ReadingInputs { jsonl, account_dir: tmp.path(), chosen: None, opened: None, declared, engine: false };
        let file = tmp.path().join("hangar/context-windows.json");
        let mut cache = ContextCache::default();
        cache.keep_windows_at(file.clone());
        // Sessão sem terminal que já terminou um turno: o sidecar guarda a janela do `result`.
        cache.reading(&inputs(&old, Some(WINDOW_1M)), &scan_tail(&old), true);
        let (ctx, _) = cache.reading(&inputs(&new, None), &scan_tail(&new), false);
        assert_eq!(ctx.map(|c| (c.used, c.window)), Some((186_979, WINDOW_1M)), "a nova, no meio do primeiro turno");
        let (ctx, _) = cache.reading(&inputs(&other, None), &scan_tail(&other), false);
        assert_eq!(ctx.map(|c| c.window), Some(WINDOW_DEFAULT), "outro modelo não herda");
        // Servidor que subiu de novo: a sessão nova já nasce na janela informada antes.
        let mut restarted = ContextCache::default();
        restarted.keep_windows_at(file.clone());
        let (ctx, _) = restarted.reading(&inputs(&new, None), &scan_tail(&new), false);
        assert_eq!(ctx.map(|c| c.window), Some(WINDOW_1M), "o aprendido sobrevive à subida do servidor");
        // Plano trocado: o informe seguinte substitui, também no arquivo.
        restarted.reading(&inputs(&old, Some(WINDOW_DEFAULT)), &scan_tail(&old), true);
        let mut again = ContextCache::default();
        again.keep_windows_at(file);
        let (ctx, _) = again.reading(&inputs(&new, None), &scan_tail(&new), false);
        assert_eq!(ctx.map(|c| c.window), Some(WINDOW_DEFAULT), "o informe mais novo vence");
    }

    /// Arquivo ilegível na hora do informe: o aprendido vai ao disco quando ele volta, mesmo com o informe repetido.
    #[test]
    fn window_learned_while_the_file_was_unreadable_is_saved_later() {
        let tmp = tempfile::tempdir().unwrap();
        let jsonl = tmp.path().join("a.jsonl");
        std::fs::write(&jsonl, r#"{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":1000}}}"#).unwrap();
        let inputs = |declared| ReadingInputs { jsonl: &jsonl, account_dir: tmp.path(), chosen: None, opened: None, declared, engine: false };
        let file = tmp.path().join("context-windows.json");
        // Pasta no lugar do arquivo: ler falha, e não por falta dele.
        std::fs::create_dir(&file).unwrap();
        let mut cache = ContextCache::default();
        cache.keep_windows_at(file.clone());
        cache.reading(&inputs(Some(WINDOW_1M)), &scan_tail(&jsonl), true);
        std::fs::remove_dir(&file).unwrap();
        cache.keep_windows_at(file.clone());
        cache.reading(&inputs(Some(WINDOW_1M)), &scan_tail(&jsonl), true);
        let mut restarted = ContextCache::default();
        restarted.keep_windows_at(file);
        let (ctx, _) = restarted.reading(&inputs(None), &scan_tail(&jsonl), false);
        assert_eq!(ctx.map(|c| c.window), Some(WINDOW_1M));
    }

    /// Na atualização o servidor velho e o novo gravam o mesmo arquivo: um não apaga o que o outro aprendeu.
    #[test]
    fn two_servers_writing_the_window_file_keep_each_others_windows() {
        let tmp = tempfile::tempdir().unwrap();
        let answer = |name: &str, model: &str| {
            let path = tmp.path().join(name);
            std::fs::write(&path, format!(r#"{{"type":"assistant","message":{{"model":"{model}","usage":{{"input_tokens":1000}}}}}}"#)).unwrap();
            path
        };
        let (opus, sonnet) = (answer("opus.jsonl", "claude-opus-5-5"), answer("sonnet.jsonl", "claude-sonnet-5-5"));
        let inputs = |jsonl, declared| ReadingInputs { jsonl, account_dir: tmp.path(), chosen: None, opened: None, declared, engine: false };
        let file = tmp.path().join("hangar/context-windows.json");
        let (mut old, mut new) = (ContextCache::default(), ContextCache::default());
        old.keep_windows_at(file.clone());
        new.keep_windows_at(file.clone());
        old.reading(&inputs(&opus, Some(WINDOW_1M)), &scan_tail(&opus), true);
        new.reading(&inputs(&sonnet, Some(WINDOW_1M)), &scan_tail(&sonnet), true);
        let mut later = ContextCache::default();
        later.keep_windows_at(file);
        for jsonl in [&opus, &sonnet] {
            let (ctx, _) = later.reading(&inputs(jsonl, None), &scan_tail(jsonl), false);
            assert_eq!(ctx.map(|c| c.window), Some(WINDOW_1M), "{}", jsonl.display());
        }
    }

    /// O limite que o usuário declara no ambiente (`CLAUDE_CODE_MAX_CONTEXT_TOKENS`) é escolha dele para aquela
    /// sessão, não a janela do modelo: não ensina as outras.
    #[test]
    fn window_declared_by_the_user_does_not_teach_other_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("settings.json"), r#"{"model": "opus"}"#).unwrap();
        let answer = |name: &str, used: u64| {
            let path = tmp.path().join(name);
            std::fs::write(&path, format!(r#"{{"type":"assistant","message":{{"model":"claude-opus-5-5","usage":{{"input_tokens":{used}}}}}}}"#)).unwrap();
            path
        };
        let (limited, fresh) = (answer("limited.jsonl", 50_000), answer("fresh.jsonl", 60_000));
        let inputs = |jsonl, declared| ReadingInputs { jsonl, account_dir: tmp.path(), chosen: None, opened: None, declared, engine: false };
        let mut cache = ContextCache::default();
        let (ctx, _) = cache.reading(&inputs(&limited, Some(500_000)), &scan_tail(&limited), false);
        assert_eq!(ctx.map(|c| c.window), Some(500_000), "a própria sessão segue o que declarou");
        let (ctx, _) = cache.reading(&inputs(&fresh, None), &scan_tail(&fresh), false);
        assert_eq!(ctx.map(|c| c.window), Some(WINDOW_DEFAULT), "a outra não herda o limite declarado");
    }

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
