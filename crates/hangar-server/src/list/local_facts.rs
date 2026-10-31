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

/// Lista e avisos usam o mesmo registro do login; só a visibilidade conserva a última leitura válida.
#[derive(Debug, Clone)]
pub struct Guests {
    store: std::sync::Arc<crate::login_guests::store::Store>,
    claims: std::collections::HashSet<String>,
}
impl Guests {
    pub fn read(claude: &Path) -> Self {
        let store = crate::login_guests::store::Store::shared(claude.join("guests.json"));
        let snapshot = store.snapshot(true);
        let claims = snapshot.as_ref().and_then(|doc| doc["sessions"].as_object())
            .into_iter().flat_map(|sessions| sessions.keys().cloned()).collect();
        Self { store, claims }
    }
    pub fn has_claims(&self) -> bool { !self.claims.is_empty() }
    pub fn claimed(&self, name: &str) -> bool { self.claims.contains(name) }
    pub fn access(&self, names: &[(String, Option<String>)]) -> (HashMap<String, String>, Vec<String>) {
        let (mut owners, mut hidden) = (HashMap::new(), Vec::new());
        for (name, life) in names {
            if let Some(owner) = self.store.owner(name, life.as_deref(), true) {
                owners.insert(name.clone(), owner.name);
                if !owner.owner_sees { hidden.push(name.clone()); }
            }
        }
        hidden.sort();
        hidden.dedup();
        (owners, hidden)
    }
}

fn text(v: &Value) -> Option<&str> { v.as_str().filter(|s| !s.is_empty()) }

fn modified(path: &Path) -> Option<f64> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()?
        .duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs_f64())
}

/// Uma leitura que falha conserva a última linha boa da execução.
type OrqRead<T> = Result<Option<T>, ()>;

fn orq_activity(timeline: &Path, now: f64) -> (&'static str, Option<f64>) {
    crate::orchestration::runs::activity(timeline, now)
}

/// A linha de uma execução: `Ok(None)` quando ela não está viva, `Err` quando um arquivo não leu.
fn orq_row(dir: &Path, now: f64) -> OrqRead<SessionRow> {
    let Some((cfg, gid, ended)) = crate::orchestration::runs::list_run(dir).map_err(|_|())? else { return Ok(None) };
    if ended { return Ok(None); }
    let Some(hb) = crate::orchestration::runs::heartbeat(dir, now).map_err(|_|())? else { return Ok(None) };
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
        // A pasta também guarda o plano `.md` de cada execução; só subpasta é execução.
        // Tipo ilegível segue para a leitura, que reporta a falha e guarda a última linha boa.
        Ok(entries) => entries.flatten()
            .filter(|e| e.file_type().map_or(true, |t| t.is_dir() || t.is_symlink() && e.path().is_dir()))
            .map(|e| e.path()).collect(),
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
