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
    ["hookfail:","retry:","retry-ok:","refusal:"].iter().any(|prefix|event.id.starts_with(prefix))
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
fn terminal_hook_failure_is_one_note_per_file_with_the_total_in_history() {
    let lines = rows("terminal_hooks.jsonl");
    let id = format!("hookfail:{}",uuid(&lines,1));
    let detail = "PostToolUse:Bash: falhou-de-proposito".to_owned();
    // Cinco falhas em dois turnos: uma nota. A linha `sdk-cli` não conta.
    assert_eq!(history("terminal_hooks.jsonl",None,TAIL_WINDOW),vec![(id.clone(),"hook_failed".to_owned(),Some(format!("{detail} (falhou 5 vezes)")))]);
    // Ao vivo a nota sai na primeira falha, antes de haver total; as repetições não geram evento.
    assert_eq!(live("terminal_hooks.jsonl",true),vec![(id.clone(),"hook_failed".to_owned(),Some(detail))]);
    // O mapa do aviso: chave de ignorar e hook sempre; o total só onde ele é conhecido.
    let meta = |events:Vec<ChatEvent>| events.into_iter().find(|event|event.id == id).unwrap().notice_meta.map(Value::Object);
    let past = merged_history_with_notices(&request(fixture("terminal_hooks.jsonl"),None,TAIL_WINDOW)).unwrap();
    assert_eq!(meta(past),Some(json!({"key":"hook:PostToolUse:Bash","subject":"PostToolUse:Bash","count":5})));
    let mut parser = LineParser::new(Provider::Claude).with_notices();
    let first = parser.feed(format!("{}\n",lines[0]).as_bytes(),0).into_iter().chain(parser.feed(format!("{}\n",lines[1]).as_bytes(),1)).collect();
    assert_eq!(meta(first),Some(json!({"key":"hook:PostToolUse:Bash","subject":"PostToolUse:Bash"})));
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
    let (history,live) = both(&lines);
    assert_eq!(history,vec![h1(2)]);
    assert_eq!(live,vec![h1(1)]);
}

/// A nota da primeira falha (`h1`) do `PostToolUse:Bash`, com o total quando ele passa de um.
fn h1(count:u32) -> (String,String,Option<String>) {
    let total = if count > 1 { format!(" (falhou {count} vezes)") } else { String::new() };
    ("hookfail:h1".to_owned(),"hook_failed".to_owned(),Some(format!("PostToolUse:Bash: falhou-de-proposito{total}")))
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
    let (history,live) = both(&lines);
    assert_eq!(history,vec![h1(2)]);
    assert_eq!(live,vec![h1(1)]);
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
    let mut blocks = with_uuid(prompt,"c3");
    blocks["message"] = json!({"role":"user","content":[{"type":"text","text":"<command-name>/model</command-name>"},{"type":"text","text":"lembrete"}]});
    let lines = [prompt.clone(),with_uuid(failure,"h1"),compact,command,blocks,with_uuid(failure,"h2"),with_uuid(end,"d1")];
    let (history,live) = both(&lines);
    assert_eq!(history,vec![h1(2)]);
    assert_eq!(live,vec![h1(1)]);
}

#[test]
fn terminal_hook_failure_before_the_first_prompt_counts_from_the_file_start() {
    let rows = rows("terminal_hooks.jsonl");
    let (prompt,failure,end) = (&rows[0],&rows[1],&rows[4]);
    let mut start = with_uuid(failure,"s1");
    start["attachment"]["hookName"] = json!("SessionStart:startup");
    start["attachment"]["hookEvent"] = json!("SessionStart");
    let lines = [start,prompt.clone(),with_uuid(failure,"h1"),with_uuid(end,"d1")];
    let expected = vec![
        ("hookfail:s1".to_owned(),"hook_failed".to_owned(),Some("SessionStart:startup: falhou-de-proposito".to_owned())),
        ("hookfail:h1".to_owned(),"hook_failed".to_owned(),Some("PostToolUse:Bash: falhou-de-proposito".to_owned())),
    ];
    let (history,live) = both(&lines);
    assert_eq!(history,expected);
    assert_eq!(live,expected);
}

/// Notas da cauda de um aparelho (`tail::backfill`), lidas dos quadros SSE.
fn backfill_notes(lines:&[Value],resume_at:Option<usize>) -> Vec<(String,String,Option<String>)> {
    let text:String = lines.iter().map(|row|format!("{row}\n")).collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path,&text).unwrap();
    let resume = resume_at.map(|at|format!("k:{}",lines[..at].iter().map(|row|row.to_string().len() as u64 + 1).sum::<u64>()));
    let frames = hangar_server::tail::backfill(&path,"k",Provider::Claude,resume.as_deref(),text.len() as u64);
    notes(frames.iter().filter_map(|frame|{
        let frame = std::str::from_utf8(frame).unwrap();
        let data = frame.lines().find_map(|row|row.strip_prefix("data: "))?;
        serde_json::from_str::<ChatEvent>(data).ok()
    }).collect())
}

/// O aparelho que volta depois da primeira falha: as repetições não viram nota nova, porque as falhas de antes
/// do ponto de retomada foram contadas.
#[test]
fn terminal_resumed_tail_after_the_first_failure_adds_no_note() {
    let rows = rows("terminal_hooks.jsonl");
    let (prompt,failure,end) = (&rows[0],&rows[1],&rows[4]);
    let mut queued = with_uuid(failure,"q1");
    queued["attachment"] = json!({"type":"queued_command","prompt":"mensagem da fila","commandMode":"prompt"});
    // O segundo prompt é o real: a cópia do primeiro com outro `uuid` cai no filtro de reescrita do `--resume`.
    let lines = [prompt.clone(),with_uuid(end,"d0"),rows[5].clone(),with_uuid(failure,"h1"),queued,
        with_uuid(failure,"h2"),with_uuid(failure,"h3"),with_uuid(end,"d1")];
    assert_eq!(both(&lines).0,vec![h1(3)]);
    assert_eq!(backfill_notes(&lines,Some(4)),vec![]);
    assert_eq!(backfill_notes(&lines,Some(3)),vec![h1(1)],"retomada antes da primeira falha ainda a mostra");
}

/// Primeira falha fora das 200 linhas da cauda e da janela do `/history`: a repetição dentro delas não vira nota.
#[test]
fn terminal_repetition_far_from_the_first_failure_adds_no_note() {
    let rows = rows("terminal_hooks.jsonl");
    let (prompt,failure,end) = (&rows[0],&rows[1],&rows[4]);
    let mut lines = vec![prompt.clone(),with_uuid(end,"d0"),rows[5].clone(),with_uuid(failure,"h1")];
    for i in 0..250 {
        let mut filler = with_uuid(prompt,&format!("f{i}"));
        filler["type"] = json!("assistant");
        filler["message"] = json!({"role":"assistant","content":[{"type":"text","text":"ok"}]});
        lines.push(filler);
    }
    lines.extend([with_uuid(failure,"h2"),with_uuid(end,"d1")]);
    assert_eq!(backfill_notes(&lines,None),vec![]);
    let text:String = lines.iter().map(|row|format!("{row}\n")).collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path,&text).unwrap();
    let full = notes(merged_history_with_notices(&request(path.clone(),None,TAIL_WINDOW)).unwrap());
    assert_eq!(full,vec![h1(2)]);
    // A janela curta cresce para trás até ter os itens pedidos: ou traz a nota de `h1`, ou nenhuma. Nunca uma de `h2`.
    let window = notes(merged_history_with_notices(&request(path,Some(5),2048)).unwrap());
    assert!(window.iter().all(|note|full.contains(note)),"{window:?} × {full:?}");
}

/// A marca da falha dentro do texto de um prompt ou de uma mensagem da fila não é falha: a contagem das linhas
/// de antes da janela só olha anexo de hook.
#[test]
fn terminal_marker_inside_a_prompt_is_not_a_failure() {
    let rows = rows("terminal_hooks.jsonl");
    let (prompt,failure,end) = (&rows[0],&rows[1],&rows[4]);
    let mut talk = with_uuid(prompt,"t1");
    talk["message"] = json!({"role":"user","content":"o anexo hook_non_blocking_error do PostToolUse:Bash aparece no transcript"});
    let mut queued = with_uuid(failure,"q1");
    queued["attachment"] = json!({"type":"queued_command","prompt":"conta os hook_non_blocking_error","commandMode":"prompt"});
    let mut lines = vec![talk,queued,with_uuid(end,"d0"),rows[5].clone()];
    for i in 0..250 {
        let mut filler = with_uuid(prompt,&format!("f{i}"));
        filler["type"] = json!("assistant");
        filler["message"] = json!({"role":"assistant","content":[{"type":"text","text":"ok"}]});
        lines.push(filler);
    }
    lines.extend([with_uuid(failure,"h1"),with_uuid(end,"d1")]);
    assert_eq!(backfill_notes(&lines,None),vec![h1(1)],"a cauda começa depois das duas linhas com a marca");
    let text:String = lines.iter().map(|row|format!("{row}\n")).collect();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.jsonl");
    std::fs::write(&path,&text).unwrap();
    assert_eq!(notes(merged_history_with_notices(&request(path,Some(3),2048)).unwrap()),vec![h1(1)]);
}

#[test]
fn terminal_history_tail_never_invents_a_different_note() {
    let full = history("terminal_hooks.jsonl",None,TAIL_WINDOW);
    let text = std::fs::read_to_string(fixture("terminal_hooks.jsonl")).unwrap();
    let starts:Vec<u64> = std::iter::once(0).chain(text.match_indices('\n').map(|(at,_)|at as u64 + 1)).collect();
    // Janela que começa em cada linha do meio (+1: o `tail_offset` pula a linha cortada). `limit` 3 = os itens da
    // janela que começa na primeira falha (a nota e 2 prompts): ela para ali, sem voltar ao byte 0, e não corta a nota.
    let mut seen = 0;
    for start in &starts[1..starts.len() - 1] {
        let tail = history("terminal_hooks.jsonl",Some(3),text.len() as u64 - start + 1);
        assert!(tail.iter().all(|note|full.contains(note)),"linha em {start}: {tail:?} fora de {full:?}");
        seen += tail.len();
    }
    assert!(seen > 0,"nenhuma janela parcial montou nota: a asserção acima não provaria nada");
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

/// A nova tentativa pode ser ignorada; a recusa é bloqueio real e sai sem chave.
#[test]
fn terminal_retry_and_refusal_carry_the_map() {
    let events = |name:&str| merged_history_with_notices(&request(fixture(name),None,TAIL_WINDOW)).unwrap();
    let metas = |name:&str,prefix:&str| -> Vec<Value> {
        events(name).into_iter().filter(|event|event.id.starts_with(prefix)).map(|event|Value::Object(event.notice_meta.expect("aviso sem mapa"))).collect()
    };
    assert_eq!(metas("terminal_retry.jsonl","retry"),vec![json!({"key":"note:retry"});3],"as duas tentativas e a volta");
    assert_eq!(metas("terminal_refusal.jsonl","refusal:"),vec![json!({})]);
    let api_error = rows("terminal_retry.jsonl").iter().filter(|row|row["isApiErrorMessage"] == true).filter_map(|row|row["uuid"].as_str().map(str::to_owned)).collect::<Vec<_>>();
    let (errors,replies):(Vec<_>,Vec<_>) = events("terminal_retry.jsonl").into_iter().filter(|event|!is_note(event))
        .partition(|event|api_error.iter().any(|uuid|event.id.starts_with(uuid.as_str())));
    assert!(!errors.is_empty() && errors.iter().all(|event|event.notice_meta == json!({"level":"error"}).as_object().cloned()),"a resposta de erro da API é bloqueio");
    assert!(replies.iter().all(|event|event.notice_meta.is_none()),"resposta comum não é aviso");
}

#[test]
fn terminal_refusal_note_only_on_cli_lines() {
    let lines = rows("terminal_refusal.jsonl");
    let expected = vec![(format!("refusal:{}",uuid(&lines,1)),"⚙️ O modelo recusou responder".to_owned(),None)];
    assert_eq!(history("terminal_refusal.jsonl",None,TAIL_WINDOW),expected);
    assert_eq!(live("terminal_refusal.jsonl",true),expected);
}
