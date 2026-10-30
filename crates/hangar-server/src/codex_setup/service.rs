//! Serviço compartilhado do preparo, com ambiente congelado pelo servidor ou pelo comando local.
use crate::{accounts::{AccountService,AccountKey,Provider,GuardMode,LockError,catalog::Account,preparation::PREPARED_MARKER},config_sync::{runtime::RuntimeConfig,runtime_values::ValueGuard},runtime::gateway::RuntimeRegistry};
use super::{repair::{self,Actions,NativeActions},files,status};
use serde_json::{Value,json};
use std::{path::PathBuf,sync::{Arc,Mutex},time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
use tokio::{sync::oneshot,task::JoinHandle};
#[derive(Default)]
struct Mutations {closed:bool,tasks:Vec<JoinHandle<()>>}
pub struct Service {
    pub(super) accounts:AccountService,
    pub(super) config:RuntimeConfig,
    pub(super) repo:PathBuf,
    pub(super) runtime:Option<Arc<RuntimeRegistry>>,
    mutations:Mutex<Mutations>,
}
impl Service {
    pub fn new(accounts:AccountService,config:RuntimeConfig,repo:PathBuf,runtime:Option<Arc<RuntimeRegistry>>)->Arc<Self>{
        Arc::new(Self{accounts,config,repo,runtime,mutations:Mutex::new(Mutations::default())})
    }
    pub fn status(&self)->Result<Value,String>{
        let now=SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_|"relógio do servidor inválido")?.as_secs_f64();
        status::read(&self.accounts.env.home,&self.accounts.env.codex_default,None,now).map_err(|e|e.message)
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
        let tasks={let mut mutations=self.mutations.lock().unwrap_or_else(|e|e.into_inner());mutations.closed=true;std::mem::take(&mut mutations.tasks)};
        let mut interrupted=false;
        for task in tasks{interrupted|=task.await.is_err();}
        if interrupted{Err("trabalho de reparo Codex interrompido".into())}else{Ok(())}
    }
}
