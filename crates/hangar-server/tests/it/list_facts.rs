//! Fatos da lista pelo Python: o último valor fica quando ele não responde no prazo, com a linha
//! marcada, e a linha em transferência ou de orquestração não passa pela classificação.
use crate::fake;

use fake::{SECRET, spawn_fake};
use hangar_api::session::SessionRow;
use hangar_server::list::facts::{self, FactsClient, ListFacts};
use serde_json::{Value, json};
use std::collections::BTreeMap;
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

/// Resposta completa do Python, com `extra` por cima.
fn full(extra: Value) -> Value {
    let mut v = json!({"states": {}, "overrides": [], "frozen": [], "orq": [], "shared": [], "owners": {},
        "hidden": [], "problems": {}, "held": {}, "stall_seconds": 300.0, "nav": {}, "shortcuts": null, "shadow": null});
    for (k, x) in extra.as_object().unwrap() { v[k] = x.clone(); }
    v
}

#[tokio::test]
async fn timeout_keeps_last_and_marks_problem() {
    let (fake, addr) = spawn_fake().await;
    *fake.list_facts.lock().unwrap() = (full(json!({"states": {"cx": state("working")}, "stall_seconds": 60.0})), Duration::ZERO);
    let client = FactsClient::new(addr, SECRET.into());
    let rows = vec![row("cx", "codex"), row("cc", "claude")];
    let first = client.fetch(&rows, 1, &BTreeMap::new(), false).await;
    assert!(first.ok);
    assert_eq!(first.facts.stall_seconds, 60.0);
    // Mesma entrada dentro do prazo: o Python não é perguntado de novo.
    let again = client.fetch(&rows, 1, &BTreeMap::new(), false).await;
    assert!(again.ok);
    assert_eq!(fake.list_facts_calls.load(SeqCst), 1);
    assert_eq!(fake.list_facts_last.lock().unwrap()["owner_clients"], 1);

    *fake.list_facts.lock().unwrap() = (full(json!({"states": {"cx": state("idle")}})), Duration::from_secs(3));
    let started = Instant::now();
    // Entrada nova (mais um cliente): pergunta, e o Python não responde em 1 s.
    let late = client.fetch(&rows, 2, &BTreeMap::new(), false).await;
    assert!(started.elapsed() < Duration::from_millis(1800), "esperou além do prazo: {:?}", started.elapsed());
    assert!(!late.ok);
    let (work, aside) = facts::apply(rows.clone(), &late.facts, late.ok);
    assert!(aside.is_empty());
    let cx = work.iter().find(|r| r.name == "cx").unwrap();
    assert_eq!(cx.state, "working", "fica o último valor bom");
    assert_eq!(cx.problema.as_deref(), Some("list_facts_unavailable"));
    // A Claude é do Rust: a falha dos fatos não a marca.
    assert_eq!(work.iter().find(|r| r.name == "cc").unwrap().problema, None);
    // Python travado não custa 1 s a cada tique: dentro do prazo, fica a falha sem perguntar.
    let asked = fake.list_facts_calls.load(SeqCst);
    let quick = Instant::now();
    assert!(!client.fetch(&rows, 3, &BTreeMap::new(), false).await.ok);
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
    let facts: ListFacts = serde_json::from_value(full(json!({
        "overrides": [troca, fim], "frozen": ["troca"], "orq": [orq],
        "shared": ["a"], "owners": {"a": "ana"}
    }))).unwrap();
    let (work, aside) = facts::apply(vec![row("troca", "codex"), row("a", "claude"), row("fim", "codex")], &facts, true);
    let names = |v: &[SessionRow]| v.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
    // Só as linhas fora de troca e de orquestração vão à classificação e à decoração.
    assert_eq!(names(&work), ["a", "fim"]);
    assert_eq!(names(&aside), ["troca", "o1"]);
    assert_eq!(aside[0].provider, "claude", "a linha da troca substitui a descoberta pelo nome");
    assert_eq!(work[1].transfer_phase.as_deref(), Some("complete"));
    assert!(work[0].shared && work[0].owner.as_deref() == Some("ana"));
}

#[test]
fn codex_service_tier_comes_from_the_live_state() {
    let mut cx = row("cx", "codex");
    cx.codex_service_tier = Some("default".into());
    let mut st = state("working");
    st["codex_service_tier"] = json!("priority");
    let mut pp = state("idle");
    pp["codex_service_tier"] = json!("priority");
    let facts: ListFacts = serde_json::from_value(full(json!({"states": {"cx": st, "pp": pp}}))).unwrap();
    let (work, _) = facts::apply(vec![cx, row("pp", "pi")], &facts, true);
    assert_eq!(work[0].codex_service_tier.as_deref(), Some("priority"), "o snapshot do Codex vence o sidecar");
    assert_eq!(work[1].codex_service_tier, None, "só linha Codex tem nível de serviço");
}

#[tokio::test]
async fn no_answer_yet_marks_every_row_and_partial_answer_is_refused() {
    let (fake, addr) = spawn_fake().await;
    // Chave faltando é contrato quebrado: não vira `hidden` vazio.
    *fake.list_facts.lock().unwrap() = (json!({"states": {}}), Duration::ZERO);
    let client = FactsClient::new(addr, SECRET.into());
    let rows = vec![row("cc", "claude"), row("cx", "codex")];
    let got = client.fetch(&rows, 0, &BTreeMap::new(), false).await;
    assert!(!got.ok && got.facts.unknown);
    let (work, _) = facts::apply(rows, &got.facts, got.ok);
    assert!(work.iter().all(|r| r.problema.as_deref() == Some("list_facts_unavailable")));
}
