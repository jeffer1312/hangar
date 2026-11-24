//! Quando a revisão é relida: fim de turno, primeira vista e branch trocada, pelas linhas da lista.
use hangar_work_review::{Ended, TurnRow, TurnTracker};

fn row(name: &str, state: &str, reply: Option<f64>) -> TurnRow {
    TurnRow { name: name.into(), lifecycle_id: Some(format!("{name}-1")), state: state.into(), last_reply_at: reply, branch: Some("PM-1".into()) }
}

fn names(ended: Vec<Ended>) -> Vec<(String, bool)> {
    let mut names: Vec<_> = ended.into_iter().map(|e| (e.name, e.first_sight)).collect();
    names.sort();
    names
}

#[test]
fn first_sight_then_only_real_turn_ends() {
    let mut tracker = TurnTracker::new();
    assert_eq!(names(tracker.ended(Some(&[row("a", "working", None), row("b", "idle", Some(1.0))]))),
        vec![("a".into(), true), ("b".into(), true)]);
    assert!(tracker.ended(Some(&[row("a", "working", None), row("b", "idle", Some(1.0))])).is_empty(), "nada mudou");
    assert_eq!(names(tracker.ended(Some(&[row("a", "idle", None), row("b", "idle", Some(1.0))]))), vec![("a".into(), false)], "working para idle");
    assert_eq!(names(tracker.ended(Some(&[row("a", "idle", None), row("b", "idle", Some(2.0))]))), vec![("b".into(), false)], "resposta nova parada");
    assert!(tracker.ended(Some(&[row("a", "working", Some(3.0)), row("b", "idle", Some(2.0))])).is_empty(), "resposta nova trabalhando não conta");
}

#[test]
fn branch_change_and_new_life_count() {
    let mut tracker = TurnTracker::new();
    tracker.ended(Some(&[row("a", "idle", None)]));
    let mut moved = row("a", "idle", None);
    moved.branch = Some("PM-2".into());
    assert_eq!(names(tracker.ended(Some(std::slice::from_ref(&moved)))), vec![("a".into(), false)]);
    moved.lifecycle_id = Some("a-2".into());
    assert_eq!(names(tracker.ended(Some(std::slice::from_ref(&moved)))), vec![("a".into(), true)], "a sessão recriada é outra");
}

#[test]
fn failed_round_changes_nothing_and_names_that_left_are_forgotten() {
    let mut tracker = TurnTracker::new();
    tracker.ended(Some(&[row("a", "working", None)]));
    assert!(tracker.ended(None).is_empty());
    assert_eq!(names(tracker.ended(Some(&[row("a", "idle", None)]))), vec![("a".into(), false)], "a rodada sem linhas não apagou o working");
    assert!(tracker.ended(Some(&[])).is_empty());
    assert_eq!(names(tracker.ended(Some(&[row("a", "idle", None)]))), vec![("a".into(), true)], "quem saiu volta como novo");
}
