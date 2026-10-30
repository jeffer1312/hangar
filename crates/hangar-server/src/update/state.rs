//! `estado.json` da atualização: o mesmo arquivo, os mesmos campos e o mesmo jeito de gravar do
//! motor `git` (`backend/app/atualizar.py`): os clientes (nativo, Expo, PWA) leem por polling e
//! não sabem qual motor escreveu.
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use crate::orchestration::codec;

/// Linhas guardadas no `log` do estado, como o `_TETO_LOG` do Python.
const LOG_CAP: usize = 400;

/// Etapas do motor de pacote, na ordem. O rótulo é o que a tela mostra.
pub const STEPS: &[(&str, &str)] = &[
    ("baixar", "Baixando a versão nova"),
    ("passos", "Aplicando o que a versão pede"),
    ("trocar", "Trocando para a versão nova"),
    ("reiniciar", "Reiniciando o servidor"),
];

/// `<config do Claude>/.hangar-update/`, a pasta do `_base()` do Python (`CLAUDE_CONFIG_DIR` ou
/// `~/.claude`). Lá moram `estado.json` e o registro `aplicados.json` dos passos.
pub fn state_dir(claude_base: &Path) -> PathBuf { claude_base.join(".hangar-update") }

#[derive(Debug, Clone)]
pub struct StateFile {
    path: PathBuf,
}

#[derive(Debug, Default)]
pub struct StateSnapshot(Map<String, Value>);
impl std::ops::Deref for StateSnapshot { type Target=Map<String,Value>; fn deref(&self)->&Self::Target{&self.0} }
impl Clone for StateSnapshot {fn clone(&self)->Self{Self(self.0.iter().map(|(key,value)|(key.clone(),codec::clone_value(value))).collect())}}
impl Drop for StateSnapshot {fn drop(&mut self){codec::discard(Value::Object(std::mem::take(&mut self.0)));}}
impl From<Map<String,Value>> for StateSnapshot {fn from(value:Map<String,Value>)->Self{Self(value)}}
impl StateSnapshot {
    pub fn failure(error: String)->Self {
        tracing::warn!(code="update_state_write_failed", %error, "estado da atualização não gravado");
        Self([("fase".into(), Value::String("pronto".into())), ("ok".into(), Value::Bool(false)), ("erro".into(), Value::String(error))].into_iter().collect())
    }
    pub fn into_value(mut self)->Value{Value::Object(std::mem::take(&mut self.0))}
    pub fn insert(&mut self,key:String,value:Value){if let Some(previous)=self.0.insert(key,value){codec::discard(previous);}}
    pub fn remove(&mut self,key:&str)->Option<Value>{self.0.remove(key)}
}
impl StateFile {
    pub fn new(state_dir:&Path)->Self{Self{path:state_dir.join("estado.json")}}
    pub fn path(&self)->&Path{&self.path}
    pub fn read(&self)->StateSnapshot {
        let value=std::fs::read_to_string(&self.path).ok().and_then(|raw|codec::loads(&raw));
        match value {
            Some(Value::Object(fields))=>StateSnapshot(fields),
            Some(other)=>{codec::discard(other);StateSnapshot::default()},
            None=>StateSnapshot::default(),
        }
    }
    pub fn try_write(&self,fields:Value)->Result<(),String>{
        let mut current=self.read();
        if let Value::Object(fields)=fields {for (key,value) in fields {current.insert(key,value);}}
        else {codec::discard(fields);}
        current.insert("ts".into(),Value::String(now()));
        let document=current.into_value();
        let encoded=codec::dumps_utf8(&document).map_err(|_|"update_state_invalid_unicode: texto contém surrogate Unicode isolado".to_owned());
        codec::discard(document);
        let bytes=encoded?;
        self.path.parent().map_or(Ok(()),std::fs::create_dir_all)
            .and_then(|_|crate::runtime::queue::atomic_write(&self.path,bytes.as_bytes()))
            .map_err(|error|format!("update_state_write_failed: {error}"))
    }
    pub fn publish(&self, fields:Value)->bool {
        match self.try_write(fields) {
            Ok(())=>true,
            Err(error)=>{tracing::warn!(code="update_state_write_failed",%error,"estado da atualização não gravado");false}
        }
    }
    pub fn write(&self,fields:Value){self.publish(fields);}
    pub fn finish(&self, fields:Value)->StateSnapshot {
        match self.try_write(fields) {Ok(())=>self.read(),Err(error)=>StateSnapshot::failure(error)}
    }
    pub fn step(&self,key:&str){
        let index=STEPS.iter().position(|(candidate,_)|*candidate==key).unwrap_or(0);
        self.write(serde_json::json!({"fase":"rodando","etapa":key,"passo":index+1,"total":STEPS.len(),"texto":STEPS[index].1,"etapa_inicio":now()}));
    }
    pub fn try_log(&self,text:&str)->Result<(),String>{
        let mut current=self.read();
        let previous=current.remove("log").unwrap_or(Value::Null);
        let mut lines=match previous {
            Value::Array(lines)=>lines,
            other if !crate::machine_peers::store::truthy(&other)=>{codec::discard(other);vec![]},
            other=>{codec::discard(other);return Err("update_state_invalid_log".into());}
        };
        let split=crate::transcript::py::splitlines(text);
        lines.extend(if split.is_empty(){vec![text]}else{split}.into_iter().map(|line|Value::String(line.into())));
        let skip=lines.len().saturating_sub(LOG_CAP);
        let kept=lines.split_off(skip);codec::discard(Value::Array(lines));
        let mut fields=Map::new();fields.insert("log".into(),Value::Array(kept));self.try_write(Value::Object(fields))
    }
    pub fn log(&self,text:&str){if let Err(error)=self.try_log(text){tracing::warn!(code="update_state_write_failed",%error,"log da atualização não gravado");}}
    pub fn try_warn(&self,text:&str)->Result<(),String>{
        let mut current=self.read();let previous=current.remove("avisos").unwrap_or(Value::Null);
        let mut warnings=match previous {
            Value::Array(values)=>values,
            Value::Object(values)=>{let mut result=vec![];for (key,value) in values{result.push(Value::String(key));codec::discard(value);}result},
            Value::String(value) if !codec::contains_nonfinite(&Value::String(value.clone()))=>{
                let mut remaining=value.as_str();let mut result=vec![];
                while !remaining.is_empty(){let item=codec::truncate_string(remaining,1);result.push(Value::String(item.into()));remaining=&remaining[item.len()..];}result
            },
            other if !crate::machine_peers::store::truthy(&other)=>{codec::discard(other);vec![]},
            other=>{codec::discard(other);return Err("update_state_invalid_warnings".into());}
        };
        warnings.push(Value::String(text.into()));let mut fields=Map::new();fields.insert("avisos".into(),Value::Array(warnings));self.try_write(Value::Object(fields))
    }
    pub fn warn(&self,text:&str){match self.try_warn(text){
        Ok(())=>tracing::warn!("{text}"),
        Err(error)=>tracing::warn!(code="update_state_write_failed",%error,"aviso da atualização não gravado"),
    }}
}

/// O `datetime.now().astimezone().isoformat(timespec="seconds")` do Python.
fn now() -> String { chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%:z").to_string() }
