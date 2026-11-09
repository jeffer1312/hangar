//! Projeção local dos fatos dos atores; transferência e orquestração conservam seu estado.
use hangar_api::session::SessionRow;
use hangar_server::list::facts::{self, ListFacts};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

fn row(name: &str, provider: &str) -> SessionRow {
    serde_json::from_value(json!({"name": name, "provider": provider})).unwrap()
}

fn state(s: &str) -> Value {
    json!({"state": s, "label": null, "question": null, "options": null, "problema": null, "status_line": "linha",
           "pending_questions": 0, "startup_steps": [], "last_activity": 1.0, "limited": false, "limit_reset": null,
           "stalled": false})
}

fn known(states: facts::States) -> ListFacts { ListFacts { states, unknown: false, ..Default::default() } }

#[test]
fn transfer_rows_not_classified() {
    let mut troca = row("troca", "claude");
    troca.transfer_phase = Some("copying".into());
    troca.problema = Some("x".into());
    let mut fim = row("fim", "codex");
    fim.transfer_phase = Some("complete".into());
    let mut orq = row("o1", "orq");
    orq.state = "working".into();
    let facts = ListFacts { overrides: vec![troca, fim], frozen: HashSet::from(["troca".into()]), orq: vec![orq],
        owners: HashMap::from([("a".into(), "ana".into())]), unknown: false, ..Default::default() };
    let (work, aside) = facts::apply(vec![row("troca", "codex"), row("a", "claude"), row("fim", "codex")], &facts, true);
    let names = |v: &[SessionRow]| v.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
    // Só as linhas fora de troca e de orquestração vão à classificação e à decoração.
    assert_eq!(names(&work), ["a", "fim"]);
    assert_eq!(names(&aside), ["troca", "o1"]);
    assert_eq!(aside[0].provider, "claude", "a linha da troca substitui a descoberta pelo nome");
    assert_eq!(work[1].transfer_phase.as_deref(), Some("complete"));
    // `shared` não vem dos fatos: sai da loja de convites do Rust (`share_bridge.rs`).
    assert!(!work[0].shared && work[0].owner.as_deref() == Some("ana"));
}

/// Codex mantém seu estado próprio; a projeção dos outros atores não o substitui.
#[test]
fn terminal_actor_state_does_not_replace_codex() {
    let st: facts::RowState = serde_json::from_value(state("working")).unwrap();
    let facts = known(Arc::new(HashMap::from([("cx".into(), st)])));
    let (work, _) = facts::apply(vec![row("cx", "codex")], &facts, true);
    assert_eq!((work[0].status_line.as_deref(), work[0].last_activity), (None, None));
}
