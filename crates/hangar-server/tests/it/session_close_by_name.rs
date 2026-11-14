//! R1.3: o `DELETE` do dono de uma sessão sem vida aberta no Rust (pane de outro programa, Codex e Claude
//! sem terminal parados, Claude cujo agente morreu, sessão que já acabou) fecha no Rust, pelo nome, e o
//! Python não recebe nada: o `then` e o nome fechado também são do Rust. O convidado com conta fecha pelo
//! mesmo caminho. O esperado vem das capturas de `contract/r1-delete/` (commit 43b407cc1), das decisões
//! D16/D17 e das decisões 1, 3 e 4 do fim da reserva.
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

/// Convidado com conta que vê as sessões sem dono, e um que não as vê.
const GUEST: &str = "convidado-ana";
const BLIND: &str = "convidado-bia";

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
        for _ in 0..400 {
            if self.python.diag().iter().any(|l| l["sessao"] == name && l["codigo"] == code) { return true; }
            // estavel: intervalo da espera pela linha do diário; a condição encerra antes do teto de 20 s.
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
    async fn delete(&self, name: &str) -> (u16, Value) { self.delete_as(name, OWNER).await }
    async fn delete_as(&self, name: &str, token: &str) -> (u16, Value) {
        let response = fake::client().delete(format!("http://{}/api/sessions/{name}", self.server)).bearer_auth(token).send().await.unwrap();
        let status = response.status().as_u16();
        let text = response.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::String(text)))
    }
    /// `closed-sessions.json` da conta desta prova: caminho real da conversa → nome.
    fn closed_names(&self) -> Value {
        std::fs::read(self.home.join(".claude/closed-sessions.json")).ok()
            .map(|raw| serde_json::from_slice::<Value>(&raw).unwrap()).unwrap_or(Value::Null)
    }
    fn loop_file(&self, name: &str) -> PathBuf { self.home.join(".claude/.hangar-loop").join(format!("{name}.json")) }
}

fn fake_tmux(root: &Path, display: &str) -> PathBuf {
    let (calls, sessions, hidden, survive) = (root.join("calls"), root.join("sessions"), root.join("hidden"), root.join("survive"));
    let script = root.join("tmux");
    crate::write_executable(&script, format!(concat!(
        "#!/bin/sh\necho \"$*\" >> '{calls}'\nS='{sessions}'\ncase \"$1\" in\n",
        // O ator do terminal do Claude pergunta o pane; `#{{pane_pid}}` responde o pid de quem chamou.
        "  display-message) for last; do :; done; case \"$last\" in '#{{pane_pid}}'*) printf \"$PPID\\t\";; esac; printf '{display}\\t%%1\\t1\\n';;\n",
        "  capture-pane) printf '────────────────────────────────\\n❯ \\n────────────────────────────────\\n';;\n",
        "  has-session) grep -qx -- \"${{3#=}}\" \"$S\" || {{ echo \"can't find session: ${{3#=}}\" >&2; exit 1; }};;\n",
        // Terminais de atalho: as linhas do `list-sessions -F` ficam num arquivo.
        "  list-sessions) cat '{shortcuts}' 2>/dev/null;;\n",
        "  kill-session) [ -e '{survive}' ] && exit 0; grep -vx -- \"${{3#=}}\" \"$S\" > \"$S.new\"; mv \"$S.new\" \"$S\";;\n",
        "  show-options) n=\"${{4#=}}\"; grep -qx -- \"${{n%:}}\" '{hidden}' && echo 1;;\n",
        "  list-panes) exit 0;;\n",
        "  *) exit 0;;\nesac\n"),
        calls = calls.display(), sessions = sessions.display(), hidden = hidden.display(), survive = survive.display(), display = display,
        shortcuts = root.join("shortcuts").display()));
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
    // O nome fechado vai à pasta que o Arquivo lê: a conta desta prova, nunca a da máquina.
    let text = |path: &Path| path.to_string_lossy().into_owned();
    state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map(
        [("HOME".to_owned(), text(&home)), ("USERPROFILE".to_owned(), text(&home)), ("CLAUDE_CONFIG_DIR".to_owned(), text(&home.join(".claude")))].into()));
    let guests = hangar_server::login_guests::Service::new(hangar_server::login_guests::Settings { guests: root.join("guests.json"),
        vault: root.join("vault.json"), runtime_config: root.join("runtime-config.json"), sync_enabled: false, bootstrap: String::new(),
        secret: vec![1], secure: false, rate_max: 10.into(), rate_window: 300.into() });
    guests.store.create("Ana", &text(&root), true, true, "ana".into(), GUEST.into()).unwrap();
    guests.store.create("Bia", &text(&root), false, true, "bia".into(), BLIND.into()).unwrap();
    state.login_guests = Arc::new(guests);
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
        Cano::Plain => "echo ready; while :; do sleep 0.2; done",
        Cano::Stubborn => "trap '' TERM; echo ready; while :; do sleep 0.2; done",
        // Um neto que sai do grupo (`setsid`) e vive mais que a espera: não é o cano nem o agente.
        Cano::LoneGrandchild => "sh -c 'setsid sleep 8 & wait' & echo ready; while :; do sleep 0.2; done",
    };
    let mut child = std::process::Command::new("sh").args(["-c", body, "hangar-cano", "--escuta", "unix:/nao-existe", "--log"]).arg(&log)
        .process_group(0).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let cano = FakeCano(child.id());
    // O cano avisa que já armou o `trap`: SIGTERM antes disso mataria o teimoso.
    let mut ready = String::new();
    std::io::BufRead::read_line(&mut std::io::BufReader::new(child.stdout.take().unwrap()), &mut ready).unwrap();
    assert_eq!(ready.trim(), "ready", "o cano falso não subiu");
    std::thread::spawn(move || { let _ = child.wait(); });
    cano
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

/// R1-04, R1-21, R1-24, D17: sessão que já não existe fecha com 200 e o corpo do `kill_session`
/// (`test_kill_nao_avisa_ninguem_do_grupo`), sem `kill-session` (só a pergunta se ela existe); o `?by=`
/// do vigia é ignorado, e o Python não recebe nada. Captura `already_dead`.
#[tokio::test]
async fn delete_r1_04_already_dead_session_is_a_success_without_kill() {
    let e = env().await;
    let (status, body) = e.delete("ghost").await;
    assert_eq!((status, body), (200, json!({"ok": true, "warning": null})));
    assert!(e.calls("kill-session").is_empty(), "nada a matar: {:?}", e.calls(""));
    let (status, body) = e.delete("ghost?by=arbitro").await;
    assert_eq!((status, body), (200, json!({"ok": true, "warning": null})), "o `by` não muda nada");
    assert_eq!(e.python.hits_to("/api/sessions/ghost"), 0, "o fechamento inteiro é do Rust");
}

/// R1-01, decisões 1 e 3: o convidado com conta fecha a sessão que alcança pelo fechamento do dono, sem
/// passar pelo Python; o que não a vê leva o 403 do porteiro e nada morre.
#[tokio::test]
async fn delete_r1_01_login_guest_closes_through_the_owner_close() {
    let e = env().await;
    e.sessions(&["cat1", "cat2"]);
    e.put_queue("cat1");
    let (status, body) = e.delete_as("cat2", BLIND).await;
    assert_eq!((status, body["detail"]["code"].as_str()), (403, Some("erro_fora_do_convidado")), "{body}");
    let (status, body) = e.delete_as("cat1", GUEST).await;
    assert_eq!((status, body), (200, json!({"ok": true, "warning": null})));
    assert_eq!(e.alive(), ["cat2"], "só a do convidado fechou");
    assert!(!e.projection("cat1").exists() && !e.chain("cat1").exists(), "fila e then saem como no fechamento do dono");
    assert_eq!(e.python.hits_to("/api/sessions/cat1") + e.python.hits_to("/api/sessions/cat2"), 0);
}

/// R5 (decisão 5): o convidado com conta numa rota só do dono que o porteiro deixa passar (a reserva
/// pré-aquecida) leva 403 do Rust, que não o desloga, e o Python não recebe nada.
#[tokio::test]
async fn r5_login_guest_on_an_owner_route_is_403_from_rust() {
    let e = env().await;
    let response = fake::client().put(format!("http://{}/api/sessions/prewarm", e.server)).bearer_auth(GUEST)
        .header("content-type", "application/json").body("{}").send().await.unwrap();
    let status = response.status().as_u16();
    let body: Value = response.json().await.unwrap();
    assert_eq!((status, body), (403, json!({"detail": {"code": "erro_fora_do_convidado", "params": {}, "msg": "fora do acesso do convidado"}})));
    assert_eq!(e.python.hits_to("/api/sessions/prewarm"), 0);
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
    // R1-02: o convidado com conta também fecha o Pi no Rust.
    e.put_queue("pi-dead");
    let (status, body) = e.delete_as("pi-dead", GUEST).await;
    assert_eq!(status, 200, "{body}");
    assert!(!e.projection("pi-dead").exists());
    assert_eq!(e.python.hits_to("/api/sessions/pi-dead"), 0);
}

/// R1.3-3/4/5: pane de outro programa fecha pelo nome; o shell `term-<nome>` só com a nossa marca; a fila
/// e o `then` saem. Capturas `unknown_program_pane` e `unknown_program_unmarked_term`.
#[tokio::test]
async fn delete_r1_3_unknown_program_pane_closes_by_name() {
    let e = env().await;
    e.sessions(&["cat1", "term-cat1", "cat2", "term-cat2"]);
    e.hidden(&["term-cat1"]);
    e.put_queue("cat1");
    std::fs::create_dir_all(e.loop_file("cat1").parent().unwrap()).unwrap();
    std::fs::write(e.loop_file("cat1"), "{\"goal\":\"g\",\"status\":\"running\"}").unwrap();
    let (status, body) = e.delete("cat1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(e.calls("kill-session"), ["kill-session -t =cat1", "kill-session -t =term-cat1"]);
    assert_eq!(e.alive(), ["cat2", "term-cat2"]);
    assert!(!e.projection("cat1").exists(), "fila apagada");
    assert!(!e.chain("cat1").exists(), "vínculo then apagado");
    // R1-14: o loop da vida que acabou também sai.
    assert!(!e.loop_file("cat1").exists(), "loop esquecido");
    assert_eq!(e.python.hits_to("/api/sessions/cat1"), 0);
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
    // R1-10: o nome fechado fica por conversa (`test_kill_guarda_o_nome_para_o_arquivo`); a que não existe
    // no disco entra pelo caminho dado.
    assert_eq!(e.closed_names()["/r/rollout-1.jsonl"]["name"], "cxh", "{}", e.closed_names());
    assert!(e.closed_names()["/r/rollout-1.jsonl"]["closed_at"].is_f64());
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
    assert_eq!(e.python.hits_to("/api/sessions/cxh"), 0);
}

/// R1-15: os terminais de atalho da sessão fechada saem junto, ainda com o nome reservado; o de outra
/// sessão fica.
#[tokio::test]
async fn delete_r1_15_shortcut_terminals_of_the_session_close_with_it() {
    let e = env().await;
    e.sessions(&["cat1", "shortcut-a1b2c3", "shortcut-d4e5f6"]);
    std::fs::write(e.root.join("shortcuts"), concat!("shortcut-a1b2c3\tcat1\ta1b2c3\t100\t0\t\t0\t1\t\t\t0\tdev\n",
        "shortcut-d4e5f6\toutra\td4e5f6\t100\t0\t\t0\t1\t\t\t0\tdev\n")).unwrap();
    let (status, body) = e.delete("cat1").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(e.alive(), ["shortcut-d4e5f6"], "o atalho da sessão fechou, o da outra ficou");
    assert!(e.calls("kill-session").contains(&"kill-session -t =shortcut-a1b2c3".to_owned()), "{:?}", e.calls(""));
}

/// R1-17 (decisão 3): os arquivos do cofre da sessão saem só com o fechamento confirmado, também
/// quando quem fecha é o convidado; recusado, o arquivo fica com a sessão viva.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn delete_r1_17_vault_files_leave_only_with_a_confirmed_close() {
    let dir = hangar_server::vault::files::base().expect("XDG_RUNTIME_DIR: a pasta dos arquivos do cofre");
    let e = env().await;
    let name = format!("cofre-r1-17-{}", std::process::id());
    e.sessions(&[name.as_str()]);
    let file = hangar_server::vault::files::write(&dir, "", &name, b"segredo").unwrap();
    std::fs::write(e.root.join("survive"), "").unwrap();
    assert_eq!(e.delete_as(&name, GUEST).await.0, 500);
    // estavel: a limpeza do cofre roda numa tarefa; meio segundo basta para uma limpeza indevida aparecer.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(file.exists(), "recusado: o arquivo segue com a sessão viva");
    std::fs::remove_file(e.root.join("survive")).unwrap();
    assert_eq!(e.delete_as(&name, GUEST).await.0, 200);
    for _ in 0..100 {
        if !file.exists() { break; }
        // estavel: intervalo da espera pela tarefa da limpeza; a condição encerra antes do teto de 5 s.
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(!file.exists(), "fechada de fato, o arquivo do cofre saiu");
}

/// Achado 2 da revisão (R1-10): fechamentos ao mesmo tempo não perdem nome fechado; ler, juntar e gravar
/// `closed-sessions.json` é um ciclo só, sob a trava.
#[cfg(target_os = "linux")]
#[tokio::test(flavor = "multi_thread")]
async fn delete_r1_10_concurrent_closes_keep_every_closed_name() {
    let e = env().await;
    let names: Vec<String> = (0..6).map(|i| format!("cx{i}")).collect();
    for (i, name) in names.iter().enumerate() {
        e.put_sidecar("codex", name, json!({"name": name, "provider": "codex", "headless": true,
            "key": format!("kkkkkkkk000001{i:02}"), "cano": null, "rollout_path": format!("/r/{name}.jsonl")}));
    }
    let closes = names.iter().map(|name| e.delete(name));
    for (status, body) in futures_util::future::join_all(closes).await { assert_eq!(status, 200, "{body}"); }
    let recorded = e.closed_names();
    for name in &names { assert_eq!(recorded[format!("/r/{name}.jsonl").as_str()]["name"], name.as_str(), "{recorded}"); }
}

/// R1-10 (`test_closed_names_ilegivel_vai_para_o_lado_antes_de_gravar`): o arquivo de nomes ilegível vai
/// para o lado (o `.bad` anterior fica, este vira `.bad.2`) e o nome novo é gravado num arquivo limpo.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn delete_r1_10_unreadable_closed_names_are_set_aside() {
    let e = env().await;
    let claude = e.home.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("closed-sessions.json.bad"), "anterior").unwrap();
    std::fs::write(claude.join("closed-sessions.json"), "{quebrado").unwrap();
    e.put_sidecar("codex", "cxa", json!({"name": "cxa", "provider": "codex", "headless": true, "key": "kkkkkkkk00000011",
        "cano": null, "rollout_path": "/r/a.jsonl"}));
    let (status, body) = e.delete("cxa").await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(std::fs::read_to_string(claude.join("closed-sessions.json.bad")).unwrap(), "anterior", "o .bad anterior fica");
    assert_eq!(std::fs::read_to_string(claude.join("closed-sessions.json.bad.2")).unwrap(), "{quebrado");
    let names = e.closed_names();
    assert_eq!((names.as_object().map(|m| m.len()), &names["/r/a.jsonl"]["name"]), (Some(1), &json!("cxa")), "{names}");
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

/// Achado 4 da revisão: a ponte `/__hangar_server/claude/<n>/close` não tem chamador (o Connect saiu do
/// Python em 6bdd33994) e saiu: 421, nada fecha, e o fechamento é só o `DELETE`.
#[tokio::test]
async fn the_claude_close_bridge_is_gone() {
    let e = env().await;
    e.sessions(&["cat1"]);
    let private = e.private().await;
    let response = fake::client().post(format!("http://{private}/__hangar_server/claude/cat1/close")).header("x-hangar-internal", SECRET).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 421);
    assert_eq!(e.alive(), ["cat1"]);
    assert!(e.calls("kill-session").is_empty());
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
    assert_eq!(e.python.hits_to("/api/sessions/hl1"), 0);
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
    // R1-10: a conversa da vida guardada dá o nome fechado, pelo caminho real (`os.path.realpath`).
    let real = std::fs::canonicalize(&transcript).unwrap().to_string_lossy().into_owned();
    assert_eq!(e.closed_names()[real.as_str()]["name"], "cc1", "{}", e.closed_names());
    assert_eq!(e.python.hits_to("/api/sessions/cc1"), 0);
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

/// Achado 3 da revisão e decisão 4: o fechamento do Pi/omp/Kimi passa pela limpeza ligada ao ator
/// (`other_cleanup`, a mesma de `routes.rs` na subida): o `then` sai e o nome fechado vai à pasta que o
/// Arquivo lê (`claude_base`, decisão do usuário), não à `dirs.claude` da lista.
#[tokio::test]
async fn delete_r1_other_terminal_close_runs_the_owner_cleanups() {
    use hangar_server::runtime::other_terminal::{OtherDiscovery, OtherDiscoveryState, OtherOptions, OtherTarget};
    let dir = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(dir.path()).unwrap();
    let tmux = fake_tmux(&root, "pi-a");
    std::fs::write(root.join("sessions"), "pi-a\n").unwrap();
    let dirs = hangar_server::list::discover_other::Dirs { home: root.join("home"), claude: root.join(".claude"), codex_home: root.join(".codex"),
        pi_sessions: root.join(".pi/agent/sessions"), omp_config: root.join(".omp"), omp_agent: root.join(".omp/agent"), kimi_home: root.join(".kimi-code") };
    std::fs::create_dir_all(&dirs.home).unwrap();
    let transcript = root.join("c.jsonl");
    std::fs::write(&transcript, "").unwrap();
    let chain = root.join(".claude/.hangar-chain/pi-a.json");
    std::fs::create_dir_all(chain.parent().unwrap()).unwrap();
    std::fs::write(&chain, r#"{"target":"outra","text":"oi"}"#).unwrap();
    let target = OtherTarget { key: "life-a".into(), generation: 1, name: "pi-a".into(), provider: hangar_transcript::Provider::Pi,
        binding: hangar_server::terminal_input::TerminalBinding { name: "pi-a".into(), pane: "%1".into(), conversation: "c".into(), generation: 1,
            created: 100, mux_argv: vec![tmux.to_string_lossy().into_owned()], windows: false, clipboard_lock_path: None },
        pane_address: "%1".into(), dirs: dirs.clone(), cwd: root.to_string_lossy().into_owned(), pane_pid: 1, agent_pid: i64::from(exited_pid()), agent_birth: 1.0,
        transcript: transcript.clone(), state_path: root.join("runtime/life-a.json"), lease_path: root.join("runtime/life-a.lock"),
        projection_dir: root.join("queue"), config_dirs: vec![root.clone()], created: 100.0 };
    let upstream = crate::refused_address();
    let mut state = AppState::new(fake::config(upstream, "127.0.0.1"));
    let (home, base) = (root.join("home").to_string_lossy().into_owned(), root.join("conta").to_string_lossy().into_owned());
    state.accounts = hangar_server::accounts::AccountService::new(hangar_server::accounts::environment::AccountEnvironment::from_map(
        [("HOME".to_owned(), home.clone()), ("USERPROFILE".to_owned(), home), ("CLAUDE_CONFIG_DIR".to_owned(), base)].into()));
    let list_dirs = parse_dirs(&json!({"home": dirs.home, "claude": dirs.claude, "codex_home": dirs.codex_home, "pi_sessions": dirs.pi_sessions,
        "omp_config": dirs.omp_config, "omp_agent": dirs.omp_agent, "kimi_home": dirs.kimi_home}).to_string());
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&tmux, Duration::from_secs(5)), capture_program: tmux.clone().into_os_string(),
        procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs: list_dirs }, FactsClient::new(upstream, SECRET.into())));
    let registry = crate::runtime_open::registry().await;
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let st = Arc::new(state);
    let mut options = OtherOptions::new(hangar_server::terminal_control::TerminalPool::new(), hangar_server::provider_models::inbox::Store::new(),
        Arc::new(hangar_server::list::procs::SystemProcs::default()), Arc::new(hangar_server::interactions::InteractionStore::default()));
    options.after_closed = Some(hangar_server::session_write::bridge::other_cleanup(Arc::downgrade(&st)));
    let found = target.clone();
    registry.bind_other(options, OtherDiscovery {
        find: Arc::new(move |_| { let found = found.clone(); Box::pin(async move { Some(Ok(OtherDiscoveryState::Ready(found))) }) }),
        names: Arc::new(|| Box::pin(async { Ok(vec![]) })),
    }).unwrap();
    let closed = registry.close_other("pi-a").await.expect("Pi daqui").map_err(|e| e.code);
    assert_eq!(closed.as_ref().map(|v| v["warnings"].clone()), Ok(json!([])), "{closed:?}");
    assert!(!chain.exists(), "o then da vida encerrada saiu");
    let names: Value = serde_json::from_slice(&std::fs::read(root.join("conta/closed-sessions.json")).unwrap()).unwrap();
    assert_eq!(names[&*transcript.to_string_lossy()]["name"], "pi-a", "{names}");
    assert!(!root.join(".claude/closed-sessions.json").exists(), "nunca na pasta da lista");
}

/// Pid de um processo que já saiu.
fn exited_pid() -> u32 {
    let mut gone = std::process::Command::new("true").spawn().unwrap();
    let pid = gone.id();
    gone.wait().unwrap();
    pid
}
