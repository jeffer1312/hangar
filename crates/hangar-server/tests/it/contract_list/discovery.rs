//! A descoberta do Rust repete, tique a tique, as entradas gravadas por `gen_list.py`.
#![cfg(target_os = "linux")]

use crate::common;

use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use hangar_server::list::discover::{discover_panes, Resolver};
use hangar_server::list::mux::Pane;
use hangar_server::list::procs::{ChildrenMap, ProcessView, CHILDREN_TTL};
use hangar_workspace::worktrees::sanitize_cwd;
use serde_json::{json, Value};

const ROOT: &str = "⟦ROOT⟧";
const SAN: &str = "⟦ROOT_SAN⟧";

struct Case { root: PathBuf, san: String }

impl Case {
    fn real(&self, s: &str) -> String {
        s.replace(SAN, &self.san).replace(ROOT, self.root.to_str().unwrap())
    }
    fn ph(&self, s: &str) -> String {
        s.replace(self.root.to_str().unwrap(), ROOT).replace(&self.san, SAN)
    }
}

/// O /proc do tique, como o `World` do gerador.
#[derive(Default)]
struct FakeProcs { procs: HashMap<i64, Value> }

impl FakeProcs {
    fn get(&self, pid: i64) -> Option<&Value> { self.procs.get(&pid) }
}

impl ProcessView for FakeProcs {
    fn children(&self, _: Duration) -> io::Result<Arc<ChildrenMap>> { unreachable!("o teste monta o mapa") }
    fn argv(&self, pid: i64) -> Vec<String> {
        self.get(pid).map(|p| p["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_string()).collect())
            .unwrap_or_default()
    }
    fn cwd(&self, pid: i64) -> Option<PathBuf> { self.get(pid).map(|p| PathBuf::from(p["cwd"].as_str().unwrap())) }
    fn env_var(&self, pid: i64, name: &str) -> io::Result<Option<OsString>> {
        Ok(self.get(pid).and_then(|p| p["env"][name].as_str()).filter(|v| !v.is_empty()).map(OsString::from))
    }
    fn start_time(&self, pid: i64) -> Option<f64> { self.get(pid).and_then(|p| p["start"].as_f64()) }
    fn fds(&self, pid: i64) -> Vec<PathBuf> {
        self.get(pid).map(|p| p["fds"].as_array().unwrap().iter().map(|f| PathBuf::from(f.as_str().unwrap())).collect())
            .unwrap_or_default()
    }
}

fn set_mtime(path: &Path, secs: f64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(UNIX_EPOCH + Duration::from_secs_f64(secs)).unwrap();
}

fn apply_fs(case: &Case, ops: &[Value], at: f64) {
    for op in ops {
        let path = PathBuf::from(case.real(op["path"].as_str().unwrap()));
        match op["op"].as_str().unwrap() {
            "mkdir" => std::fs::create_dir_all(&path).unwrap(),
            "rm" if path.is_dir() => std::fs::remove_dir_all(&path).unwrap(),
            "rm" => std::fs::remove_file(&path).unwrap(),
            "write" => {
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, case.real(op["text"].as_str().unwrap())).unwrap();
                set_mtime(&path, op["mtime"].as_f64().unwrap_or(at));
            }
            "touch" => set_mtime(&path, op["mtime"].as_f64().unwrap()),
            other => panic!("op de arquivo desconhecida: {other}"),
        }
    }
}

fn pane(case: &Case, v: &Value) -> Pane {
    Pane {
        session: v["name"].as_str().unwrap().into(),
        active: v["active"].as_bool().unwrap(),
        pid: v["pid"].as_u64().map(|p| u32::try_from(p).unwrap()),
        cwd: case.real(v["cwd"].as_str().unwrap()),
        pane_id: v["pane_id"].as_str().unwrap().into(),
        hidden: v["hidden"].as_bool().unwrap(),
        provider: v["provider"].as_str().map(String::from),
        session_created: v["session_created"].as_u64(),
        ..Pane::default()
    }
}

/// Mapa pai→filhos em ordem crescente de pid, como o fake do gerador.
fn scan(procs: &FakeProcs) -> ChildrenMap {
    let mut pids: Vec<_> = procs.procs.keys().copied().collect();
    pids.sort();
    let mut map = ChildrenMap::new();
    for pid in pids {
        map.entry(procs.procs[&pid]["ppid"].as_i64().unwrap()).or_default().push(pid);
    }
    map
}

/// Só a parte Claude da linha: as linhas dos demais provedores e os campos comuns são da Task 7.
/// `expected.rows` omite o campo igual ao de `defaults`.
fn claude_rows(rows: &[Value], defaults: &Value) -> Vec<Value> {
    let field = |r: &Value, k: &str| r.get(k).unwrap_or(&defaults[k]).clone();
    rows.iter().filter(|r| field(r, "provider") == json!("claude") && field(r, "headless") == json!(false))
        .map(|r| json!({"name": r["name"], "cwd": field(r, "cwd"), "jsonl": field(r, "jsonl"),
                        "tracked": field(r, "tracked")}))
        .collect()
}

#[test]
fn discovery_claude_sequences() {
    let golden = common::golden("list_discovery.json");
    let (mut failures, mut compared) = (Vec::new(), 0);
    for c in golden["cases"].as_array().unwrap() {
        let name = c["name"].as_str().unwrap();
        // A guarda de colisão rebaixa linhas depois da resolução (Task 7); o cache segue comparado.
        let compare_rows = name != "collision";
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let case = Case { san: sanitize_cwd(root.to_str().unwrap()), root };
        let projects = PathBuf::from(case.real(&format!("{ROOT}/home/.claude/projects")));
        let mut resolver = Resolver::default();
        let mut children: Option<(f64, Arc<ChildrenMap>)> = None;
        for (index, t) in c["ticks"].as_array().unwrap().iter().enumerate() {
            let at = t["at"].as_f64().unwrap();
            let expected = &t["expected"];
            apply_fs(&case, t["fs"].as_array().map_or(&[][..], |v| v), at);
            let procs = FakeProcs { procs: t["procs"].as_array().unwrap().iter().map(|p| {
                let p: Value = serde_json::from_str(&case.real(&p.to_string())).unwrap();
                (p["pid"].as_i64().unwrap(), p)
            }).collect() };
            for op in t["ops"].as_array().map_or(&[][..], |v| v) {
                match op["op"].as_str().unwrap() {
                    "seed" => resolver.seed(op["name"].as_str().unwrap(), &case.real(op["jsonl"].as_str().unwrap())),
                    "forget" => resolver.forget(op["name"].as_str().unwrap()),
                    "rename" => resolver.rename(op["old"].as_str().unwrap(), op["new"].as_str().unwrap()),
                    "fresh" => children = None,
                    other => panic!("op desconhecida: {other}"),
                }
            }
            if t["tmux_down"] == json!(true) {
                assert_eq!(expected["raises"], json!("MuxIndisponivel"), "{name}#{index}");
                continue;
            }
            if children.as_ref().is_none_or(|(when, _)| at - when >= CHILDREN_TTL.as_secs_f64()) {
                children = Some((at, Arc::new(scan(&procs))));
            }
            let panes: Vec<Pane> = t["panes"].as_array().unwrap().iter().map(|p| pane(&case, p)).collect();
            // O sidecar Codex de mesmo nome tira o pane da lista (a leitura dele é da Task 7).
            let sidecars = PathBuf::from(case.real(&format!("{ROOT}/home/.hangar/codex-sessions")));
            let skip = |n: &str| sidecars.join(format!("{n}.json")).exists();
            let found = discover_panes(&panes, &procs, &children.as_ref().unwrap().1, &projects, &mut resolver, &skip);
            let rows: Vec<Value> = found.iter().filter(|s| s.provider == "claude").map(|s| {
                let t = s.transcript.as_ref().expect("linha Claude sempre resolve");
                json!({"name": s.name, "cwd": case.ph(&s.cwd), "jsonl": t.jsonl.as_deref().map(|j| case.ph(j)),
                       "tracked": t.tracked})
            }).collect();
            let cache: serde_json::Map<String, Value> = resolver.cached().iter()
                .map(|(k, v)| (k.clone(), json!(case.ph(v)))).collect();
            let locked: Vec<&String> = resolver.fd_locked().iter().collect();
            let actual = json!({"rows": if compare_rows { json!(rows) } else { Value::Null },
                                "jsonl_cache": cache, "fd_locked": locked});
            let want = json!({"rows": if compare_rows { json!(claude_rows(expected["rows"].as_array().unwrap(), &golden["defaults"])) } else { Value::Null },
                              "jsonl_cache": expected["jsonl_cache"], "fd_locked": expected["fd_locked"]});
            compared += rows.len();
            if common::canon(&actual) != common::canon(&want) {
                failures.push(format!("{name}#{index}:\n  Rust   {actual}\n  Python {want}"));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert!(compared >= 25, "só {compared} linhas Claude comparadas: o golden mudou de forma?");
}
