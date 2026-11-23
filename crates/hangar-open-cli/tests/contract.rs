//! Os casos capturados do `scripts/hangar-open` em Python (base 2ad8e335e, `contract/clis/hangar-open__*`)
//! contra o `open-cli`: o pedido que chega ao `hangar-server open`, as frases e o código. A escolha do
//! binário (`loc-*`) e o "sem Python" são do atalho de shell e ficam no `scripts/test-wrappers.sh`.
#![cfg(unix)]

use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use serde_json::Value;

const CASES: &str = include_str!("contract/hangar_open_cases.json");

/// Diferenças decididas, com a fonte: o resto tem de sair igual ao Python.
fn expected_change(case: &str) -> Option<(u8, &'static str)> {
    Some(match case {
        // Traceback do Python vira frase, com o mesmo código 1.
        "provider-missing" => (1, "hangar: uso: hangar-open <claude|pi|omp|kimi|antigravity> [args do CLI]\n"),
        "provider-unknown" => (1, "hangar: provider desconhecido 'codex': use claude, pi, omp, kimi ou antigravity\n"),
        // Servidor morto por sinal diz a frase e sai 1.
        "server-killed-signal" => (1, "hangar: o hangar-server parou pelo sinal 9 sem abrir a sessão\n"),
        // O código final é o do `tmux attach`, que o atalho de shell executa: aqui só o nome sai.
        "tmux-attach-fails" => (0, ""),
        _ => return None,
    })
}

// Um processo nascido em outro teste herda o script ainda aberto para escrita, e o exec dele sai ETXTBSY.
static EXEC: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    EXEC.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn fake_server(dir: &Path, server: &Value) -> std::path::PathBuf {
    let text = |key: &str| server.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
    std::fs::write(dir.join("stdout"), text("stdout")).unwrap();
    std::fs::write(dir.join("stderr"), text("stderr")).unwrap();
    let exit = match server.get("signal").and_then(Value::as_i64) {
        Some(signal) => format!("kill -{signal} $$"),
        None => format!("exit {}", server.get("rc").and_then(Value::as_i64).unwrap_or(0)),
    };
    let script = dir.join("hangar-server");
    std::fs::write(&script, format!(
        "#!/bin/sh\nd=$(dirname \"$0\")\n[ \"$1\" = open ] || exit 99\nprintf '%s' \"$2\" > \"$d/request.json\"\ncat \"$d/stdout\"\ncat \"$d/stderr\" >&2\n{exit}\n"
    )).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script
}

fn place(value: &str, t: &Path) -> String {
    value.replace("$T", &t.to_string_lossy())
}

fn placed(value: &Value, t: &Path) -> Value {
    match value {
        Value::String(text) => Value::String(place(text, t)),
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), placed(v, t))).collect()),
        other => other.clone(),
    }
}

#[test]
fn captured_cases() {
    let cases: Value = serde_json::from_str(CASES).unwrap();
    let mut failures = Vec::new();
    for case in cases["cases"].as_array().unwrap() {
        let name = case["case"].as_str().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let t = dir.path();
        let server_dir = t.join("srv");
        std::fs::create_dir_all(&server_dir).unwrap();
        let _exec = exclusive();
        let server = fake_server(&server_dir, &case["server"]);
        let cwd = std::path::PathBuf::from(place(case["cwd"].as_str().unwrap(), t));
        std::fs::create_dir_all(&cwd).unwrap();
        let vars: BTreeMap<String, String> = case["env"].as_object().unwrap().iter()
            .map(|(k, v)| (k.clone(), place(v.as_str().unwrap(), t))).collect();
        let args: Vec<String> = case["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap().to_owned()).collect();
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let code = hangar_open_cli::run(&args, &vars, &cwd, &server, &mut out, &mut err);
        let err = String::from_utf8_lossy(&err).replace(&*t.to_string_lossy(), "$T");
        let (want_code, want_err) = match expected_change(name) {
            Some((code, err)) => (code, err.to_owned()),
            None => (case["code"].as_u64().unwrap() as u8, case["stderr"].as_str().unwrap().to_owned()),
        };
        // O stderr do `open` passa direto ao terminal, como no Python: não chega ao `err` de `run`.
        let server_said = case["server"].get("stderr").and_then(Value::as_str).unwrap_or("");
        let want_err = want_err.strip_prefix(server_said).unwrap_or(&want_err).to_owned();
        if code != want_code || err != want_err {
            failures.push(format!("{name}: código {code} (esperado {want_code}), stderr {err:?} (esperado {want_err:?})"));
        }
        let sent = std::fs::read_to_string(server_dir.join("request.json")).ok()
            .map(|text| serde_json::from_str::<Value>(&text).unwrap());
        let want_request = (!case["request"].is_null()).then(|| placed(&case["request"], t));
        if sent != want_request {
            failures.push(format!("{name}: pedido {sent:?} (esperado {want_request:?})"));
        }
        let want_out = case["attach"].as_str().map(|n| format!("{n}\n")).unwrap_or_default();
        if String::from_utf8_lossy(&out) != want_out {
            failures.push(format!("{name}: stdout {:?} (esperado {want_out:?})", String::from_utf8_lossy(&out)));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// O pedido sai com o `json.dumps` do Python: quem lê o argv do `open` vê o mesmo texto de antes.
#[test]
fn request_text_is_python_json() {
    let _exec = exclusive();
    let dir = tempfile::tempdir().unwrap();
    let server = fake_server(dir.path(), &serde_json::json!({"rc": 0, "stdout": "{\"name\": \"x\"}"}));
    let vars = BTreeMap::from([("PWD".to_owned(), "/pasta Área".to_owned())]);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let args = ["claude".to_owned(), "olá \"aspas\"".to_owned()];
    assert_eq!(hangar_open_cli::run(&args, &vars, dir.path(), &server, &mut out, &mut err), 0);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("request.json")).unwrap(),
        "{\"provider\": \"claude\", \"cwd\": \"/pasta \\u00c1rea\", \"headless\": false, \"initial_prompt\": \"ol\\u00e1 \\\"aspas\\\"\"}"
    );
}

/// Um 100 do servidor nunca chega ao wrapper: ele abriria a sessão uma segunda vez.
#[test]
fn server_100_becomes_1() {
    let _exec = exclusive();
    let dir = tempfile::tempdir().unwrap();
    let server = fake_server(dir.path(), &serde_json::json!({"rc": 100}));
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let code = hangar_open_cli::run(&["pi".to_owned()], &BTreeMap::new(), dir.path(), &server, &mut out, &mut err);
    assert_eq!(code, 1);
}
