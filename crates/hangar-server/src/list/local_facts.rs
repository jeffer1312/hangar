//! Fatos da lista que o Python dava ao Rust (`list_facts.py`) e que o Rust lê sozinho: prazo de
//! travada, dono de convidado, orquestrações, problema do Claude sem terminal e o que a linha do
//! Codex precisa do rollout. O Python só é perguntado pelas linhas de Pi, omp e Kimi.
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::time::Instant;

use hangar_api::session::SessionRow;
use serde_json::{json, Value};

/// `settings.stall_seconds` (`CP_STALL_SECONDS`, padrão 300).
const STALL_DEFAULT: f64 = 300.0;
/// `runs.ACTIVE_S`: orquestração mexida há menos que isso está trabalhando.
const ORQ_ACTIVE_S: f64 = 120.0;
const ORQ_EVENTS: [&str; 6] = ["execucao_inicio", "task_inicio", "entrega", "veredito", "sessao_trocada", "execucao_fim"];
/// `_STATUS_TAIL` do adaptador Codex.
const STATUS_TAIL: u64 = 512 << 10;
/// `registry._STATUS_TTL`.
const STATUS_TTL_S: f64 = 20.0;

/// `runtime_config.get("stall_seconds")`: o override do arquivo vence o ambiente.
pub fn stall_seconds(claude: &Path) -> f64 {
    let file = std::fs::read(claude.join("runtime-config.json")).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
    if let Some(v) = file.as_ref().and_then(|d| d.get("stall_seconds")).and_then(Value::as_f64) {
        return v;
    }
    std::env::var("CP_STALL_SECONDS").ok().and_then(|v| v.trim().parse::<i64>().ok()).map_or(STALL_DEFAULT, |v| v as f64)
}

/// `guests.json` como o `guest_users._load` o lê. Ilegível vale a última leitura boa (sem nenhuma,
/// ninguém é convidado): ler no meio de uma escrita não mostra ao dono a sessão escondida.
#[derive(Debug, Default, Clone)]
pub struct Guests {
    /// id → (nome, o dono vê as sessões dele).
    guests: HashMap<String, (String, bool)>,
    /// sessão → (id do convidado, vida gravada).
    sessions: HashMap<String, (String, Option<String>)>,
}

impl Guests {
    pub fn read(claude: &Path) -> Self {
        static LAST: LazyLock<Mutex<HashMap<PathBuf, Guests>>> = LazyLock::new(Mutex::default);
        let path = claude.join("guests.json");
        let parsed = match std::fs::read(&path) {
            Ok(raw) => Self::parse(&raw),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Some(Self::default()),
            Err(_) => None,
        };
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        match parsed {
            Some(guests) => { last.insert(path, guests.clone()); guests }
            None => {
                if crate::warn_limit::allow(None, "guests_unreadable") {
                    tracing::warn!(code = "guests_unreadable", "guests.json ilegível; fica a última leitura boa");
                }
                last.get(&path).cloned().unwrap_or_default()
            }
        }
    }

    fn parse(raw: &[u8]) -> Option<Self> {
        let raw: Value = serde_json::from_slice(raw).ok()?;
        let mut guests = HashMap::new();
        for g in raw.get("guests").map_or(Some(&Vec::new()), Value::as_array)? {
            // `Guest(**g)`: campo obrigatório faltando ou de sobra torna o arquivo inteiro ilegível.
            let known = ["id", "name", "token_hash", "root", "sees_owner", "owner_sees"];
            let obj = g.as_object()?;
            if obj.keys().any(|k| !known.contains(&k.as_str())) || ["id", "name", "token_hash", "root"].iter().any(|k| !obj.contains_key(*k)) {
                return None;
            }
            let owner_sees = obj.get("owner_sees").map_or(Some(true), Value::as_bool)?;
            guests.insert(g["id"].as_str()?.to_owned(), (g["name"].as_str()?.to_owned(), owner_sees));
        }
        let mut sessions = HashMap::new();
        for (name, entry) in raw.get("sessions").and_then(Value::as_object).into_iter().flatten() {
            let Some(guest) = entry["guest"].as_str() else { continue };
            sessions.insert(name.clone(), (guest.to_owned(), entry["life"].as_str().map(str::to_owned)));
        }
        Some(Self { guests, sessions })
    }

    pub fn has_claims(&self) -> bool { !self.sessions.is_empty() }

    pub fn claimed(&self, name: &str) -> bool { self.sessions.contains_key(name) }

    /// `owner_of`: só a vida gravada desfaz o dono. Vida gravada vazia vale sem gravar: quem adota
    /// a vida é a abertura, a leitura da lista não escreve.
    fn owner(&self, name: &str, life: Option<&str>) -> Option<&(String, bool)> {
        let (guest, stored) = self.sessions.get(name)?;
        let owner = self.guests.get(guest)?;
        match (stored, life) {
            (None, _) | (_, None) => Some(owner),
            (Some(stored), Some(life)) => (stored == life).then_some(owner),
        }
    }

    /// Dono de cada sessão e as escondidas do dono, para os nomes dados com a vida de cada um.
    pub fn access(&self, names: &[(String, Option<String>)]) -> (HashMap<String, String>, Vec<String>) {
        let (mut owners, mut hidden) = (HashMap::new(), Vec::new());
        if !self.has_claims() {
            return (owners, hidden);
        }
        for (name, life) in names {
            if let Some((owner, sees)) = self.owner(name, life.as_deref()) {
                owners.insert(name.clone(), owner.clone());
                if !sees { hidden.push(name.clone()); }
            }
        }
        hidden.sort();
        hidden.dedup();
        (owners, hidden)
    }
}

fn text(v: &Value) -> Option<&str> { v.as_str().filter(|s| !s.is_empty()) }

/// `datetime.fromisoformat(...).astimezone()` em segundos; sem fuso vale a hora local.
fn iso_epoch(v: &Value) -> Option<f64> {
    use chrono::{DateTime, Local, NaiveDateTime, TimeZone};
    let s = v.as_str()?.trim();
    if let Ok(t) = DateTime::parse_from_rfc3339(s) {
        return Some(t.timestamp_micros() as f64 / 1e6);
    }
    ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M", "%Y-%m-%d"].iter().find_map(|f| {
        let naive = NaiveDateTime::parse_from_str(s, f).ok()
            .or_else(|| chrono::NaiveDate::parse_from_str(s, f).ok().and_then(|d| d.and_hms_opt(0, 0, 0)))?;
        Local.from_local_datetime(&naive).earliest().map(|t| t.timestamp_micros() as f64 / 1e6)
    })
}

fn modified(path: &Path) -> Option<f64> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()?
        .duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

fn task_number(v: &Value) -> bool { v.is_i64() || v.is_u64() }

/// Leitura de um arquivo da orquestração: `Ok(None)` ausente, `Err` ilegível (disco, escrita no meio).
type OrqRead<T> = Result<Option<T>, ()>;

fn read_json(path: &Path) -> OrqRead<Value> {
    match std::fs::read(path) {
        Ok(raw) => serde_json::from_slice(&raw).map(Some).map_err(|_| ()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(()),
    }
}

/// `orq._le_eventos` sem `strict`: linha ruim é pulada, só os tipos conhecidos ficam.
fn orq_events(path: &Path) -> Result<Vec<Value>, ()> {
    let raw = match std::fs::read(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(()),
    };
    Ok(String::from_utf8_lossy(&raw).lines().filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
        .filter(|e| e["tipo"].as_str().is_some_and(|t| ORQ_EVENTS.contains(&t))).collect())
}

/// `runs._auto_run`: (orq.json, gid, encerrada) de uma execução `auto` com início gravado.
fn auto_run(dir: &Path) -> OrqRead<(Value, String, bool)> {
    let Some(cfg) = read_json(&dir.join("orq.json"))? else { return Ok(None) };
    if cfg.get("auto") != Some(&Value::Bool(true)) {
        return Ok(None);
    }
    let events = orq_events(&dir.join("eventos.jsonl"))?;
    let Some(gid) = events.iter().find(|e| e["tipo"] == "execucao_inicio").and_then(|e| text(&e["gid"])).map(str::to_owned) else { return Ok(None) };
    // `orq._current_end`: evento de Task depois do fim reabre a execução.
    let mut ended = false;
    for e in &events {
        if e["tipo"] == "execucao_fim" { ended = true } else if task_number(&e["task"]) { ended = false }
    }
    Ok(Some((cfg, gid, ended)))
}

/// `orq_conductor._pid_alive`: só o Linux confere; fora dele vale vivo.
fn pid_alive(pid: &Value) -> bool {
    if !cfg!(target_os = "linux") || pid.is_boolean() || !task_number(pid) {
        return true;
    }
    pid.as_i64().is_some_and(|p| Path::new(&format!("/proc/{p}")).exists())
}

/// `orq_conductor.watchdog(d, None)` sem systemd: o batimento do vigia decide se a execução vive.
fn watchdog(dir: &Path, now: f64) -> OrqRead<Value> {
    let Some(hb) = read_json(&dir.join("vigia.json"))? else { return Ok(None) };
    let Some(ts) = iso_epoch(&hb["ts"]) else { return Ok(None) };
    let iv = hb["interval_s"].as_i64().filter(|v| *v >= 0 && !hb["interval_s"].is_boolean()).unwrap_or(60);
    let fresh = now - ts <= (2 * iv + 30) as f64;
    Ok((fresh && pid_alive(&hb["pid"])).then_some(hb))
}

/// `runs._held`: trava presa por alguém, lida em `/proc/locks` (pegar a trava faria o `advance` desistir).
fn held(lock: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(ino) = std::fs::metadata(lock).map(|m| m.ino()) else { return false };
        let Ok(locks) = std::fs::read(Path::new("/proc/locks")) else { return false };
        let suffix = format!(":{ino}");
        String::from_utf8_lossy(&locks).lines().any(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            f.len() > 5 && f[1] == "FLOCK" && f[5].ends_with(&suffix)
        })
    }
    #[cfg(not(unix))]
    { let _ = lock; false }
}

/// `runs.activity`: trabalhando com a trava do `advance` presa ou com a linha do tempo mexida há pouco.
fn orq_activity(timeline: &Path, now: f64) -> (&'static str, Option<f64>) {
    let lock = timeline.parent().unwrap_or(Path::new("")).join("advance.lock");
    let last = [modified(timeline), modified(&lock)].into_iter().flatten().reduce(f64::max);
    let busy = held(&lock) || last.is_some_and(|l| now - l < ORQ_ACTIVE_S);
    (if busy { "working" } else { "idle" }, last)
}

/// A linha de uma execução: `Ok(None)` quando ela não está viva, `Err` quando um arquivo não leu.
fn orq_row(dir: &Path, now: f64) -> OrqRead<SessionRow> {
    let Some((cfg, gid, ended)) = auto_run(dir)? else { return Ok(None) };
    if ended { return Ok(None); }
    let Some(hb) = watchdog(dir, now)? else { return Ok(None) };
    let resolved = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_owned());
    let Some(name) = resolved.file_name().map(|n| n.to_string_lossy().into_owned()) else { return Ok(None) };
    let timeline = dir.join(format!("timeline-{name}.jsonl"));
    let (state, last) = orq_activity(&timeline, now);
    let arbiter = text(&hb["arbiter"]).or_else(|| text(&cfg["arbiter"]));
    Ok(serde_json::from_value(json!({
        "name": format!("{gid}-orq"), "cwd": text(&cfg["repo"]), "jsonl": timeline.to_string_lossy(),
        "provider": "orq", "tracked": true, "pair_gid": gid, "orq_arbiter": arbiter,
        "state": state, "last_activity": last,
    })).ok())
}

/// `runs.active()` + `list_facts._orq_rows`: uma linha por execução `auto` viva em `~/.hangar/orq`.
/// Arquivo que não leu mantém a última linha boa daquela execução (`keep_orq`), e o `true` diz que
/// houve falha, para o diário.
pub fn orq_rows(home: &Path, now: f64) -> (Vec<SessionRow>, bool) {
    static LAST: LazyLock<Mutex<HashMap<PathBuf, SessionRow>>> = LazyLock::new(Mutex::default);
    let root = home.join(".hangar").join("orq");
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    let mut dirs: Vec<PathBuf> = match std::fs::read_dir(&root) {
        Ok(entries) => entries.flatten().map(|e| e.path()).collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => { last.retain(|d, _| !d.starts_with(&root)); return (Vec::new(), false); }
        Err(_) => {
            let mut kept: Vec<(&PathBuf, &SessionRow)> = last.iter().filter(|(d, _)| d.starts_with(&root)).collect();
            kept.sort_by_key(|(d, _)| *d);
            return (kept.into_iter().map(|(_, r)| r.clone()).collect(), true);
        }
    };
    dirs.sort();
    let mut failed = false;
    let rows = dirs.iter().filter_map(|dir| match orq_row(dir, now) {
        Ok(Some(row)) => { last.insert(dir.clone(), row.clone()); Some(row) }
        Ok(None) => { last.remove(dir); None }
        Err(()) => { failed = true; last.get(dir).cloned() }
    }).collect();
    last.retain(|d, _| !d.starts_with(&root) || dirs.contains(d));
    (rows, failed)
}

/// Problema gravado no sidecar do Claude sem terminal (`problema: [código, detalhe]`), que o ator
/// do Rust escreve e apaga; é o mesmo que o `problema_de` do Python devolvia lido do disco.
pub fn headless_problem(home: &Path, name: &str) -> Option<String> {
    let raw = std::fs::read(crate::state::parked::sidecar_path(name, home)).ok()?;
    let meta: Value = serde_json::from_slice(&raw).ok()?;
    text(&meta["problema"][0]).map(str::to_owned)
}

/// `status_line_do_rollout`: a linha do card montada do fim do rollout do Codex.
pub fn codex_rollout_status(path: &Path, now: f64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.seek(SeekFrom::End(0)).ok()?;
    file.seek(SeekFrom::Start(len.saturating_sub(STATUS_TAIL))).ok()?;
    let mut raw = Vec::new();
    file.read_to_end(&mut raw).ok()?;
    let (mut tc, mut ctx, mut limits): (Option<Value>, Option<Value>, Option<Value>) = (None, None, None);
    for line in raw.split(|b| *b == b'\n').rev() {
        if line.iter().all(u8::is_ascii_whitespace) { continue; }
        let Ok(obj) = serde_json::from_slice::<Value>(line) else { continue };
        let payload = &obj["payload"];
        if !payload.is_object() { continue; }
        let token_count = payload["type"] == "token_count";
        let rates = &payload["rate_limits"];
        if limits.is_none() && token_count && rates.is_object() && (rates["limit_id"].is_null() || rates["limit_id"] == "codex") {
            limits = Some(rates.clone());
        }
        if tc.is_none() && token_count {
            tc = Some(payload.clone());
        } else if ctx.is_none() && obj["type"] == "turn_context" {
            ctx = Some(payload.clone());
        }
        if tc.is_some() && ctx.is_some() && limits.is_some() { break; }
    }
    let info = tc.as_ref().map(|t| &t["info"]).filter(|i| i.is_object());
    let total = info.map(|i| &i["last_token_usage"]).filter(|t| t.as_object().is_some_and(|o| !o.is_empty()));
    let usage = total.map(|t| json!({"last": {"inputTokens": t["input_tokens"], "outputTokens": t["output_tokens"]},
        "modelContextWindow": info.map_or(Value::Null, |i| i["model_context_window"].clone())}));
    let limits = limits.unwrap_or(Value::Null);
    let mut windows = serde_json::Map::new();
    for key in ["primary", "secondary"] {
        let w = &limits[key];
        if w.as_object().is_some_and(|o| !o.is_empty()) {
            windows.insert(key.into(), json!({"windowDurationMins": w["window_minutes"], "usedPercent": w["used_percent"], "resetsAt": w["resets_at"]}));
        }
    }
    let ctx = ctx.unwrap_or(Value::Null);
    let effort = &ctx["collaboration_mode"]["settings"]["reasoning_effort"];
    let payload = json!({"model": ctx["model"], "effort": effort, "token_usage": usage,
        "rate_limits": if windows.is_empty() { Value::Null } else { Value::Object(windows) }});
    let out = crate::runtime::local_policy::run_at("format_status", &payload, &json!({"provider": "codex"}), None, now)?.ok()?;
    out["status_line"].as_str().map(str::to_owned)
}

/// Linha de status do Codex por sessão, relida a cada `STATUS_TTL_S`; leitura vazia mantém a anterior.
static CODEX_STATUS: LazyLock<Mutex<HashMap<String, (Instant, Option<String>)>>> = LazyLock::new(Mutex::default);

pub fn codex_status_cached(name: &str, jsonl: &Path, now: f64) -> Option<String> {
    let mut cache = CODEX_STATUS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, line)) = cache.get(name).filter(|(at, _)| at.elapsed().as_secs_f64() <= STATUS_TTL_S) {
        return line.clone();
    }
    let previous = cache.get(name).and_then(|(_, line)| line.clone());
    let line = codex_rollout_status(jsonl, now).or(previous);
    cache.insert(name.to_owned(), (Instant::now(), line.clone()));
    line
}

/// Esquece as linhas de quem saiu da lista (o `_forget` do registro).
pub fn forget_codex_status(alive: &dyn Fn(&str) -> bool) {
    CODEX_STATUS.lock().unwrap_or_else(|e| e.into_inner()).retain(|name, _| alive(name));
}

/// Codex com terminal ainda sem thread: os passos do lançador, o menu da TUI ou a falha da abertura
/// lidos do pane (`registry.list_with_state`, `pendente_sem_thread`).
pub fn codex_launch(row: &mut SessionRow, pane: &str) {
    row.startup_steps = pane.lines().filter_map(|l| l.strip_prefix("hangar-codex-tui: ")).map(str::to_owned).collect();
    if let Some((question, options)) = crate::terminal_state::codex_menu_of(pane) {
        row.state = "awaiting_input".into();
        row.question = question;
        row.options = Some(options);
    } else if pane.lines().any(|l| l.trim() == "Pressione Enter para fechar esta sessão.") {
        row.state = "awaiting_input".into();
        row.problema = Some("codex_abertura_falhou".into());
        row.label = row.startup_steps.last().cloned();
    } else if !row.startup_steps.is_empty() {
        row.state = "working".into();
        row.label = row.startup_steps.last().cloned();
    }
}

/// Os campos que a linha Codex recebia dos fatos: atividade e linha de status do rollout, travada
/// pelo prazo, e nada de limite (o Codex não entra no radar). O estado e a pergunta são do ator do
/// Rust, que a classificação aplica depois.
pub fn codex_row(row: &mut SessionRow, stall: f64, now: f64) {
    let jsonl = row.jsonl.clone();
    row.last_activity = jsonl.as_deref().and_then(|j| modified(Path::new(j)));
    // Sem thread nada do cache vale para esta abertura (`_forget_local`).
    row.status_line = match jsonl.as_deref() {
        Some(j) => codex_status_cached(&row.name, Path::new(j), now),
        None => { CODEX_STATUS.lock().unwrap_or_else(|e| e.into_inner()).remove(&row.name); None }
    };
    row.limited = false;
    row.limit_reset = None;
    row.stalled = row.state == "working" && row.last_activity.is_some_and(|l| now - l > stall);
}

#[cfg(test)]
mod tests;
