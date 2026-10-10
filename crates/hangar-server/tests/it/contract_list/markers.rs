//! A lista de sessões repetida contra as entradas e saídas gravadas pelo Python
//! (`backend/tests/fixtures/contract/gen_list.py`).
use crate::common;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::golden;
use hangar_api::session::SessionRow;
use hangar_server::list::{facts_files, sig};
use serde_json::{Map, Value};

const ROOT: &str = "⟦ROOT⟧";
const ROOT_SAN: &str = "⟦ROOT_SAN⟧";

/// `registry.sanitize_cwd` para a pasta do caso (sempre absoluta, sem separador final).
fn sanitize(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn real(s: &str, root: &Path) -> String {
    let r = &root.to_str().unwrap().replace('\\', "/");
    s.replace(ROOT_SAN, &sanitize(r)).replace(ROOT, r)
}

/// Linha do golden com os campos omitidos (iguais ao padrão) de volta.
fn full_row(defaults: &Value, row: &Value) -> SessionRow {
    let mut obj: Map<String, Value> = defaults.as_object().unwrap().clone();
    obj.extend(row.as_object().unwrap().clone());
    serde_json::from_value(Value::Object(obj)).expect("linha do golden vira SessionRow")
}

fn set_mtime(path: &Path, epoch: f64) {
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs_f64(epoch);
    std::fs::File::options().write(true).open(path).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(t)).unwrap();
}

/// Aplica as operações `fs` de um tique (convenções do golden).
fn apply_fs(ops: &Value, root: &Path, at: f64) {
    for op in ops.as_array().into_iter().flatten() {
        let path = PathBuf::from(real(op["path"].as_str().unwrap(), root));
        match op["op"].as_str().unwrap() {
            "mkdir" => std::fs::create_dir_all(&path).unwrap(),
            "rm" if path.is_dir() => std::fs::remove_dir_all(&path).unwrap(),
            "rm" => std::fs::remove_file(&path).unwrap(),
            "write" => {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, real(op["text"].as_str().unwrap(), root)).unwrap();
                set_mtime(&path, op["mtime"].as_f64().unwrap_or(at));
            }
            "touch" => set_mtime(&path, op["mtime"].as_f64().unwrap()),
            other => panic!("operação desconhecida {other}"),
        }
    }
}

/// Repete os casos de decoração e confere o que os leitores de arquivo decidem em cada linha:
/// estado do marcador/registro nativo, pergunta aberta e statusline do sidecar.
#[test]
fn decorate_markers_cases() {
    let doc = golden("list_decorate.json");
    let defaults = &doc["defaults"];
    let wanted = ["markers_and_native_registry", "open_question_sidecar", "statusline_sidecar_age"];
    let mut seen = 0;
    let mut rows_checked = 0;
    for case in doc["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        if !wanted.contains(&name) {
            continue;
        }
        seen += 1;
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let dirs = [root.join("home/.claude"), root.join("home/.claude-alt")];
        for tick in case["ticks"].as_array().unwrap() {
            let at = tick["at"].as_f64().unwrap();
            apply_fs(&tick["fs"], root, at);
            let alive: HashSet<i64> =
                tick["procs"].as_array().unwrap().iter().map(|p| p["pid"].as_i64().unwrap()).collect();
            let hooks = facts_files::HookStates::load(&dirs);
            for raw in tick["expected"]["rows"].as_array().unwrap() {
                let row = full_row(defaults, raw);
                rows_checked += 1;
                let ctx = format!("{name} @{} {}", at - doc["t0"].as_f64().unwrap(), row.name);
                let sid = Path::new(row.jsonl.as_deref().unwrap()).file_stem().unwrap().to_str().unwrap().to_owned();
                let question = facts_files::open_question(Some(&sid), &dirs);
                match &question {
                    Some(q) => {
                        assert_eq!(row.question.as_deref(), Some(q.question.as_str()), "{ctx}");
                        assert_eq!(row.options.as_ref(), Some(&q.options), "{ctx}");
                    }
                    None => assert_eq!(row.question, None, "{ctx}"),
                }
                let marker = hooks.get_state(Some(&sid), |pid| alive.contains(&pid));
                let state = match (&question, &marker) {
                    (Some(_), _) => "awaiting_input",
                    (None, Some(m)) => m.state.as_str(),
                    (None, None) => panic!("{ctx}: caso sem marcador"),
                };
                assert_eq!(row.state, state, "{ctx}");
                match facts_files::published_status(Some(&sid), &dirs, at) {
                    Some(p) => {
                        assert_eq!(row.status_line.as_deref(), Some(p.line.as_str()), "{ctx}");
                        assert_eq!(row.model, p.model, "{ctx}");
                    }
                    // Sem sidecar valendo, a linha só pode ter vindo do pane.
                    None => if let Some(line) = &row.status_line {
                        let frames = tick["captures"][row.name.as_str()].as_array().cloned().unwrap_or_default();
                        assert!(frames.iter().any(|f| f.as_str().unwrap().contains(line.as_str())), "{ctx}");
                    },
                }
            }
        }
    }
    assert_eq!(seen, wanted.len(), "caso sumiu do golden");
    assert_eq!(rows_checked, 10, "linhas conferidas mudaram: regenerou o golden?");
}

/// `sse._list_sig` sobre as linhas gravadas sai byte a byte igual à do Python.
#[test]
fn list_sig_cases() {
    for file in ["list_decorate.json", "list_state.json"] {
        let doc = golden(file);
        let mut ticks = 0;
        for case in doc["cases"].as_array().unwrap() {
            for tick in case["ticks"].as_array().unwrap() {
                let want = tick["expected"]["sig"].as_str().expect("tique sem sig");
                let rows: Vec<SessionRow> = tick["expected"]["rows"].as_array().unwrap()
                    .iter().map(|r| full_row(&doc["defaults"], r)).collect();
                assert_eq!(sig::list_sig(&rows), want, "{file} {} @{}", case["name"], tick["at"]);
                ticks += 1;
            }
        }
        assert!(ticks > 5, "{file}: poucos tiques ({ticks})");
    }
}
