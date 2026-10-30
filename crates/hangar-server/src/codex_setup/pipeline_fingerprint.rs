//! A importação só confirma o resultado quando todas as fontes continuam iguais.
use super::{files,fingerprints,phase::Error,status};
use crate::{accounts::environment::AccountEnvironment,config_sync::{runtime::RuntimeConfig,runtime_values::ValueGuard}};
use serde_json::{Map,Value};
use std::path::Path;
pub(super) fn memory(config:&RuntimeConfig)->bool{crate::engines::probe::truthy(&ValueGuard(config.get("codex_memory_import")))}
pub(super) fn compute(env:&AccountEnvironment,config:&RuntimeConfig,account:&Path,repo:&Path,registry:Option<&Value>,phase:Option<&str>,sources:bool)->Result<String,Error>{
 let stored;
 let registry=match registry{Some(value)=>value,None=>{stored=ValueGuard(files::object(&status::root(&env.home,account).map_err(|error|Error{kind:error.kind,message:error.message})?.join("estado.json"))?);&stored}};
 let memory=memory(config);
 if let Some(phase)=phase{return fingerprints::compute(env,account,repo,registry,memory,phase,sources)}
 let mut result=Map::new();for phase in ["instructions","plugins","fragments","skills"]{result.insert(phase.into(),Value::String(fingerprints::compute(env,account,repo,registry,memory,phase,sources)?));}
 Ok(files::hash(&files::bytes(&Value::Object(result))?))
}
