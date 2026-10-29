use std::collections::BTreeMap;
use serde_json::{Value, json};
use crate::accounts::AccountService;
use super::{store, proxy, probe::truthy};

#[derive(Default)]
pub struct Selection<'a> { pub model: Option<&'a str>, pub context: Option<Value>, pub account: Option<&'a str>, pub account_home: Option<&'a str>, pub account_base: Option<&'a str> }

pub fn catalog_model(model: &str) -> &str {
    static GPT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(||regex::Regex::new(r"\Agpt-\d[^/]*\[1m\]\z").unwrap());
    if GPT.is_match(model.rsplit('/').next().unwrap_or(model)) { model.strip_suffix("[1m]").unwrap_or(model) } else { model }
}

fn type_name(v: &Value) -> &'static str { match v { Value::String(_) => "str", Value::Number(n) => if n.is_f64() { "float" } else { "int" }, Value::Bool(_) => "bool", Value::Array(_) => "list", Value::Object(_) => "dict", Value::Null => "NoneType" } }
fn boolean(record: &Value, field: &str) -> Result<Option<bool>,String> {
    let v = &record[field];
    if v.is_null() { Ok(None) } else { v.as_bool().map(Some).ok_or_else(||format!("{field}: esperado true/false (recebido {})",type_name(v))) }
}

pub fn derive(accounts: &AccountService, name: &str, selection: Selection<'_>) -> Result<BTreeMap<String,String>,String> {
    let catalog = store::state(&accounts.env).0;
    let record = catalog.get(name).ok_or_else(||format!("motor '{name}' não existe"))?;
    for field in ["api_key","model","base_url","subagent_model"] {
        if record[field].as_str().is_some_and(|s|s.contains(['\n','\r','\0'])) { return Err(format!("{field}: contém caractere proibido (quebra de linha ou nulo)")); }
    }
    let mut model = selection.model.filter(|s|!s.is_empty()).or_else(||record["model"].as_str()).ok_or("model: esperado texto")?.to_owned();
    if model.contains(['\n','\r','\0']) { return Err("model: contém caractere proibido (quebra de linha ou nulo)".into()); }
    let mut subagent = record["subagent_model"].as_str().filter(|s|!s.is_empty()).unwrap_or(&model).to_owned();
    if truthy(&record["subagent_model"]) && !record["subagent_model"].is_string() { return Err("subagent_model: esperado texto".into()); }
    let base = record["base_url"].as_str().ok_or("base_url: esperado texto")?;
    let key = record["api_key"].as_str().ok_or("api_key: esperado texto")?;
    let mut env = BTreeMap::new();
    let mut window = if selection.model.is_some_and(|s|!s.is_empty()) { selection.context.clone() } else { record.get("context_window").cloned() };
    if let Some(account) = selection.account {
        let (Some(home),Some(expected)) = (selection.account_home.filter(|s|!s.is_empty()),selection.account_base.filter(|s|!s.is_empty())) else { return Err("CLIProxyAPI: conta fixa exige raiz Codex e endereço local validados".into()) };
        let expected = store::base_url(expected)?;
        if !reqwest::Url::parse(&expected).ok().and_then(|u|u.host_str().map(store::local_host)).unwrap_or(false) { return Err("CLIProxyAPI: conta fixa exige um endereço local".into()); }
        if store::base_url(base)? != expected { return Err("CLIProxyAPI: endereço do motor mudou; conta fixa não pode usar outro provedor".into()); }
        let account = proxy::resolve(accounts,account,Some(home))?;
        let prefix = account["prefix"].as_str().unwrap();
        model = proxy::prefix_model(&model,prefix)?; subagent = proxy::prefix_model(&subagent,prefix)?;
        if window.as_ref().is_none_or(Value::is_null) && model == proxy::prefix_model(record["model"].as_str().unwrap_or(""),prefix)? { window = record.get("context_window").cloned(); }
        env.insert("CP_ENGINE_ACCOUNT".into(),account["account"].as_str().unwrap().into());
        env.insert("CP_ENGINE_CREDENTIAL_ID".into(),account["credential_id"].as_str().unwrap().into());
        env.insert("CP_ENGINE_ACCOUNT_BASE_URL".into(),expected);
    }
    env.insert("CP_ENGINE".into(),name.into()); env.insert("ANTHROPIC_BASE_URL".into(),base.into()); env.insert("ANTHROPIC_AUTH_TOKEN".into(),key.into());
    for field in ["ANTHROPIC_MODEL","ANTHROPIC_DEFAULT_OPUS_MODEL","ANTHROPIC_DEFAULT_SONNET_MODEL","ANTHROPIC_DEFAULT_HAIKU_MODEL","ANTHROPIC_DEFAULT_FABLE_MODEL"] { env.insert(field.into(),model.clone()); }
    env.insert("CLAUDE_CODE_SUBAGENT_MODEL".into(),subagent);
    if boolean(record,"auth_via_api_key")? == Some(true) { env.insert("ANTHROPIC_CUSTOM_HEADERS".into(),format!("x-api-key: {key}")); }
    if catalog_model(&model) != model { window = Some(json!(1_000_000)); }
    if let Some(window) = window.filter(truthy) {
        let value = if window == true { json!(1) } else { window };
        env.insert("CLAUDE_CODE_MAX_CONTEXT_TOKENS".into(),store::positive("context_window",&value)?.to_string());
    }
    for (field,var,default,trigger,value) in [
        ("bundled_skills","CLAUDE_CODE_DISABLE_BUNDLED_SKILLS",false,false,"1"),
        ("tool_search","ENABLE_TOOL_SEARCH",false,false,"false"),
        ("experimental_betas","CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS",false,false,"1"),
        ("prompt_caching","DISABLE_PROMPT_CACHING",true,false,"1"),
        ("adaptive_thinking","CLAUDE_CODE_DISABLE_ADAPTIVE_THINKING",true,false,"1"),
        ("gateway_model_discovery","CLAUDE_CODE_ENABLE_GATEWAY_MODEL_DISCOVERY",false,true,"1"),
        ("fine_grained_tool_streaming","CLAUDE_CODE_ENABLE_FINE_GRAINED_TOOL_STREAMING",false,true,"1"),
    ] { if boolean(record,field)?.unwrap_or(default) == trigger { env.insert(var.into(),value.into()); } }
    for (field,var) in [("auto_compact_window","CLAUDE_CODE_AUTO_COMPACT_WINDOW"),("max_output_tokens","CLAUDE_CODE_MAX_OUTPUT_TOKENS")] {
        if truthy(&record[field]) { let value = if record[field] == true { json!(1) } else { record[field].clone() }; env.insert(var.into(),store::positive(field,&value)?.to_string()); }
    }
    Ok(env)
}

pub fn service_tier(tier: &str, extra: Option<&str>) -> Result<BTreeMap<String,String>,String> {
    if !matches!(tier,"default"|"priority") { return Err("service_tier: use default ou priority".into()); }
    let value: Value = serde_json::from_str(extra.unwrap_or("{}")).map_err(|_|"CLAUDE_CODE_EXTRA_BODY: JSON inválido")?;
    let mut body = value.as_object().ok_or("CLAUDE_CODE_EXTRA_BODY: esperado um objeto JSON")?.clone();
    body.insert("service_tier".into(),json!(tier));
    if tier == "default" && body.get("speed") == Some(&json!("fast")) { body.remove("speed"); }
    Ok(BTreeMap::from([("CP_ENGINE_SERVICE_TIER".into(),tier.into()),("CLAUDE_CODE_EXTRA_BODY".into(),Value::Object(body).to_string())]))
}
