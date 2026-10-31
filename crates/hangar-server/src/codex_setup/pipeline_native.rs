//! Composição das fases usa um cliente dono e encerra seus processos antes de soltar a conta.
use super::{cancellation::{self,OperationError},config_edits,files,instructions,native::Native,phase::{Error,State},pipeline_round,plugin_controlled,plugin_native,plugin_sync,stage_flow,stage_native};
use crate::{accounts::{AccountService,AccountKey,Provider},config_sync::{runtime::RuntimeConfig,runtime_values::ValueGuard},orchestration::codec,runtime::gateway::RuntimeRegistry};
use serde_json::{Value,json};
use std::{path::Path,sync::Arc,time::Duration};
use tokio::sync::watch;
pub(super) struct Actions<'a>{pub accounts:&'a AccountService,pub config:&'a RuntimeConfig,pub repo:&'a Path,pub account:&'a Path,pub work:&'a Path,pub backups:&'a Path,pub state:&'a State,pub runtime:Option<&'a Arc<RuntimeRegistry>>,pub native:Option<Native>,pub cancel:watch::Receiver<bool>}
fn error(kind:&'static str,message:impl Into<String>)->Error{Error{kind,message:message.into()}}
fn operation(e:OperationError)->Error{match e{OperationError::Cancelled=>error("CancelledError","integração Codex interrompida"),OperationError::Failed(message)=>error("CodexNativoErro",message)}}
fn warning(state:&State,code:&str,params:Value){state.update(|v|v["avisos"].as_array_mut().unwrap().push(super::messages::message(code,params.as_object().unwrap()).expect("Mensagem versionada")));}
fn get<'a>(value:&'a Value,key:&str,default:&'a Value)->Result<&'a Value,Error>{value.as_object().map(|v|v.get(key).unwrap_or(default)).ok_or_else(||error("AttributeError","Configuração precisa ser um objeto"))}
impl Actions<'_>{
 fn check(&self)->Result<(),Error>{if *self.cancel.borrow(){Err(error("CancelledError","integração Codex interrompida"))}else{Ok(())}}
 async fn request(&mut self,method:&str,params:Value)->Result<Value,Error>{
  let native=self.native.as_ref().ok_or_else(||error("RuntimeError","Cliente Codex não foi aberto"))?;
  tokio::select!{biased;_=cancellation::stopped(&mut self.cancel)=>Err(error("CancelledError","integração Codex interrompida")),result=native.request(method,params,Duration::from_secs(120))=>result.map_err(|message|error("CodexNativoErro",message))}
 }
 async fn configure_base(&self,registry:&mut Value)->Result<(),Error>{
  let limit=ValueGuard(instructions::limit(self.account)?);let empty=json!({});let mut plan=None;let mut callback_error=None;
  let result=files::edit_config_controlled(&self.accounts.env,&self.account.join("config.toml"),self.backups,self.work,|current|{
   match config_edits::prepare(current,config_edits::Sources{mcp:&empty,agents:&empty,registry:None,history:&empty,env:None,limit:&limit,hooks:false,memory:false}){
    Ok(mut value)=>{let edits=std::mem::take(&mut value.edits);plan=Some(value);Ok(edits)},Err(e)=>{let text=e.message.clone();callback_error=Some(e);Err(text)}
   }
  },self.cancel.clone()).await;
  match result{Ok(())=>{plan.expect("Gravador consultou a configuração").confirm(registry,self.state);Ok(())},Err(e)=>Err(callback_error.unwrap_or_else(||operation(e)))}
 }
 fn skills(&self,registry:&mut Value)->Result<(),Error>{
  let empty=json!({});let raw=files::read(&self.account.join("config.toml"))?;
  let data=ValueGuard(match raw{None=>json!({}),Some(raw)=>super::instructions_toml::parse(std::str::from_utf8(&raw).map_err(|_|error("UnicodeDecodeError","Configuração TOML não está em UTF-8"))?)?});
  let enabled=get(&data,"plugins",&empty)?;let plugins=get(registry,"plugins",&empty)?.as_object().ok_or_else(||error("AttributeError","Registro de plugins precisa ser um objeto"))?;
  let confirmed=self.state.confirmed();let mut selected=serde_json::Map::new();
  for(id,value)in plugins{if !confirmed.contains(id){continue;}let fallback=Value::String(id.clone());let key=get(value,"id_codex",&fallback)?.as_str().ok_or_else(||error("TypeError","Identificador de plugin precisa ser texto"))?;if get(get(enabled,key,&empty)?,"enabled",&Value::Null)?==&Value::Bool(true){selected.insert(id.clone(),codec::clone_value(value));}}
  let selected=ValueGuard(Value::Object(selected));let mut report=ValueGuard(super::skills::reconcile(&self.accounts.env,self.account,self.repo,&selected,get(registry,"skills",&empty)?,self.backups,cfg!(windows),&mut super::skills::Disk)?);
  let manifest=report.as_object_mut().unwrap().shift_remove("manifest").unwrap();let warnings=report.as_object_mut().unwrap().shift_remove("warnings").unwrap();
  if let Some(old)=registry.as_object_mut().unwrap().insert("skills".into(),manifest){codec::discard(old);}if let Value::Array(items)=warnings{self.state.update(|v|v["avisos"].as_array_mut().unwrap().extend(items));}Ok(())
 }
}
impl pipeline_round::Actions for Actions<'_>{
 fn now(&self)->f64{plugin_native::unix_now()}
 fn available(&self)->bool{self.accounts.codex_command().is_some()}
 fn fingerprint(&mut self,sources:bool,phase:Option<&str>,registry:Option<&Value>)->Result<String,Error>{super::pipeline_fingerprint::compute(&self.accounts.env,self.config,self.account,self.repo,registry,phase,sources)}
 async fn open(&mut self)->Result<(),Error>{self.native=Some(Native::open(&self.accounts.env,&self.accounts.env.home,self.account,false,Some(self.cancel.clone())).await.map_err(operation)?);Ok(())}
 async fn close(&mut self)->Result<(),Error>{if let Some(mut native)=self.native.take(){native.close().await.map_err(|message|error("CodexNativoErro",message))?;}Ok(())}
 async fn execute(&mut self,phase:&str,registry:&mut Value,state:&State,force:bool)->Result<(),Error>{
  self.check()?;
  match phase{
   "instructions"=>{
    instructions::prepare(self.config,self.account,None,force).await?;self.check()?;
    let path=self.account.join("AGENTS.md");if path.is_file()&&!path.is_symlink(){
     let mut failure=None;let result=files::transform(&path,self.backups,|raw|{
      let changed=(||->Result<Vec<u8>,Error>{let raw=raw.ok_or_else(||error("AttributeError","Instruções desapareceram durante a leitura"))?;let text=std::str::from_utf8(raw).map_err(|_|error("UnicodeDecodeError","Instruções não estão em UTF-8"))?;super::legacy_instructions::remove(text).map(String::into_bytes)})();
      changed.map_err(|e|{let message=e.message.clone();failure=Some(e);files::Error::Invalid(message)})
     });if let Err(e)=result{return Err(failure.unwrap_or_else(||e.into()));}
    }
    let wrapper=super::hook_command::allow_wrapper().map_err(|message|error("RuntimeError",message))?;let context=super::pipeline_hooks::Context{account:self.account,backups:self.backups,wrapper:&wrapper,windows:cfg!(windows)};
    super::pipeline_hooks::legacy(&context,state,&mut |account|crate::harness_setup::codex_hooks::sync_checked(account,self.repo,cfg!(windows)).map(|_|()).map_err(|message|error("RuntimeError",message)))?;
    super::pipeline_hooks::sync(&context,&json!({}),registry,state)?;
   },
   "plugins"=>{
    let native=self.native.as_mut().ok_or_else(||error("RuntimeError","Cliente Codex não foi aberto"))?;
    let inner=plugin_native::Actions{native,env:&self.accounts.env,home:&self.accounts.env.home,account:self.account,repo:self.repo,backups:self.backups,work:self.work,state};
    plugin_sync::run(&plugin_sync::Paths{home:&self.accounts.env.home,codex:self.account,backups:self.backups},registry,state,force,&mut plugin_controlled::Actions{inner,cancel:self.cancel.clone()}).await?;
   },
   "fragments"=>{
    self.configure_base(registry).await?;
    let main=self.native.as_ref().ok_or_else(||error("RuntimeError","Cliente Codex não foi aberto"))?;
    let mut actions=stage_native::Actions{main,stage:None,env:&self.accounts.env,account:self.account,repo:self.repo,backups:self.backups,work:self.work,state,config:self.config,cancel:self.cancel.clone()};
    stage_flow::run(&stage_flow::Paths{env:&self.accounts.env,home:&self.accounts.env.home,account:self.account,work:self.work,backups:self.backups},registry,state,super::pipeline_fingerprint::memory(self.config),&mut actions).await?;
    self.check()?;let(_,orphans)=super::hook_files::materialize(self.account,&self.accounts.env.home,None,cfg!(windows))?;if !orphans.is_empty(){warning(state,"aviso_hooks_sem_arquivo",json!({"arquivos":orphans.join(", ")}));}
   },
   "skills"=>self.skills(registry)?,
   _=>return Err(error("ValueError","Fase de integração desconhecida")),
  }self.check()
 }
 async fn trust(&mut self,state:&State)->Result<(),Error>{
  let result:Result<(),Error>=async{
   let data=ValueGuard(self.request("hooks/list",json!({"cwds":[self.accounts.env.home]})).await?);
   let(edits,manual)=super::trust::edits(&data,&self.accounts.env.home,self.account,false).map_err(|message|error("ValueError",message))?;
   if !edits.is_empty(){let result=self.request("config/batchWrite",json!({"edits":edits,"reloadUserConfig":true})).await?;codec::discard(result);}
   let result=if let Some(runtime)=self.runtime{let key=AccountKey::new(Provider::Codex,self.account).map_err(|e|error("AccountError",e.to_string()))?;ValueGuard(runtime.refresh_hooks(&key).await)}else{ValueGuard(Value::Null)};
   if !result.is_null()&&result["ok"]!=true{return Err(error("CodexNativoErro","O Codex não confirmou a recarga dos hooks nas sessões abertas"));}
   if crate::orchestration::truthy(&result["approval_required"])||manual>0{state.hooks_changed();}else{state.update(|v|{v["confianca_pendente"]=json!(false);v["avisos"].as_array_mut().unwrap().retain(|item|ValueGuard(super::messages::serialize(item))["codigo"]!="aviso_hooks_alterados");});}Ok(())
  }.await;
  match result{Err(e) if matches!(e.kind,"CodexNativoErro"|"AccountError"|"HTTPException"|"ValueError")=>{if e.kind=="ValueError"{warning(state,"aviso_confianca_indisponivel",json!({}));}state.hooks_changed();state.update(|v|v["erros"].as_array_mut().unwrap().push(super::messages::message("erro_recarregar_hooks",&serde_json::Map::new()).unwrap()));Ok(())},other=>other}
 }
}
