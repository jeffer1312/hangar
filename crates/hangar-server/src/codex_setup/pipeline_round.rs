//! Rodada completa conserva a trava até fechar o cliente e publicar o checkpoint final.
use super::phase::{Error,State};
use serde_json::Value;
use std::{future::Future,path::Path};
use tokio::sync::watch;
pub struct Context<'a>{pub home:&'a Path,pub account:&'a Path,pub work:&'a Path,pub state:&'a State,pub force:bool,pub memory:bool,pub cancel:watch::Receiver<bool>}
pub trait Actions {
 fn now(&self)->f64;
 fn available(&self)->bool;
 fn fingerprint(&mut self,sources:bool,phase:Option<&str>,registry:Option<&Value>)->Result<String,Error>;
 fn execute(&mut self,phase:&str,registry:&mut Value,state:&State,force:bool)->impl Future<Output=Result<(),Error>>+Send;
 fn open(&mut self)->impl Future<Output=Result<(),Error>>+Send;
 fn close(&mut self)->impl Future<Output=Result<(),Error>>+Send;
 fn trust(&mut self,state:&State)->impl Future<Output=Result<(),Error>>+Send;
}
use super::{cancellation,files,messages,phase,plugin_native,status,timestamp};
use crate::{config_sync::runtime_values::ValueGuard,orchestration::codec};
use serde_json::json;
fn error(kind:&'static str,message:impl Into<String>)->Error{Error{kind,message:message.into()}}
fn message(code:&str,params:Value)->Value{messages::message(code,params.as_object().expect("Parâmetros de mensagem")).expect("Mensagem versionada")}
fn check(cancel:&watch::Receiver<bool>)->Result<(),Error>{if *cancel.borrow(){Err(error("CancelledError","integração Codex interrompida"))}else{Ok(())}}
fn view(context:&Context<'_>,now:f64)->Result<Value,Error>{let snapshot=ValueGuard(context.state.snapshot());status::read(context.home,context.account,Some(&snapshot),now).map_err(|e|error(e.kind,e.message))}
fn step(state:&State,index:u8,code:&str,now:f64){state.update(|v|{v["etapa"]=message(code,json!({}));v["etapa_desde"]=json!(now);v["progresso"]=json!({"passo":index,"total":5,"sub":null});});}
fn get<'a>(value:&'a Value,key:&str,default:&'a Value)->Result<&'a Value,Error>{value.as_object().map(|v|v.get(key).unwrap_or(default)).ok_or_else(||error("AttributeError","Registro da integração precisa ser um objeto"))}
fn number(value:&Value)->Result<f64,Error>{value.as_bool().map(|v|f64::from(u8::from(v))).or_else(||value.as_f64()).ok_or_else(||error("TypeError","Data de atualização inválida"))}
fn plugins_due(registry:&Value,now:f64)->Result<bool,Error>{
 let empty=json!({});let zero=json!(0);let a=number(get(registry,"marketplaces_em",&zero)?)?;let b=number(get(registry,"marketplaces_tentativa_em",&zero)?)?;let mut attempted=if a>=b{a}else{b};
 let mut retry=crate::engines::probe::truthy(get(registry,"marketplaces_pendentes",&Value::Null)?)||crate::engines::probe::truthy(get(registry,"plugins_pendentes",&Value::Null)?);
 let previous=get(get(registry,"phases",&empty)?,"plugins",&empty)?;
 if crate::engines::probe::truthy(get(previous,"errors",&Value::Null)?){retry=true;attempted=number(get(previous,"attempted_at",&json!(attempted))?)?;}
 Ok(now<attempted||now-attempted>=if retry{300.0}else{21600.0})
}
struct Phase<'a,A>{name:&'a str,force:bool,actions:&'a mut A}
impl<A:Actions+Send> phase::Actions for Phase<'_,A>{
 async fn fingerprint(&mut self,source:bool,registry:&Value)->Result<String,Error>{self.actions.fingerprint(source,Some(self.name),Some(registry))}
 async fn execute(&mut self,registry:&mut Value,state:&State)->Result<(),Error>{self.actions.execute(self.name,registry,state,self.force).await}
 fn now(&self)->f64{self.actions.now()}
}
async fn phase(context:&Context<'_>,actions:&mut(impl Actions+Send),name:&str,registry:&mut Value,force:bool)->Result<(),Error>{
 check(&context.cancel)?;phase::run(name,registry,context.state,force,&mut Phase{name,force:context.force,actions}).await?;check(&context.cancel)
}
async fn native_phases(context:&Context<'_>,actions:&mut(impl Actions+Send),registry:&mut Value)->Result<(),Error>{
 step(context.state,3,"etapa_importando",actions.now());let due=plugins_due(registry,actions.now())?;
 phase(context,actions,"plugins",registry,context.force||due).await?;
 step(context.state,4,"etapa_fragmentos",actions.now());phase(context,actions,"fragments",registry,context.force||context.memory).await?;
 plugin_native::checkpoint(context.home,context.account,context.work,registry,context.state,actions.now())?;
 step(context.state,5,"etapa_skills",actions.now());phase(context,actions,"skills",registry,context.force||due).await?;
 plugin_native::checkpoint(context.home,context.account,context.work,registry,context.state,actions.now())?;
 actions.trust(context.state).await?;check(&context.cancel)
}
pub async fn run(context:&Context<'_>,actions:&mut(impl Actions+Send))->Result<Value,Error>{
 let previous=ValueGuard(view(context,actions.now())?);let mut initial=status::snapshot();initial["estado"]=json!("executando");for key in ["plugins","ultima_execucao","confianca_pendente"]{initial[key]=codec::clone_value(&previous[key]);}
 context.state.update(|v|{let old=std::mem::replace(v,initial);codec::discard(old);});step(context.state,1,"etapa_inventariando",actions.now());
 let unavailable=if !context.home.join(".claude/settings.json").is_file(){Some("etapa_sem_claude")}else if !actions.available(){Some("etapa_sem_codex")}else{None};
 if let Some(code)=unavailable{context.state.update(|v|{v["estado"]=json!("indisponivel");v["etapa"]=message(code,json!({}));v["progresso"]=Value::Null;});return view(context,actions.now());}
 context.state.set_confirmed(Default::default());let mut registry=ValueGuard(json!({}));let mut loaded=false;let mut lock=None;let mut beginning=None;
 let result:Result<(),Error>=async{
  check(&context.cancel)?;beginning=Some(actions.fingerprint(true,None,None)?);
  let mut cancel=Some(context.cancel.clone());lock=Some(cancellation::run(&mut cancel,async{files::exclusive(&context.account.join(".hangar-integracao.lock")).await.map_err(|e|e.to_string())}).await.map_err(|e|match e{cancellation::OperationError::Cancelled=>error("CancelledError","integração Codex interrompida"),cancellation::OperationError::Failed(message)=>error("OSError",message)})?);
  let mut builder=std::fs::DirBuilder::new();builder.recursive(true);#[cfg(unix)]{use std::os::unix::fs::DirBuilderExt;builder.mode(0o700);}builder.create(context.work).map_err(files::Error::Io)?;
  registry=ValueGuard(files::object(&context.work.join("estado.json"))?);loaded=true;std::fs::create_dir_all(context.account).map_err(files::Error::Io)?;
  let settings=ValueGuard(files::object(&context.home.join(".claude/settings.json"))?);super::plugin_selection::desired(&settings).map_err(|e|error(e.kind,e.message))?;
  step(context.state,2,"etapa_instrucoes",actions.now());phase(context,actions,"instructions",&mut registry,context.force).await?;
  check(&context.cancel)?;actions.open().await?;
  let result=native_phases(context,actions,&mut registry).await;actions.close().await?;result?;
  context.state.update(|v|v["estado"]=json!(if crate::engines::probe::truthy(&v["erros"]){"parcial"}else{"ok"}));Ok(())
 }.await;
 let interrupted=match result{Ok(())=>None,Err(error) if error.kind=="CancelledError"=>{context.state.update(|v|{v["estado"]=json!("ocioso");v["etapa"]=message("etapa_interrompida",json!({}));});Some(error)},Err(error)=>{
  let expected=matches!(error.kind,"OSError"|"FileNotFoundError"|"PermissionError"|"NotADirectoryError"|"ValueError"|"JSONDecodeError"|"UnicodeDecodeError"|"RuntimeError"|"CodexNativoErro"|"AlteradoExternamente");
  let scrubber=crate::diag_report::Scrubber::new(Some(context.home.to_string_lossy().into_owned()),None);let detail=scrubber.scrub(&error.message);
  if expected{tracing::warn!(code="codex_integration_failed",kind=error.kind,detail=%detail,"Falha da integração");}
  else{tracing::error!(code="codex_integration_failed",kind=error.kind,detail=%detail,"Falha inesperada da integração");}
  context.state.update(|v|{let text=messages::serialize(&v["etapa"]);let item=if expected{message("erro_falha_etapa",json!({"tipo":error.kind,"etapa":text.get("texto").cloned().unwrap_or(json!(""))}))}else{message("erro_falha_inesperada",json!({"tipo":error.kind}))};v["erros"].as_array_mut().expect("Estado inicializado").push(item);v["estado"]=json!("erro");});None
 }};
 context.state.update(|v|v["progresso"]=Value::Null);
 if loaded{
  let last=timestamp::iso(actions.now()).map_err(|e|error(e.kind,e.message))?;context.state.update(|v|{v["ultima_execucao"]=json!(last);if v["estado"]!="ocioso"{v["etapa"]=message(if v["estado"]=="ok"{"etapa_concluido"}else{"etapa_pendencias"},json!({}));}});
  let final_status=view(context,actions.now())?;if let Some(old)=registry.as_object_mut().expect("Registro validado").insert("status".into(),final_status){codec::discard(old);}
  let fingerprint=match actions.fingerprint(true,None,None){Ok(current) if beginning.as_ref()==Some(&current)=>actions.fingerprint(false,None,None).map(Value::String).unwrap_or(Value::Null),_=>Value::Null};
  if let Some(old)=registry.as_object_mut().expect("Registro validado").insert("fingerprint".into(),fingerprint){codec::discard(old);}
  let path=context.work.join("estado.json");let saved=(||->Result<(),files::Error>{files::write(&path,&files::bytes(&registry)?,files::read(&path)?.as_deref(),None)?;Ok(())})();if let Err(failure)=saved{tracing::warn!(code="codex_checkpoint_failed","Não foi possível persistir o estado da integração");context.state.update(|v|v["estado"]=json!("erro"));return Err(failure.into());}
 }
 drop(lock);if let Some(error)=interrupted{return Err(error)}view(context,actions.now())
}
