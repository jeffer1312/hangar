//! Diagnóstico de configuração dos harnesses, sem executar o Python.
use crate::{accounts::environment::AccountEnvironment, engines::probe::truthy, orchestration::codec};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

// Configurações podem conter extras profundamente aninhados que a checagem não consulta.
struct Document(Value);
impl std::ops::Deref for Document { type Target = Value; fn deref(&self) -> &Value { &self.0 } }
impl Drop for Document { fn drop(&mut self) { codec::discard(std::mem::take(&mut self.0)); } }
fn read_json(path: &Path) -> Option<Document> {
    let text = std::fs::read_to_string(path).ok()?;
    codec::loads(&text).map(Document)
}
fn read(path: &Path, toml: bool) -> Document {
    if toml {
        let value = std::fs::read_to_string(path).ok()
            .and_then(|text| toml::from_str::<toml::Value>(&text).ok())
            .and_then(|value| serde_json::to_value(value).ok()).unwrap_or_else(|| json!({}));
        Document(value)
    } else {
        let value = read_json(path).unwrap_or_else(|| Document(json!({})));
        if value.is_object() { value } else { Document(json!({})) }
    }
}
fn item(id: &str, ok: Option<bool>, code: &str, repair: Option<&str>, info: bool, params: &[(&str, String)]) -> Value {
    let params: serde_json::Map<String, Value> = params.iter().map(|(k,v)|(k.to_string(),Value::String(v.clone()))).collect();
    json!({"id":id,"ok":ok,"codigo":code,"params":params,"conserto":repair,"info":info})
}
fn agent_dir(env: &AccountEnvironment, cli: &str) -> Result<PathBuf, String> {
    if cli == "omp" { env.omp_directories().map(|v|v.agent_dir).map_err(str::to_owned) }
    else { Ok(super::files::directory(env,cli)) }
}
fn config(env: &AccountEnvironment, cli: &str, json_name: &str) -> Result<Document, String> {
    let dir = agent_dir(env,cli)?;
    Ok(read(&dir.join(if matches!(cli,"codex"|"kimi") {"config.toml"} else {json_name}),matches!(cli,"codex"|"kimi")))
}
pub fn mcp(env: &AccountEnvironment, cli: &str) -> Result<Value, String> {
    let doc = if cli == "claude" { read(&env.home.join(".claude.json"),false) } else { config(env,cli,"mcp.json")? };
    let field = match cli {"codex"=>"mcp_servers","kimi" if truthy(&doc["mcp"])=>"mcp","kimi"=>"mcp_servers",_=>"mcpServers"};
    let servers = &doc[field];
    if !truthy(servers) { return Ok(item("mcp",Some(true),"mcp_nenhum",None,true,&[])); }
    let Some(servers) = servers.as_object() else { return Err("configuração MCP não é um objeto".into()); };
    let mut names: Vec<&str> = servers.keys().map(String::as_str).collect(); names.sort();
    Ok(item("mcp",Some(true),"mcp_ok",None,true,&[("n",names.len().to_string()),("lista",names.join(", "))]))
}
pub fn default_model(env: &AccountEnvironment, cli: &str) -> Result<Value, String> {
    let doc = if cli == "claude" {read(&env.home.join(".claude/settings.json"),false)} else {config(env,cli,"settings.json")?};
    let model = if matches!(cli,"pi"|"omp") {
        let mut parts = Vec::new();
        for key in ["defaultProvider","defaultModel"] {
            if truthy(&doc[key]) {parts.push(doc[key].as_str().ok_or("modelo padrão não é texto")?);}
        }
        parts.join("/")
    } else {
        let value = &doc[if cli=="kimi" {"default_model"} else {"model"}];
        if truthy(value) {codec::python_string(value)} else {String::new()}
    };
    Ok(if model.is_empty() {item("modelo",Some(true),"modelo_padrao_nenhum",None,true,&[])}
       else {item("modelo",Some(true),"modelo_padrao",None,true,&[("modelo",model)])})
}
pub fn codex_hooks(env: &AccountEnvironment) -> Value {
    let doc = read(&env.codex_default.join("hooks.json"),false);
    let Some(hooks) = doc["hooks"].as_object().filter(|h|!h.is_empty()) else {
        return item("hooks",Some(true),"hooks_nenhum",None,true,&[]);
    };
    let count: usize = hooks.values().filter_map(Value::as_array).map(Vec::len).sum();
    let mut events: Vec<&str> = hooks.keys().map(String::as_str).collect(); events.sort();
    item("hooks",Some(true),"hooks_codex",None,true,&[("n",count.to_string()),("eventos",events.join(", "))])
}
pub fn fullscreen(env: &AccountEnvironment, cli: &str) -> Value {
    if cli == "claude" {
        let path = env.home.join(".claude/settings.json");
        if !path.is_file() {return item("fullscreen",None,"config_ilegivel",None,false,&[]);}
        let doc = read(&path,false);
        let enabled = doc["tui"].as_str()==Some("fullscreen");
        return item("fullscreen",Some(enabled),if enabled {"fullscreen_ok"} else {"fullscreen_claude_desligado"},if enabled {None} else {Some("fullscreen:claude")},false,&[]);
    }
    let Ok(dir) = agent_dir(env,cli) else {return item("fullscreen",None,"config_ilegivel",None,false,&[]);};
    let path = dir.join("fullscreen-tui.json");
    if !path.is_file() {return item("fullscreen",Some(false),"fullscreen_desligado",Some(&format!("fullscreen:{cli}")),false,&[]);}
    let Some(doc) = read_json(&path).filter(|v|v.is_object()) else {return item("fullscreen",None,"config_ilegivel",None,false,&[]);};
    item("fullscreen",Some(true),if doc["enabled"].as_bool()==Some(true) {"fullscreen_ok"} else {"fullscreen_por_escolha"},None,false,&[])
}
