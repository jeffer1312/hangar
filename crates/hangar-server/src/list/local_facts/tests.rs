//! Os fatos lidos do disco contra o golden do Python (`backend/tests/fixtures/contract/list_facts/`).
use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use hangar_api::session::SessionRow;
use serde_json::{Value, json};

use super::*;
use crate::list::discover_other::Dirs;
use crate::list::local_transfers::{self, self as testing};

const HOME: &str = "⟦HOME⟧";

fn golden() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/list_facts/list_facts.json");
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn expand(v: &Value, home: &str) -> Value {
    match v {
        Value::String(s) => Value::String(s.replace(HOME, home)),
        Value::Array(a) => Value::Array(a.iter().map(|x| expand(x, home)).collect()),
        Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (k.clone(), expand(x, home))).collect()),
        other => other.clone(),
    }
}

/// Caminho do golden (POSIX) como o Python o grava no Windows: separador nativo.
fn native(path: &str) -> String { if cfg!(windows) { path.replace('/', "\\") } else { path.to_owned() } }

fn now() -> f64 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs_f64() }

#[test]
fn guest_access_matches_python() {
    for case in golden()["guests"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        if !case["file"].is_null() {
            std::fs::write(dir.path().join("guests.json"), case["file"].to_string()).unwrap();
        }
        let names: Vec<(String, Option<String>)> = case["names"].as_object().unwrap().iter()
            .map(|(n, l)| (n.clone(), l.as_str().map(str::to_owned))).collect();
        let (owners, hidden) = Guests::read(dir.path()).access(&names);
        let want: HashMap<String, String> = serde_json::from_value(case["owners"].clone()).unwrap();
        assert_eq!(owners, want, "{}", case["case"]);
        assert_eq!(serde_json::to_value(&hidden).unwrap(), case["hidden"], "{}", case["case"]);
    }
}

#[test]
fn orq_rows_match_python() {
    for case in golden()["orq"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join(".hangar").join("orq");
        let at = now();
        for run in case["runs"].as_array().unwrap() {
            let d = root.join(run["dir"].as_str().unwrap());
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("orq.json"), run["orq"].to_string()).unwrap();
            let events: String = run["events"].as_array().unwrap().iter().map(|e| format!("{e}\n")).collect();
            std::fs::write(d.join("eventos.jsonl"), events).unwrap();
            let mut hb = run["heartbeat"].clone();
            let ts = chrono::DateTime::from_timestamp_micros(((at + hb["ts"].as_f64().unwrap()) * 1e6) as i64).unwrap();
            hb["ts"] = json!(ts.with_timezone(&chrono::Local).to_rfc3339());
            std::fs::write(d.join("vigia.json"), hb.to_string()).unwrap();
            let tl = d.join(format!("timeline-{}.jsonl", run["dir"].as_str().unwrap()));
            std::fs::write(&tl, "{}\n").unwrap();
            let mtime = std::time::UNIX_EPOCH + std::time::Duration::from_secs_f64(at + run["timeline"].as_f64().unwrap());
            std::fs::File::options().write(true).open(&tl).unwrap().set_modified(mtime).unwrap();
        }
        let (got, _) = orq_rows(dir.path(), at);
        let home = dir.path().to_string_lossy();
        let want = case["rows"].as_array().unwrap();
        assert_eq!(got.len(), want.len(), "{}", case["case"]);
        for (row, want) in got.iter().zip(want) {
            let want = expand(want, &home);
            let jsonl = want["jsonl"].as_str().map(native);
            assert_eq!((row.name.as_str(), row.cwd.as_deref(), row.jsonl.as_deref(), row.pair_gid.as_deref(),
                row.orq_arbiter.as_deref(), row.state.as_str(), row.provider.as_str(), row.tracked),
                (want["name"].as_str().unwrap(), want["cwd"].as_str(), jsonl.as_deref(), want["pair_gid"].as_str(),
                want["orq_arbiter"].as_str(), want["state"].as_str().unwrap(), "orq", true), "{}", case["case"]);
            let last = row.last_activity.map(|l| l - at);
            assert!((last.unwrap() - want["last_activity"].as_f64().unwrap()).abs() < 0.01, "{}: {last:?}", case["case"]);
        }
    }
}

#[test]
fn thread_key_matches_python() {
    for case in golden()["thread_key"].as_array().unwrap() {
        let mut want = case["key"].as_str().unwrap().to_owned();
        // `Path.resolve()` do Python no Windows: o caminho POSIX ganha o drive atual e separador nativo.
        if cfg!(windows) {
            let posix = case["home"].as_str().unwrap().trim_end_matches('/');
            let drive = std::env::current_dir().unwrap().to_string_lossy()[..2].to_owned();
            want = want.replace(&format!("\"{posix}\""), &serde_json::to_string(&format!("{drive}{}", native(posix))).unwrap());
        }
        assert_eq!(testing::thread_key(case["home"].as_str().unwrap(), case["thread"].as_str().unwrap()), want);
    }
}

fn dirs(home: &Path) -> Dirs {
    Dirs { home: home.into(), claude: home.join(".claude"), codex_home: home.join(".codex"), pi_sessions: home.join("pi"),
        omp_config: home.join("omp"), omp_agent: home.join("omp/agent"), kimi_home: home.join("kimi") }
}

#[test]
fn transfers_match_python() {
    const FIELDS: [&str; 12] = ["name", "cwd", "jsonl", "provider", "headless", "engine", "codex_home", "conta", "lifecycle_id",
        "transfer_id", "transfer_phase", "problema"];
    for case in golden()["transfers"].as_array().unwrap() {
        let dir = tempfile::tempdir().unwrap();
        let home_path = std::fs::canonicalize(dir.path()).unwrap();
        let home = home_path.to_string_lossy().into_owned();
        // O HOME entra dentro de strings JSON: no Windows as contrabarras precisam de escape.
        let home_json = serde_json::to_string(&home).unwrap();
        let home_json = &home_json[1..home_json.len() - 1];
        for (rel, raw) in case["files"].as_object().unwrap() {
            let raw = raw.as_str().unwrap().replace(HOME, home_json);
            let mut path = home_path.join(rel);
            // O índice de thread leva o caminho do HOME no nome.
            if path.file_name().unwrap().to_string_lossy().starts_with("thread-") {
                let record = case["records"][0].clone();
                let dest = expand(&record["destination_meta"], &home);
                let key = testing::thread_key(dest["codex_home"].as_str().unwrap(), dest["thread_id"].as_str().unwrap());
                path.set_file_name(format!("thread-{}.json", testing::sha256_hex(&key)));
            }
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, raw).unwrap();
        }
        let rows: Vec<SessionRow> = case["rows"].as_array().unwrap().iter()
            .map(|r| serde_json::from_value(expand(r, &home)).unwrap()).collect();
        let got = local_transfers::transfers(&rows, &dirs(&home_path));
        if case["error"] == true {
            assert!(got.is_err(), "{}", case["case"]);
            continue;
        }
        let (touched, frozen) = got.unwrap();
        let touched: Vec<BTreeMap<&str, Value>> = touched.iter().map(|r| {
            let v = serde_json::to_value(r).unwrap();
            FIELDS.iter().map(|f| (*f, v.get(*f).cloned().unwrap_or(Value::Null))).collect()
        }).collect();
        let want: Vec<BTreeMap<&str, Value>> = case["touched"].as_array().unwrap().iter()
            .map(|r| FIELDS.iter().map(|f| {
                let v = expand(&r[*f], &home);
                // A conta passa por `resolve()`, que no Windows devolve o separador nativo e tira o `\\?\`
                // que o `canonicalize` do HOME deste teste põe; o resto é copiado.
                (*f, match (*f, v.as_str()) { ("conta", Some(s)) => Value::String(native(s).replacen(r"\\?\", "", 1)), _ => v })
            }).collect()).collect();
        assert_eq!(touched, want, "{}", case["case"]);
        let mut frozen: Vec<String> = frozen.into_iter().collect();
        frozen.sort();
        assert_eq!(serde_json::to_value(frozen).unwrap(), case["frozen"], "{}", case["case"]);
    }
}

/// `registry.list_with_state`, Codex sem thread: passos do lançador, menu e falha de abertura.
#[test]
fn codex_launch_reads_steps_menu_and_failure() {
    let row = || serde_json::from_value::<SessionRow>(json!({"name": "cx", "provider": "codex", "state": "idle"})).unwrap();
    let mut r = row();
    codex_launch(&mut r, "hangar-codex-tui: instalando\nhangar-codex-tui: abrindo\n");
    assert_eq!((r.state.as_str(), r.label.as_deref(), r.startup_steps.len()), ("working", Some("abrindo"), 2));
    let mut r = row();
    codex_launch(&mut r, "hangar-codex-tui: abrindo\nfalhou\n  Pressione Enter para fechar esta sessão.  \n");
    assert_eq!((r.state.as_str(), r.problema.as_deref(), r.label.as_deref()), ("awaiting_input", Some("codex_abertura_falhou"), Some("abrindo")));
    let mut r = row();
    codex_launch(&mut r, "nada aqui\n");
    assert_eq!((r.state.as_str(), r.label, r.startup_steps.len()), ("idle", None, 0));
}

/// A linha Codex: atividade pelo mtime do rollout, travada pelo prazo, sem radar de limite.
#[test]
fn codex_row_takes_activity_and_stall_from_the_rollout() {
    let dir = tempfile::tempdir().unwrap();
    let rollout = dir.path().join("r.jsonl");
    std::fs::write(&rollout, "").unwrap();
    let mtime = modified(&rollout).unwrap();
    let mut r = serde_json::from_value::<SessionRow>(json!({"name": "cx-row", "provider": "codex", "state": "working",
        "jsonl": rollout.to_string_lossy(), "limited": true, "limit_reset": "x"})).unwrap();
    codex_row(&mut r, 300.0, mtime + 301.0);
    assert_eq!((r.last_activity, r.stalled, r.limited, r.limit_reset.as_deref()), (Some(mtime), true, false, None));
    codex_row(&mut r, 300.0, mtime + 10.0);
    assert!(!r.stalled);
}

#[test]
fn stall_seconds_reads_the_runtime_config_first() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(stall_seconds(dir.path()), std::env::var("CP_STALL_SECONDS").ok().and_then(|v| v.parse().ok()).unwrap_or(STALL_DEFAULT));
    std::fs::write(dir.path().join("runtime-config.json"), r#"{"stall_seconds": 42}"#).unwrap();
    assert_eq!(stall_seconds(dir.path()), 42.0);
}

#[test]
fn unreadable_guests_file_keeps_the_last_good_reading() {
    let dir = tempfile::tempdir().unwrap();
    let file = json!({"guests": [{"id": "g", "name": "Ana", "token_hash": "h", "root": "/r", "owner_sees": false}],
        "sessions": {"s": {"guest": "g"}}});
    std::fs::write(dir.path().join("guests.json"), file.to_string()).unwrap();
    let names = [("s".to_owned(), None)];
    assert_eq!(Guests::read(dir.path()).access(&names).1, vec!["s"]);
    std::fs::write(dir.path().join("guests.json"), "{").unwrap();
    assert_eq!(Guests::read(dir.path()).access(&names).1, vec!["s"], "ilegível não mostra ao dono a sessão escondida");
    std::fs::remove_file(dir.path().join("guests.json")).unwrap();
    assert!(Guests::read(dir.path()).access(&names).1.is_empty(), "sem arquivo, ninguém é convidado");
}

#[test]
fn unreadable_orq_keeps_the_last_good_row_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().join(".hangar/orq/r1");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("orq.json"), json!({"auto": true, "repo": "/w"}).to_string()).unwrap();
    let start = json!({"tipo": "execucao_inicio", "gid": "g1"});
    std::fs::write(d.join("eventos.jsonl"), format!("{start}\n")).unwrap();
    let beat = || json!({"ts": chrono::Local::now().to_rfc3339(), "interval_s": 60}).to_string();
    std::fs::write(d.join("vigia.json"), beat()).unwrap();
    let names = |rows: &[SessionRow]| rows.iter().map(|r| r.name.clone()).collect::<Vec<_>>();
    let (rows, failed) = orq_rows(dir.path(), now());
    assert_eq!((names(&rows), failed), (vec!["g1-orq".to_owned()], false));
    // Batimento lido no meio da escrita: a orquestração não some por um tique.
    std::fs::write(d.join("vigia.json"), "{").unwrap();
    let (rows, failed) = orq_rows(dir.path(), now());
    assert_eq!((names(&rows), failed), (vec!["g1-orq".to_owned()], true));
    // Encerrada de verdade: sai, sem falha.
    std::fs::write(d.join("vigia.json"), beat()).unwrap();
    std::fs::write(d.join("eventos.jsonl"), format!("{start}\n{}\n", json!({"tipo": "execucao_fim"}))).unwrap();
    let (rows, failed) = orq_rows(dir.path(), now());
    assert_eq!((names(&rows), failed), (Vec::<String>::new(), false));
}

#[test]
fn headless_problem_is_the_first_sidecar_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = crate::state::parked::sidecar_path("hl", dir.path());
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, r#"{"problema": ["claude_saiu", "detalhe"]}"#).unwrap();
    assert_eq!(headless_problem(dir.path(), "hl").as_deref(), Some("claude_saiu"));
    std::fs::write(&path, r#"{"problema": null}"#).unwrap();
    assert_eq!(headless_problem(dir.path(), "hl"), None);
}
