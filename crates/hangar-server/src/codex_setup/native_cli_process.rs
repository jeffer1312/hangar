//! Processo próprio, saídas privadas e encerramento antes de devolver a posse.
use super::{Failure, Inner, Operation};
use crate::{accounts::AccountService, terminal_process};
use serde_json::{Value,json};
use std::{io::Write,process::Stdio,time::Duration};
use tokio::sync::{oneshot,watch};

pub(super) async fn run(
    inner:&Inner, operation:&Operation,args:Vec<String>,timeout:Duration,expected:&str,
    mut stopped:watch::Receiver<bool>,mut cancelled:oneshot::Receiver<()>,
)->Result<Value,Failure> {
    let directory = tempfile::Builder::new().prefix("hangar-codex-admin-").tempdir()
        .map_err(|_|Failure::from("Não foi possível criar a pasta temporária do Codex.".to_owned()))?.keep();
    *operation.directory.lock().unwrap_or_else(|e|e.into_inner())=Some(directory.clone());
    let mut command = AccountService::new(inner.env.clone()).codex_command()
        .ok_or_else(||Failure::from("Codex CLI não encontrado; instale ou configure o executável.".to_owned()))?;
    command.args(["-c","project_root_markers=[]"]).args(&args)
        .env_clear().envs(&inner.env.base).env("HOME",&inner.home).env("USERPROFILE",&inner.home)
        .env("CODEX_HOME",&inner.codex).current_dir(&directory)
        .stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    for key in terminal_process::PRIVATE_ENV_KEYS {command.env_remove(key);}
    if inner.home!=inner.env.home {
        for (key,path) in [("XDG_CONFIG_HOME",".config"),("XDG_DATA_HOME",".local/share"),
            ("XDG_STATE_HOME",".local/state"),("XDG_CACHE_HOME",".cache")] {command.env(key,inner.home.join(path));}
    }
    let mut tree = terminal_process::CommandTree::configure(&mut command)
        .map_err(|_|Failure::from("Não foi possível criar a contenção do Codex.".to_owned()))?;
    let mut child = command.spawn().map_err(|_|Failure::from("Não foi possível iniciar o Codex.".to_owned()))?;
    if tree.attach(&child).is_err() {
        let _=child.start_kill(); let _=child.wait().await;
        return Err("Não foi possível colocar o Codex na contenção própria.".to_owned().into());
    }
    let (output,output_done)=read(child.stdout.take().unwrap());
    let (errors,errors_done)=read(child.stderr.take().unwrap());
    let deadline=tokio::time::sleep(timeout);tokio::pin!(deadline);
    let mut timed_out=false;let mut was_cancelled=false;
    let exited = {
        // O líder pode sair antes do filho que ainda escreve nos mesmos pipes.
        let wait=async {
            let exited=terminal_process::leader_exited(&mut child).await;
            let _=output_done.await;
            let _=errors_done.await;
            exited
        };tokio::pin!(wait);
        tokio::select! {
            result=&mut wait=>result,
            _=&mut deadline=>{timed_out=true;Ok(())},
            _=&mut cancelled=>{was_cancelled=true;Ok(())},
            _=async {if !*stopped.borrow(){let _=stopped.changed().await;}}=>Ok(()),
        }
    };
    // O líder continua sem recolher até a árvore estar vazia, para não reutilizar seu grupo.
    let status=super::super::native_shutdown::finish(&mut child,&mut tree).await;
    let read_output=output.await;let read_errors=errors.await;
    if timed_out {return Err("O comando do Codex excedeu o tempo limite.".to_owned().into());}
    if was_cancelled {return Err("Operação administrativa do Codex cancelada.".to_owned().into());}
    if matches!(&read_output,Ok(Err(error)) if error.kind()==std::io::ErrorKind::InvalidData) || matches!(&read_errors,Ok(Err(error)) if error.kind()==std::io::ErrorKind::InvalidData) {return Err("Saída do Codex excedeu o limite de captura.".to_owned().into());}
    let (Ok(()),Ok(Ok(stdout)),Ok(Ok(stderr)))=(exited,read_output,read_errors) else {
        return Err("Não foi possível ler a saída do Codex.".to_owned().into());
    };
    let status=status.map_err(Failure::from)?;
    let code=crate::loops::check::exit_code(status);
    let tail=String::from_utf8_lossy(&stderr).chars().rev().take(500).collect::<Vec<_>>().into_iter().rev().collect::<String>().trim().to_owned();
    let parsed=if stdout.is_empty(){Err(())}else{crate::orchestration::body::parse(&stdout).map_err(|_|())}
        .map(crate::config_sync::runtime_values::ValueGuard);
    let diagnostic=!status.success() && (expected.is_empty() || !tail.contains(expected))
        || status.success() && parsed.as_ref().map_or(true,|value|!value.is_object() || crate::orchestration::truthy(&value["errors"]));
    if diagnostic {
        let env=inner.env.clone();let codex=inner.codex.clone();let args=args.clone();
        let raw_out=stdout.clone();let raw_err=stderr.clone();
        let _=tokio::task::spawn_blocking(move ||diagnose(&env,&codex,&args,code,&raw_out,&raw_err)).await;
    }
    if !status.success() {return Err(Failure {message:format!("O comando do Codex falhou (código {code})."),stderr:tail});}
    let mut value=parsed.map_err(|_|Failure::from("O comando do Codex não retornou JSON válido.".to_owned()))?;
    if !value.is_object() {return Err("O comando do Codex retornou um resultado inválido.".to_owned().into());}
    Ok(value.take())
}

fn read(mut pipe:impl tokio::io::AsyncRead+Unpin+Send+'static)->(tokio::task::JoinHandle<std::io::Result<Vec<u8>>>,oneshot::Receiver<()>) {
    let (sender,done)=oneshot::channel();
    let task=tokio::spawn(async move {
        let mut buffer=Vec::new();
        let result=crate::command_capture::drain_bounded(&mut pipe,&mut buffer,crate::command_capture::MAX_CAPTURE_BYTES).await;
        let _=sender.send(());
        match result {
            Ok(false)=>Ok(buffer),
            Ok(true)=>Err(std::io::Error::new(std::io::ErrorKind::InvalidData,"Saída do Codex excedeu o limite de captura")),
            Err(error)=>Err(error),
        }
    });
    (task,done)
}
fn diagnose(env:&crate::accounts::environment::AccountEnvironment,codex:&std::path::Path,args:&[String],code:i64,stdout:&[u8],stderr:&[u8]) {
    use super::super::files::hash;
    let write=||->std::io::Result<()> {
        let base=crate::runtime::local_policy::log_base_in(env).ok_or_else(||std::io::Error::other("diretório de logs indisponível"))?;
        let account=hash(codex.to_string_lossy().as_bytes());
        let directory=base.join("privado/codex").join(&account[..16]);
        std::fs::create_dir_all(directory.parent().unwrap())?;
        #[cfg(unix)] {use std::os::unix::fs::DirBuilderExt;
            match std::fs::DirBuilder::new().mode(0o700).create(&directory) {Ok(())=>{},Err(e) if e.kind()==std::io::ErrorKind::AlreadyExists=>{},Err(e)=>return Err(e)};}
        #[cfg(not(unix))] std::fs::create_dir_all(&directory)?;
        let key=hash(crate::transcript::pyjson::dumps(&json!(args),false).as_bytes());
        let target=directory.join(format!("cli-{}.log",&key[..16]));
        let mut temporary=tempfile::Builder::new().prefix(".cli-").tempfile_in(&directory)?;
        let payload=json!({"comando":args,"codigo":code,
            "stdout":String::from_utf8_lossy(&stdout[stdout.len().saturating_sub(8192)..]),
            "stderr":String::from_utf8_lossy(&stderr[stderr.len().saturating_sub(8192)..])});
        temporary.write_all(serde_json::to_string(&payload)?.as_bytes())?;
        crate::runtime::queue::replace(temporary.path(),&target)?;
        Ok(())
    };
    match write() {
        Ok(())=>tracing::warn!(code="codex_cli_failed",exit_code=code,"Diagnóstico da CLI gravado no log privado"),
        Err(error)=>tracing::warn!(code="codex_cli_diagnostic_failed",exit_code=code,kind=?error.kind(),"Não foi possível gravar o diagnóstico da CLI"),
    }
}

#[cfg(test)]
#[tokio::test]
async fn administrative_capture_preserves_json_and_drains_overflow() {
 use tokio::io::AsyncWriteExt;
 for size in [8193,crate::command_capture::MAX_CAPTURE_BYTES+17] {
  let (mut writer,reader)=tokio::io::duplex(8192);let (task,done)=read(reader);
  let data=vec![b'x';size];writer.write_all(&data).await.unwrap();drop(writer);done.await.unwrap();let result=task.await.unwrap();
  match &result {
   Ok(buffer)=>assert_eq!(buffer.len(),size),
   Err(error)=>assert_eq!(error.kind(),std::io::ErrorKind::InvalidData),
  }
  assert_eq!(result.is_err(),size>crate::command_capture::MAX_CAPTURE_BYTES);
 }
}
