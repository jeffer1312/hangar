use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use hangar_workflows::{get_agent, get_workflow, list_workflows};

fn transcript(root: &Path) -> PathBuf {
    let path = root.join("proj/sess.jsonl");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "").unwrap();
    path
}

fn write(root: &Path, relative: &str, value: Value) {
    let path = root.join("proj/sess").join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, serde_json::to_vec(&value).unwrap()).unwrap();
}

fn lines(root: &Path, relative: &str, values: &[Value]) {
    let path = root.join("proj/sess").join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(
        path,
        values
            .iter()
            .map(|value| format!("{value}\n"))
            .collect::<String>(),
    )
    .unwrap();
}

#[test]
fn completed_summaries_preserve_defaults_identity_and_order() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    assert_eq!(
        list_workflows(&transcript)
            .unwrap()
            .as_array()
            .unwrap()
            .len(),
        0
    );
    write(
        root.path(),
        "workflows/wf_abc.json",
        json!({
            "runId": "wf_abc", "workflowName": "review", "status": "completed",
            "agentCount": 3, "phases": [{"title": "P1"}, {"title": "P2"}],
            "totalTokens": 1200, "durationMs": 5000, "startTime": 100,
        }),
    );
    write(
        root.path(),
        "workflows/wf_defaults.json",
        json!({"startTime": "99"}),
    );
    write(
        root.path(),
        "workflows/ignored.json",
        json!({"runId": "ignored"}),
    );
    fs::write(root.path().join("proj/sess/workflows/wf_broken.json"), "{").unwrap();
    let rows = list_workflows(&transcript).unwrap();
    let rows = rows.as_array().unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["runId"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["wf_abc", "wf_defaults"]
    );
    assert_eq!(rows[0]["name"], "review");
    assert_eq!(rows[0]["status"], "completed");
    assert_eq!(rows[0]["agentCount"], 3);
    assert_eq!(rows[0]["phaseCount"], 2);
    assert_eq!(rows[0]["totalTokens"], 1200);
    assert_eq!(rows[0]["durationMs"], 5000);
    assert_eq!(rows[0]["running"], false);
    assert_eq!(rows[1]["name"], "wf_defaults");
    assert_eq!(rows[1]["agentCount"], 0);
    assert_eq!(rows[1]["phaseCount"], 0);
    assert_eq!(rows[1]["totalTokens"], 0);
    assert_eq!(rows[1]["durationMs"], 0);
    assert_eq!(rows[1]["startTime"], "99");
}

#[test]
fn live_workflows_reconstruct_agents_and_keep_completion_authoritative() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    write(
        root.path(),
        "workflows/wf_done.json",
        json!({"runId": "wf_done", "startTime": 999}),
    );
    lines(
        root.path(),
        "subagents/workflows/wf_done/journal.jsonl",
        &[json!({"type": "started", "agentId": "ignored"})],
    );
    lines(
        root.path(),
        "subagents/workflows/wf_live/journal.jsonl",
        &[
            json!(42),
            json!([]),
            json!("texto"),
            json!({"type": "started"}),
            json!({"type": "started", "agentId": "a1deadbeef"}),
            json!({"type": "started", "agentId": "a2cafef00d"}),
            json!({"type": "started", "agentId": "a1deadbeef"}),
            json!({"type": "result", "agentId": "a2cafef00d"}),
            json!({"type": "result", "agentId": "no-start"}),
        ],
    );
    lines(
        root.path(),
        "subagents/workflows/wf_live/agent-a1deadbeef.jsonl",
        &[
            json!(42),
            json!([]),
            json!({"message": {"role": "user", "content": [{"type": "tool_use", "name": "ignored"}]}}),
            json!({"message": {"model": "claude-opus", "role": "assistant",
            "usage": {"input_tokens": 10, "output_tokens": 5, "cache_creation_input_tokens": 2, "cache_read_input_tokens": 3},
            "content": [0, {"type": "tool_use", "name": "Read"}, {"type": "text", "text": " pensei "}]}}),
        ],
    );
    fs::create_dir_all(root.path().join("proj/sess/workflows/scripts")).unwrap();
    fs::write(
        root.path()
            .join("proj/sess/workflows/scripts/myflow-wf_live.js"),
        "export const meta = { name: 'myflow' }",
    )
    .unwrap();
    let rows = list_workflows(&transcript).unwrap();
    assert_eq!(rows[0]["runId"], "wf_live");
    assert_eq!(rows[0]["name"], "myflow");
    assert_eq!(rows[0]["running"], true);
    assert_eq!(rows[0]["agentCount"], 2);
    assert_eq!(rows[0]["totalTokens"], 20);
    assert_eq!(rows[1]["runId"], "wf_done");
    assert_eq!(rows.as_array().unwrap().len(), 2);
    let detail = get_workflow(&transcript, "wf_live").unwrap().unwrap();
    assert_eq!(detail["status"], "running");
    assert_eq!(detail["totalTokens"], 20);
    assert!(detail["summary"].is_null());
    assert_eq!(detail["durationMs"], 0);
    assert!(detail["phases"].as_array().unwrap().is_empty());
    let agents = detail["agents"].as_array().unwrap();
    assert_eq!(agents[0]["agentId"], "a1deadbeef");
    assert_eq!(agents[0]["label"], "a1deadbe");
    assert_eq!(agents[0]["state"], "progress");
    assert_eq!(agents[0]["model"], "claude-opus");
    assert_eq!(agents[0]["toolCalls"], 1);
    assert_eq!(agents[0]["lastToolName"], "Read");
    assert_eq!(agents[0]["resultPreview"], "pensei");
    assert_eq!(agents[1]["state"], "done");
    assert_eq!(agents[1]["tokens"], 0);
    assert!(agents[1]["model"].is_null());
    assert!(get_workflow(&transcript, "wf_missing").unwrap().is_none());
}

#[test]
fn workflow_detail_prefers_completion_and_does_not_fallback_after_bad_json() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    write(
        root.path(),
        "workflows/wf_abc.json",
        json!({
            "runId": "wf_abc", "workflowName": "rev", "status": "completed",
            "totalTokens": 50, "durationMs": 10, "summary": "ok",
            "phases": [{"title": "P", "detail": "d", "ignored": true}, {}],
            "workflowProgress": [
                {"type": "workflow_phase", "title": "P"},
                {"type": "workflow_agent", "agentId": "a1", "label": "review", "state": "done", "tokens": 50, "toolCalls": 2, "lastToolName": "Read"},
                {"type": "workflow_agent", "agentId": "a2"},
            ],
        }),
    );
    lines(
        root.path(),
        "subagents/workflows/wf_abc/journal.jsonl",
        &[json!({"type": "started", "agentId": "ignored"})],
    );
    let detail = get_workflow(&transcript, "wf_abc").unwrap().unwrap();
    assert_eq!(detail["name"], "rev");
    assert_eq!(detail["status"], "completed");
    assert_eq!(detail["totalTokens"], 50);
    assert_eq!(detail["durationMs"], 10);
    assert_eq!(detail["summary"], "ok");
    assert_eq!(detail["phases"][0]["title"], "P");
    assert_eq!(detail["phases"][0]["detail"], "d");
    assert!(detail["phases"][0].get("ignored").is_none());
    assert!(detail["phases"][1]["title"].is_null());
    assert_eq!(detail["agents"][0]["agentId"], "a1");
    assert_eq!(detail["agents"][0]["toolCalls"], 2);
    assert_eq!(detail["agents"][0]["lastToolName"], "Read");
    assert_eq!(detail["agents"][1]["tokens"], 0);
    assert!(detail["agents"][1]["model"].is_null());
    assert_eq!(detail["agents"].as_array().unwrap().len(), 2);
    fs::write(root.path().join("proj/sess/workflows/wf_abc.json"), "{").unwrap();
    assert!(get_workflow(&transcript, "wf_abc").unwrap().is_none());
    write(root.path(), "workflows/wf_abc.json", json!([]));
    assert!(get_workflow(&transcript, "wf_abc").is_err());
}

#[test]
fn agent_detail_preserves_prompt_results_repeated_tools_and_live_state() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    assert!(get_agent(&transcript, "wf_x", "nope").unwrap().is_none());
    write(
        root.path(),
        "workflows/wf_abc.json",
        json!({"workflowProgress": [
            {"type": "workflow_phase", "agentId": "a1"},
            {"type": "workflow_agent", "agentId": "a1", "label": "rev", "state": "done", "tokens": 99, "model": "opus", "phaseTitle": "P"},
            {"type": "workflow_agent", "agentId": "a1", "label": "ignored"},
        ]}),
    );
    lines(
        root.path(),
        "subagents/workflows/wf_abc/journal.jsonl",
        &[
            json!(42),
            json!([]),
            json!("texto"),
            json!({"type": "started", "agentId": "a1"}),
            json!({"type": "result", "agentId": "other", "result": "ignored"}),
            json!({"type": "result", "agentId": "a1", "result": "old"}),
            json!({"type": "result", "agentId": "a1", "result": {"verdict": "ok", "descrição": "revisão"}}),
        ],
    );
    lines(
        root.path(),
        "subagents/workflows/wf_abc/agent-a1.jsonl",
        &[
            json!(42),
            json!([]),
            json!({"type": "user", "message": {"content": "faz a coisa"}}),
            json!({"type": "user", "message": {"content": "ignored"}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "Read", "input": {"file_path": "/tmp/a.py"}},
                {"type": "tool_use", "name": "Read", "input": {"file_path": "/tmp/b.py"}},
                {"type": "tool_use", "name": "Grep", "input": {"pattern": " hello "}},
            ]}}),
        ],
    );
    let agent = get_agent(&transcript, "wf_abc", "a1").unwrap().unwrap();
    assert_eq!(agent["label"], "rev");
    assert_eq!(agent["model"], "opus");
    assert_eq!(agent["tokens"], 99);
    assert_eq!(agent["phaseTitle"], "P");
    assert_eq!(agent["prompt"], "faz a coisa");
    assert_eq!(agent["state"], "done");
    assert_eq!(
        agent["result"],
        "{\n  \"verdict\": \"ok\",\n  \"descrição\": \"revisão\"\n}"
    );
    assert_eq!(agent["toolCalls"], 3);
    assert_eq!(agent["tools"][0]["name"], "Read");
    assert_eq!(agent["tools"][0]["count"], 2);
    assert_eq!(agent["tools"][1]["name"], "Grep");
    assert_eq!(agent["tools"][1]["count"], 1);
    assert_eq!(agent["lastToolName"], "Grep");
    assert_eq!(agent["lastToolTarget"], "hello");
    fs::remove_file(root.path().join("proj/sess/workflows/wf_abc.json")).unwrap();
    lines(
        root.path(),
        "subagents/workflows/wf_abc/journal.jsonl",
        &[json!({"type": "started", "agentId": "a1"})],
    );
    let live = get_agent(&transcript, "wf_abc", "a1").unwrap().unwrap();
    assert_eq!(live["state"], "progress");
    assert_eq!(live["label"], "a1");
    assert_eq!(live["tokens"], 0);
    assert!(live["model"].is_null());
    assert_eq!(live["toolCalls"], 3);
    assert!(live["result"].is_null());
}

#[test]
fn agent_metadata_previews_and_missing_sources_keep_their_priority() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    write(
        root.path(),
        "workflows/wf_meta.json",
        json!({"workflowProgress": [
            {"type": "workflow_agent", "agentId": "éàçãõ12345", "state": "error", "toolCalls": 7,
                "promptPreview": "prévia do pedido", "resultPreview": "prévia do resultado"},
        ]}),
    );
    lines(
        root.path(),
        "subagents/workflows/wf_meta/journal.jsonl",
        &[
            json!({"type": "result", "agentId": "éàçãõ12345", "result": ""}),
            json!({"type": "result", "agentId": "only-journal", "result": "sozinho"}),
        ],
    );
    lines(
        root.path(),
        "subagents/workflows/wf_meta/agent-éàçãõ12345.jsonl",
        &[
            json!({"type": "user", "message": {"content": ""}}),
            json!({"type": "user", "message": {"content": "não é o primeiro"}}),
            json!({"type": "assistant", "message": {"content": [
                {"type": "tool_use", "name": "Read", "input": {"file_path": " ", "custom": " alvo "}},
                {"type": "tool_use", "name": "NoInput"},
            ]}}),
        ],
    );
    let agent = get_agent(&transcript, "wf_meta", "éàçãõ12345")
        .unwrap()
        .unwrap();
    assert_eq!(agent["label"], "éàçãõ123");
    assert_eq!(agent["state"], "error");
    assert_eq!(agent["toolCalls"], 7);
    assert_eq!(agent["prompt"], "prévia do pedido");
    assert_eq!(agent["result"], "prévia do resultado");
    assert_eq!(agent["lastToolName"], "NoInput");
    assert_eq!(agent["lastToolTarget"], "alvo");
    assert!(
        get_agent(&transcript, "wf_meta", "only-journal")
            .unwrap()
            .is_none()
    );
    lines(
        root.path(),
        "subagents/workflows/wf_meta/agent-empty.jsonl",
        &[],
    );
    let empty = get_agent(&transcript, "wf_meta", "empty").unwrap().unwrap();
    assert!(empty["state"].is_null());
    assert_eq!(empty["toolCalls"], 0);
    fs::remove_file(
        root.path()
            .join("proj/sess/subagents/workflows/wf_meta/agent-éàçãõ12345.jsonl"),
    )
    .unwrap();
    assert!(
        get_agent(&transcript, "wf_meta", "éàçãõ12345")
            .unwrap()
            .is_some()
    );
    write(
        root.path(),
        "workflows/wf_lixo.json",
        json!({"workflowProgress": [
            {"type": "workflow_agent", "agentId": "a1", "label": "rev"},
        ]}),
    );
    lines(
        root.path(),
        "subagents/workflows/wf_lixo/journal.jsonl",
        &[
            json!(42),
            json!([]),
            json!("texto"),
            json!({"type": "started", "agentId": "a1"}),
        ],
    );
    let meta_only = get_agent(&transcript, "wf_lixo", "a1").unwrap().unwrap();
    assert_eq!(meta_only["state"], "progress");
    assert_eq!(meta_only["label"], "rev");
}

#[test]
fn unicode_previews_targets_and_script_filename_fallback_are_preserved() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    lines(
        root.path(),
        "subagents/workflows/wf_live/journal.jsonl",
        &[json!({"type": "started", "agentId": "éàçãõ12345"})],
    );
    let target = "á".repeat(130);
    let preview = "ç".repeat(210);
    lines(
        root.path(),
        "subagents/workflows/wf_live/agent-éàçãõ12345.jsonl",
        &[
            json!({"type": "user", "message": {"content": [{"type": "text", "text": "primeiro"}, {"type": "image"}, {"type": "text", "text": "segundo"}]}}),
            json!({"type": "assistant", "message": {"role": "assistant", "content": [
                {"type": "tool_use", "input": {"query": target}}, {"type": "text", "text": preview},
            ]}}),
        ],
    );
    let script = root
        .path()
        .join("proj/sess/workflows/scripts/nome-wf_live.js");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::write(script, "export const meta = {};").unwrap();
    let rows = list_workflows(&transcript).unwrap();
    assert_eq!(rows[0]["name"], "nome");
    let detail = get_workflow(&transcript, "wf_live").unwrap().unwrap();
    assert_eq!(detail["agents"][0]["label"], "éàçãõ123");
    assert_eq!(detail["agents"][0]["resultPreview"], "ç".repeat(200));
    let agent = get_agent(&transcript, "wf_live", "éàçãõ12345")
        .unwrap()
        .unwrap();
    assert_eq!(agent["prompt"], "primeiro segundo");
    assert_eq!(agent["lastToolName"], "?");
    assert_eq!(agent["lastToolTarget"], "á".repeat(120));
}

#[test]
fn every_call_rereads_artifacts_and_keeps_large_integer_usage() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    lines(
        root.path(),
        "subagents/workflows/wf_live/journal.jsonl",
        &[json!({"type": "started", "agentId": "a1"})],
    );
    let huge: Value = serde_json::from_str("184467440737095516160").unwrap();
    lines(
        root.path(),
        "subagents/workflows/wf_live/agent-a1.jsonl",
        &[
            json!({"message": {"usage": {"input_tokens": huge, "output_tokens": 5, "cache_read_input_tokens": 1.5}}}),
        ],
    );
    assert_eq!(
        list_workflows(&transcript).unwrap()[0]["totalTokens"].to_string(),
        "184467440737095516165"
    );
    fs::remove_dir_all(root.path().join("proj/sess")).unwrap();
    assert!(
        list_workflows(&transcript)
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert!(get_workflow(&transcript, "wf_live").unwrap().is_none());
    assert!(get_agent(&transcript, "wf_live", "a1").unwrap().is_none());
    write(
        root.path(),
        "workflows/wf_live.json",
        json!({"workflowName": "nova vida"}),
    );
    assert_eq!(list_workflows(&transcript).unwrap()[0]["name"], "nova vida");
}

#[test]
fn artifact_filename_case_follows_the_platform_glob() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    write(
        root.path(),
        "workflows/WF_UPPER.JSON",
        json!({"workflowName": "caixa alta"}),
    );
    let rows = list_workflows(&transcript).unwrap();
    if cfg!(windows) {
        assert_eq!(rows[0]["name"], "caixa alta");
    } else {
        assert!(rows.as_array().unwrap().is_empty());
    }
}

#[test]
fn path_like_ids_never_leave_the_session_folder() {
    let root = tempfile::tempdir().unwrap();
    let transcript = transcript(root.path());
    write(root.path(), "secret.json", json!({"runId": "secret", "workflowProgress": []}));
    fs::create_dir_all(root.path().join("proj/sess/workflows")).unwrap();
    for id in ["../secret", "..", "a/b", "a\\b", ""] {
        assert!(get_workflow(&transcript, id).unwrap().is_none(), "{id}");
        assert!(get_agent(&transcript, id, "a1").unwrap().is_none(), "{id}");
        assert!(get_agent(&transcript, "wf_abc", id).unwrap().is_none(), "{id}");
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {
        cases: 32,
        rng_seed: proptest::test_runner::RngSeed::Fixed(42),
        ..proptest::test_runner::Config::default()
    })]
    #[test]
    fn external_artifact_bytes_never_panic(bytes in proptest::collection::vec(proptest::prelude::any::<u8>(), 0..1024)) {
        let root = tempfile::tempdir().unwrap();
        let transcript = transcript(root.path());
        let completion = root.path().join("proj/sess/workflows/wf_fuzz.json");
        let run = root.path().join("proj/sess/subagents/workflows/wf_fuzz");
        fs::create_dir_all(completion.parent().unwrap()).unwrap();
        fs::create_dir_all(&run).unwrap();
        fs::write(completion, &bytes).unwrap();
        fs::write(run.join("journal.jsonl"), &bytes).unwrap();
        fs::write(run.join("agent-a1.jsonl"), &bytes).unwrap();
        if let Ok(rows) = list_workflows(&transcript) { proptest::prop_assert!(rows.is_array()); }
        if let Ok(Some(detail)) = get_workflow(&transcript, "wf_fuzz") { proptest::prop_assert!(detail["agents"].is_array()); }
        if let Ok(Some(agent)) = get_agent(&transcript, "wf_fuzz", "a1") { proptest::prop_assert!(agent["tools"].is_array()); }
    }
}
