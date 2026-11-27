//! Fechar pelo dono uma sessão que o próprio Rust encerra: a saída do grupo que falha vai no `warning`
//! da resposta, como no fechamento que o Python fazia, sem passar por ele. Fechar o pai, só com a morte
//! confirmada, interrompe as filhas ligadas e as deixa abertas, marcadas sem pai; a varredura só marca.
#![cfg(unix)]
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hangar_server::groups::exit::{leave_and_notify, leave_dead_member};
use hangar_server::groups::orq::OrqVault;
use hangar_server::groups::service::{BoxFuture, GroupError, GroupService, JoinOwned};
use hangar_server::groups::store::PairDir;
use hangar_server::groups::sweep::{MIN_ABSENCE, SweepEnv, Sweeper};
use serde_json::{Value, json};

use crate::fake::{self, OWNER};
use crate::runtime_codex_watch::{finish, fixture, tui};
use crate::runtime_open::{app_server_process, fake_app_server, registry};

#[tokio::test]
async fn closing_a_rust_owned_grouped_session_returns_the_leave_warning() {
    let fx = fixture();
    let app_server = fake_app_server(true).await;
    let (mut app, mut tui) = (app_server_process(&app_server.endpoint), tui());
    fx.put_sidecar("cx-proof", fx.meta("cx-proof", &app_server.endpoint, Some(app.id()), &tui));
    fx.panes(&["cx-proof"]);
    let registry = registry().await;
    let watcher = fx.watcher();
    registry.bind_watcher(watcher.clone());
    watcher.round(&registry).await;
    assert!(registry.owns_codex_terminal("cx-proof"));

    let dir = tempfile::Builder::new().prefix("grp-sf-close-").tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(fake::config("127.0.0.1"));
    assert!(state.state.runtime.set(registry.clone()).is_ok());
    let groups = Arc::new(GroupService::new(PairDir::new(dir.path().join("pair"), dir.path().join("arquivo")),
        Arc::new(OrqVault::from_state(&state)), "casa".into()));
    // Par em outra máquina que não está entre os peers: o aviso de saída não sai.
    let harness: BTreeMap<String, String> = [("cx-proof", "codex"), ("longe::x", "claude")].into_iter().map(|(n, h)| (n.into(), h.into())).collect();
    groups.join(JoinOwned { name: "cx-proof".into(), others: vec!["longe::x".into()], task: String::new(), replace_task: false,
        harness, orq: false, outside: Default::default(), linked: false }).await.ok().unwrap();
    state.groups = Some(groups);
    let server = fake::spawn_state(state).await;

    let response = fake::client().delete(format!("http://{server}/api/sessions/cx-proof")).bearer_auth(OWNER).send().await.unwrap();
    assert_eq!(response.status().as_u16(), 200);
    let body: Value = serde_json::from_str(&response.text().await.unwrap()).unwrap_or(Value::Null);
    assert!(!registry.owns_codex_terminal("cx-proof"), "fechada no Rust");
    assert_eq!(body["warning"]["code"], "erro_pareamento_saida_falhou", "{body}");
    assert_eq!(body["warning"]["params"]["avisos"][0]["erro"]["code"], "erro_peer_nao_avisado", "{body}");
    finish(&mut [&mut app, &mut tui]);
}

/// Pai `pai` (pane de outro programa, que o Rust fecha pelo nome) com as filhas ligadas `filha` (Claude sem
/// terminal aberta no Rust, parada) e `fantasma` (nenhuma sessão com esse nome), nos arquivos de grupo que a lista lê.
async fn family() -> (crate::session_close_by_name::Env, tempfile::TempDir) {
    let e = crate::session_close_by_name::env_full(|_| {}, "x", |state| {
        let home = state.list.env().dirs.as_ref().unwrap().home.clone();
        state.hangar_dir = home.join(".hangar");
        let vault = Arc::new(OrqVault::from_state(state));
        state.groups = Some(Arc::new(GroupService::new(PairDir::new(home.join(".claude/.hangar-pair"), home.join("pair-arquivo")), vault, "casa".into())));
    }).await;
    let pair = e.home.join(".claude/.hangar-pair");
    std::fs::create_dir_all(&pair).unwrap();
    let harness = json!({"pai": "claude", "filha": "claude", "fantasma": "claude"});
    for (name, peers, parent) in [("pai", json!(["filha", "fantasma"]), None), ("filha", json!(["pai", "fantasma"]), Some("pai")),
        ("fantasma", json!(["pai", "filha"]), Some("pai"))] {
        let mut sidecar = json!({"peers": peers, "task": "", "gid": "g1", "harness": harness, "lead": "pai"});
        if let Some(parent) = parent { sidecar["linked_by"] = json!(parent); }
        std::fs::write(pair.join(format!("{name}.json")), sidecar.to_string()).unwrap();
    }
    let dir = tempfile::tempdir().unwrap();
    open_child(&e, dir.path()).await;
    e.sessions(&["pai"]);
    (e, dir)
}

async fn open_child(e: &crate::session_close_by_name::Env, dir: &Path) {
    crate::session_write_control::open_headless(&e.registry, dir, "filha").await;
}

fn sidecar(e: &crate::session_close_by_name::Env, name: &str) -> Option<Value> {
    std::fs::read(e.home.join(".claude/.hangar-pair").join(format!("{name}.json"))).ok().map(|raw| serde_json::from_slice(&raw).unwrap())
}

fn orphans(home: &Path) -> hangar_delegation::orphans::Orphans { hangar_delegation::orphans::read(&home.join(".hangar")).unwrap() }

#[tokio::test]
async fn closing_a_parent_interrupts_and_keeps_its_linked_child_marked_orphan() {
    let (e, _dir) = family().await;
    let (status, body) = e.delete("pai").await;
    assert_eq!(status, 200, "{body}");
    assert!(e.alive().is_empty(), "o pai fechou");
    // A filha parada conta como interrompida; a que não existe é a falha, no aviso do fechamento e no diário.
    let warnings = body["warning"]["params"]["avisos"].as_array().cloned().unwrap_or_default();
    assert_eq!(warnings.iter().map(|w| w["sessao"].as_str().unwrap_or_default()).collect::<Vec<_>>(), ["fantasma"], "{body}");
    assert_eq!(warnings[0]["erro"]["code"], "erro_limpeza_fechamento_falhou", "{body}");
    let code = warnings[0]["erro"]["params"]["codigo"].as_str().filter(|c| !c.is_empty()).expect("a falha leva o código").to_owned();
    assert!(e.diary_has("fantasma", &code).await, "{:?}", e.python.diag());
    assert!(e.registry.writable("filha").await.is_some(), "a filha segue aberta");
    let marks = orphans(&e.home);
    assert_eq!((marks.get("filha").map(|o| o.parent.as_str()), marks.get("fantasma").map(|o| o.parent.as_str())), (Some("pai"), Some("pai")));
    let child = sidecar(&e, "filha").expect("a filha segue no grupo com a irmã");
    assert_eq!(child["peers"], json!(["fantasma"]));
    assert!(child.get("linked_by").is_none(), "sem pai na lista: {child}");
    assert!(orphans(&e.home).get("pai").is_none(), "o pai fechado não vira órfão");
}

/// O convidado alcança o pai, não a árvore dele: fechar o pai não interrompe ninguém, e as filhas ficam
/// marcadas sem pai como em qualquer saída do grupo.
#[tokio::test]
async fn a_guest_closing_a_parent_does_not_interrupt_its_children() {
    let (e, _dir) = family().await;
    let (status, body) = e.delete_as("pai", crate::session_close_by_name::GUEST).await;
    assert_eq!((status, &body), (200, &json!({"ok": true, "warning": null})), "a filha que não existe nem foi procurada: {body}");
    assert!(e.alive().is_empty(), "o pai fechou");
    assert!(e.registry.writable("filha").await.is_some(), "a filha segue aberta");
    let marks = orphans(&e.home);
    assert_eq!((marks.get("filha").map(|o| o.parent.as_str()), marks.get("fantasma").map(|o| o.parent.as_str())), (Some("pai"), Some("pai")));
    assert!(!e.python.diag().iter().any(|l| l["evento"] == "rust.link_cascade_failed"), "ninguém foi interrompido: {:?}", e.python.diag());
}

#[tokio::test]
async fn an_unconfirmed_kill_leaves_the_children_linked() {
    let (e, _dir) = family().await;
    std::fs::write(e.root.join("survive"), "").unwrap();
    let (status, body) = e.delete("pai").await;
    assert_eq!((status, body["detail"]["code"].as_str()), (500, Some("session_kill_unconfirmed")), "{body}");
    assert!(e.diary_has("pai", "session_kill_unconfirmed").await, "{:?}", e.python.diag());
    let child = sidecar(&e, "filha").expect("o grupo fica");
    assert_eq!(child["linked_by"], "pai", "ligada ao pai vivo: {child}");
    assert!(child["peers"].as_array().unwrap().iter().any(|p| p == "pai"));
    assert!(orphans(&e.home).is_empty(), "sem órfã");
    assert!(!e.python.diag().iter().any(|l| l["evento"] == "rust.link_cascade_failed"), "ninguém foi interrompido: {:?}", e.python.diag());
}

/// A varredura de verdade (`leave_dead_member`) sobre uma lista falsa.
struct DeadEnv { st: Arc<hangar_server::routes::AppState>, groups: Arc<GroupService>, live: Vec<String> }

impl SweepEnv for DeadEnv {
    fn live_names(&self) -> BoxFuture<'_, Result<Vec<String>, String>> { let live = self.live.clone(); Box::pin(async move { Ok(live) }) }
    fn leave<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, GroupError>> {
        Box::pin(async move { leave_dead_member(&self.st, &self.groups, name).await.map(|(ex, _)| ex) })
    }
    fn report(&self, _: &'static str, _: &str) {}
}

/// O pai morto `pai:um` tem o sidecar `pai-um.json`; a varredura passa pelo stem antes do nome cru, e a
/// saída pelo stem desfaz o grupo de dois. A órfã guarda o nome que a filha conhecia.
#[tokio::test(start_paused = true)]
async fn the_dead_member_sweep_marks_the_children_orphan() {
    let tmp = tempfile::tempdir().unwrap();
    let pair = tmp.path().join("pair");
    std::fs::create_dir_all(&pair).unwrap();
    let harness = json!({"pai:um": "claude", "filha": "claude"});
    std::fs::write(pair.join("pai-um.json"), json!({"peers": ["filha"], "task": "", "gid": "g2", "harness": harness}).to_string()).unwrap();
    std::fs::write(pair.join("filha.json"), json!({"peers": ["pai:um"], "task": "", "gid": "g2", "harness": harness, "linked_by": "pai:um"}).to_string()).unwrap();
    let mut state = hangar_server::routes::AppState::new(fake::config(""));
    state.hangar_dir = tmp.path().join("hangar");
    let groups = Arc::new(GroupService::new(PairDir::new(pair.clone(), tmp.path().join("arquivo")), Arc::new(OrqVault::from_state(&state)), "casa".into()));
    state.groups = Some(groups.clone());
    let st = Arc::new(state);
    let mut sweeper = Sweeper::new(groups.clone(), Arc::new(DeadEnv { st: st.clone(), groups, live: vec!["filha".into()] }));
    sweeper.round().await;
    assert!(pair.join("filha.json").exists(), "ausente há menos que o prazo, o grupo fica");
    tokio::time::advance(MIN_ABSENCE).await;
    sweeper.round().await;
    let marks = hangar_delegation::orphans::read(&st.hangar_dir).unwrap();
    assert_eq!(marks.get("filha").map(|o| o.parent.as_str()), Some("pai:um"), "{marks:?}");
    assert!(!pair.join("filha.json").exists(), "o grupo de dois acabou: a filha sai sem pai");
}

/// Quem fecha sabe o nome cru: `a-b` não leva as filhas de `a b`, que divide o stem com ele e segue ligada.
#[tokio::test]
async fn closing_by_the_raw_name_does_not_mark_the_children_of_a_same_stem_session() {
    let tmp = tempfile::tempdir().unwrap();
    let pair = tmp.path().join("pair");
    std::fs::create_dir_all(&pair).unwrap();
    let harness = json!({"a b": "claude", "f": "claude", "g": "claude"});
    std::fs::write(pair.join("a-b.json"), json!({"peers": ["f", "g"], "task": "", "gid": "g3", "harness": harness}).to_string()).unwrap();
    std::fs::write(pair.join("f.json"), json!({"peers": ["a b", "g"], "task": "", "gid": "g3", "harness": harness, "linked_by": "a b"}).to_string()).unwrap();
    std::fs::write(pair.join("g.json"), json!({"peers": ["a b", "f"], "task": "", "gid": "g3", "harness": harness}).to_string()).unwrap();
    let mut state = hangar_server::routes::AppState::new(fake::config(""));
    state.hangar_dir = tmp.path().join("hangar");
    let groups = Arc::new(GroupService::new(PairDir::new(pair.clone(), tmp.path().join("arquivo")), Arc::new(OrqVault::from_state(&state)), "casa".into()));
    state.groups = Some(groups.clone());
    leave_and_notify(&state, &groups, "a-b", "test").await.unwrap();
    assert!(hangar_delegation::orphans::read(&state.hangar_dir).unwrap().is_empty(), "sem órfã");
    let child: Value = serde_json::from_slice(&std::fs::read(pair.join("f.json")).unwrap()).unwrap();
    assert_eq!(child["linked_by"], "a b", "{child}");
}

/// O nome reusado por uma sessão nova não herda a marca da vida que acabou; as outras ficam.
#[tokio::test]
async fn a_reused_name_drops_the_old_orphan_mark() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = hangar_server::routes::AppState::new(fake::config(""));
    state.hangar_dir = tmp.path().join("hangar");
    hangar_delegation::orphans::record(&state.hangar_dir, "pai", &["filha".to_owned(), "irma".to_owned()], 1.0).unwrap();
    hangar_server::groups::link_orphans::reused(&state, "filha").await;
    assert_eq!(hangar_delegation::orphans::read(&state.hangar_dir).unwrap().into_keys().collect::<Vec<_>>(), ["irma"]);
}

/// Lista falsa e marcas de órfã fixas: só a marca de quem sumiu da lista por `MIN_ABSENCE` sai.
struct OrphanEnv { live: Vec<String>, marked: Vec<String>, forgot: Mutex<Vec<String>>, reported: Mutex<Vec<String>> }

impl SweepEnv for OrphanEnv {
    fn live_names(&self) -> BoxFuture<'_, Result<Vec<String>, String>> { let live = self.live.clone(); Box::pin(async move { Ok(live) }) }
    fn leave<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Vec<String>, GroupError>> { Box::pin(async { Ok(Vec::new()) }) }
    fn orphans(&self) -> BoxFuture<'_, Vec<String>> { let marked = self.marked.clone(); Box::pin(async move { marked }) }
    fn forget_orphans<'a>(&'a self, names: &'a [String]) -> BoxFuture<'a, ()> {
        self.forgot.lock().unwrap().extend(names.iter().cloned());
        Box::pin(async {})
    }
    fn report(&self, event: &'static str, code: &str) { self.reported.lock().unwrap().push(format!("{event} {code}")); }
}

#[tokio::test(start_paused = true)]
async fn the_sweep_forgets_an_orphan_only_after_it_is_gone_from_the_list() {
    let tmp = tempfile::tempdir().unwrap();
    let groups = Arc::new(GroupService::new(PairDir::new(tmp.path().join("pair"), tmp.path().join("arquivo")),
        Arc::new(OrqVault::from_state(&hangar_server::routes::AppState::new(fake::config("")))), "casa".into()));
    let env = Arc::new(OrphanEnv { live: vec!["viva".into()], marked: vec!["viva".into(), "sumiu".into()], forgot: Mutex::default(),
        reported: Mutex::default() });
    let mut sweeper = Sweeper::new(groups, env.clone());
    sweeper.round().await;
    tokio::time::advance(MIN_ABSENCE - Duration::from_secs(1)).await;
    sweeper.round().await;
    assert!(env.forgot.lock().unwrap().is_empty(), "ausente há menos que o prazo, a marca fica");
    tokio::time::advance(Duration::from_secs(1)).await;
    sweeper.round().await;
    assert_eq!(*env.forgot.lock().unwrap(), ["sumiu"], "a da lista viva fica marcada");
}

/// Só marcas de órfã e a lista vazia: nenhuma linha de "lista indisponível" no diário, e as marcas ficam.
#[tokio::test(start_paused = true)]
async fn an_empty_list_with_only_orphan_marks_is_not_a_list_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let groups = Arc::new(GroupService::new(PairDir::new(tmp.path().join("pair"), tmp.path().join("arquivo")),
        Arc::new(OrqVault::from_state(&hangar_server::routes::AppState::new(fake::config("")))), "casa".into()));
    let env = Arc::new(OrphanEnv { live: Vec::new(), marked: vec!["sumiu".into()], forgot: Mutex::default(), reported: Mutex::default() });
    let mut sweeper = Sweeper::new(groups, env.clone());
    sweeper.round().await;
    tokio::time::advance(MIN_ABSENCE).await;
    sweeper.round().await;
    assert!(env.reported.lock().unwrap().is_empty(), "{:?}", env.reported.lock().unwrap());
    assert!(env.forgot.lock().unwrap().is_empty(), "lista vazia nunca tira marca");
}
