//! O que vai no pacote, lido do disco e do próprio servidor. Dos logs privados saem só campos
//! conhecidos; das sessões e contas, só o que a seção "Privacidade" do plano permite. Todo texto
//! de fora passa pelo `scrub` antes de qualquer corte: cortar antes parte um segredo e o esconde
//! das regras.
use chrono::{DateTime, Local, NaiveDateTime, TimeZone, Utc};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use super::scrub::{Scrubber, mask_email, shorten_paths};

const VALUE_MAX: usize = 120;
const MESSAGE_MAX: usize = 160;
/// Linha de log mais longa que isso é lida só até aqui.
pub(crate) const LINE_MAX: usize = 4096;
/// Teto fixo de linhas do `hangar-server.log`: só o diário é cortado pelo tamanho do pacote.
pub(crate) const SERVER_LOG_MAX_LINES: usize = 1500;
/// Teto do diário inteiro compactado (C2b).
pub(crate) const DIARY_GZ_MAX: usize = 10 << 20;
pub(crate) const PROBE_WAIT: Duration = Duration::from_secs(5);
/// Leitura do alvo do terminal de uma sessão: lock do registro e um arquivo.
const TARGET_WAIT: Duration = Duration::from_secs(2);

/// Subidas e quedas do backend e do `hangar-server`, conferidas no diário real.
const RESTARTS: [&str; 6] = ["backend.inicio", "backend.encerrando", "hangar_server.de_pe", "hangar_server.caiu",
    "hangar_server.partida", "hangar_server.reserva"];
/// Campos do `hangar-server.log` que podem sair; `detail` e o resto ficam de fora.
const LOG_FIELDS: [&str; 10] = ["code", "session", "reason", "stage", "kind", "window", "outcome", "status", "attempt", "wait_s"];

pub(crate) fn window() -> chrono::TimeDelta { chrono::TimeDelta::hours(24) }

/// Limpa e só depois corta.
pub(crate) fn clean(s: &Scrubber, text: &str, max: usize) -> String { s.scrub(text).trim().chars().take(max).collect() }

/// Cada linha do arquivo, até `LINE_MAX` bytes, sem carregar o arquivo nem a linha inteira.
pub(crate) fn each_line(path: &Path, mut f: impl FnMut(&str)) {
    let Ok(file) = std::fs::File::open(path) else { return };
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut line: Vec<u8> = Vec::with_capacity(512);
    let mut emit = |line: &mut Vec<u8>| {
        f(String::from_utf8_lossy(line).trim_end_matches('\r'));
        line.clear();
    };
    loop {
        let (used, ended) = match reader.fill_buf() {
            Ok([]) | Err(_) => break,
            Ok(buf) => {
                let end = memchr::memchr(b'\n', buf);
                let chunk = &buf[..end.unwrap_or(buf.len())];
                let room = LINE_MAX.saturating_sub(line.len());
                line.extend_from_slice(&chunk[..chunk.len().min(room)]);
                (end.map_or(buf.len(), |i| i + 1), end.is_some())
            }
        };
        reader.consume(used);
        if ended { emit(&mut line); }
    }
    if !line.is_empty() { emit(&mut line); }
}

// ---------- diário ----------

#[derive(Clone, Debug)]
pub(crate) struct DiaryEntry {
    pub at: DateTime<Local>,
    pub event: String,
    pub level: String,
    pub code: String,
    pub session: String,
    pub reason: String,
    pub backend: String,
}

impl DiaryEntry {
    /// `MM-DD hh:mm:ss evento nível código sessão motivo`; campo vazio vira `-`.
    pub(crate) fn compact(&self) -> String {
        let dash = |v: &str| if v.is_empty() { "-".to_owned() } else { v.to_owned() };
        let line = format!("{} {} {} {} {} {}", self.at.format("%m-%d %H:%M:%S"), dash(&self.event), dash(&self.level),
            dash(&self.code), dash(&self.session), self.reason);
        line.trim_end().to_owned()
    }

    pub(crate) fn restart_line(&self) -> String {
        format!("{} {} {} {} {}", self.at.format("%m-%d %H:%M:%S"), self.event, self.level,
            if self.code.is_empty() { "-" } else { &self.code }, self.backend).trim_end().to_owned()
    }
}

#[derive(Default)]
pub(crate) struct Diary {
    /// Nível diferente de `ok`, da mais antiga para a mais nova.
    pub problems: Vec<DiaryEntry>,
    pub restarts: Vec<DiaryEntry>,
}

/// Todo `uso-AAAA-MM-DD.jsonl` sob `diario/`, por data e caminho. `privado/` fica fora por construção.
pub(crate) fn diary_files(base: &Path) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, depth: u8, out: &mut Vec<(String, PathBuf)>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            let path = entry.path();
            if kind.is_dir() {
                if depth < 4 { walk(&path, depth + 1, out); }
                continue;
            }
            if !kind.is_file() { continue; }
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(date) = name.strip_prefix("uso-").and_then(|n| n.strip_suffix(".jsonl"))
                .filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok()) {
                out.push((date.to_owned(), path));
            }
        }
    }
    let mut out = Vec::new();
    walk(&base.join("diario"), 0, &mut out);
    out.sort();
    out
}

fn text_field(s: &Scrubber, value: &Value) -> String {
    match value {
        Value::String(text) => clean(s, text, VALUE_MAX),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// As últimas 24 h do diário (`paths` de `diary_files`): problemas (nível ≠ `ok`) e reinícios, por hora.
pub(crate) fn read_diary(paths: &[(String, PathBuf)], now: DateTime<Local>, s: &Scrubber) -> Diary {
    let since = now - window();
    let days = [now.date_naive(), since.date_naive()].map(|d| d.format("%Y-%m-%d").to_string());
    let (mut problems, mut restarts) = (Vec::new(), Vec::new());
    for (_, path) in paths.iter().filter(|(date, _)| days.contains(date)) {
        each_line(path, |line| {
            let Ok(value) = serde_json::from_str::<Value>(line) else { return };
            let Some(at) = value["ts"].as_str().and_then(|t| DateTime::parse_from_rfc3339(t).ok()) else { return };
            let at = at.with_timezone(&Local);
            if at < since { return; }
            let (event, level) = (value["evento"].as_str().unwrap_or(""), value["nivel"].as_str().unwrap_or(""));
            let restart = RESTARTS.contains(&event);
            if level == "ok" && !restart { return; }
            let entry = DiaryEntry { at, event: text_field(s, &value["evento"]), level: text_field(s, &value["nivel"]),
                code: text_field(s, &value["codigo"]), session: text_field(s, &value["sessao"]),
                reason: text_field(s, &value["motivo"]), backend: text_field(s, &value["backend"]) };
            if level != "ok" { problems.push(entry.clone()); }
            if restart { restarts.push(entry); }
        });
    }
    problems.sort_by_key(|e| e.at);
    restarts.sort_by_key(|e| e.at);
    Diary { problems, restarts }
}

// ---------- hangar-server.log ----------

static SERVER_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\S+Z)\s+(WARN|ERROR)\s+(.*)$").unwrap());
static FIELD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s([a-z_][a-z0-9_]*)=").unwrap());
static MODULE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z_][a-z0-9_]*(?:::[a-z0-9_]+)+:\s+").unwrap());

/// Uma linha `WARN`/`ERROR` desde `since`, reduzida a hora local, nível, mensagem e os campos
/// permitidos. O valor sem aspas vai até o próximo ` chave=`: o `tracing` não aspa `%valor` com espaço.
/// A mensagem não é fixa: além do `scrub`, caminho absoluto vira o último segmento.
pub(crate) fn reduce_server_line(line: &str, since: DateTime<Utc>, s: &Scrubber) -> Option<String> {
    let c = SERVER_LINE.captures(line)?;
    let at = DateTime::parse_from_rfc3339(&c[1]).ok()?.with_timezone(&Utc);
    if at < since { return None; }
    let body = MODULE.replace(&c[3], "");
    let marks: Vec<(usize, usize, &str)> = FIELD.captures_iter(&body)
        .map(|f| { let all = f.get(0).unwrap(); (all.start(), all.end(), f.get(1).unwrap().as_str()) }).collect();
    let tidy = |text: &str, max: usize| shorten_paths(&clean(s, text, usize::MAX)).chars().take(max).collect::<String>();
    let message = &body[..marks.first().map_or(body.len(), |m| m.0)];
    let mut fields: BTreeMap<&str, String> = BTreeMap::new();
    for (i, (_, end, key)) in marks.iter().enumerate() {
        let value = body[*end..marks.get(i + 1).map_or(body.len(), |m| m.0)].trim();
        let value = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).unwrap_or(value);
        if LOG_FIELDS.contains(key) { fields.entry(key).or_insert_with(|| tidy(value, VALUE_MAX)); }
    }
    let mut out = format!("{} {} {}", at.with_timezone(&Local).format("%m-%d %H:%M:%S"), &c[2], tidy(message, MESSAGE_MAX));
    for key in LOG_FIELDS {
        if let Some(value) = fields.get(key) { out += &format!(" {key}={value}"); }
    }
    Some(out)
}

/// A hora da linha dentro da janela, sem regex: decide quem entra no buffer antes da redução.
fn in_window(line: &str, since: DateTime<Utc>) -> bool {
    line.split_ascii_whitespace().next().filter(|ts| ts.ends_with('Z'))
        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok()).is_some_and(|at| at.with_timezone(&Utc) >= since)
}

/// `.4` até o atual; acima do teto ficam as mais novas e uma nota com quantas saíram. O buffer
/// guarda a linha crua: só as que ficam passam pela redução e pelo `scrub`.
// shortcut: lê o texto do fmt do tracing; trocar por log em JSON se o formato mudar.
pub(crate) fn read_server_log(base: &Path, now: DateTime<Local>, s: &Scrubber) -> Vec<String> {
    let since = (now - window()).with_timezone(&Utc);
    let dir = base.join("privado");
    let mut raw = std::collections::VecDeque::new();
    let mut dropped = 0usize;
    for n in (0..=4).rev() {
        let name = if n == 0 { "hangar-server.log".to_owned() } else { format!("hangar-server.log.{n}") };
        each_line(&dir.join(name), |line| {
            if !(line.contains(" WARN ") || line.contains(" ERROR ")) || !in_window(line, since) { return; }
            raw.push_back(line.to_owned());
            if raw.len() > SERVER_LOG_MAX_LINES { raw.pop_front(); dropped += 1; }
        });
    }
    let mut out: Vec<String> = raw.iter().filter_map(|line| reduce_server_line(line, since, s)).collect();
    if dropped > 0 { out.insert(0, format!("({dropped} linhas mais antigas fora)")); }
    out
}

// ---------- log do Python ----------

static PYTHON_LINE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}),\d+ (WARNING|ERROR|CRITICAL) \[([^\]]{1,80})\]").unwrap()
});

/// Aviso e erro por logger nas 24 h; mensagem nenhuma. A hora do log do Python é local, sem fuso.
pub(crate) fn python_counts(base: &Path, now: DateTime<Local>, s: &Scrubber) -> BTreeMap<String, [u32; 2]> {
    let since = now - window();
    let dir = base.join("privado");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).into_iter().flatten().flatten()
        .filter(|e| { let n = e.file_name().to_string_lossy().into_owned(); n.starts_with("backend.log") || n.starts_with("hangar-backend.log") })
        .map(|e| e.path()).collect();
    files.sort();
    let mut counts: BTreeMap<String, [u32; 2]> = BTreeMap::new();
    for path in files {
        each_line(&path, |line| {
            let Some(c) = PYTHON_LINE.captures(line) else { return };
            let Some(at) = NaiveDateTime::parse_from_str(&c[1], "%Y-%m-%d %H:%M:%S").ok()
                .and_then(|n| Local.from_local_datetime(&n).earliest()) else { return };
            if at < since { return; }
            counts.entry(clean(s, &c[3], 80)).or_default()[usize::from(&c[2] != "WARNING")] += 1;
        });
    }
    counts
}

// ---------- atualizações ----------

static UPDATE_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\S+Z)\s+\w+\s+update: fim\b\s*(.*)$").unwrap());

/// As 10 últimas `update: fim` de `privado/atualizacao-rust.log`, com data local.
pub(crate) fn read_updates(base: &Path, s: &Scrubber) -> Vec<String> {
    let mut found = std::collections::VecDeque::new();
    each_line(&base.join("privado").join("atualizacao-rust.log"), |line| {
        let Some(c) = UPDATE_LINE.captures(line) else { return };
        let Some(at) = DateTime::parse_from_rfc3339(&c[1]).ok() else { return };
        let rest: String = shorten_paths(&clean(s, &c[2], usize::MAX)).chars().take(200).collect();
        found.push_back(format!("{} {rest}", at.with_timezone(&Local).format("%m-%d %H:%M")));
        if found.len() > 10 { found.pop_front(); }
    });
    found.into()
}

// ---------- diário inteiro compactado ----------

fn gzip(data: impl FnOnce(&mut flate2::write::GzEncoder<Vec<u8>>) -> std::io::Result<()>) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    // Escrita em memória só falha sem memória; o que já entrou vale.
    let _ = data(&mut encoder);
    encoder.finish().unwrap_or_default()
}

/// Todo o diário, em ordem de data, cada linha pelo `scrub`: um membro gzip por dia, concatenados
/// (o `gunzip` lê como um arquivo só). Compacta do dia mais novo para o mais antigo e para no
/// primeiro que não cabe em `max`; a primeira linha diz quantos dias ficaram fora.
pub(crate) fn diary_gz(paths: &[(String, PathBuf)], scrubber: &Scrubber, max: usize) -> Vec<u8> {
    const NOTE_ROOM: usize = 256;
    let mut days: BTreeMap<&str, Vec<&Path>> = BTreeMap::new();
    for (date, path) in paths { days.entry(date.as_str()).or_default().push(path.as_path()); }
    let mut members: Vec<Vec<u8>> = Vec::new();
    let mut total = 0usize;
    for paths in days.values().rev() {
        let member = gzip(|out| {
            let mut result = Ok(());
            for path in paths {
                each_line(path, |line| {
                    if result.is_err() || line.trim().is_empty() { return; }
                    result = out.write_all(scrubber.scrub(line).as_bytes()).and_then(|()| out.write_all(b"\n"));
                });
            }
            result
        });
        if total + member.len() + NOTE_ROOM > max { break; }
        total += member.len();
        members.push(member);
    }
    let dropped = days.len() - members.len();
    let mut out = Vec::with_capacity(total + NOTE_ROOM);
    if dropped > 0 {
        let note = format!("{}\n", json!({"evento": "diag.cortado", "dias_fora": dropped}));
        out.extend(gzip(|w| w.write_all(note.as_bytes())));
    }
    for member in members.iter().rev() { out.extend_from_slice(member); }
    out
}

// ---------- sessões ----------

#[derive(Clone, Debug, Default)]
pub(crate) struct RuntimeFact {
    pub error: Option<String>,
    pub input_stalled: Option<String>,
    pub alive: Option<bool>,
    pub generation: Option<u64>,
    pub waiting: Option<usize>,
    /// Claude com terminal: o plugin pedindo (`/pull`), que prova a conversa quando falta o registro nativo.
    pub plugin: Option<crate::plugin::PullStatus>,
    /// Claude com terminal: o `sessions/<pid>.json` do agente existe.
    pub native_record: Option<bool>,
}

#[derive(Clone, Debug)]
pub(crate) struct SessionFact {
    pub name: String,
    /// Vazio: a sessão só existe no runtime, sem linha na lista.
    pub provider: String,
    pub terminal: Option<bool>,
    pub state: String,
    pub problem: Option<String>,
    pub runtime: Option<RuntimeFact>,
}

impl SessionFact {
    pub(crate) fn troubled(&self) -> bool {
        self.problem.is_some() || self.state == "dead"
            || self.runtime.as_ref().is_some_and(|r| r.error.is_some() || r.input_stalled.is_some())
    }

    pub(crate) fn line(&self) -> String {
        let mut out = self.name.clone();
        if !self.provider.is_empty() { out += &format!(" · {}", self.provider); }
        match self.terminal {
            Some(true) => out += " · com terminal",
            Some(false) => out += " · sem terminal",
            None => {}
        }
        if !self.state.is_empty() { out += &format!(" · {}", self.state); }
        if let Some(problem) = &self.problem { out += &format!(" · problema={problem}"); }
        if let Some(r) = &self.runtime {
            out += " · runtime:";
            if let Some(g) = r.generation { out += &format!(" geração={g}"); }
            if let Some(alive) = r.alive { out += &format!(" vivo={alive}"); }
            if let Some(e) = &r.error { out += &format!(" erro={e}"); }
            if let Some(s) = &r.input_stalled { out += &format!(" entrada_parada={s}"); }
            out += &format!(" fila={}", r.waiting.map_or("?".to_owned(), |n| n.to_string()));
            if let Some(native) = r.native_record { out += &format!(" registro_nativo={}", if native { "sim" } else { "não" }); }
            if let Some(p) = &r.plugin {
                out += &format!(" plugin: pull_aberto={} ultimo_pull={} conversa_nova={} fim={}", p.open,
                    p.last_seen_s.map_or("nunca".to_owned(), |s| format!("{s}s")), p.reported, p.ended);
            }
        }
        out
    }
}

/// Só nome, provedor, com/sem terminal, estado e problema: o resto da linha (última resposta,
/// pergunta, opções, rótulo, formulário, linha de status) nunca sai.
pub(crate) fn session_fact(row: &hangar_api::session::SessionRow, s: &Scrubber) -> SessionFact {
    SessionFact { name: clean(s, &row.name, VALUE_MAX), provider: clean(s, &row.provider, 40), terminal: Some(!row.headless),
        state: clean(s, &row.state, 40), problem: row.problema.as_deref().map(|p| clean(s, p, VALUE_MAX)), runtime: None }
}

/// Do retrato do ator, só erro, entrada parada, vivo e geração.
pub(crate) fn runtime_fact(snapshot: &Value, waiting: Option<usize>, s: &Scrubber) -> RuntimeFact {
    let text = |v: &Value| v.as_str().filter(|t| !t.is_empty()).map(|t| clean(s, t, VALUE_MAX));
    RuntimeFact { error: text(&snapshot["error"]), input_stalled: text(&snapshot["view"]["input_stalled"]),
        alive: snapshot["view"]["alive"].as_bool(), generation: snapshot["generation"].as_u64(), waiting, plugin: None, native_record: None }
}

/// Mensagens esperando no estado da fila; `None` se o arquivo não se leu.
fn count_waiting(path: &Path, created: f64) -> Option<usize> {
    let bytes = match crate::runtime::queue::read_replaced(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Some(0),
        Err(_) => return None,
    };
    let (state, _) = crate::runtime::queue::State::load(&bytes).ok()?;
    Some(crate::runtime::queue::waiting_ids(&state, created).len())
}

/// A lista não respondeu no prazo.
pub(crate) const LIST_TIMEOUT: &str = "list_timeout";

pub(crate) struct Sessions {
    pub list_error: Option<&'static str>,
    pub facts: Vec<SessionFact>,
}

pub(crate) async fn sessions(st: &crate::routes::AppState, s: &Scrubber) -> Sessions {
    let list = async {
        match tokio::time::timeout(PROBE_WAIT, st.list.snapshot()).await {
            Ok(Ok(produced)) => (None, produced.rows.iter().map(|row| session_fact(row, s)).collect::<Vec<_>>()),
            Ok(Err(error)) => (Some(error.code), Vec::new()),
            Err(_) => (Some(LIST_TIMEOUT), Vec::new()),
        }
    };
    let runtime = async {
        let Some(registry) = st.state.runtime.get() else { return Vec::new() };
        let (files, snapshots) = tokio::join!(registry.queue_files(), registry.list_snapshots());
        let counted = tokio::time::timeout(PROBE_WAIT, tokio::task::spawn_blocking(move || {
            files.into_iter().map(|(key, (name, path, created))| (key, name, count_waiting(&path, created))).collect::<Vec<_>>()
        })).await;
        let counted = match counted { Ok(Ok(counted)) => counted, _ => Vec::new() };
        let mut out = Vec::new();
        // Cada alvo com prazo próprio: o registro preso (fila no meio de uma troca) não leva junto os fatos já
        // lidos. Estourou uma vez, as seguintes esperariam o mesmo lock: ficam sem a linha do plugin.
        let mut stuck = false;
        for (key, name, waiting) in counted {
            let mut fact = runtime_fact(snapshots.get(&key).unwrap_or(&Value::Null), waiting, s);
            let read = async {
                let target = registry.terminal_target(&name).await?;
                let record = match &target {
                    Some(target) => tokio::fs::try_exists(target.metadata.config_dir.join("sessions").join(format!("{}.json", target.metadata.agent_pid))).await.ok(),
                    None => None,
                };
                Ok::<_, crate::runtime::protocol::RuntimeError>((target, record))
            };
            match if stuck { None } else { tokio::time::timeout(TARGET_WAIT, read).await.ok() } {
                Some(Ok((Some(target), record))) => {
                    fact.native_record = record;
                    fact.plugin = registry.plugin().ok().and_then(|plugin| plugin
                        .pull_status(&crate::terminal_facts::LocalTerminalFacts::binding(&target), std::time::Instant::now()).ok());
                }
                Some(Ok((None, _))) => {}
                // O relatório sem a linha do plugin é o que diz "não consegui ler"; o motivo fica no log.
                Some(Err(error)) => tracing::warn!(session = %name, code = %error.code, "relatório: alvo do terminal ilegível"),
                None => {
                    if !stuck { tracing::warn!(session = %name, "relatório: alvo do terminal sem resposta no prazo; as demais ficam sem a linha do plugin"); }
                    stuck = true;
                }
            }
            out.push((clean(s, &name, VALUE_MAX), fact));
        }
        out
    };
    let ((list_error, mut facts), runtime) = tokio::join!(list, runtime);
    for (name, fact) in runtime {
        match facts.iter_mut().find(|f| f.name == name) {
            // Nome repetido é a vida anterior ainda fechando; vale a geração mais nova.
            Some(row) if row.runtime.as_ref().is_none_or(|r| r.generation < fact.generation) => row.runtime = Some(fact),
            Some(_) => {}
            None => facts.push(SessionFact { name, provider: String::new(), terminal: None, state: String::new(), problem: None, runtime: Some(fact) }),
        }
    }
    Sessions { list_error, facts }
}

// ---------- contas ----------

#[derive(Clone, Debug)]
pub(crate) struct AccountFact {
    pub label: String,
    pub provider: String,
    pub active: bool,
    pub state: String,
    /// (rótulo, percentual 0–100, reset em segundos)
    pub windows: Vec<(String, Option<f64>, Option<f64>)>,
    pub refresh_expires_at: Option<f64>,
    pub spent_until: Option<f64>,
}

impl AccountFact {
    pub(crate) fn near_limit(&self) -> bool { self.windows.iter().any(|(_, pct, _)| pct.is_some_and(|p| p >= 90.0)) }
    pub(crate) fn login_expiring(&self, now: f64) -> bool { self.refresh_expires_at.is_some_and(|at| at - now < 3.0 * 86400.0) }

    pub(crate) fn line(&self, now: f64) -> String {
        let when = |at: f64| Local.timestamp_opt(at as i64, 0).single().map_or("?".to_owned(), |t| t.format("%m-%d %H:%M").to_string());
        let mut out = format!("{} · {} · {} · {}", self.label, self.provider, if self.active { "ativa" } else { "inativa" }, self.state);
        for (label, pct, reset) in &self.windows {
            out += &format!(" · {label} {}%", pct.map_or("?".to_owned(), |p| format!("{p:.0}")));
            if let Some(reset) = reset { out += &format!(" (reset {})", when(*reset)); }
        }
        if let Some(at) = self.refresh_expires_at {
            out += &format!(" · login vence {}{}", when(at), if self.login_expiring(now) { " (perto)" } else { "" });
        }
        if let Some(until) = self.spent_until.filter(|u| *u > now) { out += &format!(" · gasta até {}", when(until)); }
        out
    }
}

/// Só rótulo, provedor, ativa, estado, janelas, vencimento do login e quando gasta. O `id` (tem o
/// caminho da casa), o caminho, a assinatura da credencial e os créditos de reset nunca saem.
pub(crate) fn account_fact(row: &Value, spent: &BTreeMap<String, f64>, s: &Scrubber) -> AccountFact {
    let dir = row["id"].as_str().and_then(|id| id.split_once(':')).map(|(_, dir)| dir).filter(|d| !d.is_empty());
    let spent_until = dir.and_then(|dir| spent.get(&crate::list::discover::real(dir)).or_else(|| spent.get(dir))).copied();
    let text = |v: &Value, max: usize| clean(s, v.as_str().unwrap_or("?"), max);
    let windows = row["janelas"].as_array().map(|w| w.iter().filter(|w| w["por_modelo"] != true)
        .map(|w| (text(&w["rotulo"], 20), w["pct"].as_f64(), w["reset_ts"].as_f64())).collect()).unwrap_or_default();
    AccountFact {
        label: mask_email(&text(&row["label"], 80)),
        provider: text(&row["provedor"], 20),
        active: row["ativa"] == true,
        state: text(&row["estado"], 40),
        windows,
        refresh_expires_at: row["refresh_expires_at"].as_f64(),
        spent_until,
    }
}

pub(crate) async fn accounts(st: &crate::routes::AppState, s: &Scrubber) -> Result<Vec<AccountFact>, String> {
    let spent_path = st.accounts.quota_path().with_file_name("limit-spent.json");
    let (rows, spent) = tokio::join!(tokio::time::timeout(PROBE_WAIT, st.accounts.quotas(None, false, true)), tokio::fs::read(&spent_path));
    let rows = rows.map_err(|_| "quota_timeout".to_owned())?.map_err(|e| e.code.to_owned())?;
    let spent: BTreeMap<String, f64> = spent.ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default();
    Ok(rows.as_array().map(|rows| rows.iter().map(|row| account_fact(row, &spent, s)).collect()).unwrap_or_default())
}

// ---------- versões ----------

#[derive(Clone, Debug)]
pub(crate) struct Versions {
    pub server: String,
    /// Versão do pacote ou `dev` (modo desenvolvedor).
    pub package: String,
    pub commit: String,
    pub os: String,
    pub arch: String,
    pub claude: String,
    pub mux: String,
    /// Fuso do servidor, que dá a hora das linhas.
    pub offset: String,
}

impl Versions {
    /// O `app` do relatório: `hangar-server <versão do pacote ou do crate>`.
    pub(crate) fn app(&self) -> String {
        format!("hangar-server {}", if self.package == "dev" || self.package.is_empty() { &self.server } else { &self.package })
    }

    /// Só o que não depende de sonda: serve quando o prazo do `build` estoura antes.
    pub(crate) fn local() -> Self {
        let (package, commit) = match crate::update::running_package() {
            Some(id) => (id.version.clone(), id.commit.chars().take(9).collect()),
            None => ("dev".to_owned(), crate::update::dev::boot_head().map(|h| h.chars().take(9).collect()).unwrap_or_default()),
        };
        Versions { server: env!("CARGO_PKG_VERSION").to_owned(), package, commit, os: "?".into(),
            arch: format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH), claude: "?".into(), mux: "?".into(),
            offset: Local::now().format("%:z").to_string() }
    }
}

pub(crate) async fn versions(st: &crate::routes::AppState, s: &Scrubber) -> Versions {
    let claude = async {
        let Some(mut command) = st.accounts.claude_command() else { return "?".to_owned() };
        // A autoridade do servidor não vai ao processo filho.
        for key in crate::terminal_process::PRIVATE_ENV_KEYS { command.env_remove(key); }
        command.arg("--version").stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true);
        match tokio::time::timeout(PROBE_WAIT, command.output()).await {
            Ok(Ok(out)) if out.status.success() => String::from_utf8_lossy(&out.stdout).lines().next().map(|l| clean(s, l, 80)).unwrap_or_else(|| "?".into()),
            _ => "?".to_owned(),
        }
    };
    let mux = async {
        crate::list::mux::Mux::with_program("tmux", PROBE_WAIT).version().await.map(|v| clean(s, &v, 80)).unwrap_or_else(|| "?".into())
    };
    let os = async {
        match tokio::time::timeout(PROBE_WAIT, tokio::task::spawn_blocking(sysinfo::System::long_os_version)).await {
            Ok(Ok(Some(os))) => clean(s, &os, 80),
            _ => "?".to_owned(),
        }
    };
    let (claude, mux, os) = tokio::join!(claude, mux, os);
    Versions { claude, mux, os, ..Versions::local() }
}
