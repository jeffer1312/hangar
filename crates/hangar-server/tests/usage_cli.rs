use hangar_server::{
    stats,
    transcript::Provider,
    usage_cli::{self, SnapshotArgs},
};
use serde_json::{Value, json};
use std::{fs, io::Write, path::Path};

const CLAUDE: &str = "{\"type\": \"assistant\", \"message\": {\"id\": \"m1\", \"usage\": {\"input_tokens\": 10, \"output_tokens\": 7, \"cache_read_input_tokens\": 80, \"cache_creation_input_tokens\": 5}}}\n";
const PI: &str = "{\"type\": \"message\", \"message\": {\"role\": \"assistant\", \"usage\": {\"input\": 10, \"output\": 7, \"cacheRead\": 80, \"cacheWrite\": 5}}}\n";
const KIMI: &str = "{\"type\": \"usage.record\", \"usage\": {\"inputOther\": 10, \"output\": 7, \"inputCacheRead\": 80, \"inputCacheCreation\": 5}}\n";
const CODEX_START: &str = "{\"type\": \"event_msg\", \"payload\": {\"type\": \"token_count\", \"info\": {\"total_token_usage\": {\"input_tokens\": 100, \"cached_input_tokens\": 80, \"output_tokens\": 10}}}}\n";
const CODEX_END: &str = "{\"type\": \"event_msg\", \"payload\": {\"type\": \"token_count\", \"info\": {\"total_token_usage\": {\"input_tokens\": 150, \"cached_input_tokens\": 110, \"output_tokens\": 30}}}}\n";

fn args<'a>(provider: &'a str, path: &'a Path) -> SnapshotArgs<'a> {
    SnapshotArgs {
        provider,
        transcript: path,
        role: "revisor",
        session: "rev-t1",
        model: None,
        effort: None,
    }
}

fn snap(provider: &str, path: &Path) -> Value {
    let result = usage_cli::snapshot(args(provider, path));
    assert!(result.is_ok(), "snapshot {provider}: {result:?}");
    result.unwrap()
}

fn append(path: &Path, data: &str) {
    write!(
        fs::OpenOptions::new().append(true).open(path).unwrap(),
        "{data}"
    )
    .unwrap();
}

fn pair(path: &Path) -> (Value, Value) {
    fs::write(path, CODEX_START).unwrap();
    let start = snap("codex", path);
    append(path, &CODEX_END.repeat(2));
    let end = snap("codex", path);
    (start, end)
}

#[test]
fn usage_snapshot_keeps_the_recorded_cache_split_and_exact_schema_for_all_agents() {
    let dir = tempfile::tempdir().unwrap();
    for (provider, data, times, bytes, hash) in [
        (
            "claude",
            CLAUDE,
            2,
            326,
            "eafc72ed2e19a2bf89c827b93b46dfdf0a3471d2749bee4da630172a901beffc",
        ),
        (
            "pi",
            PI,
            1,
            125,
            "002fa4e89d85feaa71ec299b6864bb54098c7819110e8cc488dcb701a20be1d6",
        ),
        (
            "omp",
            PI,
            1,
            125,
            "002fa4e89d85feaa71ec299b6864bb54098c7819110e8cc488dcb701a20be1d6",
        ),
        (
            "kimi",
            KIMI,
            1,
            114,
            "f0de1140a5c6cb5aa5ecbdb427087414f9faa7b3abfc3f7773dfb47e6c0818fd",
        ),
    ] {
        let path = dir.path().join(format!("{provider}.jsonl"));
        fs::write(&path, "").unwrap();
        let empty = snap(provider, &path);
        assert_eq!(empty["coverage"], "sem_uso_observado");
        assert_eq!(
            empty["totals"],
            json!({"input":0,"output":0,"cache_read":0,"cache_write":0,"steps":0,"bytes":0})
        );
        append(&path, &data.repeat(times));
        let full = usage_cli::snapshot(SnapshotArgs {
            model: Some("modelo"),
            effort: Some("esforço"),
            ..args(provider, &path)
        })
        .unwrap();
        assert_eq!(
            full["totals"],
            json!({"input":10,"output":7,"cache_read":80,"cache_write":5,"steps":1,"bytes":bytes})
        );
        assert_eq!(full["sha256"], hash);
        assert_eq!(full["version"], 1);
        assert_eq!(full["provider"], provider);
        assert_eq!(full["role"], "revisor");
        assert_eq!(full["session"], "rev-t1");
        assert_eq!(full["model_observed"], "modelo");
        assert_eq!(full["effort_observed"], "esforço");
        assert_eq!(full["coverage"], "uso_observado");
        assert!(
            full["source"]["path"]
                .as_str()
                .unwrap()
                .ends_with(&format!("{provider}.jsonl"))
        );
        assert!(full["source"]["device"].is_number() && full["source"]["inode"].is_number());
        assert!(full["ts"].as_str().unwrap().ends_with("+00:00"));
        assert_eq!(full.as_object().unwrap().len(), 11);
    }
}

#[test]
fn usage_report_subtracts_the_codex_baseline_without_counting_repeated_totals() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout.jsonl");
    let (mut start, mut end) = pair(&path);
    start["ts"] = json!("2026-11-08T10:00:00.000000+00:00");
    end["ts"] = json!("2026-11-08T10:00:02.500000+00:00");
    assert_eq!(
        start["sha256"],
        "70be4f2d3465a459ce14d8b64f087e5cfd2c14a1304eecaeabd06a123e53e0ba"
    );
    assert_eq!(
        end["sha256"],
        "76a5ec563b06c03db7147d7fb082fa8a58147a4ef0e5ad9962feb0bc4208c9e1"
    );
    assert_eq!(
        end["totals"],
        json!({"input":40,"cache_read":110,"cache_write":0,"output":30,"steps":2,"bytes":482})
    );
    let result = usage_cli::report(&[(start.clone(), end.clone())]);
    assert!(result.is_ok(), "report: {result:?}");
    let result = result.unwrap();
    assert_eq!(
        result["by_role"],
        json!({"revisor":{"intervals":1,"window_seconds_sum":2.5,"tokens":{"input":20,"cache_read":30,"cache_write":0,"output":20}}})
    );
    assert_eq!(result["coverage"], "somente_fontes_registradas");
    assert_eq!(
        result["intervals"][0],
        json!({"role":"revisor","session":"rev-t1","provider":"codex","source":start["source"],"start":start["ts"],"end":end["ts"],"window_seconds":2.5,"start_bytes":160,"end_bytes":482,"tokens":{"input":20,"cache_read":30,"cache_write":0,"output":20},"model_observed":[null,null],"effort_observed":[null,null]})
    );
    assert_eq!(
        result["methodology"],
        json!([
            "Tokens = totais finais menos iniciais; entrada exclui cache lido e criado.",
            "Janela observada inclui espera; sua soma pode incluir trabalho em paralelo.",
            "Subagentes só entram com pares próprios; isto não presume o total da equipe.",
            "Modelo e esforço são observações declaradas, sem atribuição de tokens por modelo."
        ])
    );
    append(&path, CODEX_END);
    assert!(
        usage_cli::report(&[(start, end)]).is_ok(),
        "escrita posterior ao prefixo medido não o modifica"
    );
}

#[test]
fn usage_rejects_changed_sources_and_overlapping_or_incompatible_intervals() {
    let dir = tempfile::tempdir().unwrap();
    for change in ["reescrita", "truncada", "substituida", "ausente"] {
        let path = dir.path().join(format!("{change}.jsonl"));
        let (start, end) = pair(&path);
        match change {
            "reescrita" => fs::write(
                &path,
                fs::read_to_string(&path).unwrap().replacen("100", "101", 1),
            )
            .unwrap(),
            "truncada" => fs::write(&path, "").unwrap(),
            "substituida" => {
                let replacement = path.with_extension("replacement");
                fs::write(&replacement, fs::read(&path).unwrap()).unwrap();
                fs::rename(replacement, &path).unwrap();
            }
            _ => fs::remove_file(&path).unwrap(),
        }
        assert!(usage_cli::report(&[(start, end)]).is_err(), "{change}");
    }
    let path = dir.path().join("stable.jsonl");
    let (mut start, mut end) = pair(&path);
    start["ts"] = json!("2026-11-08T10:00:00+00:00");
    end["ts"] = json!("2026-11-08T10:00:01+00:00");
    let error = usage_cli::report(&[(start.clone(), end.clone()), (start.clone(), end.clone())])
        .unwrap_err();
    assert!(error.contains("sobrepostos"), "{error}");
    let (mut other_start, mut other_end) = (start.clone(), end.clone());
    other_start["provider"] = json!("pi");
    other_end["provider"] = json!("pi");
    assert!(
        usage_cli::report(&[(start.clone(), end.clone()), (other_start, other_end)])
            .unwrap_err()
            .contains("sobrepostos")
    );
    for (key, value) in [
        ("role", json!("executor")),
        ("session", json!("outro")),
        ("provider", json!("pi")),
    ] {
        let mut changed = end.clone();
        changed[key] = value;
        assert!(
            usage_cli::report(&[(start.clone(), changed)])
                .unwrap_err()
                .contains("vínculos diferentes")
        );
    }
    let mut regressed = end.clone();
    regressed["totals"]["output"] = json!(0);
    assert!(
        usage_cli::report(&[(start, regressed)])
            .unwrap_err()
            .contains("regrediram")
    );
}

#[test]
fn usage_exact_validation_does_not_turn_partial_invalid_or_reset_input_into_a_measurement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rollout.jsonl");
    for invalid in ["{quebrado}", "null", "[]"] {
        fs::write(&path, format!("{CODEX_START}{invalid}\n{CODEX_END}")).unwrap();
        let lease = stats::acquire(Provider::Codex, &path);
        let mut acc = lease.acc.lock().unwrap();
        assert_eq!(
            acc.collect().unwrap().unwrap()["out_tok"],
            30,
            "o SSE conserva sua coleta tolerante"
        );
        let error = acc.usage_totals().unwrap_err().to_string();
        assert!(error.contains("linhas inválidas"), "{error}");
        drop(acc);
        drop(lease);
        assert!(
            usage_cli::snapshot(args("codex", &path))
                .unwrap_err()
                .contains("linhas inválidas")
        );
    }
    let reset = CODEX_START
        .replace("100", "50")
        .replace("80", "40")
        .replace("10}", "5}");
    fs::write(&path, format!("{CODEX_START}{reset}{CODEX_END}")).unwrap();
    assert!(
        usage_cli::snapshot(args("codex", &path))
            .unwrap_err()
            .contains("regrediram")
    );
    fs::write(&path, CODEX_START).unwrap();
    append(&path, "{\"type\":\"event_msg\"");
    assert!(
        usage_cli::snapshot(args("codex", &path))
            .unwrap_err()
            .contains("linha parcial")
    );
    let empty = dir.path().join("empty.jsonl");
    fs::write(&empty, "").unwrap();
    let mut a = snap("codex", &empty);
    let mut b = a.clone();
    a["ts"] = json!("2026-11-08T10:00:00+00:00");
    b["ts"] = json!("2026-11-08T10:00:01+00:00");
    assert!(
        usage_cli::report(&[(a, b)])
            .unwrap_err()
            .contains("Sem uso novo")
    );
}

#[test]
fn usage_cli_prints_json_on_success_and_a_visible_error_on_missing_input() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("pi.jsonl");
    fs::write(&path, PI).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"))
        .args(["usage", "snapshot", "--provider", "pi", "--transcript"])
        .arg(&path)
        .args(["--role", "executor", "--session", "p1"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        serde_json::from_slice::<Value>(&output.stdout).unwrap()["totals"]["cache_write"],
        5
    );
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_hangar-server"))
        .args(["usage", "snapshot", "--provider", "pi", "--transcript"])
        .arg(dir.path().join("missing"))
        .args(["--role", "executor", "--session", "p1"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("Consumo não medido"));
}

proptest::proptest! {
    #![proptest_config(proptest::test_runner::Config { cases: 24, failure_persistence: None, rng_seed: proptest::test_runner::RngSeed::Fixed(741305), ..Default::default() })]
    #[test]
    fn usage_external_snapshot_values_never_panic(text in ".{0,192}") {
        let value=serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
        let _=usage_cli::report(&[(value.clone(),value)]);
    }
}
