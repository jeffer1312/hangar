//! Lista de sessões contra as entradas e saídas gravadas pelo Python (gen_list.py).
use crate::common;

use std::fs;
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};

use common::golden;
use hangar_api::session::SessionRow;
use hangar_server::list::plan::PlanTracker;
use serde_json::{json, Value};

/// Troca os marcadores de raiz do golden pela pasta do caso.
fn expand(s: &str, root: &str) -> String {
    let san: String = root.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    s.replace("⟦ROOT_SAN⟧", &san).replace("⟦ROOT⟧", root)
}

fn set_mtime(path: &str, mtime: &Value) {
    if let Some(t) = mtime.as_f64() {
        let at = UNIX_EPOCH + Duration::from_secs_f64(t);
        fs::File::options().write(true).open(path).unwrap().set_modified(at).unwrap();
    }
}

fn apply_fs(ops: &Value, root: &str) {
    for op in ops.as_array().into_iter().flatten() {
        let path = expand(op["path"].as_str().unwrap(), root);
        match op["op"].as_str().unwrap() {
            "write" => {
                fs::create_dir_all(Path::new(&path).parent().unwrap()).unwrap();
                fs::write(&path, expand(op["text"].as_str().unwrap(), root)).unwrap();
                set_mtime(&path, &op["mtime"]);
            }
            "rm" => {
                let _ = fs::remove_file(&path);
            }
            "mkdir" => fs::create_dir_all(&path).unwrap(),
            "touch" => {
                if !Path::new(&path).exists() {
                    fs::write(&path, "").unwrap();
                }
                set_mtime(&path, &op["mtime"]);
            }
            other => panic!("op desconhecida: {other}"),
        }
    }
}

fn case(file: &str, name: &str) -> (Value, Value) {
    let g = golden(file);
    let c = g["cases"].as_array().unwrap().iter().find(|c| c["name"] == name).cloned();
    (c.unwrap_or_else(|| panic!("{file} sem o caso {name}")), g)
}

const PLAN_FIELDS: [&str; 8] = [
    "plan_name",
    "plan_task",
    "plan_task_total",
    "plan_done",
    "plan_total",
    "plan_complete",
    "plan_tasks",
    "plan_hidden",
];

#[test]
fn plan_cases() {
    let (case, g) = case("list_decorate.json", "plan_with_and_without_pin");
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_str().unwrap().to_owned();
    let mono_off = g["mono_off"].as_f64().unwrap();
    let mut tracker = PlanTracker::default();
    for (i, tick) in case["ticks"].as_array().unwrap().iter().enumerate() {
        apply_fs(&tick["fs"], &root);
        let at = tick["at"].as_f64().unwrap();
        for want in tick["expected"]["rows"].as_array().unwrap() {
            let cwd = want.get("cwd").and_then(Value::as_str).map(|c| expand(c, &root));
            let mut row: SessionRow =
                serde_json::from_value(json!({"name": want["name"], "cwd": cwd})).unwrap();
            tracker.decorate(&mut row, at, at - mono_off);
            let got = serde_json::to_value(&row).unwrap();
            for f in PLAN_FIELDS {
                let exp = want.get(f).unwrap_or(&g["defaults"][f]);
                assert_eq!(&got[f], exp, "tique {i}, {}: {f}", want["name"]);
            }
        }
    }
}
