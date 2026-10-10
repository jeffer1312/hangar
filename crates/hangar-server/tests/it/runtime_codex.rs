use hangar_server::runtime::{codex::Engine,protocol::*};
use serde_json::{Value,json};

fn clock(seconds:f64) -> ClockSample { ClockSample { monotonic_s:seconds,epoch_s:1_800_000_000.0 + seconds } }
fn engine() -> Engine { Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true,"model":"gpt-6","initialized":true,"ready":true}),1,clock(10.0)) }
fn frames(effects:&[Effect]) -> Vec<Value> { effects.iter().filter_map(|e|match e { Effect::Write { frame,.. }=>Some(frame.clone()),_=>None }).collect() }
fn command(kind:OperationKind,payload:Value) -> RuntimeCommand { RuntimeCommand { operation_id:"op-1".into(),kind,payload } }
fn line(engine:&mut Engine,value:Value,time:f64) -> Vec<Effect> { engine.apply(EngineInput::Line(value),clock(time)).unwrap() }

#[test]
fn initialize_then_resume() {
    let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true}),1,clock(10.0));
    let effects = engine.bootstrap(true,"boot".into()).unwrap();
    let initialize = frames(&effects)[0].clone();
    assert_eq!(initialize["method"],"initialize");
    let effects = line(&mut engine,json!({"id":initialize["id"],"result":{}}),11.0);
    let requests = frames(&effects);
    assert_eq!(requests[0]["method"],"initialized");
    assert_eq!(requests[1]["method"],"thread/resume");
    assert_eq!(requests[1]["params"]["threadId"],"thread-1");
}

#[test]
fn takeover_restores_async_question_before_ready() {
    let question = json!({"provider":"codex", "request_id":"async:thread-1:item:0", "is_async":true,
        "questions":[{"id":"answer", "question":"Qual opção?", "options":[]}]});
    let engine = Engine::new(json!({"name":"session", "thread_id":"thread-1", "initialized":true, "ready":true,
        "async_questions":[["async:thread-1:item:0",question]], "async_seen":["item"]}),1,clock(10.0));
    assert_eq!(engine.view()["state"], "awaiting_input");
    assert_eq!(engine.view()["codex_question"]["request_id"], "async:thread-1:item:0");
    assert_eq!(engine.control_view()["deliverable"], false);
}

#[test]
fn reply_ids_and_generations() {
    let mut engine = engine();
    let effects = engine.command(command(OperationKind::ListModels,json!({})),clock(10.0)).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    assert!(id.as_str().unwrap().starts_with("hangar:1:"));
    let old = line(&mut engine,json!({"id":"hangar:0:1","result":{"data":[]}}),11.0);
    assert!(!old.iter().any(|e|matches!(e,Effect::Reply { .. })));
    let current = line(&mut engine,json!({"id":id,"result":{"data":[]}}),12.0);
    assert!(current.iter().any(|e|matches!(e,Effect::Reply { disposition:Disposition::Accepted,.. })));
}

#[test]
fn rpc_timeout_keeps_pending() {
    let mut engine = engine();
    let effects = engine.command(command(OperationKind::Input,json!({"text":"Olá"})),clock(10.0)).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    let timeout = engine.apply(EngineInput::Tick,clock(40.0)).unwrap();
    assert!(timeout.iter().any(|e|matches!(e,Effect::Reply { disposition:Disposition::Unknown,.. })));
    let reply = line(&mut engine,json!({"id":id,"result":{"turn":{"id":"turn-1"}}}),41.0);
    assert!(reply.iter().any(|e|matches!(e,Effect::Reply { disposition:Disposition::Accepted,.. })));
    assert!(frames(&reply).is_empty());
}

#[test]
fn server_request_once() {
    let mut engine = engine();
    let request = json!({"id":1,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","command":"pwd"}});
    line(&mut engine,request.clone(),10.0);
    let response = engine.command(command(OperationKind::Select,json!({"option":1})),clock(10.0)).unwrap();
    assert_eq!(frames(&response)[0]["id"],1);
    assert!(engine.command(command(OperationKind::Select,json!({"option":1})),clock(10.0)).is_err());
    let effects = line(&mut engine,json!({"id":"unknown","method":"future/request","params":{"threadId":"thread-1"}}),11.0);
    assert_eq!(frames(&effects)[0]["error"]["code"],-32601);
}

#[test]
fn turn_end_idle_before_drain() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    let effects = line(&mut engine,json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}),11.0);
    assert_eq!(engine.view()["state"],"idle");
    let state = effects.iter().position(|e|matches!(e,Effect::StateChanged)).unwrap();
    let drain = effects.iter().position(|e|matches!(e,Effect::WakeQueue)).unwrap();
    assert!(state < drain);
}

#[test]
fn first_response_times_only_the_first_nonempty_delta_of_the_current_turn() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    for (thread,turn,text) in [("other","turn-1","text"),("thread-1","old","text"),("thread-1","turn-1","")] {
        let effects = line(&mut engine,json!({"method":"item/agentMessage/delta",
            "params":{"threadId":thread,"turnId":turn,"delta":text}}),11.0);
        assert!(!effects.iter().any(|effect|matches!(effect,Effect::Publish { channel,.. } if channel == "rate")));
    }
    let effects = line(&mut engine,json!({"method":"item/agentMessage/delta",
        "params":{"threadId":"thread-1","turnId":"turn-1","delta":"text"}}),12.0);
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Publish { channel,data }
        if channel == "rate" && data == &json!({"first_response":true,"seconds":2.0,"conversation":"thread-1"}))));
    let effects = line(&mut engine,json!({"method":"item/agentMessage/delta",
        "params":{"threadId":"thread-1","turnId":"turn-1","delta":"more"}}),13.0);
    assert!(!effects.iter().any(|effect|matches!(effect,Effect::Publish { channel,.. } if channel == "rate")));
}

#[test]
fn thread_switch_clears_old_preview_and_foreign_deltas_are_ignored() {
    let mut engine = engine();
    let effects = line(&mut engine,json!({"method":"item/agentMessage/delta","params":{"threadId":"other","delta":"filho"}}),10.0);
    assert!(!effects.iter().any(|e|matches!(e,Effect::Publish { .. })));
}

#[test]
fn sandbox_requires_idle() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    assert!(engine.command(command(OperationKind::SetPermissionMode,json!({"mode":"Ask for approval"})),clock(10.0)).is_err());
}

#[test]
fn terminal_adapter_untouched() {
    let mut engine = Engine::new(json!({"name":"session","headless":false}),1,clock(10.0));
    assert!(engine.bootstrap(true,"boot".into()).is_err());
}

#[test]
fn question_hydrate_merges_notifications() {
    let mut engine = engine();
    let read = engine.command(command(OperationKind::ReadSettings,json!({"include_turns":true})),clock(10.0)).unwrap();
    let id = frames(&read)[0]["id"].clone();
    line(&mut engine,json!({"method":"item/completed","params":{"threadId":"thread-1","item":{
        "id":"question-new","type":"agentMessage","delivery":"async","questions":[{"title":"Nova pergunta","options":["A","B"]}]}}}),11.0);
    line(&mut engine,json!({"id":id,"result":{"thread":{"id":"thread-1","status":{"type":"idle"},"turns":[]}}}),12.0);
    assert_eq!(engine.view()["codex_question"]["questions"][0]["question"],"Nova pergunta");
    line(&mut engine,json!({"method":"item/completed","params":{"threadId":"thread-1","item":{
        "id":"ordinary-user","type":"userMessage","content":[{"type":"text","text":"> Nova pergunta\n\nA"}]}}}),13.0);
    assert_eq!(engine.view()["codex_question"]["questions"][0]["question"],"Nova pergunta");
}

#[test]
fn preview_full_prefix_on_takeover() {
    let mut engine = engine();
    let snapshot = CanoSnapshot::parse(json!({"type":"cano_snapshot","versao":2,"pid":42,"init":null,
        "aberto":false,"pendentes":[],"ultimo_result":null,"rate_limit":null,"stderr_tail":[],"saiu":null,
        "inflight":{"codex":{"thread-1":{"complete":true,"text":"Olá 🌎","itemId":"item-1","turnId":"turn-1"}}}})).unwrap();
    let effects = engine.hydrate(snapshot).unwrap();
    let text = effects.into_iter().find_map(|e|match e { Effect::Publish { channel,data } if channel == "preview"=>Some(data["text"].clone()),_=>None }).unwrap();
    assert_eq!(text,"Olá 🌎");
    assert_eq!(engine.control_view()["turn_id"],"turn-1");
}

#[test]
fn late_reply_does_not_reopen_a_completed_turn() {
    let mut engine = engine();
    let effects = engine.command(command(OperationKind::Input,json!({"text":"Olá"})),clock(10.0)).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),11.0);
    line(&mut engine,json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}),12.0);
    line(&mut engine,json!({"id":id,"result":{"turn":{"id":"turn-1"}}}),13.0);
    assert_eq!(engine.view()["state"],"idle");
    assert_eq!(engine.control_view()["in_progress"],false);
}

#[test]
fn late_ack_does_not_remove_reused_server_request() {
    let mut engine = engine();
    let approval = json!({"id":1,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1","command":"pwd"}});
    line(&mut engine,approval.clone(),10.0);
    engine.command(command(OperationKind::Select,json!({"option":1})),clock(10.0)).unwrap();
    line(&mut engine,json!({"method":"serverRequest/resolved","params":{"threadId":"thread-1","requestId":1}}),11.0);
    line(&mut engine,approval,12.0);
    engine.apply(EngineInput::WriteAck { operation_id:"op-1".into(),outcome:WriteOutcome::Written },clock(13.0)).unwrap();
    assert_eq!(engine.control_view()["pending"].as_array().unwrap().len(),1);
}

#[test]
fn voice_organizer_uses_same_writer_without_changing_target_thread() {
    let mut engine = engine();
    engine.command(RuntimeCommand { operation_id:"voice-open".into(),kind:OperationKind::VoiceOpen,payload:json!({"call_id":"call-1"}) },clock(10.0)).unwrap();
    let effects = engine.command(RuntimeCommand { operation_id:"voice-thread".into(),kind:OperationKind::VoiceRpc,
        payload:json!({"call_id":"call-1","method":"thread/start","params":{"ephemeral":true,"sandbox":"read-only","approvalPolicy":"never"}}) },clock(11.0)).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    line(&mut engine,json!({"id":id,"result":{"thread":{"id":"organizer-1"}}}),12.0);
    assert_eq!(engine.control_view()["thread_id"],"thread-1");
    let effects = line(&mut engine,json!({"id":7,"method":"item/tool/call","params":{"threadId":"organizer-1","name":"draft"}}),13.0);
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Publish { channel,data } if channel == "voice" && data["call_id"] == "call-1")));
    assert!(!effects.iter().any(|effect|matches!(effect,Effect::Write { .. })));
    assert!(engine.control_view()["pending"].as_array().unwrap().is_empty());
    assert!(engine.command(RuntimeCommand { operation_id:"wrong-id".into(),kind:OperationKind::VoiceRespond,
        payload:json!({"call_id":"call-1","request_id":"7","result":{}}) },clock(14.0)).is_err());
    assert!(engine.command(RuntimeCommand { operation_id:"wrong-thread".into(),kind:OperationKind::VoiceRpc,
        payload:json!({"call_id":"call-1","method":"turn/start","params":{"threadId":"thread-1","input":[]}}) },clock(15.0)).is_err());
}

#[test]
fn voice_initialize_is_virtual_and_never_sends_another_initialize() {
    let mut engine = engine();
    engine.command(RuntimeCommand { operation_id:"open".into(),kind:OperationKind::VoiceOpen,payload:json!({"call_id":"call"}) },clock(10.0)).unwrap();
    let effects = engine.command(RuntimeCommand { operation_id:"init".into(),kind:OperationKind::VoiceRpc,
        payload:json!({"call_id":"call","method":"initialize","params":{}}) },clock(11.0)).unwrap();
    assert!(frames(&effects).is_empty());
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Reply { disposition:Disposition::Accepted,.. })));
}

#[test]
fn slash_skill_waits_for_catalog_before_starting_turn() {
    let mut engine = engine();
    let effects = engine.command(command(OperationKind::Input,json!({"text":"/skill Olá", "skill_name":"skill",
        "input":[{"type":"text","text":"/skill Olá"}]})),clock(10.0)).unwrap();
    let read = frames(&effects)[0].clone();
    assert_eq!(read["method"],"skills/list");
    let effects = line(&mut engine,json!({"id":read["id"],"result":{"data":[]}}),11.0);
    let (request_id,_) = effects.iter().find_map(|effect|match effect {
        Effect::Policy { kind,request_id,payload } if kind == "skill_catalog"=>Some((request_id.clone(),payload.clone())),_=>None }).unwrap();
    assert!(frames(&effects).is_empty());
    let effects = engine.apply(EngineInput::PolicyResult { request_id,payload:json!({"skill":{"native_name":"skill","path":"/fake/skill"}}) },clock(12.0)).unwrap();
    let input = frames(&effects)[0].clone();
    assert_eq!(input["method"],"turn/start");
    assert_eq!(input["params"]["input"][1],json!({"type":"skill","name":"skill","path":"/fake/skill"}));
}

#[test]
fn organizer_request_before_thread_reply_is_preserved_once() {
    let mut engine = engine();
    engine.command(RuntimeCommand { operation_id:"open".into(),kind:OperationKind::VoiceOpen,payload:json!({"call_id":"call"}) },clock(10.0)).unwrap();
    let effects = engine.command(RuntimeCommand { operation_id:"start".into(),kind:OperationKind::VoiceRpc,
        payload:json!({"call_id":"call","method":"thread/start","params":{"ephemeral":true,"sandbox":"read-only","approvalPolicy":"never"}}) },clock(11.0)).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    let request = json!({"id":7,"method":"item/tool/call","params":{"threadId":"organizer","name":"draft"}});
    assert!(frames(&line(&mut engine,request.clone(),11.1)).is_empty());
    let effects = line(&mut engine,json!({"id":id,"result":{"thread":{"id":"organizer"}}}),12.0);
    assert_eq!(effects.iter().filter(|effect|matches!(effect,Effect::Publish { channel,.. } if channel == "voice")).count(),1);
    let duplicate = line(&mut engine,request,12.1);
    assert!(!duplicate.iter().any(|effect|matches!(effect,Effect::Publish { channel,.. } if channel == "voice")));
}

#[test]
fn subagent_request_during_voice_start_reaches_the_card_once_the_voice_is_open() {
    let mut engine = engine();
    engine.command(RuntimeCommand { operation_id:"open".into(),kind:OperationKind::VoiceOpen,payload:json!({"call_id":"call"}) },clock(10.0)).unwrap();
    let effects = engine.command(RuntimeCommand { operation_id:"start".into(),kind:OperationKind::VoiceRpc,
        payload:json!({"call_id":"call","method":"thread/start","params":{"ephemeral":true,"sandbox":"read-only","approvalPolicy":"never"}}) },clock(11.0)).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    line(&mut engine,json!({"id":12,"method":"item/commandExecution/requestApproval","params":{"threadId":"subagent","command":"ls"}}),11.1);
    line(&mut engine,json!({"id":id,"result":{"thread":{"id":"organizer"}}}),12.0);
    assert_eq!(engine.view()["state"],"awaiting_input");
    let effects = engine.command(command(OperationKind::Select,json!({"option":1})),clock(13.0)).unwrap();
    assert_eq!(reply_to(&effects,12)["result"]["decision"],"accept");
}

fn tier_notification(engine:&mut Engine,thread:&str,tier:Value,time:f64) -> Vec<Effect> {
    line(engine,json!({"method":"thread/settings/updated","params":{"threadId":thread,"threadSettings":{"serviceTier":tier}}}),time)
}
fn tier_accepted(effects:&[Effect]) -> bool {
    effects.iter().any(|effect|matches!(effect,Effect::Reply { operation_id,disposition:Disposition::Accepted,payload }
        if operation_id == "op-1" && payload["service_tier"].is_string()))
}
fn tier_update(engine:&mut Engine,tier:&str) -> Value {
    let effects = engine.command(command(OperationKind::SetServiceTier,json!({"service_tier":tier})),clock(10.0)).unwrap();
    let mut request = frames(&effects)[0].clone();
    if tier == "priority" {
        assert_eq!(request["method"],"model/list");
        request = frames(&line(engine,json!({"id":request["id"],"result":{"data":[{"model":"gpt-6","serviceTiers":[{"id":"priority"}]}]}}),10.1))[0].clone();
    }
    assert_eq!(request["method"],"thread/settings/update");
    assert_eq!(request["params"],json!({"threadId":"thread-1","serviceTier":tier}));
    request
}

#[test]
fn expired_service_tier_writes_cannot_reach_the_transport() {
    for tier in ["default","priority"] {
        let mut engine = engine();
        let effects = engine.command(command(OperationKind::SetServiceTier,json!({"service_tier":tier})),clock(10.0)).unwrap();
        let writes:Vec<_> = effects.iter().filter_map(|effect|match effect {
            Effect::Write { operation_id:Some(id),.. }=>Some(id.clone()),_=>None,
        }).collect();
        assert!(!writes.is_empty());
        assert!(writes.iter().all(|id|engine.write_is_current(id)));
        engine.apply(EngineInput::Tick,clock(20.1)).unwrap();
        assert!(writes.iter().all(|id|!engine.write_is_current(id)));
    }
}

#[test]
fn service_tier_already_active_is_accepted_without_waiting() {
    for tier in ["default","priority"] {
        let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true,"model":"gpt-6",
            "initialized":true,"ready":true,"service_tier":tier}),1,clock(10.0));
        assert_eq!(engine.control_view()["service_tier"],tier);
        let effects = engine.command(command(OperationKind::SetServiceTier,json!({"service_tier":tier})),clock(10.0)).unwrap();
        assert!(tier_accepted(&effects));
        assert!(frames(&effects).is_empty());
    }
}

#[test]
fn service_tier_control_is_strict_and_catalog_keeps_tiers() {
    assert_eq!(serde_json::to_value(OperationKind::SetServiceTier).unwrap(),"set_service_tier");
    let mut engine = engine();
    for payload in [json!({"service_tier":"fast"}),json!({"service_tier":null}),json!({"service_tier":"default","model":"other"})] {
        assert!(engine.command(command(OperationKind::SetServiceTier,payload),clock(10.0)).is_err());
    }
    let request = frames(&engine.command(command(OperationKind::ListModels,json!({})),clock(10.0)).unwrap())[0].clone();
    let effects = line(&mut engine,json!({"id":request["id"],"result":{"data":[{"model":"gpt-6","serviceTiers":[{"id":"priority"}],"defaultServiceTier":"default"}]}}),10.1);
    let catalog = effects.iter().find_map(|effect|match effect { Effect::Reply { payload,.. }=>Some(payload),_=>None }).unwrap();
    assert_eq!(catalog[0]["serviceTiers"],json!([{"id":"priority"}]));
    assert_eq!(catalog[0]["defaultServiceTier"],"default");
}

#[test]
fn priority_requires_visible_live_support_and_unchanged_model() {
    for model in [json!({"model":"gpt-6","serviceTiers":[]}),json!({"model":"gpt-6","hidden":true,"serviceTiers":[{"id":"priority"}]}),
        json!({"model":"gpt-6","serviceTiers":[{"id":"priority","hidden":true}]})] {
        let mut engine = engine();
        let request = frames(&engine.command(command(OperationKind::SetServiceTier,json!({"service_tier":"priority"})),clock(10.0)).unwrap())[0].clone();
        let effects = line(&mut engine,json!({"id":request["id"],"result":{"data":[model]}}),10.1);
        assert!(frames(&effects).is_empty());
        assert!(effects.iter().any(|effect|matches!(effect,Effect::Reply { operation_id,disposition:Disposition::Rejected,.. } if operation_id == "op-1")));
    }
    let mut engine = engine();
    let request = frames(&engine.command(command(OperationKind::SetServiceTier,json!({"service_tier":"priority"})),clock(10.0)).unwrap())[0].clone();
    line(&mut engine,json!({"method":"thread/settings/updated","params":{"threadId":"thread-1","threadSettings":{"model":"other"}}}),10.1);
    let effects = line(&mut engine,json!({"id":request["id"],"result":{"data":[{"model":"gpt-6","serviceTiers":[{"id":"priority"}]}]}}),10.2);
    assert!(frames(&effects).is_empty());
    assert!(!tier_accepted(&effects));
}

#[test]
fn service_tier_needs_ack_notification_and_authoritative_snapshot_in_either_order() {
    for tier in ["priority","default"] { for before_ack in [false,true] {
        let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true,"ready":true,
            "model":"gpt-6","effort":"high","mode":"plan"}),1,clock(10.0));
        let update = tier_update(&mut engine,tier);
        assert!(engine.command(RuntimeCommand { operation_id:"second".into(),kind:OperationKind::SetServiceTier,payload:json!({"service_tier":"default"}) },clock(10.2)).is_err());
        assert!(tier_notification(&mut engine,"other",json!(tier),10.3).is_empty());
        let effects = if before_ack {
            assert!(frames(&tier_notification(&mut engine,"thread-1",json!(tier),10.4)).is_empty());
            line(&mut engine,json!({"id":update["id"],"result":{}}),10.5)
        } else {
            let ack = line(&mut engine,json!({"id":update["id"],"result":{}}),10.4);
            assert!(!tier_accepted(&ack)); assert!(frames(&ack).is_empty());
            tier_notification(&mut engine,"thread-1",json!(tier),10.5)
        };
        assert!(!tier_accepted(&effects));
        let read = frames(&effects)[0].clone();
        assert_eq!(read["method"],"thread/resume"); assert_eq!(read["params"],json!({"threadId":"thread-1"}));
        let confirmed = line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"thread-1"},"serviceTier":tier}}),10.6);
        assert!(tier_accepted(&confirmed));
        assert_eq!(engine.view()["codex_service_tier"],tier);
        assert_eq!(engine.control_view()["service_tier"],tier);
        assert_eq!(engine.control_view()["model"],"gpt-6"); assert_eq!(engine.control_view()["effort"],"high"); assert_eq!(engine.control_view()["mode"],"plan");
    } }
}

#[test]
fn old_candidate_waits_for_new_event_and_does_not_poll() {
    let mut engine = engine(); let update = tier_update(&mut engine,"priority");
    line(&mut engine,json!({"id":update["id"],"result":{}}),10.2);
    assert!(frames(&tier_notification(&mut engine,"thread-1",json!("default"),10.3)).is_empty());
    let read = frames(&tier_notification(&mut engine,"thread-1",json!("priority"),10.4))[0].clone();
    let stale = line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"thread-1"},"serviceTier":"default"}}),10.5);
    assert!(!tier_accepted(&stale)); assert!(frames(&stale).is_empty());
    let read = frames(&tier_notification(&mut engine,"thread-1",json!("priority"),10.6))[0].clone();
    tier_notification(&mut engine,"thread-1",json!("priority"),10.7);
    let during = line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"thread-1"},"serviceTier":"default"}}),10.8);
    assert!(!tier_accepted(&during));
    let read = frames(&during)[0].clone();
    let confirmed = line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"thread-1"},"serviceTier":"priority"}}),10.9);
    assert!(tier_accepted(&confirmed));
}

#[test]
fn service_tier_errors_expiry_disconnect_and_recovery_never_accept() {
    for failure in ["rpc","timeout","eof","foreign_snapshot","write"] {
        let mut engine = engine(); let update = tier_update(&mut engine,"default");
        let effects = match failure {
            "rpc"=>line(&mut engine,json!({"id":update["id"],"error":{"message":"refused"}}),11.0),
            "timeout"=>engine.apply(EngineInput::Tick,clock(20.1)).unwrap(),
            "eof"=>line(&mut engine,json!({"type":"cano_saiu"}),11.0),
            "write"=>engine.apply(EngineInput::WriteAck { operation_id:"op-1".into(),outcome:WriteOutcome::Unknown },clock(11.0)).unwrap(),
            _=>{
                line(&mut engine,json!({"id":update["id"],"result":{}}),10.2);
                let read = frames(&tier_notification(&mut engine,"thread-1",json!("default"),10.3))[0].clone();
                line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"other"},"serviceTier":"default"}}),11.0)
            },
        };
        assert!(effects.iter().any(|effect|matches!(effect,Effect::Reply { operation_id,disposition:Disposition::Rejected | Disposition::Unknown,.. } if operation_id == "op-1")));
        assert!(!tier_accepted(&effects));
        assert!(!tier_accepted(&line(&mut engine,json!({"id":update["id"],"result":{}}),21.0)));
    }
    let mut engine = engine();
    let frame = json!({"id":"hangar:1:50","method":"thread/settings/update","params":{"threadId":"thread-1","serviceTier":"priority"}});
    engine.restore_rpc("op-1".into(),&frame,0,0);
    let recovered = line(&mut engine,json!({"id":frame["id"],"result":{}}),11.0);
    assert!(!recovered.iter().any(|effect|matches!(effect,Effect::Reply { disposition:Disposition::Accepted,.. })));
    assert!(frames(&recovered).is_empty());
    assert!(engine.control_view()["service_tier"].is_null());
}

#[test]
fn read_preserves_tier_and_resume_cannot_overwrite_newer_settings() {
    let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true,"ready":true,
        "service_tier":"priority","model":"gpt-6","effort":"high"}),1,clock(10.0));
    let read = frames(&engine.command(command(OperationKind::ReadSettings,json!({})),clock(10.0)).unwrap())[0].clone();
    line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"thread-1"}}}),10.1);
    assert_eq!(engine.control_view()["service_tier"],"priority");
    let resume = json!({"id":"hangar:1:50","method":"thread/resume","params":{"threadId":"thread-1"}});
    engine.restore_rpc("resume".into(),&resume,0,0);
    tier_notification(&mut engine,"thread-1",json!("priority"),10.2);
    line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"thread-1"},"serviceTier":null,"model":"old","reasoningEffort":"low"}}),10.3);
    assert_eq!(engine.control_view()["service_tier"],"priority"); assert_eq!(engine.control_view()["model"],"gpt-6");
    let read = frames(&engine.command(command(OperationKind::SetServiceTier,json!({"service_tier":"default"})),clock(11.0)).unwrap())[0].clone();
    line(&mut engine,json!({"id":read["id"],"result":{}}),11.1);
    let resume = frames(&tier_notification(&mut engine,"thread-1",Value::Null,11.2))[0].clone();
    assert!(tier_accepted(&line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"thread-1"},"serviceTier":null}}),11.3)));
}

#[test]
fn resume_uses_external_tier_and_thread_change_cancels_pending_choice() {
    for (wire_tier,effective) in [(json!("priority"),"priority"),(Value::Null,"default")] {
        let mut engine = engine();
        let resume = json!({"id":"hangar:1:50","method":"thread/resume","params":{"threadId":"thread-1"}});
        engine.restore_rpc("resume".into(),&resume,0,0);
        line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"thread-1","serviceTier":"wrong"},"serviceTier":wire_tier}}),10.1);
        assert_eq!(engine.control_view()["service_tier"],effective);
    }
    let mut engine = engine(); let update = tier_update(&mut engine,"default");
    let resume = json!({"id":"hangar:1:50","method":"thread/resume","params":{"threadId":"thread-1"}});
    engine.restore_rpc("resume".into(),&resume,0,0);
    let effects = line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"other"},"serviceTier":"priority"}}),10.2);
    assert!(effects.iter().any(|effect|matches!(effect,Effect::Reply { operation_id,disposition:Disposition::Unknown,.. } if operation_id == "op-1")));
    assert!(!tier_accepted(&line(&mut engine,json!({"id":update["id"],"result":{}}),10.3)));
}

#[test]
fn bootstrap_new_process_preserves_tier_but_live_resume_does_not_override() {
    for reconnect in [false,true] {
        let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true,"service_tier":"priority"}),1,clock(10.0));
        let init = frames(&engine.bootstrap(reconnect,"boot".into()).unwrap())[0].clone();
        let requests = frames(&line(&mut engine,json!({"id":init["id"],"result":{}}),10.1));
        if reconnect { assert!(requests[1]["params"].get("serviceTier").is_none()); }
        else { assert_eq!(requests[1]["params"]["serviceTier"],"priority"); }
    }
}

fn published(effects:&[Effect],name:&str) -> Vec<String> {
    effects.iter().filter_map(|e|match e { Effect::Publish { channel,data } if channel == name=>data["text"].as_str().map(str::to_owned),_=>None }).collect()
}

#[test]
fn stop_terminates_the_commands_the_interrupted_turn_was_running() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    line(&mut engine,json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1",
        "item":{"type":"commandExecution","id":"exec-1","processId":"43041","command":"sleep 300"}}}),10.5);
    line(&mut engine,json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1",
        "item":{"type":"commandExecution","id":"exec-2","processId":"43042","command":"true"}}}),10.6);
    line(&mut engine,json!({"method":"item/completed","params":{"threadId":"thread-1","turnId":"turn-1",
        "item":{"type":"commandExecution","id":"exec-2","processId":"43042"}}}),10.7);
    let interrupt = frames(&engine.command(command(OperationKind::Interrupt,json!({})),clock(11.0)).unwrap())[0].clone();
    assert_eq!(interrupt["method"],"turn/interrupt");
    let effects = line(&mut engine,json!({"id":interrupt["id"],"result":{}}),11.1);
    let terminate:Vec<_> = frames(&effects).into_iter().filter(|f|f["method"] == "thread/backgroundTerminals/terminate").collect();
    assert_eq!(terminate.len(),1);
    assert_eq!(terminate[0]["params"],json!({"threadId":"thread-1","processId":"43041"}));
    assert!(effects.iter().any(|e|matches!(e,Effect::Reply { operation_id,disposition:Disposition::Accepted,.. } if operation_id == "op-1")));
}

#[test]
fn usage_limit_has_its_own_problem_and_survives_the_failed_turn() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    line(&mut engine,json!({"method":"error","params":{"threadId":"thread-1","turnId":"turn-1","willRetry":true,
        "error":{"message":"Rate limit reached","codexErrorInfo":"rateLimitExceeded"}}}),10.5);
    assert_eq!(engine.view()["problema"],"codex_limite_uso");
    line(&mut engine,json!({"method":"error","params":{"threadId":"thread-1","turnId":"turn-1","willRetry":false,
        "error":{"message":"You've hit your usage limit.","codexErrorInfo":"usageLimitExceeded"}}}),11.0);
    line(&mut engine,json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"failed",
        "error":{"message":"turn failed"}}}}),11.5);
    assert_eq!(engine.view()["problema"],"codex_limite_uso");
    assert_eq!(engine.view()["problema_detalhe"],"You've hit your usage limit.");
}

#[test]
fn reasoning_summary_streams_on_the_thinking_channel_and_input_asks_for_it() {
    let mut engine = engine();
    let start = frames(&engine.command(command(OperationKind::Input,json!({"text":"oi"})),clock(9.0)).unwrap())[0].clone();
    assert_eq!(start["params"]["summary"],"detailed");
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    let first = line(&mut engine,json!({"method":"item/reasoning/summaryTextDelta","params":{"threadId":"thread-1","turnId":"turn-1",
        "itemId":"rs-1","summaryIndex":0,"delta":"**Plano**"}}),10.1);
    assert_eq!(published(&first,"thinking"),vec!["**Plano**"]);
    line(&mut engine,json!({"method":"item/reasoning/summaryPartAdded","params":{"threadId":"thread-1","turnId":"turn-1",
        "itemId":"rs-1","summaryIndex":1}}),11.0);
    let second = line(&mut engine,json!({"method":"item/reasoning/summaryTextDelta","params":{"threadId":"thread-1","turnId":"turn-1",
        "itemId":"rs-1","summaryIndex":1,"delta":"Depois"}}),11.3);
    assert_eq!(published(&second,"thinking"),vec!["**Plano**\n\nDepois"]);
    let answer = line(&mut engine,json!({"method":"item/started","params":{"threadId":"thread-1","turnId":"turn-1",
        "item":{"type":"agentMessage","id":"msg-1","text":""}}}),12.0);
    assert_eq!(published(&answer,"thinking"),vec![""]);
}

#[test]
fn turn_cut_by_a_dead_app_server_is_reported_after_reconnect() {
    let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true,"in_progress":true,"turn_id":"turn-1"}),2,clock(10.0));
    let init = frames(&engine.bootstrap(true,"boot".into()).unwrap())[0].clone();
    let resume = frames(&line(&mut engine,json!({"id":init["id"],"result":{}}),10.1))[1].clone();
    assert_eq!(resume["method"],"thread/resume");
    let effects = line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"thread-1","status":{"type":"idle"}}}}),10.2);
    let read = frames(&effects).into_iter().find(|f|f["method"] == "thread/read").unwrap();
    assert_eq!(read["params"]["includeTurns"],true);
    line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"thread-1","status":{"type":"idle"},
        "turns":[{"id":"turn-1","status":"interrupted"}]}}}),10.3);
    assert_eq!(engine.view()["problema"],"codex_turno_cortado");
}

#[test]
fn finished_turn_after_reconnect_is_not_reported_as_cut() {
    let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true,"in_progress":true}),2,clock(10.0));
    let init = frames(&engine.bootstrap(true,"boot".into()).unwrap())[0].clone();
    let resume = frames(&line(&mut engine,json!({"id":init["id"],"result":{}}),10.1))[1].clone();
    let effects = line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"thread-1","status":{"type":"idle"}}}}),10.2);
    let read = frames(&effects).into_iter().find(|f|f["method"] == "thread/read").unwrap();
    line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"thread-1","status":{"type":"idle"},
        "turns":[{"id":"turn-1","status":"completed"}]}}}),10.3);
    assert!(engine.view()["problema"].is_null());
}

fn diags(effects:&[Effect]) -> Vec<(DiagEvent,String)> {
    effects.iter().filter_map(|e|match e { Effect::Diag { event,code }=>Some((*event,code.clone())),_=>None }).collect()
}

#[test]
fn other_codex_version_warns_once_on_initialize() {
    let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true}),1,clock(10.0));
    let effects = engine.bootstrap(true,"boot".into()).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    let effects = line(&mut engine,json!({"id":id,"result":{"userAgent":"hangar/9.1.0 (x)"}}),11.0);
    assert_eq!(diags(&effects),vec![(DiagEvent::CodexVersion,"codex_9_1".into())]);
    assert_eq!(engine.view()["problema"],"codex_versao_nao_conferida");
    assert!(engine.view()["problema_detalhe"].as_str().unwrap().contains("9.1.0"));
}

#[test]
fn checked_codex_version_is_silent() {
    let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true}),1,clock(10.0));
    let effects = engine.bootstrap(true,"boot".into()).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    let ua = format!("hangar/{} (x)",hangar_codex::version::CHECKED);
    let effects = line(&mut engine,json!({"id":id,"result":{"userAgent":ua}}),11.0);
    assert!(diags(&effects).is_empty());
    assert!(engine.view()["problema"].is_null());
}

#[test]
fn notification_with_wrong_type_is_dropped_and_reported() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    let effects = line(&mut engine,json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","delta":5}}),11.0);
    assert_eq!(diags(&effects),vec![(DiagEvent::CodexDecode,"item_agentmessage_delta".into())]);
    assert!(effects.iter().any(|e|matches!(e,Effect::Policy { kind,.. } if kind == "unknown_private")));
    let effects = line(&mut engine,json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"completed"}}}),12.0);
    assert!(effects.iter().any(|e|matches!(e,Effect::WakeQueue)));
    assert_eq!(engine.view()["state"],"idle");
}

#[test]
fn repeated_malformed_notification_reports_privately_once() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    let mut effects = line(&mut engine,json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","delta":5}}),11.0);
    effects.extend(line(&mut engine,json!({"method":"item/agentMessage/delta","params":{"threadId":"thread-1","turnId":"turn-1","delta":6}}),11.1));
    assert_eq!(effects.iter().filter(|e|matches!(e,Effect::Policy { kind,.. } if kind == "unknown_private")).count(),1);
    assert_eq!(diags(&effects).len(),2);
}

#[test]
fn malformed_reply_reports_only_its_shape() {
    let mut engine = engine();
    let request = frames(&engine.command(command(OperationKind::ReadSettings,json!({})),clock(10.0)).unwrap())[0].clone();
    let effects = line(&mut engine,json!({"id":request["id"],"result":{"thread":"x","b":1}}),11.0);
    let payload = effects.iter().find_map(|e|match e { Effect::Policy { kind,payload,.. } if kind == "unknown_private" => Some(payload.clone()),_=>None }).unwrap();
    assert_eq!(payload["event"],json!({"method":"thread/read","result_keys":["thread","b"]}));
    let mut engine = self::engine();
    let request = frames(&engine.command(command(OperationKind::ReadSettings,json!({})),clock(12.0)).unwrap())[0].clone();
    let effects = line(&mut engine,json!({"id":request["id"],"result":"texto"}),13.0);
    let payload = effects.iter().find_map(|e|match e { Effect::Policy { kind,payload,.. } if kind == "unknown_private" => Some(payload.clone()),_=>None }).unwrap();
    assert_eq!(payload["event"],json!({"method":"thread/read","result_type":"string"}));
}

#[test]
fn unknown_notification_is_silent() {
    let mut engine = engine();
    let effects = line(&mut engine,json!({"method":"thread/novidade","params":{"threadId":"thread-1"}}),10.0);
    assert!(diags(&effects).is_empty());
    assert!(effects.is_empty());
}

#[test]
fn unknown_server_request_still_gets_method_not_found() {
    let mut engine = engine();
    let effects = line(&mut engine,json!({"id":77,"method":"foo/bar","params":{"threadId":"thread-1"}}),10.0);
    let reply = frames(&effects).into_iter().find(|f|f["id"] == 77).unwrap();
    assert_eq!(reply["error"]["code"],-32601);
    assert!(reply["error"]["message"].as_str().unwrap().contains("foo/bar"));
}

#[test]
fn reply_with_wrong_type_is_reported_and_engine_stays_usable() {
    let mut engine = engine();
    let request = frames(&engine.command(command(OperationKind::ReadSettings,json!({})),clock(10.0)).unwrap())[0].clone();
    assert_eq!(request["method"],"thread/read");
    let effects = line(&mut engine,json!({"id":request["id"],"result":{"thread":"x"}}),11.0);
    assert_eq!(diags(&effects),vec![(DiagEvent::CodexDecode,"thread_read".into())]);
    assert!(effects.iter().any(|e|matches!(e,Effect::Policy { kind,payload,.. } if kind == "unknown_private" && payload["kind"] == "decode:thread/read")));
    assert!(effects.iter().any(|e|matches!(e,Effect::Reply { disposition:Disposition::Accepted,.. })));
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),12.0);
    assert_eq!(engine.view()["state"],"working");
}

#[test]
fn undecodable_cut_check_read_is_reported_once() {
    let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true,"in_progress":true}),2,clock(10.0));
    let init = frames(&engine.bootstrap(true,"boot".into()).unwrap())[0].clone();
    let resume = frames(&line(&mut engine,json!({"id":init["id"],"result":{}}),10.1))[1].clone();
    let effects = line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"thread-1","status":{"type":"idle"}}}}),10.2);
    let read = frames(&effects).into_iter().find(|f|f["method"] == "thread/read").unwrap();
    let effects = line(&mut engine,json!({"id":read["id"],"result":{"thread":"x"}}),10.3);
    assert_eq!(effects.iter().filter(|e|matches!(e,Effect::Policy { kind,.. } if kind == "unknown_private")).count(),1);
    assert_eq!(diags(&effects),vec![(DiagEvent::CodexDecode,"thread_read".into())]);
    assert!(engine.view()["problema"].is_null());
}

#[test]
fn undecodable_user_input_request_still_asks_from_the_raw_line() {
    let mut engine = engine();
    let effects = line(&mut engine,json!({"id":5,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1",
        "questions":[{"id":"q1","header":"H","question":"Qual?","isOther":"sim"}]}}),10.0);
    assert_eq!(diags(&effects),vec![(DiagEvent::CodexDecode,"item_tool_requestuserinput".into())]);
    let view = engine.view();
    assert_eq!(view["state"],"awaiting_input");
    assert_eq!(view["codex_question"]["questions"][0]["id"],"q1");
    assert_eq!(view["codex_question"]["questions"][0]["isOther"],false);
    assert_eq!(view["codex_question"]["questions"][0]["options"],json!([]));
    assert!(diags(&line(&mut engine,json!({"method":"thread/novidade","params":{"threadId":"thread-1"}}),10.1)).is_empty());
}

#[test]
fn user_input_request_without_questions_asks_nothing() {
    let mut engine = engine();
    line(&mut engine,json!({"id":6,"method":"item/tool/requestUserInput","params":{"threadId":"thread-1"}}),10.0);
    assert!(engine.view()["codex_question"].is_null());
}

#[test]
fn undecodable_turn_completed_still_closes_the_turn() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    let effects = line(&mut engine,json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":5}}}),11.0);
    assert_eq!(diags(&effects),vec![(DiagEvent::CodexDecode,"turn_completed".into())]);
    assert!(effects.iter().any(|e|matches!(e,Effect::WakeQueue)));
    assert_eq!(engine.view()["state"],"idle");
}

#[test]
fn undecodable_approval_shows_the_raw_command() {
    let mut engine = engine();
    line(&mut engine,json!({"id":3,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1",
        "command":"rm -rf build","cwd":"/repo","reason":5}}),10.0);
    assert_eq!(engine.view()["question"],"Rodar `rm -rf build` em /repo?");
}

#[test]
fn unreadable_codex_version_is_reported_without_problem() {
    let mut engine = Engine::new(json!({"name":"session","thread_id":"thread-1","headless":true}),1,clock(10.0));
    let effects = engine.bootstrap(true,"boot".into()).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    let effects = line(&mut engine,json!({"id":id,"result":{"userAgent":"sem versão"}}),11.0);
    assert_eq!(diags(&effects),vec![(DiagEvent::CodexVersion,"codex_desconhecida".into())]);
    assert!(engine.view()["problema"].is_null());
}

#[test]
fn undecodable_turn_completed_keeps_the_retry_problem() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    line(&mut engine,json!({"method":"error","params":{"threadId":"thread-1","turnId":"turn-1","willRetry":true,
        "error":{"message":"stream disconnected"}}}),10.5);
    assert_eq!(engine.view()["problema"],"codex_sem_conexao");
    let effects = line(&mut engine,json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":5}}}),11.0);
    assert!(effects.iter().any(|e|matches!(e,Effect::WakeQueue)));
    assert_eq!(engine.view()["state"],"idle");
    assert_eq!(engine.view()["problema"],"codex_sem_conexao");
}

#[test]
fn undecodable_turn_completed_of_another_turn_is_ignored() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-2"}}}),10.0);
    line(&mut engine,json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":5}}}),11.0);
    assert_eq!(engine.view()["state"],"working");
}

#[test]
fn unreadable_command_approval_offers_no_session_wide_grant() {
    let mut engine = engine();
    line(&mut engine,json!({"id":4,"method":"item/commandExecution/requestApproval","params":{"threadId":"thread-1",
        "command":["rm","-rf","build"],"cwd":"/repo"}}),10.0);
    let view = engine.view();
    assert_eq!(view["question"],"Rodar um comando que o Hangar não conseguiu ler em /repo?");
    assert_eq!(view["options"],json!(["Permitir","Negar"]));
    assert!(engine.command(command(OperationKind::Select,json!({"option":3})),clock(10.1)).is_err());
    let answer = frames(&engine.command(command(OperationKind::Select,json!({"option":2})),clock(10.2)).unwrap());
    assert_eq!(answer[0]["result"]["decision"],"decline");
}

fn bootstrapped(meta:Value) -> (Engine,Vec<Value>) {
    let mut engine = Engine::new(meta,1,clock(10.0));
    engine.set_fresh_process(true);
    let effects = engine.bootstrap(true,"boot".into()).unwrap();
    let id = frames(&effects)[0]["id"].clone();
    let effects = line(&mut engine,json!({"id":id,"result":{"userAgent":format!("hangar/{} (x)",hangar_codex::version::CHECKED)}}),11.0);
    (engine,frames(&effects))
}

fn find_method(sent:&[Value],method:&str) -> Value { sent.iter().find(|f|f["method"] == method).unwrap().clone() }

#[test]
fn resume_carries_cwd_policy_sandbox_and_tier_like_python() {
    let (_,sent) = bootstrapped(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p",
        "permission_mode":"Ask for approval","service_tier":"priority"}));
    assert_eq!(find_method(&sent,"thread/resume")["params"],
        json!({"threadId":"t1","cwd":"/p","approvalPolicy":"on-request","sandbox":"read-only","serviceTier":"priority"}));
}

#[test]
fn permission_mode_matches_case_insensitively_like_python() {
    let (_,sent) = bootstrapped(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p","permission_mode":" ask for approval "}));
    let params = find_method(&sent,"thread/resume")["params"].clone();
    assert_eq!((params["sandbox"].clone(),params["approvalPolicy"].clone()),(json!("read-only"),json!("on-request")));
}

#[test]
fn missing_model_provider_retries_resume_with_openai() {
    let (mut engine,sent) = bootstrapped(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p"}));
    let resume = find_method(&sent,"thread/resume");
    let effects = line(&mut engine,json!({"id":resume["id"],"error":{"code":-32600,"message":"Model provider `x` not found"}}),12.0);
    let retry = find_method(&frames(&effects),"thread/resume");
    assert_eq!(retry["params"]["modelProvider"],"openai");
    // Segunda recusa igual não repete: vira erro da subida.
    let effects = line(&mut engine,json!({"id":retry["id"],"error":{"code":-32600,"message":"Model provider `x` not found"}}),13.0);
    assert!(frames(&effects).is_empty());
    assert!(effects.iter().any(|e|matches!(e,Effect::Reply { operation_id,disposition:Disposition::Rejected,.. } if operation_id == "boot")));
    assert_eq!(engine.view()["problema"],"codex_conversa_nao_abriu");
}

#[test]
fn provider_retry_and_start_fallback_skip_transfers() {
    let (mut engine,sent) = bootstrapped(json!({"name":"s","thread_id":"t1","headless":true,"transfer_id":"tr"}));
    let resume = find_method(&sent,"thread/resume");
    let effects = line(&mut engine,json!({"id":resume["id"],"error":{"code":-32600,"message":"no rollout found for thread id t1"}}),12.0);
    assert!(frames(&effects).is_empty());
}

#[test]
fn no_rollout_falls_back_to_thread_start() {
    let (mut engine,sent) = bootstrapped(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p","model":"gpt-6"}));
    let resume = find_method(&sent,"thread/resume");
    let effects = line(&mut engine,json!({"id":resume["id"],"error":{"code":-32600,"message":"no rollout found for thread id t1"}}),12.0);
    assert!(frames(&effects).iter().any(|f|f["method"] == "thread/start" && f["params"]["model"] == "gpt-6"));
}

#[test]
fn refused_effort_is_a_problem_not_a_failure() {
    let (mut engine,sent) = bootstrapped(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p","effort":"max"}));
    let resume = find_method(&sent,"thread/resume");
    let effects = line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"t1","status":{"type":"idle"},"turns":[]},"model":"gpt-6"}}),12.0);
    let update = find_method(&frames(&effects),"thread/settings/update");
    let effects = line(&mut engine,json!({"id":update["id"],"error":{"code":-32600,"message":"effort max not supported"}}),13.0);
    assert_eq!(engine.view()["problema"],"codex_esforco_nao_aplicado");
    assert_eq!(engine.control_view()["ready"],true);
    assert!(effects.iter().any(|e|matches!(e,Effect::WakeQueue)));
}

#[test]
fn select_without_pending_approval_says_no_pending_permission() {
    let mut engine = engine();
    let Err(error) = engine.command(command(OperationKind::Select,json!({"option":1})),clock(10.0)) else { panic!("devia recusar") };
    assert_eq!(error.code,"no_pending_permission");
}

fn live_attach(meta:Value) -> (Engine,Value) {
    let mut engine = Engine::new(meta,1,clock(10.0));
    let id = frames(&engine.bootstrap(true,"boot".into()).unwrap())[0]["id"].clone();
    let sent = frames(&line(&mut engine,json!({"id":id,"result":{}}),11.0));
    let resume = find_method(&sent,"thread/resume");
    (engine,resume)
}

#[test]
fn live_attach_to_thread_without_rollout_is_ready_on_the_same_thread() {
    let (mut engine,resume) = live_attach(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p","service_tier":"priority"}));
    assert_eq!(resume["params"],json!({"threadId":"t1"}));
    let effects = line(&mut engine,json!({"id":resume["id"],"error":{"code":-32600,"message":"no rollout found for thread id t1"}}),12.0);
    assert!(frames(&effects).iter().all(|f|f["method"] != "thread/start"));
    assert!(effects.iter().any(|e|matches!(e,Effect::Reply { operation_id,disposition:Disposition::Accepted,.. } if operation_id == "boot")));
    assert!(effects.iter().any(|e|matches!(e,Effect::WakeQueue)));
    assert_eq!(engine.control_view()["ready"],true);
    assert!(engine.view()["problema"].is_null());
    let effects = engine.command(command(OperationKind::Input,json!({"text":"Olá"})),clock(13.0)).unwrap();
    assert_eq!(find_method(&frames(&effects),"turn/start")["params"]["threadId"],"t1");
}

#[test]
fn live_attach_refusal_is_a_visible_problem_and_effort_refusal_stays_a_failure() {
    let (mut engine,resume) = live_attach(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p"}));
    let effects = line(&mut engine,json!({"id":resume["id"],"error":{"code":-32600,"message":"thread t1 not loaded"}}),12.0);
    assert!(frames(&effects).is_empty());
    assert!(effects.iter().any(|e|matches!(e,Effect::Reply { operation_id,disposition:Disposition::Rejected,.. } if operation_id == "boot")));
    assert_eq!(engine.view()["problema"],"codex_conversa_nao_abriu");
    assert!(engine.view()["problema_detalhe"].as_str().unwrap().contains("not loaded"));
    assert!(effects.iter().any(|e|matches!(e,Effect::Diag { event:DiagEvent::CodexBootstrap,code } if code == "thread/resume")));
}

#[test]
fn initialize_refusal_is_a_visible_problem() {
    let mut engine = Engine::new(json!({"name":"s","thread_id":"t1","headless":true}),1,clock(10.0));
    let id = frames(&engine.bootstrap(true,"boot".into()).unwrap())[0]["id"].clone();
    let effects = line(&mut engine,json!({"id":id,"error":{"code":-32600,"message":"bad client"}}),11.0);
    assert!(effects.iter().any(|e|matches!(e,Effect::Reply { operation_id,disposition:Disposition::Rejected,.. } if operation_id == "boot")));
    assert_eq!(engine.view()["problema"],"codex_conversa_nao_abriu");
}

/// A vida anterior aplicou o esforço; religar no cano vivo não o reenvia (como o Python), nem com nem sem rollout.
#[test]
fn live_attach_does_not_resend_effort() {
    let (mut engine,resume) = live_attach(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p","effort":"max","async_during_load":[]}));
    let effects = line(&mut engine,json!({"id":resume["id"],"error":{"code":-32600,"message":"no rollout found for thread id t1"}}),12.0);
    assert!(frames(&effects).iter().all(|f|f["method"] != "thread/settings/update"));
    assert_eq!(engine.control_view()["ready"],true);
    assert!(engine.control_view()["async_during_load"].is_null());

    let (mut engine,resume) = live_attach(json!({"name":"s","thread_id":"t1","headless":true,"cwd":"/p","effort":"max"}));
    let effects = line(&mut engine,json!({"id":resume["id"],"result":{"thread":{"id":"t1","status":{"type":"idle"},"turns":[]},"model":"gpt-6"}}),12.0);
    assert!(frames(&effects).iter().all(|f|f["method"] != "thread/settings/update"));
    assert_eq!(engine.control_view()["ready"],true);
}

/// O Python recusa Fast sozinho enquanto o arquivo ainda tem a thread anterior: vai no patch da thread.
#[test]
fn new_thread_saves_fast_with_the_thread_in_one_patch() {
    let mut engine = Engine::new(json!({"name":"s","headless":true,"cwd":"/p"}),1,clock(10.0));
    engine.set_fresh_process(true);
    let id = frames(&engine.bootstrap(true,"boot".into()).unwrap())[0]["id"].clone();
    let start = find_method(&frames(&line(&mut engine,json!({"id":id,"result":{}}),11.0)),"thread/start");
    let effects = line(&mut engine,json!({"id":start["id"],"result":{"thread":{"id":"t2","path":"/r.jsonl","status":{"type":"idle"},"turns":[]},
        "model":"gpt-6","serviceTier":null}}),12.0);
    let patches:Vec<Value> = effects.iter().filter_map(|e|match e { Effect::Policy { kind,payload,.. } if kind == "session.patch_meta" => Some(payload.clone()),_=>None }).collect();
    assert_eq!(patches,vec![json!({"thread_id":"t2","rollout_path":"/r.jsonl","service_tier":"default"})]);
}

fn request(engine:&mut Engine,id:i64,method:&str,params:Value) -> Vec<Effect> {
    line(engine,json!({"id":id,"method":method,"params":params}),10.0)
}
fn reply_to(effects:&[Effect],id:i64) -> Value { frames(effects).into_iter().find(|f|f["id"] == id && f.get("method").is_none()).unwrap() }

#[test]
fn permissions_request_is_a_card_and_answers_with_scope() {
    let mut engine = engine();
    request(&mut engine,5,"item/permissions/requestApproval",json!({"threadId":"thread-1","cwd":"/p","reason":"ler config",
        "permissions":{"fileSystem":{"read":["/etc/x"],"write":["/p/out"]},"network":{"enabled":true}}}));
    let view = engine.view();
    assert_eq!(view["state"],"awaiting_input");
    assert_eq!(view["options"],json!(["Permitir neste turno","Permitir na sessão","Negar"]));
    let text = view["question"].as_str().unwrap();
    assert!(text.contains("/etc/x") && text.contains("/p/out") && text.contains("rede"));
    let effects = engine.command(command(OperationKind::Select,json!({"option":2})),clock(11.0)).unwrap();
    let answer = reply_to(&effects,5);
    assert_eq!(answer["result"]["scope"],"session");
    assert_eq!(answer["result"]["permissions"]["fileSystem"]["read"],json!(["/etc/x"]));
}

#[test]
fn denied_permissions_grant_nothing() {
    let mut engine = engine();
    request(&mut engine,5,"item/permissions/requestApproval",json!({"threadId":"thread-1","permissions":{"network":{"enabled":true}}}));
    let effects = engine.command(command(OperationKind::Select,json!({"option":3})),clock(11.0)).unwrap();
    assert_eq!(reply_to(&effects,5)["result"],json!({"permissions":{},"scope":"turn"}));
}

#[test]
fn elicitation_form_becomes_a_native_question() {
    let mut engine = engine();
    request(&mut engine,6,"mcpServer/elicitation/request",json!({"threadId":"thread-1","serverName":"db","mode":"form","message":"Qual ambiente?",
        "requestedSchema":{"type":"object","properties":{"env":{"type":"string","enum":["dev","prod"],"title":"Ambiente"},"note":{"type":"string","title":"Nota"}},"required":["env"]}}));
    let view = engine.view();
    let question = &view["codex_question"];
    assert_eq!(question["request_id"],6);
    assert_eq!(question["questions"][0]["options"],json!([{"label":"dev","description":""},{"label":"prod","description":""}]));
    assert_eq!(question["questions"][1]["options"],json!([]));
    let effects = engine.command(command(OperationKind::AnswerQuestions,json!({"request_id":6,"answers":[
        {"question_id":"env","kind":"option","indices":[1]},{"question_id":"note","kind":"text","value":"urgente"}]})),clock(11.0)).unwrap();
    assert_eq!(reply_to(&effects,6)["result"],json!({"action":"accept","content":{"env":"prod","note":"urgente"}}));
}

#[test]
fn elicitation_converts_number_and_boolean_answers() {
    let mut engine = engine();
    request(&mut engine,6,"mcpServer/elicitation/request",json!({"threadId":"thread-1","serverName":"db","mode":"form","message":"x",
        "requestedSchema":{"type":"object","properties":{"n":{"type":"integer"},"ok":{"type":"boolean"}}}}));
    assert_eq!(engine.view()["codex_question"]["questions"][1]["options"][0]["label"],"sim");
    let effects = engine.command(command(OperationKind::AnswerQuestions,json!({"request_id":6,"answers":[
        {"question_id":"n","kind":"text","value":"42"},{"question_id":"ok","kind":"option","indices":[1]}]})),clock(11.0)).unwrap();
    assert_eq!(reply_to(&effects,6)["result"]["content"],json!({"n":42,"ok":false}));
}

#[test]
fn skipping_an_elicitation_form_cancels_it() {
    let mut engine = engine();
    request(&mut engine,6,"mcpServer/elicitation/request",json!({"threadId":"thread-1","serverName":"db","mode":"form","message":"x",
        "requestedSchema":{"type":"object","properties":{"n":{"type":"string"}}}}));
    let effects = engine.command(command(OperationKind::SkipQuestion,json!({"request_id":6})),clock(11.0)).unwrap();
    assert_eq!(reply_to(&effects,6)["result"],json!({"action":"cancel"}));
}

#[test]
fn elicitation_schema_that_does_not_fit_is_declined_with_a_note() {
    let mut engine = engine();
    let effects = request(&mut engine,7,"mcpServer/elicitation/request",json!({"threadId":"thread-1","serverName":"db","mode":"form","message":"x",
        "requestedSchema":{"type":"object","properties":{"deep":{"type":"object"}}}}));
    assert_eq!(reply_to(&effects,7)["result"]["action"],"decline");
    assert!(effects.iter().any(|e|matches!(e,Effect::Policy { kind,.. } if kind == "local_output")));
}

#[test]
fn elicitation_url_is_a_link_card() {
    let mut engine = engine();
    request(&mut engine,8,"mcpServer/elicitation/request",json!({"threadId":"thread-1","serverName":"gh","mode":"url","message":"Autorize","url":"https://example.com/auth","elicitationId":"e1"}));
    let view = engine.view();
    assert!(view["question"].as_str().unwrap().contains("https://example.com/auth"));
    assert_eq!(view["options"],json!(["Concluí","Cancelar"]));
    let effects = engine.command(command(OperationKind::Select,json!({"option":2})),clock(11.0)).unwrap();
    assert_eq!(reply_to(&effects,8)["result"],json!({"action":"cancel"}));
}

#[test]
fn current_time_is_answered() {
    let mut engine = engine();
    let effects = request(&mut engine,9,"currentTime/read",json!({"threadId":"thread-1"}));
    // O schema 0.161 chama o campo `currentTimeAt` (segundos Unix inteiros).
    assert_eq!(reply_to(&effects,9)["result"]["currentTimeAt"],1_800_000_010);
}

#[test]
fn request_from_a_subagent_thread_is_not_dropped() {
    let mut engine = engine();
    request(&mut engine,10,"item/commandExecution/requestApproval",json!({"threadId":"subagent-thread","command":"ls"}));
    assert_eq!(engine.view()["state"],"awaiting_input");
    let effects = engine.command(command(OperationKind::Select,json!({"option":1})),clock(11.0)).unwrap();
    assert_eq!(reply_to(&effects,10)["result"]["decision"],"accept");
}

#[test]
fn blank_command_approval_is_unreadable() {
    let mut engine = engine();
    request(&mut engine,11,"item/commandExecution/requestApproval",json!({"threadId":"thread-1","command":"   ","cwd":"/repo"}));
    let view = engine.view();
    assert_eq!(view["question"],"Rodar um comando que o Hangar não conseguiu ler em /repo?");
    assert_eq!(view["options"],json!(["Permitir","Negar"]));
}

#[test]
fn undecodable_failed_turn_completed_marks_the_turn_error() {
    let mut engine = engine();
    line(&mut engine,json!({"method":"turn/started","params":{"threadId":"thread-1","turn":{"id":"turn-1"}}}),10.0);
    line(&mut engine,json!({"method":"turn/completed","params":{"threadId":"thread-1","turn":{"id":"turn-1","status":"failed","error":"boom"}}}),11.0);
    let view = engine.view();
    assert_eq!(view["state"],"idle");
    assert_eq!(view["problema"],"headless_turno_erro");
    assert!(view["problema_detalhe"].is_null());
}

#[test]
fn mode_change_is_written_to_the_session_file() {
    let mut engine = engine();
    let read = frames(&engine.command(command(OperationKind::SetMode,json!({"mode":"plan"})),clock(10.0)).unwrap())[0].clone();
    let update = frames(&line(&mut engine,json!({"id":read["id"],"result":{"thread":{"id":"thread-1","status":{"type":"idle"},"turns":[]}}}),10.1))[0].clone();
    let effects = line(&mut engine,json!({"id":update["id"],"result":{}}),10.2);
    let patch = effects.iter().find_map(|e|match e { Effect::Policy { kind,payload,.. } if kind == "session.patch_meta" => Some(payload.clone()),_=>None }).unwrap();
    assert_eq!(patch["mode"],"plan");
    let reply = effects.iter().find_map(|e|match e { Effect::Reply { operation_id,payload,.. } if operation_id == "op-1" => Some(payload.clone()),_=>None }).unwrap();
    assert_eq!(reply["mode"],"plan");
    // Trocar só o modelo não mexe no modo gravado.
    let model = frames(&engine.command(RuntimeCommand { operation_id:"op-2".into(),kind:OperationKind::SetModel,payload:json!({"model":"gpt-6"}) },clock(11.0)).unwrap())[0].clone();
    let effects = line(&mut engine,json!({"id":model["id"],"result":{}}),11.1);
    let patch = effects.iter().find_map(|e|match e { Effect::Policy { kind,payload,.. } if kind == "session.patch_meta" => Some(payload.clone()),_=>None }).unwrap();
    assert!(patch.get("mode").is_none());
}

#[test]
fn stop_whose_turn_read_fails_still_answers() {
    // Sem turno conhecido o Stop lê a conversa antes; a leitura recusada responde a operação de cima.
    let mut engine = engine();
    let read = frames(&engine.command(command(OperationKind::Interrupt,json!({})),clock(11.0)).unwrap())[0].clone();
    assert_eq!(read["method"],"thread/read");
    let effects = line(&mut engine,json!({"id":read["id"],"error":{"code":-32603,"message":"boom"}}),11.1);
    assert!(effects.iter().any(|e|matches!(e,Effect::Reply { operation_id,disposition:Disposition::Rejected,.. } if operation_id == "op-1")));
}
