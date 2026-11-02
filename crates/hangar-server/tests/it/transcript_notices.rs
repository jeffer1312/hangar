//! Avisos da CLI no Claude com terminal, lidos do transcript: o histórico e o leitor ao vivo dão as mesmas notas.
use std::path::PathBuf;

use hangar_api::chat::ChatEvent;
use hangar_server::transcript::{merged_history, merged_history_with_notices, HistoryRequest, LineParser, Provider, TAIL_WINDOW};
use serde_json::{json, Value};

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
fn request(path:PathBuf,limit:Option<usize>,window:u64) -> HistoryRequest {
    HistoryRequest { provider:Provider::Claude,jsonl:path,queue:None,limit,tail_window:window }
}
fn history(name:&str,limit:Option<usize>,window:u64) -> Vec<(String,String,Option<String>)> {
    notes(merged_history_with_notices(&request(fixture(name),limit,window)).unwrap())
}
fn live(name:&str,with_notices:bool) -> Vec<(String,String,Option<String>)> { live_text(&std::fs::read_to_string(fixture(name)).unwrap(),with_notices) }
fn live_text(text:&str,with_notices:bool) -> Vec<(String,String,Option<String>)> {
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
    let plain = merged_history(&request(fixture("terminal_hooks.jsonl"),None,TAIL_WINDOW)).unwrap();
    assert!(notes(plain).is_empty(),"lista, retorno da filha e ditado leem o histórico comum, sem notas");
}

/// Linhas do arquivo de hooks com `uuid` novo, para montar turnos que o arquivo não tem.
fn with_uuid(row:&Value,uuid:&str) -> Value { let mut row = row.clone(); row["uuid"] = json!(uuid); row }
fn both(lines:&[Value]) -> (Vec<(String,String,Option<String>)>,Vec<(String,String,Option<String>)>) {
    let text:String = lines.iter().map(|row|format!("{row}\n")).collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path,&text).unwrap();
    (notes(merged_history_with_notices(&request(path,None,TAIL_WINDOW)).unwrap()),live_text(&text,true))
}

#[test]
fn terminal_turn_opened_by_the_queue_still_counts() {
    let rows = rows("terminal_hooks.jsonl");
    let (prompt,failure,end) = (&rows[0],&rows[1],&rows[4]);
    let mut queued = with_uuid(failure,"q1");
    queued["attachment"] = json!({"type":"queued_command","prompt":"mensagem da fila","commandMode":"prompt"});
    let lines = [prompt.clone(),with_uuid(end,"d1"),queued,with_uuid(failure,"h1"),with_uuid(failure,"h2"),with_uuid(end,"d2")];
    let expected = vec![
        ("hookfail:h1".to_owned(),"hook_failed".to_owned(),Some("PostToolUse:Bash: falhou-de-proposito".to_owned())),
        ("hookfail-sum:d2:5182a3be".to_owned(),"hook_failed".to_owned(),Some("PostToolUse:Bash: falhou-de-proposito (2× neste turno)".to_owned())),
    ];
    let (history,live) = both(&lines);
    assert_eq!(history,expected);
    assert_eq!(live,expected);
}

/// Recado de outra sessão (`hangar-send`) chega como `isMeta` com `origin.kind: "peer"`, sem prompt: abre o turno
/// da sessão parada. Medido na prova ao vivo: a primeira falha do primeiro turno sumia.
#[test]
fn terminal_turn_opened_by_a_peer_message_counts() {
    let rows = rows("terminal_hooks.jsonl");
    let (prompt,failure,end) = (&rows[0],&rows[1],&rows[4]);
    let mut peer = with_uuid(prompt,"p1");
    peer["isMeta"] = json!(true);
    peer["origin"] = json!({"kind":"peer","name":"outra-sessao","body":"[de: outra-sessao] rode ls"});
    peer["message"] = json!({"role":"user","content":"Another Claude session sent a message: rode ls"});
    let lines = [peer,with_uuid(failure,"h1"),with_uuid(failure,"h2"),with_uuid(end,"d1")];
    let expected = vec![
        ("hookfail:h1".to_owned(),"hook_failed".to_owned(),Some("PostToolUse:Bash: falhou-de-proposito".to_owned())),
        ("hookfail-sum:d1:5182a3be".to_owned(),"hook_failed".to_owned(),Some("PostToolUse:Bash: falhou-de-proposito (2× neste turno)".to_owned())),
    ];
    let (history,live) = both(&lines);
    assert_eq!(history,expected);
    assert_eq!(live,expected);
}

#[test]
fn terminal_compact_summary_and_local_command_do_not_end_the_turn() {
    let rows = rows("terminal_hooks.jsonl");
    let (prompt,failure,end) = (&rows[0],&rows[1],&rows[4]);
    let mut compact = with_uuid(prompt,"c1");
    compact["isCompactSummary"] = json!(true);
    compact["message"] = json!({"role":"user","content":"This session is being continued from a previous conversation."});
    let mut command = with_uuid(prompt,"c2");
    command["message"] = json!({"role":"user","content":"<command-name>/context</command-name>"});
    let lines = [prompt.clone(),with_uuid(failure,"h1"),compact,command,with_uuid(failure,"h2"),with_uuid(end,"d1")];
    let expected = vec![
        ("hookfail:h1".to_owned(),"hook_failed".to_owned(),Some("PostToolUse:Bash: falhou-de-proposito".to_owned())),
        ("hookfail-sum:d1:5182a3be".to_owned(),"hook_failed".to_owned(),Some("PostToolUse:Bash: falhou-de-proposito (2× neste turno)".to_owned())),
    ];
    let (history,live) = both(&lines);
    assert_eq!(history,expected);
    assert_eq!(live,expected);
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
