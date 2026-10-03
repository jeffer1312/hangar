use hangar_server::terminal_state::{analyze, reduce, ReducerFacts, ReducerMemory, STALE_LIMIT, IDLE_DEBOUNCE};

#[test]
fn terminal_debounce_limits_match_python_state_monitor() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../backend/app/state.py");
    let source = std::fs::read_to_string(path).expect("referência Python deve existir");
    let class = regex::Regex::new(r"(?m)^class StateMonitor:\r?$").unwrap()
        .find(&source).expect("classe StateMonitor deve existir");
    let rest = &source[class.end()..];
    let end = regex::Regex::new(r"(?m)^(?:class|def) ").unwrap().find(rest)
        .map_or(rest.len(), |next| next.start());
    let body = &rest[..end];
    for (name, rust) in [("STALE_LIMIT", STALE_LIMIT), ("IDLE_DEBOUNCE", IDLE_DEBOUNCE)] {
        let declaration = regex::Regex::new(&format!(r"(?m)^    {name}[ \t]*=[ \t]*([0-9]+)[ \t]*(?:#.*)?\r?$")).unwrap();
        let values: Vec<_> = declaration.captures_iter(body).collect();
        assert_eq!(values.len(), 1, "StateMonitor deve declarar {name} uma vez");
        let python: u32 = values[0][1].parse().expect("limite Python deve caber em u32");
        assert_eq!(rust, python, "limite {name} precisa acompanhar a referência Python");
    }
}

#[test]
fn terminal_parsers_and_sequences_match_python() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../backend/tests/fixtures/contract");
    let rows: serde_json::Value = serde_json::from_slice(
        &std::fs::read(root.join("golden/terminal.json")).unwrap(),
    ).unwrap();
    let mut failures = Vec::new();
    for row in rows.as_array().unwrap() {
        if let Some(pane) = row["pane"].as_str() {
            let actual = serde_json::to_value(analyze(pane)).unwrap();
            if actual != row["expected"] {
                failures.push(format!("{}: Rust {actual}; Python {}", row["name"], row["expected"]));
            }
        } else {
            let mut memory = ReducerMemory::default();
            for (index, frame) in row["sequence"].as_array().unwrap().iter().enumerate() {
                let facts: ReducerFacts = serde_json::from_value(frame["facts"].clone()).unwrap();
                let result = reduce(frame["pane"].as_str().unwrap(), memory, facts);
                assert_eq!(serde_json::to_value(&result).unwrap(), row["expected_sequence"][index], "{} frame {}", row["name"], index);
                memory = result.memory;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
