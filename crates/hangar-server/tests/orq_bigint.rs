use hangar_server::orchestration::{self as orq, panel, runs, state, timeline};
use serde_json::{Value, json};
use std::collections::BTreeSet;
fn fixture() -> Value {
    serde_json::from_str(include_str!("../src/orchestration/fixtures/bigint.json")).unwrap()
}
#[test]
fn python_task_integers_preserve_every_digit_and_type() {
    for case in orq::array(&fixture()["integers"]) {
        assert_eq!(
            orq::integer(&case["input"])
                .map(|n| json!(n.to_string()))
                .unwrap_or_default(),
            case["digits"],
            "{case}"
        );
    }
}
#[test]
fn python_large_task_state_summary_panel_and_event_ids_are_exact() {
    for case in orq::array(&fixture()["cases"]) {
        let temporary = tempfile::tempdir().unwrap();
        let closed_path = temporary.path().join("closed.jsonl");
        std::fs::write(
            &closed_path,
            orq::array(&case["closed"])
                .iter()
                .map(|v| format!("{v}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let closed = state::closed(temporary.path()).unwrap();
        let open = state::reduce(
            &case["config"],
            orq::array(&case["events"]),
            &Default::default(),
            0.,
        );
        assert_eq!(
            open, case["state_open"],
            "estado aberto {}",
            case["task_digits"]
        );
        assert_eq!(
            state::reduce(&case["config"], orq::array(&case["events"]), &closed, 0.),
            case["state"],
            "estado fechado {}",
            case["task_digits"]
        );
        assert_eq!(
            json!(runs::cards(&[case["summary"].clone()])),
            case["cards"]
        );
        assert_eq!(
            runs::summary("run", orq::array(&case["events"])),
            case["summary"],
            "resumo {}",
            case["task_digits"]
        );
        assert_eq!(
            json!(panel::task_rows(
                orq::array(&case["events"]),
                &[],
                &BTreeSet::new()
            )),
            case["rows"],
            "linhas {}",
            case["task_digits"]
        );
        assert_eq!(
            json!(panel::team(
                &case["config"],
                &open,
                orq::array(&case["events"])
            )),
            case["team"]
        );
        let integrated = orq::integer(&case["events"][1]["task"])
            .into_iter()
            .collect();
        assert_eq!(
            json!(panel::task_rows(
                orq::array(&case["events"]),
                &[],
                &integrated
            )),
            case["integrated_rows"],
            "integradas {}",
            case["task_digits"]
        );
    }
    for case in orq::array(&fixture()["entries"]) {
        assert_eq!(
            timeline::entry(&case["input"], None).unwrap_or_default(),
            case["entry"],
            "{case}"
        );
        assert_eq!(timeline::event_id(&case["input"]), case["id"], "{case}");
    }
}

#[test]
fn python_numeric_sort_and_markdown_end_of_line_are_exact() {
    let f = fixture();
    let mixed = &f["mixed"];
    assert_eq!(
        json!(panel::task_rows(
            orq::array(&mixed["events"]),
            &[],
            &BTreeSet::new()
        )),
        mixed["rows"]
    );
    assert_eq!(
        runs::summary("run", orq::array(&mixed["events"])),
        mixed["summary"]
    );
    for case in orq::array(&f["code"]) {
        let entry = timeline::entry(&case["obj"], None).unwrap();
        assert_eq!(entry, case["entry"], "{case}");
    }
}
#[test]
fn python_lossless_json_canonical_ids_and_nonfinite_shadow_are_exact() {
    let f = fixture();
    for case in orq::array(&f["raw_cases"]) {
        let value = orq::codec::loads(case["raw"].as_str().unwrap()).unwrap();
        assert_eq!(
            orq::codec::dumps(&value, true),
            case["dump"].as_str().unwrap(),
            "{case}"
        );
        assert_eq!(timeline::event_id(&value), case["id"], "{case}");
        let entry = timeline::entry(&value, None).unwrap();
        let expected = orq::codec::loads(case["entry_raw"].as_str().unwrap()).unwrap();
        assert_eq!(entry, expected, "{case}");
    }
    let tmp = tempfile::tempdir().unwrap();
    let case = &f["shadow"];
    std::fs::write(
        tmp.path().join("jev-shadow.jsonl"),
        format!("{}\n", case["raw"].as_str().unwrap()),
    )
    .unwrap();
    assert_eq!(
        timeline::entry(&case["input"], Some(tmp.path())).unwrap(),
        case["entry"]
    );
}
#[test]
fn shadow_metadata_failure_warns_once_without_conversation_text() {
    use std::sync::{Arc, Mutex};
    #[derive(Clone)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let sink = Sink(bytes.clone());
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_writer(move || sink.clone())
        .finish();
    let tmp = tempfile::tempdir().unwrap();
    // No Windows, um arquivo usado como pasta devolve NotFound; o nome inválido força o erro de metadados.
    let blocked = if cfg!(windows) {
        tmp.path().join("blocked<")
    } else {
        let path = tmp.path().join("blocked");
        std::fs::write(&path, "x").unwrap();
        path
    };
    assert_ne!(std::fs::metadata(blocked.join("jev-shadow.jsonl")).unwrap_err().kind(),
        std::io::ErrorKind::NotFound);
    let obj = &fixture()["shadow"]["input"];
    tracing::subscriber::with_default(subscriber, || {
        timeline::entry(obj, Some(&blocked));
        timeline::entry(obj, Some(&blocked));
    });
    let text = String::from_utf8(bytes.lock().unwrap().clone()).unwrap();
    assert_eq!(
        text.matches("jev-shadow.jsonl ilegível").count(),
        fixture()["metadata_warning_count"].as_u64().unwrap() as usize,
        "{text}"
    );
    assert!(!text.contains("mensagem sintética"));
}

#[test]
fn python_plan_tasks_context_labels_and_float_timestamp_cache_are_exact() {
    let f = fixture();
    assert_eq!(
        json!(orq::plan::tasks(f["plan"]["input"].as_str().unwrap())),
        f["plan"]["output"]
    );
    let tmp = tempfile::tempdir().unwrap();
    let c = &f["mtime"];
    let path = tmp.path().join(".hangar-pi/models/fixture.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let at = |ns| {
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(ns)),
            )
            .unwrap()
    };
    std::fs::write(&path, r#"{"models":[],"current":{"provider":"first"}}"#).unwrap();
    at(c["stamp_ns"].as_u64().unwrap());
    assert_eq!(
        json!(hangar_server::provider_models::pi_provider(
            "fixture.jsonl",
            tmp.path()
        )),
        c["first"]
    );
    std::fs::write(&path, r#"{"models":[],"current":{"provider":"second"}}"#).unwrap();
    at(c["stamp_ns"].as_u64().unwrap() + c["delta_ns"].as_u64().unwrap());
    assert_eq!(
        json!(hangar_server::provider_models::pi_provider(
            "fixture.jsonl",
            tmp.path()
        )),
        c["after"]
    );
}

#[tokio::test]
async fn provider_models_pm39_warm_only_launches_present_executables_and_keeps_cli_argv() {
    use hangar_server::provider_models::{catalog, warm};
    use std::sync::{Arc, Mutex};
    if let Ok(mode) = std::env::var("HANGAR_WARM_PROOF") {
        #[derive(Clone)]
        struct Sink(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Sink {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let sink = Sink(bytes.clone());
        let _guard = tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_writer(move || sink.clone())
                .finish(),
        );
        warm();
        if mode == "absent" {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        } else {
            let marker = std::env::var("HANGAR_WARM_MARKER").unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(5), async {
                while !std::path::Path::new(&marker).exists() {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
            let got = catalog(&mode).await.unwrap();
            let expected: Value =
                serde_json::from_str(&std::env::var("HANGAR_WARM_EXPECTED").unwrap()).unwrap();
            assert_eq!(json!(got), expected);
            let args = std::fs::read_to_string(marker).unwrap();
            assert_eq!(
                args.trim(),
                if mode == "pi" {
                    "--list-models"
                } else {
                    "models --json"
                }
            );
        }
        assert!(
            !String::from_utf8_lossy(&bytes.lock().unwrap())
                .contains("provider_catalog_refresh_failed")
        );
        return;
    }
    for mode in ["absent", "pi", "omp"] {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("argv");
        let expected = if mode == "omp" {
            fixture()["labels"]
                .as_array()
                .unwrap()
                .iter()
                .filter_map(|c| c.get("output"))
                .flat_map(|v| v.as_array().unwrap().clone())
                .collect::<Vec<_>>()
        } else {
            vec![
                json!({"provider":"fixture","id":"one","context":"1K","max_out":"1K","thinking":false,"images":false}),
            ]
        };
        if mode != "absent" {
            let output = if mode == "pi" {
                "fixture one 1K 1K no no".into()
            } else {
                let rows = fixture()["labels"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .filter(|c| c.get("output").is_some())
                    .map(|c| {
                        orq::codec::loads(c["raw"].as_str().unwrap()).unwrap()["models"][0].clone()
                    })
                    .collect::<Vec<_>>();
                orq::codec::dumps(&json!({"models":rows}), false)
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let path = tmp.path().join(mode);
                std::fs::write(
                    &path,
                    format!(
                        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nprintf '%s\\n' '{}'\n",
                        marker.display(),
                        output
                    ),
                )
                .unwrap();
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            #[cfg(windows)]
            {
                std::fs::write(
                    tmp.path().join(format!("{mode}.cmd")),
                    format!(
                        "@echo off\r\necho %*>>\"{}\"\r\necho {output}\r\n",
                        marker.display()
                    ),
                )
                .unwrap();
            }
        }
        let mut child = std::process::Command::new(std::env::current_exe().unwrap());
        child
            .args([
                "--exact",
                "provider_models_pm39_warm_only_launches_present_executables_and_keeps_cli_argv",
                "--nocapture",
            ])
            .env_clear()
            .env("HANGAR_WARM_PROOF", mode)
            .env("PATH", tmp.path())
            .env("HOME", tmp.path())
            .env("USERPROFILE", tmp.path())
            .env("HANGAR_WARM_MARKER", &marker)
            .env("HANGAR_WARM_EXPECTED", json!(expected).to_string())
            .env("PATHEXT", ".COM;.EXE;.BAT;.CMD");
        for key in ["SystemRoot", "ComSpec", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                child.env(key, value);
            }
        }
        let output = child.output().unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        if mode == "absent" {
            assert!(!marker.exists());
        }
    }
}

#[tokio::test]
async fn python_large_task_http_details_preserve_complete_numeric_payload() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    use hangar_server::{
        accounts::environment::AccountEnvironment, config::Config, routes::AppState,
    };
    use std::{collections::BTreeMap, sync::Arc};
    let tmp = tempfile::tempdir().unwrap();
    let config = Config::from_lookup(|key| match key {
        "HANGAR_SERVER_LISTEN" => Some("127.0.0.1:18765".into()),
        "HANGAR_SERVER_UPSTREAM" => Some("127.0.0.1:1".into()),
        "HANGAR_INTERNAL_SECRET" => Some("fixture-internal".into()),
        "CP_AUTH_TOKEN" => Some("fixture-owner".into()),
        _ => None,
    })
    .unwrap();
    let mut state = AppState::new(config);
    state.accounts.env = AccountEnvironment::from_map(BTreeMap::from([
        ("HOME".into(), tmp.path().to_string_lossy().into_owned()),
        (
            "USERPROFILE".into(),
            tmp.path().to_string_lossy().into_owned(),
        ),
    ]));
    let state = Arc::new(state);
    let dir = tmp.path().join(".hangar/orq/run");
    std::fs::create_dir_all(&dir).unwrap();
    for case in orq::array(&fixture()["cases"]) {
        std::fs::write(dir.join("orq.json"), case["config"].to_string()).unwrap();
        std::fs::write(
            dir.join("eventos.jsonl"),
            orq::array(&case["events"])
                .iter()
                .map(|v| format!("{v}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let request = Request::builder()
            .uri("/api/orq/run")
            .body(Body::empty())
            .unwrap();
        let response = orq::serve(&state, request).await;
        assert_eq!(response.status(), 200);
        assert_eq!(response.headers()["content-type"], "application/json");
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let got = orq::codec::loads(std::str::from_utf8(&bytes).unwrap()).unwrap();
        assert_eq!(got, case["detail"], "{}", case["task_digits"]);
        assert!(got["tasks"][0]["task"].is_number());
    }
}
#[test]
fn python_cli_events_round_trip_large_input_nonfinite_surrogates_and_literal_reserved_keys() {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let tmp = tempfile::tempdir().unwrap();
    let f = fixture();
    let mut lines = orq::array(&f["raw_cases"])
        .iter()
        .map(|v| format!("{}\n", v["raw"].as_str().unwrap()))
        .collect::<String>();
    let huge = json!({"task":f["integers"][9]["input"],"text":"Task sintética"});
    lines.push_str(&format!("{}\n", orq::codec::dumps(&huge, false)));
    std::fs::write(tmp.path().join("eventos.jsonl"), &lines).unwrap();
    let expected = lines
        .lines()
        .map(|line| orq::codec::loads(line).unwrap())
        .collect::<Vec<_>>();
    let input = json!({"directory":tmp.path(),"task":f["integers"][9]["input"],"padding":"x".repeat((1<<20)+17)});
    let mut child = Command::new(env!("CARGO_BIN_EXE_hangar-server"))
        .args(["orq-info", "events"])
        .env_clear()
        .env("HOME", tmp.path())
        .env("USERPROFILE", tmp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(orq::codec::dumps(&input, false).as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = std::str::from_utf8(&output.stdout).unwrap();
    let value = orq::codec::loads(raw).unwrap();
    assert_eq!(value, json!({"ok":true,"result":expected}));
    assert!(value["result"].as_array().unwrap().last().unwrap()["task"].is_number());
    assert!(!raw.contains("orq-nonfinite"));
}
#[test]
fn transcript_json_keeps_literal_reserved_keys_and_canonical_numeric_types() {
    for key in [
        "$serde_json::private::Number",
        "$serde_json::private::RawValue",
    ] {
        let raw =
            format!(r#"{{"{key}":"literal","nested":{{"{key}":"42"}},"array":[{{"{key}":42}}]}}"#);
        let value = hangar_server::transcript::pyjson::loads_lossless(&raw).unwrap();
        assert_eq!(value[key], "literal");
        assert!(value["nested"].is_object());
        assert!(value["array"][0].is_object());
    }
    for raw in ["01", "+1", "-00", "1 2", "\u{b}1", "NaN", "Infinity"] {
        assert!(
            hangar_server::transcript::pyjson::loads_lossless(raw).is_none(),
            "{raw}"
        );
    }
    for (raw, expected) in [
        ("1e3", "1000.0"),
        ("-0", "-0.0"),
        ("1.0", "1.0"),
        ("18446744073709551616", "18446744073709551616"),
        (
            "340282366920938463463374607431768211579",
            "340282366920938463463374607431768211579",
        ),
    ] {
        let value = hangar_server::transcript::pyjson::loads_lossless(raw).unwrap();
        assert_eq!(
            hangar_server::transcript::pyjson::dumps(&value, false),
            expected,
            "{raw}"
        );
    }
    let orq_zero = orq::codec::loads("-0").unwrap();
    assert_eq!(orq_zero.as_u64(), Some(0));
    assert_eq!(orq::codec::dumps(&orq_zero, false), "0");
}
