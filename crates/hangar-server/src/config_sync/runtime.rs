//! Preferências do servidor e campos externos, com leitura independente das contas do agente.
use super::{apply::truthy, preferences};
use crate::accounts::environment::AccountEnvironment;
use serde_json::{Map, Value, json};
use std::{collections::{BTreeMap,BTreeSet}, path::PathBuf, sync::{Arc, LazyLock, Mutex}, time::SystemTime};
use crate::machine_peers::dotenv::{SettingsEnvironment, SettingsSnapshot};
use super::runtime_values::{Values, ValueGuard};
use crate::orchestration::codec;

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
        let read = || -> Option<Map<String,Value>> {
            let metadata = self.path.metadata().ok()?;
            let version = (metadata.modified().ok()?,metadata.len());
            if let Some((modified,size,data)) = CACHE.lock().unwrap_or_else(|e|e.into_inner()).get(&self.path) {
                if (*modified,*size) == version {return Some(data.clone().into_map());}
            }
            let raw = std::fs::read(&self.path).ok()?;
            let value = codec::loads(std::str::from_utf8(&raw).ok()?)?;
            let data = match value {Value::Object(data)=>Values(data),other=>{codec::discard(other);Values(Map::new())}};
            CACHE.lock().unwrap_or_else(|e|e.into_inner()).insert(self.path.clone(),(version.0,version.1,data.clone()));
            Some(data.into_map())
        };
        read().unwrap_or_default()
    }
    fn environment(&self, key:&str) -> Value {
        self.defaults.get(key).cloned().unwrap_or(Value::Null)
    }
    pub fn get(&self,key:&str)->Value {
        let current=Values(self.read());
        self.effective(key,&current)
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
    fn thinking(&self)->Option<Map<String,Value>> {
        let path=self.env.home.join(".claude/settings.json");
        match std::fs::read_to_string(path) {
            Ok(raw) if raw.trim().is_empty()=>Some(Map::new()),
            Ok(raw)=>serde_json::from_str::<Value>(&raw).ok()?.as_object().cloned(),
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Some(Map::new()),
            Err(_)=>None,
        }
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
                        Err(error)=>{ for item in list {codec::discard(item);} return Err(error); }
                    };
                    let mut provider=Values::copied(provider.as_object().unwrap());
                    provider.insert("api_key".into(),json!(masked));
                    list.push(Value::Object(provider.into_map()));
                }
                let defined=!list.is_empty();(Value::Array(list),defined)
            }else if SECRETS.contains(&key) {(json!(mask_value(&value)?),truthy(&value))}
            else {let defined=!value.is_null();(std::mem::replace(&mut value.0,Value::Null),defined)};
            let mut field=json!({"valor":null,"definido":defined,"origem":origin});
            field["valor"]=value;
            result.insert(key.into(),field);
        }
        let thinking=self.thinking();
        let value=thinking.as_ref().and_then(|m|m.get("showThinkingSummaries"));
        result.insert("mostrar_pensamento".into(),json!({"valor":value.is_some_and(truthy),"definido":true,"origem":if value.is_some(){"app"}else{"env"}}));
        Ok(Value::Object(result.into_map()))
    }
    pub fn apply(&self, changes:&Map<String,Value>, remove:&BTreeSet<String>)->Result<Map<String,Value>,String> {
        let _guard=WRITES.lock().unwrap_or_else(|e|e.into_inner());
        let mut current=match std::fs::read(&self.path) {
            Ok(raw)=>Values::read(&raw).ok_or("runtime-config.json ilegível; arquivo preservado")?,
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Values(Map::new()),
            Err(error)=>return Err(error.to_string()),
        };
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
        if changes.get("jev_windows_api_key").and_then(Value::as_str).is_some_and(|s|!s.trim().is_empty()&&!s.contains('•')) {
            return Err("jev_windows_api_key: a chave do Computer Use agora é a da página Jev".into());
        }
        for key in remove {if preferences::editable(key) && let Some(value)=current.shift_remove(key){codec::discard(value);}}
        let mut thinking=None;
        for (key,value) in changes.iter() {
            if key=="mostrar_pensamento" {
                thinking=Some(value.as_bool().ok_or("mostrar_pensamento: esperado true/false")?);
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
                                if !secret.is_empty() && incoming.trim()==mask(secret) {
                                    let kind=item["kind"].as_str().unwrap_or("").trim();
                                    if previous["kind"].as_str()!=Some(kind) || (kind!="elevenlabs" && item["base_url"].as_str().unwrap_or("").trim()!=previous["base_url"].as_str().unwrap_or("")) {
                                        return Err(format!("transcription_providers: item {} mudou de tipo ou endpoint; digite a chave de novo",index+1));
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
                if let (Some(incoming),Some(secret))=(value.as_str(),effective.as_str()) {
                    if !secret.is_empty() && (incoming.trim().is_empty()||incoming.trim()==mask(secret)) {continue;}
                }
            }
            current.insert(key.clone(),preferences::coerce(key,&value)?);
        }
        if let Some(value)=thinking {self.write_thinking(value)?;}
        let document = Value::Object(current.clone().into_map());
        let encoded = codec::dumps_indented_utf8(&document,2);
        codec::discard(document);
        let encoded = encoded.map_err(|_|"runtime-config.json contém surrogate Unicode isolado; arquivo preservado")?;
        super::files::write_file(&self.path,&super::bundle::Blob {
            data:encoded.into_bytes(),mode:0o600,
        }).map_err(|e|e.to_string())?;
        CACHE.lock().unwrap_or_else(|e|e.into_inner()).remove(&self.path);
        if touches_aux||["jev_api_key","jev_endpoint","jev_model"].iter().any(|key|changes.contains_key(*key)||remove.contains(*key)) {
            self.sync_jev().map_err(|error|format!("jev: salvo, mas o Computer Use não recebeu a configuração: {error}"))?;
        }
        Ok(current.into_map())
    }
    fn write_thinking(&self,value:bool)->Result<(),String> {
        let mut data=self.thinking().ok_or("mostrar_pensamento: settings.json ilegível — não mexi nele")?;
        if data.get("showThinkingSummaries").is_some_and(|v|truthy(v)==value) {return Ok(());}
        data.insert("showThinkingSummaries".into(),json!(value));
        let path=self.env.home.join(".claude/settings.json");
        let mode=super::files::mode(&path).unwrap_or(0o600);
        super::files::write_file(&path,&super::bundle::Blob {
            data:format!("{}\n",crate::transcript::pyjson::dumps_indent(&Value::Object(data),2,false)).into_bytes(),mode,
        }).map_err(|e|format!("mostrar_pensamento: {e}"))
    }
}
fn mask_value(value:&Value)->Result<String,String> {
    if !truthy(value) {return Ok(String::new());}
    match value {
        Value::String(text) if !codec::contains_nonfinite(value)=>Ok(mask(text)),
        Value::Array(values) if values.len()<=8=>Ok("•".repeat(values.len())),
        Value::Object(values) if values.len()<=8=>Ok("•".repeat(values.len())),
        Value::Array(_)=>Err("can only concatenate list (not \"str\") to list".into()),
        Value::Object(_)=>Err("slice(None, 4, None)".into()),
        _=>{
            let kind=match value {
                Value::Bool(_)=>"bool",
                Value::Number(number) if !number.is_f64()=>"int",
                _=>"float",
            };
            Err(format!("object of type '{kind}' has no len()"))
        }
    }
}
pub fn mask(value:&str)->String {
    let count=codec::string_length(value);
    if count<=8 {return "•".repeat(count);}
    let prefix=codec::truncate_string(value,4);
    let start=codec::truncate_string(value,count-4).len();
    format!("{prefix}••••••••{}",&value[start..])
}
