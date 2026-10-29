//! Restaurar as sessões com terminal depois do reboot: o Rust guarda, enquanto roda, quais sessões do
//! Hangar estão abertas e em que conversa (`~/.hangar/session-restore.json`), e na subida, quando o servidor
//! do multiplexador não é mais o da fotografia (reboot, queda ou `kill-server` do tmux), reabre cada uma pelo
//! spec gravado, retomando a conversa. O comando vai no `new-session`, nunca digitado num
//! shell. Não depende do tmux-resurrect, e vale no Windows (psmux).
//!
//! Uma sessão sai da fotografia só depois de ausente por `ABSENT` com o servidor de pé: o desligamento
//! derruba as sessões e o servidor em segundos, e fechar uma sessão de propósito a tira da próxima subida.
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{AgentProvider, OpenCaller, Surface};

pub const FILE: &str = "session-restore.json";
/// Ausente por mais que isto com o servidor de pé: fechada de propósito.
pub const ABSENT: f64 = 120.0;
const EVERY: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Quando o servidor do multiplexador subiu (segundos desde a época), na última vez que o Rust olhou.
    #[serde(default)]
    pub mux_started: Option<u64>,
    /// Pela chave do spec.
    pub sessions: BTreeMap<String, Entry>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub name: String,
    pub provider: AgentProvider,
    pub conversation: Option<String>,
    /// O modo de permissão que o Rust viu por último (o Claude troca de modo por teclas, sem regravar o spec).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub missing_since: Option<f64>,
}

/// O spec com que a sessão volta: o gravado, com o modo da hora em que caiu. No Codex o modo
/// é o que a TUI gravou no sidecar (`record::live`); no Claude, o que a fotografia viu.
pub fn spec_for_restore(home: &Path, key: &str, entry: &Entry) -> Result<super::SessionSpec, &'static str> {
    let read = if entry.provider == AgentProvider::Codex { super::record::live(home, &entry.name) } else { super::record::read(home, key) };
    let mut spec = read.map_err(|_| "restore_spec_unreadable")?.filter(|s| s.key == key).ok_or("restore_no_spec")?;
    if spec.provider == AgentProvider::Claude && let Some(mode) = entry.permission_mode.as_deref().and_then(super::PermissionMode::claude) {
        spec.permission_mode = Some(mode);
    }
    Ok(spec)
}

/// A conversa que o transcript da lista prova: o id no nome do arquivo (Claude), depois do `_` (Pi, omp),
/// ou a pasta da sessão (Kimi, `…/<session_id>/agents/main/wire.jsonl`).
pub fn conversation_of(provider: AgentProvider, jsonl: &str) -> Option<String> {
    let path = Path::new(jsonl);
    let stem = path.file_stem()?.to_str()?;
    let id = match provider {
        AgentProvider::Claude => stem.to_owned(),
        AgentProvider::Pi | AgentProvider::Omp => stem.rsplit_once('_')?.1.to_owned(),
        AgentProvider::Kimi => path.parent()?.parent()?.parent()?.file_name()?.to_str()?.to_owned(),
        AgentProvider::Codex => return None,
    };
    (!id.is_empty()).then_some(id)
}

/// A fotografia seguinte: as vivas com a conversa de agora (a de antes, se a lista não provou); as ausentes
/// ficam até `ABSENT`.
pub fn observe(prev: &Snapshot, alive: &BTreeMap<String, Entry>, now: f64, mux_started: Option<u64>) -> Snapshot {
    let mut sessions = BTreeMap::new();
    for (key, entry) in alive {
        let conversation = entry.conversation.clone().or_else(|| prev.sessions.get(key).and_then(|e| e.conversation.clone()));
        sessions.insert(key.clone(), Entry { conversation, missing_since: None, ..entry.clone() });
    }
    for (key, entry) in &prev.sessions {
        if alive.contains_key(key) { continue; }
        let since = entry.missing_since.unwrap_or(now);
        if now - since < ABSENT { sessions.insert(key.clone(), Entry { missing_since: Some(since), ..entry.clone() }); }
    }
    Snapshot { mux_started, sessions }
}

/// O servidor do multiplexador agora: nenhum (as sessões morreram com ele), o horário em que subiu, ou
/// desconhecido (não respondeu).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuxStart { NoServer, Started(u64), Unknown }

/// O que reabrir na subida: só com outro servidor que o da fotografia, e só o que não tem sessão com o nome.
/// `None`: o multiplexador não respondeu e não dá para decidir agora.
pub fn to_restore(snap: &Snapshot, current: MuxStart, live: &HashSet<String>) -> Option<Vec<(String, Entry)>> {
    match current {
        MuxStart::Unknown => return None,
        MuxStart::Started(at) if snap.mux_started == Some(at) => return Some(Vec::new()),
        _ => {}
    }
    Some(snap.sessions.iter().filter(|(_, e)| !live.contains(&e.name)).map(|(k, e)| (k.clone(), e.clone())).collect())
}

/// O psmux sobe um servidor por sessão (o `#{start_time}` é o da mais nova e `#{server_sessions}` vem
/// vazio), então fim normal e queda não se distinguem com o Rust de pé. A decisão é uma só, na subida:
/// `Some(true)` restaura, `Some(false)` só fotografa daí em diante, `None` = o psmux não respondeu ainda.
pub fn windows_startup(current: MuxStart, snap: &Snapshot) -> Option<bool> {
    match current {
        MuxStart::Unknown => None,
        MuxStart::NoServer => Some(!snap.sessions.is_empty()),
        MuxStart::Started(_) => Some(false),
    }
}

/// O servidor da fotografia acabou porque a única sessão dele terminou (o agente saiu, o tmux fecha a
/// sessão e sai junto): fechamento, não queda, e a fotografia esvazia. `last_count`: as sessões dele na
/// última volta desta execução. Com duas ou mais sumindo juntas, no desligamento do sistema ou sem a
/// contagem, é queda (reboot, `kill-server`). ponytail: a última sessão derrubada sozinha por
/// `kill-server` também conta como fechada; distinguir exige a ordem de saída de agente e servidor.
pub fn ended_by_itself(snap: &mut Snapshot, last_count: Option<u64>, stopping: bool) -> bool {
    if last_count != Some(1) || stopping { return false; }
    snap.sessions.clear();
    true
}

/// `systemctl is-system-running` (do sistema ou do usuário) em `stopping`: o desligamento derruba as
/// sessões uma a uma, e isso é queda.
async fn stopping() -> bool {
    if !cfg!(target_os = "linux") { return false; }
    for scope in [&[][..], &["--user"][..]] {
        let mut command = tokio::process::Command::new("systemctl");
        command.args(scope).arg("is-system-running").stdin(std::process::Stdio::null()).kill_on_drop(true);
        if let Ok(Ok(out)) = tokio::time::timeout(Duration::from_secs(2), command.output()).await
            && String::from_utf8_lossy(&out.stdout).trim() == "stopping" { return true; }
    }
    false
}

/// Uma variável de formato do servidor (`display-message -p`); `Err` = sem servidor (`Ok(None)`) ou sem resposta.
async fn mux_format(program: &std::ffi::OsStr, format: &str) -> Result<Option<String>, ()> {
    let mut command = crate::terminal_input::child_command(program);
    command.args(["display-message", "-p", format]).stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(5), command.output()).await.map_err(|_| ())?.map_err(|_| ())?;
    if out.status.success() { return Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_owned())); }
    if crate::list::mux::is_absence(&String::from_utf8_lossy(&out.stderr)) { Ok(None) } else { Err(()) }
}

/// Quando o servidor do multiplexador subiu: o `#{start_time}` do tmux; sem ele (o psmux pode não ter), o
/// nascimento do processo do servidor (`#{pid}`); sem os dois, a subida da máquina.
pub async fn mux_start(program: &std::ffi::OsStr) -> MuxStart { mux_now(program).await.0 }

/// `mux_start` e o `#{server_sessions}` do mesmo servidor, numa consulta só.
async fn mux_now(program: &std::ffi::OsStr) -> (MuxStart, Option<u64>) {
    let (start, sessions) = match mux_format(program, "#{start_time}|#{server_sessions}").await {
        Err(()) => return (MuxStart::Unknown, None),
        Ok(None) => return (MuxStart::NoServer, None),
        Ok(Some(text)) => {
            let (start, sessions) = text.split_once('|').unwrap_or((&text, ""));
            (start.trim().parse::<u64>().ok(), sessions.trim().parse::<u64>().ok())
        }
    };
    if let Some(at) = start.filter(|at| *at > 0) { return (MuxStart::Started(at), sessions); }
    let pid = mux_format(program, "#{pid}").await.ok().flatten().and_then(|p| p.parse::<u32>().ok());
    let born = pid.and_then(|pid| {
        let mut system = sysinfo::System::new();
        let pid = sysinfo::Pid::from_u32(pid);
        system.refresh_processes(sysinfo::ProcessesToUpdate::Some(&[pid]), true);
        system.process(pid).map(sysinfo::Process::start_time)
    });
    (MuxStart::Started(born.unwrap_or_else(sysinfo::System::boot_time)), sessions)
}

fn path(home: &Path) -> PathBuf { home.join(".hangar").join(FILE) }

/// Ausente = vazia; ilegível também, com aviso: a próxima fotografia a refaz.
pub fn read(home: &Path) -> Snapshot {
    match std::fs::read(path(home)) {
        Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|error| {
            tracing::warn!(code = "session_restore_unreadable", %error, "fotografia das sessões ilegível; o restore desta subida não tem o que reabrir");
            Snapshot::default()
        }),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(code = "session_restore_unreadable", kind = ?error.kind(), "fotografia das sessões ilegível");
            }
            Snapshot::default()
        }
    }
}

fn write(home: &Path, snap: &Snapshot) -> std::io::Result<()> {
    std::fs::create_dir_all(home.join(".hangar"))?;
    crate::runtime::queue::atomic_write(&path(home), &serde_json::to_vec_pretty(snap)?)
}

/// Os nomes fechados de propósito desde a última fotografia: o laço os tira da que ele tem na memória antes
/// de gravar, sob esta mesma trava, para não regravar a entrada que `forget` acabou de tirar.
static CLOSED: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// A sessão fechada de propósito (o `DELETE`) sai da fotografia na hora: não volta no próximo reboot.
pub fn forget(home: &Path, name: &str) {
    let mut closed = CLOSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    closed.push(name.to_owned());
    let mut snap = read(home);
    let before = snap.sessions.len();
    snap.sessions.retain(|_, entry| entry.name != name);
    if snap.sessions.len() != before && let Err(error) = write(home, &snap) {
        tracing::warn!(code = "session_restore_unwritten", kind = ?error.kind(), "a sessão fechada ficou na fotografia até o próximo tique");
    }
}

/// As que ficaram ausentes nesta fotografia sem terem sido fechadas pelo dono: nome e conversa.
fn vanished(prev: &Snapshot, next: &Snapshot, closed: &[String]) -> Vec<(String, Option<String>)> {
    next.sessions.iter()
        .filter(|(key, entry)| entry.missing_since.is_some() && prev.sessions.get(*key).is_some_and(|was| was.missing_since.is_none())
            && !closed.contains(&entry.name))
        .map(|(_, entry)| (entry.name.clone(), entry.conversation.clone())).collect()
}

/// Tira da fotografia os nomes fechados desde a última; chamar com `CLOSED` travada.
fn drop_closed(snap: &mut Snapshot, closed: &mut Vec<String>) {
    snap.sessions.retain(|_, entry| !closed.contains(&entry.name));
    closed.clear();
}

/// Os nomes que o restore ainda pode reabrir: a varredura de grupos os conta como vivos, senão tiraria os
/// membros do grupo antes de a sessão voltar.
static HELD: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Segura os nomes da fotografia enquanto o servidor dela não é o de agora; solta quando é.
fn hold(snap: &Snapshot, settled: bool) {
    let mut held = HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    *held = if settled { Vec::new() } else { snap.sessions.values().map(|e| e.name.clone()).collect() };
}

/// Os nomes à espera do restore.
pub fn held() -> Vec<String> { HELD.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() }

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64()) }

/// A chave do spec gravada no ambiente da sessão (`CP_SESSION_KEY`).
async fn session_key(program: &std::ffi::OsStr, name: &str) -> Option<String> {
    let target = if cfg!(windows) { name.to_owned() } else { format!("={name}") };
    let mut command = crate::terminal_input::child_command(program);
    command.args(["show-environment", "-t", &target, "CP_SESSION_KEY"]).stdin(std::process::Stdio::null()).kill_on_drop(true);
    let out = tokio::time::timeout(Duration::from_secs(5), command.output()).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let key = text.lines().find_map(|l| l.trim().strip_prefix("CP_SESSION_KEY="))?;
    (key.len() == 32 && key.bytes().all(|b| b.is_ascii_hexdigit())).then(|| key.to_owned())
}

/// Chaves cujo spec ilegível já foi ao diário.
static UNREADABLE: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

/// As sessões do Hangar com terminal abertas agora: a chave, o nome, o agente e a conversa que a lista prova.
/// `known` guarda nome → (chave, agente) das sessões com terminal já provadas: só o nome novo paga a
/// consulta ao multiplexador e a leitura do spec.
// ponytail: o mesmo nome recriado com outra chave dentro de um tique segue com a chave antiga até sumir da lista.
async fn alive(st: &crate::routes::AppState, home: &Path, program: &std::ffi::OsStr,
               known: &mut HashMap<String, (String, AgentProvider)>) -> Option<BTreeMap<String, Entry>> {
    let rows = st.list.discover(None).await.ok()?;
    known.retain(|name, _| rows.iter().any(|r| !r.headless && r.name == *name));
    let mut out = BTreeMap::new();
    for row in rows.iter().filter(|r| !r.headless) {
        let Some(provider) = AgentProvider::parse(&row.provider) else { continue };
        let key = match known.get(&row.name).filter(|(_, p)| *p == provider) {
            Some((key, _)) => key.clone(),
            None => match proven(home, program, &row.name).await {
                Some(key) => { known.insert(row.name.clone(), (key.clone(), provider)); key }
                None => continue,
            },
        };
        let conversation = row.jsonl.as_deref().filter(|_| row.tracked).and_then(|j| conversation_of(provider, j));
        let permission_mode = match (provider, &conversation, st.state.runtime.get()) {
            (AgentProvider::Claude, Some(sid), Some(runtime)) => runtime.observed_claude_mode(sid),
            _ => None,
        };
        out.insert(key, Entry { name: row.name.clone(), provider, conversation, permission_mode, missing_since: None });
    }
    Some(out)
}

/// As sessões que a fotografia pulou e o motivo já anotado: uma linha no diário por sessão e motivo.
static SKIPPED: std::sync::Mutex<BTreeMap<String, &'static str>> = std::sync::Mutex::new(BTreeMap::new());

/// `true` na primeira vez deste motivo para a sessão; `None` limpa (a sessão entrou na fotografia).
fn first_skip(name: &str, code: Option<&'static str>) -> bool {
    let mut skipped = SKIPPED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    match code {
        Some(code) => skipped.insert(name.to_owned(), code) != Some(code),
        None => { skipped.remove(name); false }
    }
}

fn skip(name: &str, code: &'static str, detail: &str) {
    if !first_skip(name, Some(code)) { return; }
    crate::diag::record_blocking(crate::diag::diary_dir().as_deref(),
        &[json!({"evento": "sessao.restauro_pulada", "nivel": "aviso", "sessao": name, "codigo": code, "detalhe": detail})], "restore");
}

/// A chave da sessão `name` quando o spec dela é de uma sessão com terminal.
async fn proven(home: &Path, program: &std::ffi::OsStr, name: &str) -> Option<String> {
    let Some(key) = session_key(program, name).await else {
        skip(name, "session_restore_no_key", "a sessão não tem CP_SESSION_KEY: foi aberta fora do lançador e não volta pelo spec");
        return None;
    };
    let (spec, unreadable) = recorded(home, &key, name);
    if spec.is_none() && unreadable.is_none() {
        skip(name, "session_restore_no_spec", "a chave da sessão não tem spec gravado: ela não volta pelo spec");
    }
    if spec.is_some() { first_skip(name, None); }
    // Uma linha por spec ilegível, não uma a cada fotografia; volta a avisar depois de lido de novo.
    let first = {
        let mut reported = UNREADABLE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if unreadable.is_some() { reported.insert(key.clone()) } else { reported.remove(&key); false }
    };
    if first && let Some(line) = unreadable {
        tracing::warn!(session = %name, "spec ilegível; a sessão fica fora da fotografia");
        crate::diag::record_blocking(crate::diag::diary_dir().as_deref(), &[line], "spec ilegível");
    }
    (spec?.surface == Surface::Terminal).then_some(key)
}

/// O spec gravado da sessão viva; o ilegível devolve também a linha do diário (sem ela a sessão some da
/// fotografia sem rastro).
fn recorded(home: &Path, key: &str, name: &str) -> (Option<super::SessionSpec>, Option<serde_json::Value>) {
    match super::record::read(home, key) {
        Ok(spec) => (spec, None),
        Err(error) => (None, Some(json!({"evento": "sessao.restauro_spec_ilegivel", "nivel": "erro", "sessao": name,
            "codigo": "session_restore_spec_unreadable", "detalhe": error.to_string()}))),
    }
}

/// A conversa em que a sessão volta: a do Codex mora no sidecar dele (o lançador a atualiza).
fn restore_conversation(home: &Path, entry: &Entry) -> Option<String> {
    if entry.provider != AgentProvider::Codex { return entry.conversation.clone(); }
    let meta = crate::runtime::codex_sidecar::read(&crate::runtime::codex_sidecar::path(&crate::runtime::codex_sidecar::dir(home), &entry.name)).ok()??;
    meta.get("thread_id").and_then(|t| t.as_str()).filter(|t| !t.is_empty()).map(str::to_owned).or_else(|| entry.conversation.clone())
}

/// A abertura que traz a sessão de volta: o spec dela, na conversa em que estava, como a mesma sessão
/// (grupo, fila e par do nome ficam).
pub fn resolved_for(home: &Path, server_projects: &Path, key: &str, entry: &Entry) -> Result<super::Resolved, String> {
    let mut spec = spec_for_restore(home, key, entry)?;
    if spec.surface != Surface::Terminal { return Err("restore_no_spec".into()); }
    spec.name = entry.name.clone();
    let conversation = restore_conversation(home, entry).ok_or("restore_no_conversation")?;
    if spec.provider == AgentProvider::Claude && !super::claude::transcript(&spec, &conversation, server_projects).is_file() {
        return Err("restore_transcript_missing".into());
    }
    Ok(super::Resolved { spec, warnings: Vec::new(), provider_source: None, initial_prompt: None, remember_provider: false, previous_non_plan: None,
        resume: Some(conversation), restore: true })
}

/// Reabre uma sessão pelo spec gravado, na conversa dela; devolve o código da falha.
async fn reopen(host: &super::server::ServerHost, key: &str, entry: &Entry) -> Result<(), String> {
    let mut resolved = resolved_for(super::open::OpenHost::home(host), &super::open::OpenHost::server_projects(host), key, entry)?;
    let env = crate::accounts::environment::AccountEnvironment::from_map(super::open::OpenHost::env(host).clone());
    let runtime = tokio::runtime::Handle::current();
    let mut spec = resolved.spec;
    resolved.spec = tokio::task::spawn_blocking(move || {
        spec.refresh_engine_window(&super::facts::LocalFacts::new(env, runtime));
        spec
    }).await.map_err(|_| "restore_spec_job".to_owned())?;
    super::open::open_session(host, resolved, &OpenCaller::Owner).await.map(|_| ()).map_err(|e| e.code.unwrap_or("restore_open_failed").to_owned())
}

/// Até `wait`, o pane da sessão deixa o shell: o agente subiu. `None`: o multiplexador não respondeu.
async fn agent_up(program: &std::ffi::OsStr, name: &str, wait: Duration) -> Option<bool> {
    let deadline = tokio::time::Instant::now() + wait;
    let target = if cfg!(windows) { name.to_owned() } else { format!("={name}:") };
    loop {
        let mut command = crate::terminal_input::child_command(program);
        command.args(["display-message", "-p", "-t", &target, "#{pane_current_command}"]).stdin(std::process::Stdio::null()).kill_on_drop(true);
        let out = tokio::time::timeout(Duration::from_secs(5), command.output()).await.ok()?.ok()?;
        let current = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        // `tmux` é o filho antes do `exec`; `env`, o passo entre o `/bin/sh -c` e o `exec` do agente.
        let up = out.status.success() && !matches!(current.as_str(), "" | "tmux" | "env") && !super::legacy_restore::SHELLS.contains(&current.as_str());
        if up { return Some(true); }
        if !out.status.success() && !crate::list::mux::is_absence(&String::from_utf8_lossy(&out.stderr)) { return None; }
        if tokio::time::Instant::now() >= deadline { return Some(false); }
        tokio::time::sleep(Duration::from_millis(200).min(deadline - tokio::time::Instant::now())).await;
    }
}

/// Tentativas de uma sessão que falhou por um motivo passageiro (conta ocupada, multiplexador calado).
const TRIES: u32 = 3;

/// O que acontece depois de uma passada: a sessão que falhou por um motivo passageiro tenta de novo até
/// `TRIES`; a que nasceu, a que falhou de vez e a que esgotou as tentativas saem (`done`). `true` = nada
/// mais a tentar, e o servidor novo pode virar o da fotografia.
fn settle(results: &[(String, Result<(), String>)], attempts: &mut std::collections::HashMap<String, u32>, done: &mut HashSet<String>) -> bool {
    let passing = |code: &str| matches!(code, "account_busy" | "erro_mux_indisponivel" | "erro_lista_indisponivel" | "restore_spec_job"
        | "erro_grupo_limpeza_falhou");
    let mut left = false;
    for (key, result) in results {
        match result {
            Err(code) if passing(code) => {
                let tried = attempts.entry(key.clone()).or_insert(0);
                *tried += 1;
                if *tried < TRIES { left = true; } else { done.insert(key.clone()); }
            }
            _ => { done.insert(key.clone()); }
        }
    }
    !left
}

/// Reabre uma sessão e grava o resultado no diário.
/// Quanto o restore espera o agente sair do shell depois de abrir.
const AGENT_WAIT: Duration = Duration::from_secs(20);
/// Passadas de reconferência de quem abriu e não mostrou o agente, antes de desistir.
const AGENT_TRIES: u32 = 20;
pub const AGENT_MISSING: &str = "restore_agent_not_started";
pub const MUX_SILENT: &str = "restore_mux_unavailable";

/// O problema na linha da sessão sem agente: o canal de status do terminal existe sem ator.
fn agent_problem(st: &crate::routes::AppState, name: &str, shown: bool) {
    let Some(runtime) = st.state.runtime.get() else { return };
    let sender = runtime.terminal_status_sender(name);
    if shown { crate::runtime::terminal_status::publish_problem(&sender, AGENT_MISSING, "a sessão reabriu e o agente não subiu no terminal"); }
    else { crate::runtime::terminal_status::clear_problem(&sender, AGENT_MISSING); }
}

async fn restore_one(host: &super::server::ServerHost, st: &crate::routes::AppState, program: &std::ffi::OsStr, key: &str, entry: &Entry) -> Result<(), String> {
    let mut result = reopen(host, key, entry).await;
    // Restaurada é o agente de pé: o `new-session` volta antes de o shell soltar o comando.
    if result.is_ok() && entry.provider != AgentProvider::Codex {
        match agent_up(program, &entry.name, AGENT_WAIT).await {
            Some(true) => {}
            Some(false) => { agent_problem(st, &entry.name, true); result = Err(AGENT_MISSING.into()); }
            // O multiplexador calado não diz que o agente faltou: o código é dele, e a reconferência decide.
            None => result = Err(MUX_SILENT.into()),
        }
    }
    let mut event = json!({"evento": "sessao.restaurada", "nivel": "ok", "sessao": entry.name, "provider": entry.provider.name()});
    if let Err(code) = &result {
        tracing::warn!(session = %entry.name, code = %code, "sessão com terminal não restaurada");
        event["evento"] = json!("sessao.restauro_falhou");
        event["nivel"] = json!("erro");
        event["codigo"] = json!(code);
    }
    crate::diag::record_blocking(crate::diag::diary_dir().as_deref(), &[event], "restore");
    result
}

/// A cada `EVERY`: com outro servidor do multiplexador que o da fotografia (a subida depois do reboot, ou o
/// tmux que caiu com o Rust de pé), reabre o que ele perdeu; com o mesmo, fotografa. No Windows decide só
/// na subida (`windows_startup`).
pub async fn run(st: Arc<crate::routes::AppState>, home: PathBuf, program: std::ffi::OsString) {
    let mut snap = read(&home);
    let (mut attempts, mut done) = (std::collections::HashMap::new(), HashSet::new());
    let mut last_count: Option<u64> = None;
    let mut known = HashMap::new();
    let mut windows: Option<bool> = None;
    // As sessões da versão antiga, sem spec, voltam uma vez por subida (`legacy_restore`).
    let mut legacy = false;
    // As que abriram sem o agente aparecer: reconferidas a cada passada.
    let mut waiting: Vec<(Entry, u32)> = Vec::new();
    loop {
        waiting = recheck(&st, &program, std::mem::take(&mut waiting)).await;
        let (current, count) = mux_now(&program).await;
        let live = st.list.discover(None).await.ok().map(|rows| rows.iter().map(|r| r.name.clone()).collect::<HashSet<String>>());
        if cfg!(windows) && windows.is_none() { windows = windows_startup(current, &snap); }
        let same = if cfg!(windows) { windows == Some(false) } else { matches!(current, MuxStart::Started(at) if snap.mux_started == Some(at)) };
        // Restaurando no Windows: volta tudo o que não está vivo, como na subida sem servidor.
        let current = if cfg!(windows) && windows == Some(true) { MuxStart::NoServer } else { current };
        hold(&snap, same);
        match (same, live.and_then(|live| to_restore(&snap, current, &live))) {
            (true, _) => {
                last_count = count;
                if let Some(alive) = alive(&st, &home, &program, &mut known).await {
                    let mut next = observe(&snap, &alive, now(), snap.mux_started);
                    let mut closed = CLOSED.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let gone: Vec<serde_json::Value> = vanished(&snap, &next, &closed).into_iter().map(|(name, conversation)| json!({
                        "evento": "sessao.sumiu", "nivel": "aviso", "sessao": name, "codigo": "session_gone_without_close",
                        "conversa": conversation, "detalhe": "a sessão saiu do multiplexador sem o fechamento pelo Hangar"})).collect();
                    if !gone.is_empty() { crate::diag::record_blocking(crate::diag::diary_dir().as_deref(), &gone, "sessão sumida"); }
                    drop_closed(&mut next, &mut closed);
                    if next != snap {
                        if let Err(error) = write(&home, &next) {
                            tracing::warn!(code = "session_restore_unwritten", kind = ?error.kind(), "fotografia das sessões não gravada");
                        }
                        snap = next;
                    }
                }
            }
            // Sem a lista ou sem o multiplexador não se sabe o que existe: nada é reaberto por cima.
            (false, None) => tracing::warn!(code = "session_restore_undecided", "sem a lista ou o multiplexador; o restore espera"),
            (false, Some(_)) if ended_by_itself(&mut snap, last_count.take(), stopping().await) => {
                tracing::info!("a última sessão do multiplexador terminou sozinha; nada a restaurar");
                if let MuxStart::Started(at) = current { snap.mux_started = Some(at); }
                if let Err(error) = write(&home, &snap) {
                    tracing::warn!(code = "session_restore_unwritten", kind = ?error.kind(), "fotografia das sessões não gravada");
                }
            }
            (false, Some(pending)) => {
                let pending: Vec<(String, Entry)> = pending.into_iter().filter(|(key, _)| !done.contains(key)).collect();
                if !pending.is_empty() { tracing::info!(count = pending.len(), "restaurando as sessões com terminal que o multiplexador perdeu"); }
                let mut results = Vec::new();
                if !pending.is_empty() {
                    let host = super::server::ServerHost::new(st.clone()).await;
                    for (key, entry) in &pending {
                        let result = restore_one(&host, &st, &program, key, entry).await;
                        if result.as_ref().err().is_some_and(|code| code == AGENT_MISSING || code == MUX_SILENT) { waiting.push((entry.clone(), 0)); }
                        results.push((key.clone(), result));
                    }
                }
                // Só com nada mais a tentar o servidor de agora (o restore pode tê-lo subido) passa a ser o da
                // fotografia; antes disso as entradas ficam, e um reinício só do Rust não reabre nada.
                if settle(&results, &mut attempts, &mut done) {
                    if cfg!(target_os = "linux") && !legacy {
                        legacy = true;
                        super::legacy_restore::after_restore(&st, &home, &program).await;
                    }
                    if cfg!(windows) {
                        windows = Some(false);
                        attempts.clear();
                        done.clear();
                    } else if let MuxStart::Started(at) = mux_start(&program).await {
                        snap.mux_started = Some(at);
                        attempts.clear();
                        done.clear();
                    }
                }
            }
        }
        tokio::time::sleep(EVERY).await;
    }
}

/// Reconfere quem abriu sem o agente: o que subiu vai ao diário como restaurada e perde o problema; o que
/// esgotou as passadas vai como falha definitiva; o resto espera a próxima.
async fn recheck(st: &crate::routes::AppState, program: &std::ffi::OsStr, waiting: Vec<(Entry, u32)>) -> Vec<(Entry, u32)> {
    let mut left = Vec::new();
    for (entry, tries) in waiting {
        let (event, code) = match agent_up(program, &entry.name, Duration::ZERO).await {
            Some(true) => { agent_problem(st, &entry.name, false); ("sessao.restaurada", "restore_agent_late") }
            _ if tries + 1 >= AGENT_TRIES => ("sessao.restauro_falhou", "restore_agent_never_started"),
            _ => { left.push((entry, tries + 1)); continue; }
        };
        let nivel = if event == "sessao.restaurada" { "ok" } else { "erro" };
        crate::diag::record_blocking(crate::diag::diary_dir().as_deref(),
            &[json!({"evento": event, "nivel": nivel, "sessao": entry.name, "provider": entry.provider.name(), "codigo": code})], "restore");
    }
    left
}

/// A tarefa do restore, com o runtime e as pastas da lista de pé.
pub fn spawn(st: Arc<crate::routes::AppState>) -> Option<tokio::task::JoinHandle<()>> {
    let dirs = st.list.env().dirs.clone()?;
    st.state.runtime.get()?;
    let program = st.list.env().capture_program.clone();
    // Antes da primeira rodada da varredura de grupos: até o restore decidir, a fotografia segura os nomes.
    hold(&read(&dirs.home), false);
    Some(tokio::spawn(run(st, dirs.home, program)))
}

#[cfg(test)]
mod tests {
    /// SF6: o spec ilegível de uma sessão viva vai ao diário; o ausente não.
    #[test]
    fn an_unreadable_spec_of_a_live_session_goes_to_the_diary() {
        let home = tempfile::tempdir().unwrap();
        let key = "ab".repeat(16);
        assert_eq!(super::recorded(home.path(), &key, "s1").1, None, "sem registro: nada a dizer");
        let dir = super::super::record::dir(home.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{key}.json")), "{torto").unwrap();
        let (spec, line) = super::recorded(home.path(), &key, "s1");
        assert!(spec.is_none());
        let line = line.expect("linha do diário");
        assert_eq!((line["codigo"].as_str(), line["sessao"].as_str()), (Some("session_restore_spec_unreadable"), Some("s1")));
    }

    use super::*;

    fn entry(name: &str, conversation: Option<&str>) -> Entry {
        Entry { name: name.into(), provider: AgentProvider::Claude, conversation: conversation.map(str::to_owned), permission_mode: None, missing_since: None }
    }

    fn recorded(home: &Path, key: &str, provider: AgentProvider, mode: super::super::PermissionMode) {
        let spec = super::super::SessionSpec { version: super::super::SPEC_VERSION, key: key.into(), name: "s1".into(), cwd: "/w".into(), provider,
            surface: Surface::Terminal, account: if provider == AgentProvider::Codex { super::super::Account::Codex { account_id: "default".into() } }
            else { super::super::Account::Default }, account_source: super::super::AccountSource::Default, model: None, effort: None,
            service_tier: None, permission_mode: Some(mode), subagent_model: None, customizations: None, jev: false, read_only: false,
            worktree: None, creator: None, created_at: 1.0, conversation: Some("c1".into()),
            name_auto: None, rename_suggestion: None, rename_weak_at: None, rename_dismissed: false };
        super::super::record::write(home, &spec).unwrap();
    }

    /// No Windows só a subida do Rust sem nenhum servidor psmux e com sessões na fotografia
    /// restaura; com servidor vivo nunca, e sem resposta do psmux a decisão espera.
    #[test]
    fn on_windows_only_a_start_without_any_psmux_server_restores() {
        let full = Snapshot { mux_started: Some(10), sessions: BTreeMap::from([("k".repeat(32), entry("s1", Some("c1")))]) };
        let empty = Snapshot::default();
        assert_eq!(windows_startup(MuxStart::NoServer, &full), Some(true));
        assert_eq!(windows_startup(MuxStart::NoServer, &empty), Some(false));
        assert_eq!(windows_startup(MuxStart::Started(99), &full), Some(false), "servidor vivo: nada volta");
        assert_eq!(windows_startup(MuxStart::Started(10), &full), Some(false));
        assert_eq!(windows_startup(MuxStart::Unknown, &full), None);
    }

    /// O servidor que saiu porque a única sessão dele terminou (o agente saiu) não traz nada de
    /// volta; com mais sessões de uma vez, no desligamento ou sem a contagem vista nesta execução, é queda.
    #[test]
    fn a_server_that_ended_with_its_last_session_brings_nothing_back() {
        let snap = Snapshot { mux_started: Some(10), sessions: BTreeMap::from([("k".repeat(32), entry("s1", Some("c1")))]) };
        for (count, stopping, ended) in [(Some(1), false, true), (Some(2), false, false), (Some(1), true, false), (None, false, false)] {
            let mut after = snap.clone();
            assert_eq!(ended_by_itself(&mut after, count, stopping), ended, "{count:?} {stopping}");
            assert_eq!(after.sessions.is_empty(), ended, "{count:?} {stopping}");
        }
    }

    #[test]
    fn the_restore_comes_back_in_the_mode_the_session_had_when_it_fell() {
        use super::super::PermissionMode;
        let dir = tempfile::tempdir().unwrap();
        let key = "ab".repeat(16);
        recorded(dir.path(), &key, AgentProvider::Claude, PermissionMode::BypassPermissions);
        let lowered = Entry { permission_mode: Some("acceptEdits".into()), ..entry("s1", Some("c1")) };
        assert_eq!(spec_for_restore(dir.path(), &key, &lowered).unwrap().permission_mode, Some(PermissionMode::AcceptEdits), "Claude baixado por teclas");
        assert_eq!(spec_for_restore(dir.path(), &key, &entry("s1", Some("c1"))).unwrap().permission_mode, Some(PermissionMode::BypassPermissions),
            "sem modo visto, o do spec");
        let codex = "cd".repeat(16);
        recorded(dir.path(), &codex, AgentProvider::Codex, PermissionMode::FullAccess);
        let sidecars = crate::runtime::codex_sidecar::dir(dir.path());
        std::fs::create_dir_all(&sidecars).unwrap();
        std::fs::write(crate::runtime::codex_sidecar::path(&sidecars, "s1"), json!({"name": "s1", "provider": "codex", "key": codex, "cwd": "/w",
            "thread_id": "t1", "permission_mode": "Ask for approval"}).to_string()).unwrap();
        let entry = Entry { provider: AgentProvider::Codex, ..entry("s1", None) };
        assert_eq!(spec_for_restore(dir.path(), &codex, &entry).unwrap().permission_mode, Some(PermissionMode::AskForApproval), "o modo que a TUI gravou");
    }

    #[test]
    fn a_session_leaves_the_snapshot_only_after_being_absent_with_the_server_up() {
        let alive: BTreeMap<String, Entry> = [("k1".into(), entry("a", Some("c1"))), ("k2".into(), entry("b", None))].into();
        let first = observe(&Snapshot::default(), &alive, 1000.0, Some(7));
        assert_eq!(first.sessions.len(), 2);
        let only_a: BTreeMap<String, Entry> = [("k1".into(), entry("a", None))].into();
        let second = observe(&first, &only_a, 1010.0, Some(7));
        assert_eq!(second.sessions["k1"].conversation.as_deref(), Some("c1"), "a conversa provada antes fica quando a lista não a prova");
        assert_eq!(second.sessions["k2"].missing_since, Some(1010.0), "ausente fica, marcada");
        assert!(observe(&second, &only_a, 1010.0 + ABSENT - 1.0, Some(7)).sessions.contains_key("k2"), "o desligamento derruba tudo em segundos");
        assert!(!observe(&second, &only_a, 1010.0 + ABSENT, Some(7)).sessions.contains_key("k2"), "fechada de propósito sai");
        let back = observe(&second, &alive, 1020.0, Some(7));
        assert_eq!(back.sessions["k2"].missing_since, None, "voltou");
    }

    #[test]
    fn only_another_multiplexer_server_restores_and_never_over_a_live_name() {
        let snap = Snapshot { mux_started: Some(1000), sessions: [("k1".into(), entry("a", Some("c1"))), ("k2".into(), entry("b", Some("c2")))].into() };
        let keys = |p: Option<Vec<(String, Entry)>>| p.map(|v| v.into_iter().map(|(k, _)| k).collect::<Vec<_>>());
        assert_eq!(keys(to_restore(&snap, MuxStart::Started(1000), &HashSet::new())), Some(vec![]), "o mesmo servidor: só o Rust reiniciou");
        let live: HashSet<String> = ["a".to_owned()].into();
        assert_eq!(keys(to_restore(&snap, MuxStart::Started(5000), &live)), Some(vec!["k2".to_owned()]), "outro servidor (reboot, kill-server)");
        assert_eq!(keys(to_restore(&snap, MuxStart::NoServer, &HashSet::new())), Some(vec!["k1".to_owned(), "k2".to_owned()]), "nenhum servidor");
        assert_eq!(keys(to_restore(&snap, MuxStart::Unknown, &HashSet::new())), None, "multiplexador calado: decide depois");
    }

    #[test]
    fn a_passing_failure_is_tried_again_and_the_new_server_waits_for_it() {
        let (mut attempts, mut done) = (std::collections::HashMap::new(), HashSet::new());
        let results = |code: &str| vec![("ok".to_owned(), Ok(())), ("bad".to_owned(), Err("restore_no_spec".to_owned())), ("busy".to_owned(), Err(code.to_owned()))];
        assert!(!settle(&results("account_busy"), &mut attempts, &mut done), "a conta ocupada tenta de novo");
        assert!(done.contains("ok") && done.contains("bad") && !done.contains("busy"), "{done:?}");
        assert!(!settle(&[("busy".to_owned(), Err("erro_mux_indisponivel".to_owned()))], &mut attempts, &mut done));
        assert!(settle(&[("busy".to_owned(), Err("account_busy".to_owned()))], &mut attempts, &mut done), "esgotou: o servidor novo é adotado");
        assert!(done.contains("busy"));
    }

    /// A sessão que some do multiplexador sem o `DELETE` do dono vai ao diário uma vez, quando some.
    #[test]
    fn a_session_that_vanishes_without_a_close_is_named_once() {
        let prev = Snapshot { mux_started: Some(1), sessions: [("k1".into(), entry("a", Some("c1"))), ("k2".into(), entry("b", Some("c2"))),
            ("k3".into(), Entry { missing_since: Some(5.0), ..entry("c", None) })].into() };
        let next = observe(&prev, &BTreeMap::new(), 10.0, Some(1));
        assert_eq!(vanished(&prev, &next, &["b".to_owned()]), vec![("a".to_owned(), Some("c1".to_owned()))],
            "b foi fechada pelo dono; c já estava ausente");
    }

    /// Restaurada é o pane com o agente de pé: o shell parado (o `config.fish` travado) não conta.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_restored_pane_counts_only_once_it_left_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("tmux");
        let socket = format!("hangar-restore-agent-{}", std::process::id());
        crate::write_test_executable(&program, format!("#!/bin/sh\nSHELL=/bin/sh exec tmux -L {socket} -f /dev/null \"$@\"\n"));
        let run = |args: &[&str]| std::process::Command::new(&program).args(args).status().unwrap().success();
        // Em argumentos separados o tmux roda direto, como o lançamento: o `/bin/sh -c` do comando único fica
        // de pai no dash e se troca no bash.
        assert!(run(&["new-session", "-d", "-s", "parada", "sh", "-i"]) && run(&["new-session", "-d", "-s", "viva", "sleep", "60"]));
        let program = program.as_os_str();
        assert_eq!(agent_up(program, "viva", Duration::from_secs(5)).await, Some(true));
        assert_eq!(agent_up(program, "parada", Duration::from_millis(600)).await, Some(false), "shell parado");
        assert!(run(&["send-keys", "-t", "=parada:", "exec sleep 60", "Enter"]));
        assert_eq!(agent_up(program, "parada", Duration::from_secs(5)).await, Some(true), "o agente subiu depois");
        assert_eq!(agent_up(program, "nenhuma", Duration::from_millis(300)).await, Some(false), "sem a sessão");
        let _ = std::process::Command::new(dir.path().join("tmux")).arg("kill-server").status();
    }

    /// A sessão que a fotografia pula vai ao diário uma vez por motivo, e de novo depois de entrar nela.
    #[test]
    fn a_skipped_session_is_noted_once_per_reason() {
        assert!(first_skip("sk-a", Some("session_restore_no_key")) && !first_skip("sk-a", Some("session_restore_no_key")));
        assert!(first_skip("sk-a", Some("session_restore_no_spec")), "outro motivo");
        first_skip("sk-a", None);
        assert!(first_skip("sk-a", Some("session_restore_no_spec")), "depois de entrar na fotografia");
    }

    #[test]
    fn a_session_closed_on_purpose_leaves_the_snapshot_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let snap = Snapshot { mux_started: Some(1), sessions: [("k1".into(), entry("a", Some("c1"))), ("k2".into(), entry("b", Some("c2")))].into() };
        write(dir.path(), &snap).unwrap();
        forget(dir.path(), "a");
        assert_eq!(read(dir.path()).sessions.keys().collect::<Vec<_>>(), ["k2"], "o arquivo, já");
        let mut memory = snap.clone();
        let mut closed = CLOSED.lock().unwrap();
        drop_closed(&mut memory, &mut closed);
        assert!(!memory.sessions.contains_key("k1") && memory.sessions.contains_key("k2"), "a do laço também, antes de gravar");
        assert!(closed.is_empty());
    }

    #[test]
    fn the_names_waiting_for_the_restore_count_as_alive_until_it_settles() {
        let snap = Snapshot { mux_started: Some(1), sessions: [("k1".into(), entry("a", Some("c1"))), ("k2".into(), entry("b", None))].into() };
        hold(&snap, false);
        assert_eq!(held(), ["a", "b"]);
        hold(&snap, true);
        assert!(held().is_empty());
    }

    #[test]
    fn the_conversation_comes_from_each_agent_transcript() {
        let id = "33333333-3333-4333-8333-333333333333";
        assert_eq!(conversation_of(AgentProvider::Claude, &format!("/h/.claude/projects/-w/{id}.jsonl")).as_deref(), Some(id));
        assert_eq!(conversation_of(AgentProvider::Pi, &format!("/h/.pi/agent/sessions/--w--/2026-10-01T00-00-00-000Z_{id}.jsonl")).as_deref(), Some(id));
        assert_eq!(conversation_of(AgentProvider::Omp, &format!("/h/.omp/agent/sessions/-/2026-10-01T00-00-00-000Z_{id}.jsonl")).as_deref(), Some(id));
        assert_eq!(conversation_of(AgentProvider::Kimi, &format!("/h/.kimi-code/sessions/w/session_{id}/agents/main/wire.jsonl")).as_deref(),
            Some(format!("session_{id}").as_str()));
        assert_eq!(conversation_of(AgentProvider::Codex, "/h/.codex/sessions/rollout-x.jsonl"), None, "o Codex volta pelo sidecar");
    }
}
