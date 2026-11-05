//! O reparo administrativo usa as mesmas guardas e recursos do serviço.
use super::{batch, service::Service};
use crate::{accounts::{catalog::Account, Provider}, orchestration::{body, codec}};
use serde_json::Value;
use std::sync::Arc;
struct Actions(Arc<Service>);
impl batch::Actions for Actions {
    async fn repair(&mut self, account:&Account)->Result<(),String>{self.0.repair_account(account.clone(),true).await.map(|_|())}
    fn sessions(&mut self)->Result<Vec<Value>,String>{
        let Ok(entries)=std::fs::read_dir(self.0.accounts.env.home.join(".hangar/codex-sessions")) else{return Ok(Vec::new())};
        let mut paths=entries.flatten().map(|entry|entry.path()).filter(|path|path.extension().is_some_and(|ext|if cfg!(windows){ext.to_string_lossy().eq_ignore_ascii_case("json")}else{ext=="json"})).collect::<Vec<_>>();paths.sort();
        let mut result=Vec::new();
        for path in paths{
            let Ok(raw)=std::fs::read_to_string(path) else{continue};
            match body::parse(raw.as_bytes()){
                Ok(value) if value.is_object()=>result.push(value),
                Ok(value)=>codec::discard(value),
                Err((_,value))=>codec::discard(value),
            }
        }
        Ok(result)
    }
    fn alive(&self,pid:&Value)->Result<bool,String>{
        let value=pid.as_u64().or_else(||pid.as_bool().map(u64::from));
        if pid.as_i64().is_some_and(|value|value<=0){return Ok(false)}
        let pid=u32::try_from(value.ok_or("PID fora da faixa do sistema")?).map_err(|_|"PID fora da faixa do sistema")?;
        crate::terminal_process::pid_alive(pid).map_err(|error|error.to_string())
    }
    async fn reload(&mut self,endpoint:&str,cwd:&str,account:&Account)->Result<bool,String>{Ok(super::reload::reload(endpoint,cwd,&self.0.accounts.env.home,&account.home).await)}
}
impl Service {
    pub async fn repair_all(self:&Arc<Self>)->Result<(),String>{
        let accounts=self.accounts.snapshot(Provider::Codex).map_err(|error|error.message)?;
        batch::run(&self.accounts.env.home,&accounts,&mut Actions(self.clone())).await
    }
}
