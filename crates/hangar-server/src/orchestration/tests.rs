use super::*;
use serde_json::{Value, json};
use std::{
    fs,
    path::Path,
    sync::{Arc, Barrier},
};
fn fixture(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/orchestration/fixtures")
        .join(name);
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}
fn isolated_state(home: &Path) -> Arc<crate::routes::AppState> {
    let mut st = crate::routes::AppState::new(crate::config::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: "127.0.0.1:1".parse().unwrap(),
        internal_secret: "fixture-secret".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
    });
    st.accounts.env = crate::accounts::environment::AccountEnvironment::from_map(
        std::collections::BTreeMap::from([
            ("HOME".into(), home.to_string_lossy().into_owned()),
            ("USERPROFILE".into(), home.to_string_lossy().into_owned()),
        ]),
    );
    st.groups = None;
    Arc::new(st)
}
#[tokio::test]
async fn http_handler_future_is_send() {
    fn send<T:Send>(value:T)->T{value}
    let temporary=tempfile::tempdir().unwrap();let st=isolated_state(temporary.path());
    let request=axum::http::Request::builder().uri("/api/orq/missing")
        .body(axum::body::Body::empty()).unwrap();
    let _=send(serve(&st,request)).await;
}
#[tokio::test]
async fn or059_or065_or066_or069_http_role_writes_match_python_effects() {
    use axum::{body::{Body,to_bytes},http::Request};
    for case in array(&fixture("http-writes.json")["cases"]) {
        let temporary=tempfile::tempdir().unwrap();let home=temporary.path();let st=isolated_state(home);
        let pair=home.join(".claude/.hangar-pair");fs::create_dir_all(&pair).unwrap();
        let side=home.join(".hangar/claude-headless");fs::create_dir_all(&side).unwrap();
        fs::write(side.join("planner.json"),"{\"key\":\"key\"}").unwrap();
        let file=pair.join(text(&case["file"]));
        if !text(&case["initial"]).is_empty(){fs::write(&file,text(&case["initial"])).unwrap();}
        let mut body=case["body"].clone();body["mtime"]=json!(read(&file).unwrap().1);
        let request=Request::builder().method(text(&case["method"])).uri(text(&case["path"]))
            .header("content-type","application/json").body(Body::from(body.to_string())).unwrap();
        let response=serve(&st,request).await;
        assert_eq!(response.status().as_u16() as u64,case["status"].as_u64().unwrap(),"{case}");
        let mut value:Value=serde_json::from_slice(&to_bytes(response.into_body(),65536).await.unwrap()).unwrap();
        let version=value.as_object_mut().unwrap().remove("mtime");
        if case["version"]==true{assert_eq!(version,Some(json!(read(&file).unwrap().1)));}else{assert!(version.is_none());}
        assert_eq!(value,case["output"],"{case}");
        assert_eq!(fs::read_to_string(&file).ok().map(Value::String).unwrap_or_default(),case["final"],"{case}");
    }
}
#[tokio::test]
async fn or068_or070_or073_or075_http_invalid_bodies_match_python() {
    use axum::{
        body::{Body, to_bytes},
        http::Request,
    };
    let temporary = tempfile::tempdir().unwrap();
    let st = isolated_state(temporary.path());
    for case in array(&fixture("http-invalid.json")["cases"]) {
        let request = Request::builder()
            .method(text(&case["method"]))
            .uri(text(&case["path"]))
            .header("content-type", text(&case["content_type"]))
            .body(Body::from(text(&case["body"]).to_owned()))
            .unwrap();
        let response = serve(&st, request).await;
        assert_eq!(
            response.status().as_u16() as u64,
            case["status"].as_u64().unwrap(),
            "{case}"
        );
        let value: Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
        assert_eq!(value, case["output"], "{case}");
    }
    assert!(!temporary.path().join(".claude/.hangar-pair").exists());
}
#[test]
fn review_filetime_identity_preserves_fraction_before_float_conversion() {
    assert_eq!(
        context::filetime_seconds(134060400001234567),
        1761566400.1234567
    );
    assert_eq!(context::filetime_seconds(116444735999999999), -0.0000001);
}
#[test]
fn review_claude_start_day_uses_the_usage_bucket_timezone() {
    let since = chrono::DateTime::parse_from_rfc3339("2026-10-01T01:00:00Z").unwrap();
    let usage = crate::costs::py::LocalTs::from_iso("2026-10-01T02:00:00Z").unwrap();
    assert!(!consumption::before_start_day(usage, since));
}
#[tokio::test]
async fn review_unreadable_policy_aborts_role_write() {
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    let d = tempfile::tempdir().unwrap();
    let mut st = crate::routes::AppState::new(crate::config::Config {
        listen: "127.0.0.1:0".parse().unwrap(),
        upstream: "127.0.0.1:1".parse().unwrap(),
        internal_secret: "fixture-secret".into(),
        auth_token: "fixture-owner".into(),
        log_path: None,
        trusted: crate::auth::TrustedHosts::parse("127.0.0.1"),
    });
    st.accounts.env = crate::accounts::environment::AccountEnvironment::from_map(
        std::collections::BTreeMap::from([
            ("HOME".into(), d.path().to_string_lossy().into_owned()),
            (
                "USERPROFILE".into(),
                d.path().to_string_lossy().into_owned(),
            ),
        ]),
    );
    st.groups = None;
    let path = policy::path(d.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, [0xff, 0xfe]).unwrap();
    let req = Request::builder().method("POST").uri("/api/sessions/planner/orq/papel")
        .header("content-type","application/json").body(Body::from(json!({
            "papel":"executor","provider":"claude","conta":"padrao","modelo":"opus","esforco":"high","mtime":0
        }).to_string())).unwrap();
    let reply = serve(&Arc::new(st), req).await;
    assert_eq!(reply.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let value: Value =
        serde_json::from_slice(&to_bytes(reply.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(value["detail"]["code"], "orq_policy_read_failed");
    assert_eq!(fs::read(path).unwrap(), [0xff, 0xfe]);
    assert!(!d.path().join(".claude/.hangar-pair").exists());
}
#[test]
fn or048_cost_rounding_follows_python_binary_float() {
    for c in array(&fixture("rounding.json")["cases"]) {
        assert_eq!(
            round(
                c["input"].as_f64().unwrap(),
                c["digits"].as_i64().unwrap() as i32
            ),
            c["output"].as_f64().unwrap()
        );
    }
}
#[test]
fn or032_or040_or041_or042_or050_python_state_and_panel_contract() {
    for case in array(&fixture("state-panel.json")["cases"]) {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("run");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("orq.json"), case["config"].to_string()).unwrap();
        for (name, key) in [("eventos.jsonl", "events"), ("closed.jsonl", "closed")] {
            fs::write(
                directory.join(name),
                array(&case[key])
                    .iter()
                    .map(|v| format!("{v}\n"))
                    .collect::<String>(),
            )
            .unwrap();
        }
        assert_eq!(state::read(&directory).unwrap(), case["state"]);
        let (actual, aux) = panel::build(&directory, temporary.path());
        assert_eq!(actual, case["panel"]);
        assert_eq!(aux, case["aux"]);
    }
}
#[test]
fn or001_or007_or008_or010_python_summaries() {
    for c in array(&fixture("summaries.json")["cases"]) {
        assert_eq!(runs::summary("run", array(&c["events"])), c["summary"]);
    }
}
#[test]
fn or011_hostile_execution_ids_are_not_paths() {
    for id in ["", ".", "..", "../other", "D:foo", "a\\b", "x\0y"] {
        assert!(runs::exec_dir(Path::new("root"), id).is_none());
    }
    assert!(runs::exec_dir(Path::new("root"), "execução-1").is_some());
}
#[test]
fn or003_or004_partial_utf8_does_not_drop_other_events() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("eventos.jsonl");
    fs::write(
        &path,
        b"{\"tipo\":\"execucao_inicio\"}\n{\"tipo\":\"entrega\",\"text\":\"\xc3\"}\n{broken",
    )
    .unwrap();
    assert_eq!(runs::events(&path, true).unwrap().len(), 2);
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert!(runs::events(&path, true).is_err());
    assert!(runs::events(&path, false).unwrap().is_empty());
}
#[test]
fn or019_or020_or021_watchdog_freshness_and_platform() {
    let d = tempfile::tempdir().unwrap();
    let at = runs::when(&json!("2026-10-01T00:00:00+00:00"))
        .unwrap()
        .timestamp() as f64;
    fs::write(d.path().join("vigia.json"),json!({"ts":"2026-10-01T00:00:00+00:00","interval_s":60,"arbiter":"arb","watching":["a",2],"pid":false}).to_string()).unwrap();
    assert_eq!(runs::watchdog(d.path(), None, at + 150.)["alive"], true);
    assert_eq!(runs::watchdog(d.path(), None, at + 151.)["alive"], false);
    let units = json!({d.path().to_string_lossy().to_string():{"ActiveState":"inactive","Id":"vigia-g.service","NRestarts":"2"}});
    let w = runs::watchdog(d.path(), Some(&units), at);
    assert_eq!(w["alive"], false);
    assert_eq!(w["restarts"], 2);
    assert_eq!(w["unit"], "vigia-g");
    assert_eq!(w["watching"], json!(["a", "2"]));
}
#[test]
fn or021_without_heartbeat_distinguishes_absence() {
    let d = tempfile::tempdir().unwrap();
    assert_eq!(runs::watchdog(d.path(), None, 0.)["source"], "unavailable");
    assert_eq!(
        runs::watchdog(d.path(), Some(&json!({})), 0.)["source"],
        "none"
    );
    let units = json!({d.path().to_string_lossy().to_string():{"ActiveState":"active","Id":"vigia-g.service","ActiveEnterTimestamp":"@1000"}});
    let w = runs::watchdog(d.path(), Some(&units), 0.);
    assert_eq!(w["source"], "systemd");
    assert_eq!(w["alive"], true);
}
#[test]
fn or024_or025_group_phase_does_not_depend_on_heartbeat() {
    let d = tempfile::tempdir().unwrap();
    let run = runs::root(d.path()).join("run");
    fs::create_dir_all(&run).unwrap();
    fs::write(run.join("orq.json"), "{\"auto\":true}").unwrap();
    fs::write(
        run.join("eventos.jsonl"),
        "{\"tipo\":\"execucao_inicio\",\"gid\":\"g\"}\n",
    )
    .unwrap();
    assert_eq!(runs::group_phase(d.path(), "g").as_deref(), Some("live"));
    fs::write(
        run.join("eventos.jsonl"),
        "{\"tipo\":\"execucao_inicio\",\"gid\":\"g\"}\n{\"tipo\":\"execucao_fim\"}\n",
    )
    .unwrap();
    assert_eq!(runs::group_phase(d.path(), "g").as_deref(), Some("ended"));
    fs::remove_file(run.join("eventos.jsonl")).unwrap();
    fs::create_dir(run.join("eventos.jsonl")).unwrap();
    assert_eq!(runs::group_phase(d.path(), "g").as_deref(), Some("unknown"));
}
#[test]
fn or028_activity_uses_injected_clock() {
    let d = tempfile::tempdir().unwrap();
    let path = d.path().join("timeline.jsonl");
    fs::write(&path, "x").unwrap();
    let at = mtime_meta(&fs::metadata(&path).unwrap());
    assert_eq!(runs::activity(&path, at + 119.).0, "working");
    assert_eq!(runs::activity(&path, at + 121.).0, "idle");
}
#[test]
fn or030_or031_parse_obj_keeps_text_and_stable_identity() {
    let object = json!({"text":"decisão","ts":"2026-10-01T00:00:00+00:00","kind":"notice"});
    let event = timeline::parse_obj(&object, None).remove(0);
    assert_eq!(event.text.as_deref(), Some("decisão"));
    assert_eq!(event.id, timeline::event_id(&object));
    assert!(event.orq.is_some());
    assert_eq!(event.ts, Some(1790812800.));
    assert!(timeline::parse_obj(&json!({"text":"  "}), None).is_empty());
}
#[test]
fn or056_or057_recreated_session_gets_new_draft() {
    let d = tempfile::tempdir().unwrap();
    let side = d.path().join(".hangar/claude-headless");
    let pair = d.path().join("pairs");
    fs::create_dir_all(&side).unwrap();
    fs::write(side.join("same.json"), "{\"key\":\"first\"}").unwrap();
    let first = context::resolve(d.path(), &pair, "same", "missing-mux").unwrap();
    fs::write(side.join("same.json"), "{\"key\":\"second\"}").unwrap();
    let next = context::resolve(d.path(), &pair, "same", "missing-mux").unwrap();
    assert_ne!(first.gid, next.gid);
    assert!(!next.grouped);
}
#[test]
fn or058_real_group_does_not_need_legacy_identity() {
    let d = tempfile::tempdir().unwrap();
    fs::write(
        d.path().join("legacy.json"),
        "{\"gid\":\"g\",\"peers\":[],\"orq\":true}",
    )
    .unwrap();
    let ctx = context::resolve(d.path(), d.path(), "legacy", "missing-mux").unwrap();
    assert_eq!(ctx.gid, "g");
    assert!(ctx.grouped);
}
#[test]
fn or060_same_life_in_two_live_runs_refuses_ambiguous_context() {
    let home = tempfile::tempdir().unwrap();
    let side = home.path().join(".hangar/claude-headless");
    fs::create_dir_all(&side).unwrap();
    fs::write(side.join("same.json"), "{\"key\":\"life\"}").unwrap();
    for gid in ["g1", "g2"] {
        let directory = runs::root(home.path()).join(gid);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("orq.json"), json!({
            "auto": true, "arbiter": "same", "arbiter_identity": "claude:life"
        }).to_string()).unwrap();
        fs::write(directory.join("eventos.jsonl"), format!("{}\n", json!({
            "tipo": "execucao_inicio", "gid": gid, "arbiter": "same"
        }))).unwrap();
    }
    // O contrato Python recusa escolher um grupo quando a mesma vida pertence aos dois.
    let error = context::resolve(home.path(), &home.path().join("pairs"), "same", "missing-mux")
        .unwrap_err();
    assert_eq!(error, "a sessão está registrada em mais de uma execução viva");
}
#[test]
fn or061_promotion_moves_one_record() {
    let d = tempfile::tempdir().unwrap();
    let side = d.path().join(".hangar/claude-headless");
    let pair = d.path().join("pairs");
    fs::create_dir_all(&side).unwrap();
    fs::create_dir_all(&pair).unwrap();
    fs::write(side.join("planner.json"), "{\"key\":\"key\"}").unwrap();
    let ctx = context::resolve(d.path(), &pair, "planner", "missing-mux").unwrap();
    let content = "<!-- hangar-orq-prefix: planner -->\n\n# Contrato\n";
    let mtime = write(&ctx.path, content, Some(0.)).unwrap();
    context::promote(d.path(), &pair, "planner", "g", Some(mtime), "missing-mux").unwrap();
    assert_eq!(
        fs::read_to_string(pair.join("regras-g.md")).unwrap(),
        content
    );
    assert_eq!(
        fs::read_to_string(&ctx.path).unwrap(),
        "<!-- hangar-orq-group: g -->\n"
    );
}
#[test]
fn or061_promotion_failure_kind_comes_from_the_cause_not_the_text() {
    let d = tempfile::tempdir().unwrap();
    let side = d.path().join(".hangar/claude-headless");
    let pair = d.path().join("pairs");
    fs::create_dir_all(&side).unwrap();
    fs::create_dir_all(&pair).unwrap();
    fs::write(side.join("planner.json"), "{\"key\":\"key\"}").unwrap();
    let ctx = context::resolve(d.path(), &pair, "planner", "missing-mux").unwrap();
    let mtime = write(&ctx.path, "# Contrato\n", Some(0.)).unwrap();
    assert!(matches!(
        context::promote_classified(d.path(), &pair, "planner", "g", Some(mtime + 5.), "missing-mux"),
        Err(context::PromotionError::Changed(_))
    ));
    let source = d.path().join("source.md");
    fs::write(&source, "# Contrato\n").unwrap();
    // Escrita da marca que falha com "mudou desde" exige conferência manual, não recarregar.
    let failure = context::promote_source_with(&source, &pair.join("regras-h.md"), "# Contrato\n", 1., 1.,
        |_, _, _| Err("o contrato mudou desde a leitura".into())).unwrap_err();
    assert!(!failure.changed);
}
#[test]
fn or066_compare_and_swap_has_one_winner() {
    let d = tempfile::tempdir().unwrap();
    let path = Arc::new(d.path().join("record.md"));
    let barrier = Arc::new(Barrier::new(3));
    let mut threads = Vec::new();
    for content in ["a", "b"] {
        let path = path.clone();
        let barrier = barrier.clone();
        threads.push(std::thread::spawn(move || {
            barrier.wait();
            let _guard = context::WRITE_LOCK.lock().unwrap();
            write(&path, content, Some(0.))
        }));
    }
    barrier.wait();
    let outcomes: Vec<_> = threads.into_iter().map(|h| h.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|r| r.is_ok()).count(), 1);
    assert!(matches!(
        fs::read_to_string(path.as_ref()).unwrap().as_str(),
        "a" | "b"
    ));
}
#[test]
fn or067_or068_or069_or070_python_roles() {
    let f = fixture("roles.json");
    let base = text(&fixture("markdown.json")["cases"][2]["text"]).to_owned();
    for c in array(&f["cases"]) {
        let role: roles::Role = serde_json::from_value(c["input"].clone()).unwrap();
        let actual = roles::write(&base, &role).unwrap();
        assert_eq!(actual, c["text"]);
        assert_eq!(
            serde_json::to_value(roles::read(&actual)).unwrap(),
            c["output"]
        );
    }
}
#[test]
fn or065_controls_cannot_become_instructions() {
    for value in ["a|b", "a\nb", "a\u{1b}b", "a\u{7f}b"] {
        assert!(markdown::validate(value).is_err());
    }
    assert_eq!(markdown::validate(" Olá ").unwrap(), "Olá");
}
#[test]
fn or070_context_window_refuses_invalid_percent() {
    for window in ["9", "96", "50.5", "texto"] {
        let role = roles::Role {
            papel: "executor".into(),
            janela: window.into(),
            ..Default::default()
        };
        assert!(roles::write("", &role).is_err());
    }
}
#[test]
fn or073_claude_cache_is_passive_and_per_account() {
    let d = tempfile::tempdir().unwrap();
    fs::write(d.path().join(".hangar-models.json"),json!({"ts":1000,"resp":{"models":[{"id":"opus","name":"Opus"},{"id":"opus","name":"Outro"}]}}).to_string()).unwrap();
    let (models, reduced) = policy::claude_models(d.path(), 1001.);
    assert!(!reduced);
    assert_eq!(
        models,
        json!([{"id":"opus","name":"Opus"}])
            .as_array()
            .unwrap()
            .clone()
    );
    assert!(policy::claude_models(d.path(), 1000. + 30. * 24. * 3600.).1);
}
#[test]
fn or076_or077_policy_distinguishes_models_and_efforts() {
    let p = policy::Policy {
        conta: "api".into(),
        provider: "pi".into(),
        apelido: String::new(),
        modelos: vec!["model".into()],
        trocar: false,
    };
    assert_eq!(
        policy::permitted("omp", "api", "model", "max", &[p.clone()], None),
        None
    );
    assert_eq!(
        policy::permitted("omp", "api", "other", "max", &[p], None),
        Some("erro_orq_conta_travada")
    );
    assert_eq!(
        policy::permitted("claude", "any", "opus", "xhigh", &[], None),
        None
    );
    assert_eq!(
        policy::permitted("claude", "any", "opus", "turbo", &[], None),
        Some("erro_orq_esforco_invalido")
    );
}
#[test]
fn or048_timing_freezes_and_reopens() {
    let events = vec![
        json!({"tipo":"execucao_inicio","ts":"2026-10-01T00:00:00+00:00"}),
        json!({"tipo":"task_inicio","task":1,"ts":"2026-10-01T00:01:00+00:00"}),
        json!({"tipo":"integrada","task":1,"ts":"2026-10-01T00:03:00+00:00"}),
    ];
    let aux = json!({"timing":panel::bounds(&events)});
    let out = panel::with_timing(json!({"tasks":{"rows":[{"n":1}]}}), &aux, 1790813400.);
    assert_eq!(out["tasks"]["rows"][0]["timing"]["elapsed_seconds"], 120);
    assert_eq!(out["timing"]["elapsed_seconds"], 600);
}
proptest::proptest! {
 #[test]fn public_orq_parsers_do_not_panic(s in ".{0,1200}") {let _=command("triage",&json!({"text":s}));let _=command("plan-tasks",&json!({"text":s}));let _=timeline::parse_obj(&json!({"text":s,"kind":"unknown","task":s}),None);}
}
