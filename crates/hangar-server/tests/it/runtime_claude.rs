use crate::mods_support;

use hangar_server::runtime::{claude::ClaudeEngine, protocol::*};
use hangar_server::mods::model::{ModsCall,SurfaceEffect};
use serde_json::{Value, json};

#[test]
fn model_effort_intent_is_not_an_extra_cli_model_field() {
    let mut engine = ClaudeEngine::new(json!({"name":"session","session_id":"sid","initialized":true}),1,
        ClockSample { monotonic_s:0.0,epoch_s:1_800_000_000.0 });
    let effects = engine.command(RuntimeCommand { operation_id:"model".into(),kind:OperationKind::SetModel,
        payload:json!({"model":"test-model","effort":"high"}) },ClockSample { monotonic_s:1.0,epoch_s:1_800_000_001.0 }).unwrap();
    let request = effects.iter().find_map(|effect|match effect { Effect::Write { frame,.. }=>Some(&frame["request"]),_=>None }).unwrap();
    assert_eq!(request,&json!({"subtype":"set_model","model":"test-model"}));
}

#[test]
fn hydrated_reader_requests_usage_and_reload_stamp_without_viewers() {
    let mut engine = engine(json!({"name":"session","session_id":"sid","initialized":true}));
    let snapshot = CanoSnapshot::parse(mods_support::cano_snapshot_json()).unwrap();
    let effects = engine.hydrate(snapshot).unwrap();
    for service in ["last_usage","reload_stamp"] {
        assert!(effects.iter().any(|effect|matches!(effect,Effect::Policy { kind,.. } if kind == service)));
    }
}

fn clock(seconds:f64) -> ClockSample { ClockSample { monotonic_s:seconds,epoch_s:1_800_000_000.0 + seconds } }
fn engine(metadata:Value) -> ClaudeEngine { ClaudeEngine::new(metadata,1,clock(10.0)) }
fn line(engine:&mut ClaudeEngine,value:Value,seconds:f64) -> Vec<Effect> { engine.apply(EngineInput::Line(value),clock(seconds)).unwrap() }
fn command(kind:OperationKind,payload:Value) -> RuntimeCommand { RuntimeCommand { operation_id:"op-1".into(),kind,payload } }
fn writes(effects:&[Effect]) -> Vec<Value> { effects.iter().filter_map(|e|match e { Effect::Write { frame,.. } => Some(frame.clone()),_=>None }).collect() }

#[test]
fn init_timeout_not_deliverable() {
    let mut engine = engine(json!({"name":"session"}));
    let effects = engine.start_initialize("init".into()).unwrap();
    let request_id = writes(&effects)[0]["request_id"].clone();
    let timeout = engine.apply(EngineInput::Tick,clock(190.0)).unwrap();
    assert!(timeout.iter().any(|e|matches!(e,Effect::Reply { disposition:Disposition::Unknown,.. })));
    assert_eq!(engine.view()["iniciando"],true);
    assert_eq!(engine.view()["deliverable"],false);
    line(&mut engine,json!({"type":"control_response","response":{
        "request_id":request_id,"subtype":"success","response":{"commands":[]}}}),191.0);
    assert_eq!(engine.view()["iniciando"],false);
    assert_eq!(engine.view()["deliverable"],true);
}

#[test]
fn pending_plan_rules() {
    let mut engine = engine(json!({"name":"session","permission_mode":"plan","previous_non_plan":"bypassPermissions","initialized":true}));
    let read = line(&mut engine,json!({"type":"control_request","request_id":1,
        "request":{"subtype":"can_use_tool","tool_name":"Read","input":{}}}),10.0);
    assert_eq!(writes(&read)[0]["response"]["response"]["behavior"],"allow");
    for (index,tool) in ["ExitPlanMode","Edit","Write","MultiEdit","NotebookEdit"].into_iter().enumerate() {
        let effects = line(&mut engine,json!({"type":"control_request","request_id":index+2,
            "request":{"subtype":"can_use_tool","tool_name":tool,"input":{}}}),10.0);
        assert!(writes(&effects).is_empty());
    }
    assert_eq!(engine.view()["pending"].as_array().unwrap().len(),5);
}

#[test]
fn question_validates_current_request() {
    let mut engine = engine(json!({"name":"session","initialized":true}));
    line(&mut engine,json!({"type":"control_request","request_id":1,"request":{
        "subtype":"can_use_tool","tool_name":"AskUserQuestion","input":{"questions":[{
        "question":"Qual opção?","header":"Opção","multiSelect":true,
        "options":[{"label":"A","description":"Primeira"},{"label":"B","description":"Segunda"}]}]}}}),10.0);
    assert!(engine.command(command(OperationKind::AnswerQuestions,json!({"request_id":"1","answers":[]})),clock(10.0)).is_err());
    let effects = engine.command(command(OperationKind::AnswerQuestions,json!({"request_id":1,"answers":[{"kind":"option","indices":[0,1]}]})),clock(10.0)).unwrap();
    assert_eq!(writes(&effects)[0]["response"]["response"]["updatedInput"]["answers"]["Qual opção?"],"A, B");
}

#[test]
fn interrupt_denies_all_pending_first() {
    let mut engine = engine(json!({"name":"session","initialized":true}));
    for id in [1,2] { line(&mut engine,json!({"type":"control_request","request_id":id,
        "request":{"subtype":"can_use_tool","tool_name":"Edit","input":{}}}),10.0); }
    let effects = engine.command(command(OperationKind::Interrupt,json!({})),clock(10.0)).unwrap();
    let frames = writes(&effects);
    assert_eq!(frames.len(),3);
    assert_eq!(frames[0]["response"]["response"]["behavior"],"deny");
    assert_eq!(frames[1]["response"]["response"]["behavior"],"deny");
    assert_eq!(frames[2]["request"]["subtype"],"interrupt");
}

#[test]
fn subagent_does_not_close_parent() {
    let mut engine = engine(json!({"name":"session","initialized":true}));
    line(&mut engine,json!({"type":"command_lifecycle","state":"started"}),10.0);
    line(&mut engine,json!({"type":"result","subtype":"success","parent_tool_use_id":"child"}),11.0);
    assert_eq!(engine.view()["in_progress"],true);
}

#[test]
fn usage_last_call_is_context() {
    let mut engine = engine(json!({"name":"session","initialized":true}));
    line(&mut engine,json!({"type":"assistant","message":{"content":[],"usage":{"input_tokens":2,"cache_read_input_tokens":39000,"output_tokens":4}}}),10.0);
    line(&mut engine,json!({"type":"result","subtype":"success","usage":{"input_tokens":500000}}),11.0);
    line(&mut engine,json!({"type":"assistant","message":{"content":[],"usage":{"input_tokens":0}}}),12.0);
    assert_eq!(engine.view()["usage"]["cache_read_input_tokens"],39000);
}

#[test]
fn first_150ms_last_and_clear() {
    let mut engine = engine(json!({"name":"session","initialized":true}));
    let mut publications = Vec::new();
    for (time,event) in [(10.0,json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"Olá "}})),
        (10.1,json!({"type":"content_block_delta","delta":{"type":"text_delta","text":"mundo"}}))] {
        publications.extend(line(&mut engine,json!({"type":"stream_event","event":event}),time));
    }
    publications.extend(engine.apply(EngineInput::Tick,clock(10.15)).unwrap());
    publications.extend(line(&mut engine,json!({"type":"assistant","message":{"content":[{"type":"text","text":"Olá mundo"}]}}),10.2));
    publications.extend(engine.apply(EngineInput::Tick,clock(10.3)).unwrap());
    let texts:Vec<_> = publications.into_iter().filter_map(|e|match e {
        Effect::Publish { channel,data } if channel == "preview" => Some(data["text"].clone()),_=>None }).collect();
    assert_eq!(texts,vec![json!("Olá "),json!("Olá mundo"),json!("")]);
}

#[test]
fn unknown_control_is_neutral() {
    let mut engine = engine(json!({"name":"session"}));
    let effects = line(&mut engine,json!({"type":"control_request","request_id":"future",
        "request":{"subtype":"future_method"}}),10.0);
    assert_eq!(writes(&effects)[0]["response"]["response"],json!({}));
    assert!(effects.iter().any(|e|matches!(e,Effect::Policy { kind,.. } if kind == "unknown_private")));
}

#[test]
fn clear_discards_old_control_and_queued_write() {
    let mut engine = engine(json!({"name":"session", "initialized":true, "session_id":"before"}));
    let effects = engine.command(command(OperationKind::SetModel,json!({"model":"next"})),clock(10.0)).unwrap();
    let request_id = writes(&effects)[0]["request_id"].clone();
    let reset = line(&mut engine,json!({"type":"system", "subtype":"init", "session_id":"after"}),11.0);
    assert!(reset.iter().any(|effect|matches!(effect,Effect::Reply { disposition:Disposition::Unknown,.. })));
    assert!(!engine.write_is_current("op-1"));
    line(&mut engine,json!({"type":"control_response", "response":{"request_id":request_id,
        "subtype":"success", "response":{}}}),12.0);
    assert_ne!(engine.view()["model"], "next");
    assert_eq!(engine.view()["conversation"], "after");
}

#[test]
fn effort_is_deferred_until_a_safe_boundary() {
    let mut engine = engine(json!({"name":"session","initialized":true,"effort":"medium"}));
    line(&mut engine,json!({"type":"command_lifecycle","state":"started"}),10.0);
    let effects = engine.command(command(OperationKind::SetEffort,json!({"effort":"high"})),clock(11.0)).unwrap();
    assert!(writes(&effects).is_empty());
    assert_eq!(engine.view()["effort_intent"]["operation_id"],"op-1");
    let effects = line(&mut engine,json!({"type":"result","subtype":"success"}),12.0);
    assert!(effects.iter().any(|e|matches!(e,Effect::Write { operation_id:Some(id),.. } if id == "op-1:effort")));
    assert_eq!(engine.view()["effort"],"medium");
    line(&mut engine,json!({"type":"assistant","local_command_source":"/effort","message":{
        "content":[{"type":"text","text":"Set effort level to high (this session only)"}]}}),13.0);
    assert_eq!(engine.view()["effort"],"high");
}

/// A CLI põe a SAÍDA no `local_command_source`, não o comando.
#[test]
fn local_answer_carries_the_slash_command_that_was_written() {
    let mut engine = engine(json!({"name":"session","initialized":true}));
    engine.command(command(OperationKind::Steer,json!({"text":"/btw"})),clock(11.0)).unwrap();
    let effects = line(&mut engine,json!({"type":"assistant",
        "local_command_source":"<local-command-stdout>/btw isn't available in this environment.</local-command-stdout>",
        "message":{"content":[{"type":"text","text":"/btw isn't available in this environment."}]}}),12.0);
    assert!(effects.iter().any(|e|matches!(e,Effect::Policy { kind,payload,.. } if kind == "local_output" && payload["source"] == "/btw")));
    let again = line(&mut engine,json!({"type":"assistant","local_command_source":"<local-command-stdout>x</local-command-stdout>",
        "message":{"content":[{"type":"text","text":"x"}]}}),13.0);
    assert!(again.iter().any(|e|matches!(e,Effect::Policy { kind,payload,.. } if kind == "local_output" && payload["source"].is_null())));
}

#[test]
fn late_ack_does_not_override_result() {
    let mut engine = engine(json!({"name":"session"}));
    let effects = engine.start_initialize("init".into()).unwrap();
    let request_id = writes(&effects)[0]["request_id"].clone();
    line(&mut engine,json!({"type":"control_response","response":{
        "request_id":request_id,"subtype":"success","response":{}}}),11.0);
    let effects = engine.apply(EngineInput::WriteAck { operation_id:"init".into(),outcome:WriteOutcome::Unknown },clock(12.0)).unwrap();
    assert!(!effects.iter().any(|e|matches!(e,Effect::Reply { disposition:Disposition::Unknown,.. })));
    assert_eq!(engine.view()["deliverable"],true);
}

#[test]
fn mods_call_needs_a_surface_and_a_live_attach() {
    let mut engine = engine(json!({"name":"session","initialized":true}));
    let refused = engine.mods_call(1,ModsCall::Show { site:"p".into() },7.0,clock(10.0)).err().unwrap();
    assert_eq!(refused.code,"erro_mod_botao_inexistente","sem superfície ligada");
    engine.enable_surface("ui:t".into());
    let effects = engine.mods_call(2,ModsCall::Show { site:"p".into() },7.0,clock(10.0)).unwrap();
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Surface { effect:SurfaceEffect::Reply { token:2,result:Err(error) } }
        if error.code == "erro_mod_painel_inexistente")),"superfície ainda não ligada responde na hora");
}

fn surface_engine(metadata:Value) -> ClaudeEngine { let mut engine = engine(metadata); engine.enable_surface("ui:t".into()); engine }
fn surface_writes(effects:&[Effect]) -> Vec<Value> {
    effects.iter().filter_map(|e|match e { Effect::Surface { effect:SurfaceEffect::Write { frame, .. } }=>Some(frame.clone()),_=>None }).collect()
}
fn snapshot() -> CanoSnapshot { CanoSnapshot::parse(mods_support::cano_snapshot_json()).unwrap() }
/// Motor com a superfície ligada e a faixa já desenhada (nenhum desenho em voo).
fn attached() -> ClaudeEngine {
    let mut engine = surface_engine(json!({"name":"session","initialized":true}));
    let attach = surface_writes(&engine.hydrate(snapshot()).unwrap())[0].clone();
    let effects = line(&mut engine,json!({"type":"control_response","response":{"subtype":"success","request_id":attach["request_id"],"response":{"surfaces":["desktop"]}}}),11.0);
    let band = surface_writes(&effects).into_iter().find(|frame|frame["request"]["subtype"] == "ui_render").unwrap();
    line(&mut engine,json!({"type":"control_response","response":{"subtype":"success","request_id":band["request_id"],
        "response":{"tree":{"type":"Text"},"hooked":true}}}),11.0);
    engine
}

#[test]
fn surface_attaches_only_when_initialize_offers_it() {
    for (capabilities,attaches) in [(json!(["ui_surface_v1"]),true),(json!([]),false)] {
        let mut engine = surface_engine(json!({"name":"session"}));
        let request_id = writes(&engine.start_initialize("init".into()).unwrap())[0]["request_id"].clone();
        let effects = line(&mut engine,json!({"type":"control_response","response":{"request_id":request_id,"subtype":"success",
            "response":{"commands":[],"capabilities":capabilities}}}),11.0);
        let attach = surface_writes(&effects).into_iter().find(|frame|frame["request"]["subtype"] == "ui_attach");
        assert_eq!(attach.is_some(),attaches);
        assert!(writes(&effects).iter().all(|frame|frame["request"]["subtype"] != "ui_attach"),"ui_attach nunca vai pelo diário");
    }
}

#[test]
fn hydrated_initialized_session_attaches_again() {
    let mut surfaced = surface_engine(json!({"name":"session","initialized":true}));
    let frames = surface_writes(&surfaced.hydrate(snapshot()).unwrap());
    assert_eq!(frames[0]["request"]["subtype"],"ui_attach");
    let mut plain = engine(json!({"name":"session","initialized":true}));
    assert!(surface_writes(&plain.hydrate(snapshot()).unwrap()).is_empty(),"sem superfície ligada nada sai");
}

#[test]
fn hydrate_with_the_process_gone_does_not_attach() {
    // A15: com `saiu` no retrato, o `ui_attach` iria a um processo morto e venceria em 15 s.
    let mut engine = surface_engine(json!({"name":"session","initialized":true}));
    let mut gone = mods_support::cano_snapshot_json();
    gone["saiu"] = json!(0);
    assert!(surface_writes(&engine.hydrate(CanoSnapshot::parse(gone).unwrap()).unwrap()).is_empty());
    // Não iniciada: a ligação espera o `initialize` dizer se o processo aceita a superfície.
    let mut fresh = surface_engine(json!({"name":"session"}));
    assert!(surface_writes(&fresh.hydrate(snapshot()).unwrap()).is_empty());
}

#[test]
fn ui_lines_go_to_the_surface_and_never_to_the_unknown_log() {
    let mut engine = surface_engine(json!({"name":"session","initialized":true}));
    let attach = surface_writes(&engine.hydrate(snapshot()).unwrap())[0].clone();
    let effects = line(&mut engine,json!({"type":"control_response","response":{"subtype":"success","request_id":attach["request_id"],"response":{"surfaces":["desktop"]}}}),11.0);
    assert!(surface_writes(&effects).iter().any(|frame|frame["request"]["subtype"] == "ui_render"));
    assert!(!effects.iter().any(|e|matches!(e,Effect::Reply { .. })),"resposta ui_* não fecha operação da fila");
    let effects = line(&mut engine,json!({"type":"system","subtype":"ui_toast","plugin":"vitrine","text":"oi","timeout_ms":4000,"uuid":"u","session_id":"s"}),11.0);
    assert!(effects.iter().any(|e|matches!(e,Effect::Surface { effect:SurfaceEffect::Toast { text,.. } } if text == "oi")));
    assert!(!effects.iter().any(|e|matches!(e,Effect::Policy { kind,.. } if kind == "unknown_private" || kind == "local_output")));
}

#[test]
fn ui_copy_is_answered_off_the_journal() {
    let mut engine = attached();
    let effects = line(&mut engine,json!({"type":"control_request","request_id":"uuid-1","request":{"subtype":"ui_copy","surface":"desktop","client_id":"hangar","plugin":"vitrine","text":"Texto"}}),12.0);
    assert!(writes(&effects).is_empty(),"nada pelo diário");
    assert_eq!(surface_writes(&effects)[0]["response"]["response"],json!({"copied":true}));
    assert!(effects.iter().any(|e|matches!(e,Effect::Surface { effect:SurfaceEffect::Copied { text,.. } } if text == "Texto")));
}

#[test]
fn without_surface_ui_copy_keeps_the_empty_answer() {
    let mut engine = engine(json!({"name":"session","initialized":true}));
    let effects = line(&mut engine,json!({"type":"control_request","request_id":"uuid-1","request":{"subtype":"ui_copy","text":"x"}}),12.0);
    assert_eq!(writes(&effects)[0]["response"]["response"],json!({}));
}

#[test]
fn process_exit_fails_pending_mods_calls() {
    let mut engine = attached();
    line(&mut engine,json!({"type":"system","subtype":"ui_panes","panes":[{"id":"p","title":"P","plugin":"m"}],"shown_id":"p","focused_id":null,"focus_requested_id":null}),12.0);
    engine.mods_call(5,ModsCall::Show { site:"p".into() },7.0,clock(12.0)).unwrap();
    let effects = line(&mut engine,json!({"type":"cano_saiu","rc":1}),13.0);
    assert!(effects.iter().any(|e|matches!(e,Effect::Surface { effect:SurfaceEffect::Reply { token:5,result:Err(error) } } if error.code == "erro_mod_clique_sem_resposta")));
    assert!(effects.iter().any(|e|matches!(e,Effect::Surface { effect:SurfaceEffect::Publish { data } } if data["panes"] == json!([]))));
}

#[test]
fn turn_start_redraws_the_band_with_is_working() {
    let mut engine = attached();
    line(&mut engine,json!({"type":"command_lifecycle","state":"started"}),12.0);
    let effects = engine.apply(EngineInput::Tick,clock(12.2)).unwrap();
    let band = surface_writes(&effects).into_iter().find(|frame|frame["request"]["component"] == "AbovePrompt").expect("faixa pedida de novo");
    assert_eq!(band["request"]["props"]["isWorking"],true);
}

#[test]
fn conversation_reset_redraws_the_band_without_is_working() {
    let mut engine = attached();
    line(&mut engine,json!({"type":"command_lifecycle","state":"started"}),12.0);
    let effects = engine.apply(EngineInput::Tick,clock(12.2)).unwrap();
    let working = surface_writes(&effects).into_iter().find(|frame|frame["request"]["component"] == "AbovePrompt").unwrap();
    line(&mut engine,json!({"type":"control_response","response":{"subtype":"success","request_id":working["request_id"],
        "response":{"tree":{"type":"Text"},"hooked":true}}}),12.3);
    line(&mut engine,json!({"type":"conversation_reset"}),12.4);
    assert_eq!(engine.view()["in_progress"],false);
    let effects = engine.apply(EngineInput::Tick,clock(12.6)).unwrap();
    let band = surface_writes(&effects).into_iter().find(|frame|frame["request"]["component"] == "AbovePrompt").expect("faixa pedida de novo");
    assert_eq!(band["request"]["props"]["isWorking"],false);
}

#[test]
fn surface_deadline_enters_the_engine_clock() {
    // Sem o prazo da superfície no relógio, o ator não acordaria para ligar de novo.
    let mut engine = surface_engine(json!({"name":"session","initialized":true}));
    engine.hydrate(snapshot()).unwrap();
    // Primeiro vence o carimbo de recarga (20); passado ele, o próximo é o prazo da ligação (10 + 15).
    assert_eq!(engine.next_deadline(),Some(20.0));
    engine.apply(EngineInput::Tick,clock(20.0)).unwrap();
    assert_eq!(engine.next_deadline(),Some(25.0),"prazo da ligação no relógio do motor");
    let effects = engine.apply(EngineInput::Tick,clock(25.0)).unwrap();
    assert!(surface_writes(&effects).is_empty(),"ligação vencida espera 1 s antes de tentar de novo");
    assert!(engine.next_deadline().is_some_and(|deadline|deadline <= 26.0 + 1e-9),"a nova tentativa entra no relógio");
    let effects = engine.apply(EngineInput::Tick,clock(26.0)).unwrap();
    assert_eq!(surface_writes(&effects)[0]["request"]["subtype"],"ui_attach");
}

#[test]
fn turn_already_running_when_the_engine_opens_is_working_until_result() {
    let mut engine = engine(json!({"name":"session","session_id":"sid","initialized":true}));
    assert_eq!(engine.view()["deliverable"],true);
    for (index,event) in [
        json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"meio"}}}),
        json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"ls"}}]}}),
        json!({"type":"tool_progress","tool_use_id":"t1"}),
        json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"ok"}]}}),
    ].into_iter().enumerate() {
        line(&mut engine,event,11.0+index as f64);
        assert_eq!(engine.view()["public_state"]["state"],"working","evento {index}");
        assert_eq!(engine.view()["deliverable"],false,"evento {index}");
    }
    line(&mut engine,json!({"type":"result","subtype":"success"}),20.0);
    assert_eq!(engine.view()["public_state"]["state"],"idle");
    assert_eq!(engine.view()["deliverable"],true);
}

#[test]
fn events_outside_a_turn_do_not_open_one() {
    for event in [
        json!({"type":"system","subtype":"init","session_id":"sid","model":"m"}),
        json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}),
        json!({"type":"keep_alive"}),
        json!({"type":"assistant","local_command_source":"cost","message":{"content":[{"type":"text","text":"Total cost: $0"}]}}),
        json!({"type":"user","message":{"content":"<local-command-stdout>ok</local-command-stdout>"}}),
        json!({"type":"stream_event","parent_tool_use_id":"t9","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"sub"}}}),
    ] {
        let mut engine = engine(json!({"name":"session","session_id":"sid","initialized":true}));
        line(&mut engine,event.clone(),11.0);
        assert_eq!(engine.view()["in_progress"],false,"{event}");
        assert_eq!(engine.view()["deliverable"],true,"{event}");
    }
}
