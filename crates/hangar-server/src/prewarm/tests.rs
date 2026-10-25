//! A reserva com lançador e processos falsos: nenhum `claude` de verdade sobe aqui.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use super::*;
use crate::runtime::claude_launch::Launch;
use crate::runtime::process::Cano;
use crate::session_open::{OpenError, SessionOpenRequest};

/// Pid acima do `pid_max` do Linux: nunca é de um processo vivo.
const FAKE_PID: u32 = 5_000_000;

struct Fake {
    root: PathBuf,
    spawns: Mutex<Vec<(String, String)>>,
    next_pid: AtomicU32,
    /// Com a casa: o `spawn` passa antes pelo começo real da abertura (login, guarda, `--prep`).
    host: Option<crate::session_open::local::LocalHost>,
    /// Presa pelo teste: o `spawn` espera nela depois de anotar a subida (o `PUT` fica em curso).
    hold: tokio::sync::Mutex<()>,
    plans: AtomicU32,
}

impl Fake {
    fn new(root: &Path) -> Self {
        Self { root: root.to_owned(), spawns: Mutex::default(), next_pid: AtomicU32::new(FAKE_PID), host: None, hold: Default::default(), plans: AtomicU32::new(0) }
    }
    fn account(&self, request: &SessionOpenRequest) -> PathBuf {
        request.config_dir.clone().map_or_else(|| self.root.join(".claude"), PathBuf::from)
    }
    fn spawned(&self) -> Vec<(String, String)> { self.spawns.lock().unwrap().clone() }
}

impl Launcher for Fake {
    fn new_session_id(&self) -> String { crate::session_open::open::new_session_id() }

    fn plan<'a>(&'a self, request: &'a SessionOpenRequest, sid: &'a str) -> BoxFuture<'a, Result<Planned, OpenError>> {
        Box::pin(async move {
            self.plans.fetch_add(1, Ordering::SeqCst);
            if request.cwd == "/recusada" { return Err(OpenError::new(400, "erro_cwd_inexistente", "a pasta não existe")); }
            if request.cwd == "/ocupada" { return Err(OpenError::new(503, "claude_customizations_busy", "ocupado")); }
            let config_dir = self.account(request);
            let meta = json!({"cwd": request.cwd, "key": crate::mods::state::random_hex(16), "name": format!("pw-{}", &sid[..8]),
                "session_id": sid, "config_dir": config_dir, "model": request.model, "permission_mode": "bypassPermissions"});
            let spec = crate::session_open::record::claude_from_meta(&meta, "pw", 0.0).unwrap();
            // A impressão real vem do comando; aqui o modelo e a conta bastam para mudá-la.
            let launch = Launch { program: vec!["claude".into(), "--session-id".into(), sid.into(), "--model".into(),
                request.model.clone().unwrap_or_default()], env: BTreeMap::from([("CLAUDE_CONFIG_DIR".into(), config_dir.display().to_string()),
                ("CP_SESSION_KEY".into(), spec.key.clone())]) };
            Ok(Planned { fingerprint: fingerprint(&launch, &request.cwd), session_id: sid.into(), home: self.root.clone(),
                account: crate::accounts::AccountKey::new(crate::accounts::Provider::Claude, &config_dir).unwrap(), config_dir, launch, spec,
                previous_non_plan: None, settings: None })
        })
    }

    fn spawn<'a>(&'a self, planned: &'a Planned, _sidecar_dir: &'a Path) -> BoxFuture<'a, Result<Spawned, OpenError>> {
        Box::pin(async move {
            let guard = match &self.host {
                Some(host) => super::prepare(host, &planned.spec).await?,
                None => None,
            };
            self.spawns.lock().unwrap().push((planned.spec.key.clone(), planned.session_id.clone()));
            drop(self.hold.lock().await);
            let pid = self.next_pid.fetch_add(1, Ordering::SeqCst);
            let cano = Cano { pid, escuta: format!("unix:/nao-existe/{pid}.sock"), token: "segredo".into(), ts: 0.0, versao: 2,
                extra: serde_json::Map::from_iter([("process_birth".into(), json!(1.0))]) };
            Ok(Spawned { cano, guard })
        })
    }
}

#[derive(Default)]
struct Procs {
    killed: Mutex<Vec<u32>>,
    dead: Mutex<Vec<u32>>,
    /// O nome observado no `kill`: se ainda estava reservado naquela hora.
    watch: Mutex<Option<String>>,
    held_at_kill: Mutex<Vec<bool>>,
}

impl Processes for Procs {
    fn alive<'a>(&'a self, cano: &'a Cano, _key: &'a str) -> BoxFuture<'a, bool> {
        Box::pin(async move { !self.dead.lock().unwrap().contains(&cano.pid) })
    }
    fn kill<'a>(&'a self, cano: &'a Cano, _key: &'a str, _dir: &'a Path) -> BoxFuture<'a, Result<(), &'static str>> {
        Box::pin(async move {
            if let Some(name) = self.watch.lock().unwrap().clone() {
                self.held_at_kill.lock().unwrap().push(crate::session_open::open::NameClaim::opening(&name));
            }
            self.killed.lock().unwrap().push(cano.pid);
            Ok(())
        })
    }
}

struct World {
    _tmp: tempfile::TempDir,
    root: PathBuf,
    fake: Fake,
    procs: Arc<Procs>,
    diary: Arc<Mutex<Vec<Value>>>,
    clock: Arc<AtomicU64>,
    store: Arc<Store>,
}

fn world() -> World {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let procs = Arc::new(Procs::default());
    let diary: Arc<Mutex<Vec<Value>>> = Arc::default();
    let clock = Arc::new(AtomicU64::new(0));
    let (sink, offset, base) = (diary.clone(), clock.clone(), Instant::now());
    let store = Store::new(root.join(".hangar/prewarm"), root.join(".hangar/claude-headless"), procs.clone(),
        Arc::new(move |event| sink.lock().unwrap().push(event)))
        .with_clock(Arc::new(move || base + Duration::from_secs(offset.load(Ordering::SeqCst))));
    World { fake: Fake::new(&root), root, _tmp: tmp, procs, diary, clock, store: Arc::new(store) }
}

impl World {
    fn request(&self, model: &str) -> SessionOpenRequest {
        SessionOpenRequest { cwd: self.root.join("proj").display().to_string(), model: Some(model.into()), ..Default::default() }
    }
    async fn reserve(&self, model: &str, client: &str) -> Ticket {
        self.store.reserve(&self.fake, &self.request(model), client).await.unwrap()
    }
    fn at(&self, secs: u64) { self.clock.store(secs, Ordering::SeqCst); }
    fn killed(&self) -> Vec<u32> { self.procs.killed.lock().unwrap().clone() }
    fn events(&self, etapa: &str) -> Vec<Value> { self.diary.lock().unwrap().iter().filter(|e| e["etapa"] == etapa).cloned().collect() }
    fn jsonl(&self, sid: &str) -> PathBuf {
        let cwd = self.root.join("proj").display().to_string();
        self.root.join(".claude/projects").join(hangar_workspace::worktrees::sanitize_cwd(&cwd)).join(format!("{sid}.jsonl"))
    }
    fn write(&self, path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

#[test]
fn out_of_scope_requests_are_refused_with_422() {
    let base = SessionOpenRequest { cwd: "/tmp".into(), ..Default::default() };
    assert!(scope(&base).is_ok());
    let cases: Vec<SessionOpenRequest> = vec![
        SessionOpenRequest { provider: Some("codex".into()), ..base.clone() },
        SessionOpenRequest { headless: Some(false), ..base.clone() },
        SessionOpenRequest { engine: Some("kimi".into()), ..base.clone() },
        SessionOpenRequest { branch: Some("b".into()), ..base.clone() },
        SessionOpenRequest { new_branch: true, ..base.clone() },
        SessionOpenRequest { base: Some("main".into()), ..base.clone() },
        SessionOpenRequest { link: true, ..base.clone() },
        SessionOpenRequest { creator: Some("mae".into()), ..base.clone() },
        SessionOpenRequest { read_only: true, ..base.clone() },
    ];
    for request in cases {
        let error = scope(&request).unwrap_err();
        assert_eq!((error.status, error.code), (422, Some("erro_prewarm_nao_suportado")), "{request:?}");
    }
    assert_eq!(scope(&SessionOpenRequest { name: Some("x".into()), ..base }).unwrap_err().code, Some("erro_corpo_invalido"));
}

#[test]
fn the_fingerprint_ignores_identity_and_sees_the_rest() {
    let launch = |sid: &str, key: &str, model: &str| Launch {
        program: ["claude", "-p", "--session-id", sid, "--model", model].map(str::to_owned).into(),
        env: BTreeMap::from([("CP_SESSION_NAME", key), ("CP_SESSION_KEY", key), ("HANGAR_CANO_KEY", key), ("HANGAR_PLUGIN_TOKEN", key),
            ("CLAUDE_CONFIG_DIR", "/c")].map(|(k, v)| (k.to_owned(), v.to_owned()))),
    };
    let one = fingerprint(&launch("a", "k1", "opus"), "/p");
    assert_eq!(one, fingerprint(&launch("b", "k2", "opus"), "/p"), "id da conversa, nome, chave e token não contam");
    assert_ne!(one, fingerprint(&launch("a", "k1", "sonnet"), "/p"), "o modelo conta");
    assert_ne!(one, fingerprint(&launch("a", "k1", "opus"), "/q"), "a pasta conta");
}

#[test]
fn the_adoption_spec_ignores_identity_and_sees_the_rest() {
    let spec = |key: &str, name: &str, sid: &str, model: &str| {
        let meta = json!({"cwd": "/p", "key": key, "name": name, "session_id": sid, "model": model});
        let mut spec = crate::session_open::record::claude_from_meta(&meta, name, 0.0).unwrap();
        spec.conversation = Some(sid.into());
        spec
    };
    let one = adoption_spec(&spec("k1", "a", "s1", "opus"));
    let mut other = spec("k2", "b", "s2", "opus");
    other.created_at = 99.0;
    assert_eq!(one, adoption_spec(&other), "chave, nome, conversa e hora não contam");
    assert_ne!(one, adoption_spec(&spec("k1", "a", "s1", "sonnet")), "o modelo conta");
    let mut chosen = spec("k1", "a", "s1", "opus");
    chosen.customizations = Some(serde_json::from_value(json!({"plugins": {"x@y": false}})).unwrap());
    assert_ne!(one, adoption_spec(&chosen), "as escolhas de plugins da conversa contam");
}

/// Step 1.
#[tokio::test]
async fn same_fingerprint_renews_and_a_new_one_replaces_the_process() {
    let w = world();
    let first = w.reserve("opus", "tela").await;
    assert_eq!(w.reserve("opus", "tela").await, first, "a mesma reserva");
    assert_eq!(w.fake.spawned().len(), 1, "renovar não sobe outro processo");
    let second = w.reserve("sonnet", "tela").await;
    assert_ne!(second.id, first.id);
    assert_ne!(second.fingerprint, first.fingerprint);
    assert_eq!(w.fake.spawned().len(), 2);
    assert_eq!(w.killed(), vec![FAKE_PID], "a velha morreu");
    assert_eq!(w.store.len(), 1);
    assert!(!w.root.join(format!(".hangar/prewarm/{}.json", first.id)).exists(), "recibo da velha saiu");
    let receipt: Value = serde_json::from_slice(&std::fs::read(w.root.join(format!(".hangar/prewarm/{}.json", second.id))).unwrap()).unwrap();
    assert_eq!(receipt["cano"]["pid"], FAKE_PID + 1);
    assert_eq!(receipt["cano"]["token"], "", "o recibo não guarda o token do cano");
}

#[tokio::test]
async fn renewal_with_the_same_body_only_extends_the_deadline() {
    let w = world();
    let first = w.reserve("opus", "tela").await;
    w.at(60);
    assert_eq!(w.reserve("opus", "tela").await, first);
    assert_eq!(w.fake.plans.load(Ordering::SeqCst), 1, "o mesmo corpo não resolve de novo");
    w.at(120);
    w.store.sweep().await;
    assert_eq!(w.store.len(), 1, "o prazo andou");
    w.reserve("sonnet", "tela").await;
    assert!(w.fake.plans.load(Ordering::SeqCst) > 1, "corpo novo resolve");
    assert_eq!(w.fake.spawned().len(), 2);
}

#[tokio::test]
async fn a_dead_process_is_replaced_on_renewal() {
    let w = world();
    let first = w.reserve("opus", "tela").await;
    w.procs.dead.lock().unwrap().push(FAKE_PID);
    let second = w.reserve("opus", "tela").await;
    assert_ne!(second.id, first.id);
    assert_eq!(w.fake.spawned().len(), 2);
}

#[tokio::test]
async fn a_request_the_opening_refuses_drops_the_old_reserve_and_returns_the_same_error() {
    let w = world();
    w.reserve("opus", "tela").await;
    let refused = SessionOpenRequest { cwd: "/recusada".into(), ..Default::default() };
    let error = w.store.reserve(&w.fake, &refused, "tela").await.unwrap_err();
    assert_eq!((error.status, error.code), (400, Some("erro_cwd_inexistente")));
    assert!(w.store.is_empty());
}

#[tokio::test]
async fn a_server_failure_on_renewal_keeps_the_reserve_and_returns_the_error() {
    let w = world();
    let ticket = w.reserve("opus", "tela").await;
    let busy = SessionOpenRequest { cwd: "/ocupada".into(), ..Default::default() };
    let error = w.store.reserve(&w.fake, &busy, "tela").await.unwrap_err();
    assert_eq!((error.status, error.code), (503, Some("claude_customizations_busy")));
    assert_eq!(w.store.name_of(&ticket.id).as_deref(), Some(ticket.name.as_str()), "a reserva fica");
    assert!(w.killed().is_empty());
    assert_eq!(w.reserve("opus", "tela").await, ticket, "e segue renovável");
}

#[tokio::test]
async fn delete_by_client_releases_its_reserve_even_without_the_id() {
    let w = world();
    let mine = w.reserve("opus", "tela").await;
    let other = w.reserve("opus", "outra").await;
    w.store.release("-", Some("tela")).await;
    assert!(w.store.name_of(&mine.id).is_none() && w.store.name_of(&other.id).is_some());
    assert_eq!(w.killed(), vec![FAKE_PID]);
    w.store.release(&other.id, None).await;
    assert!(w.store.is_empty(), "sem client_id solta pelo id");
}

/// A tela que passou a pedir worktree (fora do escopo) recebe o 422 e perde a reserva que tinha.
#[tokio::test]
async fn a_put_out_of_scope_releases_the_reserve_of_that_client() {
    let w = world();
    let mine = w.reserve("opus", "tela").await;
    let other = w.reserve("opus", "outra").await;
    let request = SessionOpenRequest { branch: Some("feat".into()), ..w.request("opus") };
    let error = w.store.reserve(&w.fake, &request, "tela").await.unwrap_err();
    assert_eq!((error.status, error.code), (422, Some("erro_prewarm_nao_suportado")));
    assert!(w.store.name_of(&mine.id).is_none() && w.store.name_of(&other.id).is_some(), "só a da tela sai");
    assert_eq!(w.killed(), vec![FAKE_PID]);
    assert!(w.events("descartada").iter().any(|e| e["motivo"] == "fora_do_escopo"), "{:?}", w.events("descartada"));
}

/// O nome solta antes do kill: a reserva já saiu do mapa, e uma abertura com esse nome não pode dar 409.
#[tokio::test]
async fn the_name_is_released_before_the_process_is_killed() {
    let w = world();
    let ticket = w.reserve("opus", "tela").await;
    *w.procs.watch.lock().unwrap() = Some(ticket.name.clone());
    assert!(w.store.discard(&ticket.id, "teste").await);
    assert_eq!(*w.procs.held_at_kill.lock().unwrap(), vec![false], "o nome estava preso durante o kill");
    assert!(!crate::session_open::open::NameClaim::opening(&ticket.name));
}

#[tokio::test]
async fn delete_during_a_put_in_flight_kills_what_it_spawned() {
    let w = world();
    let held = w.fake.hold.lock().await;
    let request = w.request("opus");
    let put = w.store.reserve(&w.fake, &request, "tela");
    let delete = async {
        while w.fake.spawned().is_empty() { tokio::task::yield_now().await; }
        w.store.release("-", Some("tela")).await;
        drop(held);
    };
    let (result, ()) = tokio::join!(put, delete);
    assert_eq!(result.unwrap_err().code, Some("erro_prewarm_solta"));
    assert!(w.store.is_empty());
    assert_eq!(w.killed(), vec![FAKE_PID], "o processo da subida em curso morreu");
    w.reserve("opus", "tela").await;
    assert_eq!(w.store.len(), 1, "o PUT seguinte não é solto");
}

#[tokio::test]
async fn take_treats_an_expired_reserve_as_absent_and_discards_it() {
    let w = world();
    let ticket = w.reserve("opus", "tela").await;
    w.at(90);
    assert!(w.store.take(&ticket.id).is_none());
    assert!(!crate::session_open::open::NameClaim::opening(&ticket.name), "o nome solta na hora");
    for _ in 0..200 {
        if !w.killed().is_empty() { break; }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert_eq!(w.killed(), vec![FAKE_PID]);
    assert!(w.store.is_empty());
}

/// Step 2.
#[tokio::test]
async fn without_renewal_for_90s_the_reserve_goes_and_only_an_empty_transcript_with_it() {
    let w = world();
    let quiet = w.reserve("opus", "a").await;
    let talked = w.reserve("opus", "b").await;
    let sid = |key: &str| w.fake.spawned().into_iter().find(|(k, _)| k == key).unwrap().1;
    let (quiet_sid, talked_sid) = (sid(&quiet.id), sid(&talked.id));
    w.write(&w.jsonl(&quiet_sid), "{\"type\":\"attachment\",\"attachment\":{}}\n");
    w.write(&w.jsonl(&talked_sid), "{\"type\":\"attachment\"}\n{\"type\":\"user\",\"message\":{}}\n");
    for dir in [".hangar-active", ".hangar-state", ".hangar-prompt"] {
        w.write(&w.root.join(".claude").join(dir).join(format!("{quiet_sid}.json")), "{}");
    }
    let choices = w.root.join(format!(".hangar/claude-customizations/{quiet_sid}.json"));
    w.write(&choices, "{\"settings\":{}}");
    w.at(89);
    w.store.sweep().await;
    assert_eq!(w.store.len(), 2, "antes do prazo fica");
    w.at(90);
    w.store.sweep().await;
    assert!(w.store.is_empty());
    assert!(!w.jsonl(&quiet_sid).exists(), "transcript sem mensagem do usuário some");
    assert!(w.jsonl(&talked_sid).exists(), "transcript com mensagem do usuário fica");
    for dir in [".hangar-active", ".hangar-state", ".hangar-prompt"] {
        assert!(!w.root.join(".claude").join(dir).join(format!("{quiet_sid}.json")).exists(), "{dir}");
    }
    assert!(!choices.exists(), "as escolhas da conversa saem junto");
    assert_eq!(w.events("expirada").len(), 2);
    assert_eq!(w.events("expirada")[0]["motivo"], "sem_renovacao");
}

/// Step 3.
#[tokio::test]
async fn limits_per_server_per_client_and_max_life() {
    let w = world();
    let mut tickets = Vec::new();
    for (i, client) in ["a", "b", "c", "d"].iter().enumerate() {
        w.at(i as u64);
        tickets.push(w.reserve("opus", client).await);
    }
    assert_eq!(w.store.len(), 3, "a 4ª tira a mais velha");
    assert!(w.store.take(&tickets[0].id).is_none());
    assert_eq!(w.killed(), vec![FAKE_PID]);

    let w = world();
    let first = w.reserve("opus", "tela").await;
    let second = w.reserve("sonnet", "tela").await;
    assert_eq!(w.store.len(), 1, "uma por client_id");
    assert!(w.store.take(&first.id).is_none() && w.store.take(&second.id).is_some());

    let w = world();
    let ticket = w.reserve("opus", "tela").await;
    for t in (80..600).step_by(80) {
        w.at(t);
        assert_eq!(w.reserve("opus", "tela").await, ticket, "renovada aos {t} s");
        w.store.sweep().await;
    }
    w.at(599);
    w.store.sweep().await;
    assert_eq!(w.store.len(), 1);
    w.at(600);
    w.store.sweep().await;
    assert!(w.store.is_empty(), "renovada, morre aos 10 min");
    assert_eq!(w.events("expirada")[0]["motivo"], "vida_maxima");
}

/// Step 4.
#[tokio::test]
async fn a_receipt_left_by_a_crashed_server_is_cleaned_on_startup() {
    let w = world();
    let cwd = w.root.join("proj").display().to_string();
    let (sid, key) = ("11111111-2222-4333-8444-555555555555", "ab".repeat(16));
    let receipt = json!({"key": key, "session_id": sid, "name": "pw-x", "cwd": cwd, "config_dir": w.root.join(".claude"),
        "cano": {"pid": FAKE_PID + 7, "escuta": "unix:/nao-existe.sock", "token": "", "ts": 0.0, "versao": 2, "process_birth": 1.0}});
    w.write(&w.root.join(format!(".hangar/prewarm/{key}.json")), &receipt.to_string());
    w.write(&w.jsonl(sid), "{\"type\":\"attachment\"}\n");
    w.write(&w.root.join(format!(".claude/.hangar-active/{sid}.json")), "{}");
    // A já adotada (chave num sidecar) só perde o recibo.
    let adopted = "cd".repeat(16);
    let mut kept = receipt.clone();
    kept["key"] = json!(adopted);
    kept["cano"]["pid"] = json!(FAKE_PID + 8);
    w.write(&w.root.join(format!(".hangar/prewarm/{adopted}.json")), &kept.to_string());
    w.write(&w.root.join(".hangar/claude-headless/pw-x.json"), &json!({"key": adopted}).to_string());

    assert_eq!(w.store.clean_receipts().await, 1);
    assert_eq!(w.killed(), vec![FAKE_PID + 7], "a adotada não é morta");
    assert!(!w.jsonl(sid).exists());
    assert!(!w.root.join(format!(".claude/.hangar-active/{sid}.json")).exists());
    assert!(std::fs::read_dir(w.root.join(".hangar/prewarm")).unwrap().next().is_none(), "nenhum recibo sobra");
    assert_eq!(w.events("descartada")[0]["motivo"], "sobra_da_subida");
}

/// Step 5: o começo é o da abertura, então a recusa também. No macOS a credencial mora no Keychain.
#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn an_account_without_login_is_refused_like_the_opening() {
    let mut w = world();
    let account = w.root.join(".claude-semlogin");
    std::fs::create_dir_all(&account).unwrap();
    std::fs::write(account.join(crate::accounts::storage::CLAUDE_MARKER), "").unwrap();
    let env = BTreeMap::from([("HOME".to_owned(), w.root.display().to_string()), ("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
    w.fake.host = Some(crate::session_open::local::LocalHost::new(env, None));
    let request = SessionOpenRequest { config_dir: Some(account.display().to_string()), ..w.request("opus") };
    let error = w.store.reserve(&w.fake, &request, "tela").await.unwrap_err();
    assert_eq!((error.status, error.code), (409, Some("erro_conta_sem_login")));
    assert!(w.store.is_empty() && w.fake.spawned().is_empty());
}

/// K09: a reserva reconcilia a conta do Hangar no próprio processo (o atalho aparece) e os avisos vão ao
/// log dela.
#[cfg(target_os = "linux")]
#[tokio::test]
async fn the_reserve_reconciles_the_hangar_account_and_logs_its_warnings() {
    #[derive(Clone)] struct Capture(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Capture {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> { self.0.lock().unwrap().extend_from_slice(data); Ok(data.len()) }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }
    let output = Arc::new(Mutex::new(Vec::new()));
    let capture = Capture(output.clone());
    let subscriber = tracing_subscriber::fmt().without_time().with_ansi(false).with_writer(move || capture.clone()).finish();
    let _log = tracing::subscriber::set_default(subscriber);
    let mut w = world();
    w.write(&w.root.join(".claude/CLAUDE.md"), "# regras");
    w.write(&w.root.join(".claude/skills/falar/SKILL.md"), "oi");
    let account = w.root.join(".claude-conta2");
    w.write(&account.join(crate::accounts::storage::CLAUDE_MARKER), "");
    w.write(&account.join(".credentials.json"), r#"{"claudeAiOauth": {"accessToken": "a", "refreshToken": "r"}}"#);
    w.write(&account.join("CLAUDE.md"), "# editado na conta");
    std::fs::create_dir_all(w.root.join("proj")).unwrap();
    let env = BTreeMap::from([("HOME".to_owned(), w.root.display().to_string()), ("PATH".to_owned(), "/usr/bin:/bin".to_owned())]);
    w.fake.host = Some(crate::session_open::local::LocalHost::new(env, None));
    let request = SessionOpenRequest { config_dir: Some(account.display().to_string()), ..w.request("opus") };
    w.store.reserve(&w.fake, &request, "tela").await.unwrap();
    assert_eq!(std::fs::read_link(account.join("skills")).unwrap(), w.root.join(".claude/skills"), "a conta foi reconciliada");
    let text = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    assert!(text.lines().any(|l| l.contains("prewarm_aviso") && l.contains("'CLAUDE.md' foi alterado dentro desta conta")),
        "o aviso da reconciliação não foi ao log da reserva: {text}");
}

#[tokio::test]
async fn deleting_an_account_discards_only_its_reserves() {
    let w = world();
    let other = w.root.join(".claude-outra");
    std::fs::create_dir_all(&other).unwrap();
    let mine = w.reserve("opus", "a").await;
    let theirs = w.store.reserve(&w.fake, &SessionOpenRequest { config_dir: Some(other.display().to_string()), ..w.request("opus") }, "b")
        .await.unwrap();
    let key = crate::accounts::AccountKey::new(crate::accounts::Provider::Claude, &other).unwrap();
    w.store.discard_account(&key, "conta_excluida").await;
    assert!(w.store.take(&theirs.id).is_none());
    assert!(w.store.take(&mine.id).is_some());
    assert_eq!(w.killed(), vec![FAKE_PID + 1]);
}

/// O contrato da adoção: tirar é atômico e o nome segue reservado com quem tirou.
#[tokio::test]
async fn take_is_atomic_and_the_name_goes_with_the_reserve() {
    let w = world();
    let ticket = w.reserve("opus", "tela").await;
    let reserve = w.store.take(&ticket.id).expect("primeiro leva");
    assert_eq!(ticket.name, reserve.name, "o PUT devolve o nome da reserva");
    assert!(w.store.take(&ticket.id).is_none(), "o segundo não");
    assert!(crate::session_open::open::NameClaim::opening(&reserve.name));
    assert_eq!(reserve.cano.token, "segredo", "a adoção recebe o cano inteiro");
    let name = reserve.name.clone();
    w.store.adopted(&reserve.key, &name).await;
    drop(reserve);
    assert!(!crate::session_open::open::NameClaim::opening(&name));
    assert!(!w.root.join(format!(".hangar/prewarm/{}.json", ticket.id)).exists());
    assert!(w.killed().is_empty(), "adotada não morre");
}

#[tokio::test]
async fn a_name_typed_over_a_reserve_discards_it() {
    let w = world();
    let ticket = w.reserve("opus", "tela").await;
    let name = w.store.name_of(&ticket.id).unwrap();
    assert!(w.store.discard_name(&name, "nome_digitado").await);
    assert!(w.store.is_empty() && !crate::session_open::open::NameClaim::opening(&name));
    assert!(!w.store.discard_name(&name, "nome_digitado").await);
}

#[tokio::test]
async fn a_clean_exit_discards_every_reserve() {
    let w = world();
    w.reserve("opus", "a").await;
    w.reserve("opus", "b").await;
    w.store.discard_all("saida_do_servidor").await;
    assert!(w.store.is_empty());
    assert_eq!(w.killed().len(), 2);
}
