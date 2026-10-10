use hangar_api::state::StateEvent;
use serde_json::Value;

#[test]
fn golden_preserves_public_fields() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../backend/tests/fixtures/headless_runtime");
    for provider in ["claude", "codex"] {
        let path = root.join(format!("{provider}-golden.json"));
        let raw = std::fs::read(&path).expect("gerar o oráculo Python antes da comparação");
        let scenarios: Vec<Value> = serde_json::from_slice(&raw).unwrap();
        assert!(!scenarios.is_empty(), "o oráculo não pode ser vazio");
        for scenario in scenarios {
            for output in scenario["outputs"].as_array().unwrap() {
                if output["channel"] == "state" {
                    let data = output["data"].clone();
                    let event: StateEvent = serde_json::from_value(data.clone()).unwrap();
                    assert_eq!(serde_json::to_value(event).unwrap(), data, "{}", scenario["name"]);
                }
            }
        }
    }
}

#[test]
fn claude_public_events_match_the_python_oracle() {
    use hangar_server::runtime::{claude::ClaudeEngine,protocol::*};
    use serde_json::json;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/headless_runtime");
    let scenarios:Vec<Value> = serde_json::from_slice(&std::fs::read(root.join("scenarios.json")).unwrap()).unwrap();
    let golden:Vec<Value> = serde_json::from_slice(&std::fs::read(root.join("claude-golden.json")).expect("gerar o oráculo Python autorizado")).unwrap();
    let mut compared = 0;
    for scenario in scenarios.iter().filter(|s|s["provider"] == "claude"
        && s["conservative_changes"].as_array().unwrap().is_empty()
        && s["steps"].as_array().unwrap().iter().all(|step|step["kind"] != "command")) {
        let oracle = golden.iter().find(|g|g["name"] == scenario["name"]).unwrap();
        let states:Vec<_> = oracle["outputs"].as_array().unwrap().iter().filter(|o|o["channel"] == "state").map(|o|o["data"].clone()).collect();
        let expected_publications:Vec<_> = oracle["outputs"].as_array().unwrap().iter()
            .filter(|o|["preview","thinking","tool"].contains(&o["channel"].as_str().unwrap_or(""))).cloned().collect();
        let mut clock:ClockSample = serde_json::from_value(scenario["clock"].clone()).unwrap();
        let mut metadata = scenario["metadata"].clone();
        metadata["status_line"] = oracle["initial_state"]["status_line"].clone();
        let mut engine = ClaudeEngine::new(metadata,1,clock);
        let mut publications = Vec::new();
        for (index,step) in scenario["steps"].as_array().unwrap().iter().enumerate() {
            if step.get("clock").is_some() { clock = serde_json::from_value(step["clock"].clone()).unwrap(); }
            let input = if step["kind"] == "tick" { EngineInput::Tick } else { EngineInput::Line(step["payload"].clone()) };
            let mut effects:std::collections::VecDeque<_> = engine.apply(input,clock).unwrap().into();
            while let Some(effect) = effects.pop_front() {
                match effect {
                    Effect::Publish { channel,data } => publications.push(json!({"channel":channel,"data":data})),
                    Effect::Write { operation_id:Some(operation_id),.. } => {
                        effects.extend(engine.apply(EngineInput::WriteAck { operation_id,outcome:WriteOutcome::Written },clock).unwrap());
                    }
                    Effect::Policy { kind,request_id,.. } if kind == "format_status" => {
                        // A formatação administrativa é a fronteira falsa; o reducer permanece real.
                        effects.extend(engine.apply(EngineInput::PolicyResult { request_id,payload:json!({
                            "status_line":states[index]["status_line"],"limit_reset":states[index]["limit_reset"]}) },clock).unwrap());
                    }
                    _ => {},
                }
            }
            assert_eq!(engine.view()["public_state"],states[index],"{}: etapa {index}",scenario["name"]);
        }
        assert_eq!(publications,expected_publications,"{}",scenario["name"]);
        compared += 1;
    }
    assert!(compared >= 6,"comparação precisa incluir estados, texto, pensamento, ferramenta e falhas");
}

#[test]
fn codex_public_events_match_the_python_oracle() {
    use hangar_server::runtime::{codex::Engine,protocol::*};
    use serde_json::json;
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/headless_runtime");
    let scenarios:Vec<Value> = serde_json::from_slice(&std::fs::read(root.join("scenarios.json")).unwrap()).unwrap();
    let golden:Vec<Value> = serde_json::from_slice(&std::fs::read(root.join("codex-golden.json")).expect("gerar o oráculo Python autorizado")).unwrap();
    let mut compared = 0;
    for scenario in scenarios.iter().filter(|s|s["provider"] == "codex" && s["conservative_changes"].as_array().unwrap().is_empty()
        && s["steps"].as_array().unwrap().iter().all(|step|step["kind"] == "line")) {
        let oracle = golden.iter().find(|g|g["name"] == scenario["name"]).unwrap();
        let states:Vec<_> = oracle["outputs"].as_array().unwrap().iter().filter(|o|o["channel"] == "state").map(|o|o["data"].clone()).collect();
        let mut clock:ClockSample = serde_json::from_value(scenario["clock"].clone()).unwrap();
        let mut metadata = scenario["metadata"].clone(); metadata["initialized"] = json!(true); metadata["ready"] = json!(true);
        metadata["status_line"] = oracle["initial_state"]["status_line"].clone();
        let mut engine = Engine::new(metadata,1,clock);
        let mut state_index = 0;
        let expected_notices:Vec<_> = oracle["outputs"].as_array().unwrap().iter()
            .filter(|output|output["channel"] == "local" || output["channel"] == "response").cloned().collect();
        let mut notices = Vec::new();
        for step in scenario["steps"].as_array().unwrap() {
            if step.get("clock").is_some() { clock = serde_json::from_value(step["clock"].clone()).unwrap(); }
            let effects = engine.apply(EngineInput::Line(step["payload"].clone()),clock).unwrap();
            let changed = effects.iter().any(|effect|matches!(effect,Effect::StateChanged));
            let mut effects:std::collections::VecDeque<_> = effects.into();
            while let Some(effect) = effects.pop_front() {
                match effect {
                    Effect::Write { operation_id:Some(operation_id),frame } => {
                        if frame.get("method").is_none() && frame.get("id").is_some() {
                            notices.push(json!({"channel":"response", "data":{"id":frame["id"], "result":frame["result"], "erro":frame["error"]}}));
                        }
                        effects.extend(engine.apply(EngineInput::WriteAck { operation_id,outcome:WriteOutcome::Written },clock).unwrap());
                    }
                    Effect::Policy { kind,payload,.. } if kind == "local_output" => notices.push(json!({"channel":"local", "data":{"text":payload["text"]}})),
                    Effect::Policy { kind,request_id,.. } if kind == "format_status" => {
                        effects.extend(engine.apply(EngineInput::PolicyResult { request_id,payload:json!({"status_line":states[state_index]["status_line"]}) },clock).unwrap());
                    }
                    _ => {},
                }
            }
            if changed {
                assert_eq!(engine.view(),states[state_index],"{}: estado {state_index}",scenario["name"]);
                state_index += 1;
            }
        }
        assert_eq!(engine.view(),*states.last().unwrap(),"{}: estado final",scenario["name"]);
        assert_eq!(notices,expected_notices,"{}: recusa e aviso local",scenario["name"]);
        compared += 1;
    }
    assert!(compared >= 2,"oráculo precisa comparar perguntas e fim de turno");
}
