//! Agregações do diário de uso para a tela Status: as saídas do servidor (`hangar_server.saida`)
//! dos últimos 7 dias e a lentidão (`api.servidor`, `systemd_watchdog`) das últimas 24 h.
//!
//! Lê só a pasta de `hangar_core::diag::diary_dir()` (`diario/servidor-rust/`), onde o
//! `hangar-server` grava as três linhas; a raiz `diario/` é das telas e do Python antigo. Cada
//! arquivo fica em cache pelo tamanho e pela data de modificação, e o que cresceu é lido só do
//! ponto em que parou: o diário só recebe linhas no fim. Bloqueante: chamar fora do executor.
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::SystemTime;

use chrono::{DateTime, SecondsFormat, TimeDelta, Utc};
use hangar_api::status::{
    Exit, ExitKind, ExitsBlock, HourBucket, RouteStat, SlowBlock, WatchdogEvent, WatchdogKind,
};
use serde_json::{Map, Value};

const EXITS_DAYS: i64 = 7;
const HOURS: i64 = 24;
const TOP_ROUTES: usize = 5;
const SLOW_MS: u64 = 1000;

/// Leitor do diário com o cache dos arquivos já lidos; um por servidor.
pub struct DiaryReader {
    source: Source,
    cache: Mutex<Cache>,
}

enum Source {
    Machine,
    At(PathBuf),
}

/// O que foi deixado de fora na última leitura: linhas que não são JSON ou evento conhecido com
/// campo fora de forma, e arquivos que não abriram.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Skipped {
    pub lines: u64,
    pub files: u64,
}

#[derive(Default)]
struct Cache {
    files: HashMap<PathBuf, FileCache>,
    reads: u64,
}

#[derive(Default)]
struct FileCache {
    len: u64,
    modified: Option<SystemTime>,
    read_to: u64,
    failed: bool,
    facts: Facts,
}

#[derive(Default)]
struct Facts {
    exits: Vec<(DateTime<Utc>, Exit)>,
    requests: Vec<Request>,
    watchdog: Vec<(DateTime<Utc>, WatchdogEvent)>,
    unreadable: u64,
}

struct Request {
    at: DateTime<Utc>,
    route: String,
    code: u16,
    ms: Option<u64>,
}

impl DiaryReader {
    /// O diário desta máquina; desligado (`DIARY_OFF`, binário de teste) deixa os blocos ausentes.
    pub fn machine() -> Self {
        Self { source: Source::Machine, cache: Mutex::default() }
    }

    pub fn at(dir: PathBuf) -> Self {
        Self { source: Source::At(dir), cache: Mutex::default() }
    }

    /// Saídas do servidor com a hora da saída nos 7 dias até `now`, da mais antiga à mais nova.
    pub fn exits(&self, now: DateTime<Utc>) -> Option<ExitsBlock> {
        let since = now - TimeDelta::days(EXITS_DAYS);
        let cache = self.refresh()?;
        let mut items: Vec<_> = cache.files.values().flat_map(|file| &file.facts.exits)
            .filter(|(at, _)| *at > since && *at <= now).collect();
        items.sort_by_key(|(at, _)| *at);
        Some(ExitsBlock { items: items.into_iter().map(|(_, exit)| exit.clone()).collect() })
    }

    /// Pedidos lentos (código < 500 e ≥ 1 s) e com erro (≥ 500) em 24 faixas de uma hora, a última
    /// sendo a hora de `now`; as rotas que mais aparecem e os avisos do watchdog do mesmo período.
    pub fn slow(&self, now: DateTime<Utc>) -> Option<SlowBlock> {
        let first = hour_of(now) - TimeDelta::hours(HOURS - 1);
        let inside = |at: &DateTime<Utc>| *at >= first && *at <= now;
        let cache = self.refresh()?;
        let mut hours: Vec<HourBucket> = (0..HOURS).map(|hour| HourBucket {
            hour_start: stamp(first + TimeDelta::hours(hour)), slow: 0, errors: 0 }).collect();
        let mut routes: HashMap<&str, (RouteStat, DateTime<Utc>)> = HashMap::new();
        let mut watchdog = Vec::new();
        for facts in cache.files.values().map(|file| &file.facts) {
            for request in facts.requests.iter().filter(|request| inside(&request.at)) {
                let error = request.code >= 500;
                let slow = !error && request.ms.is_some_and(|ms| ms >= SLOW_MS);
                if !error && !slow {
                    continue;
                }
                let bucket = &mut hours[(request.at - first).num_hours() as usize];
                if error { bucket.errors += 1 } else { bucket.slow += 1 }
                let (stat, last) = routes.entry(request.route.as_str()).or_insert_with(|| (RouteStat {
                    route: request.route.clone(), count: 0, worst_ms: None, last_code: request.code }, request.at));
                stat.count += 1;
                stat.worst_ms = stat.worst_ms.max(request.ms);
                if request.at >= *last {
                    (stat.last_code, *last) = (request.code, request.at);
                }
            }
            watchdog.extend(facts.watchdog.iter().filter(|(at, _)| inside(at)));
        }
        let mut top_routes: Vec<RouteStat> = routes.into_values().map(|(stat, _)| stat).collect();
        top_routes.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.route.cmp(&b.route)));
        top_routes.truncate(TOP_ROUTES);
        watchdog.sort_by_key(|(at, _)| *at);
        Some(SlowBlock { hours, top_routes, watchdog: watchdog.into_iter().map(|(_, event)| event.clone()).collect() })
    }

    /// O que a última leitura deixou de fora, para o servidor registrar.
    pub fn skipped(&self) -> Skipped {
        let cache = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        cache.files.values().fold(Skipped::default(), |sum, file| Skipped {
            lines: sum.lines + file.facts.unreadable, files: sum.files + u64::from(file.failed) })
    }

    /// Põe o cache em dia com a pasta: arquivo que sumiu sai, arquivo com tamanho ou data nova é
    /// lido do ponto em que parou (do começo, se encolheu).
    fn refresh(&self) -> Option<std::sync::MutexGuard<'_, Cache>> {
        let dir = match &self.source {
            Source::Machine => hangar_core::diag::diary_dir()?,
            Source::At(dir) => dir.clone(),
        };
        let paths = hangar_usage_diary::files(&dir);
        let mut guard = self.cache.lock().unwrap_or_else(PoisonError::into_inner);
        let Cache { files, reads } = &mut *guard;
        files.retain(|path, _| paths.contains(path));
        for path in paths {
            let entry = files.entry(path.clone()).or_default();
            let Ok(meta) = std::fs::metadata(&path) else {
                entry.failed = true;
                continue;
            };
            let (len, modified) = (meta.len(), meta.modified().ok());
            if !entry.failed && entry.len == len && entry.modified == modified {
                continue;
            }
            if len < entry.read_to {
                *entry = FileCache::default();
            }
            *reads += 1;
            match read_tail(&path, entry) {
                Ok(()) => (entry.len, entry.modified, entry.failed) = (len, modified, false),
                Err(_) => entry.failed = true,
            }
        }
        Some(guard)
    }
}

/// Lê do ponto em que parou até a última quebra de linha: a linha que o escritor ainda está
/// gravando fica para a próxima leitura, em vez de virar linha ilegível.
fn read_tail(path: &Path, entry: &mut FileCache) -> std::io::Result<()> {
    let mut file = std::fs::File::open(path)?;
    file.seek(SeekFrom::Start(entry.read_to))?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw)?;
    let complete = raw.iter().rposition(|&byte| byte == b'\n').map_or(0, |end| end + 1);
    for line in raw[..complete].split(|&byte| byte == b'\n') {
        entry.facts.add(line);
    }
    entry.read_to += complete as u64;
    Ok(())
}

impl Facts {
    fn add(&mut self, raw: &[u8]) {
        if raw.iter().all(u8::is_ascii_whitespace) {
            return;
        }
        let Ok(line) = serde_json::from_slice::<Map<String, Value>>(raw) else {
            self.unreadable += 1;
            return;
        };
        let read = match line.get("evento").and_then(Value::as_str) {
            Some("hangar_server.saida") => exit(&line).map(|exit| self.exits.push(exit)),
            Some("api.servidor") => request(&line).map(|request| self.requests.push(request)),
            Some("systemd_watchdog") => watchdog(&line).map(|event| self.watchdog.push(event)),
            _ => Some(()),
        };
        if read.is_none() {
            self.unreadable += 1;
        }
    }
}

/// C2: `{tipo, causa?, de?, para?, quando?, erro?}`; `erro` traz as últimas linhas de erro numa
/// string só. A hora é o `quando` (a saída), não o `ts` (a subida seguinte, que gravou a linha);
/// linha sem ele, de versão anterior, fica com o `ts`.
fn exit(line: &Map<String, Value>) -> Option<(DateTime<Utc>, Exit)> {
    let at = text(line, "quando").and_then(parse).or_else(|| at(line))?;
    let kind = match text(line, "tipo")? {
        "crash" => ExitKind::Crash,
        "update" => ExitKind::Update,
        "requested" => ExitKind::Requested,
        _ => return None,
    };
    let error_lines: Vec<String> = text(line, "erro").map(|joined| joined.split('\n').flat_map(|part| part.split(" | "))
        .map(str::trim).filter(|part| !part.is_empty()).map(str::to_owned).collect()).unwrap_or_default();
    let owned = |key| text(line, key).map(str::to_owned);
    Some((at, Exit { ts: stamp(at), kind, cause: owned("causa"), from: owned("de"), to: owned("para"), error_lines }))
}

fn request(line: &Map<String, Value>) -> Option<Request> {
    Some(Request {
        at: at(line)?,
        route: text(line, "detalhe")?.to_owned(),
        code: text(line, "codigo")?.parse().ok()?,
        ms: line.get("ms").and_then(Value::as_u64),
    })
}

fn watchdog(line: &Map<String, Value>) -> Option<(DateTime<Utc>, WatchdogEvent)> {
    let at = at(line)?;
    let kind = match text(line, "tipo")? {
        "atrasado" => WatchdogKind::Late,
        "retido" => WatchdogKind::Withheld,
        _ => return None,
    };
    Some((at, WatchdogEvent { ts: stamp(at), kind, ms: line.get("ms").and_then(Value::as_u64) }))
}

fn at(line: &Map<String, Value>) -> Option<DateTime<Utc>> {
    parse(text(line, "ts")?)
}

fn parse(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text).ok().map(|at| at.with_timezone(&Utc))
}

fn text<'a>(line: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    line.get(key).and_then(Value::as_str).filter(|value| !value.is_empty())
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn hour_of(at: DateTime<Utc>) -> DateTime<Utc> {
    let secs = at.timestamp();
    DateTime::from_timestamp(secs - secs.rem_euclid(3600), 0).unwrap_or(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;

    fn utc(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text).unwrap().with_timezone(&Utc)
    }

    fn append(dir: &Path, day: &str, lines: &[Value]) {
        let mut file = std::fs::OpenOptions::new().create(true).append(true)
            .open(dir.join(format!("uso-{day}.jsonl"))).unwrap();
        for line in lines {
            writeln!(file, "{line}").unwrap();
        }
    }

    fn api(ts: &str, route: &str, code: &str, ms: u64) -> Value {
        json!({"ts": ts, "evento": "api.servidor", "nivel": "ok", "codigo": code, "detalhe": route, "ms": ms,
            "etapa": "cabecalhos", "origem": "servidor"})
    }

    fn saida(ts: &str, extra: Value) -> Value {
        let mut line = json!({"ts": ts, "evento": "hangar_server.saida", "nivel": "aviso"});
        line.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        line
    }

    fn reads(reader: &DiaryReader) -> u64 {
        reader.cache.lock().unwrap().reads
    }

    #[test]
    fn hours_split_slow_from_errors_inside_the_24h_window() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "2026-11-16", &[
            api("2026-11-16T11:59:59.000-03:00", "GET /api/costs", "200", 1500),
            api("2026-11-16T12:00:00.000-03:00", "GET /api/costs", "200", 1500),
        ]);
        append(dir.path(), "2026-11-17", &[
            api("2026-11-17T10:05:00.000-03:00", "GET /api/sessions/*/history", "503", 20),
            api("2026-11-17T14:10:00.000Z", "GET /api/costs", "200", 1800),
            api("2026-11-17T14:11:00.000Z", "POST /api/sessions", "409", 200),
            api("2026-11-17T14:31:00.000Z", "GET /api/costs", "200", 1500),
        ]);
        let slow = DiaryReader::at(dir.path().to_owned()).slow(utc("2026-11-17T14:30:00Z")).unwrap();
        assert_eq!(slow.hours.len(), 24);
        assert_eq!(slow.hours[0], HourBucket { hour_start: "2026-11-16T15:00:00Z".into(), slow: 1, errors: 0 });
        assert_eq!(slow.hours[22], HourBucket { hour_start: "2026-11-17T13:00:00Z".into(), slow: 0, errors: 1 });
        assert_eq!(slow.hours[23], HourBucket { hour_start: "2026-11-17T14:00:00Z".into(), slow: 1, errors: 0 });
        let total: u32 = slow.hours.iter().map(|hour| hour.slow + hour.errors).sum();
        assert_eq!(total, 3, "fora da janela e rápido não 5xx não contam");
    }

    #[test]
    fn top_routes_rank_by_count_with_worst_ms_and_last_code() {
        let dir = tempfile::tempdir().unwrap();
        let mut lines = Vec::new();
        for (route, count) in [("GET /a", 6), ("GET /b", 5), ("GET /c", 4), ("GET /d", 3), ("GET /e", 2), ("GET /f", 1)] {
            for minute in 0..count {
                lines.push(api(&format!("2026-11-17T12:{minute:02}:00Z"), route, "200", 1000 + minute * 100));
            }
        }
        lines.push(api("2026-11-17T13:00:00Z", "GET /a", "502", 30));
        append(dir.path(), "2026-11-17", &lines);
        let slow = DiaryReader::at(dir.path().to_owned()).slow(utc("2026-11-17T14:00:00Z")).unwrap();
        let top: Vec<_> = slow.top_routes.iter().map(|stat| (stat.route.as_str(), stat.count)).collect();
        assert_eq!(top, [("GET /a", 7), ("GET /b", 5), ("GET /c", 4), ("GET /d", 3), ("GET /e", 2)]);
        assert_eq!(slow.top_routes[0], RouteStat { route: "GET /a".into(), count: 7, worst_ms: Some(1500), last_code: 502 });
    }

    #[test]
    fn exits_of_the_last_7_days_in_time_order() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "2026-11-09", &[saida("2026-11-09T10:00:00.000-03:00", json!({"tipo": "crash"}))]);
        append(dir.path(), "2026-11-13", &[saida("2026-11-13T19:02:00.000-03:00",
            json!({"tipo": "update", "de": "0.1.0.10751", "para": "0.1.0.10790"}))]);
        append(dir.path(), "2026-11-15", &[
            saida("2026-11-15T06:12:00.000Z", json!({"tipo": "crash", "causa": "out_of_memory",
                "erro": "06:11:58 code=a | 06:11:59 code=b"})),
            saida("2026-11-15T07:00:00.000Z", json!({"tipo": "sumiu"})),
        ]);
        let reader = DiaryReader::at(dir.path().to_owned());
        let exits = reader.exits(utc("2026-11-17T12:00:00Z")).unwrap();
        assert_eq!(exits.items, [
            Exit { ts: "2026-11-13T22:02:00Z".into(), kind: ExitKind::Update, cause: None,
                from: Some("0.1.0.10751".into()), to: Some("0.1.0.10790".into()), error_lines: vec![] },
            Exit { ts: "2026-11-15T06:12:00Z".into(), kind: ExitKind::Crash, cause: Some("out_of_memory".into()),
                from: None, to: None, error_lines: vec!["06:11:58 code=a".into(), "06:11:59 code=b".into()] },
        ]);
        assert_eq!(reader.skipped(), Skipped { lines: 1, files: 0 }, "tipo desconhecido é linha ilegível");
    }

    /// A linha gravada na subida de hoje fala de uma saída de dias atrás: vale o `quando`.
    #[test]
    fn exit_time_is_quando_not_the_boot_ts() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "2026-11-17", &[
            saida("2026-11-17T09:00:00.000-03:00", json!({"tipo": "crash", "quando": "2026-11-09T10:00:00Z"})),
            saida("2026-11-17T09:00:01.000-03:00", json!({"tipo": "requested", "causa": "sigterm", "quando": "2026-11-15T10:00:00Z"})),
            saida("2026-11-17T09:00:02.000-03:00", json!({"tipo": "update", "quando": "2026-11-14T10:00:00Z"})),
            saida("2026-11-17T09:00:03.000-03:00", json!({"tipo": "crash", "quando": "torto"})),
        ]);
        let exits = DiaryReader::at(dir.path().to_owned()).exits(utc("2026-11-17T13:00:00Z")).unwrap();
        let got: Vec<_> = exits.items.iter().map(|exit| (exit.ts.as_str(), exit.kind)).collect();
        assert_eq!(got, [("2026-11-14T10:00:00Z", ExitKind::Update), ("2026-11-15T10:00:00Z", ExitKind::Requested),
            ("2026-11-17T12:00:03Z", ExitKind::Crash)], "fora da janela pelo quando sai; quando ilegível fica com o ts");
    }

    #[test]
    fn watchdog_lines_become_late_and_withheld() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "2026-11-17", &[
            json!({"ts": "2026-11-17T09:07:48.000Z", "evento": "systemd_watchdog", "nivel": "aviso", "tipo": "retido", "ms": 31000}),
            json!({"ts": "2026-11-17T06:00:00.000Z", "evento": "systemd_watchdog", "nivel": "aviso", "tipo": "atrasado", "ms": 4200}),
        ]);
        let slow = DiaryReader::at(dir.path().to_owned()).slow(utc("2026-11-17T12:00:00Z")).unwrap();
        assert_eq!(slow.watchdog, [
            WatchdogEvent { ts: "2026-11-17T06:00:00Z".into(), kind: WatchdogKind::Late, ms: Some(4200) },
            WatchdogEvent { ts: "2026-11-17T09:07:48Z".into(), kind: WatchdogKind::Withheld, ms: Some(31000) },
        ]);
    }

    #[test]
    fn second_read_skips_unchanged_files_and_reads_only_the_new_tail() {
        let dir = tempfile::tempdir().unwrap();
        append(dir.path(), "2026-11-16", &[api("2026-11-16T20:00:00Z", "GET /a", "500", 10)]);
        append(dir.path(), "2026-11-17", &[api("2026-11-17T10:00:00Z", "GET /a", "500", 10)]);
        let reader = DiaryReader::at(dir.path().to_owned());
        let now = utc("2026-11-17T12:00:00Z");
        assert_eq!(reader.slow(now).unwrap().top_routes[0].count, 2);
        assert_eq!(reads(&reader), 2);
        assert_eq!(reader.slow(now).unwrap().top_routes[0].count, 2);
        assert_eq!(reads(&reader), 2, "nada mudou: nenhum arquivo relido");
        append(dir.path(), "2026-11-17", &[api("2026-11-17T11:00:00Z", "GET /a", "500", 10)]);
        assert_eq!(reader.slow(now).unwrap().top_routes[0].count, 3, "a linha nova entra sem duplicar as antigas");
        assert_eq!(reads(&reader), 3, "só o arquivo do dia foi relido");
    }

    #[test]
    fn unreadable_line_is_skipped_and_counted_and_partial_line_waits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("uso-2026-11-17.jsonl");
        std::fs::write(&path, format!("não é json\n{}\n{{\"ts\":\"2026", api("2026-11-17T10:00:00Z", "GET /a", "500", 10))).unwrap();
        let reader = DiaryReader::at(dir.path().to_owned());
        let now = utc("2026-11-17T12:00:00Z");
        assert_eq!(reader.slow(now).unwrap().top_routes.len(), 1);
        assert_eq!(reader.skipped(), Skipped { lines: 1, files: 0 }, "a linha pela metade ainda não conta");
        let mut file = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
        writeln!(file, "-11-17T11:00:00Z\",\"evento\":\"api.servidor\",\"codigo\":\"502\",\"detalhe\":\"GET /a\",\"ms\":5}}").unwrap();
        assert_eq!(reader.slow(now).unwrap().top_routes[0].count, 2);
        assert_eq!(reader.skipped(), Skipped { lines: 1, files: 0 });
    }

    #[test]
    fn diary_off_leaves_the_blocks_absent() {
        // No binário de teste o `diary_dir()` é `None`, como com o diário desligado.
        let reader = DiaryReader::machine();
        let now = utc("2026-11-17T12:00:00Z");
        assert_eq!(reader.exits(now), None);
        assert_eq!(reader.slow(now), None);
    }

    #[test]
    fn empty_diary_gives_present_empty_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let reader = DiaryReader::at(dir.path().to_owned());
        let now = utc("2026-11-17T12:00:00Z");
        assert_eq!(reader.exits(now), Some(ExitsBlock { items: vec![] }));
        let slow = reader.slow(now).unwrap();
        assert!(slow.hours.iter().all(|hour| hour.slow == 0 && hour.errors == 0) && slow.hours.len() == 24);
    }
}
