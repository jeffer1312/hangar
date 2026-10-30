//! Reparo pontual de contas existentes e recarga dos app-servers vivos.
use super::files;
use crate::accounts::catalog::Account;
use serde_json::Value;
use std::{future::Future,path::Path};
pub trait Actions {
    fn repair(&mut self,account:&Account)->impl Future<Output=Result<(),String>>+Send;
    fn sessions(&mut self)->Result<Vec<Value>,String>;
    fn alive(&self,pid:&Value)->bool;
    fn reload(&mut self,endpoint:&str,cwd:&str,account:&Account)->impl Future<Output=Result<bool,String>>+Send;
}
pub async fn run(home:&Path,accounts:&[Account],actions:&mut impl Actions)->Result<(),String>{
    let accounts:Vec<_>=accounts.iter().filter(|account|account.home.is_dir()).collect();
    for account in &accounts{actions.repair(account).await?;}
    for session in actions.sessions()? {
        let session=crate::config_sync::runtime_values::ValueGuard(session);
        let (Some(endpoint),Some(account_home),Some(pid))=(session.get("endpoint").and_then(Value::as_str),session.get("codex_home").and_then(Value::as_str),session.get("app_pid")) else{continue;};
        if !(pid.is_i64()||pid.is_u64()||pid.is_boolean())||!actions.alive(pid){continue;}
        let resolved=super::plugins::resolved(Path::new(account_home))?;
        let mut account=None;
        for candidate in &accounts{if super::plugins::resolved(&candidate.home)?==resolved{account=Some(*candidate);break;}}
        let Some(account)=account else{continue;};
        loopback(endpoint)?;
        let default=home.to_string_lossy();let cwd=session.get("cwd").and_then(Value::as_str).filter(|value|!value.is_empty()).unwrap_or(&default);
        if !actions.reload(endpoint,cwd,account).await?{return Err("O Codex não confirmou a recarga dos hooks reparados".into());}
    }
    let proof=home.join(".hangar/codex-prepare-once-v1.json");
    let previous=files::read(&proof).map_err(|e|e.to_string())?;
    let data=serde_json::json!({"version":1,"accounts":accounts.iter().map(|account|&account.id).collect::<Vec<_>>()});
    files::write(&proof,&files::bytes(&data).map_err(|e|e.to_string())?,previous.as_deref(),None).map_err(|e|e.to_string())?;
    Ok(())
}
fn loopback(endpoint:&str)->Result<(),String>{
    let authority=endpoint.split_once("://").map(|(_,tail)|tail).or_else(||endpoint.strip_prefix("//")).unwrap_or("");
    let authority=authority.split(['/', '?', '#']).next().unwrap_or("").rsplit('@').next().unwrap_or("");
    let host=if let Some(rest)=authority.strip_prefix('['){rest.split_once(']').ok_or("Invalid IPv6 URL")?.0}else{authority.split(':').next().unwrap_or("")};
    let host=host.to_lowercase();
    if host=="localhost"{return Ok(());}
    if host.is_empty(){return Err("O app-server da conta não está em loopback".into());}
    let address=host.parse::<std::net::IpAddr>().map_err(|_|format!("'{host}' does not appear to be an IPv4 or IPv6 address"))?;
    if address.is_loopback(){Ok(())}else{Err("O app-server da conta não está em loopback".into())}
}
