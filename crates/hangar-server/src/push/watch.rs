//! Vigia sempre ligado (com ou sem lista aberta) que decide os avisos do celular: as transições dos
//! marcadores e do registro nativo ("aguardando", série de turnos e "terminou") e a varredura da lista
//! a cada `stall_poll_seconds` ("caiu", "travada", "limite" e o auto-resume). A memória dos avisos
//! fica em disco por sessão: reiniciar não perde um "terminou" nem repete um "travada".
use super::send::{Notice, Sender};
use crate::list::facts_files::HookStates;
use crate::routes::AppState;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::broadcast;

/// Fim de turno não é fim de trabalho: o "terminou" espera a sessão sossegar.
const FINISH_SETTLE: Duration = Duration::from_secs(20);
/// Parada mais longa que isto entre um turno e o próximo é trabalho novo.
const SERIES_GAP: f64 = 30.0 * 60.0;
/// O menu ou a pergunta podem atrasar um quadro em relação ao marcador.
const AWAITING_RETRY: Duration = Duration::from_millis(1500);
const TICK: Duration = Duration::from_secs(1);

/// Transição de estado de uma sessão (marcador ou registro nativo), para quem reage a fim de turno
/// (loop e encadeamento). `sid` é a chave da conversa (`locate::session_key` do transcript).
#[derive(Clone, Debug, PartialEq)]
pub struct TurnEvent { pub sid: String, pub state: String, pub ts: f64 }

fn turns() -> &'static broadcast::Sender<TurnEvent> {
    static TX: OnceLock<broadcast::Sender<TurnEvent>> = OnceLock::new();
    TX.get_or_init(|| broadcast::channel(256).0)
}

/// Assina as transições: só chegam as que acontecerem depois da assinatura, e quem atrasar mais de
/// 256 perde as mais velhas (`Lagged`).
pub fn subscribe() -> broadcast::Receiver<TurnEvent> { turns().subscribe() }

/// O que o vigia lembra entre reinícios.
#[derive(Default, Serialize, Deserialize)]
pub struct Memory {
    /// Início da série de turnos por sessão.
    #[serde(default)] pub started: BTreeMap<String, f64>,
    #[serde(default)] pub last_idle: BTreeMap<String, f64>,
    #[serde(default)] pub stalled: BTreeSet<String>,
    #[serde(default)] pub limited: BTreeSet<String>,
    #[serde(default)] pub dead: BTreeSet<String>,
    /// Vivas na varredura anterior: nome → chave da conversa.
    #[serde(default)] pub seen: BTreeMap<String, Option<String>>,
}

impl Memory {
    pub fn path(home: &Path) -> PathBuf { home.join(".hangar").join("push-watch.json") }

    pub fn load(path: &Path) -> Self {
        std::fs::read(path).ok().and_then(|raw| serde_json::from_slice(&raw).ok()).unwrap_or_default()
    }

    fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(path.parent().ok_or_else(|| std::io::Error::other("pasta ausente"))?)?;
        crate::runtime::queue::atomic_write(path, &serde_json::to_vec(self)?)
    }

    /// "working": só o primeiro turno da série grava; parada longa antes dele abre série nova.
    pub fn working(&mut self, sid: &str, ts: f64) {
        let gap = self.last_idle.get(sid).is_some_and(|parou| ts - parou > SERIES_GAP);
        if !self.started.contains_key(sid) || gap { self.started.insert(sid.to_owned(), ts); }
    }

    /// A sessão morreu: a série dela acaba.
    pub fn forget(&mut self, sid: &str) { self.started.remove(sid); self.last_idle.remove(sid); }

    /// Uma varredura da lista: o que avisar (nome, aviso) e o que liberar. `rows` = (nome, chave,
    /// travada, limite com o horário de volta).
    pub fn sweep(&mut self, rows: &[(String, Option<String>, bool, Option<Option<String>>)], notify_dead: bool) -> Vec<(String, Notice)> {
        let (mut out, mut dead_sids, mut dead_names) = (Vec::new(), Vec::new(), Vec::new());
        let now_live: BTreeMap<String, Option<String>> = rows.iter().map(|(n, k, _, _)| (n.clone(), k.clone())).collect();
        for (name, sid) in &self.seen {
            if now_live.contains_key(name) || self.dead.contains(name) { continue; }
            if notify_dead { out.push((name.clone(), Notice::Dead)); }
            if let Some(sid) = sid { dead_sids.push(sid.clone()); }
            dead_names.push(name.clone());
        }
        for sid in dead_sids { self.forget(&sid); }
        self.dead.extend(dead_names);
        self.dead.retain(|n| !now_live.contains_key(n));
        self.seen = now_live;
        let stalled: BTreeSet<String> = rows.iter().filter(|r| r.2).map(|r| r.0.clone()).collect();
        out.extend(stalled.difference(&self.stalled).map(|n| (n.clone(), Notice::Stalled)));
        self.stalled = stalled;
        let limited: BTreeMap<String, Option<String>> = rows.iter().filter_map(|r| r.3.clone().map(|reset| (r.0.clone(), reset))).collect();
        out.extend(limited.iter().filter(|(n, _)| !self.limited.contains(*n)).map(|(n, reset)| (n.clone(), Notice::Limited(reset.clone()))));
        self.limited = limited.into_keys().collect();
        out
    }
}

/// Valor efetivo de um campo do `runtime_config`: o arquivo vence o ambiente, que vence o padrão.
fn config_value(claude: &Path, env: &BTreeMap<String, String>, key: &str) -> Option<Value> {
    let file = std::fs::read(claude.join("runtime-config.json")).ok().and_then(|raw| serde_json::from_slice::<Value>(&raw).ok());
    if let Some(v) = file.as_ref().and_then(|d| d.get(key)) { return Some(v.clone()); }
    env.get(&format!("CP_{}", key.to_uppercase())).map(|v| Value::String(v.trim().to_owned()))
}

/// Booleano como o pydantic lê do ambiente.
fn truthy(v: &Value, default: bool) -> bool {
    match v {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => match s.to_ascii_lowercase().as_str() {
            "1" | "true" | "t" | "yes" | "y" | "on" => true,
            "0" | "false" | "f" | "no" | "n" | "off" => false,
            _ => default,
        },
        _ => default,
    }
}

pub struct Settings { pub notify_dead: bool, pub notify_finished: bool, pub finish_min_seconds: f64, pub automations: bool, pub auto_resume: bool, pub poll: Duration }

impl Settings {
    pub fn read(claude: &Path, env: &BTreeMap<String, String>) -> Self {
        let flag = |k: &str, d: bool| config_value(claude, env, k).map_or(d, |v| truthy(&v, d));
        let num = |k: &str, d: f64| config_value(claude, env, k).and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok())).unwrap_or(d);
        Self {
            notify_dead: flag("notify_dead", true), notify_finished: flag("notify_finished", true),
            finish_min_seconds: num("finish_min_seconds", 45.0), automations: flag("automations", true),
            auto_resume: env.get("CP_AUTO_RESUME").is_some_and(|v| truthy(&Value::String(v.clone()), false)),
            poll: Duration::from_secs_f64(env.get("CP_STALL_POLL_SECONDS").and_then(|v| v.trim().parse::<f64>().ok()).filter(|s| *s > 0.0).unwrap_or(30.0)),
        }
    }
}

/// Segundos até o horário de volta do limite (`HH[:MM][am|pm]`, fuso do sistema), ou `None` sem
/// parse confiável: auto-resume não arma sobre chute.
pub fn reset_delay(reset: &str, now: chrono::NaiveDateTime) -> Option<f64> {
    static RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?i)^(\d{1,2})(?::(\d{2}))?\s*(am|pm)?$").unwrap());
    let m = RE.captures(reset.trim())?;
    let mut hour: u32 = m[1].parse().ok()?;
    let minute: u32 = m.get(2).map_or(Some(0), |v| v.as_str().parse().ok())?;
    match m.get(3).map(|v| v.as_str().to_ascii_lowercase()).as_deref() {
        Some("pm") if hour != 12 => hour += 12,
        Some("am") if hour == 12 => hour = 0,
        _ => {}
    }
    let target = now.date().and_hms_opt(hour, minute, 0)?;
    let target = if target <= now { target + chrono::Duration::days(1) } else { target };
    Some((target - now).num_milliseconds() as f64 / 1000.0)
}

/// Uma linha do começo da última resposta, sem a marcação do markdown (`_resumo_resposta`).
pub fn summary(text: Option<&str>) -> Option<String> {
    static LINK: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
    let space = |c: char| c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c);
    let line = text?.split(space).filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ");
    let line = LINK.replace_all(&line, "$1");
    let chars: Vec<char> = line.chars().collect();
    let word = |c: Option<&char>| c.is_some_and(|c| c.is_alphanumeric() || *c == '_');
    let mut out = String::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if "`*#>|".contains(c) { i += 1; continue; }
        if c == '_' {
            let end = chars[i..].iter().position(|x| *x != '_').map_or(chars.len(), |p| i + p);
            // `(?<!\w)_+|_+(?!\w)`: some o trecho de `_` que não está entre dois caracteres de palavra.
            if !word(i.checked_sub(1).and_then(|p| chars.get(p))) || !word(chars.get(end)) {
                i = end;
                continue;
            }
            out.extend(&chars[i..end]);
            i = end;
            continue;
        }
        out.push(c);
        i += 1;
    }
    let line: String = out.trim_matches(space).to_owned();
    let n = line.chars().count();
    if n > 160 {
        let cut: String = line.chars().take(159).collect();
        return Some(format!("{}…", cut.trim_end_matches(space)));
    }
    (!line.is_empty()).then_some(line)
}

/// O que segue em segundo plano na sessão Claude e a última resposta (`background_open.scan`).
pub struct Scan { pub open: usize, pub last_reply: Option<String> }

const TAIL_BYTES: u64 = 4 * 1024 * 1024;
/// Lançamento sem notificação de fim depois disto não segura mais o aviso.
const STALE_AFTER: f64 = 30.0 * 60.0;

pub fn scan(jsonl: &Path, now: f64) -> std::io::Result<Scan> {
    use std::io::{Read, Seek, SeekFrom};
    use hangar_api::chat::ChatKind;
    static SHELL: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"Command running in background with ID:\s*([A-Za-z0-9_-]+)").unwrap());
    let mut f = std::fs::File::open(jsonl)?;
    let size = f.seek(SeekFrom::End(0))?;
    f.seek(SeekFrom::Start(size.saturating_sub(TAIL_BYTES)))?;
    let mut data = Vec::new();
    f.read_to_end(&mut data)?;
    let text = String::from_utf8_lossy(&data);
    let mut lines: Vec<&str> = text.lines().collect();
    if size > TAIL_BYTES && !lines.is_empty() { lines.remove(0); }
    let (mut launched, mut closed, mut shells): (HashMap<String, f64>, BTreeSet<String>, BTreeSet<String>) = Default::default();
    let mut reply = None;
    let mut parser = crate::transcript::LineParser::new(crate::transcript::Provider::Claude);
    for line in lines {
        for e in parser.feed(line.as_bytes(), 0) {
            let tid = e.tool_use_id.clone().unwrap_or_default();
            match e.kind {
                ChatKind::AssistantMsg if e.text.as_deref().is_some_and(|t| !t.trim().is_empty()) => reply = e.text.clone(),
                ChatKind::ToolUse if e.tool_name.as_deref() == Some("Bash")
                    && e.tool_input.as_ref().and_then(|i| i.get("run_in_background")).is_some_and(|v| truthy(v, false)) => { shells.insert(tid); }
                ChatKind::ToolResult => {
                    if let Some(task) = tid.strip_prefix("task:") {
                        closed.insert(task.to_owned());
                    } else if let Some(agent) = e.bg_agent_id.as_deref().filter(|a| !a.starts_with("teammate:")) {
                        launched.insert(agent.to_owned(), e.ts.unwrap_or(now));
                    } else if shells.contains(&tid) && let Some(m) = SHELL.captures(e.result.as_deref().unwrap_or("")) {
                        launched.insert(m[1].to_owned(), e.ts.unwrap_or(now));
                    }
                }
                _ => {}
            }
        }
    }
    let open = launched.iter().filter(|(id, at)| !closed.contains(*id) && now - **at < STALE_AFTER).count();
    Ok(Scan { open, last_reply: reply })
}

fn now_epoch() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

struct Watch {
    st: Arc<AppState>,
    env: BTreeMap<String, String>,
    sender: Arc<Sender>,
    hooks: Mutex<HookStates>,
    memory: Mutex<Memory>,
    memory_path: PathBuf,
}

impl Watch {
    fn settings(&self) -> Settings { Settings::read(&self.st.accounts.env.claude_base, &self.env) }

    fn persist(&self) {
        let saved = self.memory.lock().unwrap_or_else(|e| e.into_inner()).save(&self.memory_path);
        if saved.is_err() { self.st.diag.report("rust.push_memory_unwritable", "push", "push_memory_unwritable", "memória dos avisos não gravou"); }
    }

    fn state_of(&self, sid: &str) -> Option<crate::list::facts_files::Marker> {
        let procs = crate::list::procs::SystemProcs::whole_machine();
        self.hooks.lock().unwrap_or_else(|e| e.into_inner()).get_state(Some(sid), |pid| crate::list::bridge::pid_alive(&procs, pid))
    }

    async fn row_of(&self, sid: &str, fresh: bool) -> Option<hangar_api::session::SessionRow> {
        let produced = if fresh { self.st.list.refresh().await } else { self.st.list.snapshot().await }.ok()?;
        produced.rows.iter().find(|r| r.jsonl.as_deref().is_some_and(|j| crate::locate::session_key(j) == sid)).cloned()
    }

    /// "Aguardando" só com pergunta de verdade: pergunta aberta no sidecar ou o estado que o Rust
    /// publicou (pane com menu, ou a vista do runtime sem terminal). Relê uma vez depois de 1,5 s.
    async fn awaiting(self: Arc<Self>, sid: String) {
        for attempt in 0..2 {
            if attempt == 1 { tokio::time::sleep(AWAITING_RETRY).await; }
            let Some(row) = self.row_of(&sid, attempt == 1).await else { return };
            let ask = if row.headless { None } else {
                let dirs = self.st.list.state_dirs_blocking().map(|(d, _)| d.to_vec()).unwrap_or_default();
                crate::list::facts_files::open_question(Some(&sid), &dirs).map(|q| q.question)
            };
            if ask.is_some() || row.state == "awaiting_input" {
                self.sender.awaiting(&row.name, ask.or_else(|| row.question.clone().filter(|q| !q.is_empty())));
                return;
            }
        }
    }

    /// "Terminou" depois de a sessão sossegar, sem trabalho de fundo aberto e com a série longa.
    async fn finished(self: Arc<Self>, sid: String, started: f64, idle_ts: f64) {
        tokio::time::sleep(FINISH_SETTLE).await;
        if !self.state_of(&sid).is_some_and(|m| m.state == "idle" && m.ts == idle_ts) { return; }
        let same = |me: &Self| me.memory.lock().unwrap_or_else(|e| e.into_inner()).started.get(&sid) == Some(&started);
        if !same(&self) { return; }
        let row = self.row_of(&sid, false).await;
        let mut reply = None;
        if let Some(jsonl) = row.as_ref().filter(|r| r.provider == "claude").and_then(|r| r.jsonl.clone()) {
            match tokio::task::spawn_blocking(move || scan(Path::new(&jsonl), now_epoch())).await {
                Ok(Ok(s)) if s.open > 0 => return,
                Ok(Ok(s)) => reply = s.last_reply,
                _ => tracing::warn!(code = "push_finished_scan_failed", "terminou: transcript ilegível; aviso sai mesmo assim"),
            }
        }
        {
            let mut m = self.memory.lock().unwrap_or_else(|e| e.into_inner());
            if m.started.get(&sid) != Some(&started) { return; }
            m.started.remove(&sid);
        }
        self.persist();
        let s = self.settings();
        let Some(row) = row else { return };
        if !s.notify_finished || idle_ts - started < s.finish_min_seconds { return; }
        self.sender.notify(&row.name, Notice::Finished(summary(reply.as_deref())));
    }

    fn transition(self: &Arc<Self>, sid: &str, marker: &crate::list::facts_files::Marker) {
        let _ = turns().send(TurnEvent { sid: sid.to_owned(), state: marker.state.clone(), ts: marker.ts });
        match marker.state.as_str() {
            "awaiting_input" => { tokio::spawn(self.clone().awaiting(sid.to_owned())); }
            "working" => {
                self.memory.lock().unwrap_or_else(|e| e.into_inner()).working(sid, marker.ts);
                self.persist();
            }
            "idle" => {
                let started = {
                    let mut m = self.memory.lock().unwrap_or_else(|e| e.into_inner());
                    m.last_idle.insert(sid.to_owned(), marker.ts);
                    m.started.get(sid).copied()
                };
                self.persist();
                if let Some(started) = started { tokio::spawn(self.clone().finished(sid.to_owned(), started, marker.ts)); }
            }
            _ => {}
        }
    }

    /// Marcadores e registro nativo a cada segundo; só o arquivo que mudou é relido.
    async fn transitions(self: Arc<Self>) {
        let mut last: HashMap<String, String> = HashMap::new();
        let mut seeded = false;
        loop {
            let me = self.clone();
            let changed = tokio::task::spawn_blocking(move || {
                let dirs = me.st.list.state_dirs_blocking().map(|(d, _)| d.to_vec()).unwrap_or_default();
                let procs = crate::list::procs::SystemProcs::whole_machine();
                let mut hooks = me.hooks.lock().unwrap_or_else(|e| e.into_inner());
                hooks.refresh(&dirs).into_iter()
                    .map(|sid| { let m = hooks.get_state(Some(&sid), |pid| crate::list::bridge::pid_alive(&procs, pid)); (sid, m) })
                    .collect::<Vec<_>>()
            }).await.unwrap_or_default();
            for (sid, marker) in changed {
                let Some(marker) = marker else { last.remove(&sid); continue };
                let before = last.insert(sid.clone(), marker.state.clone());
                // A primeira leitura semeia; a sessão já aguardando não avisa de novo por um ts novo.
                if seeded && before.as_deref() != Some(marker.state.as_str()) { self.transition(&sid, &marker); }
            }
            seeded = true;
            tokio::time::sleep(TICK).await;
        }
    }

    /// Varredura da lista: "caiu" pelo sumiço, "travada" e "limite" uma vez por janela, e o auto-resume.
    async fn sweep(self: Arc<Self>) {
        let mut armed: BTreeSet<String> = BTreeSet::new();
        loop {
            let s = self.settings();
            match self.st.list.snapshot().await {
                Ok(produced) => {
                    let rows: Vec<_> = produced.rows.iter().filter(|r| r.provider != "orq").map(|r| (r.name.clone(),
                        r.jsonl.as_deref().map(crate::locate::session_key), r.stalled, r.limited.then(|| r.limit_reset.clone()))).collect();
                    let notices = self.memory.lock().unwrap_or_else(|e| e.into_inner()).sweep(&rows, s.notify_dead);
                    self.persist();
                    for (name, notice) in notices {
                        if let Notice::Limited(Some(reset)) = &notice { self.auto_resume(&s, &name, reset, &mut armed); }
                        self.sender.notify(&name, notice);
                    }
                    let limited = self.memory.lock().unwrap_or_else(|e| e.into_inner()).limited.clone();
                    armed.retain(|n| limited.contains(n));
                }
                // Retrato com erro perde o ciclo; nunca vira "todos morreram".
                Err(e) => tracing::warn!(code = e.code, "vigia de avisos: lista indisponível; ciclo perdido"),
            }
            tokio::time::sleep(s.poll).await;
        }
    }

    /// Opt-in (`CP_AUTO_RESUME` e a chave mestra): drena a fila da sessão no horário de volta.
    fn auto_resume(&self, s: &Settings, name: &str, reset: &str, armed: &mut BTreeSet<String>) {
        if !s.automations || !s.auto_resume || armed.contains(name) { return; }
        let Some(delay) = reset_delay(reset, chrono::Local::now().naive_local()) else {
            tracing::info!(session = name, reset, "auto-resume: horário de volta não parseou; não arma");
            return;
        };
        let Some(registry) = self.st.side.runtime.get().cloned() else { return };
        armed.insert(name.to_owned());
        let name = name.to_owned();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(delay)).await;
            if let Err(e) = registry.drain(&name).await { tracing::warn!(session = name, code = e.code, "auto-resume: drain falhou"); }
        });
    }
}

/// Liga o vigia. As tarefas vivem enquanto os `JoinHandle` não forem abortados.
pub fn start(st: Arc<AppState>) -> Vec<tokio::task::JoinHandle<()>> {
    if st.state.runtime.get().is_none() { return Vec::new(); }
    let env = super::server_env();
    let memory_path = Memory::path(&st.accounts.env.home);
    let w = Arc::new(Watch { sender: Sender::new(st.clone(), env.clone()), env, memory: Mutex::new(Memory::load(&memory_path)),
        memory_path, hooks: Mutex::new(HookStates::default()), st });
    vec![tokio::spawn(w.clone().transitions()), tokio::spawn(w.sweep())]
}

#[cfg(test)]
mod tests;
