//! Varredura de membro morto e de grupo `orq` sozinho: tempo de ausência, lista que falha e janela
//! de lançamento, sobre arquivos reais e um ambiente falso.
mod fake;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use hangar_server::groups::exit::leave_and_notify;
use hangar_server::groups::orq::PythonOrq;
use hangar_server::groups::service::{BoxFuture, GroupError, GroupService, JoinOwned, OrqFacts, OrqPhase, PromoteError};
use hangar_server::groups::store::PairDir;
use hangar_server::groups::sweep::{FAILED_EVENT, PANICKED_EVENT, RECOVERED_EVENT, SweepEnv, Sweeper, live_names_of};
use hangar_server::list::bridge::Produced;
use hangar_server::list::facts::ListFacts;

/// Resposta da lista falsa que faz a rodada entrar em pânico.
const PANIC: &str = "panic";

struct Phase(Mutex<BTreeMap<String, OrqPhase>>);
impl OrqFacts for Phase {
    fn phase<'a>(&'a self, gid: &'a str) -> BoxFuture<'a, OrqPhase> {
        Box::pin(async move { self.0.lock().unwrap().get(gid).copied().unwrap_or(OrqPhase::NotStarted) })
    }
    fn promote<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Result<(), PromoteError>> { Box::pin(async { Ok(()) }) }
}

struct Env {
    groups: Arc<GroupService>,
    live: Mutex<Result<Vec<String>, String>>,
    list_calls: Mutex<u32>,
    left: Mutex<Vec<String>>,
    reports: Mutex<Vec<(&'static str, String)>>,
}
impl SweepEnv for Env {
    fn live_names(&self) -> BoxFuture<'_, Result<Vec<String>, String>> {
        *self.list_calls.lock().unwrap() += 1;
        let answer = self.live.lock().unwrap().clone();
        if answer.as_ref().err().map(String::as_str) == Some(PANIC) { panic!("rodada quebrada"); }
        Box::pin(async move { answer })
    }
    fn leave<'a>(&'a self, name: &'a str) -> BoxFuture<'a, Result<Vec<String>, GroupError>> {
        self.left.lock().unwrap().push(name.to_owned());
        Box::pin(async move { self.groups.leave(name).await })
    }
    fn report(&self, event: &'static str, code: &str) { self.reports.lock().unwrap().push((event, code.to_owned())); }
}

struct Rig { _tmp: tempfile::TempDir, root: std::path::PathBuf, groups: Arc<GroupService>, env: Arc<Env>, phases: Arc<Phase>, sweeper: Sweeper }

fn rig() -> Rig {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("pair");
    let phases = Arc::new(Phase(Mutex::default()));
    let groups = Arc::new(GroupService::new(PairDir::new(root.clone(), tmp.path().join("arquivo")), phases.clone(), "casa".into()));
    let env = Arc::new(Env { groups: groups.clone(), live: Mutex::new(Ok(vec!["x".into()])), list_calls: Mutex::default(), left: Mutex::default(), reports: Mutex::default() });
    let sweeper = Sweeper::new(groups.clone(), env.clone());
    Rig { _tmp: tmp, root, groups, env, phases, sweeper }
}

async fn join(groups: &GroupService, name: &str, others: &[&str], orq: bool) -> String {
    groups.join(JoinOwned { name: name.into(), others: others.iter().map(|o| o.to_string()).collect(), task: String::new(),
        replace_task: false, harness: BTreeMap::new(), orq }).await.unwrap().gid
}

fn live(env: &Env, names: &[&str]) { *env.live.lock().unwrap() = Ok(names.iter().map(|n| n.to_string()).collect()); }
fn sidecar_exists(root: &Path, stem: &str) -> bool { root.join(format!("{stem}.json")).exists() }
fn age(root: &Path, stem: &str, by: Duration) {
    let file = std::fs::File::options().write(true).open(root.join(format!("{stem}.json"))).unwrap();
    file.set_modified(SystemTime::now() - by).unwrap();
}

#[tokio::test(start_paused = true)]
async fn dead_member_leaves_after_five_seconds_of_absence() {
    let mut r = rig();
    join(&r.groups, "a", &["b"], false).await;
    live(&r.env, &["a"]);
    r.sweeper.round().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    r.sweeper.round().await;
    assert!(r.env.left.lock().unwrap().is_empty(), "com 4 s de ausência ainda fica");
    assert!(sidecar_exists(&r.root, "b"));
    tokio::time::advance(Duration::from_secs(1)).await;
    r.sweeper.round().await;
    assert_eq!(*r.env.left.lock().unwrap(), ["b"]);
    // Os dois lados do par somem: sobrou um membro só.
    assert!(!sidecar_exists(&r.root, "b") && !sidecar_exists(&r.root, "a"));
}

#[tokio::test(start_paused = true)]
async fn absence_restarts_when_the_session_comes_back() {
    let mut r = rig();
    join(&r.groups, "a", &["b"], false).await;
    live(&r.env, &["a"]);
    r.sweeper.round().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    live(&r.env, &["a", "b"]);
    r.sweeper.round().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    live(&r.env, &["a"]);
    r.sweeper.round().await;
    tokio::time::advance(Duration::from_secs(4)).await;
    r.sweeper.round().await;
    assert!(r.env.left.lock().unwrap().is_empty(), "o relógio da ausência recomeça quando ela volta");
}

#[tokio::test(start_paused = true)]
async fn sweep_skips_failed_or_empty_list() {
    let mut r = rig();
    join(&r.groups, "a", &["b"], false).await;
    for answer in [Err("mux_unavailable".to_owned()), Err("mux_unavailable".to_owned()), Ok(vec![])] {
        *r.env.live.lock().unwrap() = answer;
        r.sweeper.round().await;
        tokio::time::advance(Duration::from_secs(30)).await;
    }
    assert!(r.env.left.lock().unwrap().is_empty());
    assert!(sidecar_exists(&r.root, "a") && sidecar_exists(&r.root, "b"));
    // Uma linha por sequência: a mesma falha repetida não repete o diário; outro código, sim.
    assert_eq!(*r.env.reports.lock().unwrap(), [(FAILED_EVENT, "mux_unavailable".to_owned()), (FAILED_EVENT, "list_empty".to_owned())]);
    live(&r.env, &["a", "b"]);
    r.sweeper.round().await;
    r.sweeper.round().await;
    assert_eq!(r.env.reports.lock().unwrap().last(), Some(&(RECOVERED_EVENT, "list_empty".to_owned())));
    assert_eq!(r.env.reports.lock().unwrap().len(), 3);
}

/// Rodada que entra em pânico não para a varredura para sempre: vai ao diário e a próxima roda.
#[tokio::test]
async fn panicking_round_goes_to_the_diary_and_the_sweep_goes_on() {
    let r = rig();
    join(&r.groups, "a", &["b"], false).await;
    *r.env.live.lock().unwrap() = Err(PANIC.to_owned());
    let sweeper = r.sweeper.guarded_round().await;
    assert_eq!(*r.env.reports.lock().unwrap(), [(PANICKED_EVENT, "groups_sweep_panicked".to_owned())]);
    live(&r.env, &["a", "b"]);
    let _ = sweeper.guarded_round().await;
    assert_eq!(*r.env.list_calls.lock().unwrap(), 2, "a rodada seguinte pergunta a lista de novo");
}

/// Sem nenhuma resposta dos fatos, a sessão vista só por eles (transferência, `orq`) é desconhecida,
/// não ausente: a rodada falha como a lista que não respondeu.
#[test]
fn list_without_facts_is_a_failed_round() {
    let rows = Arc::new(vec![serde_json::from_value::<hangar_api::session::SessionRow>(serde_json::json!({"name": "a", "provider": "claude"})).unwrap()]);
    let unknown = Produced { rows: rows.clone(), facts: Arc::new(ListFacts::default()), facts_ok: false };
    assert_eq!(live_names_of(&unknown), Err("list_facts_unknown".to_owned()));
    let known = Produced { rows, facts: Arc::new(ListFacts { unknown: false, ..Default::default() }), facts_ok: true };
    assert_eq!(live_names_of(&known), Ok(vec!["a".to_owned()]));
}

#[tokio::test(start_paused = true)]
async fn sanitized_name_counts_as_alive() {
    let mut r = rig();
    join(&r.groups, "nome com espaço", &["x"], false).await;
    live(&r.env, &["nome com espaço", "x"]);
    r.sweeper.round().await;
    tokio::time::advance(Duration::from_secs(10)).await;
    r.sweeper.round().await;
    assert!(r.env.left.lock().unwrap().is_empty(), "o stem saneado do sidecar não é um fantasma");
    // Morta de verdade: o nome cru e o stem saem juntos pela mesma ausência.
    live(&r.env, &["x"]);
    r.sweeper.round().await;
    tokio::time::advance(Duration::from_secs(5)).await;
    r.sweeper.round().await;
    assert!(!r.env.left.lock().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn lone_orq_group_dissolves_only_when_run_ended_or_grace_passed() {
    let mut r = rig();
    let mut gids = BTreeMap::new();
    for name in ["ended", "live", "unknown", "young", "old"] {
        let gid = join(&r.groups, name, &[], true).await;
        std::fs::write(r.groups.contract_path(&gid), "contrato").unwrap();
        gids.insert(name, gid);
    }
    {
        let mut phases = r.phases.0.lock().unwrap();
        phases.insert(gids["ended"].clone(), OrqPhase::Ended);
        phases.insert(gids["live"].clone(), OrqPhase::Live);
        phases.insert(gids["unknown"].clone(), OrqPhase::Unknown);
    }
    age(&r.root, "old", Duration::from_secs(3601));
    age(&r.root, "young", Duration::from_secs(3500));
    age(&r.root, "live", Duration::from_secs(7200));
    age(&r.root, "unknown", Duration::from_secs(7200));
    live(&r.env, &["ended", "live", "unknown", "young", "old"]);
    r.sweeper.round().await;
    assert!(!sidecar_exists(&r.root, "ended"), "execução acabada");
    assert!(!sidecar_exists(&r.root, "old"), "nunca iniciada e fora da janela de lançamento");
    for kept in ["live", "unknown", "young"] {
        assert!(sidecar_exists(&r.root, kept), "{kept} fica");
    }
    assert!(!r.groups.contract_path(&gids["ended"]).exists(), "o contrato foi para o arquivo");
    let archived = std::fs::read_dir(r._tmp.path().join("arquivo")).unwrap().count();
    assert_eq!(archived, 2, "ended e old");
}

#[tokio::test(start_paused = true)]
async fn no_sidecar_no_list_request() {
    let mut r = rig();
    r.sweeper.round().await;
    std::fs::create_dir_all(&r.root).unwrap();
    std::fs::write(r.root.join("external_pairs.json"), "{}").unwrap();
    r.sweeper.round().await;
    assert_eq!(*r.env.list_calls.lock().unwrap(), 0);
    join(&r.groups, "a", &["b"], false).await;
    r.sweeper.round().await;
    assert_eq!(*r.env.list_calls.lock().unwrap(), 1);
}

/// A varredura só conhece o stem saneado do sidecar; o Python acha o registro pelo nome cru.
#[tokio::test(flavor = "multi_thread")]
async fn dead_session_with_sanitized_name_ends_its_external_pair_by_the_raw_name() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("pair");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("nome-com-espa-o.json"), serde_json::json!({"peers": ["pc-ana::Y"], "task": "", "gid": "abababab", "harness": {}}).to_string()).unwrap();
    std::fs::write(root.join("external_pairs.json"), serde_json::json!([{"share_id": "sh1", "local_session": "nome com espaço", "alias": "pc-ana",
        "peer_owner": "Ana", "peer_session": "Y", "peer_address": "https://a.tail.ts.net:8443", "peer_token": "tok", "created_at": 1.0}]).to_string()).unwrap();
    let (python, upstream) = fake::spawn_fake().await;
    let mut state = hangar_server::routes::AppState::new(fake::config(upstream, ""));
    let groups = Arc::new(GroupService::new(PairDir::new(root.clone(), tmp.path().join("arquivo")),
        Arc::new(PythonOrq::from_state(&state)), "casa".into()));
    state.groups = Some(groups.clone());
    let (ex, warnings) = leave_and_notify(&state, &groups, "nome-com-espa-o").await.unwrap();
    assert_eq!(ex, ["pc-ana::Y"]);
    assert_eq!(python.internal_bodies("external-pairs/end"), vec![serde_json::json!({"name": "nome com espaço", "peer": "pc-ana::Y"})]);
    // Tratado como máquina própria, o aviso falharia (nenhuma máquina cadastrada).
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(!root.join("nome-com-espa-o.json").exists());
}
