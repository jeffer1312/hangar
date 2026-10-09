//! Contexto e modelo da sessão Claude contra os goldens de decoração e estado.
use crate::common;

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use common::{canon, golden};
use hangar_server::list::context::{self, ContextCache, ReadingInputs};
use serde_json::{json, Value};

/// Pasta do caso com os marcadores do golden trocados pelo caminho real.
struct World {
    root: PathBuf,
    _tmp: tempfile::TempDir,
}

impl World {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        Self { root, _tmp: tmp }
    }

    fn real(&self, value: &str) -> String {
        let root = self.root.to_string_lossy().replace('\\', "/");
        let san: String =
            root.trim_end_matches('/').chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
        value.replace("⟦ROOT_SAN⟧", &san).replace("⟦ROOT⟧", &root)
    }

    fn apply_fs(&self, ops: &[Value], wall: f64) {
        for op in ops {
            let path = PathBuf::from(self.real(op["path"].as_str().expect("path")));
            match op["op"].as_str().expect("op") {
                "mkdir" => std::fs::create_dir_all(&path).expect("mkdir"),
                "rm" => {
                    if path.is_dir() {
                        std::fs::remove_dir_all(&path).expect("rm dir");
                    } else {
                        std::fs::remove_file(&path).expect("rm");
                    }
                }
                "write" => {
                    std::fs::create_dir_all(path.parent().unwrap()).expect("mkdir pai");
                    std::fs::write(&path, self.real(op["text"].as_str().expect("text"))).expect("write");
                    set_mtime(&path, op["mtime"].as_f64().unwrap_or(wall));
                }
                "touch" => set_mtime(&path, op["mtime"].as_f64().expect("mtime")),
                other => panic!("op de arquivo desconhecida: {other}"),
            }
        }
    }
}

fn set_mtime(path: &Path, at: f64) {
    let file = std::fs::OpenOptions::new().write(true).open(path).expect("abrir para mtime");
    file.set_modified(UNIX_EPOCH + Duration::from_secs_f64(at)).expect("mtime");
}

fn field<'a>(row: &'a Value, defaults: &'a Value, key: &str) -> &'a Value {
    row.get(key).unwrap_or(&defaults[key])
}

/// Pid do agente Claude do pane: o próprio processo do pane ou o primeiro descendente `claude`.
fn agent_proc<'a>(procs: &'a [Value], pane_pid: u64) -> Option<&'a Value> {
    let is_claude =
        |p: &Value| p["argv"][0].as_str().is_some_and(|a| a.rsplit('/').next() == Some("claude"));
    let mut stack = vec![pane_pid];
    while let Some(pid) = stack.pop() {
        if let Some(p) = procs.iter().find(|p| p["pid"].as_u64() == Some(pid)) {
            if is_claude(p) {
                return Some(p);
            }
        }
        stack.extend(procs.iter().filter(|p| p["ppid"].as_u64() == Some(pid)).filter_map(|p| p["pid"].as_u64()));
    }
    None
}

/// Modelo que a statusline recebeu (`<config>/.hangar-status/<stem>.json`), com o teto de um dia.
fn chosen_model(world: &World, stem: &str, wall: f64) -> Option<String> {
    for cfg in ["⟦ROOT⟧/home/.claude", "⟦ROOT⟧/home/.claude-alt"] {
        let path = PathBuf::from(world.real(cfg)).join(".hangar-status").join(format!("{stem}.json"));
        let Ok(raw) = std::fs::read_to_string(&path) else { continue };
        let Ok(obj) = serde_json::from_str::<Value>(&raw) else { continue };
        if !obj["line"].as_str().is_some_and(|l| !l.trim().is_empty()) {
            continue;
        }
        if obj["ts"].as_f64().is_some_and(|ts| wall - ts > 86_400.0) {
            continue;
        }
        return obj["model"].as_str().filter(|m| !m.is_empty()).map(str::to_owned);
    }
    None
}

fn run_context_cases(file: &str) -> usize {
    let g = golden(file);
    let defaults = &g["defaults"];
    let mono_off = g["mono_off"].as_f64().expect("mono_off");
    let mut checked = 0;
    for case in g["cases"].as_array().expect("casos") {
        let name = case["name"].as_str().unwrap();
        let world = World::new();
        let mut cache = ContextCache::default();
        for (i, tick) in case["ticks"].as_array().unwrap().iter().enumerate() {
            let wall = tick["at"].as_f64().unwrap();
            world.apply_fs(tick["fs"].as_array().map(Vec::as_slice).unwrap_or(&[]), wall);
            for op in tick["ops"].as_array().map(Vec::as_slice).unwrap_or(&[]) {
                match op["op"].as_str() {
                    Some("forget") => cache.forget(op["name"].as_str().unwrap()),
                    Some("seed" | "fresh") => {}
                    other => panic!("{name}: op sem porte neste teste: {other:?}"),
                }
            }
            let procs = tick["procs"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            let panes = tick["panes"].as_array().map(Vec::as_slice).unwrap_or(&[]);
            for row in tick["expected"]["rows"].as_array().expect("rows") {
                if field(row, defaults, "provider").as_str() != Some("claude") {
                    continue;
                }
                let row_name = row["name"].as_str().unwrap();
                let (want_ctx, want_model) = (field(row, defaults, "context"), field(row, defaults, "model"));
                let Some(jsonl) = field(row, defaults, "jsonl").as_str().map(|j| world.real(j)) else {
                    assert!(want_ctx.is_null() && want_model.is_null(), "{name}/{i}/{row_name}: sem jsonl");
                    continue;
                };
                let stem = Path::new(&jsonl).file_stem().unwrap().to_string_lossy().into_owned();
                let conta = field(row, defaults, "conta").as_str().map(|c| world.real(c));
                let account_dir = context::config_dir_of(conta.as_deref())
                    .unwrap_or_else(|| PathBuf::from(world.real("⟦ROOT⟧/home/.claude")));
                let (opened, declared) = if field(row, defaults, "headless").as_bool() == Some(true) {
                    let meta_path = PathBuf::from(world.real("⟦ROOT⟧/home/.hangar/claude-headless"))
                        .join(format!("{row_name}.json"));
                    let meta: Value = std::fs::read_to_string(meta_path)
                        .ok()
                        .and_then(|raw| serde_json::from_str(&raw).ok())
                        .unwrap_or(Value::Null);
                    (meta["model"].as_str().map(str::to_owned), context::declared_window_value(&meta["context_window"]))
                } else {
                    let pane_pid = panes.iter().find(|p| p["name"].as_str() == Some(row_name)).and_then(|p| p["pid"].as_u64());
                    match pane_pid.and_then(|pid| agent_proc(procs, pid)) {
                        Some(p) => {
                            let argv: Vec<String> =
                                p["argv"].as_array().unwrap().iter().map(|a| world.real(a.as_str().unwrap())).collect();
                            let env = p["env"]["CLAUDE_CODE_MAX_CONTEXT_TOKENS"].as_str();
                            (context::opened_model(&argv), env.and_then(context::declared_window))
                        }
                        None => (None, None),
                    }
                };
                let chosen = chosen_model(&world, &stem, wall);
                let engine = !field(row, defaults, "engine").is_null();
                let now = wall - mono_off;
                let (ctx, model) = if cache.stale(row_name, &jsonl, now) {
                    let version = context::source_version(&jsonl);
                    let reading = context::claude_reading(&ReadingInputs {
                        jsonl: Path::new(&jsonl),
                        account_dir: &account_dir,
                        chosen: chosen.as_deref(),
                        opened: opened.as_deref(),
                        declared,
                        engine,
                    });
                    cache.store(row_name, &jsonl, now, reading, version)
                } else {
                    cache.cached(row_name, &jsonl)
                };
                let got = json!({"context": ctx, "model": model});
                let want = json!({"context": want_ctx, "model": want_model});
                assert_eq!(canon(&got), canon(&want), "{file} {name} tique {i} {row_name}");
                checked += 1;
            }
        }
    }
    checked
}

#[test]
fn context_cases() {
    let checked = run_context_cases("list_decorate.json") + run_context_cases("list_state.json");
    assert!(checked > 0, "nenhuma linha Claude conferida");
}

#[test]
fn context_session_model_rules() {
    let dir = tempfile::tempdir().unwrap();
    let account = dir.path();
    std::fs::write(account.join("settings.json"), r#"{"model": "opus[1m]"}"#).unwrap();
    // Snapshot datado perde a data; família igual à da conta em 1M ganha o [1m].
    assert_eq!(context::session_model(Some("claude-opus-4-5-20251101"), None, account, 10, false).as_deref(),
               Some("claude-opus-4-5[1m]"));
    // Família diferente da conta, uso dentro dos 200k: sem [1m].
    assert_eq!(context::session_model(Some("claude-sonnet-4-5"), None, account, 10, false).as_deref(),
               Some("claude-sonnet-4-5"));
    // Sem resposta: modelo da abertura sem a data; motor não cai na conta.
    assert_eq!(context::session_model(None, Some("claude-haiku-4-5-20251001"), account, 0, false).as_deref(),
               Some("claude-haiku-4-5"));
    assert_eq!(context::session_model(None, None, account, 0, true), None);
    assert_eq!(context::session_model(None, None, account, 0, false).as_deref(), Some("opus[1m]"));
    // [1m] já na resposta fica como está; uso acima de 200k só cabe em 1M.
    assert_eq!(context::session_model(Some("claude-opus-4-5[1m]"), Some("haiku"), account, 10, false).as_deref(),
               Some("claude-opus-4-5[1m]"));
    assert_eq!(context::session_model(Some("claude-sonnet-4-5"), Some("haiku"), account, 200_001, false).as_deref(),
               Some("claude-sonnet-4-5[1m]"));
    // Escolha vazia da statusline cai na abertura.
    let reading = context::claude_reading(&ReadingInputs {
        jsonl: &account.join("ausente.jsonl"),
        account_dir: account,
        chosen: Some(""),
        opened: Some("claude-haiku-4-5"),
        declared: None,
        engine: false,
    });
    assert_eq!(reading, (None, Some("claude-haiku-4-5".into())));
    // settings.json quebrado não derruba: fica sem modelo.
    std::fs::write(account.join("settings.json"), "[1, 2]").unwrap();
    assert_eq!(context::session_model(None, None, account, 0, false), None);
}

#[test]
fn context_reading_skips_sidechain_synthetic_and_zero() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("s.jsonl");
    let usage = |n: u64| json!({"input_tokens": n, "cache_read_input_tokens": 0, "cache_creation_input_tokens": 0});
    let lines = [
        json!({"type": "assistant", "message": {"model": "claude-opus-4-5", "usage": usage(1000)}}),
        json!({"type": "assistant", "isSidechain": true, "message": {"model": "claude-haiku-4-5", "usage": usage(5)}}),
        json!({"type": "assistant", "message": {"model": "<synthetic>", "usage": usage(7)}}),
        json!({"type": "assistant", "message": {"model": "claude-opus-4-5", "usage": usage(0)}}),
    ];
    let text: String = lines.iter().map(|l| format!("{l}\n")).collect::<String>() + "{\"usage\": quebrado\n";
    std::fs::write(&jsonl, text).unwrap();
    let (ctx, answered) = context::read(&jsonl, dir.path(), None, None);
    assert_eq!(ctx.map(|c| (c.used, c.window)), Some((1000, 200_000)));
    assert_eq!(answered.as_deref(), Some("claude-opus-4-5"));
    // Transcript que sumiu: nada, sem erro.
    assert_eq!(context::read(&dir.path().join("x.jsonl"), dir.path(), None, None), (None, None));
}

#[test]
fn context_uses_the_measured_window_without_a_1m_model_alias() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("measured.jsonl");
    std::fs::write(&jsonl, r#"{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":76604}}}"#).unwrap();
    std::fs::write(jsonl.with_extension("context.json"), r#"{"used":76604,"window":1000000}"#).unwrap();
    let (ctx, model) = context::read(&jsonl, dir.path(), Some("opus"), None);
    assert_eq!(ctx.map(|c| (c.used, c.window)), Some((76_604, 1_000_000)));
    assert_eq!(model.as_deref(), Some("claude-opus-5-5"));
}

#[test]
fn declared_window_wins_over_a_measurement_from_before_resume() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("resumed.jsonl");
    std::fs::write(&jsonl, r#"{"type":"assistant","message":{"usage":{"input_tokens":90002}}}"#).unwrap();
    std::fs::write(jsonl.with_extension("context.json"), r#"{"used":90002,"window":1000000}"#).unwrap();
    assert_eq!(context::read(&jsonl, dir.path(), None, Some(256_000)).0.map(|c| c.window), Some(256_000));
}

#[test]
fn context_cache_sees_the_first_answer_and_measurement_before_its_ttl() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("new.jsonl");
    std::fs::write(&jsonl, "").unwrap();
    let path = jsonl.to_str().unwrap();
    let mut cache = ContextCache::default();
    cache.store("new", path, 0.0, (None, Some("opus".into())), context::source_version(path));
    assert!(!cache.stale("new", path, 1.0));
    std::fs::write(&jsonl, r#"{"type":"assistant","message":{"model":"claude-opus-5-5","usage":{"input_tokens":76604}}}"#).unwrap();
    assert!(cache.stale("new", path, 2.0), "a primeira resposta não espera 20 segundos");
    let version = context::source_version(path);
    cache.store("new", path, 2.0, context::read(&jsonl, dir.path(), Some("opus"), None), version);
    assert!(!cache.stale("new", path, 3.0));
    std::fs::write(jsonl.with_extension("context.json"), r#"{"used":76604,"window":1000000}"#).unwrap();
    assert!(cache.stale("new", path, 4.0), "a medida do Claude não espera o cache vencer");
}

#[test]
fn context_cache_does_not_stamp_new_files_on_an_old_reading() {
    let dir = tempfile::tempdir().unwrap();
    let jsonl = dir.path().join("racing.jsonl");
    std::fs::write(&jsonl, "").unwrap();
    let path = jsonl.to_str().unwrap();
    let mut cache = ContextCache::default();
    let version = context::source_version(path);
    let reading = context::read(&jsonl, dir.path(), None, None);
    std::fs::write(jsonl.with_extension("context.json"), r#"{"used":76604,"window":1000000}"#).unwrap();
    cache.store("new", path, 0.0, reading, version);
    assert!(cache.stale("new", path, 1.0), "a medida que chegou durante a leitura não pode ficar presa no cache");
}

#[test]
fn context_cache_keeps_previous_only_for_same_transcript() {
    let mut cache = ContextCache::default();
    let ctx = hangar_api::session::ContextUse { used: 5, window: 200_000 };
    assert!(cache.stale("s", "/a.jsonl", 0.0));
    let first = cache.store("s", "/a.jsonl", 0.0, (Some(ctx), Some("m".into())), context::source_version("/a.jsonl"));
    assert_eq!(first, (Some(ctx), Some("m".into())));
    // Dentro dos 20 s não relê.
    assert!(!cache.stale("s", "/a.jsonl", 20.0));
    assert_eq!(cache.cached("s", "/a.jsonl"), first);
    // Venceu e a leitura veio vazia: mesmo transcript mantém contexto e modelo.
    assert!(cache.stale("s", "/a.jsonl", 20.5));
    assert_eq!(cache.store("s", "/a.jsonl", 20.5, (None, Some("conta".into())), context::source_version("/a.jsonl")), first);
    // Transcript novo: relê na hora e não herda nada.
    assert!(cache.stale("s", "/b.jsonl", 21.0));
    assert_eq!(cache.cached("s", "/b.jsonl"), (None, None));
    assert_eq!(cache.store("s", "/b.jsonl", 21.0, (None, Some("conta".into())), context::source_version("/b.jsonl")), (None, Some("conta".into())));
    cache.forget("s");
    assert!(cache.stale("s", "/b.jsonl", 21.0));
    assert_eq!(cache.cached("s", "/b.jsonl"), (None, None));
}
