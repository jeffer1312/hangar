use hangar_server::groups::local::*;
use hangar_server::groups::model::Sidecar;
use hangar_server::groups::service::{BoxFuture, GroupError, GroupService, JoinOwned, OrqFacts, OrqPhase, PromoteError};
use hangar_server::groups::store::PairDir;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

struct Disk(RefCell<BTreeMap<String, Sidecar>>);
impl Disk {
    fn new() -> Self { Disk(RefCell::new(BTreeMap::new())) }
    fn read(&self) -> impl Fn(&str) -> Option<Sidecar> + '_ { move |n| self.0.borrow().get(n).cloned() }
    fn apply(&self, writes: &[(String, Sidecar)], clears: &[String]) {
        let mut d = self.0.borrow_mut();
        for c in clears { d.remove(c); }
        for (n, s) in writes { d.insert(n.clone(), s.clone()); }
    }
}
fn none() -> BTreeMap<String, String> { BTreeMap::new() }
fn gid(s: &'static str) -> impl Fn() -> String { move || s.to_owned() }
fn input<'a>(name: &'a str, others: &'a [String], task: &'a str, h: &'a BTreeMap<String, String>) -> JoinInput<'a> {
    JoinInput { name, others, task, replace_task: false, harness: h, orq: false }
}

#[test]
fn two_solo_sessions_become_a_group() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "t", &h), &gid("g1")).ok().unwrap();
    assert_eq!(p.members, ["a", "b"]);
    assert!(p.new_gid);
    assert_eq!(p.task, "t");
    d.apply(&p.writes, &[]);
    assert_eq!(d.read()("b").unwrap().peers, ["a"]);
}

#[test]
fn third_member_keeps_gid() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let p = plan_join(&d.read(), &input("c", &["a".into()], "", &h), &gid("g2")).ok().unwrap();
    assert_eq!(p.gid, "g1");
    assert!(!p.new_gid);
    assert_eq!(p.members.len(), 3);
}

#[test]
fn different_task_without_replace_is_refused_before_any_write() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "x", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    match plan_join(&d.read(), &input("c", &["a".into()], "y", &h), &gid("g2")) {
        Err(JoinRefusal::TaskConflict { existing }) => assert_eq!(existing, "x"),
        _ => panic!("tarefa diferente sem replace_task precisa ser recusada"),
    }
}

#[test]
fn different_task_with_replace_wins_and_empty_task_inherits() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "x", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let inherit = plan_join(&d.read(), &input("c", &["a".into()], "", &h), &gid("g2")).ok().unwrap();
    assert_eq!(inherit.task, "x");
    let others = ["a".to_owned()];
    let replace = JoinInput { replace_task: true, ..input("c", &others, "y", &h) };
    assert_eq!(plan_join(&d.read(), &replace, &gid("g2")).ok().unwrap().task, "y");
}

#[test]
fn fusion_keeps_first_gid_and_lists_loser() {
    let (d, h) = (Disk::new(), none());
    for (a, b, g) in [("a", "b", "g1"), ("c", "d", "g2")] {
        let p = plan_join(&d.read(), &input(a, &[b.into()], "", &h), &gid(g)).ok().unwrap();
        d.apply(&p.writes, &[]);
    }
    let p = plan_join(&d.read(), &input("a", &["c".into()], "", &h), &gid("g3")).ok().unwrap();
    assert_eq!((p.gid.as_str(), p.merged.as_slice()), ("g1", &["g2".to_owned()][..]));
}

#[test]
fn external_or_remote_member_never_enters_a_local_group() {
    let (d, h) = (Disk::new(), none());
    d.apply(&[("a".into(), Sidecar { peers: vec!["fulano::x".into()], task: Some(String::new()), gid: "e1".into(), ..Default::default() })], &[]);
    assert!(matches!(plan_join(&d.read(), &input("b", &["a".into()], "", &h), &gid("g")), Err(JoinRefusal::Mix)));
}

#[test]
fn one_local_and_one_remote_is_a_valid_cross_machine_pair() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["srv::x".into()], "", &h), &gid("g1")).ok().unwrap();
    assert_eq!(p.members, ["a", "srv::x"]);
    assert_eq!(p.writes.len(), 1, "o remoto vive na máquina dele e não ganha sidecar aqui");
    assert_eq!(p.writes[0].0, "a");
    assert_eq!(p.writes[0].1.peers, ["srv::x"]);
    assert_eq!(p.before.keys().collect::<Vec<_>>(), ["a"]);
    // Dois remotos, ou um remoto com grupo local de mais de uma sessão, continua recusado.
    assert!(matches!(plan_join(&d.read(), &input("a", &["srv::x".into(), "srv::y".into()], "", &h), &gid("g")), Err(JoinRefusal::Mix)));
    assert!(matches!(plan_join(&d.read(), &input("a", &["b".into(), "srv::x".into()], "", &h), &gid("g")), Err(JoinRefusal::Mix)));
}

#[test]
fn harness_of_existing_members_survives_and_caller_wins() {
    let d = Disk::new();
    let first: BTreeMap<String, String> = [("a".into(), "claude".into()), ("b".into(), "codex".into())].into();
    let p = plan_join(&d.read(), &input("a", &["b".into()], "", &first), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let update: BTreeMap<String, String> = [("c".into(), "pi".into()), ("b".into(), "kimi".into())].into();
    let p = plan_join(&d.read(), &input("c", &["a".into()], "", &update), &gid("g2")).ok().unwrap();
    let harness = &p.writes[0].1.harness;
    assert_eq!((harness["a"].as_str(), harness["b"].as_str(), harness["c"].as_str()), ("claude", "kimi", "pi"));
}

#[test]
fn leave_of_pair_dissolves_and_archives() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let l = plan_leave(&d.read(), "a", false).unwrap();
    assert_eq!(l.ex_peers, ["b"]);
    assert_eq!(l.clears, ["a", "b"]);
    assert_eq!(l.archive.as_deref(), Some("g1"));
}

#[test]
fn leave_without_group_plans_nothing() {
    assert!(plan_leave(&Disk::new().read(), "a", false).is_none());
}

#[test]
fn leave_keeps_remaining_group() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into(), "c".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let l = plan_leave(&d.read(), "a", false).unwrap();
    d.apply(&l.writes, &l.clears);
    assert_eq!(d.read()("b").unwrap().peers, ["c"]);
    assert!(l.archive.is_none());
}

#[test]
fn live_orq_group_keeps_last_member() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &JoinInput { name: "arb", others: &["exec".into()], task: "", replace_task: false, harness: &h, orq: true }, &gid("o1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let l = plan_leave(&d.read(), "exec", true).unwrap();
    d.apply(&l.writes, &l.clears);
    let arb = d.read()("arb").unwrap();
    assert!(arb.peers.is_empty() && arb.orq, "grupo orq com execução viva fica com o árbitro sozinho");
    assert!(l.archive.is_none());
}

#[test]
fn rename_rewrites_own_and_peers() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let (clears, writes) = plan_rename(&d.read(), "a", "a2");
    d.apply(&writes, &clears);
    assert!(d.read()("a").is_none());
    assert_eq!(d.read()("a2").unwrap().peers, ["b"]);
    assert_eq!(d.read()("b").unwrap().peers, ["a2"]);
}

#[test]
fn external_link_writes_only_the_local_sidecar() {
    let d = Disk::new();
    let w = plan_external_link(&d.read(), "a", "fulano::x", &none(), &gid("e1")).ok().unwrap();
    assert_eq!(w.len(), 1);
    assert_eq!((w[0].0.as_str(), w[0].1.gid.as_str(), w[0].1.task.as_deref()), ("a", "e1", Some("")));
    assert_eq!(w[0].1.peers, ["fulano::x"]);
    d.apply(&w, &[]);
    assert!(matches!(plan_external_link(&d.read(), "a", "outro::y", &none(), &gid("e2")), Err(JoinRefusal::AlreadyGrouped)));
}

// ---- GroupService: disco real num tempdir e orquestração falsa ----

struct FakeOrq { phase: OrqPhase, conflict: Option<String>, promotes: AtomicUsize }
impl FakeOrq {
    fn new(phase: OrqPhase, conflict: Option<&str>) -> Arc<Self> {
        Arc::new(Self { phase, conflict: conflict.map(str::to_owned), promotes: AtomicUsize::new(0) })
    }
}
impl OrqFacts for FakeOrq {
    fn phase<'a>(&'a self, _gid: &'a str) -> BoxFuture<'a, OrqPhase> { Box::pin(async move { self.phase }) }
    fn promote<'a>(&'a self, _name: &'a str, _gid: &'a str) -> BoxFuture<'a, Result<(), PromoteError>> {
        Box::pin(async move {
            self.promotes.fetch_add(1, Ordering::SeqCst);
            self.conflict.clone().map_or(Ok(()), |text| Err(PromoteError::Conflict(text)))
        })
    }
}

fn service(tmp: &tempfile::TempDir, orq: Arc<FakeOrq>) -> (GroupService, PairDir) {
    let make = || PairDir::new(tmp.path().join("pair"), tmp.path().join("arquivo"));
    (GroupService::new(make(), orq, "srv".into()), make())
}
fn owned(name: &str, others: &[&str], orq: bool) -> JoinOwned {
    JoinOwned { name: name.into(), others: others.iter().map(|s| (*s).into()).collect(), task: String::new(), replace_task: false, harness: none(), orq }
}

#[tokio::test]
async fn join_restores_on_partial_write_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    svc.join(owned("a", &["z"], false)).await.unwrap();
    let (a_before, z_before) = (dir.sidecar("a").unwrap(), dir.sidecar("z").unwrap());
    // O temporário de `c` é uma pasta: a, z e b gravam e a escrita de `c` falha no fim.
    std::fs::create_dir(dir.root().join("c.json.tmp")).unwrap();
    let err = svc.join(owned("a", &["b", "c"], false)).await.err().expect("a escrita de c precisa falhar");
    assert!(matches!(err, GroupError::Store(_)));
    assert_eq!(dir.sidecar("a").unwrap(), a_before);
    assert_eq!(dir.sidecar("z").unwrap(), z_before);
    assert!(dir.sidecar("b").unwrap().is_none(), "sem grupo assimétrico");
}

#[tokio::test]
async fn orq_join_promotes_once_and_restores_on_conflict() {
    let tmp = tempfile::tempdir().unwrap();
    let orq = FakeOrq::new(OrqPhase::Ended, Some("arquivo mudou"));
    let (svc, dir) = service(&tmp, orq.clone());
    match svc.join(owned("arb", &["exec"], true)).await {
        Err(GroupError::Orq(failed)) => assert_eq!(failed, PromoteError::Conflict("arquivo mudou".into())),
        _ => panic!("conflito na promoção precisa voltar como GroupError::Orq"),
    }
    assert!(dir.sidecar("arb").unwrap().is_none() && dir.sidecar("exec").unwrap().is_none());
    assert_eq!(orq.promotes.load(Ordering::SeqCst), 1);

    let tmp = tempfile::tempdir().unwrap();
    let orq = FakeOrq::new(OrqPhase::Ended, None);
    let (svc, dir) = service(&tmp, orq.clone());
    let first = svc.join(owned("arb", &["exec"], true)).await.unwrap();
    assert!(first.orq && dir.sidecar("exec").unwrap().unwrap().orq);
    // Grupo que já tem gid não promove de novo.
    svc.join(owned("c", &["arb"], false)).await.unwrap();
    assert_eq!(orq.promotes.load(Ordering::SeqCst), 1);
}

/// Promoção que falha depois de deixar o temporário de `arb` virar pasta: a volta atrás não apaga o sidecar dele.
struct SabotagedOrq(std::path::PathBuf);
impl OrqFacts for SabotagedOrq {
    fn phase<'a>(&'a self, _gid: &'a str) -> BoxFuture<'a, OrqPhase> { Box::pin(async { OrqPhase::Ended }) }
    fn promote<'a>(&'a self, _name: &'a str, _gid: &'a str) -> BoxFuture<'a, Result<(), PromoteError>> {
        Box::pin(async move {
            std::fs::create_dir(self.0.join("arb.json.tmp")).unwrap();
            Err(PromoteError::Unavailable("groups_orq_promote_status_503".into()))
        })
    }
}

#[tokio::test]
async fn orq_join_whose_undo_fails_reports_the_disk_not_the_promote() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("pair");
    let svc = GroupService::new(PairDir::new(root.clone(), tmp.path().join("arquivo")), Arc::new(SabotagedOrq(root.clone())), "srv".into());
    let err = svc.join(owned("arb", &["exec"], true)).await.err().expect("a promoção falhou");
    // Responder 503 "tente de novo" com o sidecar `orq` ainda gravado seria dizer que nada ficou.
    assert!(matches!(err, GroupError::Store(_)), "{err:?}");
}

#[tokio::test]
async fn join_reports_only_loose_sessions_as_newcomers() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, _) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    let first = svc.join(owned("a", &["b"], false)).await.unwrap();
    assert_eq!(first.newcomers, ["a", "b"]);
    let second = svc.join(owned("c", &["a"], false)).await.unwrap();
    assert_eq!(second.newcomers, ["c"]);
    assert_eq!(second.gid, first.gid);
}

/// Cada escrita de grupo avisa a lista, para ela não servir o selo de antes.
#[tokio::test]
async fn every_group_write_tells_the_list() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, _) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    let bumps = Arc::new(AtomicUsize::new(0));
    let counter = bumps.clone();
    let svc = svc.with_change_hook(Arc::new(move || { counter.fetch_add(1, Ordering::SeqCst); }));
    let out = svc.join(owned("a", &["b"], false)).await.unwrap();
    assert_eq!(bumps.load(Ordering::SeqCst), 1, "join");
    svc.rename("a", "c").await.unwrap();
    assert_eq!(bumps.load(Ordering::SeqCst), 2, "rename");
    svc.leave("c").await.unwrap();
    assert_eq!(bumps.load(Ordering::SeqCst), 3, "leave");
    svc.leave("zzz").await.unwrap();
    assert_eq!(bumps.load(Ordering::SeqCst), 3, "sair sem grupo não escreve nada");
    svc.restore(out.before).await.unwrap();
    assert_eq!(bumps.load(Ordering::SeqCst), 4, "restore");
    svc.external_link("b", "m::x", none()).await.unwrap();
    svc.external_unlink("b", "m::x").await.unwrap();
    assert_eq!(bumps.load(Ordering::SeqCst), 6, "par externo");
}

#[tokio::test]
async fn restore_puts_sidecars_back_as_they_were() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    let out = svc.join(owned("a", &["b"], false)).await.unwrap();
    svc.restore(out.before).await.unwrap();
    assert!(dir.sidecar("a").unwrap().is_none() && dir.sidecar("b").unwrap().is_none());
}

#[tokio::test]
async fn leave_archives_contract_file() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    let out = svc.join(owned("a", &["b"], false)).await.unwrap();
    std::fs::write(dir.contract_path(&out.gid), "combinado\n").unwrap();
    assert_eq!(svc.leave("a").await.unwrap(), ["b"]);
    assert!(dir.sidecar("a").unwrap().is_none() && dir.sidecar("b").unwrap().is_none());
    assert!(!dir.contract_path(&out.gid).exists());
    let archived: Vec<String> = std::fs::read_dir(tmp.path().join("arquivo")).unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    assert_eq!(archived.len(), 1);
    assert!(archived[0].starts_with(&format!("grupo-{}-", out.gid)), "{archived:?}");
    // Idempotente: quem já saiu não tem ex-companheiros.
    assert!(svc.leave("a").await.unwrap().is_empty());
}

#[tokio::test]
async fn leave_counts_live_and_unknown_orq_as_alive() {
    for (phase, keeps) in [(OrqPhase::Live, true), (OrqPhase::Unknown, true), (OrqPhase::Ended, false), (OrqPhase::NotStarted, false)] {
        let tmp = tempfile::tempdir().unwrap();
        let (svc, dir) = service(&tmp, FakeOrq::new(phase, None));
        svc.join(owned("arb", &["exec"], true)).await.unwrap();
        svc.leave("exec").await.unwrap();
        assert_eq!(dir.sidecar("arb").unwrap().is_some(), keeps, "{phase:?}");
    }
}

#[tokio::test]
async fn rename_moves_sidecar_through_the_service() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    svc.join(owned("a", &["b"], false)).await.unwrap();
    svc.rename("a", "a2").await.unwrap();
    assert!(dir.sidecar("a").unwrap().is_none());
    assert_eq!(dir.sidecar("b").unwrap().unwrap().peers, ["a2"]);
}

#[tokio::test]
async fn join_refusal_leaves_the_disk_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    let mut first = owned("a", &["b"], false);
    first.task = "x".into();
    svc.join(first).await.unwrap();
    let mut other = owned("c", &["a"], false);
    other.task = "y".into();
    assert!(matches!(svc.join(other).await.err(), Some(GroupError::Refused(JoinRefusal::TaskConflict { .. }))));
    assert!(dir.sidecar("c").unwrap().is_none());
}

#[tokio::test]
async fn external_link_refuses_grouped_session() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    svc.join(owned("a", &["b"], false)).await.unwrap();
    assert!(matches!(svc.external_link("a", "fulano::x", none()).await.err(), Some(GroupError::Refused(JoinRefusal::AlreadyGrouped))));

    let gid = svc.external_link("solo", "fulano::x", none()).await.unwrap();
    let st = dir.sidecar("solo").unwrap().unwrap();
    assert_eq!((st.gid.as_str(), st.peers.as_slice()), (gid.as_str(), &["fulano::x".to_owned()][..]));
    assert_eq!(gid.len(), 8);
    svc.external_unlink("solo", "fulano::x").await.unwrap();
    assert!(dir.sidecar("solo").unwrap().is_none());
}
