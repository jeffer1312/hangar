//! Avisos da CLI no Claude com terminal, lidos do transcript: o histórico e o leitor ao vivo dão as mesmas notas.
use std::path::PathBuf;

use hangar_api::chat::ChatEvent;
use hangar_server::transcript::{merged_history, HistoryRequest, LineParser, Provider, TAIL_WINDOW};
use serde_json::Value;

fn fixture(name:&str) -> PathBuf { PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claude_events").join(name) }
fn rows(name:&str) -> Vec<Value> {
    std::fs::read_to_string(fixture(name)).unwrap().lines().map(|row|serde_json::from_str(row).unwrap()).collect()
}
fn uuid(rows:&[Value],at:usize) -> String { rows[at]["uuid"].as_str().unwrap().to_owned() }

fn is_note(event:&ChatEvent) -> bool {
    ["hookfail:","hookfail-sum:","retry:","retry-ok:","refusal:"].iter().any(|prefix|event.id.starts_with(prefix))
}
fn notes(events:Vec<ChatEvent>) -> Vec<(String,String,Option<String>)> {
    events.into_iter().filter(is_note).map(|event|(event.id,event.text.unwrap_or_default(),event.hook_error)).collect()
}
fn history(name:&str,limit:Option<usize>,window:u64) -> Vec<(String,String,Option<String>)> {
    notes(merged_history(&HistoryRequest { provider:Provider::Claude,jsonl:fixture(name),queue:None,limit,tail_window:window }).unwrap())
}
fn live(name:&str,with_notices:bool) -> Vec<(String,String,Option<String>)> {
    let text = std::fs::read_to_string(fixture(name)).unwrap();
    let parser = LineParser::new(Provider::Claude);
    let mut parser = if with_notices { parser.with_notices() } else { parser };
    let mut at = 0u64;
    let mut events = Vec::new();
    for line in text.split_inclusive('\n') {
        events.extend(parser.feed(line.as_bytes(),at));
        at += line.len() as u64;
    }
    notes(events)
}

#[test]
fn terminal_hook_failures_sum_per_turn_in_history_and_live() {
    let lines = rows("terminal_hooks.jsonl");
    // md5("PostToolUse:Bash")[..8]
    let sum_id = |closing:usize| format!("hookfail-sum:{}:5182a3be",uuid(&lines,closing));
    let detail = "PostToolUse:Bash: falhou-de-proposito".to_owned();
    let expected = vec![
        (format!("hookfail:{}",uuid(&lines,1)),"hook_failed".to_owned(),Some(detail.clone())),
        (sum_id(4),"hook_failed".to_owned(),Some(format!("{detail} (3× neste turno)"))),
        // Turno interrompido: sem `turn_duration`, a soma fecha no prompt seguinte.
        (format!("hookfail:{}",uuid(&lines,6)),"hook_failed".to_owned(),Some(detail.clone())),
        (sum_id(8),"hook_failed".to_owned(),Some(format!("{detail} (2× neste turno)"))),
    ];
    assert_eq!(history("terminal_hooks.jsonl",None,TAIL_WINDOW),expected,"a linha `sdk-cli` não gera nota");
    assert_eq!(live("terminal_hooks.jsonl",true),expected);
    assert!(live("terminal_hooks.jsonl",false).is_empty(),"quem não pede as notas (push, última resposta) não as recebe");
}

#[test]
fn terminal_history_tail_never_invents_a_different_note() {
    let full = history("terminal_hooks.jsonl",None,TAIL_WINDOW);
    for window in [256,512,1024,2048] {
        let tail = history("terminal_hooks.jsonl",Some(200),window);
        assert!(tail.iter().all(|note|full.contains(note)),"janela {window}: {tail:?} fora de {full:?}");
    }
}

#[test]
fn terminal_api_retry_note_and_comeback() {
    let lines = rows("terminal_retry.jsonl");
    let answer = lines.iter().position(|row|row["type"] == "assistant").unwrap();
    let expected = vec![
        (format!("retry:{}",uuid(&lines,1)),"⚙️ A API falhou (529 Overloaded); nova tentativa 1 de 3".to_owned(),None),
        (format!("retry-ok:{}",uuid(&lines,answer)),"⚙️ A API voltou na 4ª tentativa".to_owned(),None),
        // Terminou em erro da API: a resposta de erro já aparece, sem fecho.
        (format!("retry:{}",uuid(&lines,7)),"⚙️ A API falhou (529 Overloaded); nova tentativa 1 de 2".to_owned(),None),
    ];
    assert_eq!(history("terminal_retry.jsonl",None,TAIL_WINDOW),expected);
    assert_eq!(live("terminal_retry.jsonl",true),expected);
}

#[test]
fn terminal_refusal_note_only_on_cli_lines() {
    let lines = rows("terminal_refusal.jsonl");
    let expected = vec![(format!("refusal:{}",uuid(&lines,1)),"⚙️ O modelo recusou responder".to_owned(),None)];
    assert_eq!(history("terminal_refusal.jsonl",None,TAIL_WINDOW),expected);
    assert_eq!(live("terminal_refusal.jsonl",true),expected);
}
