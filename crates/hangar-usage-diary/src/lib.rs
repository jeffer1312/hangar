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
    ("cliente", Text),
];

/// Por que um lote não chegou ao disco. `Disk` vira `gravadas: 0`, os outros um 500. O tipo do
/// erro de E/S fica junto: é ele que diz ao suporte se foi disco cheio, permissão ou outra coisa.
#[derive(Debug, PartialEq, Eq)]
pub enum WriteError {
    /// O disco recusou o append: a tela reenvia o lote.
    Disk(std::io::ErrorKind),
    /// A pasta do diário não pôde ser criada.
    Directory(std::io::ErrorKind),
}

/// Duas telas (celular e desktop) mandam lote ao mesmo tempo e o arquivo do dia é um só.
static ONE_WRITER: Mutex<()> = Mutex::new(());

/// Grava um lote da tela (`{"eventos": [...]}` ou a lista crua) em `base` (a pasta `diario`).
/// Devolve quantas linhas foram tratadas: linha descartada (teto, inválida) conta, porque a tela
/// reenvia quando o número não bate; corpo ilegível dá 0, e o disco recusando também.
pub fn record_screen(base: &Path, body: &[u8], now: DateTime<FixedOffset>) -> Result<usize, WriteError> {
    record_screen_with_owner(base, body, now, None)
}

/// `owner_collection` vem do portão do servidor, nunca do corpo do cliente.
///
/// # Errors
/// Devolve `WriteError` se não puder preparar a pasta. Falha no append mantém o contrato de reenvio.
pub fn record_screen_with_owner(base: &Path, body: &[u8], now: DateTime<FixedOffset>,
                                owner: Option<(&str, &[(&'static str, String)], DateTime<FixedOffset>)>) -> Result<usize, WriteError> {
    let owner_collection = owner.map(|(collection, _, _)| collection);
    let Some(batch) = parse_body(body) else { return Ok(0) };
    let batch = match batch {
        Value::Object(mut fields) => fields.shift_remove("eventos").unwrap_or(Value::Null),
        other => other,
    };
    let Value::Array(items) = batch else { return Ok(0) };
    let received = &items[..items.len().min(BATCH_MAX)];
    let lines = received.iter().filter_map(|item| {
        let mut line = clean(item, now)?;
        line.insert("origem".into(), json!("tela"));
        if let Some(collection) = owner_collection && screen_exportable(item)
            && let Some(at) = item["ts"].as_str().and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            && at <= now {
            // O diário local pode corrigir o horário para a chegada; a coleta nunca usa esse substituto.
            line.insert("telemetry_at".into(), json!(at.to_rfc3339()));
            if line.get("nivel").and_then(Value::as_str).is_some_and(|level| level != "ok") {
                let (ctx, started) = owner.map_or((&[][..], at), |(_, ctx, started)| (ctx, started));
                // O retrato é o do servidor na chegada: falha de antes da subida dele aconteceu com ele fora.
                let ctx: Vec<_> = ctx.iter().map(|(key, value)| (*key, if *key == "up" && at < started { "down".to_owned() } else { value.clone() })).collect();
                put_ctx(&mut line, &ctx);
            }
            mark_telemetry(&mut line, "owner_screen", collection);
        }
        Some(line)
    }).collect();
    match write(base, lines, now) {
        Ok(()) => Ok(received.len()),
        Err(WriteError::Disk(kind)) => {
            tracing::warn!(code = "diag_write_failed", ?kind, "lote da tela não gravado no diário; a tela reenvia");
            Ok(0)
        }
        Err(error) => Err(error),
    }
}

/// A marca vinda do cliente só pode tirar uma linha da coleta. As versões antigas
/// também redirecionavam `machines.request_failed`, sem comprovar a origem.
pub fn screen_exportable(item: &Value) -> bool {
    item.get("local_only").is_none_or(|value| value == &Value::Bool(false))
        && !matches!(item["evento"].as_str(), Some("settings.snapshot" | "machines.request_failed"))
}

/// Origem de uma linha produzida pelo servidor, depois de conferir a autoria.
#[derive(Clone, Copy)]
pub enum OwnerAction { Http, Mcp }

/// Nível de uma falha que o servidor percebeu sozinho.
#[derive(Clone, Copy)]
pub enum Severity { Warn, Error }

static BUILD: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// O binário (`versão-commit` da telemetria) que carimba as linhas da coleta; só a primeira vale.
pub fn set_build(build: &str) { let _ = BUILD.set(build.to_owned()); }

/// Acrescenta ao diário apenas os campos técnicos da ação, sem corpo, argumentos nem resposta.
///
/// # Errors
/// Devolve `WriteError` se a pasta não puder ser preparada ou o append falhar.
pub fn record_owner_action(base: &Path, origin: OwnerAction, collection: &str, now: DateTime<FixedOffset>,
                           action: &str, stage: &'static str, code: Option<&str>, ms: u64,
                           cause: Option<&str>, ctx: &[(&'static str, String)]) -> Result<(), WriteError> {
    let (event, source) = match origin { OwnerAction::Http => ("http.action", "owner_http"), OwnerAction::Mcp => ("mcp.tool", "owner_mcp") };
    let mut line = Map::from_iter([
        ("ts".into(), json!(now.to_rfc3339())), ("origem".into(), json!("servidor")),
        ("evento".into(), json!(event)), ("action".into(), json!(action)), ("etapa".into(), json!(stage)),
        ("nivel".into(), json!(if code.is_some() { "erro" } else { "ok" })), ("ms".into(), json!(ms)),
    ]);
    if let Some(code) = code {
        line.insert("codigo".into(), json!(code));
        if let Some(cause) = cause { line.insert("causa".into(), json!(cause)); }
        put_ctx(&mut line, ctx);
    }
    mark_telemetry(&mut line, source, collection);
    write(base, vec![line], now)
}

/// Falha que o servidor percebeu sozinho (`server_health`): código e causa fechados, retrato da hora.
/// `at` é quando aconteceu, se não foi agora (a saída da vida anterior do servidor), e `build` o binário
/// em que aconteceu, quando não foi este.
///
/// # Errors
/// Devolve `WriteError` se a pasta não puder ser preparada ou o append falhar.
#[allow(clippy::too_many_arguments)]
pub fn record_health(base: &Path, collection: &str, now: DateTime<FixedOffset>, at: Option<DateTime<FixedOffset>>, event: &str,
                     code: &str, cause: &str, severity: Severity, ctx: &[(&'static str, String)], build: Option<&str>) -> Result<(), WriteError> {
    let mut line = Map::from_iter([
        ("ts".into(), json!(now.to_rfc3339())), ("origem".into(), json!("servidor")), ("evento".into(), json!(event)),
        ("nivel".into(), json!(match severity { Severity::Warn => "aviso", Severity::Error => "erro" })),
        ("codigo".into(), json!(code)), ("causa".into(), json!(cause)),
    ]);
    if let Some(at) = at { line.insert("telemetry_at".into(), json!(at.to_rfc3339())); }
    put_ctx(&mut line, ctx);
    mark_telemetry(&mut line, "server_health", collection);
    if let Some(build) = build { line.insert("telemetry_build".into(), json!(build)); }
    write(base, vec![line], now)
}

/// O retrato da hora vai num objeto próprio, fora dos campos da tela.
fn put_ctx(line: &mut Map<String, Value>, ctx: &[(&'static str, String)]) {
    if !ctx.is_empty() {
        line.insert("ctx".into(), Value::Object(ctx.iter().map(|(key, value)| ((*key).to_owned(), json!(value))).collect()));
    }
}

fn mark_telemetry(line: &mut Map<String, Value>, source: &'static str, collection: &str) {
    line.insert("telemetry_origin".into(), json!(source));
    line.insert("telemetry_collection".into(), json!(collection));
    if let Some(build) = BUILD.get() { line.insert("telemetry_build".into(), json!(build)); }
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
    let event = py::strip(raw.get("evento")?.as_str()?);
    // O retrato é só da telemetria; confirmar o lote não exige escrevê-lo no diário geral.
    if event.is_empty() || event == "settings.snapshot" {
        return None;
    }
    let mut out = Map::new();
    for &(name, kind) in FIELDS {
        let Some(value) = raw.get(name) else { continue };
        let kept = match (kind, value) {
            (Text, Value::String(text)) => {
                let mut text: String = py::strip(text).chars().take(TEXT_MAX).collect();
                if text.is_empty() {
                    continue;
                }
                // Surrogate solto não vira UTF-8: troca por U+FFFD em vez de perder o lote inteiro.
                py::scrub_str(&mut text);
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
    std::fs::create_dir_all(base).map_err(|error| WriteError::Directory(error.kind()))?;
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
        .map_err(|error| WriteError::Disk(error.kind()))?;
    prune(base, now.date_naive());
    Ok(())
}

/// Guarda os dias mais recentes; roda no append, porque o arquivo só cresce quando há uso.
fn prune(base: &Path, today: NaiveDate) {
    let Some(cutoff) = today.checked_sub_signed(TimeDelta::days(i64::from(DAYS_KEPT) - 1)) else { return };
    let cutoff = cutoff.format("%Y-%m-%d").to_string();
    for (day, path) in dated_files(base) {
        if day < cutoff {
            match std::fs::remove_file(&path) {
                // Duas telas gravando juntas podam o mesmo dia: o segundo não acha mais o arquivo.
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                    tracing::warn!(code = "diag_prune_failed", kind = ?error.kind(), day, "dia velho do diário não apagado");
                }
                _ => {}
            }
        }
    }
}

/// Todo `uso-AAAA-MM-DD.jsonl` sob o diário (raiz, `servidor-rust/`, `legado/…`), do mais antigo
/// ao mais novo. Link simbólico fica de fora, de arquivo ou de pasta.
pub fn files(base: &Path) -> Vec<PathBuf> {
    dated_files(base).into_iter().map(|(_, path)| path).collect()
}

/// Como `files`, com o dia (`AAAA-MM-DD`) do nome de cada um.
pub fn dated_files(base: &Path) -> Vec<(String, PathBuf)> {
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
            // Um arquivo por vez, até o teto de 8 MiB do dia; o `splitlines` já quebra em `\r` e `\r\n`.
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(error) => {
                    tracing::warn!(code = "diag_read_failed", kind = ?error.kind(), file = %relative(base, path),
                        "dia do diário ilegível; o resumo sai sem ele");
                    continue;
                }
            };
            let text = String::from_utf8_lossy(&bytes);
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

/// Pedaço lido de cada arquivo no download: o diário inteiro nunca fica em memória.
const DOWNLOAD_CHUNK: usize = 64 * 1024;

/// O corpo do download, entregue em pedaços a `sink`: a linha de cabeçalho e os dias concatenados,
/// o mais antigo primeiro, cada arquivo como o `read_text` do Python o via (UTF-8 trocando o
/// inválido, fim de linha `\n`). Arquivo que não abre, ou que falha no meio, ganha uma linha
/// `diag.arquivo_ilegivel` no lugar do que faltou; só o `sink` devolve erro.
pub fn download_to(base: &Path, header: &str, sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>) -> std::io::Result<()> {
    download_chunked(base, header, DOWNLOAD_CHUNK, sink)
}

/// `download_to` com o tamanho do pedaço escolhido: as provas cortam no meio de caractere e de `\r\n`.
pub fn download_chunked(base: &Path, header: &str, chunk: usize, sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>) -> std::io::Result<()> {
    sink(format!("{header}\n").as_bytes())?;
    // O último byte entregue: a marca de falha no meio de um arquivo começa numa linha própria.
    let last = std::cell::Cell::new(b'\n');
    let mut tracked = |part: &[u8]| {
        if let Some(&byte) = part.last() {
            last.set(byte);
        }
        sink(part)
    };
    for path in files(base) {
        let failed = match std::fs::File::open(&path) {
            Ok(file) => normalize(file, chunk.max(1), &mut tracked)?,
            Err(error) => Some(error.kind()),
        };
        if let Some(kind) = failed {
            let name = relative(base, &path);
            tracing::warn!(code = "diag_read_failed", ?kind, file = %name, "dia do diário ilegível no download");
            let mut marker = if last.get() == b'\n' { String::new() } else { "\n".to_owned() };
            marker += &pyjson::dumps_unicode(&json!({"evento": "diag.arquivo_ilegivel", "nivel": "aviso",
                "origem": "servidor", "arquivo": name, "erro": format!("{kind:?}")}), false);
            marker.push('\n');
            tracked(marker.as_bytes())?;
        }
    }
    Ok(())
}

/// O caminho do arquivo dentro do diário, com `/`: nunca a pasta pessoal de quem baixou.
fn relative(base: &Path, path: &Path) -> String {
    let inside = path.strip_prefix(base).unwrap_or(path);
    let parts: Vec<String> = inside.components().map(|part| part.as_os_str().to_string_lossy().into_owned()).collect();
    parts.join("/")
}

/// Normaliza um arquivo para o `sink`. `Ok(Some(kind))`: a leitura falhou no meio e o que já saiu
/// fica (o Python pulava o arquivo inteiro); quem chama marca a falha. `Err`: o `sink` recusou.
fn normalize(mut reader: impl std::io::Read, chunk: usize, sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>) -> std::io::Result<Option<std::io::ErrorKind>> {
    let mut buffer = vec![0; chunk];
    let (mut carry, mut pending_cr, mut failed) = (Vec::new(), false, None);
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                failed = Some(error.kind());
                break;
            }
        };
        carry.extend_from_slice(&buffer[..read]);
        let (text, rest) = decode_lossy(&carry, false);
        carry = rest;
        emit_lines(&text, &mut pending_cr, sink)?;
    }
    let (text, _) = decode_lossy(&carry, true);
    emit_lines(&text, &mut pending_cr, sink)?;
    if pending_cr {
        sink(b"\n")?;
    }
    Ok(failed)
}

/// `String::from_utf8_lossy` em pedaços: sequência incompleta no fim fica para o próximo pedaço,
/// e só no fim do arquivo vira U+FFFD.
fn decode_lossy(bytes: &[u8], end: bool) -> (String, Vec<u8>) {
    let mut out = String::with_capacity(bytes.len());
    let mut rest = bytes;
    loop {
        match std::str::from_utf8(rest) {
            Ok(text) => {
                out.push_str(text);
                return (out, Vec::new());
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                out.push_str(std::str::from_utf8(valid).unwrap_or_default());
                match error.error_len() {
                    Some(length) => {
                        out.push('\u{FFFD}');
                        rest = &after[length..];
                    }
                    None if end => {
                        out.push('\u{FFFD}');
                        return (out, Vec::new());
                    }
                    None => return (out, after.to_vec()),
                }
            }
        }
    }
}

/// `\r\n` e `\r` viram `\n`; um `\r` no fim do pedaço espera o próximo caractere.
fn emit_lines(text: &str, pending_cr: &mut bool, sink: &mut dyn FnMut(&[u8]) -> std::io::Result<()>) -> std::io::Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut out = String::with_capacity(text.len() + 1);
    for c in text.chars() {
        if std::mem::take(pending_cr) {
            out.push('\n');
            if c == '\n' {
                continue;
            }
        }
        if c == '\r' { *pending_cr = true } else { out.push(c) }
    }
    sink(out.as_bytes())
}

#[cfg(test)]
mod tests {
    /// Leitor que entrega um pedaço e depois falha, como um disco que cai no meio do download.
    struct FailsAfter(Option<&'static [u8]>);

    impl std::io::Read for FailsAfter {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            match self.0.take() {
                Some(part) => {
                    buffer[..part.len()].copy_from_slice(part);
                    Ok(part.len())
                }
                None => Err(std::io::ErrorKind::PermissionDenied.into()),
            }
        }
    }

    #[test]
    fn a_read_failure_in_the_middle_keeps_what_came_out_and_reports_the_kind() {
        let mut out = Vec::new();
        let failed = super::normalize(FailsAfter(Some(b"a\r\nmeia")), 64, &mut |part| {
            out.extend_from_slice(part);
            Ok(())
        }).unwrap();
        assert_eq!(failed, Some(std::io::ErrorKind::PermissionDenied));
        assert_eq!(out, b"a\nmeia");
    }
}
