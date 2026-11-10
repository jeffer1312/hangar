//! R1.3: o `DELETE` do dono de uma sessão sem vida aberta no Rust (pane de outro programa, Codex e Claude
//! sem terminal parados, Claude cujo agente morreu, sessão que já acabou) fecha no Rust, pelo nome, e só
//! depois segue marcado ao Python, para as lojas dele. O esperado vem das capturas de
//! `contract/r1-delete/` (commit 43b407cc1) e das decisões D16/D17.
#![cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::AppState;
use hangar_server::runtime::codex_terminal::{WatchConfig, Watcher};
use hangar_server::runtime::gateway::RuntimeRegistry;
use serde_json::{Value, json};

use crate::fake::{self, Fake, OWNER, SECRET};

/// Multiplexador falso: `sessions` tem uma sessão por linha, `hidden` as marcadas `@cp_hidden`, `survive`
/// faz o `kill-session` responder 0 sem derrubar (o psmux), e `calls` registra cada comando.
struct Env {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    queue: PathBuf,
    tmux: PathBuf,
    python: Arc<Fake>,
    server: std::net::SocketAddr,
    registry: Arc<RuntimeRegistry>,
    share: Arc<hangar_server::share::ShareState>,
    upstream: std::net::SocketAddr,
}

impl Env {
    /// A porta privada das pontes que o Python chama, sobre o mesmo registro.
    async fn private(&self) -> std::net::SocketAddr {
        let mut state = AppState::new(fake::config(self.upstream, "127.0.0.1"));
        assert!(state.state.runtime.set(self.registry.clone()).is_ok());
        state.share = Some(self.share.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let router = hangar_server::routes::terminal_router(Arc::new(state));
        tokio::spawn(async move { axum::serve(listener, router.into_make_service_with_connect_info::<std::net::SocketAddr>()).await.unwrap() });
        address
    }
    fn sessions(&self, names: &[&str]) {
        std::fs::write(self.root.join("sessions"), names.iter().map(|n| format!("{n}\n")).collect::<String>()).unwrap();
    }
    fn hidden(&self, names: &[&str]) {
        std::fs::write(self.root.join("hidden"), names.iter().map(|n| format!("{n}\n")).collect::<String>()).unwrap();
    }
    fn alive(&self) -> Vec<String> {
        std::fs::read_to_string(self.root.join("sessions")).unwrap_or_default().lines().map(str::to_owned).collect()
    }
    fn calls(&self, verb: &str) -> Vec<String> {
        std::fs::read_to_string(self.root.join("calls")).unwrap_or_default().lines().filter(|l| l.starts_with(verb)).map(str::to_owned).collect()
    }
    /// Convite na vida que a varredura dos convites vê (a vida falsa: todo `s*` nasceu em 1); outra vida
    /// seria revogada sozinha.
    async fn invite(&self, name: &str) -> (hangar_server::share::store::Share, String) {
        let life = self.share.lives.life(name, true).await;
        self.share.store.create(name, life, hangar_server::share::store::Kind::Share, true).unwrap()
    }
    fn restore_path(&self) -> PathBuf { self.home.join(".hangar/session-restore.json") }
    /// A sessão na fotografia que o reboot reabre.
    fn restore_entry(&self, name: &str) {
        std::fs::create_dir_all(self.restore_path().parent().unwrap()).unwrap();
        std::fs::write(self.restore_path(), json!({"sessions": {format!("k-{name}"): {"name": name, "provider": "claude", "conversation": null}}}).to_string()).unwrap();
    }
    fn restore_names(&self) -> Vec<String> {
        let snap: Value = serde_json::from_slice(&std::fs::read(self.restore_path()).unwrap()).unwrap();
        snap["sessions"].as_object().unwrap().values().map(|entry| entry["name"].as_str().unwrap().to_owned()).collect()
    }
    /// A linha do diário com esse código chega (a gravação é em segundo plano).
    async fn diary_has(&self, name: &str, code: &str) -> bool {
        for _ in 0..40 {
            if self.python.diag().iter().any(|l| l["sessao"] == name && l["codigo"] == code) { return true; }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        false
    }
    fn projection(&self, name: &str) -> PathBuf { self.queue.join(format!("{name}.jsonl")) }
    fn chain(&self, name: &str) -> PathBuf { self.queue.parent().unwrap().join(".hangar-chain").join(format!("{name}.json")) }
    fn put_queue(&self, name: &str) {
        std::fs::create_dir_all(&self.queue).unwrap();
        std::fs::write(self.projection(name), "{\"id\":\"a\",\"text\":\"um\",\"ts\":1.0,\"delivered\":false}\n{\"id\":\"b\",\"text\":\"dois\",\"ts\":2.0,\"delivered\":false}\n").unwrap();
        std::fs::create_dir_all(self.chain(name).parent().unwrap()).unwrap();
        std::fs::write(self.chain(name), "{\"next\":\"outra\"}").unwrap();
    }
    fn sidecar_dir(&self, provider: &str) -> PathBuf {
        self.home.join(".hangar").join(if provider == "codex" { "codex-sessions" } else { "claude-headless" })
    }
    fn put_sidecar(&self, provider: &str, name: &str, meta: Value) -> PathBuf {
        let dir = self.sidecar_dir(provider);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}.json"));
        std::fs::write(&path, meta.to_string()).unwrap();
        path
    }
    async fn delete(&self, name: &str) -> (u16, Value) {
        let response = fake::client().delete(format!("http://{}/api/sessions/{name}", self.server)).bearer_auth(OWNER).send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    /// O último `DELETE` que chegou ao Python: a marca do Rust e a conversa fechada.
    fn forwarded(&self, name: &str) -> (Option<String>, Option<String>) {
        let (path, headers) = self.python.last_hit();
        assert_eq!(path, format!("/api/sessions/{name}"));
        let header = |key: &str| headers.get(key).and_then(|v| v.to_str().ok()).map(str::to_owned);
        (header("x-hangar-rust-closed"),
         header("x-hangar-closed-transcript").map(|t| percent_encoding::percent_decode_str(&t).decode_utf8().unwrap().into_owned()))
    }
}

fn fake_tmux(root: &Path, display: &str) -> PathBuf {
    let (calls, sessions, hidden, survive) = (root.join("calls"), root.join("sessions"), root.join("hidden"), root.join("survive"));
    let script = root.join("tmux");
    crate::write_executable(&script, format!(concat!(
        "#!/bin/sh\necho \"$*\" >> '{calls}'\nS='{sessions}'\ncase \"$1\" in\n",
        // O ator do terminal do Claude pergunta o pane; `#{{pane_pid}}` responde o pid de quem chamou.
        "  display-message) for last; do :; done; case \"$last\" in '#{{pane_pid}}'*) printf \"$PPID\\t\";; esac; printf '{display}\\t%%1\\t1\\n';;\n",
        "  capture-pane) printf '────────────────────────────────\\n❯ \\n────────────────────────────────\\n';;\n",
        "  has-session) grep -qx -- \"${{3#=}}\" \"$S\";;\n",
        "  kill-session) [ -e '{survive}' ] && exit 0; grep -vx -- \"${{3#=}}\" \"$S\" > \"$S.new\"; mv \"$S.new\" \"$S\";;\n",
        "  show-options) n=\"${{4#=}}\"; grep -qx -- \"${{n%:}}\" '{hidden}' && echo 1;;\n",
        "  list-panes) exit 0;;\n",
        "  *) exit 0;;\nesac\n"),
        calls = calls.display(), sessions = sessions.display(), hidden = hidden.display(), survive = survive.display(), display = display));
    std::fs::write(&sessions, "").unwrap();
    std::fs::write(&hidden, "").unwrap();
    script
}

/// Servidor do dono com a vigia ligada no multiplexador falso e a lista nas pastas do teste.
async fn env_with(adjust: impl FnOnce(&Arc<RuntimeRegistry>), display: &str) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let home = root.join("home");
    let queue = home.join(".claude").join(".hangar-queue");
    let tmux = fake_tmux(&root, display);
    let registry = crate::runtime_open::registry().await;
    registry.bind_watcher(Arc::new(Watcher::new(WatchConfig { home: home.clone(), queue_dir: queue.clone(),
        mux: Mux::with_program(&tmux, Duration::from_secs(5)), mux_program: tmux.clone().into_os_string() })));
    adjust(&registry);
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = AppState::new(fake::config(upstream, "127.0.0.1"));
    state.diag = python.bind_diary(state.diag.clone());
    let dirs = parse_dirs(&json!({"home": home, "claude": home.join(".claude"), "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"), "omp_agent": home.join(".omp/agent"),
        "kimi_home": home.join(".kimi-code")}).to_string());
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&tmux, Duration::from_secs(5)), capture_program: tmux.clone().into_os_string(),
        procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs }, FactsClient::new(upstream, SECRET.into())));
    let share = crate::share_pair_routes::share_state(&root.join("shares").join("shares.json"), &root.join("pair"), Arc::new(std::sync::Mutex::new(1_000.0)));
    state.share = Some(share.clone());
    state.write_gate_wait = Duration::from_secs(5);
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let server = fake::spawn_state(state).await;
    Env { _dir: dir, root, home, queue, tmux, python, server, registry, share, upstream }
}

async fn env() -> Env { env_with(|_| {}, "x").await }

/// Cano falso: o argv que o `process::kill` reconhece (`--escuta`, `--log …/cano-<key16>.log`), num grupo
/// próprio (o encerramento é por `killpg`). `stubborn`: ignora o SIGTERM. Uma thread o colhe ao sair. Cada
/// teste usa a sua chave: cano vivo de outra prova com a mesma chave seguraria os rastros desta. O grupo
/// morre no `Drop`, também quando uma asserção cai.
fn fake_cano(dir: &Path, key: &str, body: Cano) -> FakeCano {
    std::fs::create_dir_all(dir).unwrap();
    let log = dir.join(format!("cano-{key}.log"));
    std::fs::write(&log, "").unwrap();
    let body = match body {
        Cano::Plain => "while :; do sleep 0.2; done",
        Cano::Stubborn => "trap '' TERM; while :; do sleep 0.2; done",
        // Um neto que sai do grupo (`setsid`) e vive mais que a espera: não é o cano nem o agente.
        Cano::LoneGrandchild => "sh -c 'setsid sleep 8 & wait' & while :; do sleep 0.2; done",
    };
    let mut child = std::process::Command::new("sh").args(["-c", body, "hangar-cano", "--escuta", "unix:/nao-existe", "--log"]).arg(&log)
        .process_group(0).spawn().unwrap();
    let pid = child.id();
    std::thread::spawn(move || { let _ = child.wait(); });
    std::thread::sleep(Duration::from_millis(150));
    FakeCano(pid)
}

enum Cano { Plain, Stubborn, LoneGrandchild }

struct FakeCano(u32);

impl Drop for FakeCano {
    fn drop(&mut self) { kill_group(self.0) }
}

#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool { Path::new(&format!("/proc/{pid}")).exists() && !std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default().contains(") Z ") }

fn kill_group(pid: u32) { let _ = std::process::Command::new("kill").args(["-9", &format!("-{pid}")]).status(); }

fn cano(pid: u32) -> Value { json!({"pid": pid, "escuta": "unix:/nao-existe", "token": "t", "ts": 1.0, "versao": 1}) }

/// R1.3-1, D17: sessão que já não existe fecha com 200, sem `kill-session` (só a pergunta se ela existe), e
/// o Python recebe o pedido marcado, para as lojas dele. Captura `already_dead`.
#[tokio::test]
async fn delete_r1_3_already_dead_session_is_a_success_without_kill() {
    let e = env().await;
    let (status, body) = e.delete("ghost").await;
    assert_eq!(status, 200, "{body}");
    assert!(e.calls("kill-session").is_empty(), "nada a matar: {:?}", e.calls(""));
    assert_eq!(e.python.hits_to("/api/sessions/ghost"), 1);
    assert_eq!(e.forwarded("ghost").0.as_deref(), Some(SECRET), "fechada no Rust: o Python só limpa as lojas dele");
}

/// R1.3-1 no caminho Pi/omp/Kimi: o spec diz Pi e a sessão sumiu (`other_terminal_dead`); fechar é sucesso
/// pelo nome, nunca o 500 da descoberta.
#[tokio::test]
async fn delete_r1_3_dead_pi_session_is_a_success() {
    use hangar_server::runtime::other_terminal::{OtherDiscovery, OtherOptions};
    let e = env_with(|registry| {
        let options = OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(), hangar_server::provider_models::inbox::Store::new(),
            Arc::new(hangar_server::list::procs::SystemProcs::default()), Arc::new(hangar_server::interactions::InteractionStore::default()));
        registry.bind_other(options, OtherDiscovery {
            find: Arc::new(|_| Box::pin(async { Some(Err(hangar_server::runtime::protocol::RuntimeError::new("other_terminal_dead", "morta"))) })),
            names: Arc::new(|| Box::pin(async { Ok(vec![]) })),
        }).unwrap();
    }, "x").await;
    e.put_queue("pi-dead");
    let (status, body) = e.delete("pi-dead").await;
    assert_eq!(status, 200, "{body}");
    assert!(e.calls("kill-session").is_empty());
    assert!(!e.projection("pi-dead").exists(), "a fila do nome sai junto");
    assert_eq!(e.forwarded("pi-dead").0.as_deref(), Some(SECRET));
}

/// R1.3-3/4/5: pane de outro programa fecha pelo nome; o shell `term-<nome>` só com a nossa marca; a fila
/// e o `then` saem. Capturas `unknown_program_pane` e `unknown_program_unmarked_term`.
#[tokio::test]
async fn delete_r1_3_unknown_program_pane_closes_by_name() {
    let e = env().await;
    e.sessions(&["cat1", "term-cat1", "cat2", "term-cat2"]);
    e.hidden(&["term-cat1"]);
    e.put_queue("cat1");
    let (status, body) = e.delete("cat1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(e.calls("kill-session"), ["kill-session -t =cat1", "kill-session -t =term-cat1"]);
    assert_eq!(e.alive(), ["cat2", "term-cat2"]);
    assert!(!e.projection("cat1").exists(), "fila apagada");
    assert!(!e.chain("cat1").exists(), "vínculo then apagado");
    assert_eq!(e.forwarded("cat1").0.as_deref(), Some(SECRET));
    // Sem a marca o `term-` é de outro: fica.
    let (status, _) = e.delete("cat2").await;
    assert_eq!(status, 200);
    assert_eq!(e.alive(), ["term-cat2"]);
}

/// R1.3-2, D16: o pane que sobrevive ao `kill-session` (psmux) é 500 com código, e nada durável sai: fila,
/// `then`, shell e convites ficam, e o Python não recebe nada. Captura `survives_kill`.
#[tokio::test]
async fn delete_r1_3_pane_that_survives_is_500_and_keeps_everything() {
    // Nomes `s*`: a vida falsa dos convites os dá por vivos, e a varredura não os revoga sozinha.
    let e = env().await;
    e.sessions(&["s-cat1", "term-s-cat1"]);
    e.hidden(&["term-s-cat1"]);
    e.put_queue("s-cat1");
    let (invite, _) = e.invite("s-cat1").await;
    std::fs::write(e.root.join("survive"), "").unwrap();
    e.restore_entry("s-cat1");
    let (status, body) = e.delete("s-cat1").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (500, Some("session_kill_unconfirmed")), "{body}");
    assert_eq!(e.calls("kill-session"), ["kill-session -t =s-cat1"], "o shell não é tocado");
    assert!(e.projection("s-cat1").exists() && e.chain("s-cat1").exists());
    assert!(e.share.store.all().iter().any(|s| s.id == invite.id && s.revoked_at.is_none()), "convite intacto");
    assert_eq!(e.python.hits_to("/api/sessions/s-cat1"), 0);
    assert_eq!(e.restore_names(), ["s-cat1"], "segue viva: volta no próximo reboot");
    // A falha vai ao diário com o código.
    assert!(e.diary_has("s-cat1", "session_kill_unconfirmed").await, "{:?}", e.python.diag());
    // Fechada de fato, sai da fotografia.
    std::fs::remove_file(e.root.join("survive")).unwrap();
    assert_eq!(e.delete("s-cat1").await.0, 200);
    assert!(e.restore_names().is_empty());
}

/// R1.3-9: os convites da sessão fechada pelo nome são revogados; a revogação que falha vira aviso e o
/// fechamento segue. Captura `unknown_program_share_bridge_fails` (`erro_convite_nao_acompanhou`).
#[tokio::test]
async fn delete_r1_3_invites_are_revoked_and_a_failure_is_a_warning() {
    use std::os::unix::fs::PermissionsExt;
    let e = env().await;
    e.sessions(&["s-cat1", "s-cat2"]);
    let (first, _) = e.invite("s-cat1").await;
    e.invite("s-cat2").await;
    let (status, body) = e.delete("s-cat1").await;
    assert_eq!(status, 200, "{body}");
    assert!(e.share.store.all().iter().any(|s| s.id == first.id && s.revoked_at.is_some()), "convite revogado");
    // A pasta dos convites só de leitura: gravar a revogação falha.
    let shares = e.root.join("shares");
    std::fs::set_permissions(&shares, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (status, body) = e.delete("s-cat2").await;
    std::fs::set_permissions(&shares, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["warning"]["code"], "erro_convite_nao_acompanhou", "{body}");
    assert_eq!(e.alive(), Vec::<String>::new(), "fechou mesmo assim");
}

/// R1.3-12: com a troca de conversa em curso (porta retida), o fechamento recusa e nada é morto.
#[tokio::test]
async fn delete_r1_3_transfer_in_progress_refuses() {
    let e = env().await;
    e.sessions(&["cat1"]);
    e.registry.ingress().hold("cat1", Duration::ZERO).await.unwrap();
    let (status, body) = e.delete("cat1").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (409, Some("session_transfer_busy")), "{body}");
    assert!(e.calls("kill-session").is_empty());
    assert_eq!(e.python.hits_to("/api/sessions/cat1"), 0);
}

/// Codex sem terminal parado, com cano vivo, morto ou sem cano: o Rust mata o cano e apaga sidecar, rastros
/// do cano e fila, e manda a conversa (`rollout_path`) junto; o shell `term-<nome>` só sai com a nossa
/// marca (`registry.py` `_kill_hidden_shell`). Capturas `codex_headless_*`.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn delete_r1_3_parked_codex_headless_closes_in_rust() {
    const KEY: &str = "kkkkkkkk0000000a";
    let e = env().await;
    let dir = e.sidecar_dir("codex");
    let process = fake_cano(&dir, KEY, Cano::Plain);
    let pid = process.0;
    let sidecar = e.put_sidecar("codex", "cxh", json!({"name": "cxh", "provider": "codex", "headless": true, "key": KEY,
        "cano": cano(pid), "rollout_path": "/r/rollout-1.jsonl"}));
    e.put_queue("cxh");
    e.sessions(&["term-cxh", "term-cxn"]);
    e.hidden(&["term-cxh"]);
    let (status, body) = e.delete("cxh").await;
    assert_eq!(status, 200, "{body}");
    assert!(!alive(pid), "cano encerrado");
    assert_eq!((sidecar.exists(), dir.join(format!("cano-{KEY}.log")).exists(), e.projection("cxh").exists()), (false, false, false));
    assert_eq!(e.forwarded("cxh"), (Some(SECRET.into()), Some("/r/rollout-1.jsonl".into())));
    assert_eq!(e.alive(), ["term-cxn"], "o shell marcado saiu");
    // Sem cano gravado (o `save(headless=True)` antes de subir) e com o cano já morto.
    for (name, process) in [("cxn", Value::Null), ("cxd", cano(exited_pid()))] {
        let sidecar = e.put_sidecar("codex", name, json!({"name": name, "provider": "codex", "headless": true, "key": KEY, "cano": process}));
        e.put_queue(name);
        let (status, body) = e.delete(name).await;
        assert_eq!(status, 200, "{name}: {body}");
        assert!(!sidecar.exists() && !e.projection(name).exists(), "{name}");
    }
    assert_eq!(e.alive(), ["term-cxn"], "sem a marca o shell é de outro: fica");
    assert_eq!(e.python.hits_to("/api/sessions/cxh"), 1);
}

/// O `kill` da ponte do Codex (o `DELETE` que entrou pelo Python, Connect e convidado) fecha aqui o Codex
/// sem terminal parado: 200, nunca o 421 que devolvia ao Python.
#[tokio::test]
async fn delete_r1_3_codex_bridge_kill_closes_a_parked_headless() {
    let e = env().await;
    let sidecar = e.put_sidecar("codex", "cxb", json!({"name": "cxb", "provider": "codex", "headless": true, "key": "kkkkkkkk0000000e", "cano": null}));
    e.put_queue("cxb");
    let private = e.private().await;
    let response = fake::client().post(format!("http://{private}/__hangar_server/codex/cxb/kill")).header("x-hangar-internal", SECRET).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    assert!(!sidecar.exists() && !e.projection("cxb").exists());
}

/// D16: o cano do Codex sem terminal que ignora o SIGTERM segura o fechamento: 500 com código, e sidecar e
/// fila ficam. Captura `codex_headless_cano_ignores_term`.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn delete_r1_3_codex_headless_cano_that_survives_is_500() {
    const KEY: &str = "kkkkkkkk0000000b";
    let e = env().await;
    let dir = e.sidecar_dir("codex");
    let process = fake_cano(&dir, KEY, Cano::Stubborn);
    let sidecar = e.put_sidecar("codex", "cxh", json!({"name": "cxh", "provider": "codex", "headless": true, "key": KEY, "cano": cano(process.0)}));
    e.put_queue("cxh");
    let (status, body) = e.delete("cxh").await;
    assert!(alive(process.0), "o cano seguia vivo");
    assert_eq!((status, body["detail"]["code"].as_str()), (500, Some("session_kill_unconfirmed")), "{body}");
    assert!(sidecar.exists() && e.projection("cxh").exists(), "sidecar e fila conservados");
    assert_eq!(e.python.hits_to("/api/sessions/cxh"), 0);
}

/// Claude sem terminal parado (sem ator aqui): o cano morre, e sidecar, rastros e fila saem. Capturas
/// `claude_headless_dead_cano`/`_live_cano`; a que ignora o SIGTERM é 500 com tudo conservado (D16).
#[cfg(target_os = "linux")]
#[tokio::test]
async fn delete_r1_3_parked_claude_headless_closes_in_rust() {
    const KEY: &str = "kkkkkkkk0000000c";
    let e = env().await;
    let dir = e.sidecar_dir("claude");
    let process = fake_cano(&dir, KEY, Cano::Plain);
    let sidecar = e.put_sidecar("claude", "hl1", json!({"name": "hl1", "provider": "claude", "headless": true, "key": KEY, "cano": cano(process.0)}));
    e.put_queue("hl1");
    let (status, body) = e.delete("hl1").await;
    assert_eq!(status, 200, "{body}");
    assert!(!alive(process.0));
    assert_eq!((sidecar.exists(), dir.join(format!("cano-{KEY}.log")).exists(), e.projection("hl1").exists()), (false, false, false));
    assert_eq!(e.forwarded("hl1").0.as_deref(), Some(SECRET));
    const STUBBORN: &str = "kkkkkkkk0000000d";
    let stubborn = fake_cano(&dir, STUBBORN, Cano::Stubborn);
    let sidecar = e.put_sidecar("claude", "hl2", json!({"name": "hl2", "provider": "claude", "headless": true, "key": STUBBORN, "cano": cano(stubborn.0)}));
    e.put_queue("hl2");
    let (status, body) = e.delete("hl2").await;
    assert!(alive(stubborn.0));
    assert_eq!((status, body["detail"]["code"].as_str()), (500, Some("session_kill_unconfirmed")), "{body}");
    assert!(sidecar.exists() && e.projection("hl2").exists());
}

/// Um neto que saiu do grupo do cano (`setsid`) não segura o fechamento: só o cano e o agente contam, como no
/// Python, que esperava só o cano.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn delete_r1_3_a_detached_grandchild_does_not_hold_the_close() {
    const KEY: &str = "kkkkkkkk0000000f";
    let e = env().await;
    let dir = e.sidecar_dir("claude");
    let process = fake_cano(&dir, KEY, Cano::LoneGrandchild);
    let sidecar = e.put_sidecar("claude", "hl3", json!({"name": "hl3", "provider": "claude", "headless": true, "key": KEY, "cano": cano(process.0)}));
    let (status, body) = e.delete("hl3").await;
    assert_eq!(status, 200, "{body}");
    assert!(!alive(process.0) && !sidecar.exists());
}

/// O pid gravado que já não é o cano da chave não leva sinal, e o fechamento diz que nada foi morto.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn delete_r1_3_a_foreign_pid_is_not_killed_and_says_so() {
    let e = env().await;
    let mut other = std::process::Command::new("sleep").arg("30").spawn().unwrap();
    e.put_sidecar("claude", "hl4", json!({"name": "hl4", "provider": "claude", "headless": true, "key": "kkkkkkkk00000010", "cano": cano(other.id())}));
    let closed = e.registry.close_by_name(None, "hl4", None, async { None }).await.map_err(|error| error.code);
    let survived = alive(other.id());
    let _ = other.kill();
    let _ = other.wait();
    assert!(survived, "processo alheio intacto");
    assert_eq!(closed.map(|value| value["killed"].clone()), Ok(json!(false)));
}

/// A fila que não sai depois do encerramento é 500 com código, como o `PromptQueue.clear` que falhava no
/// Python, e o Python não recebe nada.
#[tokio::test]
async fn delete_r1_3_a_queue_that_cannot_be_cleared_is_500() {
    use std::os::unix::fs::PermissionsExt;
    let e = env().await;
    e.sessions(&["cat9"]);
    e.put_queue("cat9");
    std::fs::set_permissions(&e.queue, std::fs::Permissions::from_mode(0o555)).unwrap();
    let (status, body) = e.delete("cat9").await;
    std::fs::set_permissions(&e.queue, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!((status, body["detail"]["code"].as_str()), (500, Some("session_queue_clear_failed")), "{body}");
    assert_eq!(e.python.hits_to("/api/sessions/cat9"), 0);
}

/// Claude com terminal cujo agente morreu (o pane ficou com o shell): a entrada sai sem matar processo, o
/// pane fecha pelo nome e a conversa da vida guardada vai junto. Captura `claude_agent_dead_terminal`.
#[tokio::test]
async fn delete_r1_3_claude_with_dead_agent_closes_by_name() {
    use hangar_server::runtime::terminal::TerminalTarget;
    use hangar_server::terminal_input::TerminalBinding;
    let e = env_with(|_| {}, "cc1").await;
    e.sessions(&["cc1"]);
    e.put_queue("cc1");
    let dir = e.root.join("cc");
    std::fs::create_dir_all(dir.join("sessions")).unwrap();
    let transcript = dir.join("cc1.jsonl");
    std::fs::write(&transcript, "").unwrap();
    // O agente é um filho do teste (o pane responde o pid do teste); morre antes do fechamento.
    let mut agent = std::process::Command::new("sleep").arg("60").spawn().unwrap();
    let agent_pid = agent.id();
    std::fs::write(dir.join("sessions").join(format!("{agent_pid}.json")), json!({"pid": agent_pid, "sessionId": "sid"}).to_string()).unwrap();
    let binding = TerminalBinding { name: "cc1".into(), pane: "%1".into(), conversation: "sid".into(), generation: 1, created: 1,
        mux_argv: vec![e.tmux.to_string_lossy().into_owned()], windows: false, clipboard_lock_path: None };
    let birth = hangar_server::terminal_facts::agent_birth(agent_pid).unwrap();
    e.registry.open_terminal(TerminalTarget { key: "k-cc1".into(), generation: 1, name: "cc1".into(), binding,
        lease_path: dir.join("cc1.lease"), state_path: dir.join("cc1.state"), projection_dir: dir.join("cc1.projection"), transcript: transcript.clone(),
        created: 0.0, plugin_key: None,
        metadata: hangar_server::terminal_facts::TerminalMetadata { config_dir: dir.clone(), agent_pid, agent_birth: birth,
            legacy_import_after: None, fingerprint: "fixture".into(), origin: "uds:fixture".into() } }).await.unwrap();
    agent.kill().unwrap();
    agent.wait().unwrap();
    let (status, body) = e.delete("cc1").await;
    assert_eq!(status, 200, "{body}");
    assert!(!e.registry.has_entry(None, "cc1").await, "a entrada saiu");
    assert_eq!(e.calls("kill-session"), ["kill-session -t =cc1"]);
    assert!(!e.projection("cc1").exists());
    assert_eq!(e.forwarded("cc1"), (Some(SECRET.into()), Some(transcript.to_string_lossy().into_owned())));
}

/// Pi/omp/Kimi fechado pelo ator: o shell `term-<nome>` com a nossa marca fecha junto.
#[tokio::test]
async fn delete_r1_3_other_terminal_close_takes_the_marked_shell() {
    use hangar_server::runtime::other_terminal::{OtherDiscovery, OtherDiscoveryState, OtherOptions, OtherTarget};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_owned();
    let tmux = fake_tmux(&root, "pi-a");
    std::fs::write(root.join("sessions"), "pi-a\nterm-pi-a\n").unwrap();
    std::fs::write(root.join("hidden"), "term-pi-a\n").unwrap();
    let dirs = hangar_server::list::discover_other::Dirs { home: root.join("home"), claude: root.join(".claude"), codex_home: root.join(".codex"),
        pi_sessions: root.join(".pi/agent/sessions"), omp_config: root.join(".omp"), omp_agent: root.join(".omp/agent"), kimi_home: root.join(".kimi-code") };
    std::fs::create_dir_all(&dirs.home).unwrap();
    let target = OtherTarget { key: "life-a".into(), generation: 1, name: "pi-a".into(), provider: hangar_transcript::Provider::Pi,
        binding: hangar_server::terminal_input::TerminalBinding { name: "pi-a".into(), pane: "%1".into(), conversation: "c".into(), generation: 1,
            created: 100, mux_argv: vec![tmux.to_string_lossy().into_owned()], windows: false, clipboard_lock_path: None },
        pane_address: "%1".into(), dirs, cwd: root.to_string_lossy().into_owned(), pane_pid: 1, agent_pid: i64::from(exited_pid()), agent_birth: 1.0,
        transcript: root.join("c.jsonl"), state_path: root.join("runtime/life-a.json"), lease_path: root.join("runtime/life-a.lock"),
        projection_dir: root.join("queue"), config_dirs: vec![root.clone()], created: 100.0 };
    let options = OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(), hangar_server::provider_models::inbox::Store::new(),
        Arc::new(hangar_server::list::procs::SystemProcs::default()), Arc::new(hangar_server::interactions::InteractionStore::default()));
    let registry = crate::runtime_open::registry().await;
    let found = target.clone();
    registry.bind_other(options, OtherDiscovery {
        find: Arc::new(move |_| { let found = found.clone(); Box::pin(async move { Some(Ok(OtherDiscoveryState::Ready(found))) }) }),
        names: Arc::new(|| Box::pin(async { Ok(vec![]) })),
    }).unwrap();
    let closed = registry.close_other("pi-a").await.expect("Pi daqui");
    assert!(closed.is_ok(), "{:?}", closed.err().map(|e| e.code));
    let left = std::fs::read_to_string(root.join("sessions")).unwrap();
    assert_eq!(left, "", "o pane e o shell marcado fecharam");
    // Sem a marca o `term-` é de outro: fica.
    std::fs::write(root.join("sessions"), "pi-a\nterm-pi-a\n").unwrap();
    std::fs::write(root.join("hidden"), "").unwrap();
    let closed = registry.close_other("pi-a").await.expect("Pi daqui");
    assert!(closed.is_ok(), "{:?}", closed.err().map(|e| e.code));
    assert_eq!(std::fs::read_to_string(root.join("sessions")).unwrap(), "term-pi-a\n");
}

/// Pid de um processo que já saiu.
fn exited_pid() -> u32 {
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    pid
}
