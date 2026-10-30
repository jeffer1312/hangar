//! Fetch e atualização automática do checkout, com os mesmos impedimentos do botão e do trabalho local.
use super::{Service,releases::{self,Source}};use std::time::Duration;
impl Service {
 pub fn auto_reason(&self,source:&dyn Source,platform:Option<&str>,now:chrono::DateTime<chrono::FixedOffset>)->Result<Option<String>,String>{
 super::automatic::reason(&super::automatic::Checkout{service:self,source,platform},now)
 }

 pub fn startup_steps(&self)->Result<(),String>{
  let outcome=self.startup_outcome();
  if let Some(message)=outcome.message(){println!("{message}");}
  Ok(())
 }
 pub fn startup_outcome(&self)->StartupOutcome{prepare_startup(&CheckoutStartup(self),cfg!(windows))}

}
pub fn spawn(st:&crate::routes::AppState){if matches!(crate::update::http::mode(&st.accounts.env.home),crate::update::http::Mode::Package(_)){return;}let service=match Service::from_state(st){Ok(service)=>service,Err(error)=>{tracing::warn!(code="update_auto_config_failed",%error,"atualização periódica sem configuração");return;}};
 let runtime=match crate::config_sync::runtime::RuntimeConfig::from_state(st){Ok(runtime)=>runtime,Err(error)=>{tracing::warn!(code="update_auto_config_failed",%error,"atualização periódica sem preferências");return;}};
 let list=st.list.clone();let port=st.cfg.listen.port();
 let fetch=service.clone();tokio::spawn(async move{loop{let service=fetch.clone();let result=tokio::task::spawn_blocking(move||service.git(&["fetch","origin"],120)).await;match result{Ok(Ok(_))=>{},Ok(Err(error))=>tracing::debug!(%error,"fetch periodico falhou"),Err(error)=>tracing::debug!(%error,"fetch periodico falhou")};tokio::time::sleep(Duration::from_secs(1800)).await;}});
 let live=super::periodic_tick::Live{service,runtime,list,port};
 tokio::spawn(async move{super::periodic_tick::run(&live).await;});
}

/// A tentativa termina antes de liberar as ações; falha de manutenção continua observável.
#[derive(Debug,PartialEq,Eq)]
pub enum StartupOutcome { Package, Deferred, Applied(Vec<String>), Failed(String) }
impl StartupOutcome {
 pub fn message(&self)->Option<String>{match self{
  Self::Package=>None,
  Self::Deferred=>Some("[hangar] passos de atualizacao pendentes que chamam o instalador ficam para o botao Atualizar".into()),
  Self::Applied(ids) if ids.is_empty()=>None,
  Self::Applied(ids)=>Some(format!("[hangar] passos de atualizacao aplicados: {}",ids.join(", "))),
  Self::Failed(error)=>Some(format!("[hangar] AVISO: passo de atualizacao falhou ({error}); o app segue subindo")),
 }}
}
pub trait StartupSource {
 fn package(&self)->Result<bool,String>;
 fn pending_commands(&self)->Result<Vec<String>,String>;
 fn apply(&self)->Result<Vec<String>,String>;
}
pub fn prepare_startup(source:&dyn StartupSource,windows:bool)->StartupOutcome{
 let run=||->Result<StartupOutcome,String>{
  if source.package()?{return Ok(StartupOutcome::Package);}
  if windows&&source.pending_commands()?.iter().any(|command|command.contains("install.ps1")){return Ok(StartupOutcome::Deferred);}
  source.apply().map(StartupOutcome::Applied)
 };
 run().unwrap_or_else(StartupOutcome::Failed)
}
struct CheckoutStartup<'a>(&'a Service);
impl StartupSource for CheckoutStartup<'_>{
 fn package(&self)->Result<bool,String>{Ok(matches!(crate::update::http::mode(&self.0.home),crate::update::http::Mode::Package(_)))}
 fn pending_commands(&self)->Result<Vec<String>,String>{
  let service=self.0;let (steps,_)=crate::update::steps::read_steps(&service.root.join("docs/atualizacoes"),cfg!(windows));
  let done=crate::update::steps::applied(&service.state_dir)?;
  Ok(steps.into_iter().filter(|step|!step.destructive&&!done.contains(&step.id)).map(|step|step.command).collect())
 }
 fn apply(&self)->Result<Vec<String>,String>{
  let service=self.0;let runner=crate::update::ShellRunner{timeout:Duration::from_secs(600)};
  crate::update::steps::apply_checkout_steps(&service.root,&service.home,&service.state_dir,&runner,&service.state(),false).map_err(|error|error.to_string())
 }
}
