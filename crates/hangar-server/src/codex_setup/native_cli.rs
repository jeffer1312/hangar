//! Comandos administrativos admitidos pertencem ao cliente até terminar a limpeza.
use crate::accounts::environment::AccountEnvironment;
use serde_json::{Value,json};
use std::{path::{Path,PathBuf},sync::{Arc,Mutex},time::Duration};

#[path="native_cli_process.rs"]
mod process;

#[derive(Clone)]
pub struct AdminCli { inner: Arc<Inner> }
struct Inner {
    env: AccountEnvironment,
    home: PathBuf,
    codex: PathBuf,
    state: Mutex<State>,
    cleanup: tokio::sync::Mutex<()>,
    #[cfg(test)]
    observer: Mutex<Option<tokio::sync::mpsc::UnboundedSender<PathBuf>>>,
}
#[derive(Default)]
struct State { closed: bool, operations: Vec<Arc<Operation>> }
struct Operation {
    stop: tokio::sync::watch::Sender<bool>,
    done: tokio::sync::watch::Receiver<Option<Result<(),String>>>,
    directory: Mutex<Option<PathBuf>>,
}
struct CancelOnDrop(tokio::sync::oneshot::Sender<()>);
#[derive(Debug)]
struct Failure { message:String, stderr:String }
impl From<String> for Failure {
    fn from(message:String)->Self { Self {message,stderr:String::new()} }
}

impl AdminCli {
    pub fn new(env: AccountEnvironment, home: &Path, codex: &Path) -> Self {
        Self {inner:Arc::new(Inner {env,home:home.into(),codex:codex.into(),state:Mutex::new(State::default()),cleanup:tokio::sync::Mutex::new(()),
            #[cfg(test)] observer:Mutex::new(None),
        })}
    }
    pub async fn run(&self,args:Vec<String>,timeout:Duration)->Result<Value,String> {
        self.execute(args,timeout,"").await.map_err(|error|error.message)
    }
    pub async fn add(&self,id:&str,timeout:Duration)->Result<Value,String> {
        self.run(vec!["plugin".into(),"add".into(),id.into(),"--json".into()],timeout).await
    }
    pub async fn plugins(&self,timeout:Duration)->Result<Vec<Value>,String> {
        let result = self.run(vec!["plugin".into(),"list".into(),"--json".into()],timeout).await?;
        inventory(result,"installed","O Codex retornou um inventário de plugins inválido.")
    }
    pub async fn marketplaces(&self,timeout:Duration)->Result<Vec<Value>,String> {
        let result = self.run(vec!["plugin".into(),"marketplace".into(),"list".into(),"--json".into()],timeout).await?;
        inventory(result,"marketplaces","O Codex retornou um inventário de marketplaces inválido.")
    }
    pub async fn upgrade(&self,name:&str,timeout:Duration)->Result<Value,String> {
        const BUSY:&str = "auto-upgrade was in flight";
        match self.execute(vec!["plugin".into(),"marketplace".into(),"upgrade".into(),name.into(),"--json".into()],timeout,BUSY).await {
            Err(error) if error.stderr.contains(BUSY) => Ok(json!({"selectedMarketplaces":[name],"upgradedRoots":[],"errors":[]})),
            result => result.map_err(|error|error.message),
        }
    }
    async fn execute(&self,args:Vec<String>,timeout:Duration,expected:&str)->Result<Value,Failure> {
        let (stop, stopped) = tokio::sync::watch::channel(false);
        let (done, completion) = tokio::sync::watch::channel(None);
        let operation = Arc::new(Operation {stop,done:completion,directory:Mutex::new(None)});
        let (reply, result) = tokio::sync::oneshot::channel();
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let _cancel = CancelOnDrop(cancel);
        let expected = expected.to_owned();
        {
            let mut state = self.inner.state.lock().unwrap_or_else(|e|e.into_inner());
            if state.closed { return Err("O cliente nativo do Codex está encerrando.".to_owned().into()); }
            state.operations.push(operation.clone());
            let inner = self.inner.clone();
            // O aguardante só recebe o resultado; a tarefa conserva processo e pasta no cancelamento.
            tokio::spawn(async move {
                let result = process::run(&inner,&operation,args,timeout,&expected,stopped,cancelled).await;
                let cleanup = remove_directory(&operation);
                if cleanup.is_ok() {
                    inner.state.lock().unwrap_or_else(|e|e.into_inner()).operations.retain(|item|!Arc::ptr_eq(item,&operation));
                }
                let result = match (result,&cleanup) {
                    (Ok(_),Err(error))=>Err(error.clone().into()),
                    (result,_)=>result,
                };
                done.send_replace(Some(cleanup));
                let _ = reply.send(result);
            });
        }
        result.await.map_err(|_|Failure::from("Operação administrativa do Codex interrompida.".to_owned()))?
    }
    pub fn begin_close(&self) {
        let mut state = self.inner.state.lock().unwrap_or_else(|e|e.into_inner());
        state.closed = true;
        for operation in &state.operations { operation.stop.send_replace(true); }
    }
    pub fn active_operations(&self)->usize { self.inner.state.lock().unwrap_or_else(|e|e.into_inner()).operations.len() }
    pub async fn close(&self)->Result<(),String> {
        self.begin_close();
        let _cleanup = self.inner.cleanup.lock().await;
        let operations = self.inner.state.lock().unwrap_or_else(|e|e.into_inner()).operations.clone();
        let mut failure = None;
        for operation in operations {
            let mut done = operation.done.clone();
            while done.borrow().is_none() {
                if done.changed().await.is_err() { return Err("Limpeza administrativa do Codex interrompida.".into()); }
            }
            if let Err(error) = remove_directory(&operation) { failure = Some(error); }
            else { self.inner.state.lock().unwrap_or_else(|e|e.into_inner()).operations.retain(|item|!Arc::ptr_eq(item,&operation)); }
        }
        failure.map_or(Ok(()),Err)
    }
}

fn remove_directory(operation:&Operation)->Result<(),String> {
    let mut path = operation.directory.lock().unwrap_or_else(|e|e.into_inner());
    if let Some(directory) = path.as_ref() {
        match std::fs::remove_dir_all(directory) {
            Ok(())=>{},
            Err(error) if error.kind()==std::io::ErrorKind::NotFound=>{},
            Err(_)=>return Err("Não foi possível remover a pasta temporária do Codex.".into()),
        }
        *path = None;
    }
    Ok(())
}

fn inventory(value:Value,key:&str,error:&str)->Result<Vec<Value>,String> {
    let mut value = crate::config_sync::runtime_values::ValueGuard(value);
    if !value[key].as_array().is_some_and(|items|items.iter().all(Value::is_object)) { return Err(error.into()); }
    let Value::Array(items) = value.get_mut(key).unwrap().take() else { unreachable!() };
    Ok(items)
}

#[cfg(all(test,unix))]
#[path="native_cli_tests.rs"]
mod tests;
