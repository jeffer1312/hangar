//! Códigos e textos do reconciliador; parâmetros são publicados como texto.
use crate::orchestration::codec;
use serde_json::{Map,Value,json};
use std::sync::LazyLock;
static CATALOG:LazyLock<Value>=LazyLock::new(||serde_json::from_str(include_str!("messages.json")).expect("catálogo Codex versionado"));
pub fn message(code:&str,params:&Map<String,Value>)->Result<Value,String>{
 let template=CATALOG.get(code).and_then(Value::as_str).ok_or_else(||format!("Mensagem Codex desconhecida: {code}"))?;
 let params:Map<String,Value>=params.iter().map(|(key,value)|(key.clone(),json!(codec::python_string(value)))).collect();
 let mut text=String::new();let mut rest=template;
 while let Some(open)=rest.find('{'){
  text.push_str(&rest[..open]);let tail=&rest[open+1..];
  let end=tail.find('}').ok_or("Mensagem Codex inválida")?;
  let field=&tail[..end];
  text.push_str(params.get(field).and_then(Value::as_str).ok_or_else(||format!("Parâmetro Codex ausente: {field}"))?);
  rest=&tail[end+1..];
 }
 text.push_str(rest);
 Ok(json!({"codigo":code,"params":params,"texto":text}))
}
pub fn serialize(value:&Value)->Value {
 if value.is_object(){codec::clone_value(value)}else{json!({"codigo":null,"params":{},"texto":if value.is_null(){String::new()}else{codec::python_string(value)}})}
}
