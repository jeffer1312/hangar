use hangar_server::groups::local::*;
use hangar_server::groups::model::Sidecar;
use hangar_server::groups::service::{BoxFuture, GroupError, GroupService, JoinOwned, OrqFacts, OrqPhase, PromoteError};
use hangar_server::groups::store::PairDir;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashSet};
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
    JoinInput { name, others, task, replace_task: false, harness: h, orq: false, outside: &mine, linked: false }
}
fn mine(_: &str) -> bool { false }
/// `fulano::*` e `bia::*` são pares de fora; qualquer outro `x::y` é máquina própria.
fn outside(p: &str) -> bool { p.starts_with("fulano::") || p.starts_with("bia::") }

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
fn own_machine_remote_never_enters_a_local_group() {
    let (d, h) = (Disk::new(), none());
    d.apply(&[("a".into(), Sidecar { peers: vec!["srv::x".into()], task: Some(String::new()), gid: "e1".into(), ..Default::default() })], &[]);
    assert!(matches!(plan_join(&d.read(), &input("b", &["a".into()], "", &h), &gid("g")), Err(JoinRefusal::Mix)));
}

#[test]
fn outside_member_joins_like_a_regular_member() {
    let (d, h) = (Disk::new(), none());
    d.apply(&[("a".into(), Sidecar { peers: vec!["fulano::x".into()], task: Some(String::new()), gid: "e1".into(), ..Default::default() })], &[]);
    let others = ["a".to_owned()];
    let p = plan_join(&d.read(), &JoinInput { outside: &outside, ..input("b", &others, "", &h) }, &gid("g")).ok().unwrap();
    assert_eq!((p.members.as_slice(), p.gid.as_str()), (&["b".to_owned(), "a".into(), "fulano::x".into()][..], "e1"));
    d.apply(&p.writes, &[]);
    assert_eq!(d.read()("a").unwrap().peers, ["b", "fulano::x"]);
    assert_eq!(d.read()("b").unwrap().peers, ["a", "fulano::x"]);
    // Dois pares de fora no mesmo grupo também valem.
    let others = ["a".to_owned(), "bia::w".to_owned()];
    assert!(plan_join(&d.read(), &JoinInput { outside: &outside, ..input("c", &others, "", &h) }, &gid("g")).is_ok());
    // Máquina própria junto de membro de fora continua recusada.
    let others = ["a".to_owned(), "srv::y".to_owned()];
    assert!(matches!(plan_join(&d.read(), &JoinInput { outside: &outside, ..input("c", &others, "", &h) }, &gid("g")), Err(JoinRefusal::Mix)));
}

#[test]
fn orq_group_refuses_outside_member() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &JoinInput { orq: true, ..input("arb", &["exec".to_owned()], "", &h) }, &gid("o1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    d.apply(&[("solo".into(), Sidecar { peers: vec!["fulano::x".into()], task: Some(String::new()), gid: "e1".into(), ..Default::default() })], &[]);
    let others = ["solo".to_owned()];
    let joined = plan_join(&d.read(), &JoinInput { outside: &outside, ..input("arb", &others, "", &h) }, &gid("g"));
    assert!(matches!(joined, Err(JoinRefusal::OrqOutside)));
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
    let p = plan_join(&d.read(), &JoinInput { name: "arb", others: &["exec".into()], task: "", replace_task: false, harness: &h, orq: true, outside: &mine, linked: false }, &gid("o1")).ok().unwrap();
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
fn link_marks_only_the_child_and_survives_merge_join_and_rename() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &JoinInput { linked: true, ..input("pai", &["filha".into()], "", &h) }, &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    assert_eq!(d.read()("filha").unwrap().linked_by.as_deref(), Some("pai"));
    assert_eq!(d.read()("pai").unwrap().linked_by, None);
    // Fusão com outro grupo e join comum depois: quem já era ligada continua.
    let p = plan_join(&d.read(), &input("x", &["y".into()], "", &h), &gid("g2")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let p = plan_join(&d.read(), &input("x", &["pai".into()], "", &h), &gid("g3")).ok().unwrap();
    d.apply(&p.writes, &[]);
    assert_eq!(d.read()("filha").unwrap().linked_by.as_deref(), Some("pai"));
    assert!(["x", "y", "pai"].iter().all(|m| d.read()(m).unwrap().linked_by.is_none()));
    // Pai renomeado leva o vínculo da filha junto.
    let (clears, writes) = plan_rename(&d.read(), "pai", "pai2");
    d.apply(&writes, &clears);
    assert_eq!(d.read()("filha").unwrap().linked_by.as_deref(), Some("pai2"));
}

#[test]
fn lead_is_who_formed_the_group_and_survives_join_merge_and_rename() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("coord", &["a".into(), "b".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    assert!(["coord", "a", "b"].iter().all(|m| d.read()(m).unwrap().lead.as_deref() == Some("coord")));
    // Quem entra depois (mesmo chamando o pareamento) não vira a principal.
    let p = plan_join(&d.read(), &input("c", &["a".into()], "", &h), &gid("g2")).ok().unwrap();
    d.apply(&p.writes, &[]);
    assert_eq!(d.read()("c").unwrap().lead.as_deref(), Some("coord"));
    // Fusão: vale a principal do grupo sobrevivente.
    let p = plan_join(&d.read(), &input("x", &["y".into()], "", &h), &gid("g3")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let p = plan_join(&d.read(), &input("b", &["x".into()], "", &h), &gid("g4")).ok().unwrap();
    assert_eq!(p.gid, "g1");
    d.apply(&p.writes, &[]);
    assert_eq!(d.read()("y").unwrap().lead.as_deref(), Some("coord"));
    let (clears, writes) = plan_rename(&d.read(), "coord", "coord2");
    d.apply(&writes, &clears);
    assert!(["coord2", "a", "b", "c", "x", "y"].iter().all(|m| d.read()(m).unwrap().lead.as_deref() == Some("coord2")));
}

#[test]
fn parent_leaving_clears_the_link_so_rejoin_does_not_revive_it() {
    let (d, h) = (Disk::new(), none());
    let kids = ["f1".to_owned(), "f2".to_owned()];
    let p = plan_join(&d.read(), &JoinInput { linked: true, ..input("pai", &kids, "", &h) }, &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let l = plan_leave(&d.read(), "pai", false).unwrap();
    d.apply(&l.writes, &l.clears);
    assert!(kids.iter().all(|k| d.read()(k).unwrap().linked_by.is_none()));
    let p = plan_join(&d.read(), &input("pai", &["f1".into()], "", &h), &gid("g2")).ok().unwrap();
    d.apply(&p.writes, &[]);
    assert_eq!(d.read()("f1").unwrap().linked_by, None, "join comum depois da saída não religa");
}

#[test]
fn link_into_an_orq_group_is_refused() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &JoinInput { orq: true, ..input("arb", &["exec".to_owned()], "", &h) }, &gid("o1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let child = ["filha".to_owned()];
    let linked = JoinInput { linked: true, ..input("arb", &child, "", &h) };
    assert!(matches!(plan_join(&d.read(), &linked, &gid("g")), Err(JoinRefusal::Orq)));
}

#[test]
fn external_link_of_solo_session_writes_only_its_sidecar() {
    let d = Disk::new();
    let w = plan_external_link(&d.read(), "a", "fulano::x", &none(), &outside, &gid("e1")).ok().unwrap();
    assert_eq!(w.len(), 1);
    assert_eq!((w[0].0.as_str(), w[0].1.gid.as_str(), w[0].1.task.as_deref()), ("a", "e1", Some("")));
    assert_eq!(w[0].1.peers, ["fulano::x"]);
    d.apply(&w, &[]);
    // Segundo par de fora na mesma sessão.
    let w = plan_external_link(&d.read(), "a", "bia::w", &none(), &outside, &gid("e2")).ok().unwrap();
    assert_eq!((w[0].1.gid.as_str(), w[0].1.peers.as_slice()), ("e1", &["fulano::x".to_owned(), "bia::w".into()][..]));
}

#[test]
fn external_link_of_grouped_session_reaches_every_local_member() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["b".into()], "t", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let w = plan_external_link(&d.read(), "a", "fulano::x", &none(), &outside, &gid("e1")).ok().unwrap();
    d.apply(&w, &[]);
    for (name, peers) in [("a", ["b", "fulano::x"]), ("b", ["a", "fulano::x"])] {
        let st = d.read()(name).unwrap();
        assert_eq!((st.peers.as_slice(), st.gid.as_str(), st.task.as_deref()), (&peers.map(String::from)[..], "g1", Some("t")), "{name}");
    }
}

#[test]
fn external_link_refuses_orq_and_own_machine_groups() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &JoinInput { orq: true, ..input("arb", &["exec".to_owned()], "", &h) }, &gid("o1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    assert!(matches!(plan_external_link(&d.read(), "exec", "fulano::x", &none(), &outside, &gid("e")), Err(JoinRefusal::OrqOutside)));
    let p = plan_join(&d.read(), &input("c", &["srv::y".into()], "", &h), &gid("g2")).ok().unwrap();
    d.apply(&p.writes, &[]);
    assert!(matches!(plan_external_link(&d.read(), "c", "fulano::x", &none(), &outside, &gid("e")), Err(JoinRefusal::Mix)));
}

/// Decisão do usuário (B13): o árbitro sozinho num `orq` recusa o par externo com código próprio; o grupo fica.
#[test]
fn lone_orq_arbiter_refuses_an_external_pair() {
    let d = Disk::new();
    let arbiter = Sidecar { peers: vec![], task: Some("t".into()), gid: "o1".into(), harness: none(), orq: true, fed: None, linked_by: None, lead: None };
    d.apply(&[("arb".to_owned(), arbiter.clone())], &[]);
    assert!(matches!(plan_external_link(&d.read(), "arb", "fulano::x", &none(), &outside, &gid("e1")), Err(JoinRefusal::Orq)));
    assert_eq!(d.read()("arb"), Some(arbiter));
}

/// PAIR-34/35 (I-1A): sair do par com máquina própria não lê nem apaga o sidecar de uma sessão
/// local cujo nome saneado coincide com o do remoto.
#[test]
fn leave_of_cross_pair_touches_only_the_local_side() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &input("a", &["srv::x".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let homonym = Sidecar { peers: vec!["z".into()], task: Some(String::new()), gid: "h1".into(), ..Default::default() };
    d.apply(&[("srv::x".into(), homonym.clone())], &[]);
    let l = plan_leave(&d.read(), "a", false).unwrap();
    assert_eq!((l.clears.as_slice(), l.writes.len()), (&["a".to_owned()][..], 0));
    assert_eq!(l.before.keys().collect::<Vec<_>>(), ["a"]);
    d.apply(&l.writes, &l.clears);
    assert_eq!(d.read()("srv::x"), Some(homonym));
}

/// PAIR-36/37: sem grupo o rename só apaga o antigo; companheiro remoto não é regravado aqui.
#[test]
fn rename_skips_remote_companion_and_without_group_only_clears() {
    let (d, h) = (Disk::new(), none());
    assert_eq!(plan_rename(&d.read(), "solta", "nova"), (vec!["solta".to_owned()], Vec::new()));
    let p = plan_join(&d.read(), &input("a", &["srv::x".into()], "", &h), &gid("g1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let (clears, writes) = plan_rename(&d.read(), "a", "a2");
    assert_eq!(clears, ["a"]);
    assert_eq!(writes.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["a2"]);
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
    JoinOwned { name: name.into(), others: others.iter().map(|s| (*s).into()).collect(), task: String::new(), replace_task: false, harness: none(), orq, outside: HashSet::new(), linked: false }
}
fn far(addresses: &[&str]) -> HashSet<String> { addresses.iter().map(|a| (*a).to_owned()).collect() }

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

/// PAIR-32: a saída que falha no meio volta todos ao retrato e devolve o erro.
#[tokio::test]
async fn leave_write_failure_restores_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    svc.join(owned("a", &["b", "c"], false)).await.unwrap();
    let before: Vec<_> = ["a", "b", "c"].iter().map(|n| dir.sidecar(n).unwrap()).collect();
    // `a` sai e `b` é regravado; a escrita de `c` falha porque o temporário dele é uma pasta.
    std::fs::create_dir(dir.root().join("c.json.tmp")).unwrap();
    let err = svc.leave("a").await.err().expect("a escrita de c precisa falhar");
    assert!(matches!(err, GroupError::Store(_)), "{err:?}");
    let after: Vec<_> = ["a", "b", "c"].iter().map(|n| dir.sidecar(n).unwrap()).collect();
    assert_eq!(after, before, "sem saída pela metade");
}

/// PAIR-24: o grupo `orq` continua `orq` quando o join seguinte não repete a marca.
#[test]
fn orq_mark_survives_a_join_without_it() {
    let (d, h) = (Disk::new(), none());
    let p = plan_join(&d.read(), &JoinInput { orq: true, ..input("arb", &["exec".to_owned()], "", &h) }, &gid("o1")).ok().unwrap();
    d.apply(&p.writes, &[]);
    let p = plan_join(&d.read(), &input("exec2", &["arb".to_owned()], "", &h), &gid("g")).ok().unwrap();
    assert!(p.orq);
    d.apply(&p.writes, &[]);
    assert!(["arb", "exec", "exec2"].iter().all(|m| d.read()(m).unwrap().orq));
}

/// O árbitro do orquestrar-auto nasce sozinho: `orq` sem companheiro cria o grupo de um.
#[tokio::test]
async fn orq_group_of_one_member_is_created() {
    let tmp = tempfile::tempdir().unwrap();
    let orq = FakeOrq::new(OrqPhase::Ended, None);
    let (svc, dir) = service(&tmp, orq.clone());
    let out = svc.join(owned("arb", &[], true)).await.unwrap();
    assert_eq!((out.members.as_slice(), out.orq), (&["arb".to_owned()][..], true));
    let st = dir.sidecar("arb").unwrap().unwrap();
    assert!(st.peers.is_empty() && st.orq && st.gid == out.gid);
    assert_eq!(orq.promotes.load(Ordering::SeqCst), 1);
}

/// PAIR-38 (I-1A): rename que falha no meio volta o grupo ao que era.
#[tokio::test]
async fn rename_failure_restores_the_group() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    svc.join(owned("a", &["b"], false)).await.unwrap();
    let (a, b) = (dir.sidecar("a").unwrap(), dir.sidecar("b").unwrap());
    std::fs::create_dir(dir.root().join("b.json.tmp")).unwrap();
    assert!(matches!(svc.rename("a", "a2").await, Err(GroupError::Store(_))));
    assert_eq!((dir.sidecar("a").unwrap(), dir.sidecar("b").unwrap()), (a, b));
    assert!(dir.sidecar("a2").unwrap().is_none());
}

/// PAIR-22 (I-1A): contrato do perdedor com UTF-8 inválido não derruba a fusão; o arquivo fica.
#[tokio::test]
async fn fusion_with_unreadable_loser_contract_still_joins() {
    let tmp = tempfile::tempdir().unwrap();
    let (python, upstream) = crate::fake::spawn_fake().await;
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    let svc = svc.with_diary(hangar_server::diag::DiagClient::new(upstream, crate::fake::SECRET.into()));
    let first = svc.join(owned("a", &["b"], false)).await.unwrap();
    let loser = svc.join(owned("c", &["d"], false)).await.unwrap();
    std::fs::write(dir.contract_path(&loser.gid), b"\xff\xfe quebrado").unwrap();
    let merged = svc.join(owned("a", &["c"], false)).await.unwrap();
    assert_eq!(merged.gid, first.gid);
    assert_eq!(dir.sidecar("d").unwrap().unwrap().gid, first.gid);
    assert!(dir.contract_path(&loser.gid).exists());
    // A decisão (I-1A) manda a falha ao diário exportável, não só ao log.
    crate::fake::wait_until(move || python.diag().iter().any(|d| d["evento"] == "rust.groups_contract_merge_failed")).await;
}

/// Promoção que espera um sinal e então recusa: segura a trava do grupo enquanto espera.
struct GatedOrq { entered: tokio::sync::Notify, release: tokio::sync::Notify }
impl OrqFacts for GatedOrq {
    fn phase<'a>(&'a self, _gid: &'a str) -> BoxFuture<'a, OrqPhase> { Box::pin(async { OrqPhase::Ended }) }
    fn promote<'a>(&'a self, _name: &'a str, _gid: &'a str) -> BoxFuture<'a, Result<(), PromoteError>> {
        Box::pin(async move {
            self.entered.notify_one();
            self.release.notified().await;
            Err(PromoteError::Conflict("mudou".into()))
        })
    }
}

/// PAIR-15/18: join concorrente espera o primeiro desfazer; o desfazer não apaga o que o segundo gravou.
#[tokio::test]
async fn concurrent_join_waits_for_the_undo_of_the_first() {
    let tmp = tempfile::tempdir().unwrap();
    let orq = Arc::new(GatedOrq { entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() });
    let make = || PairDir::new(tmp.path().join("pair"), tmp.path().join("arquivo"));
    let (svc, dir) = (Arc::new(GroupService::new(make(), orq.clone(), "srv".into())), make());
    let first = tokio::spawn({ let svc = svc.clone(); async move { svc.join(owned("arb", &["exec"], true)).await } });
    orq.entered.notified().await;
    let second = tokio::spawn({ let svc = svc.clone(); async move { svc.join(owned("c", &["exec"], false)).await } });
    tokio::task::yield_now().await;
    assert!(dir.sidecar("c").unwrap().is_none(), "o segundo join espera a trava");
    orq.release.notify_one();
    assert!(matches!(first.await.unwrap(), Err(GroupError::Orq(_))));
    second.await.unwrap().unwrap();
    let exec = dir.sidecar("exec").unwrap().unwrap();
    assert_eq!((exec.peers.as_slice(), exec.orq), (&["c".to_owned()][..], false));
    assert!(dir.sidecar("arb").unwrap().is_none());
}

/// PAIR-03: sidecar torto vale "sem grupo" para quem opera, nunca erro.
#[tokio::test]
async fn unreadable_sidecar_counts_as_no_group() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    std::fs::create_dir_all(dir.root()).unwrap();
    std::fs::write(dir.root().join("a.json"), "{").unwrap();
    assert!(svc.leave("a").await.unwrap().is_empty());
    let out = svc.join(owned("a", &["b"], false)).await.unwrap();
    assert_eq!((out.members.as_slice(), out.newcomers.as_slice()), (&["a".to_owned(), "b".into()][..], &["a".to_owned(), "b".into()][..]));
    assert_eq!(dir.sidecar("a").unwrap().unwrap().peers, ["b"]);
}

/// PAIR-15: saída e dissolução concorrentes com um join esperam a mesma trava.
#[tokio::test]
async fn leave_and_dissolve_wait_for_a_join_in_flight() {
    let tmp = tempfile::tempdir().unwrap();
    let orq = Arc::new(GatedOrq { entered: tokio::sync::Notify::new(), release: tokio::sync::Notify::new() });
    let make = || PairDir::new(tmp.path().join("pair"), tmp.path().join("arquivo"));
    let svc = Arc::new(GroupService::new(make(), orq.clone(), "srv".into()));
    // Join de dois, preso na promoção: sem a trava, a saída de `exec` acharia o grupo e devolveria `arb`.
    let first = tokio::spawn({ let svc = svc.clone(); async move { svc.join(owned("arb", &["exec"], true)).await } });
    orq.entered.notified().await;
    let mut leave = tokio::spawn({ let svc = svc.clone(); async move { svc.leave("exec").await } });
    assert!(tokio::time::timeout(std::time::Duration::from_millis(300), &mut leave).await.is_err(), "a saída espera a trava");
    orq.release.notify_one();
    assert!(first.await.unwrap().is_err());
    assert!(leave.await.unwrap().unwrap().is_empty(), "a saída rodou depois do desfazer");
    // Grupo `orq` de um, preso na promoção: sem a trava, a dissolução o apagaria.
    let first = tokio::spawn({ let svc = svc.clone(); async move { svc.join(owned("solo", &[], true)).await } });
    orq.entered.notified().await;
    let mut dissolve = tokio::spawn({ let svc = svc.clone(); async move { svc.dissolve_lone_orq(std::time::Duration::ZERO).await } });
    assert!(tokio::time::timeout(std::time::Duration::from_millis(300), &mut dissolve).await.is_err(), "a dissolução espera a trava");
    orq.release.notify_one();
    assert!(first.await.unwrap().is_err());
    assert!(dissolve.await.unwrap().unwrap().is_empty(), "a dissolução rodou depois do desfazer");
}

/// PAIR-36: o rename leva a chave do `harness` nos dois sidecars.
#[tokio::test]
async fn rename_moves_the_harness_key_in_every_sidecar() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    let harness: BTreeMap<String, String> = [("a".into(), "claude".into()), ("b".into(), "codex".into())].into();
    svc.join(JoinOwned { harness, ..owned("a", &["b"], false) }).await.unwrap();
    svc.rename("a", "a2").await.unwrap();
    for name in ["a2", "b"] {
        let h = dir.sidecar(name).unwrap().unwrap().harness;
        assert_eq!((h.get("a2").map(String::as_str), h.contains_key("a")), (Some("claude"), false), "{name}");
    }
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
    svc.external_link("b", "m::x", none(), far(&["m::x"])).await.unwrap();
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
async fn external_link_and_unlink_cover_the_whole_group() {
    let tmp = tempfile::tempdir().unwrap();
    let (svc, dir) = service(&tmp, FakeOrq::new(OrqPhase::Ended, None));
    let out = svc.join(owned("a", &["b"], false)).await.unwrap();
    assert_eq!(svc.external_link("a", "fulano::x", none(), far(&["fulano::x"])).await.unwrap(), out.gid);
    assert_eq!(dir.sidecar("b").unwrap().unwrap().peers, ["a", "fulano::x"]);
    // Membro local que entra depois, pela rota de grupo, também vê o de fora.
    svc.join(JoinOwned { outside: far(&["fulano::x"]), ..owned("c", &["a"], false) }).await.unwrap();
    assert_eq!(dir.sidecar("c").unwrap().unwrap().peers, ["a", "b", "fulano::x"]);
    svc.external_unlink("a", "fulano::x").await.unwrap();
    for (name, peers) in [("a", ["c", "b"]), ("b", ["c", "a"]), ("c", ["a", "b"])] {
        assert_eq!(dir.sidecar(name).unwrap().unwrap().peers, peers, "{name}");
    }
    svc.join(owned("arb", &["exec"], true)).await.unwrap();
    assert!(matches!(svc.external_link("exec", "fulano::y", none(), far(&["fulano::y"])).await.err(), Some(GroupError::Refused(JoinRefusal::OrqOutside))));

    let gid = svc.external_link("solo", "fulano::x", none(), far(&["fulano::x"])).await.unwrap();
    let st = dir.sidecar("solo").unwrap().unwrap();
    assert_eq!((st.gid.as_str(), st.peers.as_slice()), (gid.as_str(), &["fulano::x".to_owned()][..]));
    assert_eq!(gid.len(), 8);
    svc.external_unlink("solo", "fulano::x").await.unwrap();
    assert!(dir.sidecar("solo").unwrap().is_none());
}
