//! Proveniência nativa de nomes globais, sem adotar caminhos de projeto.
use super::{messages,phase::{Error,State}};
use serde_json::{Value,Map,json};
use std::collections::BTreeSet;
fn kind(value:&Value)->&'static str{match value{Value::Null=>"NoneType",Value::Bool(_)=>"bool",Value::Number(n) if n.to_string().contains(['.','e','E'])=>"float",Value::Number(_)=>"int",Value::String(_)=>"str",Value::Array(_)=>"list",Value::Object(_)=>"dict"}}
fn get<'a>(value:&'a Value,key:&str,default:&'a Value)->Result<&'a Value,Error>{value.as_object().map(|map|map.get(key).unwrap_or(default)).ok_or_else(||Error{kind:"AttributeError",message:format!("'{}' object has no attribute 'get'",kind(value))})}
fn valid_name(value:&Value,windows:bool)->Option<&str>{
 let name=value.as_str()?;
 if name.is_empty()||matches!(name,"."|"..")||name.contains('/')||windows&&(name.contains('\\')||name.as_bytes().get(1)==Some(&b':')){return None;}
 Some(name)
}
pub fn collect(entries:Result<&[Value],String>,state:&State,windows:bool)->Result<Value,Error>{
 let mut mcp=BTreeSet::new();let mut agents=BTreeSet::new();let mut commands=BTreeSet::new();let empty=json!([]);
 match entries{Ok(entries)=>{
  for entry in entries{
   let provider=get(entry,"providerId",&Value::Null)?;
   if !provider.is_null()&&provider!="claude-code"{continue;}
   let successes=get(entry,"successes",&empty)?;
   let items=match successes{
    Value::Array(items)=>items.as_slice(),
    Value::Object(items) if items.is_empty()=>&[],
    Value::String(text) if text.is_empty()=>&[],
    Value::Object(_)|Value::String(_)=>return Err(Error{kind:"AttributeError",message:"'str' object has no attribute 'get'".into()}),
    value=>return Err(Error{kind:"TypeError",message:format!("'{}' object is not iterable",kind(value))}),
   };
   for item in items{
    let item_type=get(item,"itemType",&Value::Null)?;
    if item_type.is_array()||item_type.is_object(){let kind=kind(item_type);return Err(Error{kind:"TypeError",message:format!("cannot use '{kind}' as a dict key (unhashable type: '{kind}')")});}
    let target=get(item,"target",&Value::Null)?;
    let selected=match item_type.as_str(){Some("MCP_SERVER_CONFIG")=>Some(&mut mcp),Some("SUBAGENTS")=>Some(&mut agents),Some("COMMANDS")=>Some(&mut commands),_=>None};
    if let Some(selected)=selected{if let Some(name)=valid_name(target,windows){if get(item,"cwd",&Value::Null)?.is_null(){selected.insert(name.to_owned());}}}
   }
  }
 },Err(_)=>{
  state.update(|value|value["avisos"].as_array_mut().expect("Estado inicializado").push(messages::message("aviso_historico_indisponivel",&Map::new()).expect("Mensagem versionada")));
 }};
 Ok(json!({"mcp_servers":mcp,"agents":agents,"commands":commands}))
}

pub async fn read(native:&super::native::Native,state:&State)->Result<Value,Error>{
 match super::import_api::histories(native,std::time::Duration::from_secs(120)).await{
  Ok(items)=>{let items=crate::config_sync::runtime_values::ValueGuard(Value::Array(items));collect(Ok(items.as_array().unwrap()),state,cfg!(windows))},
  Err(error)=>collect(Err(error),state,cfg!(windows)),
 }
}
