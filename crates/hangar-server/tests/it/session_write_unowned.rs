//! Texto para sessão que o Rust não adotou (R1.1/R1.2), num tmux isolado com panes reais: nome sem
//! sessão, shell, `claude login` e o Codex cru (TUI falsa que repinta capturas reais do codex-cli 0.167.0,
//! `fixtures/raw_codex/`). O esperado vem das capturas do Python (`contract/r1-input/`), com as
//! diferenças aprovadas (D7, D15). Nada chega ao Python.
#![cfg(unix)]
use crate::fake;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use fake::{OWNER, SECRET, client};
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::AppState;
use hangar_server::runtime::gateway::RuntimeRegistry;
use serde_json::{Value, json};

const MSG_UNBOUND: &str = "vínculo gerenciado indisponível; escrita suspensa";

pub(crate) struct Rig {
    pub dir: tempfile::TempDir,
    label: String,
    pub home: PathBuf,
    pub python: Arc<fake::Fake>,
    pub server: SocketAddr,
    /// A porta privada (as pontes que o Python chama).
    pub private: SocketAddr,
    pub runtime: Arc<RuntimeRegistry>,
    pub state: Arc<AppState>,
}

impl Drop for Rig {
    fn drop(&mut self) { let _ = Command::new("tmux").args(["-L", &self.label, "kill-server"]).output(); }
}

/// Sem o ambiente do Hangar de quem roda os testes: `CP_PROVIDER=claude` herdado faria o pane de shell
/// parecer um Claude nascendo.
fn isolated(command: &mut Command) -> &mut Command {
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(|k| k.starts_with("CP_") || k.starts_with("HANGAR_")) { command.env_remove(key); }
    }
    command
}

fn fixture(name: &str) -> PathBuf { Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/it/fixtures/raw_codex").join(name) }

impl Rig {
    /// Servidor com o tmux isolado na lista, nas escritas e na captura; sem `runtime`, o estado fica sem o
    /// registro (como no teste antigo `input.rs:448`).
    pub(crate) async fn build(with_runtime: bool) -> Rig {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let dir = tempfile::tempdir().unwrap();
        let label = format!("hangar-unowned-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed));
        let root = dir.path().to_path_buf();
        // O multiplexador da lista e das escritas: o tmux isolado, com falhas ligadas por arquivo.
        let wrapper = root.join("tmux");
        crate::write_executable(&wrapper, format!(r#"#!/bin/sh
case " $* " in *" send-keys "*" -l "*) [ -e '{root}/fail-literal' ] && exit 1;; esac
case "$1" in load-buffer|paste-buffer|set-buffer) [ -e '{root}/fail-buffer' ] && exit 1;; esac
[ "$1" = send-keys ] && [ -e '{root}/slow-keys' ] && grep -qx -- "$3" '{root}/slow-keys' && sleep 2
exec tmux -L '{label}' -f /dev/null "$@"
"#, root = root.display()));
        let rig_label = label.clone();
        let run = move |args: &[&str]| {
            let out = isolated(Command::new("tmux").args(["-L", &rig_label, "-f", "/dev/null"]).args(args)).output().unwrap();
            assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8(out.stdout).unwrap()
        };
        run(&["new-session", "-d", "-s", "keep", "-x", "160", "-y", "45", "cat"]);
        let socket = PathBuf::from(run(&["display-message", "-p", "#{socket_path}"]).trim());
        let home = root.join("home");
        std::fs::create_dir_all(home.join(".claude")).unwrap();
        let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
            "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
            "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
        let (python, upstream) = fake::spawn_fake().await;
        let mut state = AppState::new(fake::config(upstream, "127.0.0.1"));
        state.diag = python.bind_diary(state.diag.clone());
        state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&wrapper, Duration::from_secs(5)),
            capture_program: wrapper.clone().into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
            FactsClient::new(upstream, SECRET.into())));
        state.terminal = hangar_server::terminal_control::TerminalPool::with_program("tmux", Some(socket), hangar_server::terminal_control::Limits::default());
        let env = hangar_server::accounts::environment::AccountEnvironment::from_map([("HOME".into(), home.to_string_lossy().into_owned())].into());
        let runtime = Arc::new(RuntimeRegistry::new(upstream, SECRET.into(), "instance-test".into()).with_codex_environment(env).with_diagnostics(python.diagnostics()));
        runtime.bind_watcher(Arc::new(hangar_server::runtime::codex_terminal::Watcher::new(hangar_server::runtime::codex_terminal::WatchConfig {
            home: home.clone(), queue_dir: home.join(".claude/.hangar-queue"), mux: Mux::with_program(&wrapper, Duration::from_secs(5)),
            mux_program: wrapper.clone().into_os_string() })));
        if with_runtime { assert!(state.state.runtime.set(runtime.clone()).is_ok()); }
        state.side.locate = fake::fake_locate(upstream);
        let pair = home.join(".claude/.hangar-pair");
        state.groups = Some(Arc::new(hangar_server::groups::service::GroupService::new(hangar_server::groups::store::PairDir::new(pair.clone(), root.join("arquivo")),
            Arc::new(hangar_server::groups::orq::PythonOrq::from_state(&state).with_paths(home.clone(), pair)), "rig".into())));
        let state = Arc::new(state);
        let serve = |app: axum::Router| async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
            addr
        };
        let server = serve(hangar_server::routes::router(state.clone())).await;
        let private = serve(hangar_server::routes::terminal_router(state.clone())).await;
        Rig { dir, label, home, python, server, private, runtime, state }
    }

    pub(crate) async fn new() -> Rig { Rig::build(true).await }

    pub(crate) fn tmux(&self, args: &[&str]) -> String {
        let out = isolated(Command::new("tmux").args(["-L", &self.label]).args(args)).output().unwrap();
        assert!(out.status.success(), "{args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    }

    pub(crate) fn pane(&self, name: &str, command: &str) {
        let cwd = self.dir.path().join("w");
        std::fs::create_dir_all(&cwd).unwrap();
        self.tmux(&["new-session", "-d", "-s", name, "-x", "160", "-y", "45", "-c", cwd.to_str().unwrap(), command]);
    }

    /// Cada `send-keys` ao pane da sessão demora 2 s.
    pub(crate) fn slow(&self, name: &str) {
        let pane = self.tmux(&["display-message", "-p", "-t", &format!("={name}:"), "#{pane_id}"]);
        let path = self.dir.path().join("slow-keys");
        let mut list = std::fs::read_to_string(&path).unwrap_or_default();
        list.push_str(&pane);
        std::fs::write(path, list).unwrap();
    }

    pub(crate) fn flag(&self, name: &str, on: bool) {
        let path = self.dir.path().join(name);
        if on { std::fs::write(path, "").unwrap(); } else { let _ = std::fs::remove_file(path); }
    }

    /// Pane de shell com o filho `codex` (como `command codex` digitado), pintando a captura `screen`.
    /// `echo`: o composer `› ` mostra o que chega e o Enter o envia; `static`: a tela nunca muda.
    pub(crate) async fn codex(&self, name: &str, screen: &str, mode: &str) {
        let log = self.dir.path().join(format!("codex-{name}"));
        std::fs::create_dir_all(&log).unwrap();
        self.pane(name, &format!("bash --norc --noprofile -c '( exec -a codex python3 {} {} {} {mode} ); exec sleep 100000'",
            fixture("fake_codex.py").display(), fixture(screen).display(), log.display()));
        let first = std::fs::read_to_string(fixture(screen)).unwrap().lines().find(|l| !l.trim().is_empty()).unwrap().trim().to_owned();
        self.screen_shows(name, &first).await;
    }

    pub(crate) async fn screen_shows(&self, name: &str, needle: &str) {
        for _ in 0..200 {
            if self.tmux(&["capture-pane", "-p", "-t", &format!("={name}:")]).contains(needle) { return; }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("a tela de {name} não mostrou {needle:?}: {}", self.tmux(&["capture-pane", "-p", "-t", &format!("={name}:")]));
    }

    /// O que a TUI falsa leu do tty (`keys`) e o que cada Enter enviou (`submitted`).
    pub(crate) fn tui(&self, name: &str, file: &str) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join(format!("codex-{name}")).join(file)).unwrap_or_default()
            .lines().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    pub(crate) fn queue(&self, name: &str) -> Option<Vec<Value>> {
        let path = self.home.join(".claude/.hangar-queue").join(format!("{name}.jsonl"));
        std::fs::read_to_string(path).ok().map(|raw| raw.lines().map(|l| serde_json::from_str(l).unwrap()).collect())
    }

    pub(crate) async fn post(&self, path: &str, body: &str) -> (u16, Value) {
        let response = client().post(format!("http://{}{path}", self.server)).header("content-type", "application/json")
            .bearer_auth(OWNER).body(body.to_owned()).send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }

    pub(crate) async fn input(&self, name: &str, body: Value) -> (u16, Value) { self.post(&format!("/api/sessions/{name}/input"), &body.to_string()).await }

    fn python_inputs(&self) -> usize { self.python.input_calls() + self.python.hits_to("/api/broadcast") }
}

fn submitted(rig: &Rig, name: &str) -> Vec<Value> { rig.tui(name, "submitted.jsonl") }

// R1.1-1
#[tokio::test]
async fn name_without_pane_or_sidecar_is_404_and_nothing_is_queued() {
    let rig = Rig::new().await;
    let (status, body) = rig.input("ghost", json!({"text": "oi"})).await;
    assert_eq!((status, body), (404, json!({"detail": {"code": "erro_sessao_recado_nao_enfileirado", "params": {}, "msg": "sessão não encontrada — recado NÃO enfileirado"}})));
    assert_eq!(rig.queue("ghost"), None);
    assert_eq!(rig.python_inputs(), 0);
}

// R1.1-2 + D15: shell e programa qualquer recusam na hora, mesmo com o pane recém-criado.
#[tokio::test]
async fn shell_or_unknown_program_pane_is_400_at_once_and_nothing_is_typed() {
    let rig = Rig::new().await;
    rig.pane("sh1", "bash --norc --noprofile");
    rig.pane("cat1", "cat");
    for name in ["sh1", "cat1"] {
        let start = Instant::now();
        let (status, body) = rig.input(name, json!({"text": "oi"})).await;
        assert!(start.elapsed() < Duration::from_secs(5), "D15: sem a espera de nascimento ({:?})", start.elapsed());
        assert_eq!((status, body), (400, json!({"detail": {"code": "erro_envio_falhou", "params": {"erro": MSG_UNBOUND}, "msg": MSG_UNBOUND}})), "{name}");
        assert!(!rig.tmux(&["capture-pane", "-p", "-t", &format!("={name}:")]).contains("oi"), "{name}: nada digitado");
        assert_eq!(rig.queue(name), None);
    }
    assert_eq!(rig.python_inputs(), 0);
}

// R1.1-3
#[tokio::test]
async fn claude_login_pane_is_400_without_rust_binding() {
    let rig = Rig::new().await;
    let cwd = rig.dir.path().join("w");
    std::fs::create_dir_all(&cwd).unwrap();
    assert!(Command::new("mkfifo").arg(cwd.join("login")).status().unwrap().success());
    // `claude login` = um `cat` lendo o FIFO ./login, preso para sempre (como na captura).
    rig.pane("lg1", "bash --norc --noprofile -c '( exec -a claude cat login ); exec sleep 100000'");
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (status, body) = rig.input("lg1", json!({"text": "oi"})).await;
    assert_eq!((status, body), (400, json!({"detail": {"code": "erro_envio_falhou", "params": {}, "msg": "sessão Claude sem vínculo no Rust"}})));
    assert_eq!(rig.python_inputs(), 0);
}

// R1.1-4, R1.1-5, R1.1-12: texto digitado com prova, Enter cru, linha `delivered` na fila com o `ts` de antes.
#[tokio::test]
async fn raw_codex_idle_types_the_text_and_queues_it_delivered() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    let before = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64();
    let (status, body) = rig.input("cx", json!({"text": "olá mundo"})).await;
    assert_eq!((status, body), (200, json!({"ok": true, "delivered": true, "steered": false, "native": false})));
    assert_eq!(submitted(&rig, "cx"), vec![json!({"text": "olá mundo"})]);
    assert_eq!(rig.tui("cx", "keys.jsonl").last(), Some(&json!({"read": "\r"})), "Enter como CR cru");
    let rows = rig.queue("cx").unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!((rows[0]["text"].as_str(), rows[0]["delivered"].as_bool(), rows[0]["id"].as_str().map(str::len)), (Some("olá mundo"), Some(true), Some(32)));
    assert!(rows[0]["ts"].as_f64().unwrap() >= before);
    // `steer` não muda nada no pane: `steered` sempre falso.
    let (status, body) = rig.input("cx", json!({"text": "de novo", "steer": true})).await;
    assert_eq!((status, body), (200, json!({"ok": true, "delivered": true, "steered": false, "native": false})));
    assert_eq!(rig.queue("cx").unwrap().len(), 2);
    assert_eq!(rig.python_inputs(), 0);
}

// R1.1-6: sem confirmação nem redreno depois da resposta (o Codex cru não tem transcript).
#[tokio::test]
async fn raw_codex_nothing_more_happens_after_the_answer() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    assert_eq!(rig.input("cx", json!({"text": "oi"})).await.0, 200);
    let (keys, rows) = (rig.tui("cx", "keys.jsonl"), rig.queue("cx"));
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!((rig.tui("cx", "keys.jsonl"), rig.queue("cx")), (keys, rows));
}

// R1.1-4 slash, R1.1-7: barra com Enter duplo e sem linha; `/clear` apaga a fila.
#[tokio::test]
async fn raw_codex_slash_gets_two_enters_and_clear_drops_the_queue() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    let (status, body) = rig.input("cx", json!({"text": "/model"})).await;
    assert_eq!((status, body["delivered"].clone()), (200, json!(true)));
    assert_eq!(submitted(&rig, "cx"), vec![json!({"text": "/model"}), json!({"text": ""})]);
    assert_eq!(rig.queue("cx"), None);
    assert_eq!(rig.input("cx", json!({"text": "fica na fila"})).await.0, 200);
    assert!(rig.queue("cx").is_some());
    let (status, _) = rig.input("cx", json!({"text": "/clear"})).await;
    assert_eq!(status, 200);
    assert_eq!(rig.queue("cx"), None, "o /clear leva a fila");
    assert!(!rig.home.join(".claude/.hangar-queue/cx.jsonl.tmp").exists());
}

// R1.1-4 multilinha: colagem com bracketed paste; sem buffer, linha a linha com C-j.
#[tokio::test]
async fn raw_codex_multiline_pastes_and_falls_back_to_ctrl_j() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    assert_eq!(rig.input("cx", json!({"text": "linha 1\nlinha 2"})).await.0, 200);
    assert_eq!(rig.tui("cx", "keys.jsonl")[0], json!({"read": "\u{1b}[200~linha 1\rlinha 2\u{1b}[201~"}));
    rig.flag("fail-buffer", true);
    let (status, body) = rig.input("cx", json!({"text": "linha 3\nlinha 4"})).await;
    assert_eq!((status, body["delivered"].clone()), (200, json!(true)));
    let keys: String = rig.tui("cx", "keys.jsonl").iter().skip(2).map(|k| k["read"].as_str().unwrap().to_owned()).collect();
    assert_eq!(keys, "linha 3\nlinha 4\r");
    assert_eq!(submitted(&rig, "cx"), vec![json!({"text": "linha 1\nlinha 2"}), json!({"text": "linha 3\nlinha 4"})]);
}

// R1.1-4: controle recusado antes de tocar o pane ou a fila.
#[tokio::test]
async fn raw_codex_control_character_is_400_with_nothing_typed_or_queued() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    let (status, body) = rig.input("cx", json!({"text": "a\u{7}b"})).await;
    assert_eq!((status, body), (400, json!({"detail": {"code": "erro_envio_falhou", "params": {"erro": "control characters not allowed in prompt"},
        "msg": "falha ao enviar: control characters not allowed in prompt"}})));
    assert_eq!((rig.tui("cx", "keys.jsonl"), rig.queue("cx")), (vec![], None));
}

// R1.1-4 parcial: `send-keys -l` recusado = nada submetido, sem linha na fila.
#[tokio::test]
async fn raw_codex_refused_literal_is_an_incomplete_send_without_enter() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    rig.flag("fail-literal", true);
    let (status, body) = rig.input("cx", json!({"text": "oi"})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (400, json!("erro_envio_incompleto_composer")));
    assert_eq!((submitted(&rig, "cx"), rig.queue("cx")), (vec![], None));
}

// R1.1-5: digitado e a fila sem gravar = entregue, sem linha (só o log).
#[tokio::test]
async fn raw_codex_typed_with_unwritable_queue_is_still_delivered() {
    use std::os::unix::fs::PermissionsExt;
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    let queue = rig.home.join(".claude/.hangar-queue");
    std::fs::create_dir_all(&queue).unwrap();
    std::fs::set_permissions(&queue, std::fs::Permissions::from_mode(0o500)).unwrap();
    let (status, body) = rig.input("cx", json!({"text": "oi"})).await;
    std::fs::set_permissions(&queue, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!((status, body["delivered"].clone()), (200, json!(true)));
    assert_eq!((submitted(&rig, "cx"), rig.queue("cx")), (vec![json!({"text": "oi"})], None));
}

// R1.1-9: o cliente que desiste no meio não desfaz a digitação nem a linha da fila.
#[tokio::test]
async fn raw_codex_send_survives_the_client_going_away() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    rig.slow("cx");
    let request = client().post(format!("http://{}/api/sessions/cx/input", rig.server)).bearer_auth(OWNER)
        .header("content-type", "application/json").body(json!({"text": "oi"}).to_string()).timeout(Duration::from_millis(1500)).send().await;
    assert!(request.is_err(), "o cliente desistiu antes da resposta");
    for _ in 0..200 {
        if rig.queue("cx").is_some() { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!((submitted(&rig, "cx"), rig.queue("cx").map(|rows| rows[0]["delivered"].clone())), (vec![json!({"text": "oi"})], Some(json!(true))));
}

// A digitação na sessão sem dono não segura o passe da porta: quem fecha a porta (troca de agente,
// renomear) não espera por ela.
#[tokio::test]
async fn raw_codex_send_never_holds_the_ingress_pass() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    rig.slow("cx");
    let url = format!("http://{}/api/sessions/cx/input", rig.server);
    let sending = tokio::spawn(client().post(url).bearer_auth(OWNER).header("content-type", "application/json").body(r#"{"text":"oi"}"#).send());
    for _ in 0..200 {
        if !rig.tui("cx", "keys.jsonl").is_empty() || rig.tmux(&["capture-pane", "-p", "-t", "=cx:"]).contains("› oi") { break; }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(rig.runtime.ingress().close("cx", Duration::from_millis(300)).await.is_ok(), "passe na mão durante a digitação");
    rig.runtime.ingress().open("cx");
    assert_eq!(sending.await.unwrap().unwrap().status().as_u16(), 200);
}

// R1.1-8: troca de agente em curso fecha a porta: 409 na hora, nada digitado.
#[tokio::test]
async fn transfer_in_progress_answers_busy_without_typing() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    rig.runtime.ingress().hold("cx", Duration::from_secs(1)).await.unwrap();
    let (status, body) = rig.input("cx", json!({"text": "oi"})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (409, json!("session_transfer_busy")));
    assert_eq!(rig.tui("cx", "keys.jsonl"), Vec::<Value>::new());
}

// D7: corpo fora de forma é 422 com código, não o do FastAPI.
#[tokio::test]
async fn invalid_body_for_an_unowned_name_is_rust_422() {
    let rig = Rig::new().await;
    for body in [r#"{"txt":1}"#, "oi"] {
        let (status, reply) = rig.post("/api/sessions/cx/input", body).await;
        assert_eq!((status, reply["detail"]["code"].clone()), (422, json!("erro_corpo_invalido")), "{body}");
    }
    assert_eq!(rig.python_inputs(), 0);
}

// R1.1-11, R1.2: um nome por vez, falha de um não para os outros, formato do Python por tipo.
#[tokio::test]
async fn broadcast_answers_each_unowned_name_in_order() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    rig.pane("sh1", "bash --norc --noprofile");
    let (status, body) = rig.post("/api/broadcast", &json!({"names": ["cx", "ghost", "sh1"], "text": "oi todos"}).to_string()).await;
    assert_eq!(status, 200);
    assert_eq!(body, json!({"results": {
        "cx": {"ok": true, "error": null, "delivered": true, "native": false},
        "ghost": {"ok": false, "error": {"code": "erro_sessao_inexistente", "params": {}, "msg": "sessão não encontrada"}, "delivered": false},
        "sh1": {"ok": false, "error": {"code": "erro_envio_falhou", "params": {"erro": MSG_UNBOUND}, "msg": MSG_UNBOUND}}}}));
    assert_eq!(submitted(&rig, "cx"), vec![json!({"text": "oi todos"})]);
    let (_, body) = rig.post("/api/broadcast", &json!({"names": ["cx"], "text": "a\u{7}b"}).to_string()).await;
    assert_eq!(body["results"]["cx"], json!({"ok": false, "error": {"code": "erro_envio_falhou", "params": {"erro": "control characters not allowed in prompt"},
        "msg": "falha ao enviar: control characters not allowed in prompt"}}));
    assert_eq!(rig.python_inputs(), 0);
}

// R1.1-4 (paridade do Python): tela que nunca fica pronta espera 12 s e digita assim mesmo.
#[tokio::test]
async fn raw_codex_screen_never_ready_types_after_the_readiness_wait() {
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_trust_dialog.txt", "static").await;
    let start = Instant::now();
    let (status, body) = rig.input("cx", json!({"text": "oi"})).await;
    assert!(start.elapsed() >= Duration::from_secs(12), "{:?}", start.elapsed());
    assert_eq!((status, body["delivered"].clone()), (200, json!(true)));
    assert_eq!(submitted(&rig, "cx"), vec![json!({"enter_on_static_screen": "pane_codex_trust_dialog.txt"})]);
}

// R1.1 por dentro (aviso de grupo, par, MCP…): o mesmo envio, sem ida ao Python.
#[tokio::test]
async fn internal_delivery_to_an_unowned_session_is_served_in_rust() {
    use hangar_server::session_write::input::{deliver_text, write_input};
    let rig = Rig::new().await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    rig.pane("sh1", "bash --norc --noprofile");
    assert_eq!(deliver_text(&rig.state, "cx", "aviso").await, Ok(()));
    assert_eq!(submitted(&rig, "cx"), vec![json!({"text": "aviso"})]);
    assert_eq!(deliver_text(&rig.state, "ghost", "aviso").await,
        Err(json!({"code": "erro_sessao_recado_nao_enfileirado", "params": {}, "msg": "sessão não encontrada — recado NÃO enfileirado"})));
    assert_eq!(deliver_text(&rig.state, "sh1", "aviso").await, Err(json!({"code": "erro_envio_falhou", "params": {"erro": MSG_UNBOUND}, "msg": MSG_UNBOUND})));
    let (status, body) = write_input(&rig.state, "cx", "com steer", true).await;
    assert_eq!((status.as_u16(), body), (200, json!({"ok": true, "delivered": true, "steered": false, "native": false})));
    assert_eq!(rig.python_inputs(), 0);
}

// R1.1-10: aviso que entra pela porta do Connect (o Python repassa e segura o pedido). O recado a
// sessão sem dono sai depois da rota, em processo, cada sessão no máximo 3 s; quem não recebeu (recusa ou
// prazo) entra em `failed` como `srv::nome`, e o diário registra. A entrega lenta termina sozinha depois.
#[tokio::test(flavor = "multi_thread")]
async fn connect_forwarded_notice_delivers_unowned_sessions_after_the_route_within_three_seconds() {
    let rig = Rig::new().await;
    rig.codex("lenta", "pane_codex_idle.txt", "echo").await;
    rig.slow("lenta");
    let pair = rig.home.join(".claude/.hangar-pair");
    std::fs::create_dir_all(&pair).unwrap();
    for name in ["lenta", "ghost"] {
        std::fs::write(pair.join(format!("{name}.json")), json!({"peers": ["casa::s0"], "task": "t", "gid": "g1", "harness": {},
            "fed": {"owner": "casa", "local": false, "version": 1}}).to_string()).unwrap();
    }
    let ask = json!({"protocol": 1, "origin": "casa", "sender": "s0", "text": "oi", "notice": "n1"});
    let start = Instant::now();
    let resp = client().post(format!("http://{}/__hangar_server/forwarded", rig.private)).header("x-hangar-path", "/api/groups/g1/deliver")
        .header("x-hangar-internal", SECRET).header("content-type", "application/json").body(ask.to_string()).send().await.unwrap();
    let elapsed = start.elapsed();
    assert_eq!(resp.status().as_u16(), 200);
    let got: Value = serde_json::from_str(&resp.text().await.unwrap()).unwrap();
    let mut failed: Vec<String> = got["failed"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_owned()).collect();
    failed.sort();
    assert_eq!(failed, ["rig::ghost", "rig::lenta"], "{got}");
    assert!(got.get("hangar_deliveries").is_none(), "{got}");
    assert!(elapsed >= Duration::from_secs(3) && elapsed < Duration::from_secs(8), "prazo de 3 s por sessão: {elapsed:?}");
    let diary = fake::wait_diary(rig.python.diary_dir(), |row| row["evento"] == "grupos.entrega_adiada_falhou").await;
    assert!(diary.iter().any(|row| row["evento"] == "grupos.entrega_adiada_falhou"), "{diary:?}");
    for _ in 0..200 {
        if !submitted(&rig, "lenta").is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(submitted(&rig, "lenta"), vec![json!({"text": "[grupo: casa::s0] oi"})], "a entrega largada pelo prazo termina");
    assert_eq!(rig.python_inputs(), 0);
}

// `input.rs:448`: sem o registro do runtime, o envio também é do Rust.
#[tokio::test]
async fn internal_delivery_without_runtime_is_served_in_rust() {
    let rig = Rig::build(false).await;
    rig.codex("cx", "pane_codex_idle.txt", "echo").await;
    assert_eq!(hangar_server::session_write::input::deliver_text(&rig.state, "cx", "aviso").await, Ok(()));
    assert_eq!(submitted(&rig, "cx"), vec![json!({"text": "aviso"})]);
    let err = hangar_server::session_write::input::deliver_text(&rig.state, "ghost", "aviso").await.unwrap_err();
    assert_eq!(err["code"], "erro_sessao_recado_nao_enfileirado");
    assert_eq!(rig.python_inputs(), 0);
}
