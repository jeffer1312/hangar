//! Classificação da lista repetida tique a tique contra o Python (`gen_list.py`): estado pelo
//! marcador ou pelo pane, segunda captura do spinner, rebaixamento de `awaiting`, teto e validade
//! da statusline, radar de limite e sem terminal pelo runtime.
use crate::common;

use std::sync::Mutex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use common::golden;
use hangar_api::session::SessionRow;
use hangar_server::list::classify::{CaptureFailed, CaptureSource, Classifier, Effect, Facts};
use hangar_server::list::facts_files::HookStates;
use serde_json::{json, Map, Value};

const ROOT: &str = "⟦ROOT⟧";
const ROOT_SAN: &str = "⟦ROOT_SAN⟧";

fn sanitize(path: &str) -> String {
    path.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn real(s: &str, root: &Path) -> String {
    let r = &root.to_str().unwrap().replace('\\', "/");
    s.replace(ROOT_SAN, &sanitize(r)).replace(ROOT, r)
}

fn ph(s: &str, root: &Path) -> String {
    let r = &root.to_str().unwrap().replace('\\', "/");
    s.replace(r, ROOT).replace(&sanitize(r), ROOT_SAN)
}

fn set_mtime(path: &Path, epoch: f64) {
    let t = SystemTime::UNIX_EPOCH + Duration::from_secs_f64(epoch);
    std::fs::File::options().write(true).open(path).unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(t)).unwrap();
}

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

/// Quadros gravados por nome (o primeiro é consumido, o último se repete) e relógio do caso.
struct Fake {
    frames: Mutex<HashMap<String, Vec<String>>>,
    wall: Mutex<f64>,
    mono_off: f64,
}

impl CaptureSource for Fake {
    async fn capture(&self, name: &str) -> Result<String, CaptureFailed> {
        let mut frames = self.frames.lock().unwrap();
        let list = frames.entry(name.to_owned()).or_default();
        Ok(match list.len() {
            0 => String::new(),
            1 => list[0].clone(),
            _ => list.remove(0),
        })
    }
    async fn pause(&self, d: Duration) { *self.wall.lock().unwrap() += d.as_secs_f64(); }
    fn wall(&self) -> f64 { *self.wall.lock().unwrap() }
    fn mono(&self) -> f64 { self.wall() - self.mono_off }
}

/// Linha descoberta: a gravada com os campos que a classificação decide de volta ao padrão.
fn discovered(defaults: &Value, row: &Value) -> SessionRow {
    let mut obj: Map<String, Value> = defaults.as_object().unwrap().clone();
    obj.extend(row.as_object().unwrap().clone());
    for field in ["state", "label", "question", "options", "last_activity", "limited", "limit_reset",
                  "status_line", "stalled", "problema"] {
        obj.insert(field.into(), defaults[field].clone());
    }
    serde_json::from_value(Value::Object(obj)).unwrap()
}

fn round3(x: f64) -> f64 { (x * 1000.0).round() / 1000.0 }

fn aged(cache: &BTreeMap<String, (f64, Option<String>)>, mono: f64, root: &Path) -> Value {
    Value::Object(cache.iter().map(|(k, (t, v))| {
        (k.clone(), json!([round3(mono - t), v.as_deref().map(|s| ph(s, root))]))
    }).collect())
}

/// Só o que é das linhas Claude: Codex, Pi, omp e Kimi vêm dos fatos do Python.
fn only(v: &Value, names: &HashSet<String>) -> Value {
    Value::Object(v.as_object().unwrap().iter().filter(|(k, _)| names.contains(*k))
        .map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn run_case(doc: &Value, case: &Value) -> usize {
    let name = case["name"].as_str().unwrap();
    let defaults = &doc["defaults"];
    let t0 = doc["t0"].as_f64().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let dirs = [root.join("home/.claude"), root.join("home/.claude-alt")];
    let mut classifier = Classifier::default();
    let rt = tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap();
    let mut checked = 0;
    for tick in case["ticks"].as_array().unwrap() {
        let at = tick["at"].as_f64().unwrap();
        let ctx = format!("{name} @{}", at - t0);
        apply_fs(&tick["fs"], root, at);
        let alive: HashSet<i64> = tick["procs"].as_array().unwrap().iter().map(|p| p["pid"].as_i64().unwrap()).collect();
        let hooks = HookStates::load(&dirs);
        let mut headless = BTreeMap::new();
        for (n, snap) in tick["facts"]["headless_snapshot"].as_object().into_iter().flatten() {
            headless.insert(n.clone(), json!({"view": {"alive": true, "public_state": snap}, "error": null}));
        }
        let mut problems = BTreeMap::new();
        for key in ["runtime_problem", "headless_problem"] {
            for (n, p) in tick["facts"][key].as_object().into_iter().flatten() {
                problems.insert(n.clone(), p[0].as_str().unwrap().to_owned());
            }
        }
        let fake = Fake {
            frames: Mutex::new(tick["captures"].as_object().into_iter().flatten()
                .map(|(k, v)| (k.clone(), v.as_array().unwrap().iter().map(|f| real(f.as_str().unwrap(), root)).collect()))
                .collect()),
            wall: Mutex::new(at),
            mono_off: doc["mono_off"].as_f64().unwrap(),
        };
        let expected = &tick["expected"];
        let want: Vec<SessionRow> = expected["rows"].as_array().unwrap().iter()
            .map(|r| {
                let mut obj = defaults.as_object().unwrap().clone();
                obj.extend(serde_json::from_str::<Map<String, Value>>(&real(&r.to_string(), root)).unwrap());
                serde_json::from_value(Value::Object(obj)).unwrap()
            }).collect();
        let mut rows: Vec<SessionRow> = expected["rows"].as_array().unwrap().iter()
            .map(|r| discovered(defaults, &serde_json::from_str(&real(&r.to_string(), root)).unwrap()))
            .collect();
        let alive_fn = |pid: i64| alive.contains(&pid);
        let facts = Facts { hooks: &hooks, alive: &alive_fn, config_dirs: &dirs, headless: Some(&headless),
                            problems: &problems, stall_seconds: 300.0, held: &BTreeMap::new(),
                            monitors: &hangar_server::state::published::Published::default() };
        let effects = rt.block_on(classifier.classify(&mut rows, &facts, &fake));
        let claude: HashSet<String> = rows.iter().filter(|r| r.provider == "claude").map(|r| r.name.clone()).collect();
        for (got, want) in rows.iter().zip(&want) {
            if got.provider != "claude" {
                continue;
            }
            checked += 1;
            let ctx = format!("{ctx} {}", got.name);
            assert_eq!(got.state, want.state, "{ctx} state");
            assert_eq!(got.label, want.label, "{ctx} label");
            assert_eq!(got.question, want.question, "{ctx} question");
            assert_eq!(got.options, want.options, "{ctx} options");
            assert_eq!(got.last_activity, want.last_activity, "{ctx} last_activity");
            assert_eq!(got.limited, want.limited, "{ctx} limited");
            assert_eq!(got.limit_reset, want.limit_reset, "{ctx} limit_reset");
            assert_eq!(got.status_line, want.status_line, "{ctx} status_line");
            assert_eq!(got.stalled, want.stalled, "{ctx} stalled");
            assert_eq!(got.problema, want.problema, "{ctx} problema");
        }
        let mut got_effects: Vec<Value> = effects.iter()
            .map(|e| match e { Effect::DemoteAwaiting { sid, .. } => json!(["demote_awaiting", sid]) }).collect();
        got_effects.sort_by_key(|v| v.to_string());
        assert_eq!(Value::Array(got_effects), expected["effects"], "{ctx} effects");
        // Quem executa o efeito é o Python (`hook_state.demote_awaiting`): sidecar idle, ts mantido.
        for Effect::DemoteAwaiting { sid, .. } in &effects {
            let mut rewritten = false;
            for dir in &dirs {
                let f = dir.join(".hangar-state").join(format!("{sid}.json"));
                if let Ok(raw) = std::fs::read(&f) {
                    let ts = serde_json::from_slice::<Value>(&raw).unwrap()["ts"].clone();
                    std::fs::write(&f, json!({"state": "idle", "ts": ts}).to_string()).unwrap();
                    rewritten = true;
                }
            }
            assert!(rewritten, "{ctx}: rebaixamento sem marcador para regravar ({sid})");
        }
        let mono = fake.mono();
        let idle: Map<String, Value> = classifier.idle_checked().iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        assert_eq!(Value::Object(idle), only(&expected["idle_checked"], &claude), "{ctx} idle_checked");
        assert_eq!(aged(classifier.status_cache(), mono, root), only(&expected["status_cache"], &claude), "{ctx} status_cache");
        assert_eq!(aged(classifier.limit_cache(), mono, root), only(&expected["limit_cache"], &claude), "{ctx} limit_cache");
        let labels: Map<String, Value> = classifier.label_cache().iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        assert_eq!(Value::Object(labels), only(&expected["label_cache"], &claude), "{ctx} label_cache");
    }
    checked
}

/// Todos os casos de estado e os de decoração, que exercitam marcador, registro nativo, pergunta
/// aberta, statusline do sidecar, travada e sem terminal pelo mesmo caminho.
#[test]
fn state_sequences() {
    let mut rows = 0;
    let mut cases = 0;
    for file in ["list_state.json", "list_decorate.json"] {
        let doc = golden(file);
        for case in doc["cases"].as_array().unwrap() {
            rows += run_case(&doc, case);
            cases += 1;
        }
    }
    assert_eq!(cases, 12, "casos mudaram: regenerou o golden?");
    assert!(rows > 60, "poucas linhas Claude conferidas ({rows})");
}
