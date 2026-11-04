use hangar_api::state::StateEvent;
use serde_json::Value;

#[path = "runtime_contract_cases.rs"]
mod cases;

fn scenarios() -> Vec<Value> {
    let rows: Vec<Value> = serde_json::from_str(cases::SCENARIOS).unwrap();
    assert_eq!(rows.len(), 24);
    rows
}

/// As saídas esperadas, com os campos de `STATE` devolvidos a cada estado.
fn expected(table: &str, total: usize) -> Vec<Value> {
    let expand = |compact: &Value| {
        let mut full: Value = serde_json::from_str(cases::STATE).unwrap();
        for (key, value) in compact.as_object().unwrap() { full[key] = value.clone(); }
        full
    };
    let mut rows: Vec<Value> = serde_json::from_str(table).unwrap();
    assert_eq!(rows.len(), total);
    for row in &mut rows {
        row["initial_state"] = expand(&row["initial_state"]);
        for output in row["outputs"].as_array_mut().unwrap() {
            if output["channel"] == "state" { output["data"] = expand(&output["data"]); }
        }
    }
    rows
}

#[test]
fn state_events_keep_every_public_field() {
    for scenario in expected(cases::CLAUDE, 13).into_iter().chain(expected(cases::CODEX, 11)) {
        for output in scenario["outputs"].as_array().unwrap() {
            if output["channel"] == "state" {
                let data = output["data"].clone();
                let event: StateEvent = serde_json::from_value(data.clone()).unwrap();
                assert_eq!(serde_json::to_value(event).unwrap(), data, "{}", scenario["name"]);
            }
        }
    }
}

/// O `interaction_id` do cartão é aleatório: a comparação é do resto. `codex_subagents` fica fora da tabela.
fn without_ids(mut state:Value) -> Value {
    if let Some(state) = state.as_object_mut() { state.remove("interaction_id"); state.remove("codex_subagents"); }
    if let Some(form) = state["codex_question"].as_object_mut() { form.remove("interaction_id"); }
    state
}

#[test]
fn claude_public_events() {
    use hangar_server::runtime::{claude::ClaudeEngine,protocol::*};
    use serde_json::json;
    let scenarios = scenarios();
    let golden = expected(cases::CLAUDE, 13);
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
        // Como em produção: com o store o cartão ganha o ID, que `without_ids` tira da comparação.
        engine.set_interactions("key-oracle".into(),std::sync::Arc::default()).unwrap();
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
            assert_eq!(without_ids(engine.view()["public_state"].clone()),states[index],"{}: etapa {index}",scenario["name"]);
        }
        assert_eq!(publications,expected_publications,"{}",scenario["name"]);
        compared += 1;
    }
    assert!(compared >= 6,"comparação precisa incluir estados, texto, pensamento, ferramenta e falhas");
}

#[test]
fn codex_public_events() {
    use hangar_server::runtime::{codex::Engine,protocol::*};
    use serde_json::json;
    let scenarios = scenarios();
    let golden = expected(cases::CODEX, 11);
    let mut compared = 0;
    for scenario in scenarios.iter().filter(|s|s["provider"] == "codex" && s["conservative_changes"].as_array().unwrap().is_empty()
        && s["steps"].as_array().unwrap().iter().all(|step|step["kind"] == "line")) {
        let oracle = golden.iter().find(|g|g["name"] == scenario["name"]).unwrap();
        let states:Vec<_> = oracle["outputs"].as_array().unwrap().iter().filter(|o|o["channel"] == "state").map(|o|o["data"].clone()).collect();
        let mut clock:ClockSample = serde_json::from_value(scenario["clock"].clone()).unwrap();
        let mut metadata = scenario["metadata"].clone(); metadata["initialized"] = json!(true); metadata["ready"] = json!(true);
        metadata["status_line"] = oracle["initial_state"]["status_line"].clone();
        let mut engine = Engine::new(metadata,1,clock);
        engine.set_interactions("key-oracle".into(),std::sync::Arc::default()).unwrap();
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
                assert_eq!(without_ids(engine.view()),states[state_index],"{}: estado {state_index}",scenario["name"]);
                state_index += 1;
            }
        }
        assert_eq!(without_ids(engine.view()),*states.last().unwrap(),"{}: estado final",scenario["name"]);
        assert_eq!(notices,expected_notices,"{}: recusa e aviso local",scenario["name"]);
        compared += 1;
    }
    assert!(compared >= 2,"comparação precisa incluir perguntas e fim de turno");
}
