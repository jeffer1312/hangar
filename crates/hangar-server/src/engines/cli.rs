use std::{io::Write, process::{Command,ExitCode}};
use crate::accounts::{AccountService,environment::AccountEnvironment};
use super::{environment::{self,Selection},store,settings};

const HELP: &str = "hangar-engine --list | --env <motor> | --exec <motor> [--model <id>] [--context <n>] [--account <id> --account-home <pasta> --account-base-url <url>] [--service-tier default|priority] -- <comando>";

pub fn run(args: &[String]) -> ExitCode {
    let accounts = AccountService::new(AccountEnvironment::capture());
    match execute(args,&accounts) {
        Ok(code) => ExitCode::from(code),
        Err((code,error)) => { eprintln!("hangar-engine: {error}"); ExitCode::from(code) }
    }
}

fn execute(args: &[String], accounts: &AccountService) -> Result<u8,(u8,String)> {
    if args.len() == 1 && matches!(args[0].as_str(),"--help"|"-h") { println!("{HELP}"); return Ok(0); }
    if args == ["--list"] {
        let mut catalog: Vec<_> = store::state(&accounts.env).0.into_iter().collect(); catalog.sort_by(|a,b|a.0.cmp(&b.0));
        for (name,e) in catalog { println!("{name}\t{}\t{}",e["label"].as_str().unwrap_or(&name),e["model"].as_str().unwrap_or("")); }
        return Ok(0);
    }
    if args.len() == 2 && args[0] == "--env" {
        for (key,value) in derive(accounts,&args[1],Selection::default())? { println!("{key}={value}"); }
        return Ok(0);
    }
    if args.first().map(String::as_str) != Some("--exec") || args.len() < 4 { return Err((2,HELP.into())); }
    let cut = args.iter().enumerate().skip(2).find(|(_,a)| *a == "--").map(|(i,_)|i).ok_or((2,HELP.into()))?;
    let mut selected = Selection::default(); let mut tier = None; let mut i = 2;
    while i < cut {
        let name = &args[i];
        if !["--model","--context","--account","--account-home","--account-base-url","--service-tier"].contains(&name.as_str()) { return Err((2,format!("opção desconhecida {name:?}"))); }
        let value = args.get(i+1).filter(|_|i+1<cut).ok_or_else(||(2,format!("{name} precisa de um valor")))?;
        match name.as_str() {
            "--model" => selected.model = Some(value), "--account" => selected.account = Some(value),
            "--account-home" => selected.account_home = Some(value), "--account-base-url" => selected.account_base = Some(value),
            "--context" => selected.context = Some(serde_json::from_str::<serde_json::Value>(value).ok().filter(|v|v.is_i64() || v.is_u64()).ok_or((2,"--context precisa de um número".into()))?),
            "--service-tier" => { if !matches!(value.as_str(),"default"|"priority") { return Err((2,"--service-tier precisa de default ou priority".into())); } tier = Some(value); },
            _ => unreachable!(),
        }
        i += 2;
    }
    if selected.account.is_none() && (selected.account_home.is_some() || selected.account_base.is_some()) { return Err((2,"vínculo de conta sem --account".into())); }
    let fixed = selected.account.is_some();
    let derived = derive(accounts,&args[1],selected)?;
    let mut env = accounts.env.base.clone();
    if fixed && !derived.contains_key("CLAUDE_CODE_MAX_CONTEXT_TOKENS") { env.remove("CLAUDE_CODE_MAX_CONTEXT_TOKENS"); }
    if !fixed { for key in ["CP_ENGINE_ACCOUNT","CP_ENGINE_CREDENTIAL_ID","CP_ENGINE_ACCOUNT_BASE_URL"] { env.remove(key); } }
    env.extend(derived); env.remove("CP_ENGINE_SERVICE_TIER");
    let mut cmd = args[cut+1..].to_vec();
    if cmd.is_empty() { return Err((2,"falta o comando depois de --".into())); }
    let mut temporary = None;
    if let Some(tier) = tier {
        let cwd = std::env::current_dir().map_err(|e|(126,format!("falha ao executar: {e}")))?;
        let (resolved,settings) = settings::resolve(&cmd,&mut env,tier,&cwd,&accounts.env.home).map_err(|e|(2,e))?;
        let mut file = tempfile::Builder::new().prefix("hangar-engine-").suffix(".json").tempfile().map_err(|e|(126,e.to_string()))?;
        file.write_all(settings.to_string().as_bytes()).map_err(|e|(126,e.to_string()))?;
        cmd = vec![resolved[0].clone(),"--settings".into(),file.path().to_string_lossy().into_owned()]; cmd.extend_from_slice(&resolved[1..]); temporary = Some(file);
    }
    let mut command = Command::new(&cmd[0]); command.args(&cmd[1..]).env_clear().envs(env);
    #[cfg(unix)] {
        use std::os::unix::process::CommandExt;
        // O exec mantém o arquivo para as recargas do Claude; falha devolve e limpa o temporário.
        let error = command.exec(); drop(temporary);
        Err((126,format!("falha ao executar: {error}")))
    }
    #[cfg(not(unix))] {
        let result = command.status().map_err(|e|(126,format!("falha ao executar: {e}")))?;
        drop(temporary); Ok(result.code().unwrap_or(1) as u8)
    }
}

fn derive(accounts: &AccountService, name: &str, selection: Selection<'_>) -> Result<std::collections::BTreeMap<String,String>,(u8,String)> {
    if !store::state(&accounts.env).0.contains_key(name) {
        let mut names: Vec<_> = store::state(&accounts.env).0.keys().cloned().collect(); names.sort();
        return Err((1,format!("motor '{name}' não existe (configurados: {})",if names.is_empty() { "nenhum".into() } else { names.join(", ") })));
    }
    environment::derive(accounts,name,selection).map_err(|e|(1,format!("motor '{name}' inválido: {e}")))
}
