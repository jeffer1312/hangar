//! A vida de uma sessão no lançador: o arquivo da sessão (gravado na conversa nova, na retomada e na
//! troca de conversa, apagado na saída só se ainda é desta vida), o app-server com as ressubidas e a
//! escuta de threads.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Map, Value, json};
use tokio::time::Instant;

use super::app_server::{self, Diary, Server, Tail};
use super::plan::Plan;
use super::{Screen, threads};
use crate::runtime::codex_sidecar;

/// Quedas toleradas em 5 min antes de assumir que o app-server cai em laço.
const MAX_FALLS: usize = 3;
const FALL_WINDOW: Duration = Duration::from_secs(300);
/// Fechar o terminal derruba os dois juntos: a TUI saindo logo depois do app-server é o fim da sessão.
const SESSION_END: Duration = Duration::from_millis(1500);

/// O que as gravações do arquivo da sessão precisam, comum à escuta de threads e ao lançador.
pub(crate) struct Session {
    dir: PathBuf,
    name: String,
    pub endpoint: String,
    cwd: String,
    resume: Option<String>,
    model: Option<String>,
    effort: Option<String>,
    limit: Option<u64>,
    service_tier: Option<String>,
    codex_home: String,
    account: String,
    mode: Option<&'static str>,
    imported: bool,
    key: Option<String>,
    launcher_pid: u32,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    /// A conversa que esta vida gravou por último.
    previous: Option<String>,
    app_pid: u32,
    tui_pid: Option<u32>,
    /// A chave que ficou no arquivo na última gravação desta vida: a saída só apaga o arquivo com ela.
    written_key: Option<String>,
}

fn object(value: Value) -> Map<String, Value> {
    match value { Value::Object(map) => map, _ => Map::new() }
}

impl Session {
    pub fn new(dir: PathBuf, plan: &Plan, endpoint: String, key: Option<&str>) -> Self {
        Self {
            dir, name: plan.name.clone(), endpoint, cwd: plan.cwd.clone(), resume: plan.resume.clone(), model: plan.model.clone(),
            effort: plan.effort.clone(), limit: plan.limit, service_tier: plan.service_tier.clone(),
            codex_home: plan.codex_home.to_string_lossy().into_owned(), account: plan.account.clone(), mode: plan.mode,
            imported: plan.imported(), key: key.map(str::to_owned), launcher_pid: std::process::id(), state: Mutex::default(),
        }
    }

    fn state(&self) -> std::sync::MutexGuard<'_, State> { self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner) }

    fn path(&self) -> PathBuf { codex_sidecar::path(&self.dir, &self.name) }

    pub fn set_app_pid(&self, pid: u32) { self.state().app_pid = pid; }

    /// Falha do arquivo da sessão não derruba a sessão: a TUI segue e a linha diz o que não ficou gravado.
    fn report(&self, error: &std::io::Error, screen: &dyn Screen) {
        screen.say(&format!("arquivo da sessão não gravado (codex_sidecar_io: {:?}); a sessão segue", error.kind()));
    }

    /// Fast: na criação, o escolhido; depois, o do arquivo, só se a conversa dele é a atual. Trocar o
    /// Fast depois de abrir não é desfeito pela ressubida.
    pub fn selected_tier(&self) -> Option<String> {
        let previous = self.state().previous.clone();
        if previous.is_none() && self.service_tier.is_some() {
            return self.service_tier.clone();
        }
        let thread = previous.or_else(|| self.resume.clone()).filter(|thread| !thread.is_empty())?;
        let saved = codex_sidecar::load(&self.path())?;
        (saved.get("thread_id").and_then(Value::as_str) == Some(thread.as_str()))
            .then(|| saved.get("service_tier").and_then(Value::as_str).map(str::to_owned)).flatten()
    }

    /// A conversa principal desta vida: grava o arquivo na primeira, troca na `/new`. O app lê daqui o
    /// modelo e o esforço escolhidos, e o servidor se liga pelo endpoint e pelo pid.
    pub fn save_thread(&self, thread_id: &str, rollout: &str, thread: Option<&Value>, screen: &dyn Screen) {
        let mut state = self.state();
        let result = match state.previous.clone() {
            None => self.first_save(&mut state, thread_id, rollout).map(|_| ()),
            Some(previous) if previous != thread_id => match thread {
                Some(thread) => codex_sidecar::switch_thread(&self.dir, &self.name, thread, &previous, &self.endpoint, state.app_pid).map(|_| ()),
                None => Ok(()),
            },
            Some(_) => Ok(()),
        };
        if let Err(error) = result {
            self.report(&error, screen);
        }
        state.previous = Some(thread_id.to_owned());
    }

    fn first_save(&self, state: &mut State, thread_id: &str, rollout: &str) -> std::io::Result<Option<Map<String, Value>>> {
        let existing = codex_sidecar::load(&self.path());
        let same = existing.as_ref().is_some_and(|meta| {
            meta.get("key").is_some_and(hangar_core::pycompat::truthy)
                && meta.get("thread_id").and_then(Value::as_str) == Some(thread_id)
                && self.resume.as_deref() == Some(thread_id)
        });
        let pids = |meta: &mut Map<String, Value>| {
            meta.insert("launcher_pid".into(), json!(self.launcher_pid));
            if let Some(tui) = state.tui_pid { meta.insert("tui_pid".into(), json!(tui)); }
        };
        let written = if same {
            let mut fields = object(json!({"headless": false, "cano": null, "launching": false, "endpoint": self.endpoint,
                "app_pid": state.app_pid, "model": self.model, "effort": self.effort, "rollout_path": rollout,
                "tool_output_token_limit": self.limit}));
            pids(&mut fields);
            if let Some(mode) = self.mode { fields.insert("permission_mode".into(), json!(mode)); }
            if self.imported {
                fields.extend(object(json!({"codex_home": self.codex_home, "codex_account": self.account, "cwd": self.cwd})));
            }
            if let Some(tier) = &self.service_tier { fields.insert("service_tier".into(), json!(tier)); }
            codex_sidecar::update(&self.dir, &self.name, &fields)?
        } else {
            let mut meta = object(json!({"name": self.name, "provider": "codex", "thread_id": thread_id, "rollout_path": rollout,
                "cwd": self.cwd, "model": self.model, "effort": self.effort, "endpoint": self.endpoint, "app_pid": state.app_pid}));
            if let Some(tier) = &self.service_tier { meta.insert("service_tier".into(), json!(tier)); }
            meta.insert("codex_home".into(), json!(self.codex_home));
            meta.insert("codex_account".into(), json!(self.account));
            if let Some(key) = &self.key { meta.insert("key".into(), json!(key)); }
            if let Some(mode) = self.mode { meta.insert("permission_mode".into(), json!(mode)); }
            if let Some(limit) = self.limit { meta.insert("tool_output_token_limit".into(), json!(limit)); }
            pids(&mut meta);
            Some(codex_sidecar::save(&self.dir, &self.name, meta)?)
        };
        if let Some(meta) = &written {
            state.written_key = meta.get("key").and_then(Value::as_str).map(str::to_owned);
        }
        Ok(written)
    }

    /// A TUI subiu: o servidor só dá por pronta a volta ao terminal com o `tui_pid`. Só no arquivo desta
    /// vida; sem ele, a conversa nova o grava junto.
    pub fn tui_started(&self, pid: u32, screen: &dyn Screen) {
        let mut state = self.state();
        state.tui_pid = Some(pid);
        let fields = object(json!({"tui_pid": pid, "launcher_pid": self.launcher_pid}));
        if let Err(error) = codex_sidecar::update_if(&self.dir, &self.name, &[("endpoint", json!(self.endpoint))], &fields) {
            self.report(&error, screen);
        }
    }

    /// O app-server ressubiu na mesma porta: só o pid muda, e só no arquivo desta vida.
    pub fn server_restarted(&self, pid: u32, screen: &dyn Screen) {
        let mut state = self.state();
        let previous = state.app_pid;
        state.app_pid = pid;
        let fields = object(json!({"app_pid": pid}));
        let life = [("endpoint", json!(self.endpoint)), ("app_pid", json!(previous))];
        if let Err(error) = codex_sidecar::update_if(&self.dir, &self.name, &life, &fields) {
            self.report(&error, screen);
        }
    }

    /// Na saída: apaga o arquivo só se ainda é desta vida (endpoint, pid do app-server e chave); uma
    /// sessão recriada com o mesmo nome já o reescreveu.
    pub fn remove(&self, screen: &dyn Screen) {
        let state = self.state();
        let key = state.written_key.clone().or_else(|| self.key.clone());
        if let Err(error) = codex_sidecar::delete_if_life(&self.dir, &self.name, &self.endpoint, state.app_pid, key.as_deref()) {
            screen.say(&format!("sidecar nao removido: {error}"));
        }
    }
}

/// O rollout da conversa pelo id, o mais recente: `<CODEX_HOME>/sessions/*/*/*/rollout-*-<id>.jsonl`.
pub(crate) fn rollout_of(codex_home: &Path, thread: &str) -> Option<PathBuf> {
    let suffix = format!("-{thread}.jsonl");
    let level = |dir: &Path| -> Vec<PathBuf> {
        std::fs::read_dir(dir).into_iter().flatten().flatten()
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .map(|entry| entry.path()).filter(|path| path.is_dir()).collect()
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for year in level(&codex_home.join("sessions")) {
        for month in level(&year) {
            for day in level(&month) {
                for entry in std::fs::read_dir(&day).into_iter().flatten().flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let matches = name.strip_prefix("rollout-").is_some_and(|rest| rest.len() >= suffix.len() && rest.ends_with(&suffix));
                    if matches && let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) {
                        found.push((modified, entry.path()));
                    }
                }
            }
        }
    }
    found.into_iter().max_by_key(|(modified, _)| *modified).map(|(_, path)| path)
}

/// Os processos da vida: o app-server da vez, a cauda dele e as escutas de threads.
#[derive(Default)]
pub(crate) struct Life {
    session: Option<Arc<Session>>,
    codex: PathBuf,
    env: BTreeMap<String, String>,
    diary: Option<Diary>,
    tail: Option<Tail>,
    server: Option<Server>,
    listeners: Vec<tokio::task::JoinHandle<()>>,
}

impl Life {
    pub fn start(&mut self, session: Arc<Session>, codex: PathBuf, env: BTreeMap<String, String>, diary: Diary) {
        self.session = Some(session);
        self.codex = codex;
        self.env = env;
        self.diary = Some(diary);
        self.tail = Some(Tail::new().unwrap_or_else(|_| Tail::none()));
    }

    /// Sobe o app-server com as flags da sessão; o Fast é relido a cada subida. O pid da vida não muda
    /// aqui: na primeira subida quem o grava é o `flow`; na ressubida, só depois de o novo abrir a porta
    /// (`server_restarted`), que confere o arquivo pelo pid anterior.
    pub async fn spawn_server(&mut self, plan: &Plan) -> std::io::Result<()> {
        let (Some(session), Some(tail), Some(diary)) = (self.session.clone(), self.tail.as_mut(), self.diary.as_ref()) else {
            return Err(std::io::Error::other("lançador sem sessão"));
        };
        let tier = session.selected_tier();
        let argv = super::plan::server_argv(&self.codex.to_string_lossy(), plan, &session.endpoint, tier.as_deref(), &self.env);
        match Server::spawn(&argv, &plan.cwd, &self.env, tail) {
            Ok(server) => {
                self.server = Some(server);
                Ok(())
            }
            Err(error) => {
                diary.failure(&app_server::Failure::new("criar_processo", "abrir_app_server").error(&error), tail);
                Err(error)
            }
        }
    }

    /// O pid do app-server de agora.
    pub fn server_pid(&self) -> Option<u32> { self.server.as_ref().map(|server| server.pid) }

    /// Escuta as threads principais que a TUI abre, enquanto este app-server viver.
    pub fn listen(&mut self, cwd: &str) {
        let (Some(session), Some(server)) = (self.session.clone(), self.server.as_ref()) else { return };
        self.listeners.push(tokio::spawn(threads::listen(session, cwd.to_owned(), server.gone.clone())));
    }

    pub async fn wait_port(&mut self, limit: Duration, screen: &dyn Screen) -> bool {
        let (Some(session), Some(server), Some(tail), Some(diary)) = (self.session.as_ref(), self.server.as_mut(), self.tail.as_mut(), self.diary.as_ref()) else {
            return false;
        };
        app_server::wait_port(server, &session.endpoint, limit, tail, diary, screen).await
    }

    fn server_dead(&mut self) -> bool { self.server.as_mut().is_none_or(Server::exited) }

    /// Enquanto a TUI vive: o app-server que cai com ela viva sobe de novo na mesma porta (a TUI
    /// `--remote` desiste de reconectar em ~1 min e não volta). Devolve o código da TUI.
    pub async fn watch(&mut self, tui: &mut tokio::process::Child, plan: &Plan, screen: &dyn Screen) -> i32 {
        let code = |status: std::process::ExitStatus| hangar_core::terminal_process::exit_code(status).unwrap_or(1);
        let mut falls: Vec<Instant> = Vec::new();
        let mut gave_up = false;
        loop {
            match tui.try_wait() {
                Ok(Some(status)) => return code(status),
                Ok(None) => {}
                Err(_) => return 1,
            }
            if !gave_up && self.server_dead() {
                if let Ok(Ok(status)) = tokio::time::timeout(SESSION_END, tui.wait()).await {
                    return code(status);
                }
                let now = Instant::now();
                falls.retain(|at| now.duration_since(*at) < FALL_WINDOW);
                falls.push(now);
                if falls.len() > MAX_FALLS {
                    // A TUI fica: o pane ainda mostra o histórico e este aviso.
                    screen.say("app-server caindo em laço; desisto de ressubir");
                    gave_up = true;
                    continue;
                }
                self.restart(plan, screen).await;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }

    async fn restart(&mut self, plan: &Plan, screen: &dyn Screen) {
        let Some(session) = self.session.clone() else { return };
        if let (Some(server), Some(tail), Some(diary)) = (self.server.as_ref(), self.tail.as_mut(), self.diary.as_ref()) {
            app_server::report_dead(server, &session.endpoint, "executar_app_server", tail, diary, screen);
        }
        if let Err(error) = self.spawn_server(plan).await {
            screen.say(&format!("não deu para subir o app-server do Codex ({:?})", error.kind()));
            return;
        }
        if !self.wait_port(app_server::PORT_WAIT, screen).await {
            // Vivo sem porta enganaria a vigia; morto, conta como queda.
            if let Some(server) = self.server.as_mut() { server.kill().await; }
            return;
        }
        if let Some(server) = self.server.as_ref() {
            session.server_restarted(server.pid, screen);
        }
        // A escuta antiga morreu com o socket: sem uma nova, a sessão que caiu antes da primeira
        // thread nunca ganharia o arquivo.
        self.listen(&plan.cwd);
        screen.say(&format!("app-server ressubido em {}", session.endpoint));
    }

    /// Sempre na saída: derruba o app-server, espera as escutas (senão gravariam o arquivo apontando
    /// para o servidor morto) e apaga o arquivo desta vida.
    pub async fn finish(&mut self, screen: &dyn Screen) {
        let Some(server) = self.server.as_mut() else { return };
        server.stop().await;
        let deadline = Instant::now() + Duration::from_secs(5);
        for mut listener in self.listeners.drain(..) {
            if tokio::time::timeout_at(deadline, &mut listener).await.is_err() {
                listener.abort();
            }
        }
        if let Some(session) = &self.session {
            session.remove(screen);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Lines(std::sync::Mutex<Vec<String>>);
    impl Screen for Lines {
        fn say(&self, line: &str) { self.0.lock().unwrap().push(line.to_owned()); }
    }

    fn plan(name: &str) -> Plan {
        Plan { name: name.into(), cwd: "/w".into(), codex_home: "/h/.codex".into(), account: "default".into(), approval: "never",
            sandbox: "danger-full-access", mode: Some("Full Access"), imported_key: None, resume: None, model: Some("gpt-6".into()),
            effort: Some("high".into()), service_tier: Some("priority".into()), limit: Some(144000), prompt: None, config: vec![] }
    }

    fn thread(id: &str, extra: Value) -> Value {
        let mut thread = json!({"id": id, "path": format!("/h/.codex/sessions/{id}.jsonl"), "cwd": "/w", "source": "cli", "threadSource": "user"});
        for (key, value) in extra.as_object().unwrap() { thread[key] = value.clone(); }
        thread
    }

    fn read(dir: &Path, name: &str) -> Value { Value::Object(codex_sidecar::load(&codex_sidecar::path(dir, name)).unwrap()) }

    /// Conversa nova: o arquivo completo, com a chave do pane, o modo e, novo, `tui_pid` e `launcher_pid`
    /// (a versão anterior só os gravava na retomada). `launcher__new-session-sync-off-full-args`.
    #[test]
    fn a_new_conversation_writes_the_whole_file_with_both_pids() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), &plan("codex-proj"), "ws://127.0.0.1:9".into(), Some("key-0001"));
        session.set_app_pid(40);
        let lines = Lines(Default::default());
        session.tui_started(41, &lines);
        assert!(!codex_sidecar::path(dir.path(), "codex-proj").exists(), "sem arquivo, o pid da TUI espera a conversa");
        session.save_thread("thread-1", "/r/thread-1.jsonl", Some(&thread("thread-1", json!({}))), &lines);
        assert_eq!(read(dir.path(), "codex-proj"), json!({"name": "codex-proj", "provider": "codex", "thread_id": "thread-1",
            "rollout_path": "/r/thread-1.jsonl", "cwd": "/w", "model": "gpt-6", "effort": "high", "endpoint": "ws://127.0.0.1:9", "app_pid": 40,
            "service_tier": "priority", "codex_home": "/h/.codex", "codex_account": "default", "key": "key-0001", "permission_mode": "Full Access",
            "tool_output_token_limit": 144000, "launcher_pid": std::process::id(), "tui_pid": 41}));
        assert!(lines.0.lock().unwrap().is_empty());
        assert_eq!(session.selected_tier().as_deref(), Some("priority"), "a conversa do arquivo é a atual");
    }

    /// `/new` troca a conversa; subagente e outra pasta não. `launcher__new-thread-sequence-new-and-subagent`.
    #[test]
    fn a_new_primary_switches_and_a_subagent_does_not() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), &plan("cx"), "ws://127.0.0.1:9".into(), Some("k"));
        session.set_app_pid(40);
        let lines = Lines(Default::default());
        session.save_thread("t1", "/r/t1", Some(&thread("t1", json!({}))), &lines);
        session.save_thread("t2", "/h/.codex/sessions/t2.jsonl", Some(&thread("t2", json!({}))), &lines);
        let meta = read(dir.path(), "cx");
        assert_eq!(meta["thread_id"], "t2");
        assert!(meta.get("service_tier").is_none() && meta.get("tool_output_token_limit").is_none(), "eram da conversa anterior");
        // O subagente chega ao lançador só se for principal; a regra do arquivo recusa os outros.
        session.save_thread("t3", "/r/t3", Some(&thread("t3", json!({"parentThreadId": "t2"}))), &lines);
        session.save_thread("t4", "/r/t4", Some(&thread("t4", json!({"cwd": "/outra"}))), &lines);
        assert_eq!(read(dir.path(), "cx")["thread_id"], "t2");
    }

    /// A saída apaga o arquivo só com endpoint, pid e chave desta vida. O lançador anterior conferia
    /// só o pid: um arquivo com o mesmo pid e outra chave (outra sessão de mesmo nome) era apagado.
    #[test]
    fn the_exit_removes_only_the_file_of_this_life() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), &plan("cx"), "ws://127.0.0.1:9".into(), Some("k"));
        session.set_app_pid(40);
        let lines = Lines(Default::default());
        session.save_thread("t1", "/r/t1", None, &lines);
        let mut other = codex_sidecar::load(&codex_sidecar::path(dir.path(), "cx")).unwrap();
        other.insert("key".into(), json!("outra"));
        codex_sidecar::put(dir.path(), "cx", &other).unwrap();
        session.remove(&lines);
        assert_eq!(read(dir.path(), "cx")["key"], "outra", "mesmo pid, outra chave: fica");
        other.insert("key".into(), json!("k"));
        codex_sidecar::put(dir.path(), "cx", &other).unwrap();
        session.remove(&lines);
        assert!(!codex_sidecar::path(dir.path(), "cx").exists());
        assert!(lines.0.lock().unwrap().is_empty());
    }

    /// Retomada da mesma conversa com chave: mescla no arquivo que existe, com o modo e os pids.
    #[test]
    fn resuming_the_same_conversation_merges_into_the_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        codex_sidecar::put(dir.path(), "cx", &object(json!({"name": "cx", "thread_id": "t-r", "key": "k", "transfer_id": "tr",
            "headless": true, "cano": {"pid": 3}, "launching": true, "cwd": "/w", "service_tier": "default"}))).unwrap();
        let mut resumed = plan("cx");
        (resumed.resume, resumed.service_tier, resumed.limit) = (Some("t-r".into()), None, None);
        let session = Session::new(dir.path().to_path_buf(), &resumed, "ws://127.0.0.1:9".into(), Some("k"));
        session.set_app_pid(40);
        let lines = Lines(Default::default());
        session.save_thread("t-r", "/r/t-r", None, &lines);
        let meta = read(dir.path(), "cx");
        for (field, value) in [("headless", json!(false)), ("cano", json!(null)), ("launching", json!(false)), ("app_pid", json!(40)),
            ("rollout_path", json!("/r/t-r")), ("transfer_id", json!("tr")), ("service_tier", json!("default")),
            ("tool_output_token_limit", json!(null)), ("permission_mode", json!("Full Access")), ("launcher_pid", json!(std::process::id()))] {
            assert_eq!(meta[field], value, "{field}");
        }
        session.tui_started(41, &lines);
        assert_eq!(read(dir.path(), "cx")["tui_pid"], 41);
        assert_eq!(session.selected_tier().as_deref(), Some("default"), "o Fast trocado depois de abrir fica");
        session.server_restarted(42, &lines);
        assert_eq!(read(dir.path(), "cx")["app_pid"], 42);
    }

    /// Queda depois da primeira gravação: o arquivo passa ao pid novo, a `/new` seguinte troca a conversa
    /// por ele, e a saída apaga o arquivo. Antes, o pid novo era anotado antes de conferir o arquivo, e
    /// nada disso acontecia (arquivo órfão apontando para o app-server morto).
    #[test]
    fn after_a_fall_the_file_follows_the_new_app_server() {
        let dir = tempfile::tempdir().unwrap();
        let session = Session::new(dir.path().to_path_buf(), &plan("cx"), "ws://127.0.0.1:9".into(), Some("k"));
        session.set_app_pid(40);
        let lines = Lines(Default::default());
        session.save_thread("t1", "/r/t1", Some(&thread("t1", json!({}))), &lines);
        session.server_restarted(41, &lines);
        assert_eq!(read(dir.path(), "cx")["app_pid"], 41);
        session.save_thread("t2", "/h/.codex/sessions/t2.jsonl", Some(&thread("t2", json!({}))), &lines);
        assert_eq!(read(dir.path(), "cx")["thread_id"], "t2", "a `/new` depois da queda");
        session.remove(&lines);
        assert!(!codex_sidecar::path(dir.path(), "cx").exists());
        assert!(lines.0.lock().unwrap().is_empty());
    }

    /// `~/.hangar` sem escrita não derruba o lançador depois da TUI: a linha diz o código e a sessão segue.
    /// O lançador anterior saía com 1 e matava o app-server (`launcher__update-hangar-dir-not-writable`).
    #[cfg(unix)]
    #[test]
    fn an_unwritable_hangar_folder_is_reported_and_the_session_goes_on() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let hangar = home.path().join(".hangar");
        std::fs::create_dir_all(&hangar).unwrap();
        std::fs::set_permissions(&hangar, std::fs::Permissions::from_mode(0o555)).unwrap();
        if std::fs::create_dir(hangar.join("probe")).is_ok() { return; } // root escreve em qualquer lugar
        let session = Session::new(codex_sidecar::dir(home.path()), &plan("cx"), "ws://127.0.0.1:9".into(), Some("k"));
        let lines = Lines(Default::default());
        session.tui_started(41, &lines);
        session.save_thread("t1", "/r", None, &lines);
        session.remove(&lines);
        let lines = lines.0.lock().unwrap();
        assert_eq!(lines[0], "arquivo da sessão não gravado (codex_sidecar_io: PermissionDenied); a sessão segue");
        assert_eq!(lines.len(), 3, "{lines:?}");
        std::fs::set_permissions(&hangar, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    #[test]
    fn the_rollout_is_the_newest_file_of_the_conversation() {
        let home = tempfile::tempdir().unwrap();
        let day = home.path().join("sessions/2026/11/17");
        std::fs::create_dir_all(&day).unwrap();
        assert_eq!(rollout_of(home.path(), "thread-r"), None);
        std::fs::write(day.join("rollout-2026-11-17T10-00-00-thread-r.jsonl"), "{}\n").unwrap();
        std::fs::write(day.join("rollout-2026-11-17T10-00-00-other-thread-r2.jsonl"), "{}\n").unwrap();
        std::fs::write(day.join("x-thread-r.jsonl"), "{}\n").unwrap();
        assert_eq!(rollout_of(home.path(), "thread-r"), Some(day.join("rollout-2026-11-17T10-00-00-thread-r.jsonl")));
        assert_eq!(rollout_of(home.path(), "missing"), None);
    }
}
