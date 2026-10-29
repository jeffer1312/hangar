//! Publicação do Jev nos registros ativos e guardados do Computer Use.
use super::{apply::truthy, legacy::{AUX,base_url,read_object}, runtime::RuntimeConfig};
use serde_json::{Map,Value,json};
use std::path::Path;
const NAME:&str="hangar-computer-control";
const LLM:[&str;4]=["LLM_PROXY_URL","LLM_MODEL","LLM_EFFORT","LLM_PROXY_KEY"];
const REPLACED:[&str;7]=["TYPESAFE_API_KEY","JEV_ENDPOINT","JEV_MODEL","LLM_PROXY_URL","LLM_PROXY_KEY","LLM_MODEL","LLM_EFFORT"];

fn env_of(entry:&Map<String,Value>,path:&Path)->Result<Map<String,Value>,String> {
    match entry.get("env").filter(|v|truthy(v)) {
        None=>Ok(Map::new()),
        Some(Value::Object(env))=>Ok(env.clone()),
        _=>Err(format!("env em {} não é um objeto JSON",path.display())),
    }
}
fn write(path:&Path,data:Map<String,Value>,private:bool)->Result<(),String> {
    super::files::write_file(path,&super::bundle::Blob {data:crate::transcript::pyjson::dumps_indent(&json!(data),2,true).into_bytes(),mode:if private{0o600}else{super::files::mode(path).unwrap_or(0o644)}}).map_err(|e|e.to_string())
}
impl RuntimeConfig {
    fn jev_env(&self)->Map<String,Value> {
        let mut output=Map::new();
        let key=self.get("jev_api_key").as_str().unwrap_or("").trim().to_owned();
        if !key.is_empty() {
            let endpoint=self.get("jev_endpoint");let model=self.get("jev_model");
            let (endpoint,model)=crate::voice::jev::destination(&key,endpoint.as_str(),model.as_str());
            output.insert("TYPESAFE_API_KEY".into(),json!(key));
            output.insert("JEV_ENDPOINT".into(),json!(endpoint.unwrap_or_else(||"https://api.typesafe.ai/v1/systemone".into())));
            output.insert("JEV_MODEL".into(),json!(model.unwrap_or_else(||"jev-latest".into())));
        }
        for (key,target) in [AUX[0],AUX[2],AUX[3],AUX[1]].into_iter().zip(LLM) {
            let value=self.get(key).as_str().unwrap_or("").trim().to_owned();
            if value.is_empty(){continue;}
            let value=if key==AUX[0] {
                let base=base_url(&value);let split=base.find(['?','#']).unwrap_or(base.len());
                format!("{}/chat/completions{}",&base[..split],&base[split..])
            }else{value};
            output.insert(target.into(),json!(value));
        }
        output
    }
    fn preserve_old(&self,path:&Path,old:&Map<String,Value>,new:&Map<String,Value>)->Result<(),String> {
        let old:Map<String,Value>=LLM.into_iter().filter_map(|key|old.get(key).filter(|v|truthy(v)).map(|v|(key.into(),json!(crate::orchestration::codec::python_string(v))))).collect();
        if old.is_empty()||old.iter().all(|(k,v)|new.get(k)==Some(v)){return Ok(());}
        let backup=self.env.home.join(".hangar/computer-use-llm-antigo.json");
        let mut previous=read_object(&backup)?;
        let source=path.to_string_lossy().into_owned();
        if previous.contains_key(&source){return Ok(());}
        previous.insert(source,json!(old));
        write(&backup,previous,true)
    }
    pub fn sync_jev(&self)->Result<(),String> {
        let jev=self.jev_env();
        let service=crate::accounts::AccountService::new(self.env.clone());
        let mut paths=vec![self.env.home.join(".claude.json")];
        let default=crate::accounts::catalog::resolved(&self.env.home.join(".claude"));
        for (_,account) in service.claude_accounts().map_err(|e|e.message)? {
            let path=account.home.join(".claude.json");
            if crate::accounts::catalog::resolved(&account.home)!=default&&path.is_file(){paths.push(path);}
        }
        let merged=|env:&Map<String,Value>| {
            let mut result:Map<_,_>=env.iter().filter(|(k,_)|!REPLACED.contains(&k.as_str())).map(|(k,v)|(k.clone(),v.clone())).collect();
            result.extend(jev.clone());result
        };
        let mut pending=Vec::new();
        for path in paths {
            let mut data=read_object(&path)?;
            let servers=data.get("mcpServers").filter(|v|truthy(v));
            if servers.is_some_and(|v|!v.is_object()){return Err(format!("mcpServers em {} não é um objeto JSON",path.display()));}
            let entry=servers.and_then(|v|v.get(NAME)).and_then(Value::as_object).cloned();
            if let Some(mut entry)=entry {
                let env=env_of(&entry,&path)?;let next=merged(&env);
                if next!=env {
                    self.preserve_old(&path,&env,&jev)?;
                    entry.insert("env".into(),json!(next));
                    data.get_mut("mcpServers").unwrap()[NAME]=json!(entry);
                    pending.push((path,data));
                }
            }
        }
        let path=self.env.home.join(".hangar/computer-control.json");
        let mut parked=read_object(&path)?;
        if !parked.is_empty() {
            let env=env_of(&parked,&path)?;let next=merged(&env);
            if next!=env {
                self.preserve_old(&path,&env,&jev)?;
                parked.insert("env".into(),json!(next));write(&path,parked,true)?;
            }
        }
        for (path,data) in pending {write(&path,data,false)?;}
        Ok(())
    }
}
