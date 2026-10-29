//! Preferências do servidor e campos externos, com leitura independente das contas do agente.
use super::{apply::truthy, preferences};
use crate::accounts::environment::AccountEnvironment;
use serde_json::{Map, Value, json};
use std::{collections::{BTreeMap,BTreeSet}, path::PathBuf, sync::{Arc, LazyLock, Mutex}, time::SystemTime};
use crate::machine_peers::dotenv::{SettingsEnvironment, SettingsSnapshot};

pub const SECRETS: &[&str] = &["groq_api_key","elevenlabs_api_key","llm_api_key","llm_briefing_api_key","jev_api_key","jev_texto_api_key","jev_aux_api_key"];
type Cache = BTreeMap<PathBuf, (SystemTime,u64,Map<String,Value>)>;
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
                if (*modified,*size) == version {return Some(data.clone());}
            }
            let data = serde_json::from_slice::<Value>(&std::fs::read(&self.path).ok()?).ok()?.as_object().cloned().unwrap_or_default();
            CACHE.lock().unwrap_or_else(|e|e.into_inner()).insert(self.path.clone(),(version.0,version.1,data.clone()));
            Some(data)
        };
        read().unwrap_or_default()
    }
    fn environment(&self, key:&str) -> Value {
        self.defaults.get(key).cloned().unwrap_or(Value::Null)
    }
    pub fn get(&self,key:&str)->Value {
        let current=self.read();
        self.effective(key,&current)
    }
    fn effective(&self,key:&str,current:&Map<String,Value>)->Value {
        if preferences::editable(key) {
            if let Some(value)=current.get(key) {return value.clone();}
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
    pub fn state(&self)->Value {
        let current=self.read();
        let mut result=Map::new();
        for key in preferences::BOOLEAN.iter().chain(preferences::INTEGER).chain(preferences::TEXT).copied().chain(["transcription_providers"]) {
            let value=self.effective(key,&current);
            let origin=if current.contains_key(key){"app"}else{"env"};
            let (value,defined)=if key=="transcription_providers" {
                let list:Vec<_>=value.as_array().into_iter().flatten().filter(|p|p.is_object()).map(|p|{
                    let mut p=p.clone();p["api_key"]=json!(mask(p["api_key"].as_str().unwrap_or("")));p
                }).collect();
                let defined=!list.is_empty();(json!(list),defined)
            }else if SECRETS.contains(&key) {(json!(mask(value.as_str().unwrap_or(""))),truthy(&value))}
            else {let defined=!value.is_null();(value,defined)};
            result.insert(key.into(),json!({"valor":value,"definido":defined,"origem":origin}));
        }
        let thinking=self.thinking();
        let value=thinking.as_ref().and_then(|m|m.get("showThinkingSummaries"));
        result.insert("mostrar_pensamento".into(),json!({"valor":value.is_some_and(truthy),"definido":true,"origem":if value.is_some(){"app"}else{"env"}}));
        Value::Object(result)
    }
    pub fn apply(&self, changes:&Map<String,Value>, remove:&BTreeSet<String>)->Result<Map<String,Value>,String> {
        let _guard=WRITES.lock().unwrap_or_else(|e|e.into_inner());
        let mut current=match std::fs::read(&self.path) {
            Ok(raw)=>serde_json::from_slice::<Value>(&raw).ok().and_then(|v|v.as_object().cloned()).ok_or("runtime-config.json ilegível; arquivo preservado")?,
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>Map::new(),
            Err(error)=>return Err(error.to_string()),
        };
        let mut requested=changes.clone();
        if requested.shift_remove("jev_aux_cliproxy") == Some(Value::Bool(true)) {
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
        for key in remove {if preferences::editable(key){current.shift_remove(key);}}
        let mut thinking=None;
        for (key,value) in changes {
            if key=="mostrar_pensamento" {
                thinking=Some(value.as_bool().ok_or("mostrar_pensamento: esperado true/false")?);
                continue;
            }
            if !preferences::editable(key) {continue;}
            let mut value=value.clone();
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
                let effective=self.effective(key,&current);
                if let (Some(incoming),Some(secret))=(value.as_str(),effective.as_str()) {
                    if !secret.is_empty() && (incoming.trim().is_empty()||incoming.trim()==mask(secret)) {continue;}
                }
            }
            current.insert(key.clone(),preferences::coerce(key,&value)?);
        }
        if let Some(value)=thinking {self.write_thinking(value)?;}
        super::files::write_file(&self.path,&super::bundle::Blob {
            data:crate::transcript::pyjson::dumps_indent(&Value::Object(current.clone()),2,false).into_bytes(),mode:0o600,
        }).map_err(|e|e.to_string())?;
        CACHE.lock().unwrap_or_else(|e|e.into_inner()).remove(&self.path);
        if touches_aux||["jev_api_key","jev_endpoint","jev_model"].iter().any(|key|changes.contains_key(*key)||remove.contains(*key)) {
            self.sync_jev().map_err(|error|format!("jev: salvo, mas o Computer Use não recebeu a configuração: {error}"))?;
        }
        Ok(current)
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
pub fn mask(value:&str)->String {
    let chars:Vec<_>=value.chars().collect();
    if chars.len()<=8 {return "•".repeat(chars.len());}
    format!("{}••••••••{}",chars[..4].iter().collect::<String>(),chars[chars.len()-4..].iter().collect::<String>())
}
