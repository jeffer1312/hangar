//! Sessões com terminal que o systemd derrubou (o `systemd-oomd` mata o escopo do pane inteiro quando a
//! máquina fica sem memória). Saem do multiplexador sem o fechamento pelo Hangar e, sem isto, sumiam da
//! lista sem jeito de voltar. Só vira registro a que tem no journal `Failed with result` da unidade do pane:
//! a saída pelo próprio agente (`/exit`) não deixa essa linha e é a pessoa saindo.
//!
//! O registro é pelo nome, em memória atrás de `MAP` e espelhado em `~/.hangar/fallen-sessions.json`. A lista
//! mostra cada um como linha `dead`; o Reabrir volta pelo spec (`restore::reopen`) e o `DELETE` o dispensa.
use std::collections::{BTreeMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use hangar_api::session::SessionRow;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::AgentProvider;
use super::restore::Entry;
use crate::routes::{AppState, cors};
use crate::session_write::{detail_body, json_response, session_name};

pub const FILE: &str = "fallen-sessions.json";
pub const OOM: &str = "terminal_out_of_memory";
pub const OTHER: &str = "terminal_died";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Fallen {
    /// A chave do spec (`session-specs/<key>.json`).
    pub key: String,
    pub provider: AgentProvider,
    pub conversation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_mode: Option<String>,
    /// A pasta do spec, para a linha da lista.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    pub died_at: f64,
    /// O `<r>` de `Failed with result '<r>'` (`oom-kill`, …).
    pub cause: String,
}

impl Fallen {
    /// O problema da linha na lista.
    pub fn problem(&self) -> &'static str { if self.cause == "oom-kill" { OOM } else { OTHER } }

    /// A entrada da fotografia com que o restore a reabre.
    pub fn entry(&self, name: &str) -> Entry {
        Entry { name: name.to_owned(), provider: self.provider, conversation: self.conversation.clone(),
            permission_mode: self.permission_mode.clone(), missing_since: None, unit: None }
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Stored { version: u32, sessions: BTreeMap<String, Fallen> }

/// O mapa e a pasta de onde veio: outra pasta (os testes) relê do disco.
static MAP: std::sync::Mutex<Option<(PathBuf, BTreeMap<String, Fallen>)>> = std::sync::Mutex::new(None);

fn path(home: &Path) -> PathBuf { home.join(".hangar").join(FILE) }

/// Ausente = vazio; ilegível também, com aviso.
fn load(home: &Path) -> BTreeMap<String, Fallen> {
    match std::fs::read(path(home)) {
        Ok(raw) => serde_json::from_slice::<Stored>(&raw).map(|s| s.sessions).unwrap_or_else(|error| {
            tracing::warn!(code = "fallen_unreadable", %error, "registro das sessões caídas ilegível; começa vazio");
            BTreeMap::new()
        }),
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(code = "fallen_unreadable", kind = ?error.kind(), "registro das sessões caídas ilegível");
            }
            BTreeMap::new()
        }
    }
}

/// Aplica `change` sob a trava; `true` dele grava o arquivo, ainda sob a trava.
fn with<T>(home: &Path, change: impl FnOnce(&mut BTreeMap<String, Fallen>) -> (T, bool)) -> T {
    let mut guard = MAP.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if guard.as_ref().is_some_and(|(from, _)| from != home) { *guard = None; }
    let (_, map) = guard.get_or_insert_with(|| (home.to_owned(), load(home)));
    let (out, changed) = change(map);
    if changed {
        let stored = Stored { version: 1, sessions: map.clone() };
        let written = std::fs::create_dir_all(home.join(".hangar"))
            .and_then(|()| crate::runtime::queue::atomic_write(&path(home), &serde_json::to_vec_pretty(&stored)?));
        if let Err(error) = written {
            tracing::warn!(code = "fallen_unwritten", kind = ?error.kind(), "registro das sessões caídas não gravado; vale até o restart");
        }
    }
    out
}

pub fn get(home: &Path, name: &str) -> Option<Fallen> { with(home, |map| (map.get(name).cloned(), false)) }

#[cfg(test)]
fn insert(home: &Path, name: &str, fallen: Fallen) {
    with(home, |map| ((), map.insert(name.to_owned(), fallen.clone()).as_ref() != Some(&fallen)));
}

/// Grava só se o nome não foi fechado de propósito desde `since`. Conferido sob a trava do mapa: o `forget`
/// marca antes de remover, então ou ele vê o registro e o tira, ou a gravação vê a marca e desiste.
pub fn insert_unless_forgotten(home: &Path, name: &str, since: std::time::Instant, fallen: Fallen) -> bool {
    with(home, |map| {
        if forgotten_since(name, since) { return (false, false); }
        let changed = map.insert(name.to_owned(), fallen.clone()).as_ref() != Some(&fallen);
        (true, changed)
    })
}

pub fn remove(home: &Path, name: &str) -> Option<Fallen> {
    with(home, |map| { let gone = map.remove(name); let changed = gone.is_some(); (gone, changed) })
}

/// Nome com sessão viva (com ou sem terminal) não está caído: a sessão voltou por outro caminho.
pub fn forget_live(home: &Path, live: &HashSet<String>) {
    with(home, |map| { let before = map.len(); map.retain(|name, _| !live.contains(name)); ((), map.len() != before) });
}

/// As caídas sem sessão viva com o nome, como linhas `dead` da lista que o dono vê. Só na publicação: a foto
/// da lista (`Produced`) segue só com as vivas, que é o que push, grupos e as rotas leem. O registro na
/// memória é o que o laço do restore carregou na subida.
pub fn rows(home: &Path, live: &[&SessionRow]) -> Vec<SessionRow> {
    let names: HashSet<&str> = live.iter().map(|r| r.name.as_str()).collect();
    with(home, |map| (map.iter().filter(|(name, _)| !names.contains(name.as_str())).filter_map(|(name, fallen)| {
        let row = json!({"name": name, "provider": fallen.provider.name(), "state": "dead", "problema": fallen.problem(),
            "cwd": fallen.cwd, "last_activity": fallen.died_at});
        serde_json::from_value::<SessionRow>(row)
            .inspect_err(|error| tracing::warn!(code = "fallen_row_invalid", %error, session = %name, "linha da sessão caída não montada"))
            .ok()
    }).collect(), false))
}

fn answer(headers: &HeaderMap, status: StatusCode, body: Value) -> Response {
    let mut response = json_response(status, body);
    cors(headers, response.headers_mut());
    response
}

/// `POST /api/sessions/{name}/reopen`: a caída volta pelo spec, na conversa em que estava (o mesmo
/// caminho do restore depois do reboot). O registro só sai quando a sessão abriu.
pub async fn reopen_route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let owner = match crate::routes::gate(&st, peer, &req).await { Ok((_, owner)) => owner, Err(e) => return crate::auth::unavailable(e) };
    if !owner { return answer(&headers, StatusCode::FORBIDDEN, detail_body("erro_so_dono", "só o dono reabre a sessão", json!({}))); }
    let (Some(name), Some(dirs)) = (session_name(req.uri().path()), st.list.env().dirs.clone()) else {
        return answer(&headers, StatusCode::NOT_FOUND, detail_body("session_not_fallen", "a sessão não está caída", json!({})));
    };
    // A marca antes da leitura: um Dispensar que termine entre as duas não deixa reabrir o registro já tirado.
    let Some(reopening) = Reopening::begin(&name) else {
        return answer(&headers, StatusCode::CONFLICT, detail_body("session_reopening", "a sessão já está reabrindo", json!({"name": name})));
    };
    let Some(fallen) = get(&dirs.home, &name) else {
        return answer(&headers, StatusCode::NOT_FOUND, detail_body("session_not_fallen", "a sessão não está caída", json!({"name": name})));
    };
    // Cliente que cai no meio não interrompe a abertura. O mesmo caminho do restore: agente de pé, diário, e o
    // registro só sai no sucesso.
    let entry = fallen.entry(&name);
    let program = st.list.env().capture_program.clone();
    let task = tokio::spawn(async move {
        let _reopening = reopening;
        let host = super::server::ServerHost::new(st.clone()).await;
        super::restore::restore_one(&host, &st, &program, &fallen.key, &entry).await
    });
    match task.await {
        Ok(Ok(())) => answer(&headers, StatusCode::OK, json!({"ok": true})),
        Ok(Err(code)) if code == "erro_nome_em_uso" =>
            answer(&headers, StatusCode::CONFLICT, detail_body(&code, "já existe uma sessão com esse nome", json!({}))),
        Ok(Err(code)) => answer(&headers, StatusCode::UNPROCESSABLE_ENTITY, detail_body(&code, "a sessão não reabriu", json!({}))),
        Err(error) => {
            tracing::error!(code = "restore_open_failed", %error, session = %name, "a reabertura da sessão caída parou no meio");
            answer(&headers, StatusCode::INTERNAL_SERVER_ERROR, detail_body("restore_open_failed", "a sessão não reabriu", json!({})))
        }
    }
}

/// Nomes reabrindo agora; sai quando a tarefa da reabertura acaba, mesmo com o cliente já desconectado.
static REOPENING: std::sync::Mutex<std::collections::BTreeSet<String>> = std::sync::Mutex::new(std::collections::BTreeSet::new());

pub(crate) struct Reopening(String);

impl Reopening {
    pub(crate) fn begin(name: &str) -> Option<Self> {
        REOPENING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).insert(name.to_owned()).then(|| Self(name.to_owned()))
    }
    #[cfg(test)]
    fn active(name: &str) -> bool { REOPENING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).contains(name) }
}

impl Drop for Reopening {
    fn drop(&mut self) { REOPENING.lock().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&self.0); }
}

/// `POST /api/sessions/{name}/dismiss`: tira a caída da lista sem tocar em sessão nenhuma. Só age se o nome
/// ainda está caído: a linha velha no app não fecha a sessão que acabou de reabrir com o mesmo nome.
pub async fn dismiss_route(State(st): State<Arc<AppState>>, ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request) -> Response {
    let headers = req.headers().clone();
    let owner = match crate::routes::gate(&st, peer, &req).await { Ok((_, owner)) => owner, Err(e) => return crate::auth::unavailable(e) };
    if !owner { return answer(&headers, StatusCode::FORBIDDEN, detail_body("erro_so_dono", "só o dono dispensa a sessão", json!({}))); }
    let (Some(name), Some(dirs)) = (session_name(req.uri().path()), st.list.env().dirs.clone()) else {
        return answer(&headers, StatusCode::NOT_FOUND, detail_body("session_not_fallen", "a sessão não está caída", json!({})));
    };
    // Reabrindo (por outro aparelho): dispensar agora tiraria o registro, e a reabertura que falhar não teria
    // mais como tentar de novo pela lista. A marca vem antes da leitura: a reabertura que terminar entre as
    // duas já tirou o registro, e o `forget` não apaga da fotografia a sessão que acabou de voltar.
    let Some(_hold) = Reopening::begin(&name) else {
        return answer(&headers, StatusCode::CONFLICT, detail_body("session_reopening", "a sessão está reabrindo", json!({"name": name})));
    };
    if get(&dirs.home, &name).is_none() {
        return answer(&headers, StatusCode::NOT_FOUND, detail_body("session_not_fallen", "a sessão não está caída", json!({"name": name})));
    }
    // O `forget` também tira o nome da fotografia do restore: a dispensada não volta no próximo reboot.
    super::restore::forget(&dirs.home, &name);
    answer(&headers, StatusCode::OK, json!({"ok": true}))
}

/// O `<r>` da última linha `Failed with result '<r>'` do journal da unidade.
pub fn failure(journal: &str) -> Option<String> {
    journal.lines().rev().find_map(|line| {
        let rest = line.split("Failed with result '").nth(1)?;
        Some(rest.split('\'').next()?.to_owned()).filter(|r| !r.is_empty())
    })
}

/// A unidade do systemd de um processo: o último trecho do cgroup v2 quando é um escopo.
pub fn unit_of_cgroup(cgroup: &str) -> Option<String> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    let unit = path.trim().rsplit('/').next()?;
    // Só o escopo que o tmux abre por pane é da sessão; sem ele o pane herda o grupo do servidor ou do login.
    (unit.starts_with("tmux-spawn-") && unit.ends_with(".scope")).then(|| unit.to_owned())
}

/// A unidade do pane de cada sessão (o processo que o multiplexador lançou), numa consulta só. Relida a
/// cada tique: o pane recriado com o mesmo nome (troca de conta, `/recarregar`) tem outra unidade. Só Linux.
pub async fn pane_units(program: &std::ffi::OsStr) -> BTreeMap<String, String> {
    if !cfg!(target_os = "linux") { return BTreeMap::new(); }
    crate::accounts::release::panes(program).await.into_iter().filter_map(|(pid, name)| {
        Some((name, unit_of_cgroup(&std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?)?))
    }).collect()
}

/// Quando cada nome foi fechado de propósito pela última vez: a confirmação que começou antes disso não
/// grava (o nome pode ter sido aberto e fechado de novo enquanto o journal era lido).
static FORGOTTEN: std::sync::Mutex<BTreeMap<String, std::time::Instant>> = std::sync::Mutex::new(BTreeMap::new());

pub fn mark_forgotten(name: &str) {
    let now = std::time::Instant::now();
    let mut forgotten = FORGOTTEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    // A confirmação em voo dura no máximo o journal (~20 s): marca mais velha que isso não decide mais nada.
    forgotten.retain(|_, at| now.duration_since(*at) < Duration::from_secs(120));
    forgotten.insert(name.to_owned(), now);
}

fn forgotten_since(name: &str, since: std::time::Instant) -> bool {
    FORGOTTEN.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(name).is_some_and(|at| *at >= since)
}

async fn journal(unit: &str) -> Option<String> {
    let run = tokio::process::Command::new("journalctl")
        // `-b`: o nome do escopo pode se repetir entre boots, e a falha de um boot antigo não é desta sessão.
        .args(["--user", "-b", "-u", unit, "-o", "cat", "-n", "20", "--no-pager"])
        .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).kill_on_drop(true).output();
    match tokio::time::timeout(Duration::from_secs(2), run).await {
        Ok(Ok(out)) => Some(String::from_utf8_lossy(&out.stdout).into_owned()),
        Ok(Err(error)) => { tracing::warn!(code = "fallen_journal_failed", %error, %unit, "journalctl não rodou"); None }
        Err(_) => { tracing::warn!(code = "fallen_journal_timeout", %unit, "journalctl passou de 2 s"); None }
    }
}

/// O systemd grava o `Failed with result` alguns segundos depois de matar o escopo.
const JOURNAL_TRIES: u32 = 4;
const JOURNAL_GAP: Duration = Duration::from_secs(5);

/// A falha do escopo `unit`, esperando o journal alcançar a morte. O `Consumed …` sai por último, também
/// no `/exit`: com ele e sem `Failed`, a unidade acabou bem e não há o que esperar.
pub async fn cause(unit: &str) -> Option<String> {
    for attempt in 0..JOURNAL_TRIES {
        let text = journal(unit).await.unwrap_or_default();
        if let Some(found) = failure(&text) { return Some(found); }
        if text.contains(": Consumed ") { return None; }
        if attempt + 1 < JOURNAL_TRIES { tokio::time::sleep(JOURNAL_GAP).await; }
    }
    tracing::warn!(code = "fallen_journal_silent", %unit, "o journal não disse como a unidade acabou; a sessão não fica na lista");
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oomd_kill_of_november_6_is_a_failure_and_a_normal_end_is_not() {
        let oomd = "Started tmux child pane 1674300 launched by process 1637.\n\
            tmux-spawn-c2949ae0-48a7-4308-bbe6-74c96151cf1c.scope: systemd-oomd killed 118 process(es) in this unit.\n\
            tmux-spawn-c2949ae0-48a7-4308-bbe6-74c96151cf1c.scope: Failed with result 'oom-kill'.\n\
            tmux-spawn-c2949ae0-48a7-4308-bbe6-74c96151cf1c.scope: Consumed 3h 40.261s CPU time over 13h 25min 51.639s wall clock time, 5.6G memory peak, 10.9G memory swap peak.";
        assert_eq!(failure(oomd).as_deref(), Some("oom-kill"));
        let exit = "Started tmux child pane 1976563 launched by process 1637.\n\
            tmux-spawn-becb7e48-f1bb-4aa5-9a55-9eb6cf7deed2.scope: Consumed 1min 17.447s CPU time over 7min 16.148s wall clock time, 937.3M memory peak, 51.3M memory swap peak.";
        assert_eq!(failure(exit), None, "/exit: a pessoa saiu");
        let killed = "teste.scope: Killed unit cgroup '/user.slice/x/teste.scope' with SIGKILL on client request.";
        assert_eq!(failure(killed), None);
    }

    #[test]
    fn the_unit_is_the_scope_of_the_cgroup() {
        let cgroup = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/tmux-spawn-7ba9eb0b-4cd5-4c65-a55e-68d979c03ed7.scope\n";
        assert_eq!(unit_of_cgroup(cgroup).as_deref(), Some("tmux-spawn-7ba9eb0b-4cd5-4c65-a55e-68d979c03ed7.scope"));
        assert_eq!(unit_of_cgroup("0::/user.slice/user-1000.slice/session-2.slice\n"), None);
        // Tmux sem escopo por pane: o login e o terminal se repetem entre sessões e boots, não dizem qual caiu.
        assert_eq!(unit_of_cgroup("0::/user.slice/user-1000.slice/session-2.scope\n"), None);
        assert_eq!(unit_of_cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-kitty-1234.scope\n"), None);
    }

    fn oom(key: &str) -> Fallen {
        Fallen { key: key.into(), provider: AgentProvider::Claude, conversation: Some("c1".into()), permission_mode: None,
            cwd: Some("/home/x".into()), died_at: 10.0, cause: "oom-kill".into() }
    }

    fn row(name: &str) -> SessionRow { serde_json::from_value(json!({"name": name, "provider": "claude", "state": "idle"})).unwrap() }

    /// A caída vira linha `dead` com o problema e a pasta; com sessão viva do mesmo nome, nenhuma linha.
    #[test]
    fn a_fallen_session_is_a_dead_row_until_a_live_one_takes_the_name() {
        let home = tempfile::tempdir().unwrap();
        insert(home.path(), "connect", oom("k1"));
        let other = row("outra");
        let dead = rows(home.path(), &[&other]);
        assert_eq!(dead.iter().map(|r| (r.name.as_str(), r.state.as_str(), r.problema.as_deref(), r.cwd.as_deref())).collect::<Vec<_>>(),
            vec![("connect", "dead", Some(OOM), Some("/home/x"))]);
        assert!(rows(home.path(), &[&row("connect")]).is_empty(), "a viva com o nome esconde a caída");
    }

    /// O registro sobrevive ao restart (arquivo) e sai quando a sessão volta viva ou é dispensada.
    #[test]
    fn the_record_survives_a_restart_and_leaves_when_the_name_lives_again() {
        let home = tempfile::tempdir().unwrap();
        insert(home.path(), "a", oom("k1"));
        insert(home.path(), "b", oom("k2"));
        *MAP.lock().unwrap() = None;
        assert_eq!(get(home.path(), "a"), Some(oom("k1")), "relido do arquivo");
        forget_live(home.path(), &HashSet::from(["a".to_owned()]));
        assert_eq!(get(home.path(), "a"), None);
        assert_eq!(remove(home.path(), "b"), Some(oom("k2")));
        *MAP.lock().unwrap() = None;
        assert_eq!(get(home.path(), "b"), None, "relido do arquivo, sem a dispensada");
    }

    /// Dispensada (ou reaberta e fechada) enquanto o journal era lido: a confirmação que começou antes não grava.
    #[test]
    fn a_name_closed_after_the_check_started_is_not_recorded() {
        let home = tempfile::tempdir().unwrap();
        let started = std::time::Instant::now();
        assert!(insert_unless_forgotten(home.path(), "antes", started, oom("k1")));
        mark_forgotten("depois");
        assert!(!insert_unless_forgotten(home.path(), "depois", started, oom("k2")));
        assert_eq!(get(home.path(), "depois"), None);
        assert_eq!(get(home.path(), "antes"), Some(oom("k1")));
    }

    /// Um nome reabre uma vez por vez, e fica livre quando a reabertura acaba (o fim da tarefa solta a marca).
    #[test]
    fn a_name_reopens_once_at_a_time() {
        let first = Reopening::begin("reabre-um").expect("primeira reabertura");
        assert!(Reopening::begin("reabre-um").is_none(), "a segunda espera");
        assert!(Reopening::active("reabre-um"));
        drop(first);
        assert!(!Reopening::active("reabre-um"));
        assert!(Reopening::begin("reabre-um").is_some());
    }

    #[test]
    fn the_problem_tells_memory_from_any_other_failure() {
        let fallen = |cause: &str| Fallen { key: "k".into(), provider: AgentProvider::Claude, conversation: None,
            permission_mode: None, cwd: None, died_at: 0.0, cause: cause.into() };
        assert_eq!(fallen("oom-kill").problem(), OOM);
        assert_eq!(fallen("signal").problem(), OTHER);
    }
}
