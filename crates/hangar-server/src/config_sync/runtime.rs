//! Preferências do servidor e campos externos, com leitura independente das contas do agente.
use super::preferences;
use crate::orchestration::truthy;
use crate::accounts::environment::AccountEnvironment;
use serde_json::{Map, Value, json};
use std::{collections::{BTreeMap,BTreeSet}, path::PathBuf, sync::{Arc, LazyLock, Mutex}, time::SystemTime};
use crate::machine_peers::dotenv::{SettingsEnvironment, SettingsSnapshot};
use super::runtime_values::{Values, ValueGuard};
use crate::orchestration::codec;
use super::runtime_error::{RuntimeError, StorageOperation};

pub const SECRETS: &[&str] = &["groq_api_key","elevenlabs_api_key","llm_api_key","llm_briefing_api_key","jev_api_key","jev_texto_api_key","jev_aux_api_key"];
type Cache = BTreeMap<PathBuf, (SystemTime,u64,Values)>;
static CACHE: LazyLock<Mutex<Cache>> = LazyLock::new(||Mutex::new(BTreeMap::new()));
static WRITES: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct RuntimeConfig {
    pub env: AccountEnvironment,
    pub path: PathBuf,
    pub settings: Arc<SettingsSnapshot>,
    defaults: Map<String,Value>,
}
impl RuntimeConfig {
    pub fn new(env: AccountEnvironment) -> Result<Self,String> {
        let backend=crate::install_root::root(&env.home).map(|root|root.join("backend"));
        let snapshot=SettingsSnapshot::capture(&env.home,backend.as_deref(),SettingsEnvironment::from_pairs(env.ordered_pairs()))?;
        Self::from_snapshot(env,Arc::new(snapshot))
    }
    pub fn from_snapshot(env: AccountEnvironment,settings:Arc<SettingsSnapshot>)->Result<Self,String> {
        let defaults=super::runtime_defaults::load(&settings)?;
        Ok(Self {path:env.claude_base.join("runtime-config.json"),env,settings,defaults})
    }
    pub fn from_state(state:&crate::routes::AppState)->Result<Self,String> {
        Self::from_snapshot(state.accounts.env.clone(),state.settings_snapshot()?)
    }
    pub fn read(&self) -> Map<String,Value> {
        read_at(&self.path)
    }
    fn environment(&self, key:&str) -> Value {
        self.defaults.get(key).cloned().unwrap_or(Value::Null)
    }
    pub fn get(&self,key:&str)->Value {
        let current=Values(self.read());
        self.effective(key,&current)
    }
    /// Configuração efetiva compartilhada pela transcrição e pela organização do ditado.
    pub(crate) fn transcription_snapshot(&self) -> ValueGuard {
        // Uma leitura só: uma escrita no meio não mistura duas versões do arquivo.
        let current = Values(self.read());
        let get = |key: &str| self.effective(key, &current);
        let value_or = |key: &str, default: Value| {
            let value = get(key);
            if truthy(&value) { value } else { default }
        };
        let mut providers = ValueGuard(value_or("transcription_providers", json!([])));
        let key = ValueGuard(get("groq_api_key"));
        let key = crate::transcript::py::strip(key.as_str().unwrap_or(""));
        let legacy = if !truthy(&providers) && !key.is_empty() {
            json!({"id":"legacy", "kind":"openai", "api_key":key,
                "base_url":value_or("transcription_base_url",json!("")),
                "model":value_or("transcription_model",json!(""))})
        } else { Value::Null };
        let mut organization = ValueGuard(json!({"dictation_include_recent_messages":
            truthy(&get("dictation_include_recent_messages"))}));
        for field in ["dictation_organization_mode", "dictation_claude_model", "dictation_codex_model",
            "ditado_estilo", "llm_base_url", "llm_api_key", "llm_model", "llm_reasoning_effort",
            "llm_briefing_base_url", "llm_briefing_api_key", "llm_briefing_model"] {
            organization[field] = value_or(field,json!(""));
        }
        if organization["dictation_organization_mode"] != "external_api" {
            for field in ["llm_base_url", "llm_api_key", "llm_briefing_base_url", "llm_briefing_api_key"] {
                organization[field] = json!("");
            }
        }
        // Mesma conversão do `int()` da rota Python: "12" salvo como texto vale 12.
        let max_minutes = value_or("dictation_max_minutes", json!(0));
        let max_minutes = crate::value_coercion::integer(&max_minutes).unwrap_or(max_minutes);
        ValueGuard(json!({"providers":providers.take(), "legacy":legacy,
            "organization":organization.take(),
            "user_vocabulary":value_or("ditado_vocabulario",json!("")),
            "dictation_max_minutes":max_minutes,
            "state_path":self.env.home.join(".hangar/transcription-wait.json").to_string_lossy()}))
    }
    fn effective(&self,key:&str,current:&Map<String,Value>)->Value {
        if preferences::editable(key) {
            if let Some(value)=current.get(key) {return codec::clone_value(value);}
            if super::legacy::AUX.contains(&key) && !super::legacy::AUX.iter().any(|k|current.contains_key(*k)) {
                return self.legacy_aux(current,false).ok().and_then(|values|values.get(key).cloned()).unwrap_or_else(||json!(""));
            }
        }
        self.environment(key)
    }
    fn thinking(&self)->Option<Values> {
        main_claude_settings(&self.env.home).map(Values)
    }
    pub fn state(&self)->Result<Value,String> {
        let current=Values(self.read());
        let mut result=Values(Map::new());
        for key in preferences::BOOLEAN.iter().chain(preferences::INTEGER).chain(preferences::TEXT).copied().chain(["transcription_providers"]) {
            let mut value=ValueGuard(self.effective(key,&current));
            let origin=if current.contains_key(key){"app"}else{"env"};
            let (value,defined)=if key=="transcription_providers" {
                let mut list=Vec::new();
                for provider in value.as_array().into_iter().flatten().filter(|p|p.is_object()) {
                    let masked=match mask_value(&provider["api_key"]) {
                        Ok(masked)=>masked,
                        Err(error)=>{ for item in list {codec::discard(item);} return Err(error.message); }
                    };
                    let mut provider=Values::copied(provider.as_object().unwrap());
                    provider.insert("api_key".into(),json!(masked));
                    list.push(Value::Object(provider.into_map()));
                }
                let defined=!list.is_empty();(Value::Array(list),defined)
            }else if SECRETS.contains(&key) {(json!(mask_value(&value).map_err(|error|error.message)?),truthy(&value))}
            else {let defined=!value.is_null();(std::mem::replace(&mut value.0,Value::Null),defined)};
            let mut field=json!({"valor":null,"definido":defined,"origem":origin});
            field["valor"]=value;
            result.insert(key.into(),field);
        }
        let thinking=self.thinking();
        let value=thinking.as_ref().and_then(|m|m.get("showThinkingSummaries"));
        result.insert("mostrar_pensamento".into(),json!({"valor":value.is_some_and(truthy),"definido":true,"origem":if value.is_some(){"app"}else{"env"}}));
        if let Some(settings)=thinking.as_ref() {
            let idle=settings.get("idleCompaction");
            result.insert("compactar_parada".into(),json!({"valor":idle.is_none_or(truthy),"definido":true,"origem":if idle.is_some(){"app"}else{"env"}}));
            let min=settings.get("env").and_then(|env|env.get(IDLE_COMPACT_MIN_ENV));
            result.insert("compactar_parada_min_mil".into(),json!({"valor":idle_compact_min_tokens(settings)/1000,"definido":true,"origem":if min.is_some(){"app"}else{"env"}}));
        }
        Ok(Value::Object(result.into_map()))
    }
    pub fn apply(&self, changes:&Map<String,Value>, remove:&BTreeSet<String>)->Result<Map<String,Value>,String> {
        self.apply_checked(changes, remove).map_err(|error|error.message)
    }
    pub fn apply_checked(&self, changes:&Map<String,Value>, remove:&BTreeSet<String>)->Result<Map<String,Value>,RuntimeError> {
        let _guard=WRITES.lock().unwrap_or_else(|e|e.into_inner());
        let mut current=read_for_write(&self.path)?;
        let mut requested=Values::copied(changes);
        let proxy = requested.shift_remove("jev_aux_cliproxy").unwrap_or(Value::Null);
        let import_proxy = proxy == Value::Bool(true);
        codec::discard(proxy);
        if import_proxy {
            let key=crate::cliproxy_config::client_key(&self.env)
                .ok_or("jev_aux_cliproxy: o ~/.cli-proxy-api/config.yaml não tem nenhuma api-key")?;
            requested.insert("jev_aux_api_key".into(),json!(key));
        }
        let changes=&requested;
        let touches_aux=super::legacy::AUX.iter().any(|key|changes.contains_key(*key)||remove.contains(*key));
        if touches_aux&&!super::legacy::AUX.iter().any(|key|current.contains_key(*key)) {
            let legacy=self.legacy_aux(&current,true)?;
            current.extend(legacy.into_iter().filter(|(_,value)|truthy(value)));
        }
        if changes.get("jev_windows_api_key").and_then(Value::as_str).is_some_and(|s|!crate::transcript::py::strip(s).is_empty()&&!s.contains('•')) {
            return Err("jev_windows_api_key: a chave do Computer Use agora é a da página Jev".into());
        }
        for key in remove {if preferences::editable(key) && let Some(value)=current.shift_remove(key){codec::discard(value);}}
        let mut thinking=None;
        let mut idle=None;
        let mut idle_min=None;
        for (key,value) in changes.iter() {
            if key=="mostrar_pensamento" {
                thinking=Some(ValueGuard(codec::clone_value(value)));
                continue;
            }
            if key=="compactar_parada" {
                idle=Some(value.as_bool().ok_or("compactar_parada: esperado true/false")?);
                continue;
            }
            if key=="compactar_parada_min_mil" {
                // Os campos numéricos dos apps chegam como texto, como nos outros inteiros de `preferences::coerce`.
                let thousands=value.as_u64().or_else(||value.as_str().and_then(|s|s.trim().parse().ok()))
                    .ok_or("compactar_parada_min_mil: esperado um número inteiro")?;
                if thousands < IDLE_COMPACT_FLOOR/1000 {
                    return Err(format!("compactar_parada_min_mil: o Claude não compacta abaixo de {} mil tokens",IDLE_COMPACT_FLOOR/1000).into());
                }
                idle_min=Some(thousands.saturating_mul(1000));
                continue;
            }
            if !preferences::editable(key) {continue;}
            let mut value=ValueGuard(codec::clone_value(value));
            if key=="transcription_providers" {
                if let Some(items)=value.as_array_mut() {
                    let old=current.get(key).and_then(Value::as_array);
                    for (index,item) in items.iter_mut().enumerate() {
                        if let Some(incoming)=item.get("api_key").and_then(Value::as_str) {
                            let previous=old.into_iter().flatten().filter(|p|p.is_object()).rev().find(|p|p.get("id")==item.get("id"));
                            if let Some(previous)=previous {
                                let secret=previous["api_key"].as_str().unwrap_or("");
                                if !secret.is_empty() && incoming.trim()==crate::engines::store::mask(secret) {
                                    let kind=item["kind"].as_str().unwrap_or("").trim();
                                    if previous["kind"].as_str()!=Some(kind) || (kind!="elevenlabs" && item["base_url"].as_str().unwrap_or("").trim()!=previous["base_url"].as_str().unwrap_or("")) {
                                        return Err(format!("transcription_providers: item {} mudou de tipo ou endpoint; digite a chave de novo",index+1).into());
                                    }
                                    item["api_key"]=json!(secret);
                                }
                            }
                        }
                    }
                }
            }
            if SECRETS.contains(&key.as_str()) {
                let effective=ValueGuard(self.effective(key,&current));
                if let Some(incoming)=value.as_str().filter(|_|!codec::contains_nonfinite(&value)) {
                    let masked=mask_value(&effective)?;
                    let incoming=crate::transcript::py::strip(incoming);
                    if truthy(&effective) && (incoming.is_empty()||incoming==masked) {continue;}
                }
            }
            if preferences::INTEGER.contains(&key.as_str()) && codec::infinite(&value) {
                return Err(RuntimeError::new("OverflowError","cannot convert float infinity to integer"));
            }
            current.insert(key.clone(),preferences::coerce(key,&value)?);
        }
        let thinking=thinking.map(|value|value.as_bool().ok_or("mostrar_pensamento: esperado true/false")).transpose()?;
        write_values(&self.path, &current)?;
        // O settings.json do Claude vem depois: falhou, a resposta diz que o resto ficou gravado.
        let rest_saved=|error:RuntimeError|format!("demais ajustes salvos; {}",error.message);
        if let Some(value)=thinking {
            self.write_thinking(value).map_err(rest_saved)?;
        }
        if idle.is_some() || idle_min.is_some() {
            self.write_idle_compaction(idle,idle_min).map_err(rest_saved)?;
        }
        if touches_aux||["jev_api_key","jev_endpoint","jev_model"].iter().any(|key|changes.contains_key(*key)||remove.contains(*key)) {
            self.sync_jev().map_err(|error|format!("jev: salvo, mas o Computer Use não recebeu a configuração: {error}"))?;
        }
        Ok(current.into_map())
    }
    fn write_thinking(&self,value:bool)->Result<(),RuntimeError> {
        let mut data=self.thinking().ok_or("mostrar_pensamento: settings.json ilegível — não mexi nele")?;
        if data.get("showThinkingSummaries").is_some_and(|v|truthy(v)==value) {return Ok(());}
        data.insert("showThinkingSummaries".into(),json!(value));
        self.write_claude_settings(data,"mostrar_pensamento")
    }
    /// O ajuste é o do próprio Claude Code: o Hangar só o escreve e o segue ao compactar a sessão sem terminal.
    fn write_idle_compaction(&self,enabled:Option<bool>,min_tokens:Option<u64>)->Result<(),RuntimeError> {
        let mut data=self.thinking().ok_or("compactar_parada: settings.json ilegível — não mexi nele")?;
        if let Some(enabled)=enabled { data.insert("idleCompaction".into(),json!(enabled)); }
        if let Some(min)=min_tokens {
            let mut env=match data.get("env") { Some(Value::Object(env))=>env.clone(), Some(_)=>return Err("compactar_parada_min_mil: o `env` do settings.json não é um objeto — não mexi nele".into()), None=>Map::new() };
            if min==IDLE_COMPACT_DEFAULT { env.shift_remove(IDLE_COMPACT_MIN_ENV); } else { env.insert(IDLE_COMPACT_MIN_ENV.into(),json!(min.to_string())); }
            if env.is_empty() { data.shift_remove("env"); } else { data.insert("env".into(),Value::Object(env)); }
        }
        self.write_claude_settings(data,"compactar_parada")
    }
    fn write_claude_settings(&self,data:Values,field:&str)->Result<(),RuntimeError> {
        let document=ValueGuard(Value::Object(data.into_map()));
        let text=codec::dumps_indented_utf8(&document,2).map_err(|_|RuntimeError::new(
            "UnicodeEncodeError","settings.json contém surrogate Unicode isolado; arquivo preservado"))?;
        let path=self.env.home.join(".claude/settings.json");
        let mode=super::files::mode(&path).unwrap_or(0o600);
        super::files::write_file(&path,&super::bundle::Blob {
            data:format!("{text}\n").into_bytes(),mode,
        }).map_err(|e|RuntimeError::from(format!("{field}: {e}")))
    }
}

pub const IDLE_COMPACT_MIN_ENV:&str = "CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS";
/// Mínimo e piso do próprio Claude Code para a compactação da sessão parada.
pub const IDLE_COMPACT_DEFAULT:u64 = 200_000;
pub const IDLE_COMPACT_FLOOR:u64 = 100_000;

pub fn idle_compact_min_tokens(settings:&Map<String,Value>) -> u64 {
    settings.get("env").and_then(|env|env.get(IDLE_COMPACT_MIN_ENV))
        .and_then(|v|v.as_u64().or_else(||v.as_str().and_then(|s|s.trim().parse().ok())))
        .unwrap_or(IDLE_COMPACT_DEFAULT).max(IDLE_COMPACT_FLOOR)
}

/// Mínimo de tokens para compactar a sessão parada; `None` = o Claude não compactaria (desligado ou sem autocompact).
pub fn idle_compaction(settings:&Map<String,Value>) -> Option<u64> {
    let off=|key:&str|settings.get(key).is_some_and(|v|!truthy(v));
    // Como o Claude lê a variável: só 1/true/yes/on liga; "0" e "false" não desligam o autocompact.
    let env_on=|key:&str|settings.get("env").and_then(|env|env.get(key)).is_some_and(|v|match v {
        Value::String(s)=>["1","true","yes","on"].contains(&s.trim().to_ascii_lowercase().as_str()),
        other=>other.as_u64()==Some(1) || other.as_bool()==Some(true),
    });
    if off("idleCompaction") || off("autoCompact") || env_on("DISABLE_AUTO_COMPACT") || env_on("DISABLE_COMPACT") { return None; }
    Some(idle_compact_min_tokens(settings))
}

/// O `settings.json` principal, de onde o espelho das contas copia: ilegível = `None`.
pub fn main_claude_settings(home:&std::path::Path) -> Option<Map<String,Value>> {
    match std::fs::read_to_string(home.join(".claude/settings.json")) {
        Ok(raw) if crate::transcript::py::strip(&raw).is_empty()=>Some(Map::new()),
        Ok(raw)=>Values::read(crate::transcript::py::strip(&raw).as_bytes()).map(Values::into_map),
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Some(Map::new()),
        Err(_)=>None,
    }
}
fn read_at(path: &std::path::Path) -> Map<String, Value> {
    let read = || -> Option<Map<String,Value>> {
        let version = path.metadata().ok().and_then(|metadata|metadata.modified().ok().map(|modified|(modified,metadata.len())));
        if let Some(version) = version {
            if let Some((modified,size,data)) = CACHE.lock().unwrap_or_else(|e|e.into_inner()).get(path) {
                if (*modified,*size) == version {return Some(data.clone().into_map());}
            }
        }
        // A troca no Windows pode esconder o caminho antes mesmo de obter metadata.
        let raw = crate::runtime::queue::read_replaced(path).ok()?;
        let value = codec::loads(std::str::from_utf8(&raw).ok()?)?;
        let data = match value {Value::Object(data)=>Values(data),other=>{codec::discard(other);Values(Map::new())}};
        if let Some(version) = version {
            CACHE.lock().unwrap_or_else(|e|e.into_inner()).insert(path.to_owned(),(version.0,version.1,data.clone()));
        }
        Some(data.into_map())
    };
    read().unwrap_or_default()
}
fn read_for_write(path: &std::path::Path) -> Result<Values, RuntimeError> {
    match crate::runtime::queue::read_replaced(path) {
        Ok(raw) => Values::read(&raw).ok_or_else(|| RuntimeError::new("ValueError", "runtime-config.json ilegível; arquivo preservado").storage(StorageOperation::Read)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Values(Map::new())),
        Err(error) => Err(RuntimeError::io(&error, StorageOperation::Read)),
    }
}
fn write_values(path: &std::path::Path, current: &Values) -> Result<(), RuntimeError> {
    let document = ValueGuard(Value::Object(current.clone().into_map()));
    let encoded = codec::dumps_indented_utf8(&document, 2).map_err(|_| RuntimeError::new(
        "UnicodeEncodeError", "runtime-config.json contém surrogate Unicode isolado; arquivo preservado"
    ).storage(StorageOperation::Write))?;
    super::files::write_file(path, &super::bundle::Blob { data: encoded.into_bytes(), mode: 0o600 })
        .map_err(|error| RuntimeError { kind: "OSError", message: error.to_string(), os_error: error.os_error, operation: Some(StorageOperation::Write) })?;
    CACHE.lock().unwrap_or_else(|e|e.into_inner()).remove(path);
    Ok(())
}
pub(crate) fn set_sync(path: &std::path::Path, enabled: bool) -> Result<(), RuntimeError> {
    let _guard = WRITES.lock().unwrap_or_else(|e|e.into_inner());
    let mut current = read_for_write(path)?;
    current.insert("sync".into(), json!(enabled));
    write_values(path, &current)
}
pub(crate) fn sync_enabled(path: &std::path::Path, fallback: bool) -> bool {
    Values(read_at(path)).get("sync").map_or(fallback, truthy)
}
fn mask_value(value:&Value)->Result<String,RuntimeError> {
    if !truthy(value) {return Ok(String::new());}
    match value {
        Value::String(text) if !codec::contains_nonfinite(value)=>Ok(crate::engines::store::mask(text)),
        Value::Array(values) if values.len()<=8=>Ok("•".repeat(values.len())),
        Value::Object(values) if values.len()<=8=>Ok("•".repeat(values.len())),
        Value::Array(values)=>{
            let first=ValueGuard(Value::Array(values[..4].iter().map(codec::clone_value).collect()));
            let last=ValueGuard(Value::Array(values[values.len()-4..].iter().map(codec::clone_value).collect()));
            Ok(format!("{}••••••••{}",codec::python_string(&first),codec::python_string(&last)))
        },
        Value::Object(_)=>Err(RuntimeError::new("KeyError","slice(None, 4, None)")),
        _=>{
            let kind=match value {
                Value::Bool(_)=>"bool",
                Value::Number(number) if !number.is_f64()=>"int",
                _=>"float",
            };
            Err(RuntimeError::new("TypeError",format!("object of type '{kind}' has no len()")))
        }
    }
}

#[cfg(test)]
mod idle_compaction_tests {
    use super::*;

    fn settings(value:Value) -> Map<String,Value> { value.as_object().unwrap().clone() }

    #[test]
    fn follows_the_claude_setting_and_floor() {
        assert_eq!(idle_compaction(&settings(json!({}))),Some(IDLE_COMPACT_DEFAULT));
        assert_eq!(idle_compaction(&settings(json!({"idleCompaction":false}))),None);
        assert_eq!(idle_compaction(&settings(json!({"autoCompact":false}))),None);
        assert_eq!(idle_compaction(&settings(json!({"env":{"DISABLE_AUTO_COMPACT":"1"}}))),None);
        assert_eq!(idle_compaction(&settings(json!({"env":{"DISABLE_AUTO_COMPACT":"0","DISABLE_COMPACT":"false"}}))),Some(IDLE_COMPACT_DEFAULT));
        assert_eq!(idle_compaction(&settings(json!({"env":{"CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS":"150000"}}))),Some(150_000));
        assert_eq!(idle_compaction(&settings(json!({"env":{"CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS":"5000"}}))),Some(IDLE_COMPACT_FLOOR));
    }
}
