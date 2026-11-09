use hangar_transcript::interactions::*;
use serde_json::{Value, json};
use std::io::Write;
use proptest::prelude::*;

fn wire(lines: &[Value]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    for line in lines { writeln!(file, "{line}").unwrap(); }
    file
}
fn request(id: &str, tool: &str, display: Value) -> Value {
    json!({"type":"interaction.request","agentId":"main","id":id,"kind":"approval",
        "toolCallId":"tool_x","request":{"turnId":8,"toolCallId":"tool_x","toolName":tool,"action":"",
        "display":display},"time":1786454000000u64})
}
fn loop_event(event: Value) -> Value { json!({"type":"context.append_loop_event","event":event}) }

#[test]
fn h012_pi_pending_question_and_omp_first_usable_question() {
    let args = json!({"question":"Seguir?","options":[{"label":"Sim"},{"label":"Não"}]});
    let call = json!({"type":"message","id":"n1","message":{"role":"assistant","content":[
        {"type":"toolCall","name":"question","id":"call_1","arguments":args}]}});
    let file = wire(&[call.clone()]);
    let pending = read_pending_pi_call(file.path(), "question").unwrap().unwrap();
    assert_eq!(pending.id, "call_1");
    assert_eq!(pending.args, args.as_object().unwrap().clone());
    let result = json!({"type":"message","message":{"role":"toolResult","toolName":"question","toolCallId":"call_1","content":[]}});
    assert!(read_pending_pi_call(wire(&[call.clone(),result.clone()]).path(), "question").unwrap().is_none());
    let mut new_call = call.clone();
    new_call["message"]["content"][0]["id"] = json!("call_2");
    let file = wire(&[call,result,new_call]);
    assert_eq!(read_pending_pi_call(file.path(), "question").unwrap().unwrap().id,"call_2");
    let omp = wire(&[json!({"type":"message","message":{"role":"assistant","content":[{"type":"toolCall",
        "name":"ask","id":"ask_1","arguments":{"questions":[{}, {"question":"Qual cor você prefere?","options":[{"label":"Azul"},42,{"label":"Verde"},{}]}]}}]}})]);
    let pending = read_pending_pi_call(omp.path(), "ask").unwrap().unwrap();
    assert_eq!(pending.args, json!({"question":"Qual cor você prefere?","header":"","options":[{"label":"Azul"},{"label":"Verde"}]}).as_object().unwrap().clone());
    assert!(read_pending_pi_call(omp.path(), "question").unwrap().is_none());
}

#[test]
fn h020_kimi_pending_call_closes_by_result_without_tool_name() {
    let call = loop_event(json!({"type":"tool.call","name":"AskUserQuestion","toolCallId":"q1","args":{"questions":[{"question":"Seguir?"}]}}));
    let file = wire(&[call.clone()]);
    assert_eq!(read_pending_kimi_call(file.path()).unwrap().unwrap().id, "q1");
    let result = loop_event(json!({"type":"tool.result","toolCallId":"q1","result":{"output":"Sim"}}));
    assert!(read_pending_kimi_call(wire(&[call,result]).path()).unwrap().is_none());
    let file = wire(&[loop_event(json!({"type":"tool.call","name":"Read","toolCallId":"q2","args":{"questions":[]}}))]);
    assert!(read_pending_kimi_call(file.path()).unwrap().is_none());
}

#[test]
fn h021_delivery_needs_positive_evidence_and_unreadable_stays_unknown() {
    let file = wire(&[loop_event(json!({"type":"tool.result","toolCallId":"q1"}))]);
    assert_eq!(kimi_response_arrived(file.path(), "q1").unwrap(), true);
    assert_eq!(kimi_response_arrived(file.path(), "q2").unwrap(), false);
    let missing = file.path().with_extension("missing");
    assert!(kimi_response_arrived(&missing, "q1").is_err());
    assert!(kimi_interaction_resolved(&missing, "a1").is_err());
}

#[test]
fn h022_kimi_approval_resolves_only_same_id_and_does_not_duplicate_question() {
    let req = request("a1", "ExitPlanMode", json!({"kind":"plan_review"}));
    let other = json!({"type":"interaction.resolved","id":"a0"});
    let file = wire(&[other, req.clone()]);
    assert_eq!(read_pending_kimi_interaction(file.path()).unwrap().unwrap()["id"], "a1");
    assert!(!kimi_interaction_resolved(file.path(), "a1").unwrap());
    let file = wire(&[req.clone(),json!({"type":"interaction.resolved","id":"a1"})]);
    assert!(read_pending_kimi_interaction(file.path()).unwrap().is_none());
    assert!(kimi_interaction_resolved(file.path(), "a1").unwrap());
    let mut question = req; question["kind"] = json!("question");
    assert!(read_pending_kimi_interaction(wire(&[question]).path()).unwrap().is_none());
}

#[test]
fn h023_kimi_plan_choices_preserve_order_and_feedback() {
    for (options, expected) in [(json!(null),json!(["Approve","Reject","Revise"])),
        (json!([{"label":"Única"}]),json!(["Approve","Reject","Revise"])),
        (json!([{"label":"Caminho A"},{"label":"Caminho B"}]),json!(["Caminho A","Caminho B","Reject","Revise"]))] {
        let file = wire(&[request("a1", "ExitPlanMode", json!({"kind":"plan_review","options":options}))]);
        let pending = read_pending_kimi_interaction(file.path()).unwrap().unwrap();
        assert_eq!(pending["titulo"], "Ready to build with this plan?");
        assert_eq!(pending["resumo"], "");
        let choices = pending["escolhas"].as_array().unwrap();
        assert_eq!(choices.iter().map(|v| v["label"].clone()).collect::<Vec<_>>(), expected.as_array().unwrap().clone());
        assert_eq!(choices.last().unwrap()["requires_feedback"], true);
        assert!(choices[..choices.len()-1].iter().all(|c| c["requires_feedback"] == false));
    }
}

#[test]
fn h023_kimi_command_and_goal_choices_match_tui() {
    let file = wire(&[request("a1", "Bash", json!({"kind":"command","command":"ls"}))]);
    let pending = read_pending_kimi_interaction(file.path()).unwrap().unwrap();
    assert_eq!(pending["titulo"], "Run this command?");
    assert_eq!(pending["resumo"], "`ls`");
    assert_eq!(pending["escolhas"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect::<Vec<_>>(),
        ["Approve once","Approve for this session","Reject","Reject with feedback"]);
    for (mode, labels) in [("yolo",vec!["Switch to Auto and start","Keep YOLO and start","Do not start"]),
        ("manual",vec!["Switch to Auto and start","Switch to YOLO and start","Start in Manual","Do not start"])] {
        let file = wire(&[request("a1", "Goal", json!({"kind":"goal_start","mode":mode}))]);
        let pending = read_pending_kimi_interaction(file.path()).unwrap().unwrap();
        assert_eq!(pending["escolhas"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect::<Vec<_>>(), labels);
    }
    let mut no_display = request("a1", "Bash", json!({"kind":"command"}));
    no_display["request"].as_object_mut().unwrap().remove("display");
    assert!(read_pending_kimi_interaction(wire(&[no_display]).path()).unwrap().is_none());
    for (tool, display, action, title, summary) in [
        ("Write",json!({"kind":"file_io","path":"/tmp/a"}),"","Write this file?","`/tmp/a`"),
        ("Edit",json!({"kind":"diff","path":"/tmp/a"}),"","Apply these edits?","`/tmp/a`"),
        ("TaskStop",json!({"kind":"generic"}),"Parar tarefa","Stop this task?","Parar tarefa"),
        ("Fetch",json!({"kind":"url_fetch","url":"https://example.com"}),"","Approve Fetch?","`https://example.com`"),
        ("Search",json!({"kind":"search","query":"termo"}),"","Approve Search?","`termo`"),
    ] {
        let mut req = request("a1", tool, display);
        req["request"]["action"] = json!(action);
        let file = wire(&[req]);
        let pending = read_pending_kimi_interaction(file.path()).unwrap().unwrap();
        assert_eq!(pending["titulo"],title);
        assert_eq!(pending["resumo"],summary);
    }
}

#[test]
fn h024_kimi_corrupt_lines_do_not_hide_following_request() {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    writeln!(file,"{{\"type\":\"interaction.req").unwrap();
    writeln!(file,"{}",request("a1", "Bash",json!({"kind":"command","command":"ls"}))).unwrap();
    assert_eq!(read_pending_kimi_interaction(file.path()).unwrap().unwrap()["id"],"a1");
}

proptest! {
    #[test]
    fn external_wire_and_question_arguments_never_panic(text in ".{0,256}", bytes in prop::collection::vec(any::<u8>(), 0..1024)) {
        let mut file = tempfile::NamedTempFile::new().unwrap();
        file.write_all(&bytes).unwrap();
        writeln!(file,"{}",request("a1", "Bash", json!({"kind":text,"command":text}))).unwrap();
        let _ = read_pending_pi_call(file.path(), "question");
        let _ = read_pending_pi_call(file.path(), "ask");
        let _ = read_pending_kimi_call(file.path());
        let _ = read_pending_kimi_interaction(file.path());
        let _ = kimi_response_arrived(file.path(), &text);
        let _ = kimi_interaction_resolved(file.path(), &text);
    }
}
