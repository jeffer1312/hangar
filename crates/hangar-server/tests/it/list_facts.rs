//! Estado de Pi/omp/Kimi pelo Python: o último valor fica quando ele não responde no prazo, e a linha
//! em transferência ou de orquestração não passa pela classificação.
use crate::fake;

use fake::{SECRET, spawn_fake};
use hangar_api::session::SessionRow;
use hangar_server::list::facts::{self, FactsClient, ListFacts};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::Ordering::SeqCst;
use std::time::{Duration, Instant};

fn row(name: &str, provider: &str) -> SessionRow {
    serde_json::from_value(json!({"name": name, "provider": provider})).unwrap()
}

fn state(s: &str) -> Value {
    json!({"state": s, "label": null, "question": null, "options": null, "problema": null, "status_line": "linha",
           "pending_questions": 0, "startup_steps": [], "last_activity": 1.0, "limited": false, "limit_reset": null,
           "stalled": false})
}

fn known(states: facts::States) -> ListFacts { ListFacts { states, unknown: false, ..Default::default() } }

#[tokio::test]
async fn timeout_after_good_answer_keeps_last_without_marking() {
    let (fake, addr) = spawn_fake().await;
    *fake.list_facts.lock().unwrap() = (json!({"states": {"pp": state("working")}}), Duration::ZERO);
    let client = FactsClient::new(addr, SECRET.into());
    let rows = vec![row("pp", "pi")];
    let first = client.fetch(&rows, &BTreeMap::new()).await;
    assert!(first.ok);
    // Mesma entrada dentro do prazo: o Python não é perguntado de novo.
    let again = client.fetch(&rows, &BTreeMap::new()).await;
    assert!(again.ok);
    assert_eq!(fake.list_facts_calls.load(SeqCst), 1);

    *fake.list_facts.lock().unwrap() = (json!({"states": {"pp": state("idle")}}), Duration::from_secs(3));
    let started = Instant::now();
    // Entrada nova (outro pid): pergunta, e o Python não responde em 1 s.
    let late = client.fetch(&rows, &BTreeMap::from([("pp".into(), 7)])).await;
    assert!(started.elapsed() < Duration::from_millis(1800), "esperou além do prazo: {:?}", started.elapsed());
    // Uma falha logo depois de resposta boa fica na carência: os últimos fatos, sem marcar a lista.
    // O fim da carência (3 falhas ou 5 s) está nos testes de `list::facts`.
    assert!(late.ok && !late.fresh);
    let (work, aside) = facts::apply(rows.clone(), &known(late.states), late.ok);
    assert!(aside.is_empty());
    assert_eq!((work[0].state.as_str(), work[0].problema.as_deref()), ("working", None), "fica o último valor bom");
    // Python travado não custa 1 s a cada tique: dentro do prazo, fica a falha sem perguntar.
    let asked = fake.list_facts_calls.load(SeqCst);
    let quick = Instant::now();
    let cached = client.fetch(&rows, &BTreeMap::from([("pp".into(), 8)])).await;
    assert!(cached.ok && !cached.fresh, "ainda na carência, sem perguntar de novo");
    assert!(quick.elapsed() < Duration::from_millis(100));
    assert_eq!(fake.list_facts_calls.load(SeqCst), asked);
}

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

/// Codex não é mais linha do Python: o estado dos fatos não chega a ela.
#[test]
fn python_state_reaches_only_pi_omp_and_kimi() {
    let st: facts::RowState = serde_json::from_value(state("working")).unwrap();
    let facts = known(Arc::new(HashMap::from([("cx".into(), st)])));
    let (work, _) = facts::apply(vec![row("cx", "codex")], &facts, true);
    assert_eq!((work[0].status_line.as_deref(), work[0].last_activity), (None, None));
}

#[tokio::test]
async fn no_answer_yet_marks_pi_rows_and_partial_answer_is_refused() {
    let (fake, addr) = spawn_fake().await;
    // Chave faltando é contrato quebrado: não vira estado vazio.
    *fake.list_facts.lock().unwrap() = (json!({"estados": {}}), Duration::ZERO);
    let client = FactsClient::new(addr, SECRET.into());
    let rows = vec![row("cc", "claude"), row("pp", "pi")];
    let got = client.fetch(&rows[1..], &BTreeMap::new()).await;
    assert!(!got.ok);
    let (work, _) = facts::apply(rows, &known(got.states), got.ok);
    let marked: Vec<_> = work.iter().map(|r| r.problema.as_deref()).collect();
    assert_eq!(marked, [None, Some("list_facts_unavailable")]);
}
