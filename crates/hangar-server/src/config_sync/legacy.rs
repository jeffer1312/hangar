//! Escolha indivisível do modelo auxiliar anterior à configuração compartilhada.
use super::runtime::RuntimeConfig;
use crate::orchestration::truthy;
use serde_json::{Map, Value, json};
use std::path::Path;

pub const AUX: [&str;4] = ["jev_aux_base_url","jev_aux_api_key","jev_aux_modelo","jev_aux_esforco"];

pub fn read_object(path:&Path)->Result<Map<String,Value>,String> {
    let raw=match std::fs::read(path) {
        Ok(raw)=>raw,
        Err(error) if error.kind()==std::io::ErrorKind::NotFound=>return Ok(Map::new()),
        Err(error)=>return Err(format!("não consegui ler {}: {}",path.display(),error)),
    };
    let value:Value=serde_json::from_slice(&raw).map_err(|_|format!("não consegui ler {}: JSON ou encoding inválido",path.display()))?;
    value.as_object().cloned().ok_or_else(||format!("{} não é um objeto JSON",path.display()))
}

fn text(value:Option<&Value>)->String {
    match value.filter(|v|truthy(v)) {
        Some(Value::String(value))=>value.trim().into(),
        Some(value)=>crate::orchestration::codec::python_string(value).trim().into(),
        None=>String::new(),
    }
}

pub fn base_url(value:&str)->String {
    // A query pertence ao endpoint, depois do caminho da operação.
    let split=value.find(['?','#']).unwrap_or(value.len());
    let path=value[..split].trim_end_matches('/');
    format!("{}{}",path.strip_suffix("/chat/completions").unwrap_or(path),&value[split..])
}

fn values(url:String,key:String,model:String,effort:String)->Map<String,Value> {
    AUX.into_iter().zip([base_url(&url),key,model,effort]).map(|(k,v)|(k.into(),json!(v))).collect()
}

impl RuntimeConfig {
    pub fn legacy_aux(&self,current:&Map<String,Value>,strict:bool)->Result<Map<String,Value>,String> {
        let url=text(current.get("jev_texto_base_url"));
        let browser=if url.is_empty(){Map::new()}else{values(url,text(current.get("jev_texto_api_key")),text(current.get("jev_texto_modelo")),String::new())};
        let known=(|| {
            let path=self.env.home.join(".claude.json");
            let active=read_object(&path)?;
            let servers=active.get("mcpServers").filter(|v|truthy(v));
            if servers.is_some_and(|v|!v.is_object()) {return Err(format!("mcpServers em {} não é um objeto JSON",path.display()));}
            if let Some(entry)=servers.and_then(|v|v.get("hangar-computer-control")).and_then(Value::as_object) {return Ok(entry.clone());}
            read_object(&self.env.home.join(".hangar/computer-control.json"))
        })();
        let known=match known {
            Ok(entry)=>entry,
            Err(error) if strict=>return Err(format!("jev_aux: não consegui ler o modelo antigo do Computer Use ({error}); tente de novo")),
            Err(error)=>{tracing::warn!(%error,"Modelo auxiliar antigo ilegível");Map::new()},
        };
        let env=known.get("env").and_then(Value::as_object);
        let get=|key|text(env.and_then(|env|env.get(key)));
        let url=get("LLM_PROXY_URL");
        let computer=if url.is_empty(){Map::new()}else{values(url,get("LLM_PROXY_KEY"),get("LLM_MODEL"),get("LLM_EFFORT"))};
        if browser.is_empty(){return Ok(computer);}
        if computer.is_empty(){return Ok(browser);}
        let same=browser[AUX[0]]==computer[AUX[0]]&&browser[AUX[2]]==computer[AUX[2]]&&(!truthy(&browser[AUX[1]])||!truthy(&computer[AUX[1]])||browser[AUX[1]]==computer[AUX[1]]);
        if same {return Ok(AUX.into_iter().map(|key|(key.into(),if truthy(&browser[key]){browser[key].clone()}else{computer[key].clone()})).collect());}
        let score=|entry:&Map<String,Value>|entry.values().filter(|v|truthy(v)).count();
        tracing::warn!("Modelos auxiliares antigos diferentes; selecionada a origem mais completa");
        Ok(if score(&browser)>=score(&computer){browser}else{computer})
    }
}
