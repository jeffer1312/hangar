//! Todo campo e todo método que os tipos de `proto` usam existem no schema da versão conferida.
//! O schema vem de `codex app-server generate-json-schema --experimental`; o recorte guarda só as
//! definições alcançadas a partir dos tipos usados. Regerar: `scripts/conferir-codex-schema`.
use crate::proto::*;
use serde_json::{Map,Value,json};
use std::collections::{BTreeMap,BTreeSet};

fn slice_path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("schema").join(format!("{}.json",crate::version::CHECKED))
}

/// Tipos nossos sem nome no schema → definições do schema que eles representam.
const LOCAL_NAMES:&[(&str,&[&str])] = &[
    ("CollaborationSettings",&["Settings"]),
    ("ThreadOnlyParams",&["PermissionsRequestApprovalParams","McpServerElicitationRequestParams"]),
];

fn schema_names(ours:&str) -> Vec<String> {
    LOCAL_NAMES.iter().find(|(name,_)|*name == ours).map_or_else(||vec![ours.to_owned()],|(_,names)|names.iter().map(|n|(*n).to_owned()).collect())
}

fn ours() -> Vec<Value> {
    macro_rules! s { ($($t:ty),*) => { vec![$(serde_json::to_value(schemars::schema_for!($t)).unwrap()),*] } }
    s!(ClientRequest,ServerNotification,ServerRequest,InitializeResponse,ThreadStartResponse,ThreadReadResponse,
       TurnStartResponse,ModelListResponse,GetAccountRateLimitsResponse,LoginAccountResponse,CancelLoginAccountResponse)
}

/// Definições nomeadas dos nossos tipos: nome → propriedades usadas.
fn our_defs() -> BTreeMap<String,BTreeSet<String>> {
    let mut out = BTreeMap::new();
    for root in ours() {
        let mut defs:Vec<(String,Value)> = root["$defs"].as_object().cloned().unwrap_or_default().into_iter().collect();
        if let Some(title) = root["title"].as_str() { defs.push((title.into(),root.clone())); }
        for (name,def) in defs {
            if let Some(props) = def["properties"].as_object() {
                out.entry(name).or_insert_with(BTreeSet::new).extend(props.keys().cloned());
            }
        }
    }
    out
}

/// Nossos ramos com tag (`type`/`method`): (enum, valor da tag) → propriedades além da tag.
fn our_tagged() -> BTreeMap<(String,String),BTreeSet<String>> {
    let mut out = BTreeMap::new();
    for root in ours() {
        let mut defs:Vec<(String,Value)> = root["$defs"].as_object().cloned().unwrap_or_default().into_iter().collect();
        if let Some(title) = root["title"].as_str() { defs.push((title.into(),root.clone())); }
        for (name,def) in defs {
            for branch in def["oneOf"].as_array().into_iter().chain(def["anyOf"].as_array()).flatten() {
                for tag in ["type","method"] {
                    let value = &branch["properties"][tag];
                    let Some(value) = value["const"].as_str().or_else(||value["enum"][0].as_str()) else { continue };
                    // O `#[serde(other)] Unknown` vira "unknown" com `rename_all`; não existe no schema.
                    if value.eq_ignore_ascii_case("unknown") { continue; }
                    let props = branch["properties"].as_object().map(|p|p.keys().filter(|k|*k != tag).cloned().collect()).unwrap_or_default();
                    out.insert((name.clone(),value.to_owned()),props);
                }
            }
        }
    }
    out
}

// Lido em tempo de execução: com `include_str!` o regerador não compila sem o recorte que ele cria.
fn slice() -> Value {
    let path = slice_path();
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e|panic!("{}: {e}; rode scripts/conferir-codex-schema",path.display()));
    serde_json::from_str(&text).expect("recorte inválido")
}

fn their_props(def:&Value) -> BTreeSet<String> {
    def["properties"].as_object().map(|p|p.keys().cloned().collect()).unwrap_or_default()
}

fn their_branch<'a>(def:&'a Value,tag:&str,value:&str) -> Option<&'a Value> {
    def["oneOf"].as_array().into_iter().chain(def["anyOf"].as_array()).flatten().find(|branch|{
        let v = &branch["properties"][tag];
        v["const"] == value || v["enum"].as_array().is_some_and(|e|e.iter().any(|x|x == value))
    })
}

#[test]
fn fields_used_exist_in_checked_schema() {
    let slice = slice();
    assert_eq!(slice["version"],crate::version::CHECKED);
    let defs = &slice["definitions"];
    let mut missing = Vec::new();
    for (ours,props) in our_defs() {
        for name in schema_names(&ours) {
            let def = &defs[&name];
            if def.is_null() { missing.push(format!("{name} (definição)")); continue; }
            let theirs = their_props(def);
            for prop in &props { if !theirs.contains(prop) { missing.push(format!("{name}.{prop}")); } }
        }
    }
    for ((name,value),props) in our_tagged() {
        if ["ClientRequest","ServerNotification","ServerRequest"].contains(&name.as_str()) {
            let methods = slice["methods"][&name].as_array().cloned().unwrap_or_default();
            if !methods.iter().any(|m|m == value.as_str()) { missing.push(format!("{name} método {value}")); }
            continue;
        }
        match their_branch(&defs[&name],"type",&value) {
            None => missing.push(format!("{name} tipo {value}")),
            Some(branch) => { let theirs = their_props(branch); for p in props { if !theirs.contains(&p) { missing.push(format!("{name}::{value}.{p}")); } } }
        }
    }
    assert!(missing.is_empty(),"campos usados pelo Hangar que não existem no Codex {}: {missing:#?}",crate::version::CHECKED);
}

/// `CODEX_SCHEMA_DIR=<saída do generate-json-schema --experimental> cargo test -p hangar-codex -- --ignored regenerate_slice`
#[test]
#[ignore]
fn regenerate_slice() {
    let dir = std::path::PathBuf::from(std::env::var("CODEX_SCHEMA_DIR").expect("CODEX_SCHEMA_DIR"));
    // v2 vence a raiz, que vence v1: o mesmo nome aparece em mais de uma pasta.
    let mut all:Map<String,Value> = Map::new();
    let mut methods = json!({});
    for sub in ["v2","","v1"] {
        let mut files:Vec<_> = std::fs::read_dir(dir.join(sub)).unwrap().flatten().map(|e|e.path())
            .filter(|p|p.extension().is_some_and(|e|e == "json") && !p.file_name().unwrap().to_string_lossy().starts_with("codex_app_server_protocol")).collect();
        files.sort();
        for file in files {
            let value:Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
            for (name,def) in value["definitions"].as_object().cloned().unwrap_or_default() { all.entry(name).or_insert(def); }
            if let Some(title) = value["title"].as_str() {
                let mut root = value.clone(); root.as_object_mut().unwrap().remove("definitions");
                if ["ClientRequest","ServerNotification","ServerRequest"].contains(&title) && sub.is_empty() {
                    methods[title] = json!(root["oneOf"].as_array().unwrap().iter().filter_map(|b|b["properties"]["method"]["enum"][0].as_str()).collect::<Vec<_>>());
                }
                all.entry(title.to_owned()).or_insert(root);
            }
        }
    }
    let mut wanted:Vec<String> = our_defs().into_keys().flat_map(|n|schema_names(&n)).collect();
    wanted.extend(our_tagged().into_keys().map(|(n,_)|n).filter(|n|!["ClientRequest","ServerNotification","ServerRequest"].contains(&n.as_str())));
    let mut out = Map::new();
    while let Some(name) = wanted.pop() {
        if out.contains_key(&name) { continue; }
        let Some(def) = all.get(&name).cloned() else { continue };
        let text = def.to_string();
        for part in text.split("\"$ref\":\"").skip(1) {
            if let Some(reference) = part.split('"').next() { wanted.push(reference.rsplit('/').next().unwrap().to_owned()); }
        }
        out.insert(name,def);
    }
    let slice = json!({"version":crate::version::CHECKED,"definitions":out,"methods":methods});
    let path = slice_path();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path,serde_json::to_string_pretty(&slice).unwrap() + "\n").unwrap();
}
