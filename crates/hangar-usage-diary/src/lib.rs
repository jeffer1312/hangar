//! Diário de uso exportável: as linhas que a tela manda, as últimas linhas e o arquivo baixado.
//!
//! Conteúdo de conversa nunca entra. Só os campos de `FIELDS` passam, e campo desconhecido é
//! descartado: campo novo entra quando alguém o escreve lá, nunca por acidente de um cliente novo.
//! Um arquivo por dia em `diario/`, sete dias guardados; o teto do dia corta sucesso antes de falha.
#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};

use chrono::{DateTime, FixedOffset, NaiveDate, NaiveTime, SecondsFormat, TimeDelta};
use hangar_core::pycompat::{py, pyjson};
use serde_json::{Map, Value, json};

pub const DAYS_KEPT: u32 = 7;
/// Teto do dia: o arquivo continua mandável por chat. Passou, o dia para de receber sucesso.
const DAY_CAP: u64 = 4 << 20;
/// Teto rígido: só para um laço de falhas não encher o disco.
const HARD_CAP: u64 = 8 << 20;
const TEXT_MAX: usize = 300;
/// Linhas por envio; acima disso é ruído ou cliente com defeito.
const BATCH_MAX: usize = 50;
const LEVELS: [&str; 3] = ["ok", "aviso", "erro"];
const LATEST_DEFAULT: i64 = 60;
const LATEST_MAX: i64 = 200;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Int,
    Bool,
}
use Kind::{Bool, Int, Text};

/// Os campos aceitos numa linha da tela, na ordem em que saem no arquivo.
const FIELDS: &[(&str, Kind)] = &[
    ("ts", Text), ("evento", Text), ("nivel", Text), ("tela", Text), ("sessao", Text),
    ("provider", Text), ("codigo", Text), ("detalhe", Text), ("ms", Int), ("cli", Text),
    ("req", Text), ("seq", Int), ("pilha", Text), ("so", Text), ("navegador", Text),
    ("versao", Text), ("vista", Text), ("tela_px", Text), ("backend", Text),
    ("inicio_backend", Text), ("pid_backend", Int), ("comando", Text), ("erro_tipo", Text),
    ("causa_tipo", Text), ("errno", Int), ("winerror", Int), ("retorno", Int),
    ("limite_ms", Int), ("simultaneos", Int), ("etapa", Text), ("caracteres", Int),
    ("linhas", Int), ("composer_legivel", Bool), ("reguas", Int), ("altura_composer", Int),
    ("fundo_composer", Int), ("pastes_antes", Int), ("pastes_depois", Int), ("limpou", Bool),
    ("sessao_windows_backend", Int), ("sessao_windows_pane", Int), ("threads_backend", Int),
    ("memoria_total_mb", Int), ("memoria_disponivel_mb", Int), ("recursos_erro", Text),
    ("operacao", Text), ("conta_id", Text), ("tentativa", Int), ("quantidade", Int),
    ("espera_ms", Int), ("commit", Text), ("tag", Text), ("operacao_rust", Text),
    ("motivo", Text), ("escrita", Bool), ("esperado", Int), ("recebido", Text), ("causa", Text),
    ("causa_interna", Text), ("campo", Text), ("pasta", Text), ("raise_site", Text),
    ("falhas", Int), ("sessoes", Text), ("entradas", Int), ("importadas", Int),
    ("preservadas", Int), ("api", Int), ("requires_api", Int), ("rota", Text), ("saida", Text),
];

/// Por que um lote não chegou ao disco. `Disk` vira `gravadas: 0`, os outros um 500.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// O disco recusou o append: a tela reenvia o lote.
    Disk,
    /// A pasta do diário não pôde ser criada.
    Directory,
    /// Texto com surrogate solto não vira UTF-8.
    Unencodable,
}

/// Duas telas (celular e desktop) mandam lote ao mesmo tempo e o arquivo do dia é um só.
static ONE_WRITER: Mutex<()> = Mutex::new(());

/// Grava um lote da tela (`{"eventos": [...]}` ou a lista crua) em `base` (a pasta `diario`).
/// Devolve quantas linhas foram tratadas: linha descartada (teto, inválida) conta, porque a tela
/// reenvia quando o número não bate; corpo ilegível dá 0, e o disco recusando também.
pub fn record_screen(base: &Path, body: &[u8], now: DateTime<FixedOffset>) -> Result<usize, WriteError> {
    let Some(batch) = parse_body(body) else { return Ok(0) };
    let batch = match batch {
        Value::Object(mut fields) => fields.shift_remove("eventos").unwrap_or(Value::Null),
        other => other,
    };
    let Value::Array(items) = batch else { return Ok(0) };
    let received = &items[..items.len().min(BATCH_MAX)];
    let lines = received.iter().filter_map(|item| clean(item, now)).map(|mut line| {
        line.insert("origem".into(), json!("tela"));
        line
    }).collect();
    match write(base, lines, now) {
        Ok(()) => Ok(received.len()),
        Err(WriteError::Disk) => Ok(0),
        Err(error) => Err(error),
    }
}

/// O `request.json()`: UTF-8 com ou sem BOM, espaço JSON em volta, surrogate solto aceito.
// shortcut: corpo em UTF-16/32 (que o json.loads detectava) vira ilegível; nenhum cliente manda assim.
fn parse_body(body: &[u8]) -> Option<Value> {
    let body = body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body);
    let text = std::str::from_utf8(body).ok()?;
    pyjson::loads_lossless(text.trim_matches([' ', '\t', '\n', '\r']))
}

/// Uma linha da tela virando uma linha do arquivo.
fn clean(raw: &Value, now: DateTime<FixedOffset>) -> Option<Map<String, Value>> {
    let raw = raw.as_object()?;
    if py::strip(raw.get("evento")?.as_str()?).is_empty() {
        return None;
    }
    let mut out = Map::new();
    for &(name, kind) in FIELDS {
        let Some(value) = raw.get(name) else { continue };
        let kept = match (kind, value) {
            (Text, Value::String(text)) => {
                let text: String = py::strip(text).chars().take(TEXT_MAX).collect();
                if text.is_empty() {
                    continue;
                }
                Value::String(text)
            }
            (Int, Value::Number(number)) if integer(number) => value.clone(),
            (Bool, Value::Bool(_)) => value.clone(),
            _ => continue,
        };
        out.insert(name.into(), kept);
    }
    match out.get("ts").and_then(Value::as_str).map(|ts| screen_ts(ts, now)) {
        Some(Some(ts)) => {
            out.insert("ts".into(), json!(ts));
        }
        Some(None) => {
            out.shift_remove("ts");
        }
        None => {}
    }
    // Nível ausente ou inventado vira "ok": recusar a linha perderia o evento, que é o dado.
    if !out.get("nivel").and_then(Value::as_str).is_some_and(|level| LEVELS.contains(&level)) {
        out.insert("nivel".into(), json!("ok"));
    }
    Some(out)
}

/// Inteiro do JSON; número com ponto ou expoente é float e não passa, como no `isinstance(int)`.
fn integer(number: &serde_json::Number) -> bool {
    number.is_i64() || number.is_u64() || !number.to_string().contains(['.', 'e', 'E'])
}

/// O horário da tela no fuso e no formato do arquivo, ou `None` (vale o do envio).
///
/// A tela agrupa o lote antes de mandar: carimbar tudo na chegada colava eventos distintos no mesmo
/// instante. Só vale o mesmo dia do servidor, porque o arquivo é um por dia, e nunca o futuro
/// além de um minuto (trânsito do envio): ele já aconteceu quando chegou. Atraso é legítimo.
fn screen_ts(raw: &str, now: DateTime<FixedOffset>) -> Option<String> {
    let local = *now.offset();
    let when = parse_iso(raw, local)?.with_timezone(&local);
    if when.date_naive() != now.date_naive() || when > now + TimeDelta::minutes(1) {
        return None;
    }
    Some(when.to_rfc3339_opts(SecondsFormat::Millis, false))
}

/// `datetime.fromisoformat` nas formas que um relógio manda: data; data, separador de um caractere e
/// hora (`HH`, `HH:MM`, `HH:MM:SS`, fração); fuso `Z`, `±HH`, `±HHMM`, `±HH:MM`. Sem fuso, o do servidor.
// shortcut: o fuso do servidor é o deslocamento de agora (sem a regra de horário de verão do dia);
// formas raras do Python (semana ISO, data sem hífen, fuso com segundos) caem no horário do envio.
fn parse_iso(raw: &str, local: FixedOffset) -> Option<DateTime<FixedOffset>> {
    let date = raw.get(..10).filter(|date| iso_date(date))?;
    let date = NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    let rest = &raw[10..];
    let Some(separator) = rest.chars().next() else {
        return date.and_time(NaiveTime::MIN).and_local_timezone(local).single();
    };
    let (time, offset) = split_offset(&rest[separator.len_utf8()..])?;
    let naive = date.and_time(parse_time(time)?);
    naive.and_local_timezone(offset.unwrap_or(local)).single()
}

/// `AAAA-MM-DD` em dígitos ASCII, como o arquivo do dia e o `fromisoformat` exigem.
fn iso_date(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 10 && bytes[4] == b'-' && bytes[7] == b'-'
        && [0, 1, 2, 3, 5, 6, 8, 9].iter().all(|&at| bytes[at].is_ascii_digit())
}

fn split_offset(text: &str) -> Option<(&str, Option<FixedOffset>)> {
    if let Some(time) = text.strip_suffix('Z') {
        return Some((time, Some(FixedOffset::east_opt(0)?)));
    }
    let Some(at) = text.find(['+', '-']) else { return Some((text, None)) };
    let (time, zone) = text.split_at(at);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let zone = zone[1..].replace(':', "");
    if !(zone.len() == 2 || zone.len() == 4) || !zone.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let hours: i32 = zone[..2].parse().ok()?;
    let minutes: i32 = zone.get(2..).filter(|m| !m.is_empty()).map_or(Some(0), |m| m.parse().ok())?;
    if hours > 23 || minutes > 59 {
        return None;
    }
    Some((time, Some(FixedOffset::east_opt(sign * (hours * 3600 + minutes * 60))?)))
}

fn parse_time(text: &str) -> Option<NaiveTime> {
    let (clock, fraction) = text.split_once(['.', ',']).map_or((text, None), |(clock, fraction)| (clock, Some(fraction)));
    let parts: Vec<&str> = clock.split(':').collect();
    if parts.is_empty() || parts.len() > 3 || parts.iter().any(|part| part.len() != 2 || !part.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    if fraction.is_some() && parts.len() != 3 {
        return None;
    }
    let number = |at: usize| parts.get(at).map_or(Some(0), |part| part.parse::<u32>().ok());
    let micros = match fraction {
        None => 0,
        Some(digits) if (1..=9).contains(&digits.len()) && digits.bytes().all(|b| b.is_ascii_digit()) => {
            format!("{digits:0<6}")[..6].parse().ok()?
        }
        Some(_) => return None,
    };
    NaiveTime::from_hms_micro_opt(number(0)?, number(1)?, number(2)?, micros)
}

/// Append no arquivo do dia, com os tetos e a poda. Bloqueante.
fn write(base: &Path, lines: Vec<Map<String, Value>>, now: DateTime<FixedOffset>) -> Result<(), WriteError> {
    if lines.is_empty() {
        return Ok(());
    }
    let day = base.join(format!("uso-{}.jsonl", now.format("%Y-%m-%d")));
    std::fs::create_dir_all(base).map_err(|_| WriteError::Directory)?;
    let stamp = now.to_rfc3339_opts(SecondsFormat::Millis, false);
    let _one = ONE_WRITER.lock().unwrap_or_else(PoisonError::into_inner);
    let size = std::fs::metadata(&day).map_or(0, |metadata| metadata.len());
    if size >= HARD_CAP {
        return Ok(());
    }
    let lines: Vec<_> = if size >= DAY_CAP {
        lines.into_iter().filter(|line| line.get("nivel").and_then(Value::as_str) != Some("ok")).collect()
    } else {
        lines
    };
    if lines.is_empty() {
        return Ok(());
    }
    let mut text = String::new();
    for line in lines {
        // O horário da linha (o da tela) vence o do envio, que fica na mesma posição.
        let mut row = Map::from_iter([("ts".to_owned(), json!(stamp))]);
        row.extend(line);
        text += &pyjson::dumps_unicode(&Value::Object(row), false);
        text.push('\n');
    }
    if text.chars().any(py::is_marker) {
        return Err(WriteError::Unencodable);
    }
    for (cap, what) in [(DAY_CAP, "sucesso"), (HARD_CAP, "tudo")] {
        if size < cap && cap <= size + text.len() as u64 {
            // Um arquivo que simplesmente cessa parece máquina desligada.
            text += &pyjson::dumps(&json!({"ts": stamp, "evento": "diag.teto", "nivel": "aviso", "origem": "servidor",
                "etapa": what, "detalhe": format!("teto de {cap} bytes no dia")}), false);
            text.push('\n');
        }
    }
    use std::io::Write;
    std::fs::OpenOptions::new().create(true).append(true).open(&day)
        .and_then(|mut file| file.write_all(text.as_bytes()))
        .map_err(|_| WriteError::Disk)?;
    prune(base, now.date_naive());
    Ok(())
}

/// Guarda os dias mais recentes; roda no append, porque o arquivo só cresce quando há uso.
fn prune(base: &Path, today: NaiveDate) {
    let Some(cutoff) = today.checked_sub_signed(TimeDelta::days(i64::from(DAYS_KEPT) - 1)) else { return };
    let cutoff = cutoff.format("%Y-%m-%d").to_string();
    for (day, path) in dated_files(base) {
        if day < cutoff {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Todo `uso-AAAA-MM-DD.jsonl` sob o diário (raiz, `servidor-rust/`, `legado/…`), do mais antigo
/// ao mais novo. Link simbólico fica de fora, de arquivo ou de pasta.
pub fn files(base: &Path) -> Vec<PathBuf> {
    dated_files(base).into_iter().map(|(_, path)| path).collect()
}

fn dated_files(base: &Path) -> Vec<(String, PathBuf)> {
    let mut found = Vec::new();
    let mut pending = vec![base.to_owned()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && let Some(day) = entry.file_name().to_str().and_then(day_of) {
                found.push((day.to_owned(), entry.path()));
            }
        }
    }
    found.sort();
    found
}

fn day_of(name: &str) -> Option<&str> {
    let day = name.strip_prefix("uso-")?.strip_suffix(".jsonl")?;
    iso_date(day).then_some(day)
}

/// O texto do arquivo como o `read_text` do Python o via: UTF-8 trocando o inválido, fim de linha `\n`.
fn read_text(path: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).replace("\r\n", "\n").replace('\r', "\n"))
}

/// Quantas linhas o resumo mostra: `?ultimas=` (padrão 60) entre 1 e 200. `Err` traz o valor
/// recusado, como a validação de inteiro da rota antiga.
pub fn latest_count(query: Option<&str>) -> Result<usize, String> {
    let raw = query.into_iter().flat_map(|query| form_pairs(query))
        .filter(|(key, _)| key == "ultimas").map(|(_, value)| value).last();
    let count = match raw {
        None => LATEST_DEFAULT,
        Some(raw) => parse_int(&raw).ok_or(raw)?,
    };
    Ok(count.clamp(1, LATEST_MAX) as usize)
}

fn form_pairs(query: &str) -> Vec<(String, String)> {
    query.split('&').filter(|pair| !pair.is_empty()).map(|pair| {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        let decode = |part: &str| percent_decode(&part.replace('+', " "));
        (decode(key), decode(value))
    }).collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[at] == b'%' && let (Some(high), Some(low)) = (bytes.get(at + 1).and_then(|b| hex(*b)), bytes.get(at + 2).and_then(|b| hex(*b))) {
            out.push((high * 16 + low) as u8);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Inteiro em texto como a validação da rota antiga o lia: espaço em volta, sinal, `_` entre dígitos
/// e `.000` no fim. Fora de `i64` satura (o resultado é limitado a 200 de qualquer jeito).
fn parse_int(raw: &str) -> Option<i64> {
    let text = raw.trim();
    let text = match text.split_once('.') {
        Some((whole, zeros)) if zeros.bytes().all(|b| b == b'0') => whole,
        Some(_) => return None,
        None => text,
    };
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    let valid = !digits.is_empty() && !digits.starts_with('_') && !digits.ends_with('_') && !digits.contains("__")
        && digits.bytes().all(|b| b.is_ascii_digit() || b == b'_');
    if !valid {
        return None;
    }
    let digits = digits.replace('_', "");
    Some(match digits.parse::<i64>() {
        Ok(value) if negative => -value,
        Ok(value) => value,
        Err(_) if negative => i64::MIN,
        Err(_) => i64::MAX,
    })
}

/// As `count` linhas mais recentes, já lidas, mais novas primeiro. Lê de trás para frente, arquivo por
/// arquivo, e para ao juntar `count`: parsear a semana inteira para mostrar sessenta seria desperdício.
/// Dentro do dia, a ordem é por `ts` e `seq`, mais novos primeiro.
pub fn latest(base: &Path, count: usize) -> Vec<Value> {
    let files = files(base);
    let reversed: Vec<&PathBuf> = files.iter().rev().collect();
    let mut out = Vec::new();
    for day_files in reversed.chunk_by(|a, b| a.file_name() == b.file_name()) {
        let mut day = Vec::new();
        for path in day_files {
            let Some(text) = read_text(path) else { continue };
            let mut collected = 0;
            for raw in py::splitlines(&text).into_iter().rev() {
                let Some(value) = pyjson::loads_lossless(raw.trim_matches([' ', '\t', '\n', '\r'])) else { continue };
                if value.is_object() {
                    day.push(value);
                    collected += 1;
                }
                if collected >= count {
                    break;
                }
            }
        }
        day.sort_by(|a, b| order(b).partial_cmp(&order(a)).unwrap_or(std::cmp::Ordering::Equal));
        out.extend(day);
        if out.len() >= count {
            out.truncate(count);
            return out;
        }
    }
    out
}

// shortcut: `seq` que não é número conta como 0; a rota antiga dava 500 se precisasse compará-lo.
fn order(row: &Value) -> (String, f64) {
    let ts = match row.get("ts") {
        None => String::new(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Null) => "None".into(),
        Some(Value::Bool(true)) => "True".into(),
        Some(Value::Bool(false)) => "False".into(),
        Some(other) => other.to_string(),
    };
    let seq = match row.get("seq") {
        Some(Value::Number(number)) => number.as_f64().unwrap_or(0.0),
        Some(Value::Bool(flag)) => f64::from(u8::from(*flag)),
        _ => 0.0,
    };
    (ts, seq)
}

/// Números para a tela decidir o que mostrar no botão, sem baixar o arquivo inteiro.
pub fn summary(base: &Path) -> Value {
    let files = files(base);
    let bytes: u64 = files.iter().filter_map(|path| std::fs::metadata(path).ok()).map(|metadata| metadata.len()).sum();
    let names: Vec<String> = files.iter().filter_map(|path| path.file_name()).map(|name| name.to_string_lossy().into_owned()).collect();
    let days = names.iter().collect::<HashSet<_>>().len();
    json!({"dias": days, "bytes": bytes, "arquivos": names, "dias_guardados": DAYS_KEPT})
}

/// O corpo do download: a linha de cabeçalho e os dias concatenados, o mais antigo primeiro.
pub fn download(base: &Path, header: &str) -> String {
    let mut out = format!("{header}\n");
    for path in files(base) {
        if let Some(text) = read_text(&path) {
            out.push_str(&text);
        }
    }
    out
}
