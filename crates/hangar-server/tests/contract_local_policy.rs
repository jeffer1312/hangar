use hangar_server::runtime::local_policy::{is_local, run_at};
use serde_json::{json, Value};
use std::path::Path;

fn golden(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/tests/fixtures/contract/local_policy").join(name);
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

fn substitute(value: &Value, root: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace("{ROOT}", root)),
        Value::Array(items) => Value::Array(items.iter().map(|item| substitute(item, root)).collect()),
        Value::Object(fields) => Value::Object(fields.iter().map(|(key, item)| (key.clone(), substitute(item, root))).collect()),
        other => other.clone(),
    }
}

fn materialize(files: &Value, root: &Path) {
    use base64::Engine;
    for (rel, spec) in files.as_object().into_iter().flatten() {
        let path = root.join(rel);
        if spec["dir"] == true { std::fs::create_dir_all(&path).unwrap(); continue; }
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        if let Some(data) = spec["b64"].as_str() {
            std::fs::write(&path, base64::engine::general_purpose::STANDARD.decode(data).unwrap()).unwrap();
        } else if let Some(size) = spec["size"].as_u64() {
            std::fs::write(&path, vec![0u8; size as usize]).unwrap();
        } else {
            std::fs::write(&path, spec["text"].as_str().unwrap().repeat(spec["times"].as_u64().unwrap_or(1) as usize)).unwrap();
        }
    }
}

/// Os goldens mexem em TZ, HOME e nas variáveis de pasta e de esforço, que são do processo: um por vez.
static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn local_policies_match_the_python_golden() {
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let mut failures = Vec::new();
    let mut total = 0;
    for file in ["prepare_prompt.json", "format_status_claude.json", "format_status_codex.json", "skill_catalog.json",
        "last_usage.json", "reload_stamp.json", "unknown_private.json"] {
        let document = golden(file);
        // SAFETY: os outros testes deste binário que leem ou escrevem o ambiente seguram `ENV`; os demais
        // só chamam tipos remotos ou recusam o caminho antes de ler o HOME.
        unsafe { std::env::set_var("TZ", document["tz"].as_str().unwrap()); }
        for case in document["cases"].as_array().unwrap() {
            // O golden sai do Linux: o Windows ignora o TZ do processo e abre pasta como "Permission denied".
            // No Windows o log privado mora em LOCALAPPDATA, não no HOME do golden.
            if cfg!(windows) && (case["kind"] == "unknown_private"
                || case["name"].as_str().is_some_and(|n| n.starts_with("limit_rejected") || n == "directory_named_like_image")) {
                continue;
            }
            total += 1;
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().to_str().unwrap();
            materialize(&case["files"], dir.path());
            // SAFETY: idem.
            unsafe {
                std::env::remove_var("CLAUDE_CODE_EFFORT_LEVEL");
                std::env::set_var("HOME", dir.path().join("home"));
                // No Windows `home_dir` (e o `Path.home()` do Python) leem o USERPROFILE.
                std::env::set_var("USERPROFILE", dir.path().join("home"));
                for (key, value) in case["env"].as_object().into_iter().flatten() { std::env::set_var(key, value.as_str().unwrap()); }
            }
            let kind = case["kind"].as_str().unwrap();
            assert!(is_local(kind), "{kind} deve rodar no Rust");
            let payload = substitute(&case["payload"], root);
            let meta = substitute(&case["meta"], root);
            let quota = case.get("quota");
            let now = case["now"].as_f64().unwrap_or(0.0);
            let call = |payload: &Value| match run_at(kind, payload, &meta, quota, now) {
                Some(Ok(value)) => value,
                Some(Err(_)) => json!({"error": true}),
                None => json!("not local"),
            };
            // `unknown_private` é uma sequência de chamadas (o teto é por chave) e o resultado inclui o arquivo gravado.
            let actual = if let Some(steps) = case["steps"].as_array() {
                let results: Vec<Value> = steps.iter().map(|step| call(&substitute(step, root))).collect();
                let files: serde_json::Map<String, Value> = case["read"].as_array().into_iter().flatten().map(|rel| {
                    let rel = rel.as_str().unwrap();
                    (rel.to_owned(), std::fs::read_to_string(dir.path().join(rel)).map_or(Value::Null, Value::String))
                }).collect();
                json!({"results": results, "files": files})
            } else { call(&payload) };
            #[cfg(unix)]
            if case["name"] == "claude_first_line" {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(dir.path().join("home/.hangar/logs/privado")).unwrap().permissions().mode() & 0o777;
                assert_eq!(mode, 0o700, "a pasta do log privado é só do dono");
            }
            let expected = substitute(&case["expected"], root);
            if actual != expected {
                failures.push(format!("{file}/{}: Rust {actual}; Python {expected}", case["name"]));
            }
        }
    }
    assert!(total > 150, "golden menor que o esperado: {total}");
    assert!(failures.is_empty(), "{} divergências:\n{}", failures.len(), failures.join("\n"));
}

/// `parked_state_at` (sessão Claude sem terminal parada) contra o golden do `_state_stream` do Python.
#[test]
fn parked_state_matches_the_python_golden() {
    use hangar_server::{list::discover_other::sanitize_session_name, state::parked::parked_state_at};
    let _env = ENV.lock().unwrap_or_else(|e| e.into_inner());
    let (mut failures, mut total) = (Vec::new(), 0);
    let document = golden("parked_state.json");
    let (saved_home, saved_profile) = (std::env::var_os("HOME"), std::env::var_os("USERPROFILE"));
    // SAFETY: ver `ENV`; a status line lê TZ, HOME e as variáveis de pasta e de esforço.
    unsafe { std::env::set_var("TZ", document["tz"].as_str().unwrap()); }
    for case in document["cases"].as_array().unwrap() {
        total += 1;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_str().unwrap();
        materialize(&case["files"], dir.path());
        let home = dir.path().join("home");
        // SAFETY: idem.
        unsafe {
            std::env::set_var("HOME", &home);
            // No Windows `home_dir` (e o `Path.home()` do Python) leem o USERPROFILE.
            std::env::set_var("USERPROFILE", &home);
            for key in ["CLAUDE_CODE_EFFORT_LEVEL", "CLAUDE_CONFIG_DIR", "CP_PROJECTS_DIR"] { std::env::remove_var(key); }
            for (key, value) in case["env"].as_object().into_iter().flatten() { std::env::set_var(key, value.as_str().unwrap()); }
        }
        let name = case["name"].as_str().unwrap();
        if !case["sidecar"].is_null() {
            let sidecar = home.join(".hangar/claude-headless").join(format!("{}.json", sanitize_session_name(name)));
            std::fs::create_dir_all(sidecar.parent().unwrap()).unwrap();
            std::fs::write(sidecar, substitute(&case["sidecar"], root).to_string()).unwrap();
        }
        let actual = match parked_state_at(name, &home) {
            None => json!({"state": "dead"}),
            Some(state) => json!({"state": state.state, "claude_permission_mode": state.claude_permission_mode,
                "claude_previous_non_plan": state.claude_previous_non_plan, "status_line": state.status_line,
                "problema": state.problema, "problema_detalhe": state.problema_detalhe}),
        };
        if actual != case["expected"] { failures.push(format!("{name}: Rust {actual}; Python {}", case["expected"])); }
    }
    // SAFETY: idem.
    unsafe {
        for key in ["CLAUDE_CODE_EFFORT_LEVEL", "CLAUDE_CONFIG_DIR", "CP_PROJECTS_DIR"] { std::env::remove_var(key); }
        match saved_home { Some(home) => std::env::set_var("HOME", home), None => std::env::remove_var("HOME") }
        match saved_profile { Some(home) => std::env::set_var("USERPROFILE", home), None => std::env::remove_var("USERPROFILE") }
    }
    assert!(total >= 15, "golden menor que o esperado: {total}");
    assert!(failures.is_empty(), "{} divergências:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn reload_stamp_refuses_tilde_forms_it_does_not_expand() {
    for dir in ["~someone", "~someone/cfg", "~\\cfg"] {
        let meta = json!({"provider": "claude", "config_dir": dir, "cano": {"config_marca": "0".repeat(40)}});
        assert!(matches!(run_at("reload_stamp", &json!({}), &meta, None, 0.0), Some(Err(_))), "{dir} deve falhar");
    }
}

#[test]
fn remote_kinds_are_not_local() {
    for kind in ["native_message", "session.patch_meta", "terminal_facts", "quota", "answer_body"] {
        assert!(!is_local(kind), "{kind} continua no Python ou saiu do serviço");
        assert!(run_at(kind, &json!({}), &json!({"provider": "claude"}), None, 0.0).is_none());
    }
}
