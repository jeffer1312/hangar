//! Preparo concluído permanece válido até uma atualização explícita.
use super::phase::Error;
use serde_json::Value;
fn get<'a>(value:&'a Value,key:&str,default:&'a Value)->Result<&'a Value,Error>{value.as_object().map(|map|map.get(key).unwrap_or(default)).ok_or_else(||Error{kind:"AttributeError",message:"Estado da integração precisa ser um objeto".into()})}
fn timestamp(value:&Value)->Result<f64,Error>{let text=value.as_str().ok_or_else(||Error{kind:"TypeError",message:"fromisoformat: argument must be str".into()})?;crate::transcript::py::iso_timestamp(text).ok_or_else(||Error{kind:"ValueError",message:"Data de integração inválida".into()})}
pub fn needed(running:bool,record:&Value,now:f64,fingerprint:impl FnOnce()->Result<String,Error>)->Result<bool,Error>{
 if running{return Ok(false);}
 let empty=serde_json::json!({});let status=get(record,"status",&empty)?;let state=get(status,"estado",&Value::Null)?;
 if state=="ok"{return Ok(false);}
 match fingerprint(){Ok(current)=>{if get(record,"fingerprint",&Value::Null)?!=&Value::String(current){return Ok(true);}},Err(error) if matches!(error.kind,"OSError"|"FileNotFoundError"|"PermissionError"|"NotADirectoryError"|"ValueError"|"JSONDecodeError"|"UnicodeDecodeError")=>return Ok(true),Err(error)=>return Err(error)}
 let next=get(status,"proxima_atualizacao",&Value::Null)?;if crate::orchestration::truthy(next)&&timestamp(next)?<=now{return Ok(true);}
 let last=get(status,"ultima_execucao",&Value::Null)?;if !crate::orchestration::truthy(last)||state=="ocioso"{return Ok(true);}
 if state=="parcial"||state=="erro"{return Ok(timestamp(last)?+300.0<=now);}
 Ok(false)
}
#[cfg(test)]
mod fuzz{
 use proptest::prelude::*;
 proptest!{#![proptest_config(ProptestConfig::with_cases(48))]
 #[test] fn codex_service_reconcile_external_dates_do_not_panic(date in ".{0,128}",state in ".{0,32}"){let record=serde_json::json!({"fingerprint":"same","status":{"estado":state,"ultima_execucao":date,"proxima_atualizacao":date}});let _=super::needed(false,&record,1000.0,||Ok("same".into()));}
 }
}
