//! Reparo da conta sem reinstalação; a cópia inicial precede qualquer alteração de hooks.
use serde_json::{Value, json};
use std::{future::Future, path::{Path, PathBuf}, time::Duration};
use super::{files, native::Native, plugins, trust};
use crate::accounts::environment::AccountEnvironment;

pub trait Actions {
    fn install(&mut self, account: &Path) -> impl Future<Output=Result<(),String>> + Send;
    fn available(&self) -> bool;
    fn plugin(&mut self, root: &Path, backups: &Path) -> impl Future<Output=Result<(),String>> + Send;
    fn open(&mut self) -> impl Future<Output=Result<(),String>> + Send;
    fn request(&mut self, method: &str, params: Value) -> impl Future<Output=Result<Value,String>> + Send;
    fn close(&mut self) -> impl Future<Output=Result<(),String>> + Send;
}

pub async fn repair_locked(home: &Path, account: &Path, id: &str, actions: &mut impl Actions) -> Result<Option<bool>,String> {
    std::fs::create_dir_all(account).map_err(|e|e.to_string())?;
    let private=home.join(".hangar/codex-prepare-once");
    let mut builder=std::fs::DirBuilder::new();
    std::fs::create_dir_all(private.parent().unwrap()).map_err(|e|e.to_string())?;
    #[cfg(unix)] {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    match builder.create(&private) {
        Ok(())=>{},
        Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists && private.is_dir()=>{},
        Err(e)=>return Err(e.to_string()),
    }
    let hooks=account.join("hooks.json");
    let backups=private.join("backups").join(id);
    let initial=backups.join("hooks.json");
    if hooks.is_file() && !initial.exists() {
        let raw=std::fs::read(&hooks).map_err(|e|e.to_string())?;
        files::write(&initial,&raw,None,None).map_err(|e|e.to_string())?;
    }
    actions.install(account).await?;
    if !actions.available() {return Ok(None);}
    let cache=account.join("plugins/cache/ecc/ecc");
    let mut roots=Vec::new();
    match std::fs::read_dir(&cache) {
        Ok(entries)=>for entry in entries {
            let path=entry.map_err(|e|e.to_string())?.path();
            if path.is_dir(){roots.push(path);}
        },
        Err(e) if e.kind()==std::io::ErrorKind::NotFound=>{},
        Err(e)=>return Err(e.to_string()),
    }
    roots.sort();
    for root in roots {actions.plugin(&root,&backups).await?;}
    actions.open().await?;
    let result=async {
        let data=actions.request("hooks/list",json!({"cwds":[]})).await?;
        let (edits,manual)=trust::edits(&data,home,account,false)?;
        if !edits.is_empty(){actions.request("config/batchWrite",json!({"edits":edits,"reloadUserConfig":true})).await?;}
        Ok(Some(manual>0))
    }.await;
    actions.close().await?;
    result
}

pub struct NativeActions {
    env: AccountEnvironment,
    account: PathBuf,
    repo: PathBuf,
    native: Option<Native>,
}
impl NativeActions {
    pub fn new(env:AccountEnvironment,account:PathBuf,repo:PathBuf)->Self {
        Self{env,account,repo,native:None}
    }
}
impl Actions for NativeActions {
    async fn install(&mut self,account:&Path)->Result<(),String> {
        let account=account.to_owned();let repo=self.repo.clone();
        tokio::task::spawn_blocking(move || crate::harness_setup::codex_hooks::sync_checked(&account,&repo,cfg!(windows)))
            .await.map_err(|_|"instalação dos hooks interrompida".to_owned())??;
        Ok(())
    }
    fn available(&self)->bool {crate::cli_probe::resolve_program("codex",&self.env).is_some()}
    async fn plugin(&mut self,root:&Path,backups:&Path)->Result<(),String> {
        let (server,wrapper)=super::hook_command::allow_wrapper()?;
        let context=plugins::Context{account:&self.account,backups,server:&server,windows:cfg!(windows)};
        plugins::adapt(root,&context,|v|Ok(super::normalize::hooks(v,&server,&wrapper,cfg!(windows))),||{},&plugins::NativeRuntime(self.env.clone())).await
    }
    async fn open(&mut self)->Result<(),String> {
        self.native=Some(Native::open(&self.env,&self.env.home,&self.account).await?);Ok(())
    }
    async fn request(&mut self,method:&str,params:Value)->Result<Value,String> {
        self.native.as_ref().ok_or("Codex administrativo não está aberto")?.request(method,params,Duration::from_secs(120)).await
    }
    async fn close(&mut self)->Result<(),String> {
        if let Some(native)=self.native.as_mut(){native.close().await?;}
        self.native=None;Ok(())
    }
}
