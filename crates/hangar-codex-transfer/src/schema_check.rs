//! Métodos e campos do app-server que a importação usa existem no Codex instalado. Quem roda é o
//! workflow semanal `codex-schema` e o `scripts/conferir-codex-schema`, sobre a saída de
//! `codex app-server generate-json-schema --experimental`.
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::path::Path;

const METHODS: &[&str] = &[
    "initialize",
    "config/read",
    "thread/start",
    "thread/read",
    "thread/settings/update",
    "skills/list",
    "mcpServerStatus/list",
    "thread/inject_items",
];

// As chaves lidas do `config` além destas são livres: o `Config` do schema aceita chave extra.
const FIELDS: &[(&str, &[&str])] = &[
    ("InitializeParams", &["clientInfo", "capabilities"]),
    ("InitializeCapabilities", &["experimentalApi"]),
    ("ConfigReadParams", &["cwd", "includeLayers"]),
    ("ConfigReadResponse", &["config"]),
    (
        "Config",
        &[
            "developer_instructions",
            "instructions",
            "model_provider",
            "model_context_window",
            "model_auto_compact_token_limit",
        ],
    ),
    (
        "ThreadStartParams",
        &["approvalPolicy", "sandbox", "developerInstructions", "model"],
    ),
    (
        "ThreadStartResponse",
        &["thread", "cwd", "model", "modelProvider", "instructionSources"],
    ),
    ("Thread", &["id", "path", "cwd", "model", "reasoningEffort"]),
    (
        "ThreadSettingsUpdateParams",
        &["threadId", "effort", "collaborationMode"],
    ),
    ("CollaborationMode", &["mode", "settings"]),
    (
        "Settings",
        &["model", "reasoning_effort", "developer_instructions"],
    ),
    ("ThreadReadParams", &["threadId", "includeTurns"]),
    ("ThreadReadResponse", &["thread"]),
    ("SkillsListParams", &["cwds", "forceReload"]),
    ("SkillsListResponse", &["data"]),
    ("SkillsListEntry", &["cwd", "errors", "skills"]),
    ("SkillMetadata", &["enabled"]),
    ("ListMcpServerStatusParams", &["threadId", "cursor"]),
    ("ListMcpServerStatusResponse", &["data", "nextCursor"]),
    ("McpServerStatus", &["tools", "toolsError", "runtimeStatus"]),
    ("ThreadInjectItemsParams", &["threadId", "items"]),
];

fn props(def: &Value) -> BTreeSet<String> {
    let own = def["properties"].as_object().into_iter().flat_map(Map::keys);
    let branches = def["oneOf"]
        .as_array()
        .into_iter()
        .chain(def["anyOf"].as_array())
        .flatten()
        .flat_map(|b| b["properties"].as_object().into_iter().flat_map(Map::keys));
    own.chain(branches).cloned().collect()
}

fn missing(defs: &Map<String, Value>, methods: &BTreeSet<String>) -> Vec<String> {
    let mut out = Vec::new();
    for (name, fields) in FIELDS {
        match defs.get(*name) {
            None => out.push(format!("{name} (definição)")),
            Some(def) => {
                let theirs = props(def);
                out.extend(
                    fields
                        .iter()
                        .filter(|f| !theirs.contains(**f))
                        .map(|f| format!("{name}.{f}")),
                );
            }
        }
    }
    out.extend(
        METHODS
            .iter()
            .filter(|m| !methods.contains(**m))
            .map(|m| format!("método {m}")),
    );
    out
}

fn read(dir: &Path) -> (Map<String, Value>, BTreeSet<String>) {
    // v2 vence a raiz, que vence v1: o mesmo nome aparece em mais de uma pasta.
    let mut defs = Map::new();
    for sub in ["v2", "", "v1"] {
        let mut files: Vec<_> = std::fs::read_dir(dir.join(sub))
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension().is_some_and(|e| e == "json")
                    && !p.file_name().unwrap().to_string_lossy().starts_with("codex_app_server_protocol")
            })
            .collect();
        files.sort();
        for file in files {
            let value: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
            for (name, def) in value["definitions"].as_object().cloned().unwrap_or_default() {
                defs.entry(name).or_insert(def);
            }
            if let Some(title) = value["title"].as_str() {
                defs.entry(title.to_owned()).or_insert(value.clone());
            }
        }
    }
    let requests: Value =
        serde_json::from_slice(&std::fs::read(dir.join("ClientRequest.json")).unwrap()).unwrap();
    let methods = requests["oneOf"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|b| b["properties"]["method"]["enum"][0].as_str().map(str::to_owned))
        .collect();
    (defs, methods)
}

/// `CODEX_SCHEMA_DIR=<saída do generate-json-schema --experimental> cargo test -p
/// hangar-codex-transfer --lib -- --ignored fields_used_exist_in_installed_codex`.
#[test]
#[ignore]
fn fields_used_exist_in_installed_codex() {
    let dir = std::path::PathBuf::from(std::env::var("CODEX_SCHEMA_DIR").expect("CODEX_SCHEMA_DIR"));
    let (defs, methods) = read(&dir);
    let missing = missing(&defs, &methods);
    assert!(
        missing.is_empty(),
        "campos da transferência que não existem no Codex de {}: {missing:#?}",
        dir.display()
    );
}

#[test]
fn a_used_field_or_method_that_disappeared_is_reported() {
    let mut defs = Map::new();
    for (name, fields) in FIELDS {
        let properties: Map<String, Value> = fields
            .iter()
            .filter(|f| !(*name == "Thread" && **f == "path"))
            .map(|f| ((*f).to_owned(), Value::Object(Map::new())))
            .collect();
        defs.insert((*name).to_owned(), serde_json::json!({"properties": properties}));
    }
    let methods = METHODS
        .iter()
        .filter(|m| **m != "thread/inject_items")
        .map(|m| (*m).to_owned())
        .collect();
    assert_eq!(
        missing(&defs, &methods),
        ["Thread.path", "método thread/inject_items"]
    );
}
