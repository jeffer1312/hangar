//! Efeitos dos hooks da rodada, com backup e aviso de confiança compartilhado.
use super::{files,hooks,normalize,phase::{Error,State}};
use crate::{config_sync::{runtime_values::ValueGuard,value_equality},orchestration::{body,codec}};
use serde_json::{Value,json};
use std::path::Path;
pub struct Context<'a>{pub account:&'a Path,pub backups:&'a Path,pub wrapper:&'a (String,String),pub windows:bool}
fn parse(raw:Option<&[u8]>)->Result<Value,files::Error>{
 match raw.filter(|v|!v.is_empty()){None=>Ok(json!({})),Some(raw)=>body::parse(raw).map_err(|_|files::Error::Invalid("Documento de hooks inválido".into()))}
}
fn merged_bytes(raw:Option<&[u8]>,current:&Value,merged:&Value)->Result<Vec<u8>,files::Error>{
 if raw.is_some_and(|v|!v.is_empty())&&value_equality::equal(current,merged){Ok(raw.unwrap().to_vec())}else{files::bytes(merged)}
}
pub fn sync(context:&Context<'_>,source:&Value,registry:&mut Value,state:&State)->Result<(),Error>{
 let previous=ValueGuard(codec::clone_value(registry.get("hooks").unwrap_or(&json!({}))));
 let path=context.account.join("hooks.json");let mut normalized=ValueGuard(codec::clone_value(source));
 if files::read(&path)?.is_some()||crate::engines::probe::truthy(&source["hooks"]){
  let changed=files::transform(&path,context.backups,|raw|{
   let current=ValueGuard(parse(raw)?);
   let normalized_current=ValueGuard(normalize::hooks(&current,&context.wrapper.0,&context.wrapper.1,context.windows));
   normalized=ValueGuard(normalize::hooks(source,&context.wrapper.0,&context.wrapper.1,context.windows));
   let merged=ValueGuard(if crate::engines::probe::truthy(&normalized){hooks::merge(&normalized_current,&normalized,&previous).map_err(files::Error::Invalid)?}else{codec::clone_value(&normalized_current)});
   merged_bytes(raw,&current,&merged)
  })?;
  if changed{state.hooks_changed();}
 }
 if crate::engines::probe::truthy(source){if let Some(old)=registry.as_object_mut().ok_or_else(||Error{kind:"TypeError",message:"Registro precisa ser um objeto".into()})?.insert("hooks".into(),codec::clone_value(&normalized)){codec::discard(old);}}
 Ok(())
}
pub fn legacy(context:&Context<'_>,state:&State,install:&mut impl FnMut(&Path)->Result<(),Error>)->Result<(),Error>{
 let mirror=context.account.join(".hangar-hooks.json");let Some(raw)=files::read(&mirror)? else{return Ok(())};let previous=ValueGuard(parse(Some(&raw))?);let path=context.account.join("hooks.json");
 if files::read(&path)?.is_some()&&files::transform(&path,context.backups,|raw|{let current=ValueGuard(parse(raw)?);let merged=ValueGuard(hooks::merge(&current,&json!({"hooks":{}}),&previous).map_err(files::Error::Invalid)?);merged_bytes(raw,&current,&merged)})?{state.hooks_changed();}
 files::backup(&mirror,&raw,context.backups)?;std::fs::remove_file(&mirror).map_err(files::Error::Io)?;install(context.account)
}
