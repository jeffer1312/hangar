use hangar_server::{stats::acquire, transcript::Provider};
use serde_json::json;

#[test]
fn pi_stats_preserve_usage_tool_time_and_first_response() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    let rows = [
        json!({"type":"message","message":{"role":"user","timestamp":1000_000,"content":[{"type":"text","text":"oi"}]}}),
        json!({"type":"message","message":{"role":"assistant","timestamp":1003_000,
                "content":[{"type":"toolCall","id":"t1"}],"usage":{"input":20,"output":40,"cacheRead":80,"cacheWrite":0}}}),
        json!({"type":"message","message":{"role":"toolResult","timestamp":1005_000,"toolCallId":"t1","content":[]}}),
    ];
    std::fs::write(
        &path,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let lease = acquire(Provider::Pi, &path);
    let snapshot = lease.acc.lock().unwrap().collect().unwrap();
    assert!(snapshot.is_some(), "Pi publica suas estatísticas");
    let snapshot = snapshot.unwrap();
    assert_eq!(snapshot["turns"], 1);
    assert_eq!(snapshot["steps"], 1);
    assert_eq!(snapshot["in_tok"], 100);
    assert_eq!(snapshot["cache_pct"], 80);
    assert_eq!(snapshot["ttft_ms"], 3000);
    assert_eq!(snapshot["llm_ms"], 3000);
    assert_eq!(snapshot["tool_ms"], 2000);
}

fn agent_snapshot(
    provider: Provider,
    rows: &[serde_json::Value],
) -> serde_json::Map<String, serde_json::Value> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.jsonl");
    std::fs::write(
        &path,
        rows.iter()
            .map(|row| format!("{row}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let lease = acquire(provider, &path);
    let result = lease.acc.lock().unwrap().collect();
    assert!(
        result.is_ok(),
        "JSONL do agente é lido sem erro: {result:?}"
    );
    let snapshot = result.unwrap();
    assert!(snapshot.is_some(), "o agente publica suas estatísticas");
    let snapshot = snapshot.unwrap();
    if matches!(provider, Provider::Pi | Provider::Omp | Provider::Kimi) {
        assert!(lease.acc.lock().unwrap().last_call_ts().is_none());
        assert!(!snapshot.contains_key("tok_s_last"));
        assert!(!snapshot.contains_key("tok_s_recent"));
    }
    snapshot
}

#[test]
fn pi_stats_duration_excludes_generation_from_tool_time() {
    for provider in [Provider::Pi, Provider::Omp] {
        let rows = [
            json!({"type":"message","message":{"role":"user","timestamp":1000_000,"content":[{"type":"text","text":"oi"}]}}),
            json!({"type":"message","message":{"role":"assistant","timestamp":1001_000,
                "_piClaudeStyleThinkingDurationMs":7000,
                "content":[{"type":"toolCall","id":"t1"}],"usage":{"input":20,"output":700,"cacheRead":80,"cacheWrite":0}}}),
            json!({"type":"message","message":{"role":"toolResult","timestamp":1012_000,"toolCallId":"t1","content":[]}}),
        ];
        let snapshot = agent_snapshot(provider, &rows);
        assert_eq!(snapshot["llm_ms"], 7000);
        assert_eq!(snapshot["tok_s"], 100.0);
        assert_eq!(snapshot["tool_ms"], 4000);
        assert_eq!(snapshot["ttft_ms"], 1000);
    }
}

#[test]
fn kimi_stats_preserve_delta_usage_request_time_tools_and_first_response() {
    let rows = [
        json!({"type":"turn.prompt","time":1000_000}),
        json!({"type":"llm.request","time":1001_000}),
        json!({"type":"context.append_loop_event","time":1002_000,"event":{"type":"content.part"}}),
        json!({"type":"context.append_loop_event","time":1003_000,"event":{"type":"tool.call","id":"c1"}}),
        json!({"type":"context.append_loop_event","time":1007_000,"event":{"type":"tool.result","id":"c1"}}),
        json!({"type":"usage.record","time":1009_000,"usage":{"inputOther":100,"output":50,"inputCacheRead":300,"inputCacheCreation":0}}),
    ];
    let snapshot = agent_snapshot(Provider::Kimi, &rows);
    assert_eq!(snapshot["turns"], 1);
    assert_eq!(snapshot["steps"], 1);
    assert_eq!(snapshot["in_tok"], 400);
    assert_eq!(snapshot["out_tok"], 50);
    assert_eq!(snapshot["llm_ms"], 8000);
    assert_eq!(snapshot["tok_s"], 6.2);
    assert_eq!(snapshot["tool_ms"], 4000);
    assert_eq!(snapshot["cache_pct"], 75);
    assert_eq!(snapshot["cache_read_tok"], 300);
    assert_eq!(snapshot["ttft_ms"], 2000);
}

fn pi_message(role: &str, timestamp: i64, extra: serde_json::Value) -> serde_json::Value {
    let mut message = json!({"role":role,"timestamp":timestamp});
    message
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    json!({"type":"message","message":message})
}

fn kimi_loop(kind: &str, timestamp: i64, id: &str) -> serde_json::Value {
    json!({"type":"context.append_loop_event","time":timestamp,"event":{"type":kind,"id":id}})
}

#[test]
fn agent_stats_legacy_clock_and_tool_capacity_match_the_recorded_contract() {
    let mut observed = Vec::new();
    for provider in [Provider::Pi, Provider::Omp] {
        let rows = [
            pi_message("user", 10_000, json!({})),
            pi_message(
                "assistant",
                9_000,
                json!({"usage":{"input":20,"cacheRead":80,"cacheWrite":10,"output":700}}),
            ),
            pi_message(
                "assistant",
                10_000,
                json!({"usage":{"input":20,"cacheRead":80,"cacheWrite":10,"output":700}}),
            ),
        ];
        let snapshot = agent_snapshot(provider, &rows);
        observed.push((
            provider.as_str(),
            "clock",
            snapshot.get("llm_ms").cloned(),
            snapshot.get("tool_ms").cloned(),
        ));
    }
    for provider in [Provider::Pi, Provider::Omp, Provider::Kimi] {
        for (scenario, id) in [("new", "extra"), ("update", "t0")] {
            let rows = if provider == Provider::Kimi {
                let mut rows = vec![
                    json!({"type":"turn.prompt","time":999_000}),
                    json!({"type":"llm.request","time":999_000}),
                ];
                rows.extend((0..200).map(|i| kimi_loop("tool.call", 1000_000, &format!("t{i}"))));
                rows.extend([
                    kimi_loop("tool.call", 1001_000, id),
                    kimi_loop("tool.result", 1004_000, "t0"),
                    kimi_loop("tool.result", 1010_000, "extra"),
                    json!({"type":"usage.record","time":1011_000,"usage":{"inputOther":20,"inputCacheRead":80,"inputCacheCreation":10,"output":70}}),
                ]);
                rows
            } else {
                let calls: Vec<_> = (0..200)
                    .map(|i| json!({"type":"toolCall","id":format!("t{i}")}))
                    .collect();
                vec![
                    pi_message("user", 999_000, json!({})),
                    pi_message("assistant", 1000_000, json!({"usage":{},"content":calls})),
                    pi_message(
                        "assistant",
                        1001_000,
                        json!({"usage":{},"content":[{"type":"toolCall","id":id}]}),
                    ),
                    pi_message("toolResult", 1004_000, json!({"toolCallId":"t0"})),
                    pi_message("toolResult", 1010_000, json!({"toolCallId":"extra"})),
                ]
            };
            let snapshot = agent_snapshot(provider, &rows);
            observed.push((
                provider.as_str(),
                scenario,
                snapshot.get("llm_ms").cloned(),
                snapshot.get("tool_ms").cloned(),
            ));
        }
    }
    assert_eq!(
        observed,
        vec![
            ("pi", "clock", Some(json!(1000)), None),
            ("omp", "clock", Some(json!(1000)), None),
            ("pi", "new", Some(json!(2000)), Some(json!(4000))),
            ("pi", "update", Some(json!(2000)), Some(json!(4000))),
            ("omp", "new", Some(json!(2000)), Some(json!(4000))),
            ("omp", "update", Some(json!(2000)), Some(json!(4000))),
            ("kimi", "new", Some(json!(12000)), Some(json!(4000))),
            ("kimi", "update", Some(json!(12000)), Some(json!(4000))),
        ]
    );
}

#[test]
fn pi_stats_duration_boundaries_and_cache_creation_follow_the_reference() {
    let variants = [
        (
            None,
            Some(json!(1000)),
            Some(json!(4000)),
            Some(json!(700.0)),
        ),
        (
            Some(json!(true)),
            Some(json!(1000)),
            Some(json!(4000)),
            Some(json!(700.0)),
        ),
        (
            Some(json!(-1)),
            Some(json!(1000)),
            Some(json!(4000)),
            Some(json!(700.0)),
        ),
        (
            Some(json!("7000")),
            Some(json!(1000)),
            Some(json!(4000)),
            Some(json!(700.0)),
        ),
        (
            Some(json!(3600001)),
            Some(json!(1000)),
            Some(json!(4000)),
            Some(json!(700.0)),
        ),
        (Some(json!(0)), None, Some(json!(4000)), None),
        (
            Some(json!(3600000)),
            Some(json!(3600000)),
            None,
            Some(json!(0.2)),
        ),
        (
            Some(json!(1500.5)),
            Some(json!(1500)),
            Some(json!(2499)),
            Some(json!(466.5)),
        ),
    ];
    for provider in [Provider::Pi, Provider::Omp] {
        for (duration, llm, tool, rate) in &variants {
            let mut assistant = pi_message(
                "assistant",
                1001_000,
                json!({"usage":{"input":20,"cacheRead":80,"cacheWrite":10,"output":700},"content":[{"type":"toolCall","id":"t1"}]}),
            );
            if let Some(duration) = duration {
                assistant["message"]["_piClaudeStyleThinkingDurationMs"] = duration.clone();
            }
            let rows = [
                pi_message("user", 1000_000, json!({})),
                assistant,
                pi_message("toolResult", 1005_000, json!({"toolCallId":"t1"})),
            ];
            let snapshot = agent_snapshot(provider, &rows);
            assert_eq!(snapshot.get("llm_ms"), llm.as_ref());
            assert_eq!(snapshot.get("tool_ms"), tool.as_ref());
            assert_eq!(snapshot.get("tok_s"), rate.as_ref());
            assert_eq!(snapshot["in_tok"], 110);
            assert_eq!(snapshot["cache_pct"], 73);
            assert_eq!(snapshot["cache_read_tok"], 80);
            assert_eq!(snapshot["ttft_ms"], 1000);
            assert!(!snapshot.contains_key("cache_write_tok"));
            assert!(!snapshot.contains_key("cache_write"));
        }
    }
}

fn kimi_usage(time: i64) -> serde_json::Value {
    json!({"type":"usage.record","time":time,"usage":{"inputOther":20,"inputCacheRead":80,"inputCacheCreation":10,"output":70}})
}

#[test]
fn kimi_stats_usage_records_are_deltas_and_first_content_is_counted_once() {
    let rows = [
        json!({"type":"turn.prompt","time":1000_000}),
        json!({"type":"llm.request","time":1001_000}),
        kimi_loop("content.part", 1002_000, ""),
        kimi_loop("content.part", 1003_000, ""),
        kimi_usage(1004_000),
        json!({"type":"llm.request","time":1005_000}),
        kimi_usage(1007_000),
    ];
    let snapshot = agent_snapshot(Provider::Kimi, &rows);
    assert_eq!(snapshot["turns"], 1);
    assert_eq!(snapshot["steps"], 2);
    assert_eq!(snapshot["in_tok"], 220);
    assert_eq!(snapshot["out_tok"], 140);
    assert_eq!(snapshot["llm_ms"], 5000);
    assert_eq!(snapshot["tok_s"], 28.0);
    assert_eq!(snapshot["cache_read_tok"], 160);
    assert_eq!(snapshot["ttft_ms"], 2000);
}

#[test]
fn kimi_stats_tool_alias_and_first_response_average_preserve_turns() {
    let rows = [
        json!({"type":"turn.prompt","time":1000_000}),
        json!({"type":"llm.request","time":1001_000}),
        kimi_loop("content.part", 1002_000, ""),
        json!({"type":"context.append_loop_event","time":1003_000,"event":{"type":"tool.call","id":"","toolCallId":"alias"}}),
        json!({"type":"context.append_loop_event","time":1005_000,"event":{"type":"tool.result","toolCallId":"alias"}}),
        kimi_usage(1006_000),
        json!({"type":"turn.prompt","time":1010_000}),
        kimi_loop("content.part", 1011_000, ""),
        kimi_usage(1012_000),
    ];
    let snapshot = agent_snapshot(Provider::Kimi, &rows);
    assert_eq!(snapshot["turns"], 2);
    assert_eq!(snapshot["steps"], 2);
    assert_eq!(snapshot["tool_ms"], 2000);
    assert_eq!(snapshot["ttft_ms"], 1500);
    assert_eq!(snapshot["llm_ms"], 5000);
}

#[test]
fn kimi_stats_request_clock_is_consumed_even_without_valid_usage() {
    let rows = [
        json!({"type":"llm.request","time":1001_000}),
        json!({"type":"llm.request","time":true}),
        kimi_usage(1006_000),
        json!({"type":"llm.request","time":1007_000}),
        json!({"type":"usage.record","time":1008_000,"usage":false}),
        kimi_usage(1009_000),
    ];
    let snapshot = agent_snapshot(Provider::Kimi, &rows);
    assert_eq!(snapshot["turns"], 0);
    assert_eq!(snapshot["steps"], 2);
    assert_eq!(snapshot["in_tok"], 220);
    assert_eq!(snapshot["out_tok"], 140);
    assert_eq!(snapshot["llm_ms"], 1000);
    assert!(!snapshot.contains_key("ttft_ms"));
    let gaps = [
        json!({"type":"llm.request","time":1000}),
        kimi_usage(3601001),
        json!({"type":"llm.request","time":10000}),
        kimi_usage(9000),
        json!({"type":"llm.request","time":10000}),
        kimi_usage(3601000),
    ];
    let snapshot = agent_snapshot(Provider::Kimi, &gaps);
    assert_eq!(snapshot["steps"], 3);
    assert_eq!(snapshot["llm_ms"], 3591000);
}

fn native_usage(provider: Provider, output: i64) -> serde_json::Value {
    if provider == Provider::Kimi {
        json!({"type":"usage.record","time":1003_000,"usage":{"inputOther":1,"output":output}})
    } else {
        pi_message(
            "assistant",
            1003_000,
            json!({"usage":{"input":1,"output":output}}),
        )
    }
}

fn claude_row(kind: &str, second: u8, message: serde_json::Value) -> serde_json::Value {
    json!({"type":kind,"timestamp":format!("2026-01-01T00:00:{second:02}Z"),"message":message})
}
fn codex_row(second: u8, payload: serde_json::Value) -> serde_json::Value {
    json!({"type":"response_item","timestamp":format!("2026-01-01T00:00:{second:02}Z"),"payload":payload})
}
fn codex_counter() -> serde_json::Value {
    json!({"type":"event_msg","timestamp":"2026-01-01T00:00:12Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1,"output_tokens":1}}}})
}
#[test]
fn agent_stats_helpers_keep_the_existing_claude_and_codex_policies() {
    let clock_claude = [
        claude_row("user", 0, json!({"content":"pedido"})),
        claude_row(
            "assistant",
            10,
            json!({"id":"a","usage":{"output_tokens":50}}),
        ),
        claude_row("user", 5, json!({"content":"outro"})),
        claude_row(
            "assistant",
            12,
            json!({"id":"b","usage":{"output_tokens":50}}),
        ),
    ];
    let clock_codex = [
        codex_row(
            0,
            json!({"type":"message","role":"user","content":[{"text":"pedido"}]}),
        ),
        codex_row(10, json!({"type":"reasoning"})),
        codex_row(
            5,
            json!({"type":"message","role":"user","content":[{"text":"outro"}]}),
        ),
        codex_row(12, json!({"type":"reasoning"})),
        codex_counter(),
    ];
    for (provider, rows) in [
        (Provider::Claude, clock_claude.as_slice()),
        (Provider::Codex, clock_codex.as_slice()),
    ] {
        assert_eq!(agent_snapshot(provider, rows)["llm_ms"], 12000);
    }
    let calls: Vec<_> = (0..200)
        .map(|i| json!({"type":"tool_use","id":format!("lost-{i}")}))
        .collect();
    let tools_claude = [
        claude_row("user", 0, json!({"content":"pedido"})),
        claude_row("assistant", 1, json!({"id":"a","usage":{},"content":calls})),
        claude_row(
            "assistant",
            2,
            json!({"id":"b","usage":{},"content":[{"type":"tool_use","id":"live"}]}),
        ),
        claude_row(
            "user",
            5,
            json!({"content":[{"type":"tool_result","tool_use_id":"live"}]}),
        ),
    ];
    let mut tools_codex = vec![codex_row(
        0,
        json!({"type":"message","role":"user","content":[{"text":"pedido"}]}),
    )];
    tools_codex.extend((0..200).map(|i| {
        codex_row(
            1,
            json!({"type":"function_call","call_id":format!("lost-{i}")}),
        )
    }));
    tools_codex.extend([
        codex_row(2, json!({"type":"function_call","call_id":"live"})),
        codex_row(5, json!({"type":"function_call_output","call_id":"live"})),
        codex_counter(),
    ]);
    for (provider, rows) in [
        (Provider::Claude, tools_claude.as_slice()),
        (Provider::Codex, tools_codex.as_slice()),
    ] {
        assert_eq!(agent_snapshot(provider, rows)["tool_ms"], 3000);
    }
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config {cases:48,rng_seed:proptest::test_runner::RngSeed::Fixed(0x5354_4154_5341),failure_persistence:None,..proptest::test_runner::Config::default()})]
    #[test]
    fn agent_stats_external_json_never_panics(raw in proptest::collection::vec(proptest::collection::vec(proptest::prelude::any::<u8>(),0..128),0..8),shapes in proptest::collection::vec(0u8..6,0..8)) {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("session.jsonl");
        let mut data=format!("{}\n{}\n",native_usage(Provider::Pi,1),native_usage(Provider::Kimi,1)).into_bytes();
        for (i,raw) in raw.iter().enumerate() {
            let line=match shapes.get(i).copied().unwrap_or(0) {
                0=>raw.clone(),
                1=>json!({"type":"message","message":{"role":"assistant","timestamp":true,"_piClaudeStyleThinkingDurationMs":true,"usage":{"input":u64::MAX,"cacheRead":true,"cacheWrite":"99","output":i64::MIN},"content":[null,{"type":"toolCall","id":raw.len()}]}}).to_string().into_bytes(),
                2=>json!({"type":"usage.record","time":raw.len(),"usage":{"inputOther":u64::MAX,"inputCacheRead":u64::MAX,"inputCacheCreation":u64::MAX,"output":u64::MAX}}).to_string().into_bytes(),
                3=>json!({"type":"context.append_loop_event","time":false,"event":{"type":"tool.result","id":raw.len(),"toolCallId":[]}}).to_string().into_bytes(),
                4=>json!({"type":"llm.request","time":i64::MIN}).to_string().into_bytes(),
                _=>json!({"type":"message","message":raw.len()}).to_string().into_bytes(),
            };
            data.extend(line);data.push(b'\n');
        }
        std::fs::write(&path,data).unwrap();
        for provider in [Provider::Pi,Provider::Omp,Provider::Kimi] {
            let lease=acquire(provider,&path);let result=lease.acc.lock().unwrap().collect();
            proptest::prop_assert!(result.is_ok());
            let snapshot=result.unwrap();proptest::prop_assert!(snapshot.is_some());
            let snapshot=snapshot.unwrap();
            proptest::prop_assert!(snapshot.get("steps").and_then(serde_json::Value::as_i64).is_some_and(|n|n>=1));
        }
    }
}

#[test]
fn agent_stats_non_numeric_timestamps_never_become_durations() {
    for provider in [Provider::Pi, Provider::Omp] {
        let rows = [
            json!({"type":"message","message":{"role":"user","timestamp":true}}),
            json!({"type":"message","message":{"role":"assistant","timestamp":"1001000","usage":{"input":20,"cacheRead":80,"cacheWrite":10,"output":700}}}),
        ];
        let snapshot = agent_snapshot(provider, &rows);
        assert_eq!(snapshot["turns"], 1);
        assert_eq!(snapshot["steps"], 1);
        assert_eq!(snapshot["in_tok"], 110);
        assert!(!snapshot.contains_key("llm_ms"));
        assert!(!snapshot.contains_key("ttft_ms"));
    }
    let rows = [
        json!({"type":"turn.prompt","time":true}),
        json!({"type":"llm.request","time":"1001000"}),
        json!({"type":"context.append_loop_event","time":1002_000,"event":[]}),
        kimi_loop("content.part", 1003_000, ""),
        json!({"type":"usage.record","time":1004_000,"usage":{}}),
        json!({"type":"usage.record","time":1005_000,"usage":false}),
    ];
    let snapshot = agent_snapshot(Provider::Kimi, &rows);
    assert_eq!(snapshot["turns"], 1);
    assert_eq!(snapshot["steps"], 1);
    assert_eq!(snapshot["in_tok"], 0);
    assert!(!snapshot.contains_key("llm_ms"));
    assert!(!snapshot.contains_key("ttft_ms"));
}

#[test]
fn pi_stats_usage_sums_each_assistant_record() {
    for provider in [Provider::Pi, Provider::Omp] {
        let rows = [
            pi_message("user", 1000_000, json!({})),
            pi_message(
                "assistant",
                1001_000,
                json!({"usage":{"input":20,"cacheRead":80,"cacheWrite":10,"output":700}}),
            ),
            pi_message(
                "assistant",
                1002_000,
                json!({"usage":{"input":20,"cacheRead":80,"cacheWrite":10,"output":700}}),
            ),
        ];
        let snapshot = agent_snapshot(provider, &rows);
        assert_eq!(snapshot["steps"], 2);
        assert_eq!(snapshot["in_tok"], 220);
        assert_eq!(snapshot["out_tok"], 1400);
        assert_eq!(snapshot["cache_read_tok"], 160);
        assert_eq!(snapshot["llm_ms"], 2000);
        assert_eq!(snapshot["ttft_ms"], 1000);
    }
}

#[test]
fn pi_stats_malformed_usage_keeps_tool_and_clock_events() {
    for provider in [Provider::Pi, Provider::Omp] {
        let rows = [
            json!({"type":"session"}),
            json!({"type":"message","message":[]}),
            pi_message("user", 1000_000, json!({})),
            pi_message(
                "assistant",
                1001_000,
                json!({"usage":false,"content":[null,{"type":"toolCall","id":"t"}]}),
            ),
            pi_message("toolResult", 1003_000, json!({"toolCallId":"t"})),
            pi_message(
                "assistant",
                1004_000,
                json!({"usage":{"input":20,"cacheRead":80,"cacheWrite":10,"output":700},"content":[{"type":"toolCall","id":"t1"}]}),
            ),
        ];
        let snapshot = agent_snapshot(provider, &rows);
        assert_eq!(snapshot["steps"], 1);
        assert_eq!(snapshot["llm_ms"], 2000);
        assert_eq!(snapshot["tool_ms"], 2000);
        assert_eq!(snapshot["ttft_ms"], 1000);
        assert_eq!(snapshot["in_tok"], 110);
        assert_eq!(snapshot["out_tok"], 700);
    }
}
