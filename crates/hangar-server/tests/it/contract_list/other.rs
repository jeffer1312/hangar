//! A lista inteira do Rust (descoberta da Task 6 + provedores, sidecars, campos comuns e colisão da
//! Task 7) contra as linhas gravadas por `gen_list.py`, tique a tique.
#![cfg(target_os = "linux")]

use crate::common;

use hangar_api::session::SessionRow;
use hangar_server::list::discover::Resolver;
use hangar_server::list::discover_other::{self, Dirs};
use hangar_server::list::links;
use hangar_server::list::mux::Pane;
use hangar_server::list::procs::{CHILDREN_TTL, ChildrenMap, ProcessView};
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

const ROOT: &str = "⟦ROOT⟧";
const SAN: &str = "⟦ROOT_SAN⟧";

/// A pasta de um caso: os marcadores do golden viram caminhos de verdade nela.
struct World {
    _dir: tempfile::TempDir,
    root: String,
}

impl World {
    fn new() -> Self {
        // Sem ponto no nome: a pasta do Pi troca só separador, o `⟦ROOT_SAN⟧` troca todo símbolo.
        let dir = tempfile::Builder::new().prefix("hangarlist").tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap().to_string_lossy().into_owned();
        assert!(!root.contains('.'), "{root}");
        Self { _dir: dir, root }
    }

    fn real_str(&self, s: &str) -> String {
        s.replace(SAN, &hangar_workspace::worktrees::sanitize_cwd(&self.root)).replace(ROOT, &self.root)
    }

    fn real(&self, v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(self.real_str(s)),
            Value::Array(a) => Value::Array(a.iter().map(|x| self.real(x)).collect()),
            Value::Object(o) => Value::Object(o.iter().map(|(k, x)| (self.real_str(k), self.real(x))).collect()),
            other => other.clone(),
        }
    }

    fn dirs(&self) -> Dirs {
        let home = PathBuf::from(&self.root).join("home");
        Dirs {
            claude: home.join(".claude"),
            codex_home: home.join(".codex"),
            pi_sessions: home.join(".pi/agent/sessions"),
            omp_config: home.join(".omp"),
            omp_agent: home.join(".omp/agent"),
            kimi_home: home.join(".kimi-code"),
            home,
        }
    }

    fn apply_fs(&self, ops: &[Value], at: f64) {
        for op in ops {
            let path = PathBuf::from(self.real_str(op["path"].as_str().unwrap()));
            match op["op"].as_str().unwrap() {
                "mkdir" => std::fs::create_dir_all(&path).unwrap(),
                "rm" if path.is_dir() => std::fs::remove_dir_all(&path).unwrap(),
                "rm" => std::fs::remove_file(&path).unwrap(),
                "write" => {
                    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                    std::fs::write(&path, self.real_str(op["text"].as_str().unwrap())).unwrap();
                    set_mtime(&path, op["mtime"].as_f64().unwrap_or(at));
                }
                "touch" => set_mtime(&path, op["mtime"].as_f64().unwrap()),
                other => panic!("op {other}"),
            }
        }
    }
}

fn set_mtime(path: &Path, at: f64) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs_f64(at)).unwrap();
}

/// O `/proc` do tique; pid ausente responde como processo morto.
struct FakeProcs(HashMap<i64, Value>);

impl ProcessView for FakeProcs {
    fn children(&self, _: Duration) -> io::Result<Arc<ChildrenMap>> { unreachable!("o teste monta o mapa") }
    fn argv(&self, pid: i64) -> Vec<String> {
        self.0.get(&pid).map(|p| p["argv"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect()).unwrap_or_default()
    }
    fn cwd(&self, pid: i64) -> Option<PathBuf> { self.0.get(&pid).map(|p| PathBuf::from(p["cwd"].as_str().unwrap())) }
    fn env_var(&self, pid: i64, name: &str) -> io::Result<Option<OsString>> {
        let p = self.0.get(&pid).ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?;
        Ok(p["env"].get(name).and_then(Value::as_str).map(OsString::from))
    }
    fn start_time(&self, pid: i64) -> Option<f64> { self.0.get(&pid).and_then(|p| p["start"].as_f64()) }
    fn fds(&self, pid: i64) -> Vec<PathBuf> {
        self.0.get(&pid).map(|p| p["fds"].as_array().unwrap().iter().map(|f| PathBuf::from(f.as_str().unwrap())).collect()).unwrap_or_default()
    }
}

/// Mapa pai→filhos em ordem crescente de pid, como o fake do gerador.
fn scan(procs: &FakeProcs) -> ChildrenMap {
    let mut pids: Vec<i64> = procs.0.keys().copied().collect();
    pids.sort();
    let mut map = ChildrenMap::new();
    for pid in pids {
        map.entry(procs.0[&pid]["ppid"].as_i64().unwrap()).or_default().push(pid);
    }
    map
}

fn pane_of(p: &Value) -> Pane {
    Pane {
        session: p["name"].as_str().unwrap().to_owned(),
        active: p["active"].as_bool().unwrap(),
        pid: p["pid"].as_u64().map(|v| u32::try_from(v).unwrap()),
        cwd: p["cwd"].as_str().unwrap().to_owned(),
        pane_id: p["pane_id"].as_str().unwrap().to_owned(),
        hidden: p["hidden"].as_bool().unwrap(),
        provider: p["provider"].as_str().map(str::to_owned),
        session_created: p["session_created"].as_u64(),
        ..Pane::default()
    }
}

/// A linha como o golden grava: campo igual ao padrão sai. O golden vem do Python, que não adormece.
fn dump(row: &SessionRow, defaults: &Map<String, Value>) -> Value {
    let Value::Object(mut map) = serde_json::to_value(row).unwrap() else { unreachable!() };
    map.retain(|k, v| defaults.get(k) != Some(v) && !matches!(k.as_str(), "asleep" | "wake_surface"));
    Value::Object(map)
}

/// Repete cada caso tique a tique, com o mapa de filhos em cache de 3 s e as operações do
/// registro, e devolve as divergências.
fn replay(golden: &Value, cases: &[&str]) -> Vec<String> {
    let defaults = golden["defaults"].as_object().unwrap();
    let t0 = golden["t0"].as_f64().unwrap();
    let mut failures = Vec::new();
    for name in cases {
        let case = golden["cases"].as_array().unwrap().iter().find(|c| c["name"] == *name).unwrap_or_else(|| panic!("caso {name}"));
        let world = World::new();
        let dirs = world.dirs();
        let mut resolver = Resolver::default();
        let mut children: Option<(f64, Arc<ChildrenMap>)> = None;
        for tick in case["ticks"].as_array().unwrap() {
            let at = tick["at"].as_f64().unwrap();
            world.apply_fs(tick["fs"].as_array().map_or(&[][..], Vec::as_slice), at);
            let procs = FakeProcs(tick["procs"].as_array().unwrap().iter().map(|p| (p["pid"].as_i64().unwrap(), world.real(p))).collect());
            for op in tick["ops"].as_array().map_or(&[][..], Vec::as_slice) {
                match op["op"].as_str().unwrap() {
                    "seed" => resolver.seed(op["name"].as_str().unwrap(), &world.real_str(op["jsonl"].as_str().unwrap())),
                    "forget" => resolver.forget(op["name"].as_str().unwrap()),
                    "rename" => resolver.rename(op["old"].as_str().unwrap(), op["new"].as_str().unwrap()),
                    "fresh" => children = None,
                    other => panic!("op {other}"),
                }
            }
            if tick["expected"].get("raises").is_some() {
                continue;
            }
            if children.as_ref().is_none_or(|(when, _)| at - when >= CHILDREN_TTL.as_secs_f64()) {
                children = Some((at, Arc::new(scan(&procs))));
            }
            let panes: Vec<Pane> = tick["panes"].as_array().unwrap().iter().map(|p| pane_of(&world.real(p))).collect();
            let found = discover_other::discover_rows(&panes, &procs, &children.as_ref().unwrap().1, &mut resolver, &dirs);
            let rows = found.rows;
            // Pid do agente só de linha com terminal, e só o que o argv identificou.
            for (n, pid) in &found.agent_pids {
                assert!(rows.iter().any(|r| &r.name == n && !r.headless), "{name}: pid de linha sem terminal {n}");
                assert!(procs.0.contains_key(&i64::from(*pid)), "{name}: pid {pid} fora do tique");
            }
            let got: Vec<Value> = rows.iter().map(|r| dump(r, defaults)).collect();
            let expected: Vec<Value> = tick["expected"]["rows"].as_array().unwrap().iter().map(|r| world.real(r)).collect();
            if got != expected {
                failures.push(format!("{name} @{}:\n  Rust   {}\n  Python {}", at - t0, json!(got), json!(expected)));
            }
        }
    }
    failures
}

#[test]
fn discovery_other_sequences() {
    let golden = common::golden("list_discovery.json");
    let failures = replay(&golden, &["codex_with_and_without_thread", "claude_headless"]);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn links_cases() {
    let golden = common::golden("list_discovery.json");
    // As linhas Claude: vida, worktree, vínculos, motor, conta e a guarda de colisão.
    let failures = replay(&golden, &[
        "links_worktree_engine_account", "collision", "claude_fd_open_then_locked", "claude_fd_aux_ignored",
        "claude_session_id_then_clear", "claude_session_id_with_sibling", "claude_marker_by_pid_cache_newest",
        "claude_marker_by_session_id", "seed_rename_forget", "created_under_one_second", "mux_unavailable",
        "agent_pane_choice",
    ]);
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let all: Vec<&str> = golden["cases"].as_array().unwrap().iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(all.len(), 16, "caso novo no golden: inclua na cobertura de descoberta");

    // Loop pelo sidecar, nas linhas da decoração.
    let golden = common::golden("list_decorate.json");
    let case = golden["cases"].as_array().unwrap().iter().find(|c| c["name"] == "loop_stalled_last_reply_headless").unwrap();
    let world = World::new();
    let tick = &case["ticks"][0];
    world.apply_fs(tick["fs"].as_array().unwrap(), tick["at"].as_f64().unwrap());
    let rows = tick["expected"]["rows"].as_array().unwrap();
    assert!(rows.iter().any(|r| r.get("loop_status").is_some()), "o caso tem loop");
    for expected in rows {
        let mut row = links::blank_row(expected["name"].as_str().unwrap());
        links::fill_loop(&mut row, &world.dirs());
        for field in ["loop_status", "loop_iter", "loop_max"] {
            assert_eq!(serde_json::to_value(&row).unwrap()[field], expected.get(field).cloned().unwrap_or(Value::Null), "{} {field}", row.name);
        }
    }

    // Colisão: a dona pelo sid fica; sem ela, a única tracked; duas tracked, ninguém.
    let row = |name: &str, jsonl: &str, tracked: bool| {
        let mut r = links::blank_row(name);
        r.jsonl = Some(jsonl.to_owned());
        r.tracked = tracked;
        r
    };
    let sids = HashMap::from([("dona".to_owned(), Some("abc".to_owned())), ("b".to_owned(), None)]);
    let mut rows = vec![row("dona", "/x/abc.jsonl", false), row("b", "/x/abc.jsonl", true), row("so", "/x/s.jsonl", false)];
    links::dedupe_collisions(&mut rows, &sids);
    assert_eq!((rows[0].jsonl.as_deref(), rows[1].jsonl.as_deref(), rows[2].jsonl.as_deref()), (Some("/x/abc.jsonl"), None, Some("/x/s.jsonl")));
    assert!(!rows[1].tracked);
    let mut rows = vec![row("a", "/x/m.jsonl", false), row("b", "/x/m.jsonl", true)];
    links::dedupe_collisions(&mut rows, &HashMap::new());
    assert_eq!((rows[0].jsonl.as_deref(), rows[1].jsonl.as_deref()), (None, Some("/x/m.jsonl")));
    let mut rows = vec![row("a", "/x/m.jsonl", true), row("b", "/x/m.jsonl", true)];
    links::dedupe_collisions(&mut rows, &HashMap::new());
    assert!(rows.iter().all(|r| r.jsonl.is_none() && !r.tracked));
}

#[test]
fn ticket_key_and_pi_paths() {
    let procs = FakeProcs(HashMap::from([(7, json!({"ppid": 1, "argv": ["omp"], "cwd": "/", "env": {"PSMUX_SESSION": "omp s/1"}, "start": 0.0, "fds": []}))]));
    assert_eq!(discover_other::ticket_key("%%3", None, &procs), "3");
    assert_eq!(discover_other::ticket_key("%3", Some(7), &procs), "omp-s-1");
    assert!(discover_other::is_pi_subagent("/s/x/2027-01-15T08-00-00-000Z_88888888-8888-4888-8888-888888888888/44bad0fb/a.jsonl"));
    assert!(discover_other::is_pi_subagent("/s/x/run-2/a.jsonl"));
    assert!(!discover_other::is_pi_subagent("/s/x/2027-01-15T08-00-00-000Z_88888888-8888-4888-8888-888888888888.jsonl"));
    // Valores calculados pelo `kimi_sessions.workdir_key` e pelo `sanitize_session_name` do Python.
    assert_eq!(discover_other::kimi_workdir_key("/tmp/kimi-acp-probe"), "wd_kimi-acp-probe_15ca61fc9ec9");
    assert_eq!(discover_other::sanitize_session_name("Área de trabalho."), "Area-de-trabalho");
}

/// Padrões do contrato original, escritos aqui para não depender da captura de saída.
fn native_row_defaults() -> Value {
    json!({
        "lifecycle_id": null, "transfer_id": null, "transfer_phase": null,
        "cwd": null, "jsonl": null, "provider": "claude", "headless": false,
        "engine": null, "engine_account": null, "codex_home": null, "codex_service_tier": null,
        "conta": null, "state": "idle", "last_activity": null, "last_reply": null,
        "last_reply_at": null, "tracked": true, "branch": null, "worktree": false,
        "worktree_path": null, "worktree_gone": false, "git_cwd": null, "git_dirty": null,
        "git_ahead": null, "git_behind": null, "git_added": null, "git_removed": null,
        "avisos": [], "label": null, "startup_steps": [], "question": null,
        "pending_questions": 0, "options": null, "stalled": false, "problema": null,
        "limited": false, "limit_reset": null, "shared": false, "guest_kind": null,
        "owner": null, "then_target": null, "status_line": null, "context": null,
        "model": null, "pair_peers": null, "pair_external": null, "pair_gid": null,
        "pair_task": null, "orq_arbiter": null, "loop_status": null, "loop_iter": null,
        "loop_max": null, "plan_name": null, "plan_task": null, "plan_task_total": null,
        "plan_done": null, "plan_total": null, "plan_complete": null,
        "plan_tasks": null, "plan_hidden": null
    })
}

#[test]
fn native_ticket_sequences_have_literal_rows() {
    // Só as entradas do contrato medido; nenhum resultado esperado acompanha a fixture.
    let cases: Value = serde_json::from_str(include_str!("../fixtures/discovery/native_inputs.json")).unwrap();
    assert_eq!(cases.as_array().unwrap().iter().map(|case| case["name"].as_str().unwrap()).collect::<Vec<_>>(), ["pi_ticket", "kimi_ticket"]);
    let defaults = native_row_defaults();
    let root_pi = "⟦ROOT⟧/home/.pi/agent/sessions/-⟦ROOT_SAN⟧-work-g--/2027-01-15T08-00-00-000Z_88888888-8888-4888-8888-888888888888.jsonl";
    for case in cases.as_array().unwrap() {
        let expected = match case["name"].as_str().unwrap() {
            "pi_ticket" => vec![
                ("pi", "pi", "g", Some("⟦ROOT⟧/work/g/declarado.jsonl")),
                ("pi", "pi", "g", Some(root_pi)),
                ("pi", "pi", "g", Some(root_pi)),
                ("pi", "pi", "g", Some(root_pi)),
                ("pi", "pi", "g", None),
                ("omp", "omp", "g", Some("⟦ROOT⟧/work/g/omp.jsonl")),
            ],
            "kimi_ticket" => vec![
                ("kimi", "kimi", "k", Some("⟦ROOT⟧/home/.kimi-code/sessions/wd/kimi-sess-1/agents/main/wire.jsonl")),
                ("kimi", "kimi", "k", None),
                ("kimi", "kimi", "k", None),
            ],
            name => panic!("entrada sem expectativa literal: {name}"),
        };
        let world = World::new();
        let mut resolver = Resolver::default();
        let ticks = case["ticks"].as_array().unwrap();
        assert_eq!(ticks.len(), expected.len());
        for (tick, (name, provider, cwd, transcript)) in ticks.iter().zip(expected) {
            world.apply_fs(tick["fs"].as_array().map_or(&[], Vec::as_slice), tick["at"].as_f64().unwrap());
            let procs = FakeProcs(tick["procs"].as_array().unwrap().iter()
                .map(|p| (p["pid"].as_i64().unwrap(), world.real(p))).collect());
            let panes = tick["panes"].as_array().unwrap().iter().map(|p| pane_of(&world.real(p))).collect::<Vec<_>>();
            let found = discover_other::discover_rows(&panes, &procs, &scan(&procs), &mut resolver, &world.dirs());
            let mut row = json!({"name": name, "provider": provider,
                "lifecycle_id": "t:1799999400", "cwd": format!("⟦ROOT⟧/work/{cwd}")});
            if let Some(path) = transcript { row["jsonl"] = json!(path); }
            else { row["tracked"] = json!(false); }
            assert_eq!(found.rows.iter().map(|r| dump(r, defaults.as_object().unwrap())).collect::<Vec<_>>(),
                vec![world.real(&row)], "{} @{}", case["name"], tick["at"]);
            assert_eq!(found.agent_pids, HashMap::from([(name.to_owned(), if provider == "kimi" { 900 } else if provider == "omp" { 810 } else { 800 })]));
        }
    }
}

fn write_native_input(world: &World, path: &str, text: &str) {
    let path = PathBuf::from(world.real_str(path));
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, world.real_str(text)).unwrap();
}

fn native_ticket(world: &World, provider: &str, pane: &str, pid: i64, cwd: &str, procs: &FakeProcs) -> Option<String> {
    discover_other::ticket_transcript(provider, pane, Some(pid), cwd, procs, &world.dirs(),
        &mut hangar_server::list::discover::Problems::default())
}

#[test]
fn native_ticket_rejects_unprovable_freshness_and_wrong_json_types() {
    // Casos de test_pi_registry e test_kimi_sessions: tipo válido errado difere de JSON ilegível.
    let world = World::new();
    let mut procs = FakeProcs(HashMap::from([(7, json!({"env": {"CP_PI_SESSION": "bbb"}, "start": 1700000000.0}))]));
    write_native_input(&world, "⟦ROOT⟧/home/.pi/agent/sessions/--w--/2026_bbb.jsonl", "");
    let fallback = world.real_str("⟦ROOT⟧/home/.pi/agent/sessions/--w--/2026_bbb.jsonl");
    for provider in ["pi", "kimi"] {
        let sub = if provider == "pi" { ".hangar-pi" } else { ".hangar-kimi" };
        let ticket = format!("⟦ROOT⟧/home/.claude/{sub}/123.json");
        for text in ["null", "[1, 2, 3]", "\"uma string\"", "42"] {
            write_native_input(&world, &ticket, text);
            assert_eq!(native_ticket(&world, provider, "%123", 7, "/w", &procs), None, "{provider}: {text}");
        }
        write_native_input(&world, &ticket, "{");
        assert_eq!(native_ticket(&world, provider, "%123", 7, "/w", &procs),
            (provider == "pi").then(|| fallback.clone()));
        for ts in [Value::Null, json!("1700000600")] {
            write_native_input(&world, &ticket, &json!({"file": "/s/aaa.jsonl", "session_id": "session_abc", "ts": ts}).to_string());
            assert_eq!(native_ticket(&world, provider, "%123", 7, "/w", &procs),
                (provider == "pi").then(|| fallback.clone()));
        }
        write_native_input(&world, &ticket, &json!({"file": "/s/aaa.jsonl", "session_id": "session_abc", "ts": 1700000600.0}).to_string());
        procs.0.get_mut(&7).unwrap()["start"] = Value::Null;
        assert_eq!(native_ticket(&world, provider, "%123", 7, "/w", &procs),
            (provider == "pi").then(|| fallback.clone()));
        procs.0.get_mut(&7).unwrap()["start"] = json!(1700000000.0);
    }
}

#[test]
fn native_omp_profile_fallback_and_pi_duplicate_selection() {
    // Vetores preservados de test_pi_sessions: busca omp fora do cwd e Pi restrito ao cwd.
    let world = World::new();
    let mut procs = FakeProcs(HashMap::from([(7, json!({"env": {"OMP_PROFILE": "work", "CP_PI_SESSION": "d5bb540a-af7b-45a7-ad80-e7de61326ead"}, "start": 1.0}))]));
    let actual = "⟦ROOT⟧/home/.omp/profiles/work/agent/sessions/-/2026-09-04T19-42-03-000Z_d5bb540a-af7b-45a7-ad80-e7de61326ead.jsonl";
    write_native_input(&world, actual, "");
    assert_eq!(native_ticket(&world, "omp", "%7", 7, "/tmp", &procs), Some(world.real_str(actual)));
    assert_eq!(native_ticket(&world, "pi", "%7", 7, "/tmp", &procs), None);
    write_native_input(&world, "⟦ROOT⟧/home/.claude/.hangar-pi/7.json",
        r#"{"file":"/requested/2026-09-04T19-42-03-000Z_d5bb540a-af7b-45a7-ad80-e7de61326ead.jsonl","ts":2}"#);
    assert_eq!(native_ticket(&world, "omp", "%7", 7, "/tmp", &procs), Some(world.real_str(actual)));
    std::fs::remove_file(world.real_str("⟦ROOT⟧/home/.claude/.hangar-pi/7.json")).unwrap();
    procs.0.get_mut(&7).unwrap()["env"]["CP_PI_SESSION"] = json!("abc");
    for (name, at) in [("2026-07-01T00-00-00-000Z_abc.jsonl", 1.0), ("2026-07-27T00-00-00-000Z_abc.jsonl", 2.0)] {
        let path = format!("⟦ROOT⟧/home/.pi/agent/sessions/--w--/{name}");
        write_native_input(&world, &path, "");
        set_mtime(Path::new(&world.real_str(&path)), at);
    }
    assert_eq!(native_ticket(&world, "pi", "%7", 7, "/w", &procs),
        Some(world.real_str("⟦ROOT⟧/home/.pi/agent/sessions/--w--/2026-07-27T00-00-00-000Z_abc.jsonl")));
}

#[test]
fn native_kimi_cwd_hash_fallback_and_psmux_collision() {
    let world = World::new();
    // Hash medido pelo CLI, sem usar o Rust para construir o caminho esperado.
    let dir = "⟦ROOT⟧/home/.kimi-code/sessions/wd_kimi-acp-probe_15ca61fc9ec9/session_aaa";
    let procs = FakeProcs(HashMap::from([
        (11, json!({"env": {"PSMUX_SESSION": "alfa"}, "start": 1.0})),
        (12, json!({"env": {"PSMUX_SESSION": "beta"}, "start": 1.0})),
    ]));
    write_native_input(&world, "⟦ROOT⟧/home/.claude/.hangar-kimi/alfa.json",
        r#"{"session_id":"session_aaa","cwd":"/tmp/kimi-acp-probe","ts":1}"#);
    write_native_input(&world, "⟦ROOT⟧/home/.claude/.hangar-kimi/beta.json",
        r#"{"session_id":"session_bbb","cwd":"/tmp/kimi-acp-probe","ts":1}"#);
    assert_eq!(native_ticket(&world, "kimi", "%1", 11, "/outro-cwd", &procs), None);
    std::fs::create_dir_all(world.real_str(dir)).unwrap();
    assert_eq!(native_ticket(&world, "kimi", "%1", 11, "/outro-cwd", &procs),
        Some(world.real_str(&format!("{dir}/agents/main/wire.jsonl"))));
    write_native_input(&world, "⟦ROOT⟧/home/.kimi-code/session_index.jsonl",
        "{\"sessionId\":\"session_bbb\",\"sessionDir\":\"⟦ROOT⟧/indexed\"}\n");
    assert_eq!(native_ticket(&world, "kimi", "%1", 12, "/outro-cwd", &procs),
        Some(world.real_str("⟦ROOT⟧/indexed/agents/main/wire.jsonl")));
    assert_eq!(native_ticket(&world, "kimi", "%1", 11, "/outro-cwd", &procs),
        Some(world.real_str(&format!("{dir}/agents/main/wire.jsonl"))));
}

#[test]
fn native_provider_names_match_the_preserved_process_cases() {
    use hangar_server::list::discover::provider_from_argv;
    for (command, expected) in [
        ("pi --session-id abc".to_owned(), Some("pi")),
        ("/usr/bin/pi -e x.ts --session-id abc".to_owned(), Some("pi")),
        (format!("pi{}", " ".repeat(120)), Some("pi")),
        ("kimi ".to_owned(), Some("kimi")),
        (format!("kimi-code{}", " ".repeat(120)), Some("kimi")),
        ("/usr/bin/kimi-code --model apikey/k3".to_owned(), Some("kimi")),
        ("pip install requests".to_owned(), None),
        ("pipx run x".to_owned(), None),
        ("python -m pip list".to_owned(), None),
        ("mpirun -n 2 a.out".to_owned(), None),
        ("/opt/pi/bin/node server.js".to_owned(), None),
    ] {
        let argv = command.split_whitespace().map(str::to_owned).collect::<Vec<_>>();
        assert_eq!(provider_from_argv(&argv), expected, "{command}");
    }
}

#[test]
fn native_fresh_pi_ticket_wins_and_orphan_subagent_is_not_guessed() {
    let world = World::new();
    let procs = FakeProcs(HashMap::from([(7, json!({"env": {"CP_PI_SESSION": "aaa"}, "start": 1700000000.0}))]));
    let ticket = "⟦ROOT⟧/home/.claude/.hangar-pi/9.json";
    let target = "⟦ROOT⟧/Área de trabalho/2026-07-27T00-00-00-000Z_bbb.jsonl";
    write_native_input(&world, ticket, &json!({"file": target, "id": "bbb", "ts": 1700000600.0}).to_string());
    assert!(!Path::new(&world.real_str(target)).exists());
    assert_eq!(native_ticket(&world, "pi", "%9", 7, "/w", &procs), Some(world.real_str(target)));
    write_native_input(&world, ticket,
        r#"{"file":"⟦ROOT⟧/orfa/t/run-0/session.jsonl","ts":1700000600}"#);
    assert_eq!(native_ticket(&world, "pi", "%9", 7, "/w", &procs), None);
}
