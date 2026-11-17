//! Repete pelos hooks Rust as capturas reais do Python (`tests/fixtures/contract/*.json`, base
//! 2ad8e335e): mesmo stdin, mesmo ambiente e mesma árvore antes; mesmo código, stdout, stderr e
//! arquivos depois. `ts` é comparado por presença (modo N); as diferenças aceitas pelo contrato
//! (R1-D05…D08) têm o esperado escrito no próprio teste.
#![cfg(unix)]
use base64::Engine;
use hangar_agent_hooks::{Context, Outcome, askq, nav, preview, state, tree::Processes};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/contract");
/// O pid vivo (o `sleeper` da captura) gravado nos `_srv.json`: no replay vira o deste processo.
const CAPTURE_ALIVE_PID: &str = "3239413";

fn load(name: &str) -> Value {
    serde_json::from_slice(&fs::read(format!("{FIXTURES}/{name}.json")).unwrap()).unwrap()
}

fn names(prefix: &str) -> Vec<String> {
    let mut out: Vec<String> = fs::read_dir(FIXTURES).unwrap().flatten()
        .map(|e| e.file_name().to_string_lossy().trim_end_matches(".json").to_owned())
        .filter(|n| n.starts_with(prefix))
        .collect();
    out.sort();
    out
}

fn bytes_of(content: &Value) -> Option<Vec<u8>> {
    if let Some(text) = content.get("text").and_then(Value::as_str) {
        return Some(text.as_bytes().to_vec());
    }
    content.get("b64").and_then(Value::as_str).map(|b64| base64::engine::general_purpose::STANDARD.decode(b64).unwrap())
}

fn mode_of(entry: &Value) -> u32 {
    u32::from_str_radix(entry["mode"].as_str().unwrap().trim_start_matches("0o"), 8).unwrap()
}

/// A árvore de antes; pastas sem escrita só depois de tudo criado.
fn materialize(root: &Path, files: &Value, alive_pid: u32) {
    let mut modes = Vec::new();
    for (path, entry) in files.as_object().into_iter().flatten() {
        let target = root.join(path);
        if entry["type"] == "dir" {
            fs::create_dir_all(&target).unwrap();
            modes.push((target, mode_of(entry)));
            continue;
        }
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        let content = &entry["content"];
        let bytes = if content["omitted"] == true {
            vec![b'x'; entry["size"].as_u64().unwrap() as usize]
        } else {
            let mut bytes = bytes_of(content).unwrap();
            if path.ends_with("_srv.json") {
                bytes = String::from_utf8(bytes).unwrap().replace(CAPTURE_ALIVE_PID, &alive_pid.to_string()).into_bytes();
            }
            bytes
        };
        fs::write(&target, bytes).unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(mode_of(entry))).unwrap();
    }
    modes.sort_by_key(|(path, _)| std::cmp::Reverse(path.components().count()));
    for (path, mode) in modes {
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }
}

#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Dir,
    File(Vec<u8>, u32),
}

fn snapshot(root: &Path) -> BTreeMap<String, Entry> {
    let mut out = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
            if relative.starts_with("stubs") || relative.starts_with("work") {
                continue;
            }
            let meta = fs::symlink_metadata(&path).unwrap();
            if meta.is_dir() {
                out.insert(relative, Entry::Dir);
                pending.push(path);
            } else {
                out.insert(relative, Entry::File(fs::read(&path).unwrap_or_default(), meta.permissions().mode() & 0o777));
            }
        }
    }
    out
}

/// Pastas sem escrita voltam a 0755 para o tempdir sair.
fn release(root: &Path) {
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o755));
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                pending.push(entry.path());
            }
        }
    }
}

fn substitute(text: &str, root: &Path, port: u16) -> String {
    text.replace("$T", &root.to_string_lossy()).replace("$HTTP_PORT", &port.to_string())
}

fn context(fixture: &Value, root: &Path, port: u16) -> Context {
    let mut vars = BTreeMap::new();
    for (key, value) in fixture["input"]["env"].as_object().into_iter().flatten() {
        vars.insert(key.clone(), substitute(value.as_str().unwrap(), root, port));
    }
    let home = vars.get("HOME").map(PathBuf::from);
    Context { home, vars }
}

/// A árvore de processos da captura: `sh -c <comando>` (pai do hook) dentro das camadas, do pid 1 para dentro.
struct Table(BTreeMap<u32, (Vec<String>, u32)>);
const SH_PID: u32 = 1000;

impl Table {
    fn of(fixture: &Value) -> Table {
        let mut map = BTreeMap::new();
        let layers = fixture["harness"]["layers"].as_array().cloned().unwrap_or_default();
        for (index, layer) in layers.iter().enumerate() {
            let mut argv = vec![layer["argv0"].as_str().unwrap().to_owned()];
            argv.extend(layer["extra"].as_array().into_iter().flatten().map(|a| a.as_str().unwrap().to_owned()));
            argv.push(String::new());
            map.insert(index as u32 + 1, (argv, index as u32));
        }
        let command = fixture["harness"]["command"].as_str().unwrap_or_default().to_owned();
        map.insert(SH_PID, (vec!["sh".into(), "-c".into(), command, String::new()], layers.len() as u32));
        Table(map)
    }
}

impl Processes for Table {
    fn argv(&self, pid: u32) -> Option<Vec<String>> { self.0.get(&pid).map(|(argv, _)| argv.clone()) }
    fn parent(&self, pid: u32) -> Option<u32> { self.0.get(&pid).map(|(_, parent)| *parent) }
}

/// O Jev falso: grava cada pedido e responde o que o caso da captura respondia.
#[derive(Clone, Default)]
struct Requests(Arc<Mutex<Vec<(String, Vec<(String, String)>, Vec<u8>)>>>);

struct Reply {
    status: u16,
    body: String,
    delay: f64,
}

fn jev_server(reply: Reply) -> (u16, Requests) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Requests::default();
    let seen = requests.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut stream = stream;
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            let header_end = loop {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 { break None; }
                raw.extend_from_slice(&buf[..n]);
                if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") { break Some(at + 4); }
            };
            let Some(header_end) = header_end else { continue };
            let head = String::from_utf8_lossy(&raw[..header_end]).into_owned();
            let mut lines = head.split("\r\n");
            let path = lines.next().unwrap_or_default().split(' ').nth(1).unwrap_or_default().to_owned();
            let headers: Vec<(String, String)> = lines.filter_map(|l| l.split_once(": ")).map(|(k, v)| (k.to_ascii_lowercase(), v.to_owned())).collect();
            let length: usize = headers.iter().find(|(k, _)| k == "content-length").map_or(0, |(_, v)| v.parse().unwrap());
            while raw.len() < header_end + length {
                let n = stream.read(&mut buf).unwrap_or(0);
                if n == 0 { break; }
                raw.extend_from_slice(&buf[..n]);
            }
            seen.0.lock().unwrap().push((path, headers, raw[header_end..].to_vec()));
            std::thread::sleep(std::time::Duration::from_secs_f64(reply.delay));
            let _ = write!(stream, "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                reply.status, reply.body.len(), reply.body);
        }
    });
    (port, requests)
}

fn ok(score: f64) -> Reply {
    Reply { status: 200, body: format!("{{\"answers\": {{\"precisa\": {{\"noul\": {score}}}}}}}"), delay: 0.0 }
}

/// O que cada caso de navegador tinha no PATH e na rede (capture.py, `r1_nav`).
fn nav_world(name: &str, root: &Path) -> (u16, Option<Requests>) {
    let tmux = match name {
        "nav__tmux-pane-one-pane" => Some(("%1\tminha-sessao\n%2\toutra\n", 0)),
        "nav__tmux-pane-zero-panes" => Some(("%2\toutra\n", 0)),
        "nav__tmux-pane-two-panes" => Some(("%1\tminha-sessao\n%1\toutra\n", 0)),
        "nav__tmux-pane-tmux-fails" => Some(("", 1)),
        _ => None,
    };
    if let Some((out, rc)) = tmux {
        let stubs = root.join("stubs");
        fs::create_dir_all(&stubs).unwrap();
        fs::write(stubs.join("tmux.out"), out).unwrap();
        let script = stubs.join("tmux");
        fs::write(&script, format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$(dirname \"$0\")/tmux.argv\"\ncat \"$(dirname \"$0\")/tmux.out\"\nexit {rc}\n")).unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let reply = match name {
        "nav__jev-high" | "nav__jev-model-env" | "nav__jev-long-prompt-tail" | "nav__jev-regex-miss-no-call" => Some(ok(0.9)),
        "nav__jev-threshold-0.5" => Some(ok(0.5)),
        "nav__jev-low" => Some(ok(0.2)),
        "nav__jev-short-prompt-no-call" | "nav__jev-no-key-no-call" | "nav__jev-pause-active" | "nav__jev-pause-expired" => Some(ok(0.1)),
        "nav__jev-500-then-paused" => Some(Reply { status: 500, body: "{\"error\": \"boom\"}".into(), delay: 0.0 }),
        "nav__log-rotation-256k" | "nav__jev-fails-pause-not-written" => Some(Reply { status: 500, body: "{}".into(), delay: 0.0 }),
        "nav__jev-slow-timeout" => Some(Reply { status: 200, body: "{\"answers\": {\"precisa\": {\"noul\": 0.1}}}".into(), delay: 3.0 }),
        "nav__jev-bad-json" => Some(Reply { status: 200, body: "não json".into(), delay: 0.0 }),
        _ => None,
    };
    match reply {
        Some(reply) => {
            let (port, requests) = jev_server(reply);
            (port, Some(requests))
        }
        None if name == "nav__jev-unreachable" => {
            let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let port = closed.local_addr().unwrap().port();
            drop(closed);
            (port, None)
        }
        None => (0, None),
    }
}

fn log_line_shape(expected: &str, actual: &str) -> bool {
    let re = regex::Regex::new(r"^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d ").unwrap();
    let (expected, actual): (Vec<&str>, Vec<&str>) = (expected.lines().collect(), actual.lines().collect());
    // Texto da exceção pode diferir (decisão 13.4); a forma, o começo do motivo e o fim não.
    expected.len() == actual.len()
        && actual.iter().all(|line| re.is_match(line))
        && expected.iter().zip(&actual).all(|(e, a)| {
            let (e, a) = (re.replace(e, ""), re.replace(a, ""));
            e.split('(').next() == a.split('(').next() && e.rsplit(')').next() == a.rsplit(')').next()
        })
}

fn same_content(path: &str, expected: &Value, actual: &[u8], mode: u32) -> Result<(), String> {
    let content = &expected["content"];
    if content["omitted"] == true {
        return (actual.len() as u64 == expected["size"].as_u64().unwrap()).then_some(()).ok_or(format!("{path}: tamanho"));
    }
    let want = bytes_of(content).unwrap();
    if path.ends_with(".log") {
        if mode != mode_of(expected) { return Err(format!("{path}: modo {mode:o}")); }
        let (want, got) = (String::from_utf8_lossy(&want), String::from_utf8_lossy(actual));
        return log_line_shape(&want, &got).then_some(()).ok_or(format!("{path}: {want:?} × {got:?}"));
    }
    if path.contains("/.hangar-askq/") || !path.contains("/.hangar-") {
        return (want == actual).then_some(()).ok_or(format!("{path}: bytes {:?} × {:?}", String::from_utf8_lossy(&want), String::from_utf8_lossy(actual)));
    }
    let (Ok(mut want), Ok(mut got)) = (serde_json::from_slice::<Value>(&want), serde_json::from_slice::<Value>(actual)) else {
        return (want == actual).then_some(()).ok_or(format!("{path}: bytes"));
    };
    for value in [&mut want, &mut got] {
        if let Some(ts) = value.get_mut("ts") {
            if !ts.is_number() { return Err(format!("{path}: ts não é número")); }
            *ts = Value::Null;
        }
    }
    (want == got).then_some(()).ok_or(format!("{path}: {want} × {got}"))
}

fn stdin_of(step: &Value, fixture: &Value) -> Vec<u8> {
    let stdin = if step.get("stdin").is_some() { &step["stdin"] } else { &fixture["input"]["stdin"] };
    bytes_of(stdin).unwrap_or_default()
}

fn run(hook: &str, stdin: &[u8], ctx: &Context, fixture: &Value) -> Outcome {
    match hook {
        "state" => state::handle(stdin, ctx, &Table::of(fixture), Some(SH_PID)),
        "preview" => preview::handle(stdin, ctx),
        "askq" => askq::handle(stdin, ctx),
        "nav" => nav::handle(stdin, ctx),
        other => panic!("{other}"),
    }
}

/// Diferenças aceitas: o esperado do Rust onde ele difere da captura.
fn accepted(name: &str, step: usize, mut expected: Value) -> Value {
    // R1-D08: o índice 0 atrasado da mensagem fechada não republica (o Python republicava).
    if (name, step) == ("preview__late-delta-index0-after-stop", 3) {
        expected["files"] = serde_json::json!({"created": {}, "changed": {}, "removed": []});
    }
    expected
}

/// R1-D06: `session_id` hostil fica dentro da pasta do marcador, com o basename.
const BASENAME_CASES: [&str; 3] = ["state__hostile-session_id-dotdot", "state__hostile-session_id-slash", "askq__pre-hostile-session_id"];

fn replay(name: &str) {
    let fixture = load(name);
    let hook = name.split("__").next().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("home")).unwrap();
    let env = &fixture["input"]["env"];
    if env.get("CLAUDE_CONFIG_DIR").is_some_and(|v| v.as_str().is_some_and(|v| v.contains("$T/cfg"))) {
        fs::create_dir_all(root.join("cfg")).unwrap();
    }
    materialize(root, &fixture["input"]["files_before"], std::process::id());
    if name.ends_with("config-dir-readonly") || name.ends_with("config-not-writable") {
        fs::set_permissions(root.join("cfg"), fs::Permissions::from_mode(0o500)).unwrap();
    }
    if name == "nav__jev-pause-expired" {
        let pause = fs::File::options().write(true).open(root.join("home/.hangar/nav/_jev_pausa")).unwrap();
        pause.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(301)).unwrap();
    }
    let (port, requests) = if hook == "nav" { nav_world(name, root) } else { (0, None) };
    let ctx = context(&fixture, root, port);
    let steps: Vec<Value> = match fixture.get("steps") {
        Some(steps) => steps.as_array().unwrap().clone(),
        None => vec![fixture["output"].clone()],
    };
    for (index, step) in steps.iter().enumerate() {
        let step = accepted(name, index, step.clone());
        let before = snapshot(root);
        let outcome = run(hook, &stdin_of(&step, &fixture), &ctx, &fixture);
        let after = snapshot(root);
        assert_eq!(outcome.code as i64, step["code"].as_i64().unwrap(), "{name}[{index}] código");
        let stdout = if name == "nav__stdin-cp1252-simulated" {
            // R1-D05: o Rust lê UTF-8 também no Windows; vale o resultado da referência UTF-8.
            bytes_of(&load("nav__stdin-utf8-reference")["steps"][0]["stdout"]).unwrap()
        } else {
            bytes_of(&step["stdout"]).unwrap_or_default()
        };
        assert_eq!(String::from_utf8_lossy(&outcome.stdout), String::from_utf8_lossy(&stdout), "{name}[{index}] stdout");
        assert_eq!(String::from_utf8_lossy(&outcome.stderr), String::from_utf8_lossy(&bytes_of(&step["stderr"]).unwrap_or_default()), "{name}[{index}] stderr");
        let created: BTreeSet<&String> = after.keys().filter(|k| !before.contains_key(*k)).collect();
        let removed: BTreeSet<&String> = before.keys().filter(|k| !after.contains_key(*k)).collect();
        let changed: BTreeSet<&String> = after.iter().filter(|(k, v)| before.get(*k).is_some_and(|b| b != *v)).map(|(k, _)| k).collect();
        let files = &step["files"];
        if BASENAME_CASES.contains(&name) {
            assert!(created.iter().all(|path| path.split('/').nth(1).is_some_and(|top| top.starts_with(".hangar-"))), "{name}: {created:?}");
            let key = if name.ends_with("dotdot") { "x.json" } else if name.ends_with("slash") { "b.json" } else { "fora.json" };
            let folders: &[&str] = if hook == "askq" { &[".hangar-askq"] } else { &[".hangar-state", ".hangar-prompt", ".hangar-active"] };
            for folder in folders {
                assert!(created.contains(&format!("cfg/{folder}/{key}")), "{name}: {folder}/{key} em {created:?}");
            }
            continue;
        }
        let want_created: BTreeSet<String> = files["created"].as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect();
        // R1-D07: o temporário fixo de antes (`<sid>.json.tmp`) não é mais o do hook; quem o leva é a poda.
        let want_removed: BTreeSet<String> = files["removed"].as_array().into_iter().flatten()
            .filter_map(|v| v.as_str()).filter(|p| !p.ends_with(".tmp")).map(str::to_owned).collect();
        let want_changed: BTreeSet<String> = files["changed"].as_object().into_iter().flatten().map(|(k, _)| k.clone()).collect();
        assert_eq!(created.iter().map(|s| (*s).clone()).collect::<BTreeSet<_>>(), want_created, "{name}[{index}] criados");
        assert_eq!(removed.iter().map(|s| (*s).clone()).collect::<BTreeSet<_>>(), want_removed, "{name}[{index}] apagados");
        assert_eq!(changed.iter().map(|s| (*s).clone()).collect::<BTreeSet<_>>(), want_changed, "{name}[{index}] alterados");
        for (path, entry) in files["created"].as_object().into_iter().flatten() {
            if let Some(Entry::File(bytes, mode)) = after.get(path) {
                same_content(path, entry, bytes, *mode).unwrap_or_else(|e| panic!("{name}[{index}] {e}"));
            }
        }
        for (path, entry) in files["changed"].as_object().into_iter().flatten() {
            if let Some(Entry::File(bytes, mode)) = after.get(path) {
                same_content(path, &entry["after"], bytes, *mode).unwrap_or_else(|e| panic!("{name}[{index}] {e}"));
            }
        }
    }
    if let Some(requests) = requests {
        let got = requests.0.lock().unwrap().clone();
        let want = fixture["http"].as_array().cloned().unwrap_or_default();
        assert_eq!(got.len(), want.len(), "{name}: pedidos ao Jev");
        for ((path, headers, body), want) in got.iter().zip(&want) {
            assert_eq!(path, want["raw_path"].as_str().unwrap(), "{name}: caminho");
            let header = |key: &str| headers.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
            assert_eq!(header("authorization"), Some("Bearer fake-typesafe-key"), "{name}");
            assert_eq!(header("content-type"), Some("application/json"), "{name}");
            let want_body: Value = serde_json::from_slice(&bytes_of(&want["body"]).unwrap()).unwrap();
            assert_eq!(serde_json::from_slice::<Value>(body).unwrap(), want_body, "{name}: corpo");
        }
    }
    if name.starts_with("nav__tmux-pane-") && name != "nav__tmux-pane-no-tmux-on-path" {
        let argv = fs::read_to_string(root.join("stubs/tmux.argv")).unwrap();
        assert_eq!(argv, "list-panes\n-a\n-F\n#{pane_id}\t#{session_name}\n", "{name}: R1-S15");
    }
    release(root);
}

macro_rules! replay_all {
    ($test:ident, $prefix:literal) => {
        #[test]
        fn $test() {
            let cases = names($prefix);
            assert!(!cases.is_empty());
            for name in cases {
                if name.starts_with("preview__concurrent") {
                    continue;
                }
                replay(&name);
            }
        }
    };
}

// H-01…H-12 (marcadores de estado, prompt e ativo; árvore de processos; stdin tolerante).
replay_all!(h01_h12_state_hook_matches_the_python_captures, "state__");
// H-15…H-23 (prévia por índice, Stop, retardatários, trava, falhas com uma linha no stderr).
replay_all!(h15_h23_preview_hook_matches_the_python_captures, "preview__");
// H-43…H-46 (pergunta gravada crua, apagada só pela mesma chamada, sem stdout).
replay_all!(h43_h46_askq_capture_matches_the_python_captures, "askq__");
// H-32…H-41 (navegador: pid vivo, nome da sessão, URL, regex, Jev, pausa, log).
replay_all!(h32_h41_nav_hook_matches_the_python_captures, "nav__");
