//! Políticas puras do ator (preparar o prompt, formatar a linha de status, achar a skill do Codex).
//! Saem byte a byte como as funções Python de onde vieram; o golden em
//! `backend/tests/fixtures/contract/local_policy` prova a paridade. Nada aqui fala com o Python.
use super::protocol::RuntimeError;
use indexmap::IndexMap;
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use std::sync::LazyLock;

/// Teto da API por imagem; acima disso fica só o path (o `Read` do agente abre).
const IMAGE_CAP: u64 = 5 * 1024 * 1024;
const FAMILIES: [&str; 4] = ["opus", "sonnet", "haiku", "fable"];

static NATIVE_PREFIX: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?s)^\[(de|grupo|painel):\s*([^\]]+)\]\s*").unwrap());

fn error(code: &str) -> RuntimeError { RuntimeError::new(code, "entrada de serviço inválida") }

pub fn is_local(kind: &str) -> bool {
    matches!(kind, "prepare_prompt" | "format_status" | "skill_catalog" | "last_usage" | "reload_stamp" | "unknown_private")
}

/// `None` = o serviço não é local. `meta["provider"]` escolhe Claude ou Codex; `quota` é o
/// `{"windows":[…]}` do `GET /internal/quota` (só Claude).
pub fn run(kind: &str, payload: &Value, meta: &Value, quota: Option<&Value>) -> Option<Result<Value, RuntimeError>> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |elapsed| elapsed.as_secs_f64());
    run_at(kind, payload, meta, quota, now)
}

/// Igual a `run` com o relógio dado: a idade das janelas de cota depende da hora.
pub fn run_at(kind: &str, payload: &Value, meta: &Value, quota: Option<&Value>, now: f64) -> Option<Result<Value, RuntimeError>> {
    let provider = meta["provider"].as_str().unwrap_or("");
    Some(match kind {
        "prepare_prompt" => prepare_prompt(payload, provider),
        "format_status" => match provider {
            "claude" => claude_status(payload, meta, quota, now),
            "codex" => Ok(codex_status(payload, now)),
            _ => Err(error("policy_provider")),
        },
        "skill_catalog" => skill_catalog(payload),
        "last_usage" => last_usage(meta),
        "reload_stamp" => reload_stamp(meta),
        "unknown_private" => unknown_private(payload, meta, now),
        _ => return None,
    })
}

// Os predicados do Python (`str.isspace`, truthiness) não coincidem com os do Rust em alguns
// casos de borda; o golden depende de os dois lados concordarem.
fn py_space(c: char) -> bool { c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c) }

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(flag) => *flag,
        Value::Number(number) => number.as_f64().is_some_and(|n| n != 0.0),
        Value::String(text) => !text.is_empty(),
        Value::Array(items) => !items.is_empty(),
        Value::Object(fields) => !fields.is_empty(),
    }
}

fn text(value: &Value) -> Option<&str> { value.as_str().filter(|text| !text.is_empty()) }

// ---- prepare_prompt ----

fn prepare_prompt(payload: &Value, provider: &str) -> Result<Value, RuntimeError> {
    let text = payload["text"].as_str().ok_or_else(|| error("policy_input"))?;
    match provider {
        "claude" => {
            let (content, notices) = prompt_blocks(text);
            Ok(json!({"content": content, "notices": notices, "native_candidate": NATIVE_PREFIX.is_match(text)}))
        }
        "codex" => {
            // O nome é o que vem depois da barra da primeira palavra (`split()[0][1:]`).
            let trimmed = text.trim_start_matches(py_space);
            let name = trimmed.starts_with('/').then(|| trimmed.split(py_space).next().unwrap_or("")[1..].to_owned());
            Ok(json!({"input": [{"type": "text", "text": text}], "skill_name": name}))
        }
        _ => Err(error("policy_provider")),
    }
}

/// Trechos depois de `📎 imagem:` até o próximo `📎` ou o fim da linha, como o
/// `📎\s*imagem:\s*(.+?)(?=\s*📎|$)` multilinha do Python (o `regex` não tem lookahead).
fn marker_captures(text: &str) -> Vec<String> {
    const LABEL: [char; 7] = ['i', 'm', 'a', 'g', 'e', 'm', ':'];
    let chars: Vec<char> = text.chars().collect();
    let ends_at = |p: usize| -> bool {
        if p == chars.len() || chars[p] == '\n' { return true; }
        let mut k = p;
        while k < chars.len() && py_space(chars[k]) { k += 1; }
        chars.get(k) == Some(&'📎')
    };
    let (mut out, mut i) = (Vec::new(), 0);
    while i < chars.len() {
        if chars[i] != '📎' { i += 1; continue; }
        let mut j = i + 1;
        while j < chars.len() && py_space(chars[j]) { j += 1; }
        if !chars[j..].starts_with(&LABEL) { i += 1; continue; }
        let label_end = j + LABEL.len();
        let mut blanks_end = label_end;
        while blanks_end < chars.len() && py_space(chars[blanks_end]) { blanks_end += 1; }
        // O `\s*` guloso devolve espaço quando o `.+?` não tem outro caractere (e `.` não casa `\n`).
        let Some(start) = (label_end..=blanks_end).rev().find(|&s| chars.get(s).is_some_and(|c| *c != '\n')) else { i += 1; continue; };
        let end = (start + 1..=chars.len()).find(|&p| ends_at(p)).unwrap_or(chars.len());
        out.push(chars[start..end].iter().collect());
        i = end;
    }
    out
}

fn image_mime(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_string_lossy().to_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// O trecho inteiro, depois só a primeira palavra; pontuação colada no fim não conta.
fn image_path(excerpt: &str) -> Option<&Path> {
    let whole = excerpt.trim_matches(py_space);
    let first = excerpt.split(py_space).find(|word| !word.is_empty()).unwrap_or("");
    [whole, first].into_iter()
        .map(|candidate| candidate.trim_end_matches(['.', ',', ';', ':', ')']))
        .find(|candidate| !candidate.is_empty() && image_mime(Path::new(candidate)).is_some())
        .map(Path::new)
}

fn strerror(error: &std::io::Error) -> String {
    // O Python usa o `strerror` da libc também no Windows; a frase do FormatMessage seria outra.
    match error.kind() {
        std::io::ErrorKind::NotFound => return "No such file or directory".into(),
        std::io::ErrorKind::PermissionDenied => return "Permission denied".into(),
        _ => {}
    }
    let text = error.to_string();
    text.split(" (os error ").next().unwrap_or(&text).to_owned()
}

fn prompt_blocks(text: &str) -> (Vec<Value>, Vec<String>) {
    use base64::Engine;
    let mut blocks = vec![json!({"type": "text", "text": text})];
    let mut notices = Vec::new();
    for excerpt in marker_captures(text) {
        let Some(path) = image_path(&excerpt) else { continue };
        let Some(mime) = image_mime(path) else { continue };
        let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        let mut data = Vec::new();
        let opened = std::fs::File::open(path).and_then(|mut file| {
            file.by_ref().take(IMAGE_CAP + 1).read_to_end(&mut data)?;
            file.metadata().map(|meta| meta.len())
        });
        match opened {
            Err(error) => notices.push(format!("⚠️ Imagem não anexada (não abre: {}); só o path foi: {name}", strerror(&error))),
            Ok(size) if data.len() as u64 > IMAGE_CAP => notices.push(format!(
                "⚠️ Imagem não anexada ({} MB, teto 5 MB); só o path foi: {name}", size.max(data.len() as u64) / (1024 * 1024))),
            Ok(_) => blocks.push(json!({"type": "image", "source": {"type": "base64", "media_type": mime,
                "data": base64::engine::general_purpose::STANDARD.encode(&data)}})),
        }
    }
    (blocks, notices)
}

// ---- format_status ----

/// `10k`, `2M`: o arredondamento do Python é para o par (2,5 -> 2), o que `round` do Rust não faz.
fn fmt_tok(n: f64) -> String {
    let n = n.round_ties_even() as i64;
    if n.abs() >= 1_000_000 { format!("{}M", (n as f64 / 1_000_000.0).round_ties_even() as i64) }
    else if n.abs() >= 1_000 { format!("{}k", (n as f64 / 1_000.0).round_ties_even() as i64) }
    else { n.to_string() }
}

/// Tempo relativo curto até o reset (`2d1h`, `34m`).
fn format_reset(resets_at: f64, now: f64) -> String {
    let delta = ((resets_at - now) as i64).max(0);
    let (days, rest) = (delta / 86400, delta % 86400);
    let (hours, minutes) = (rest / 3600, rest % 3600 / 60);
    if days > 0 { if hours > 0 { format!("{days}d{hours}h") } else { format!("{days}d") } }
    else if hours > 0 { if minutes > 0 { format!("{hours}h{minutes}m") } else { format!("{hours}h") } }
    else { format!("{minutes}m") }
}

/// `claude-opus-5[1m]` -> `Opus5·1M`; id fora das famílias conhecidas (motor, alias) passa como veio.
fn model_label(model: &str) -> String {
    let mut base = model.trim_matches(py_space);
    let one_m = base.to_lowercase().ends_with("[1m]");
    if one_m { base = &base[..base.char_indices().rev().nth(3).map_or(0, |(at, _)| at)]; }
    let lower = base.to_lowercase();
    let parts: Vec<&str> = lower.strip_prefix("claude-").unwrap_or(&lower).split('-').collect();
    if !FAMILIES.contains(&parts[0]) { return model.to_owned(); }
    let version = parts[1..].iter()
        .filter(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()) && part.chars().count() < 8)
        .copied().collect::<Vec<_>>().join(".");
    let family = format!("{}{}", parts[0][..1].to_uppercase(), &parts[0][1..]);
    let label = if family == "Opus" { format!("{family}{version}") } else { format!("{family} {version}") };
    format!("{}{}", label.trim_matches(py_space), if one_m { "·1M" } else { "" })
}

/// Esforço que a CLI usa quando a sessão não escolheu: env, depois o `settings.json` da conta.
fn default_effort(config_dir: &Value) -> Option<String> {
    if let Some(level) = std::env::var("CLAUDE_CODE_EFFORT_LEVEL").ok().filter(|level| !level.is_empty()) { return Some(level); }
    let base = match text(config_dir) {
        Some(dir) => Path::new(dir).to_path_buf(),
        None => std::env::home_dir()?.join(".claude"),
    };
    let settings: Value = serde_json::from_str(&std::fs::read_to_string(base.join("settings.json")).ok()?).ok()?;
    settings.get("effortLevel").and_then(text).map(str::to_owned)
}

fn local_hhmm(secs: i64) -> Option<String> {
    #[cfg(unix)]
    {
        // A crate `libc` não expõe `tzset`.
        unsafe extern "C" { fn tzset(); }
        let when = secs as libc::time_t;
        // SAFETY: `tm` é zerado e só lido depois de `localtime_r` ter preenchido; `tzset` relê o TZ do processo.
        unsafe {
            let mut parts: libc::tm = std::mem::zeroed();
            tzset();
            if libc::localtime_r(&when, &mut parts).is_null() { return None; }
            Some(format!("{:02}:{:02}", parts.tm_hour, parts.tm_min))
        }
    }
    #[cfg(not(unix))]
    {
        use chrono::TimeZone;
        chrono::Local.timestamp_opt(secs, 0).single().map(|moment| moment.format("%H:%M").to_string())
    }
}

fn claude_status(payload: &Value, meta: &Value, quota: Option<&Value>, now: f64) -> Result<Value, RuntimeError> {
    // O Python chamava `.get` no que veio do cano e falhava se não fosse objeto.
    let rate = &payload["rate_limit_info"];
    if truthy(rate) && !rate.is_object() { return Err(error("policy_input")); }
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = text(&payload["model"]) {
        let model = if truthy(&meta["engine_account"]) { model.split_once('/').map_or(model, |(_, rest)| rest) } else { model };
        let mut segment = format!("🤖 {}", model_label(model));
        if let Some(effort) = text(&payload["effort"]).map(str::to_owned).or_else(|| default_effort(&meta["config_dir"])) {
            segment.push_str(&format!(" ({effort})"));
        }
        parts.push(segment);
    }
    let count = |key: &str| payload["usage"][key].as_f64().unwrap_or(0.0);
    let used = count("input_tokens") + count("cache_creation_input_tokens") + count("cache_read_input_tokens");
    if let Some(window) = payload["context_window"].as_f64().filter(|window| used != 0.0 && *window != 0.0) {
        parts.push(format!("💬 {}/{} {}/{}", fmt_tok(used), fmt_tok(count("output_tokens")), fmt_tok(used), fmt_tok(window)));
    }
    if let Some(cost) = payload["cost"].as_f64() { parts.push(format!("💵 ${cost:.2}")); }
    for window in quota.and_then(|quota| quota["windows"].as_array()).into_iter().flatten().filter(|window| !truthy(&window["por_modelo"])) {
        let Some(emoji) = (match window["rotulo"].as_str() { Some("5h") => Some("⚡"), Some("7d") => Some("📅"), _ => None }) else { continue };
        let Some(pct) = window["pct"].as_f64() else { continue };
        let mut segment = format!("{emoji}{}:{}%", window["rotulo"].as_str().unwrap_or(""), pct.round_ties_even() as i64);
        if let Some(reset) = window["reset_ts"].as_f64().filter(|reset| *reset != 0.0) { segment.push_str(&format!(" ↺{}", format_reset(reset, now))); }
        parts.push(segment);
    }
    let limit_reset = if rate["status"] == "rejected" { rate["resetsAt"].as_f64().and_then(|at| local_hhmm(at.floor() as i64)) } else { None };
    Ok(json!({"status_line": if parts.is_empty() { Value::Null } else { json!(parts.join(" │ ")) }, "limit_reset": limit_reset}))
}

fn codex_window(window: &Value, now: f64) -> Option<String> {
    if !truthy(window) { return None; }
    let (minutes, pct) = (window["windowDurationMins"].as_f64()?, window["usedPercent"].as_f64()?);
    let (emoji, label) = if (270.0..=330.0).contains(&minutes) { ("⚡", "5h") }
        else if (10020.0..=10140.0).contains(&minutes) { ("📅", "7d") }
        else { return None };
    let mut segment = format!("{emoji}{label}:{}%", pct.round_ties_even() as i64);
    if let Some(reset) = window["resetsAt"].as_f64().filter(|reset| *reset != 0.0) { segment.push_str(&format!(" ↺{}", format_reset(reset, now))); }
    Some(segment)
}

/// `model`/`effort` da escolha explícita; sem ela, o default da thread (`default_*`).
fn codex_status(payload: &Value, now: f64) -> Value {
    let mut parts: Vec<String> = Vec::new();
    if let Some(model) = text(&payload["model"]).or_else(|| text(&payload["default_model"])) {
        let mut segment = format!("🤖 {model}");
        if let Some(effort) = text(&payload["effort"]).or_else(|| text(&payload["default_effort"])) { segment.push_str(&format!(" ({effort})")); }
        parts.push(segment);
    }
    let usage = &payload["token_usage"];
    if truthy(usage) {
        let last = &usage["last"];
        if let (Some(used), Some(window)) = (last["inputTokens"].as_f64().filter(|n| *n != 0.0), usage["modelContextWindow"].as_f64().filter(|n| *n != 0.0)) {
            parts.push(format!("💬 {}/{} {}/{}", fmt_tok(used), fmt_tok(last["outputTokens"].as_f64().unwrap_or(0.0)), fmt_tok(used), fmt_tok(window)));
        }
    }
    if truthy(&payload["rate_limits"]) {
        parts.extend(["primary", "secondary"].iter().filter_map(|key| codex_window(&payload["rate_limits"][key], now)));
    }
    json!({"status_line": if parts.is_empty() { Value::Null } else { json!(parts.join(" │ ")) }})
}

// ---- skill_catalog ----

/// Habilitadas com nome e caminho; homônimas ganham `:<sha256(path)[:8]>`; ordenadas por nome.
fn skill_catalog(payload: &Value) -> Result<Value, RuntimeError> {
    let catalog = payload.get("catalog").filter(|catalog| catalog.is_object()).ok_or_else(|| error("policy_input"))?;
    let found = skills(catalog).into_iter().find(|skill| payload.get("name").is_some_and(|wanted| &skill["name"] == wanted));
    Ok(json!({"skill": found}))
}

/// O `skills_do_catalogo` do Python sobre a resposta do `skills/list`.
pub fn skills(catalog: &Value) -> Vec<Value> {
    use ring::digest;
    // Mesmo caminho listado duas vezes: fica a última, na posição da primeira (dict do Python).
    let mut by_path: IndexMap<&str, &Value> = IndexMap::new();
    for skill in catalog["data"].as_array().into_iter().flatten().flat_map(|group| group["skills"].as_array().into_iter().flatten()) {
        if let (true, Some(_), Some(path)) = (truthy(&skill["enabled"]), text(&skill["name"]), text(&skill["path"])) { by_path.insert(path, skill); }
    }
    let mut counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for skill in by_path.values() { *counts.entry(skill["name"].as_str().unwrap_or("")).or_default() += 1; }
    let mut skills: Vec<Value> = by_path.iter().map(|(path, skill)| {
        let native = skill["name"].as_str().unwrap_or("");
        let name = if counts[native] > 1 {
            let hash = digest::digest(&digest::SHA256, path.as_bytes());
            format!("{native}:{}", hash.as_ref()[..4].iter().map(|byte| format!("{byte:02x}")).collect::<String>())
        } else { native.to_owned() };
        json!({"name": name, "native_name": native, "path": path, "display": format!("/{name}"),
            "description": skill["description"], "source": "skill", "destructive": false})
    }).collect();
    skills.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    skills
}

// ---- last_usage ----

/// Cauda lida do transcript: o `usage` do último turno está sempre perto do fim.
const TAIL_TRANSCRIPT: u64 = 512 << 10;

/// `str.splitlines()`: o Python também quebra em VT, FF, FS/GS/RS, NEL e U+2028/9.
fn py_splitlines(text: &str) -> Vec<&str> {
    let (mut lines, mut start) = (Vec::new(), 0);
    let mut chars = text.char_indices().peekable();
    while let Some((at, c)) = chars.next() {
        if !matches!(c, '\n' | '\r' | '\x0b' | '\x0c' | '\x1c' | '\x1d' | '\x1e' | '\u{85}' | '\u{2028}' | '\u{2029}') { continue; }
        lines.push(&text[start..at]);
        start = at + c.len_utf8();
        if c == '\r' && chars.peek().is_some_and(|(_, next)| *next == '\n') { chars.next(); start += 1; }
    }
    if start < text.len() { lines.push(&text[start..]); }
    lines
}

fn tail_lines(file: &mut std::fs::File) -> std::io::Result<String> {
    use std::io::{Seek, SeekFrom};
    let size = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(size.saturating_sub(TAIL_TRANSCRIPT)))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Transcript ausente é conversa sem turno (`usage` nulo); qualquer outra falha ao abrir é erro,
/// e leitura que falha depois de aberto vira nulo, como no Python.
fn last_usage(meta: &Value) -> Result<Value, RuntimeError> {
    let path = meta["jsonl"].as_str().filter(|path| !path.is_empty()).ok_or_else(|| error("policy_input"))?;
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => return Ok(json!({"usage": null})),
        Err(_) => return Err(RuntimeError::new("policy_io", "transcript ilegível")),
    };
    // No Linux abrir pasta funciona; o `open` do Python recusa (IsADirectoryError).
    if file.metadata().is_ok_and(|stat| stat.is_dir()) { return Err(RuntimeError::new("policy_io", "transcript ilegível")); }
    let Ok(text) = tail_lines(&mut file) else { return Ok(json!({"usage": null})) };
    for line in py_splitlines(&text).into_iter().rev() {
        if !line.contains("\"assistant\"") { continue; }
        // Primeira linha do corte ou escrita em andamento.
        let Some(entry) = crate::transcript::pyjson::loads_lossless(line) else { continue };
        // O Python chamava `.get` no que veio e estourava se não fosse objeto.
        let Value::Object(entry) = entry else { return Err(error("policy_input")) };
        if entry.get("type").and_then(Value::as_str) != Some("assistant") || entry.get("isSidechain").is_some_and(truthy) { continue; }
        let message = entry.get("message").filter(|message| truthy(message));
        if message.is_some_and(|message| !message.is_object()) { return Err(error("policy_input")); }
        let Some(usage) = message.and_then(|message| message.get("usage")).filter(|usage| usage.is_object()) else { continue };
        if ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"].iter().any(|key| usage.get(*key).is_some_and(truthy)) {
            return Ok(json!({"usage": usage}));
        }
    }
    Ok(json!({"usage": null}))
}

// ---- reload_stamp ----

/// `Path.read_text`: UTF-8 estrito e quebras de linha universais; ausente é `None`.
fn read_text(path: &Path) -> Result<Option<String>, RuntimeError> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(RuntimeError::new("policy_io", "configuração ilegível")),
    };
    let text = String::from_utf8(bytes).map_err(|_| RuntimeError::new("policy_io", "configuração ilegível"))?;
    Ok(Some(text.replace("\r\n", "\n").replace('\r', "\n")))
}

/// Impressão do que o `claude -p` lê ao nascer: só o `mcpServers` do `.claude.json` e o
/// `settings.json` inteiro. Ilegível é erro: "não li" não pode virar "mudou" nem "não mudou".
fn config_stamp(config_dir: Option<&str>) -> Result<String, RuntimeError> {
    let home = || std::env::home_dir().ok_or_else(|| error("policy_input"));
    let root = match config_dir {
        Some("~") => home()?,
        Some(dir) if dir.starts_with("~/") => home()?.join(&dir[2..]),
        // `~usuario` e `~\…` o Python resolve de outro jeito; ler outro caminho calado daria marca errada.
        Some(dir) if dir.starts_with('~') => return Err(error("policy_input")),
        Some(dir) => Path::new(dir).to_path_buf(),
        None => home()?.join(".claude"),
    };
    let mcp = match read_text(&root.join(".claude.json"))? {
        Some(text) => match crate::transcript::pyjson::loads_lossless(&text) {
            Some(Value::Object(fields)) => fields.get("mcpServers").cloned().unwrap_or(Value::Null),
            Some(_) => Value::Null,
            None => return Err(RuntimeError::new("policy_io", "configuração ilegível")),
        },
        None => Value::Null,
    };
    let settings = read_text(&root.join("settings.json"))?.unwrap_or_default();
    let joined = format!("{}\0{settings}", crate::transcript::pyjson::dumps(&mcp, true));
    Ok(sha1_smol::Sha1::from(joined.as_bytes()).digest().to_string())
}

/// Motivo `config` só com marca gravada no cano e diferente da de agora.
fn reload_stamp(meta: &Value) -> Result<Value, RuntimeError> {
    let recorded = &meta["cano"]["config_marca"];
    if !truthy(recorded) { return Ok(json!({"reason": null})); }
    let current = config_stamp(text(&meta["config_dir"]))?;
    Ok(json!({"reason": if recorded.as_str() == Some(current.as_str()) { Value::Null } else { json!("config") }}))
}

// ---- unknown_private ----

const UNKNOWN_PER_KIND: usize = 30;
const UNKNOWN_FILE_CAP: u64 = 10 << 20;
type UnknownKey = (String, String, String);
static UNKNOWN_COUNTS: LazyLock<std::sync::Mutex<std::collections::HashMap<UnknownKey, usize>>> = LazyLock::new(Default::default);

/// Mesmo lugar do `log_paths.base()` do Python: o log é do Hangar da máquina, não da conta.
fn log_base() -> Option<std::path::PathBuf> {
    if cfg!(windows) {
        let root = std::env::var_os("LOCALAPPDATA").filter(|dir| !dir.is_empty()).map(std::path::PathBuf::from)
            .or_else(|| std::env::home_dir().map(|home| home.join("AppData").join("Local")))?;
        return Some(root.join("hangar").join("logs"));
    }
    Some(std::env::home_dir()?.join(".hangar").join("logs"))
}

/// Só a pasta folha é `0700` (como o `mkdir(mode=…, parents=True)` do Python); as de cima seguem o umask.
fn private_dir(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    #[cfg(unix)]
    let created = { use std::os::unix::fs::DirBuilderExt; std::fs::DirBuilder::new().mode(0o700).create(path) };
    #[cfg(not(unix))]
    let created = std::fs::create_dir(path);
    match created {
        Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        other => other,
    }
}

/// Payload bruto no log privado (pode carregar texto de conversa), nunca no diário.
fn unknown_private(payload: &Value, meta: &Value, now: f64) -> Result<Value, RuntimeError> {
    let kind = payload["kind"].as_str().filter(|kind| kind.chars().count() <= 512).ok_or_else(|| error("policy_input"))?;
    if !payload["event"].is_object() { return Err(error("policy_input")); }
    let io = |_| RuntimeError::new("policy_io", "log privado não gravado");
    let mut counts = UNKNOWN_COUNTS.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let key = (meta["key"].to_string(), meta["generation"].to_string(), kind.to_owned());
    if counts.get(&key).copied().unwrap_or(0) >= UNKNOWN_PER_KIND { return Ok(json!({"recorded": false, "reason": "type_limit"})); }
    let directory = log_base().ok_or_else(|| error("policy_input"))?.join("privado");
    private_dir(&directory).map_err(io)?;
    let file = directory.join(if meta["provider"] == "claude" { "claude-headless-desconhecidos.jsonl" } else { "codex-headless-desconhecidos.jsonl" });
    if std::fs::metadata(&file).is_ok_and(|stat| stat.len() > UNKNOWN_FILE_CAP) { return Ok(json!({"recorded": false, "reason": "file_limit"})); }
    let mut line = crate::transcript::pyjson::dumps_unicode(&json!({"ts": now, "sessao": meta["name"], "tipo": kind, "evento": payload["event"]}), false);
    line.push('\n');
    let mut stream = std::fs::OpenOptions::new().create(true).append(true).open(&file).map_err(io)?;
    std::io::Write::write_all(&mut stream, line.as_bytes()).map_err(io)?;
    *counts.entry(key).or_default() += 1;
    Ok(json!({"recorded": true}))
}
