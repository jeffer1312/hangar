//! `PUT`/`DELETE /api/sessions/{name}/auto-continue`: o interruptor do continuar sozinho mora no spec da
//! sessão e só o dono mexe nele; o Cancelar troca no lugar a nota "continuo quando a rede voltar".
#![cfg(unix)]
use crate::fake;

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use fake::{OWNER, client};
use hangar_server::accounts::{AccountService, environment::AccountEnvironment};
use hangar_server::list::bridge::{ListBridge, ListEnv, parse_dirs};
use hangar_server::list::facts::FactsClient;
use hangar_server::list::mux::Mux;
use hangar_server::routes::{AppState, router};
use hangar_server::runtime::gateway::RuntimeRegistry;
use hangar_server::session_open::{SessionChange, record};
use serde_json::{Value, json};

const KEY: &str = "0123456789abcdef0123456789abcdef";
/// Convidado com login que vê as sessões do dono: passa o porteiro e chega à rota.
const GUEST: &str = "convidado-que-ve";

struct Srv { addr: SocketAddr, home: PathBuf, _tasks: Vec<tokio::task::JoinHandle<()>>, _dir: tempfile::TempDir }

fn write(path: PathBuf, value: &Value) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn read(path: PathBuf) -> Option<Value> { std::fs::read(path).ok().and_then(|raw| serde_json::from_slice(&raw).ok()) }

/// `ledger`: o `auto-continue.json` gravado antes de a agenda subir (a espera que ela retoma). `guest`: cadastra
/// o convidado com login `GUEST`.
async fn server(ledger: Option<Value>, guest: bool) -> Srv {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let home = root.join("home");
    let claude = home.join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    if let Some(ledger) = ledger { write(home.join(".hangar/auto-continue.json"), &ledger); }
    // Sem sessão com terminal: as sessões do teste são sem terminal, descobertas pelo sidecar.
    let script = root.join("tmux");
    crate::write_executable(&script, "#!/bin/sh\ncase \"$1\" in\n  has-session) exit 1 ;;\n  display-message) echo 1700000000 ;;\n  *) exit 0 ;;\nesac\n");
    let dirs = parse_dirs(&json!({"home": home, "claude": claude, "codex_home": home.join(".codex"),
        "pi_sessions": home.join(".pi/agent/sessions"), "omp_config": home.join(".omp"),
        "omp_agent": home.join(".omp/agent"), "kimi_home": home.join(".kimi-code")}).to_string());
    let env = AccountEnvironment::from_map(BTreeMap::from([("HOME".to_owned(), home.to_string_lossy().into_owned())]));
    let mut state = AppState::new(fake::config(""));
    state.hangar_dir = home.join(".hangar");
    state.mission = Arc::new(hangar_mission::Store::new(state.hangar_dir.join("mission")));
    state.accounts = AccountService::new(env.clone());
    if guest {
        use hangar_server::login_guests::{Service, Settings};
        let service = Service::new(Settings { guests: home.join("guests.json"), vault: home.join("vault.json"),
            runtime_config: home.join("runtime-config.json"), sync_enabled: false, bootstrap: String::new(),
            secret: b"fixture".to_vec(), secure: false, rate_max: 10.into(), rate_window: 300.into() });
        service.store.create("Bia", home.to_str().unwrap(), true, true, "bia".into(), GUEST.into()).unwrap();
        state.share = None;
        state.login_guests = Arc::new(service);
    }
    state.list = Arc::new(ListBridge::new(ListEnv { mux: Mux::with_program(&script, Duration::from_secs(5)),
        capture_program: script.into_os_string(), procs: Arc::new(hangar_server::list::procs::SystemProcs::default()), dirs },
        FactsClient::new()));
    // A fila da adormecida é a do arquivo da casa do teste.
    assert!(state.state.runtime.set(Arc::new(RuntimeRegistry::new("i".into()).with_codex_environment(env))).is_ok());
    let state = Arc::new(state);
    let tasks = hangar_server::auto_continue::spawn(state.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = router(state);
    tokio::spawn(async move { axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>()).await.unwrap() });
    Srv { addr, home, _tasks: tasks, _dir: dir }
}

impl Srv {
    fn sidecar_path(&self, name: &str) -> PathBuf { self.home.join(format!(".hangar/claude-headless/{name}.json")) }
    fn spec_path(&self) -> PathBuf { self.home.join(format!(".hangar/session-specs/{KEY}.json")) }
    fn spec(&self) -> Value { read(self.spec_path()).unwrap() }

    /// Claude sem terminal com registro; `asleep`: adormecida, a nota vai à fila pelo arquivo.
    fn headless(&self, name: &str, asleep: bool) {
        let cwd = self.home.join("w");
        std::fs::create_dir_all(&cwd).unwrap();
        let mut meta = json!({"name": name, "key": KEY, "session_id": "11111111-2222-4333-8444-555555555555", "cwd": cwd});
        if asleep { meta["asleep"] = json!({"wake_surface": "headless"}); }
        write(self.sidecar_path(name), &meta);
        write(self.spec_path(), &json!({"version": 1, "key": KEY, "name": name, "cwd": cwd, "provider": "claude", "surface": "headless",
            "account": {"kind": "default"}, "account_source": "default", "jev": false, "read_only": false, "created_at": 1.0}));
    }

    async fn answer(resp: reqwest::Response) -> (u16, Value) {
        let status = resp.status().as_u16();
        (status, serde_json::from_str(&resp.text().await.unwrap()).unwrap_or(Value::Null))
    }

    async fn switch(&self, name: &str, token: &str, body: Value) -> (u16, Value) {
        Self::answer(client().put(format!("http://{}/api/sessions/{name}/auto-continue", self.addr)).bearer_auth(token)
            .header("content-type", "application/json").body(body.to_string()).send().await.unwrap()).await
    }

    async fn cancel(&self, name: &str, token: &str) -> (u16, Value) {
        Self::answer(client().delete(format!("http://{}/api/sessions/{name}/auto-continue", self.addr)).bearer_auth(token)
            .send().await.unwrap()).await
    }
}

/// Os `state` de toda nota do continuar sozinho guardada no arquivo da fila.
fn note_states(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            if map.get("kind") == Some(&json!("auto_continue")) && let Some(state) = map.get("state").and_then(Value::as_str) {
                out.push(state.to_owned());
            }
            map.values().for_each(|v| note_states(v, out));
        }
        Value::Array(items) => items.iter().for_each(|v| note_states(v, out)),
        _ => {}
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_owner_turns_it_off_and_a_rewrite_from_an_older_spec_keeps_it() {
    let srv = server(None, false).await;
    srv.headless("s", false);
    // A reabertura que leu o registro antes da rota e grava o dela depois.
    let before = record::read(&srv.home, KEY).unwrap().unwrap();
    assert_eq!(srv.switch("s", OWNER, json!({"enabled": false})).await, (200, json!({"enabled": false})));
    assert_eq!(srv.spec()["auto_continue_off"], json!(true));
    let change: SessionChange = serde_json::from_value(json!({"reason": "reload"})).unwrap();
    record::write(&srv.home, &before.with(&change)).unwrap();
    assert_eq!(srv.spec()["auto_continue_off"], json!(true), "regravar o spec lido antes da rota não religa");
    let recorded = record::read(&srv.home, KEY).unwrap().unwrap();
    assert!(recorded.with(&change).auto_continue_off, "a reabertura parte do spec gravado");
    let live = record::claude_live(&srv.home, &read(srv.sidecar_path("s")).unwrap(), "s").unwrap();
    assert!(live.auto_continue_off, "o spec remontado do sidecar para relançar conserva o interruptor");
    assert_eq!(srv.switch("s", OWNER, json!({"enabled": true})).await, (200, json!({"enabled": true})));
    assert!(srv.spec().get("auto_continue_off").is_none(), "ligado de novo, o campo sai do registro");
    let (status, body) = srv.switch("nenhuma", OWNER, json!({"enabled": false})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (404, json!("session_not_found")));
    let (status, body) = srv.switch("s", OWNER, json!({"ligado": false})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (422, json!("erro_corpo_invalido")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_guest_cannot_switch_it() {
    let srv = server(None, true).await;
    srv.headless("s", false);
    let (status, body) = srv.switch("s", GUEST, json!({"enabled": false})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (403, json!("erro_so_dono")));
    let (status, body) = srv.cancel("s", GUEST).await;
    assert_eq!((status, body["detail"]["code"].clone()), (403, json!("erro_so_dono")));
    assert!(srv.spec().get("auto_continue_off").is_none(), "a recusa não grava nada");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_token_gets_the_counted_401() {
    let srv = server(None, false).await;
    srv.headless("s", false);
    let (status, body) = srv.switch("s", "errado", json!({"enabled": false})).await;
    assert_eq!((status, body["detail"]["code"].clone()), (401, json!("erro_nao_autorizado")));
    let (status, body) = srv.cancel("s", "errado").await;
    assert_eq!((status, body["detail"]["code"].clone()), (401, json!("erro_nao_autorizado")));
    assert!(srv.spec().get("auto_continue_off").is_none(), "a recusa não grava nada");
}

#[tokio::test(flavor = "multi_thread")]
async fn cancel_rewrites_the_waiting_note() {
    // A agenda só olha a idade da parada no tique, bem depois do Cancelar.
    let hit = json!({"name": "s", "provider": "claude", "kind": "network", "line_id": "line-1", "at": 1_700_000_000.0, "life": format!("k:{KEY}")});
    let srv = server(Some(json!({"sessions": {"s": {"waiting": {"hit": hit, "count": 1, "note_out": true}}}})), false).await;
    srv.headless("s", true);
    // A resposta sai depois de a nota trocar.
    assert_eq!(srv.cancel("s", OWNER).await, (200, json!({"cancelled": true})));
    let queue = read(srv.home.join(format!(".claude/.hangar-queue/runtime/{KEY}.json"))).expect("a nota foi à fila da adormecida");
    let mut states = Vec::new();
    note_states(&queue, &mut states);
    assert!(states.iter().any(|state| state == "cancelled"), "a nota de espera virou cancelada: {states:?}");
    let ledger = read(srv.home.join(".hangar/auto-continue.json")).unwrap();
    assert!(ledger["sessions"]["s"].get("waiting").is_none(), "a espera sai do registro: {ledger}");
    assert_eq!(srv.cancel("s", OWNER).await, (200, json!({"cancelled": false})), "sem espera, nada a cancelar");
}
