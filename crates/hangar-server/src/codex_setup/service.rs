//! Serviço compartilhado do preparo, com ambiente congelado pelo servidor ou pelo comando local.
use crate::{accounts::{AccountService,AccountKey,Provider,GuardMode,LockError,catalog::Account,preparation::PREPARED_MARKER},config_sync::{runtime::RuntimeConfig,runtime_values::ValueGuard},runtime::gateway::RuntimeRegistry};
use super::{repair::{self,Actions,NativeActions},files,status};
use serde_json::{Value,json};
use std::{path::PathBuf,sync::{Arc,Mutex},time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use tokio::{sync::{oneshot,watch},task::JoinHandle};
#[derive(Default)]
struct Mutations {closed:bool,tasks:Vec<JoinHandle<()>>,completion:Option<watch::Receiver<Option<Result<(),String>>>>}
trait RoundRunner:Send+Sync {
 fn run(self:Arc<Self>,service:Arc<Service>,reason:String,force:bool,cancel:watch::Receiver<bool>)->std::pin::Pin<Box<dyn std::future::Future<Output=Result<(),super::phase::Error>>+Send>>;
}
#[derive(Clone)]
struct Round {forced:bool,result:watch::Receiver<Option<Result<(),String>>>,cancel:watch::Sender<bool>}
pub struct Service {
    pub(super) accounts:AccountService,
    pub(super) config:RuntimeConfig,
    pub(super) repo:PathBuf,
    pub(super) runtime:Option<Arc<RuntimeRegistry>>,
    mutations:Mutex<Mutations>,
    state:super::phase::State,
    rounds:Mutex<Option<Round>>,
    runner:Option<Arc<dyn RoundRunner>>,
    #[cfg(test)]
    before_start:Option<Arc<dyn Fn(&str)+Send+Sync>>,
}
impl Service {
    pub fn new(accounts:AccountService,config:RuntimeConfig,repo:PathBuf,runtime:Option<Arc<RuntimeRegistry>>)->Arc<Self>{
        Arc::new(Self{accounts,config,repo,runtime,mutations:Mutex::new(Mutations::default()),state:super::phase::State::new(status::snapshot()),rounds:Mutex::new(None),runner:None,#[cfg(test)] before_start:None})
    }
    #[cfg(test)]
    fn new_with_runner(accounts:AccountService,config:RuntimeConfig,repo:PathBuf,runtime:Option<Arc<RuntimeRegistry>>,runner:Arc<dyn RoundRunner>)->Arc<Self>{
        let mut service=Self::new(accounts,config,repo,runtime);Arc::get_mut(&mut service).unwrap().runner=Some(runner);service
    }
    pub async fn start(self:&Arc<Self>,reason:&str,force:bool)->Result<Value,String>{self.start_round(reason,force).map(|(status,_)|status)}
    fn start_round(self:&Arc<Self>,reason:&str,force:bool)->Result<(Value,Round),String>{
        #[cfg(test)] if let Some(hook)=&self.before_start{hook(reason);}
        let mut mutations=self.mutations.lock().unwrap_or_else(|e|e.into_inner());
        if mutations.closed{return Err("serviço Codex encerrado".into());}
        let mut rounds=self.rounds.lock().unwrap_or_else(|e|e.into_inner());
        if let Some(round)=rounds.as_ref().filter(|round|round.result.borrow().is_none()){return Ok((self.status()?,round.clone()));}
        match std::fs::remove_file(self.accounts.env.codex_default.join(PREPARED_MARKER)){Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},Err(e)=>return Err(e.to_string())}
        let mut state=self.status()?;state["estado"]=json!("executando");state["etapa"]=super::messages::message("etapa_aguardando",&serde_json::Map::new()).expect("Mensagem versionada");
        self.state.update(|current|crate::orchestration::codec::discard(std::mem::replace(current,state)));
        let initial=self.status()?;
        let(sender,result)=watch::channel(None);let(cancel,signal)=watch::channel(false);
        let selected=Round{forced:force,result,cancel};*rounds=Some(selected.clone());
        let service=self.clone();let reason=reason.to_owned();
        mutations.tasks.retain(|task|!task.is_finished());
        mutations.tasks.push(tokio::spawn(async move{
            let result=if let Some(runner)=service.runner.clone(){runner.run(service.clone(),reason,force,signal).await}else{service.round_owned(force,signal).await};
            if let Err(error)=&result{
                service.state.update(|state|{
                    // Falhas da guarda antecedem o pipeline, mas também encerram a rodada publicada.
                    if state["estado"]!="executando"{return;}
                    let cancelled=error.kind=="CancelledError";
                    state["estado"]=json!(if cancelled{"ocioso"}else{"erro"});
                    state["progresso"]=Value::Null;
                    state["etapa"]=super::messages::message(if cancelled{"etapa_interrompida"}else{"etapa_pendencias"},&serde_json::Map::new()).expect("Mensagem versionada");
                    if !cancelled{state["erros"].as_array_mut().expect("Estado inicializado").push(super::messages::message("erro_falha_inesperada",json!({"tipo":error.kind}).as_object().unwrap()).expect("Mensagem versionada"));}
                });
            }
            sender.send_replace(Some(result.map_err(|error|error.message)));
        }));
        Ok((initial,selected))
    }
    pub async fn update_and_wait(self:&Arc<Self>,reason:&str,force:bool)->Result<Value,String>{
        if force{
            loop{
                // A rodada e seu tipo são escolhidos na mesma trava que agenda o trabalho.
                let (status,round)=self.start_round(reason,true)?;crate::orchestration::codec::discard(status);
                let forced=round.forced;Self::wait_round(round).await?;
                if forced{break;}
            }
        }else{
            if super::sync_enabled(&self.config)&&self.needs_reconcile()?{crate::orchestration::codec::discard(self.start("sessao",false).await?);}
            if let Some(round)=self.current_round().filter(|round|round.result.borrow().is_none()){Self::wait_round(round).await?;}
        }
        self.status()
    }
    fn current_round(&self)->Option<Round>{self.rounds.lock().unwrap_or_else(|e|e.into_inner()).clone()}
    async fn wait_round(mut round:Round)->Result<(),String>{loop{if let Some(result)=round.result.borrow().clone(){return result;}round.result.changed().await.map_err(|_|"rodada Codex interrompida".to_owned())?;}}
    async fn wait_current(&self)->Result<(),String>{if let Some(round)=self.current_round(){Self::wait_round(round).await}else{Ok(())}}
    fn needs_reconcile(&self)->Result<bool,String>{
        if self.current_round().is_some_and(|round|round.result.borrow().is_none()){return Ok(false);}
        let account=&self.accounts.env.codex_default;let work=status::root(&self.accounts.env.home,account).map_err(|e|e.message)?;
        let record=match files::object(&work.join("estado.json")){Ok(value)=>ValueGuard(value),Err(_)=>return Ok(true)};
        let now=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|e|e.to_string())?.as_secs_f64();
        super::reconcile::needed(false,&record,now,||super::pipeline_fingerprint::compute(&self.accounts.env,&self.config,account,&self.repo,None,None,false)).map_err(|e|e.message)
    }
    async fn round_owned(&self,force:bool,cancel:watch::Receiver<bool>)->Result<(),super::phase::Error>{
        use super::phase::Error;
        let account=&self.accounts.env.codex_default;let key=AccountKey::new(Provider::Codex,account).map_err(|e|Error{kind:"AccountError",message:e.to_string()})?;
        let mut signal=cancel.clone();
        let _lifecycle=tokio::select!{biased;_=super::cancellation::stopped(&mut signal)=>return Err(Error{kind:"CancelledError",message:"integração Codex interrompida".into()}),result=self.accounts.locks.acquire(&key,GuardMode::Shared,Instant::now()+Duration::from_secs(5))=>result.map_err(|e|Error{kind:"AccountError",message:match e{LockError::Busy=>"account_busy".into(),LockError::Io(e)=>e.to_string()}})?};
        let work=status::root(&self.accounts.env.home,account).map_err(|e|Error{kind:e.kind,message:e.message})?;let backups=work.join("backups");
        let context=super::pipeline_round::Context{home:&self.accounts.env.home,account,work:&work,state:&self.state,force,memory:super::pipeline_fingerprint::memory(&self.config),cancel:cancel.clone()};
        let mut actions=super::pipeline_native::Actions{accounts:&self.accounts,config:&self.config,repo:&self.repo,account,work:&work,backups:&backups,state:&self.state,runtime:self.runtime.as_ref(),native:None,cancel};
        let result=super::pipeline_round::run(&context,&mut actions).await?;crate::orchestration::codec::discard(result);Ok(())
    }
    pub fn status(&self)->Result<Value,String>{
        let now=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|"relógio do servidor inválido")?.as_secs_f64();
        status::read(&self.accounts.env.home,&self.accounts.env.codex_default,Some(&ValueGuard(self.state.snapshot())),now).map_err(|e|e.message)
    }
    pub async fn repair_account(self:&Arc<Self>,account:Account,adopt:bool)->Result<Option<bool>,String>{
        let actions=NativeActions::new(self.accounts.env.clone(),account.home.clone(),self.repo.clone());
        self.repair_with(account,adopt,actions).await
    }
    pub async fn repair_with<A:Actions+Send+'static>(self:&Arc<Self>,account:Account,adopt:bool,mut actions:A)->Result<Option<bool>,String>{
        let receive={
            let mut mutations=self.mutations.lock().unwrap_or_else(|e|e.into_inner());
            if mutations.closed{return Err("serviço Codex encerrado".into());}
            mutations.tasks.retain(|task|!task.is_finished());
            let(send,receive)=oneshot::channel();let service=self.clone();
            let task=tokio::spawn(async move{
                let result=service.repair_owned(account,adopt,&mut actions).await;
                // A resposta pode sumir; a tarefa conserva as guardas até a última escrita.
                let _=send.send(result);
            });
            mutations.tasks.push(task);
            receive
        };
        receive.await.map_err(|_|"trabalho de reparo Codex interrompido".to_owned())?
    }
    async fn repair_owned(&self,account:Account,adopt:bool,actions:&mut impl Actions)->Result<Option<bool>,String>{
        let key=AccountKey::new(Provider::Codex,&account.home).map_err(|e|e.to_string())?;
        let _lifecycle=self.accounts.locks.acquire(&key,GuardMode::Shared,Instant::now()+Duration::from_secs(5)).await.map_err(|error|match error{
            LockError::Busy=>"account_busy".to_owned(),LockError::Io(error)=>error.to_string(),
        })?;
        let current=self.accounts.resolve(Provider::Codex,&account.id).map_err(|e|e.message)?;
        if super::plugins::resolved(&current.home)?!=super::plugins::resolved(&account.home)?{
            return Err("A conta Codex mudou durante o reparo".into());
        }
        let _integration=files::exclusive(&account.home.join(".hangar-integracao.lock")).await.map_err(|e|e.to_string())?;
        let ready=if account.is_default{
            let now=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|"relógio do servidor inválido")?.as_secs_f64();
            let state=ValueGuard(status::read(&self.accounts.env.home,&account.home,None,now).map_err(|e|e.message)?);
            state["estado"]=="ok"
        }else{
            let state=ValueGuard(self.accounts.stored_status(&account,&key));state["status"]=="ready"
        };
        let pending=repair::repair_locked(&self.accounts.env.home,&account.home,&account.id,actions).await?;
        if adopt&&ready&&pending.is_some(){
            let marker=account.home.join(PREPARED_MARKER);let previous=files::read(&marker).map_err(|e|e.to_string())?;
            files::write(&marker,&files::bytes(&json!({"version":1})).map_err(|e|e.to_string())?,previous.as_deref(),None).map_err(|e|e.to_string())?;
        }
        Ok(pending)
    }
    pub async fn shutdown(&self)->Result<(),String>{
        let mut completion={
            let mut mutations=self.mutations.lock().unwrap_or_else(|e|e.into_inner());
            mutations.closed=true;
            if let Some(round)=self.current_round(){round.cancel.send_replace(true);}
            if let Some(completion)=&mutations.completion { completion.clone() }
            else {
                let tasks=std::mem::take(&mut mutations.tasks);
                let(sender,receiver)=watch::channel::<Option<Result<(),String>>>(None);
                // A conclusão pertence ao serviço; cancelar um aguardante não solta os trabalhadores.
                tokio::spawn(async move{
                    let mut interrupted=false;
                    for task in tasks{interrupted|=task.await.is_err();}
                    sender.send_replace(Some(if interrupted{Err("trabalho de reparo Codex interrompido".into())}else{Ok(())}));
                });
                mutations.completion=Some(receiver.clone());
                receiver
            }
        };
        loop {
            let result=completion.borrow().clone();
            if let Some(result)=result{return result;}
            completion.changed().await.map_err(|_|"encerramento do serviço Codex interrompido".to_owned())?;
        }
    }
}

#[cfg(test)]
#[path="service_round_tests.rs"]
mod round_tests;

#[cfg(test)]
#[path="service_finished_tests.rs"]
mod finished_tests;
